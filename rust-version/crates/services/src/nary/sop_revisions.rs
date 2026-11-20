//! Port of backend/app/services/nary/sop_revisions.py: SOP revision drafts (design §7; Nary Phase 3 item E).
//!
//! Opus (job nary.sop_revision) rewrites a published sales SOP for the way the company works with Nary, from the
//! brief in nary.sop_revisions.briefs, keeping the SOP's structure. The draft is an ordinary SOP revision (a draft KB article pointing at the live one), so
//! the existing flow applies unchanged: the Owner edits, signs, and the sop_article approval publishes it.
//! nary.sop_revisions.codes names the SOPs read as house-format reference ("related") and the code of a new SOP
//! Nary may propose ("new", the Nary operating procedure): a draft SOP with no live version. The defaults name
//! none, so nothing is drafted until the Owner sets them.
//!
//! Started from the Nary page (one Celery task per request); never automatic.

use granary_compat::pyjson::{self, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_json, SettingError};
use granary_db::rows::knowledge::KBArticle;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{commit, llm, persona, py_slice, rollback, runs};

pub const WHERE: &str = "Nary & AI → Nary and you on WhatsApp";
pub const CODES_KEY: &str = "nary.sop_revisions.codes";

/// nary.sop_revisions.codes: {"related": [SOP codes], "new": SOP code or ""}.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SopCodes {
    /// SOPs passed to the model as house-format reference; the first one gives a new SOP its category.
    pub related: Vec<String>,
    /// The code of the new SOP Nary may draft (None: no new SOP is proposed).
    pub new: Option<String>,
}

fn codes_error() -> ApiError {
    SettingError::new(
        CODES_KEY,
        "must be {\"related\": [SOP codes], \"new\": SOP code or \"\"}",
        WHERE,
    )
    .into()
}

/// Parse nary.sop_revisions.codes (missing parts read as empty).
pub fn parse_codes(v: &Value) -> ApiResult<SopCodes> {
    let Value::Object(m) = v else {
        return Err(codes_error());
    };
    let related = match m.get("related") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => {
            let mut out = vec![];
            for x in a {
                let Value::String(c) = x else {
                    return Err(codes_error());
                };
                let c = c.trim();
                if !c.is_empty() {
                    out.push(c.to_string());
                }
            }
            out
        }
        Some(_) => return Err(codes_error()),
    };
    let new = match m.get("new") {
        None | Some(Value::Null) => None,
        Some(Value::String(c)) => Some(c.trim().to_string()).filter(|c| !c.is_empty()),
        Some(_) => return Err(codes_error()),
    };
    Ok(SopCodes { related, new })
}

/// `codes(db)`: the configured SOP codes (nary.sop_revisions.codes).
pub async fn codes(db: &mut PgConnection) -> ApiResult<SopCodes> {
    let v = require_json(db, CODES_KEY, WHERE).await?;
    parse_codes(&v)
}

/// `SCHEMA`: the draft the model returns.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {"title": {"type": "string"}, "summary": {"type": "string"}, "content": {"type": "string"},
                       "version_notes": {"type": "string"}, "sop_trigger": {"type": "string"},
                       "sop_owner_role": {"type": "string"}},
        "required": ["title", "summary", "content", "version_notes", "sop_trigger", "sop_owner_role"],
        "additionalProperties": false,
    })
}

pub const RULES: &str = "Revise this sales SOP. Keep its structure (Purpose, Scope, Prerequisites, Procedure with owners,
KPIs, Related SOPs) and its tone. Change only what the brief requires, and everything the brief implies
elsewhere in the text. Facts that must hold:
- Nary is the company's AI sales assistant. It drafts; the Owner approves. Actions earn autonomy one by one
  (levels: shadow, ask, act and tell, silent); anything involving money is never promoted past asking.
- Customer replies, acknowledgements and follow-ups come from Nary, signed \"Nary\" with the company name on WhatsApp and with the company email signature (as Nary) by email.
- Volume tiers, discounts, margins and costs are never mentioned to customers.
- Settings, not the SOP, hold numbers that change (cadence days, thresholds, payment terms); refer to
  \"the setting\" instead of restating a number that lives there.
Write it in English. version_notes: what changed, in one short paragraph.";

