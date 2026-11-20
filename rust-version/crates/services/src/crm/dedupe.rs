//! Port of backend/app/services/crm/dedupe.py: duplicate detection and merge for organizations and contacts.
//!
//! Detection is deterministic: exact email, normalized phone, tax id, then normalized-name similarity. A
//! second, optional pass compares name embeddings to raise what string matching misses ("Acme Corp" vs
//! "The Acme Corporation"); it only ever adds candidates, each labelled as an AI match, and with AI off the
//! deterministic pass runs alone.
//!
//! Merging is never automatic. `merge_preview` shows which field wins and how many related rows move;
//! `merge_records` only runs on an explicit confirmation and records an audit entry.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use once_cell::sync::Lazy;
use regex::Regex;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::matching;
use granary_compat::text::pystr;
use granary_compat::text::utils::normalize_phone;
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_db::rows::crm::{Contact, Organization};
use granary_db::rows::crm_dedupe::CRMDuplicateCandidate;
use granary_db::rows::Vector;

use crate::crm::followups::py_commit;
use crate::crm::writes::{insert_audit, py_round, AuditEntry};

pub const OBJECT_TYPES: [&str; 2] = ["organization", "contact"];

/// A cosine similarity at or above this is worth a human's attention.
pub const EMBED_MIN_SIMILARITY: f64 = 0.88;
/// Neighbours compared per record.
pub const EMBED_NEIGHBOURS: i64 = 5;

/// `DedupeError` (a ValueError the routers turn into a 400), or anything else that went wrong.
#[derive(Debug)]
pub enum DedupeError {
    Invalid(String),
    Api(ApiError),
}

impl DedupeError {
    fn new(msg: impl Into<String>) -> DedupeError {
        DedupeError::Invalid(msg.into())
    }
}

impl std::fmt::Display for DedupeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DedupeError::Invalid(m) => f.write_str(m),
            DedupeError::Api(e) => write!(f, "{e}"),
        }
    }
}

impl From<ApiError> for DedupeError {
    fn from(e: ApiError) -> Self {
        DedupeError::Api(e)
    }
}
impl From<sqlx::Error> for DedupeError {
    fn from(e: sqlx::Error) -> Self {
        DedupeError::Api(e.into())
    }
}
impl From<DedupeError> for ApiError {
    /// What routers/crm_records.py does: `HTTPException(400, str(e))`.
    fn from(e: DedupeError) -> ApiError {
        match e {
            DedupeError::Invalid(m) => ApiError::http(400, m),
            DedupeError::Api(a) => a,
        }
    }
}

pub type Result<T> = std::result::Result<T, DedupeError>;

/// `normalize_name(raw)` (golden-tested in granary_compat).
pub use granary_compat::text::matching::normalize_name;

/// `_similarity(a, b)`: token-set Jaccard.
pub fn _similarity(a: &str, b: &str) -> f64 {
    matching::jaccard(a, b)
}

/// An organization or contact row (`_MODELS[object_type]`).
#[derive(Debug, Clone)]
pub enum Rec {
    Organization(Organization),
    Contact(Contact),
}

impl Rec {
    pub fn id(&self) -> Uuid {
        match self {
            Rec::Organization(o) => o.id,
            Rec::Contact(c) => c.id,
        }
    }
    pub fn email(&self) -> Option<&str> {
        match self {
            Rec::Organization(o) => o.email.as_deref(),
            Rec::Contact(c) => c.email.as_deref(),
        }
    }
    pub fn phone(&self) -> Option<&str> {
        match self {
            Rec::Organization(o) => o.phone.as_deref(),
            Rec::Contact(c) => c.phone.as_deref(),
        }
    }
    fn tax_id(&self) -> Option<&str> {
        match self {
            Rec::Organization(o) => o.tax_id.as_deref(),
            Rec::Contact(_) => None,
        }
    }
    fn website(&self) -> Option<&str> {
        match self {
            Rec::Organization(o) => o.website.as_deref(),
            Rec::Contact(_) => None,
        }
    }
    fn organization_id(&self) -> Option<Uuid> {
        match self {
            Rec::Organization(_) => None,
            Rec::Contact(c) => c.organization_id,
        }
    }
    pub fn is_deleted(&self) -> bool {
        match self {
            Rec::Organization(o) => o.is_deleted,
            Rec::Contact(c) => c.is_deleted,
        }
    }
    pub fn custom(&self) -> &Value {
        match self {
            Rec::Organization(o) => &o.custom,
            Rec::Contact(c) => &c.custom,
        }
    }
    /// `getattr(rec, field, None)` for the merge fields, as the JSON the preview shows.
    pub fn field(&self, name: &str) -> Value {
        fn s(v: &Option<String>) -> Value {
            v.clone().map(Value::String).unwrap_or(Value::Null)
        }
        match self {
            Rec::Organization(o) => match name {
                "name" => Value::String(o.name.clone()),
                "industry" => s(&o.industry),
                "website" => s(&o.website),
                "country" => s(&o.country),
                "city" => s(&o.city),
                "address" => s(&o.address),
                "postal_code" => s(&o.postal_code),
                "phone" => s(&o.phone),
                "email" => s(&o.email),
                "tax_id" => s(&o.tax_id),
                "registration_number" => s(&o.registration_number),
                "employee_count" => o.employee_count.map(Value::from).unwrap_or(Value::Null),
                "description" => s(&o.description),
                "notes" => s(&o.notes),
                _ => Value::Null,
            },
            Rec::Contact(c) => match name {
                "first_name" => Value::String(c.first_name.clone()),
                "last_name" => s(&c.last_name),
                "email" => s(&c.email),
                "phone" => s(&c.phone),
                "whatsapp" => s(&c.whatsapp),
                "position" => s(&c.position),
                "department" => s(&c.department),
                "linkedin_url" => s(&c.linkedin_url),
                "country" => s(&c.country),
                "notes" => s(&c.notes),
                _ => Value::Null,
            },
        }
    }
}

/// `_label(object_type, rec)`.
pub fn _label(object_type: &str, rec: &Rec) -> String {
    match (object_type, rec) {
        ("organization", Rec::Organization(o)) => o.name.clone(),
        (_, Rec::Contact(c)) => pystr::strip(&format!(
            "{} {}",
            c.first_name,
            c.last_name.as_deref().unwrap_or("")
        ))
        .to_string(),
        (_, Rec::Organization(o)) => o.name.clone(),
    }
}

static WEB_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^https?://(www\.)?").unwrap());

fn site(raw: Option<&str>) -> String {
    let lowered = pystr::lower(pystr::strip(raw.unwrap_or("")));
    let s = WEB_PREFIX.replace(&lowered, "").to_string();
    s.trim_end_matches('/').to_string()
}

/// `_compare(object_type, a, b)`: score a pair 0-100 with human-readable reasons.
pub fn _compare(object_type: &str, a: &Rec, b: &Rec) -> (f64, Vec<String>) {
    let mut reasons: Vec<String> = Vec::new();
    let mut score: f64 = 0.0;

    let ae = pystr::lower(pystr::strip(a.email().unwrap_or("")));
    let be = pystr::lower(pystr::strip(b.email().unwrap_or("")));
    let same_email = !ae.is_empty() && ae == be;
    if same_email {
        score = score.max(98.0);
        reasons.push(format!("identical email ({ae})"));
    }

    let ap = normalize_phone(a.phone());
    let bp = normalize_phone(b.phone());
    let same_phone = !ap.is_empty() && ap == bp;
    if same_phone {
        score = score.max(92.0);
        reasons.push(format!("same phone ({ap})"));
    }

    if object_type == "organization" {
        let at = pystr::strip(a.tax_id().unwrap_or("")).to_string();
        let bt = pystr::strip(b.tax_id().unwrap_or("")).to_string();
        if !at.is_empty() && at == bt {
            score = score.max(97.0);
            reasons.push(format!("same tax id ({at})"));
        }
        let aw = site(a.website());
        let bw = site(b.website());
        if !aw.is_empty() && aw == bw {
            score = score.max(90.0);
            reasons.push(format!("same website ({aw})"));
        }
    }

    let na = normalize_name(Some(&_label(object_type, a)));
    let nb = normalize_name(Some(&_label(object_type, b)));
    if !na.is_empty() && na == nb {
        score = score.max(88.0);
        reasons.push(format!("identical name after normalization ('{na}')"));
    } else {
        let sim = _similarity(&na, &nb);
        if sim >= 0.7 {
            score = score.max(60.0 + sim * 25.0);
            reasons.push(format!(
                "similar name ({}% token overlap)",
                (sim * 100.0).trunc() as i64
            ));
        }
    }

    // A contact only counts as a name match within the same organization.
    if object_type == "contact"
        && !same_email
        && !same_phone
        && a.organization_id() != b.organization_id()
    {
        return (0.0, vec![]);
    }

    (py_round(score.min(100.0), 2), reasons)
}

async fn load_records(
    db: &mut PgConnection,
    object_type: &str,
    sql_tail: &str,
    ids: Option<&[Uuid]>,
    limit: Option<i64>,
) -> Result<Vec<Rec>> {
    let table = if object_type == "organization" {
        "organizations"
    } else {
        "contacts"
    };
    let sql = format!("SELECT * FROM {table} {sql_tail}");
    let mut out = Vec::new();
    if object_type == "organization" {
        let mut q = sqlx::query_as::<_, Organization>(&sql);
        if let Some(ids) = ids {
            q = q.bind(ids.to_vec());
        }
        if let Some(l) = limit {
            q = q.bind(l);
        }
        for r in q.fetch_all(&mut *db).await? {
            out.push(Rec::Organization(r));
        }
    } else {
        let mut q = sqlx::query_as::<_, Contact>(&sql);
        if let Some(ids) = ids {
            q = q.bind(ids.to_vec());
        }
        if let Some(l) = limit {
            q = q.bind(l);
        }
        for r in q.fetch_all(&mut *db).await? {
            out.push(Rec::Contact(r));
        }
    }
    Ok(out)
}