/// `str(x)` in an f-string.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        other => pyjson::str_value(&PyValue::from(other)),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `briefs(db)`: nary.sop_revisions.briefs, {SOP code: what to change}.
pub async fn briefs(db: &mut PgConnection) -> ApiResult<Map<String, Value>> {
    match require_json(db, "nary.sop_revisions.briefs", WHERE).await? {
        // {} = no SOP has a revision brief yet (nothing to draft)
        Value::Object(m) => Ok(m),
        _ => Err(SettingError::new(
            "nary.sop_revisions.briefs",
            "must be {SOP code: what to change}",
            WHERE,
        )
        .into()),
    }
}

/// A string field of the model's answer (`data["title"]`).
fn field(data: &Value, k: &str) -> String {
    data.get(k).map(py_str).unwrap_or_default()
}

/// `draft(db, code, user)`: draft one revision (or the new SOP named in nary.sop_revisions.codes). Raises 409 when a revision is already open.
/// Commits.
pub async fn draft(db: &mut PgConnection, code: &str, user: &AuthUser) -> ApiResult<KBArticle> {
    let brief = briefs(db).await?.get(code).cloned().filter(truthy);
    let Some(brief) = brief else {
        return Err(ApiError::http(
            400,
            format!("No brief for {code} (nary.sop_revisions.briefs)"),
        ));
    };
    let lives: Vec<KBArticle> =
        sqlx::query_as("SELECT * FROM kb_articles WHERE sop_code = $1 AND is_deleted = false")
            .bind(code)
            .fetch_all(&mut *db)
            .await?;
    if lives.len() > 1 {
        return Err(ApiError::internal(anyhow::anyhow!(
            "MultipleResultsFound: Multiple rows were found when one or none was required"
        )));
    }
    let live = lives.into_iter().next();
    if let Some(l) = &live {
        if l.status.as_deref() == Some("published")
            && crate::later::nary_sop_revisions::_open_revision(db, l)
                .await?
                .is_some()
        {
            return Err(ApiError::http(
                409,
                format!("{code} already has a revision waiting; finish or delete it first"),
            ));
        }
    }
    let sop_codes = codes(db).await?;
    if live.is_none() && sop_codes.new.as_deref() != Some(code) {
        return Err(ApiError::http(
            404,
            format!("{code} is not in the knowledge base"),
        ));
    }
    let related = _related_text(db, &sop_codes.related, code, 2).await?;
    let mut run = runs::start_run(
        db,
        "manual",
        "nary.sop_revision",
        Some(code),
        "internal",
        Some(code),
    )
    .await?;
    let mut content = format!("SOP: {code}\nBRIEF: {}\n\n", py_str(&brief));
    match &live {
        Some(l) => content.push_str(&format!("CURRENT TEXT:\n{}", l.content.as_deref().unwrap_or("None"))),
        None => content.push_str(
            "This SOP is new. Write it from the brief, in the house format of the related SOPs below.",
        ),
    }
    if !related.is_empty() {
        content.push_str(&format!("\n\nRELATED SOPs (for reference):\n{related}"));
    }
    let system = persona::system_prompt("staff", RULES, "", "");
    let data = llm::ask_json(
        db,
        "nary.sop_revision",
        &system,
        &content,
        &schema(),
        Some(&mut run),
        None,
        None,
    )
    .await?;
    let usable = data
        .as_ref()
        .filter(|d| truthy(d))
        .and_then(|d| d.get("content"))
        .filter(|c| truthy(c))
        .map(|c| !pystr::strip(&py_str(c)).is_empty())
        .unwrap_or(false);
    let Some(data) = data.filter(|_| usable) else {
        runs::finish_run(db, &mut run, "failed", None, Some("no usable draft")).await?;
        commit(db).await?;
        return Err(ApiError::http(
            502,
            format!("Nary couldn't draft {code}. Try again."),
        ));
    };
    // the house style (ai/style.py) already removed em dashes
    let content = field(&data, "content");
    let revision_id: Uuid = match &live {
        Some(l) => {
            crate::later::nary_sop_revisions::_create_revision(db, l, user)
                .await?
                .id
        }
        None => {
            // the new SOP takes the category of the first related SOP (none configured: no category)
            let sales: Vec<(Option<Uuid>,)> = match sop_codes.related.first() {
                Some(first) => {
                    sqlx::query_as("SELECT category_id FROM kb_articles WHERE sop_code = $1")
                        .bind(first)
                        .fetch_all(&mut *db)
                        .await?
                }
                None => vec![],
            };
            if sales.len() > 1 {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "MultipleResultsFound: Multiple rows were found when one or none was required"
                )));
            }
            let category = sales.into_iter().next().and_then(|s| s.0);
            let id = Uuid::new_v4();
            // The model's column defaults (sop_steps [], access_level all_staff, counters 0, ...).
            sqlx::query(
                "INSERT INTO kb_articles (id, slug, article_type, sop_code, status, version, created_by_id, category_id, \
                 tags, title, sop_steps, access_level, is_private, related_article_ids, view_count, helpful_votes, \
                 unhelpful_votes, pdf_import_truncated, is_deleted, created_at, updated_at) \
                 VALUES ($1, $2, 'sop', $3, 'draft', '1.0', $4, $5, $6, $7, '[]'::jsonb, 'all_staff', false, '{}', 0, 0, 0, \
                 false, false, now(), now())",
            )
            .bind(id)
            .bind(format!("{}-nary-operating-procedure", pystr::lower(code)))
            .bind(code)
            .bind(user.id)
            .bind(category)
            .bind(vec!["sales".to_string(), "nary".to_string()])
            .bind(field(&data, "title"))
            .execute(&mut *db)
            .await?;
            id
        }
    };
    let revision: KBArticle = sqlx::query_as(
        "UPDATE kb_articles SET title = $2, summary = $3, content = $4, version_notes = $5, sop_trigger = $6, \
         sop_owner_role = $7, updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(revision_id)
    .bind(py_slice(&field(&data, "title"), 500))
    .bind(field(&data, "summary"))
    .bind(&content)
    .bind(field(&data, "version_notes"))
    .bind(field(&data, "sop_trigger"))
    .bind(py_slice(&field(&data, "sop_owner_role"), 100))
    .fetch_one(&mut *db)
    .await?;
    runs::finish_run(
        db,
        &mut run,
        "done",
        Some(&format!("draft revision of {code}")),
        None,
    )
    .await?;
    commit(db).await?;
    Ok(revision)
}