async fn load_one(db: &mut PgConnection, object_type: &str, id: Uuid) -> Result<Option<Rec>> {
    Ok(load_records(
        db,
        object_type,
        "WHERE id = ANY($1) LIMIT 1",
        Some(&[id]),
        None,
    )
    .await?
    .into_iter()
    .next())
}

/// `refresh_embeddings(db, object_type, records)`: embed the records whose stored vector is missing or was
/// made from a different name. Returns how many were (re-)embedded.
pub async fn refresh_embeddings(
    db: &mut PgConnection,
    object_type: &str,
    records: &[Rec],
) -> anyhow::Result<i64> {
    let mut labels: Vec<(Uuid, String)> = Vec::new();
    for r in records {
        let text = pystr::strip(&_label(object_type, r)).to_string();
        // dict: a later record with the same id replaces the earlier one in place
        if let Some(slot) = labels.iter_mut().find(|(id, _)| *id == r.id()) {
            slot.1 = text;
        } else {
            labels.push((r.id(), text));
        }
    }
    labels.retain(|(_, t)| !t.is_empty());
    if labels.is_empty() {
        return Ok(0);
    }
    let ids: Vec<Uuid> = labels.iter().map(|(id, _)| *id).collect();
    let stored_rows: Vec<(Uuid, Uuid, String, Option<Vector>)> = sqlx::query_as(
        "SELECT id, record_id, label, embedding FROM crm_embeddings WHERE object_type = $1 AND record_id = ANY($2)",
    )
    .bind(object_type)
    .bind(&ids)
    .fetch_all(&mut *db)
    .await?;
    let mut stored: HashMap<Uuid, (Uuid, String, bool)> = HashMap::new();
    for (id, rid, label, emb) in stored_rows {
        stored.insert(rid, (id, label, emb.is_some()));
    }
    let todo: Vec<(Uuid, String)> = labels
        .iter()
        .filter(|(rid, text)| match stored.get(rid) {
            None => true,
            Some((_, label, has)) => label != text || !has,
        })
        .cloned()
        .collect();
    if todo.is_empty() {
        return Ok(0);
    }
    let texts: Vec<String> = todo.iter().map(|(_, t)| t.clone()).collect();
    // cfg = await get_ai_config(db); provider = get_provider(cfg, "embedding");
    // vectors = await provider.embed(model=get_model(cfg, "embedding"), texts=...)
    let vectors: Vec<Vector> = {
        use crate::ai::registry::{get_ai_config, get_model, get_provider};
        let cfg = get_ai_config(&mut *db)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let provider =
            get_provider(&cfg, "embedding", false).map_err(|e| anyhow::anyhow!("{e}"))?;
        let model = get_model(&cfg, "embedding").map_err(|e| anyhow::anyhow!("{e}"))?;
        provider
            .embed(&model, &texts)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .into_iter()
            .map(|v| Vector(v.into_iter().map(|x| x as f32).collect()))
            .collect()
    };
    for ((rid, text), vec) in todo.iter().zip(vectors) {
        match stored.get(rid) {
            None => {
                sqlx::query(
                    "INSERT INTO crm_embeddings (id, object_type, record_id, label, embedding) VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(Uuid::new_v4())
                .bind(object_type)
                .bind(rid)
                .bind(text)
                .bind(vec)
                .execute(&mut *db)
                .await?;
            }
            Some((id, _, _)) => {
                sqlx::query("UPDATE crm_embeddings SET label = $1, embedding = $2, updated_at = now() WHERE id = $3")
                    .bind(text)
                    .bind(vec)
                    .bind(id)
                    .execute(&mut *db)
                    .await?;
            }
        }
    }
    py_commit(db).await?;
    Ok(todo.len() as i64)
}

/// `embedding_neighbours(db, object_type, records)`: pairs whose names mean the same thing
/// (id_a, id_b, similarity), a < b.
pub async fn embedding_neighbours(
    db: &mut PgConnection,
    object_type: &str,
    records: &[Rec],
) -> anyhow::Result<Vec<(String, String, f64)>> {
    refresh_embeddings(db, object_type, records).await?;
    let ids: Vec<Uuid> = records.iter().map(|r| r.id()).collect();
    let rows: Vec<(Uuid, Option<Vector>)> = sqlx::query_as(
        "SELECT record_id, embedding FROM crm_embeddings WHERE object_type = $1 AND record_id = ANY($2)",
    )
    .bind(object_type)
    .bind(&ids)
    .fetch_all(&mut *db)
    .await?;
    let mut pairs: Vec<((String, String), f64)> = Vec::new();
    for (record_id, embedding) in rows {
        let Some(embedding) = embedding else { continue };
        let near: Vec<(Uuid, f64)> = sqlx::query_as(
            "SELECT record_id, (embedding <=> $1)::float8 AS dist FROM crm_embeddings \
             WHERE object_type = $2 AND record_id != $3 AND record_id = ANY($4) ORDER BY dist LIMIT $5",
        )
        .bind(&embedding)
        .bind(object_type)
        .bind(record_id)
        .bind(&ids)
        .bind(EMBED_NEIGHBOURS)
        .fetch_all(&mut *db)
        .await?;
        for (other_id, distance) in near {
            let similarity = 1.0 - distance;
            if similarity < EMBED_MIN_SIMILARITY {
                break; // ordered by distance: the rest are further still
            }
            let mut key = [
                record_id.hyphenated().to_string(),
                other_id.hyphenated().to_string(),
            ];
            key.sort();
            let key = (key[0].clone(), key[1].clone());
            match pairs.iter_mut().find(|(k, _)| *k == key) {
                Some((_, s)) => *s = s.max(similarity),
                None => pairs.push((key, similarity)),
            }
        }
    }
    Ok(pairs.into_iter().map(|((a, b), s)| (a, b, s)).collect())
}

fn object_type_error() -> DedupeError {
    DedupeError::new(format!(
        "object_type must be one of {}",
        OBJECT_TYPES.join(", ")
    ))
}

/// `scan(db, object_type, threshold=70.0, limit=500)`: compare candidates and record open pairs. Writes
/// only to crm_duplicate_candidates. {"scanned", "new_candidates", "from_embeddings"}.
pub async fn scan(
    db: &mut PgConnection,
    object_type: &str,
    threshold: f64,
    limit: i64,
) -> Result<Value> {
    if !OBJECT_TYPES.contains(&object_type) {
        return Err(object_type_error());
    }
    let rows = load_records(
        db,
        object_type,
        "WHERE is_deleted = false ORDER BY created_at DESC LIMIT $1",
        None,
        Some(limit),
    )
    .await?;

    // Block on the first normalized-name token (plus email and phone) so this stays near-linear.
    let mut buckets: Vec<(String, Vec<usize>)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        let n = normalize_name(Some(&_label(object_type, r)));
        let mut keys: Vec<String> = vec![if n.is_empty() {
            String::new()
        } else {
            pystr::split(&n)[0].to_string()
        }];
        if let Some(email) = r.email().filter(|e| !e.is_empty()) {
            let k = pystr::lower(pystr::strip(email));
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let p = normalize_phone(r.phone());
        if !p.is_empty() && !keys.contains(&p) {
            keys.push(p);
        }
        for k in keys {
            let slot = *index.entry(k.clone()).or_insert_with(|| {
                buckets.push((k, Vec::new()));
                buckets.len() - 1
            });
            buckets[slot].1.push(i);
        }
    }

    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut found: i64 = 0;
    for (_, group) in &buckets {
        if group.len() < 2 || group.len() > 60 {
            continue;
        }
        for (gi, &ai) in group.iter().enumerate() {
            for &bi in &group[gi + 1..] {
                let (a, b) = (&rows[ai], &rows[bi]);
                let pair = sorted_pair(a.id(), b.id());
                if seen.contains(&pair) {
                    continue;
                }
                seen.insert(pair.clone());
                let (score, reasons) = _compare(object_type, a, b);
                if score < threshold {
                    continue;
                }
                found += _upsert_candidate(db, object_type, &pair, score, &reasons, false).await?;
            }
        }
    }

    // Second pass: same company, different name. Additive, never fatal.
    let by_id: HashMap<String, &Rec> = rows
        .iter()
        .map(|r| (r.id().hyphenated().to_string(), r))
        .collect();
    let mut embedded: i64 = 0;
    let pass: anyhow::Result<()> = async {
        for (a_id, b_id, similarity) in embedding_neighbours(db, object_type, &rows).await? {
            let pair = (a_id.clone(), b_id.clone());
            let (Some(a), Some(b)) = (by_id.get(&a_id), by_id.get(&b_id)) else {
                continue;
            };
            if object_type == "contact" && a.organization_id() != b.organization_id() {
                continue; // two companies can each employ a John Doe
            }
            let score = py_round(similarity * 100.0, 2);
            if score < threshold {
                continue;
            }
            let reason = format!(
                "names mean the same thing ({}% similar, AI)",
                (similarity * 100.0).trunc() as i64
            );
            if seen.contains(&pair) {
                _upsert_candidate(db, object_type, &pair, score, &[reason], true)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                continue;
            }
            seen.insert(pair.clone());
            let new_rows = _upsert_candidate(db, object_type, &pair, score, &[reason], false)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            found += new_rows;
            embedded += new_rows;
        }
        Ok(())
    }
    .await;
    if let Err(exc) = pass {
        let msg: String = exc.to_string().chars().take(200).collect();
        tracing::info!(object_type, error = %msg, "dedupe_embedding_pass_skipped");
    }

    py_commit(db).await?;
    Ok(json!({"scanned": rows.len(), "new_candidates": found, "from_embeddings": embedded}))
}

fn sorted_pair(a: Uuid, b: Uuid) -> (String, String) {
    let (a, b) = (a.hyphenated().to_string(), b.hyphenated().to_string());
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// `Decimal(stored) == score` as Python compares a Decimal with a float (exactly).
fn decimal_equals_float(stored: Decimal, score: f64) -> bool {
    // A 2-place decimal equals a binary float only when it is a multiple of 1/4.
    let quarters = stored * Decimal::from(4);
    if quarters.fract() != Decimal::ZERO {
        return false;
    }
    stored.to_f64() == Some(score)
}

/// `_upsert_candidate(...)`: record an open pair. 1 when it is new, 0 when it already existed.
async fn _upsert_candidate(
    db: &mut PgConnection,
    object_type: &str,
    pair: &(String, String),
    score: f64,
    reasons: &[String],
    merge: bool,
) -> Result<i64> {
    let a = Uuid::parse_str(&pair.0).map_err(|e| ApiError::internal(anyhow::anyhow!(e)))?;
    let b = Uuid::parse_str(&pair.1).map_err(|e| ApiError::internal(anyhow::anyhow!(e)))?;
    let exists: Option<CRMDuplicateCandidate> = sqlx::query_as(
        "SELECT * FROM crm_duplicate_candidates WHERE object_type = $1 AND record_a_id = $2 AND record_b_id = $3 LIMIT 1",
    )
    .bind(object_type)
    .bind(a)
    .bind(b)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(exists) = exists {
        if exists.status == "open" {
            let current: Vec<Value> = match &exists.reasons {
                Some(Value::Array(v)) => v.clone(),
                _ => vec![],
            };
            let (new_reasons, new_score): (Vec<Value>, f64) = if merge {
                let mut out = current.clone();
                for r in reasons {
                    let v = Value::String(r.clone());
                    if !current.contains(&v) {
                        out.push(v);
                    }
                }
                (out, exists.score.to_f64().unwrap_or(0.0).max(score))
            } else {
                (reasons.iter().cloned().map(Value::String).collect(), score)
            };
            // The ORM writes only what changed (the stored Decimal against the new float, compared exactly).
            let reasons_changed =
                exists.reasons.as_ref() != Some(&Value::Array(new_reasons.clone()));
            let score_changed = !decimal_equals_float(exists.score, new_score);
            if reasons_changed || score_changed {
                sqlx::query(
                    "UPDATE crm_duplicate_candidates SET score = $1::text::numeric, reasons = $2, updated_at = now() WHERE id = $3",
                )
                .bind(granary_compat::pyjson::py_float_repr(new_score))
                .bind(Value::Array(new_reasons))
                .bind(exists.id)
                .execute(&mut *db)
                .await?;
            }
        }
        return Ok(0);
    }
    sqlx::query(
        "INSERT INTO crm_duplicate_candidates (id, object_type, record_a_id, record_b_id, score, reasons, status) \
         VALUES ($1, $2, $3, $4, $5::text::numeric, $6, 'open')",
    )
    .bind(Uuid::new_v4())
    .bind(object_type)
    .bind(a)
    .bind(b)
    .bind(granary_compat::pyjson::py_float_repr(score))
    .bind(Value::Array(reasons.iter().cloned().map(Value::String).collect()))
    .execute(&mut *db)
    .await?;
    Ok(1)
}

/// `list_candidates(db, object_type=None, status="open", limit=50)`.
pub async fn list_candidates(
    db: &mut PgConnection,
    object_type: Option<&str>,
    status: &str,
    limit: i64,
) -> Result<Vec<Value>> {
    let rows: Vec<CRMDuplicateCandidate> = match object_type.filter(|o| !o.is_empty()) {
        Some(ot) => sqlx::query_as(
            "SELECT * FROM crm_duplicate_candidates WHERE status = $1 AND object_type = $2 ORDER BY score DESC LIMIT $3",
        )
        .bind(status)
        .bind(ot)
        .bind(limit)
        .fetch_all(&mut *db)
        .await?,
        None => sqlx::query_as("SELECT * FROM crm_duplicate_candidates WHERE status = $1 ORDER BY score DESC LIMIT $2")
            .bind(status)
            .bind(limit)
            .fetch_all(&mut *db)
            .await?,
    };
    let mut out = Vec::new();
    for c in rows {
        if !OBJECT_TYPES.contains(&c.object_type.as_str()) {
            // `_MODELS[c.object_type]`: a KeyError in Python.
            return Err(
                ApiError::internal(anyhow::anyhow!("KeyError: '{}'", c.object_type)).into(),
            );
        }
        let a = load_one(db, &c.object_type, c.record_a_id).await?;
        let b = load_one(db, &c.object_type, c.record_b_id).await?;
        let (Some(a), Some(b)) = (a, b) else { continue };
        let side = |r: &Rec| {
            json!({"id": r.id().hyphenated().to_string(), "label": _label(&c.object_type, r),
                   "email": r.email(), "phone": r.phone()})
        };
        out.push(json!({
            "id": c.id.hyphenated().to_string(),
            "object_type": c.object_type,
            "score": c.score.to_f64().unwrap_or(0.0),
            "reasons": c.reasons.clone().filter(|r| !py_falsy(r)).unwrap_or_else(|| json!([])),
            "a": side(&a),
            "b": side(&b),
        }));
    }
    Ok(out)
}

/// Python truthiness of a JSON value (`x or default`).
fn py_falsy(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// `_RELATED[object_type]`: (table, fk column, label).
pub fn related(object_type: &str) -> &'static [(&'static str, &'static str, &'static str)] {
    if object_type == "organization" {
        &[
            ("contacts", "organization_id", "contacts"),
            ("leads", "organization_id", "leads"),
            ("deals", "organization_id", "deals"),
            ("quotations", "customer_org_id", "quotations"),
            ("invoices", "customer_org_id", "invoices"),
            ("crm_email_threads", "linked_org_id", "email threads"),
            // batch 2 T2: every other table with a foreign key to organizations (A2), so nothing stays on
            // the merged record (tests/nary_fix2_hygiene.rs checks the list against the schema)
            ("projects", "customer_org_id", "projects"),
            ("sales_documents", "organization_id", "sales documents"),
            ("contracts", "customer_org_id", "contracts"),
            (
                "crm_research_people",
                "organization_id",
                "researched people",
            ),
            ("partner_accounts", "organization_id", "partner accounts"),
            (
                "partner_deal_registrations",
                "customer_org_id",
                "partner deal registrations",
            ),
            ("production_orders", "customer_org_id", "production orders"),
            ("rma_cases", "customer_org_id", "RMA cases"),
            ("warranty_records", "customer_org_id", "warranty records"),
            ("vendors", "organization_id", "vendor records"),
        ]
    } else {
        &[
            ("leads", "contact_id", "leads"),
            ("deals", "contact_id", "deals"),
            ("activities", "contact_id", "activities"),
            ("quotations", "customer_contact_id", "quotations"),
            ("invoices", "customer_contact_id", "invoices"),
            ("crm_email_threads", "linked_contact_id", "email threads"),
        ]
    }
}

/// batch 2 T2: rows that point at a record by type and id (no foreign key): (table, type column, id column,
/// type value, label). Moved by a merge like `related` (activities, files, reminders, Nary's notes, research).
/// The id column is a uuid except where `typed_id_cast` says it is text.
pub fn related_typed(
    object_type: &str,
) -> &'static [(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)] {
    if object_type == "organization" {
        &[
            (
                "activities",
                "entity_type",
                "entity_id",
                "organization",
                "activities",
            ),
            (
                "attachments",
                "entity_type",
                "entity_id",
                "crm_org",
                "files",
            ),
            (
                "crm_reminders",
                "entity_type",
                "entity_id",
                "organization",
                "reminders",
            ),
            (
                "nary_memory",
                "subject_type",
                "subject_id",
                "organization",
                "Nary memory notes",
            ),
            (
                "crm_research_runs",
                "object_type",
                "record_id",
                "organization",
                "research runs",
            ),
            (
                "crm_ai_field_runs",
                "object_type",
                "record_id",
                "organization",
                "enrichment runs",
            ),
        ]
    } else {
        &[]
    }
}

/// batch 2 T2: the typed references whose id column is text (attachments.entity_id, nary_memory.subject_id).
fn typed_id_cast(table: &str) -> &'static str {
    match table {
        "attachments" | "nary_memory" => "::text",
        _ => "",
    }
}

/// batch 2 T2: a related table with a unique key next to the foreign key: a row whose twin is already on the
/// kept record stays where it is (e.g. the same researched person found for both copies of a company).
fn unique_with(table: &str) -> Option<&'static str> {
    match table {
        "crm_research_people" => Some("name_key"),
        _ => None,
    }
}

/// batch 2 T2: the rows of one related table that a merge moves ($1 = the merged record, $2 = the kept one
/// when the table has a unique key).
fn movable_where(table: &str, fk: &str) -> String {
    match unique_with(table) {
        Some(col) => format!(
            "{fk} = $1 AND NOT EXISTS (SELECT 1 FROM {table} k WHERE k.{fk} = $2 AND k.{col} = {table}.{col})"
        ),
        None => format!("{fk} = $1"),
    }
}

/// batch 2 T2: what a merge would move off `merge_id`, per table ({"what", "count"}, non-zero only).
pub async fn move_counts(
    db: &mut PgConnection,
    object_type: &str,
    keep_id: Uuid,
    merge_id: Uuid,
) -> Result<Vec<Value>> {
    let mut moves = Vec::new();
    for (table, fk, label) in related(object_type) {
        let sql = format!(
            "SELECT count(*) FROM {table} WHERE {}",
            movable_where(table, fk)
        );
        let mut q = sqlx::query_as::<_, (i64,)>(&sql).bind(merge_id);
        if unique_with(table).is_some() {
            q = q.bind(keep_id);
        }
        let (n,) = q.fetch_one(&mut *db).await?;
        if n != 0 {
            moves.push(json!({"what": label, "count": n}));
        }
    }
    for (table, tcol, icol, tval, label) in related_typed(object_type) {
        let (n,): (i64,) = sqlx::query_as(&format!(
            "SELECT count(*) FROM {table} WHERE {tcol} = $2 AND {icol} = $1{}",
            typed_id_cast(table)
        ))
        .bind(merge_id)
        .bind(*tval)
        .fetch_one(&mut *db)
        .await?;
        if n != 0 {
            moves.push(json!({"what": label, "count": n}));
        }
    }
    Ok(moves)
}