/// `_related_text(db, related, code, limit=2)`: the related SOPs of nary.sop_revisions.codes (the first
/// `limit` rows), each cut at 6000 characters, without the SOP being revised.
pub async fn _related_text(
    db: &mut PgConnection,
    related: &[String],
    code: &str,
    limit: usize,
) -> ApiResult<String> {
    if related.is_empty() {
        return Ok(String::new());
    }
    let rows: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT sop_code, content FROM kb_articles WHERE sop_code = ANY($1) AND is_deleted = false",
    )
    .bind(related)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows
        .iter()
        .take(limit)
        .filter(|r| r.0.as_deref() != Some(code))
        .map(|r| {
            format!(
                "{}:\n{}",
                r.0.as_deref().unwrap_or("None"),
                py_slice(r.1.as_deref().unwrap_or(""), 6000)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n"))
}

/// `draft_many(db, codes, user)`: {"drafted": [{code, id}], "failed": [{code, error}]}; the person is told
/// what was drafted.
pub async fn draft_many(
    db: &mut PgConnection,
    codes: &[String],
    user: &AuthUser,
) -> ApiResult<Value> {
    let mut done: Vec<(String, String)> = vec![];
    let mut failed: Vec<Value> = vec![];
    for code in codes {
        match draft(db, code, user).await {
            Ok(rev) => done.push((code.clone(), rev.id.to_string())),
            // `except HTTPException` (SettingError is one too); anything else propagates.
            Err(ApiError::Http { detail, .. }) => {
                rollback(db).await?;
                failed.push(json!({"code": code, "error": detail}));
            }
            Err(e) => return Err(e),
        }
    }
    let drafted: Vec<Value> = done
        .iter()
        .map(|(c, i)| json!({"code": c, "id": i}))
        .collect();
    if !done.is_empty() {
        let list = PyValue::List(
            done.iter()
                .map(|(c, i)| {
                    PyValue::Dict(vec![
                        ("code".into(), PyValue::Str(c.clone())),
                        ("id".into(), PyValue::Str(i.clone())),
                    ])
                })
                .collect(),
        );
        let dumped = pyjson::dumps(&list, &pyjson::DumpOpts::default())
            .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
        let title = format!("Nary drafted {} SOP revision(s)", done.len());
        let body = format!("Review, edit and sign them in the knowledge base; the approval publishes them.\n{dumped}");
        crate::approvals::notify::notify(
            db,
            [user.id],
            crate::approvals::notify::Notice::new(
                &title,
                &body,
                "/knowledge/articles?type=sop",
                "nary_sop",
                &done[0].1,
            )
            .type_("nary_done")
            .email(false),
        )
        .await?;
        commit(db).await?;
    }
    Ok(json!({"drafted": drafted, "failed": failed}))
}