/// `_MERGE_FIELDS[object_type]`.
pub fn merge_fields(object_type: &str) -> &'static [&'static str] {
    if object_type == "organization" {
        &[
            "name",
            "industry",
            "website",
            "country",
            "city",
            "address",
            "postal_code",
            "phone",
            "email",
            "tax_id",
            "registration_number",
            "employee_count",
            "description",
            "notes",
        ]
    } else {
        &[
            "first_name",
            "last_name",
            "email",
            "phone",
            "whatsapp",
            "position",
            "department",
            "linkedin_url",
            "country",
            "notes",
        ]
    }
}

/// `_merge_pair(db, object_type, keep_id, merge_id, lock=...)`: both live records of a merge.
async fn _merge_pair(
    db: &mut PgConnection,
    object_type: &str,
    keep_id: Uuid,
    merge_id: Uuid,
    lock: bool,
) -> Result<(Rec, Rec)> {
    if !OBJECT_TYPES.contains(&object_type) {
        return Err(object_type_error());
    }
    if keep_id == merge_id {
        return Err(DedupeError::new("Cannot merge a record into itself."));
    }
    let tail = if lock {
        "WHERE id = ANY($1) ORDER BY id FOR UPDATE"
    } else {
        "WHERE id = ANY($1) ORDER BY id"
    };
    let rows = load_records(db, object_type, tail, Some(&[keep_id, merge_id]), None).await?;
    let keep = rows.iter().find(|r| r.id() == keep_id).cloned();
    let merge = rows.iter().find(|r| r.id() == merge_id).cloned();
    let (Some(keep), Some(merge)) = (keep, merge) else {
        return Err(DedupeError::new("One of the records no longer exists."));
    };
    for rec in [&keep, &merge] {
        if rec.is_deleted() {
            return Err(DedupeError::new(format!(
                "{} was already merged or deleted.",
                _label(object_type, rec)
            )));
        }
    }
    Ok((keep, merge))
}

fn blank(v: &Value) -> bool {
    matches!(v, Value::Null) || v.as_str() == Some("")
}

/// The pure part of `merge_preview`: the field table (which side wins) and `custom_after`.
pub fn preview_fields(object_type: &str, keep: &Rec, merge: &Rec) -> (Vec<Value>, Value) {
    let mut fields = Vec::new();
    for f in merge_fields(object_type) {
        let (kv, mv) = (keep.field(f), merge.field(f));
        // The surviving record wins; the loser only fills fields it left blank.
        let taken = if !blank(&kv) {
            "keep"
        } else if !blank(&mv) {
            "merge"
        } else {
            "empty"
        };
        let result = if taken == "keep" {
            kv.clone()
        } else {
            mv.clone()
        };
        fields.push(
            json!({"field": f, "keep": kv, "merge": mv, "result": result, "taken_from": taken}),
        );
    }
    let mut merged: Map<String, Value> = match merge.custom() {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    if let Value::Object(k) = keep.custom() {
        for (key, v) in k {
            merged.insert(key.clone(), v.clone());
        }
    }
    (fields, Value::Object(merged))
}

async fn preview_with(
    db: &mut PgConnection,
    object_type: &str,
    keep: &Rec,
    merge: &Rec,
    merge_id: Uuid,
) -> Result<Value> {
    let (fields, custom_after) = preview_fields(object_type, keep, merge);
    let moves = move_counts(db, object_type, keep.id(), merge_id).await?;
    Ok(json!({
        "object_type": object_type,
        "keep": {"id": keep.id().hyphenated().to_string(), "label": _label(object_type, keep)},
        "merge": {"id": merge.id().hyphenated().to_string(), "label": _label(object_type, merge)},
        "fields": fields,
        "moves": moves,
        "custom_after": custom_after,
    }))
}

/// `merge_preview(db, object_type, keep_id, merge_id)`: exactly what a merge would do.
pub async fn merge_preview(
    db: &mut PgConnection,
    object_type: &str,
    keep_id: Uuid,
    merge_id: Uuid,
) -> Result<Value> {
    let (keep, merge) = _merge_pair(db, object_type, keep_id, merge_id, false).await?;
    preview_with(db, object_type, &keep, &merge, merge_id).await
}

/// `merge_records(db, user, object_type, keep_id, merge_id, candidate_id=None)`: apply a merge.
/// Irreversible; only ever called from an explicit confirmation.
pub async fn merge_records(
    db: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    keep_id: Uuid,
    merge_id: Uuid,
    candidate_id: Option<Uuid>,
) -> Result<Value> {
    // Lock both rows: two people confirming the same merge must not both move the related rows.
    let (keep, merge) = _merge_pair(db, object_type, keep_id, merge_id, true).await?;
    let preview = preview_with(db, object_type, &keep, &merge, merge_id).await?;
    let table = if object_type == "organization" {
        "organizations"
    } else {
        "contacts"
    };

    // keep: the fields the loser fills, and the merged custom values (written only when something changed)
    let mut sets: Vec<String> = Vec::new();
    let mut texts: Vec<Option<String>> = Vec::new();
    let mut employee_count: Option<i64> = None;
    let mut changed_fields: Vec<String> = Vec::new();
    for row in preview["fields"].as_array().into_iter().flatten() {
        if row["taken_from"] != "merge" {
            continue;
        }
        let field = row["field"].as_str().unwrap_or_default().to_string();
        changed_fields.push(field.clone());
        if field == "employee_count" {
            employee_count = row["merge"].as_i64();
            sets.push("employee_count = $2".into());
        } else {
            texts.push(row["merge"].as_str().map(str::to_string));
            sets.push(format!("{field} = ${}", texts.len() + 3));
        }
    }
    let custom_after = preview["custom_after"].clone();
    let custom_changed = &custom_after != keep.custom();
    if custom_changed {
        sets.push("custom = $3".into());
    }
    if !sets.is_empty() {
        let sql = format!(
            "UPDATE {table} SET {}, updated_at = now() WHERE id = $1",
            sets.join(", ")
        );
        let mut q = sqlx::query(&sql)
            .bind(keep_id)
            .bind(employee_count.map(|n| n as i32))
            .bind(&custom_after);
        for t in &texts {
            q = q.bind(t);
        }
        q.execute(&mut *db).await?;
    }

    let mut moved: i64 = 0;
    for (rel, fk, _) in related(object_type) {
        // $1 = the merged record, $2 = the kept one (see movable_where)
        let res = sqlx::query(&format!(
            "UPDATE {rel} SET {fk} = $2, updated_at = now() WHERE {}",
            movable_where(rel, fk)
        ))
        .bind(merge_id)
        .bind(keep_id)
        .execute(&mut *db)
        .await?;
        moved += res.rows_affected() as i64;
    }
    // batch 2 T2: the rows that point at it by type and id, and its other open duplicate pairs
    for (rel, tcol, icol, tval, _) in related_typed(object_type) {
        let res = sqlx::query(&format!(
            "UPDATE {rel} SET {icol} = $1{c}, updated_at = now() WHERE {tcol} = $3 AND {icol} = $2{c}",
            c = typed_id_cast(rel)
        ))
        .bind(keep_id)
        .bind(merge_id)
        .bind(*tval)
        .execute(&mut *db)
        .await?;
        moved += res.rows_affected() as i64;
    }
    if object_type == "organization" {
        sqlx::query(
            "UPDATE crm_duplicate_candidates SET status = 'merged', resolved_by_id = $1, resolved_at = now(), \
             merged_into_id = $2, updated_at = now() WHERE object_type = 'organization' AND status = 'open' \
             AND (record_a_id = $3 OR record_b_id = $3) AND ($4::uuid IS NULL OR id <> $4)",
        )
        .bind(user.id)
        .bind(keep_id)
        .bind(merge_id)
        .bind(candidate_id)
        .execute(&mut *db)
        .await?;
    }

    sqlx::query(&format!(
        "UPDATE {table} SET is_deleted = true, deleted_at = $1, updated_at = now() WHERE id = $2"
    ))
    .bind(Utc::now())
    .bind(merge_id)
    .execute(&mut *db)
    .await?;

    if let Some(cid) = candidate_id {
        sqlx::query(
            "UPDATE crm_duplicate_candidates SET status = 'merged', resolved_by_id = $1, resolved_at = $2, \
             merged_into_id = $3, updated_at = now() WHERE id = $4",
        )
        .bind(user.id)
        .bind(Utc::now())
        .bind(keep_id)
        .bind(cid)
        .execute(&mut *db)
        .await?;
    }

    let keep_label = preview["keep"]["label"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let merge_label = preview["merge"]["label"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    insert_audit(
        db,
        AuditEntry {
            user_id: Some(user.id),
            user_name: Some(user.name.clone()),
            action: "merge".into(),
            entity_type: object_type.into(),
            entity_id: Some(keep_id.hyphenated().to_string()),
            entity_label: Some(keep_label.chars().take(200).collect()),
            old_values: json!({"merged_record": merge_id.hyphenated().to_string(), "label": merge_label}),
            new_values: json!({"moved_related_rows": moved}),
            changed_fields: if changed_fields.is_empty() { None } else { Some(changed_fields) },
            origin: "ui".into(),
            notes: Some(format!("Merged {merge_label} into {keep_label}")),
        },
    )
    .await?;
    py_commit(db).await?;
    Ok(
        json!({"ok": true, "kept_id": keep_id.hyphenated().to_string(),
              "merged_id": merge_id.hyphenated().to_string(), "moved_rows": moved}),
    )
}

/// `dismiss(db, user, candidate_id)`.
pub async fn dismiss(db: &mut PgConnection, user: &AuthUser, candidate_id: Uuid) -> Result<Value> {
    let res = sqlx::query(
        "UPDATE crm_duplicate_candidates SET status = 'dismissed', resolved_by_id = $1, resolved_at = $2, \
         updated_at = now() WHERE id = $3",
    )
    .bind(user.id)
    .bind(Utc::now())
    .bind(candidate_id)
    .execute(&mut *db)
    .await?;
    if res.rows_affected() == 0 {
        return Err(DedupeError::new("Candidate not found."));
    }
    py_commit(db).await?;
    Ok(json!({"ok": true}))
}
