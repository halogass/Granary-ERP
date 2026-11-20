//! Nary fix batch 2, agent T2 (Owner review 14-19 Oct, items A2, A4, A9): CRM hygiene from the chat.
//!
//! ```text
//!     find_duplicate_organizations   read: companies that are one company (normalised name + dedupe pairs)
//!     propose_organization_merge     card organization.merge: keep one, merge one or more into it
//!     list_nary_cards                read (Owner / Nary admin): Nary's cards and what was decided on them
//!     propose_contact_delete         card contact.delete (Owner only): the CRM's soft delete of a contact
//! ```
//!
//! The merge is the CRM > Duplicates merge (crm/dedupe.rs merge_records), once per merged company; it moves
//! everything that points at an organization (every foreign key plus activities, files, reminders, Nary's
//! notes and research, see dedupe::related / related_typed). The delete is DELETE /contacts/{id}: the row
//! is marked deleted, nothing else changes.

use std::collections::{HashMap, HashSet};

use chrono::{Duration, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::utils::py_uuid;
use granary_core::auth::AuthUser;
use granary_core::clock;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::crm::{Contact, Organization};
use granary_db::rows::nary::AgentPendingAction;

use super::actions::{apply_fn, entity_fn, need_actor, to_uuid, Action, Field, Schema, Ty};
use crate::crm::dedupe::{self, DedupeError};

// ─── small helpers ────────────────────────────────────────────────────────────

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

/// A trimmed, non-empty text argument.
fn text(args: &Value, k: &str) -> Option<String> {
    match args.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn flag(args: &Value, k: &str) -> bool {
    match args.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1"),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => false,
    }
}

fn int(args: &Value, k: &str, default: i64, min: i64, max: i64) -> i64 {
    let v = match args.get(k) {
        Some(Value::Number(n)) => n.as_f64().map(|f| f as i64),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    v.unwrap_or(default).clamp(min, max)
}

fn dedupe_msg(e: DedupeError) -> String {
    match e {
        DedupeError::Invalid(m) => m,
        DedupeError::Api(a) => api_msg(&a),
    }
}

fn api_msg(e: &ApiError) -> String {
    match e {
        ApiError::Http {
            detail: Value::String(s),
            ..
        } => s.clone(),
        ApiError::Http { detail, .. } => detail.to_string(),
        ApiError::App { detail, .. } => detail.clone(),
        other => crate::ai::tools::catalog::exc_str(other),
    }
}

fn opt(v: &Option<String>) -> String {
    v.clone().unwrap_or_default().trim().to_string()
}

async fn local(db: &mut PgConnection, t: chrono::DateTime<Utc>) -> ApiResult<String> {
    let tz = clock::local_tz(db).await?;
    Ok(t.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string())
}

/// Is this person Nary's approver (the Owner) or the superadmin.
async fn is_owner(db: &mut PgConnection, user: &AuthUser) -> ApiResult<bool> {
    crate::ai::mcp_access::is_approver(db, user).await
}

// ─── A2: duplicate organizations ──────────────────────────────────────────────

/// The name two copies of one company share: dedupe's normalize_name (lower case, punctuation gone, Ltd /
/// Inc / LLC and the other legal words dropped) with the spaces removed too ("Global Tech" = "GlobalTech").
pub fn org_key(name: &str) -> String {
    dedupe::normalize_name(Some(name)).replace(' ', "")
}

async fn load_orgs(db: &mut PgConnection, ids: &[Uuid]) -> ApiResult<Vec<Organization>> {
    Ok(
        sqlx::query_as("SELECT * FROM organizations WHERE id = ANY($1)")
            .bind(ids.to_vec())
            .fetch_all(&mut *db)
            .await?,
    )
}

fn moves_text(moves: &[Value]) -> String {
    if moves.is_empty() {
        return "nothing linked".into();
    }
    moves
        .iter()
        .map(|m| {
            format!(
                "{} {}",
                m["count"].as_i64().unwrap_or(0),
                m["what"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

const COMPARED: [&str; 9] = [
    "name", "email", "phone", "website", "city", "address", "tax_id", "industry", "country",
];

fn org_field(o: &Organization, f: &str) -> String {
    let v = match f {
        "name" => Some(o.name.clone()),
        "email" => o.email.clone(),
        "phone" => o.phone.clone(),
        "website" => o.website.clone(),
        "city" => o.city.clone(),
        "address" => o.address.clone(),
        "tax_id" => o.tax_id.clone(),
        "industry" => o.industry.clone(),
        "country" => o.country.clone(),
        _ => None,
    };
    opt(&v)
}

/// `find_duplicate_organizations`: groups of organizations that look like one company.
pub async fn _h_find_duplicate_organizations(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let scope = crate::ai::tools::catalog::_crm_owner_scope(user);
    // every word of the query (normalised like the names) is in the group's name
    let query: Vec<String> = text(&args, "query")
        .map(|q| {
            dedupe::normalize_name(Some(&q))
                .split(' ')
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let limit = int(&args, "limit", 20, 1, 50) as usize;
    let orgs: Vec<Organization> = sqlx::query_as(
        "SELECT * FROM organizations WHERE is_deleted = false AND ($1::uuid IS NULL OR assigned_to_id = $1) \
         ORDER BY created_at",
    )
    .bind(scope)
    .fetch_all(&mut *db)
    .await?;
    let index: HashMap<Uuid, usize> = orgs.iter().enumerate().map(|(i, o)| (o.id, i)).collect();
    // union-find over the organizations
    let mut parent: Vec<usize> = (0..orgs.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut reasons: HashMap<(usize, usize), Vec<String>> = HashMap::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for (i, o) in orgs.iter().enumerate() {
        let k = org_key(&o.name);
        if k.is_empty() {
            continue;
        }
        match by_key.get(&k) {
            Some(&j) => {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
                reasons.entry((j, i)).or_default().push(format!(
                    "same name without legal forms (Ltd, Inc, LLC), case and spaces ('{k}')"
                ));
            }
            None => {
                by_key.insert(k, i);
            }
        }
    }
    let pairs: Vec<(Uuid, Uuid, Option<Value>)> = sqlx::query_as(
        "SELECT record_a_id, record_b_id, reasons FROM crm_duplicate_candidates \
         WHERE object_type = 'organization' AND status = 'open'",
    )
    .fetch_all(&mut *db)
    .await?;
    for (a, b, why) in pairs {
        let (Some(&i), Some(&j)) = (index.get(&a), index.get(&b)) else {
            continue;
        };
        let (ra, rb) = (root(&mut parent, i), root(&mut parent, j));
        parent[ra] = rb;
        let list = reasons.entry((i.min(j), i.max(j))).or_default();
        for r in why.as_ref().and_then(Value::as_array).into_iter().flatten() {
            if let Some(s) = r.as_str() {
                list.push(format!("{s} (CRM duplicate scan)"));
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..orgs.len() {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() > 1).collect();
    if !query.is_empty() {
        groups.retain(|g| {
            g.iter().any(|&i| {
                let key = org_key(&orgs[i].name);
                query.iter().all(|w| key.contains(w.as_str()))
            })
        });
    }
    for g in groups.iter_mut() {
        g.sort();
    }
    groups.sort_by_key(|g| g[0]);
    let total = groups.len();
    let mut out = Vec::new();
    for g in groups.into_iter().take(limit) {
        let mut members = Vec::new();
        let mut best: Option<(i64, usize)> = None;
        for &i in &g {
            let o = &orgs[i];
            let moves = dedupe::move_counts(db, "organization", Uuid::nil(), o.id)
                .await
                .map_err(ApiError::from)?;
            let linked: i64 = moves.iter().map(|m| m["count"].as_i64().unwrap_or(0)).sum();
            if best.is_none_or(|(n, _)| linked > n) {
                best = Some((linked, i));
            }
            let created = local(db, o.created_at).await?;
            members.push(json!({
                "id": o.id.to_string(),
                "name": o.name,
                "email": o.email, "phone": o.phone, "website": o.website, "city": o.city,
                "tax_id": o.tax_id,
                "created": created,
                "linked": moves_text(&moves),
            }));
        }
        let mut differs = Vec::new();
        for f in COMPARED {
            let vals: Vec<String> = g.iter().map(|&i| org_field(&orgs[i], f)).collect();
            let distinct: HashSet<&String> = vals.iter().collect();
            if distinct.len() > 1 {
                let shown: Vec<String> = vals
                    .iter()
                    .map(|v| {
                        if v.is_empty() {
                            "(empty)".into()
                        } else {
                            v.clone()
                        }
                    })
                    .collect();
                differs.push(format!("{f}: {}", shown.join(" / ")));
            }
        }
        let mut why: Vec<String> = Vec::new();
        for (a, ai) in g.iter().enumerate() {
            for bi in &g[a + 1..] {
                for r in reasons.get(&(*ai, *bi)).into_iter().flatten() {
                    if !why.contains(r) {
                        why.push(r.clone());
                    }
                }
            }
        }
        let keep = best.map(|(_, i)| orgs[i].id.to_string());
        out.push(json!({
            "organizations": members,
            "why_duplicates": why,
            "what_differs": differs,
            "suggested_keep_id": keep,
        }));
    }
    Ok(json!({
        "groups": out,
        "total_groups": total,
        "note": "Merge a group with propose_organization_merge (keep_id = the one to keep, usually the one with \
                 most linked; merge_ids = the others). Show the user the groups and what differs first.",
    }))
}

/// The ids a merge names: `merge_ids` (a list or one id) or `merge_id`, in order, without repeats.
fn merge_ids(args: &Value) -> Result<Vec<Uuid>, String> {
    let mut raw: Vec<Value> = match args.get("merge_ids") {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::String(s)) => s.split(',').map(|x| json!(x.trim())).collect(),
        _ => vec![],
    };
    if let Some(v) = args.get("merge_id").filter(|v| !v.is_null()) {
        raw.push(v.clone());
    }
    let mut out = Vec::new();
    for v in raw {
        let s = v.as_str().unwrap_or("").trim().to_string();
        if s.is_empty() {
            continue;
        }
        let id = py_uuid(&s).ok_or_else(|| format!("'{s}' is not an organization id"))?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// What a merge of `gone` into `keep` does: per merged company what moves, and which empty fields of the
/// kept company get filled from which (the first merged company with a value, as the merges run in order).
pub async fn merge_plan(db: &mut PgConnection, keep: Uuid, gone: &[Uuid]) -> Result<Value, String> {
    if gone.is_empty() {
        return Err("Name at least one organization to merge (merge_ids).".into());
    }
    if gone.contains(&keep) {
        return Err("The organization kept can't also be merged into itself.".into());
    }
    let mut per = Vec::new();
    let mut filled: Map<String, Value> = Map::new();
    let mut keep_label = String::new();
    let mut taken: HashSet<String> = HashSet::new();
    for id in gone {
        let prev = dedupe::merge_preview(db, "organization", keep, *id)
            .await
            .map_err(dedupe_msg)?;
        keep_label = prev["keep"]["label"].as_str().unwrap_or("").to_string();
        let label = prev["merge"]["label"].as_str().unwrap_or("").to_string();
        let mut fills = Vec::new();
        for f in prev["fields"].as_array().into_iter().flatten() {
            if f["taken_from"] != "merge" {
                continue;
            }
            let field = f["field"].as_str().unwrap_or("").to_string();
            if !taken.insert(field.clone()) {
                continue; // an earlier merged company already fills it
            }
            let value = crate::crm::attributes::py::str_of(&f["merge"]);
            fills.push(json!({"field": field, "value": value}));
            filled.insert(field, json!({"value": value, "from": label}));
        }
        per.push(json!({
            "id": id.to_string(),
            "label": label,
            "moves": prev["moves"].clone(),
            "fills": fills,
        }));
    }
    Ok(
        json!({"keep": {"id": keep.to_string(), "label": keep_label}, "merged": per, "filled": filled}),
    )
}

fn plan_lines(plan: &Value) -> Vec<String> {
    let keep = plan["keep"]["label"].as_str().unwrap_or("");
    let mut lines = vec![format!("Keep: {keep}")];
    for m in plan["merged"].as_array().into_iter().flatten() {
        let moves: Vec<Value> = m["moves"].as_array().cloned().unwrap_or_default();
        lines.push(format!(
            "Merge away: {}: moves {}",
            m["label"].as_str().unwrap_or(""),
            if moves.is_empty() {
                "nothing".to_string()
            } else {
                moves_text(&moves)
            }
        ));
        for f in m["fills"].as_array().into_iter().flatten() {
            lines.push(format!(
                "  fills {} of {keep}: {}",
                f["field"].as_str().unwrap_or(""),
                f["value"].as_str().unwrap_or("")
            ));
        }
    }
    if plan["filled"].as_object().is_none_or(|m| m.is_empty()) {
        lines.push(format!("No field of {keep} changes (its own values win)."));
    }
    lines.push("Then the merged copies are removed (irreversible).".into());
    lines
}

fn plan_summary(plan: &Value) -> String {
    let keep = plan["keep"]["label"].as_str().unwrap_or("");
    let parts: Vec<String> = plan["merged"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let moves: Vec<Value> = m["moves"].as_array().cloned().unwrap_or_default();
            let fills: Vec<String> = m["fills"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| f["field"].as_str().unwrap_or("").to_string())
                .collect();
            format!(
                "{} (moves {}; fills {})",
                m["label"].as_str().unwrap_or(""),
                if moves.is_empty() {
                    "nothing".into()
                } else {
                    moves_text(&moves)
                },
                if fills.is_empty() {
                    "nothing".into()
                } else {
                    fills.join(", ")
                }
            )
        })
        .collect();
    format!(
        "Merge into organization {keep}: {}; then the merged copies are removed",
        parts.join("; ")
    )
}

/// `propose_organization_merge`: a card that merges one or more organizations into the one kept.
pub async fn _h_propose_organization_merge(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let Some(keep) = text(&args, "keep_id").and_then(|s| py_uuid(&s)) else {
        return err(
            "keep_id must be an organization id (find_duplicate_organizations or search_records).",
        );
    };
    let gone = match merge_ids(&args) {
        Ok(g) => g,
        Err(e) => return err(e),
    };
    let mut ids = vec![keep];
    ids.extend(gone.iter().copied());
    for o in load_orgs(db, &ids).await? {
        if let Err(e) = crate::crm::access::assert_can_view(user, o.assigned_to_id) {
            return err(format!("You can't see {}: {}", o.name, api_msg(&e)));
        }
    }
    let plan = match merge_plan(db, keep, &gone).await {
        Ok(p) => p,
        Err(e) => return err(format!("Can't merge them: {e}")),
    };
    let summary = plan_summary(&plan);
    let payload = json!({
        "keep_id": keep.to_string(),
        "merge_ids": gone.iter().map(|g| g.to_string()).collect::<Vec<_>>(),
    });
    crate::ai::tools::catalog::_autonomy_card(
        db,
        user,
        "organization.merge",
        payload,
        summary,
        None,
    )
    .await
}

fn payload_ids(p: &Value) -> ApiResult<(Uuid, Vec<Uuid>)> {
    let keep = to_uuid(&p["keep_id"])?;
    let mut gone = Vec::new();
    for v in p["merge_ids"].as_array().into_iter().flatten() {
        gone.push(to_uuid(v)?);
    }
    Ok((keep, gone))
}

async fn _apply_org_merge(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let user = need_actor(actor)?;
    let (keep, gone) = payload_ids(p)?;
    // everything is checked before the first merge runs (each merge commits)
    let plan = merge_plan(db, keep, &gone)
        .await
        .map_err(|e| ApiError::http(400, e))?;
    let mut moved = 0i64;
    let mut merged = Vec::new();
    for id in &gone {
        let r = dedupe::merge_records(db, user, "organization", keep, *id, None)
            .await
            .map_err(ApiError::from)?;
        moved += r["moved_rows"].as_i64().unwrap_or(0);
        merged.push(json!(id.to_string()));
    }
    let names: Vec<String> = plan["merged"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| m["label"].as_str().unwrap_or("").to_string())
        .collect();
    Ok(json!({
        "label": format!("Merged {} into {}", names.join(", "), plan["keep"]["label"].as_str().unwrap_or("")),
        "kept_id": keep.to_string(),
        "merged_ids": merged,
        "moved_rows": moved,
    }))
}

async fn _describe_org_merge(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let (keep, gone) = payload_ids(p)?;
    Ok(match merge_plan(db, keep, &gone).await {
        Ok(plan) => {
            let names: Vec<String> = plan["merged"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| m["label"].as_str().unwrap_or("").to_string())
                .collect();
            json!({
                "label": format!("Merge {} into {}", names.join(", "), plan["keep"]["label"].as_str().unwrap_or("")),
                "lines": plan_lines(&plan),
                "after": {"merged": plan["merged"].clone(), "filled": plan["filled"].clone()},
            })
        }
        Err(e) => json!({"label": "Merge organizations", "error": e}),
    })
}

// ─── A4: Nary's cards and what was decided ───────────────────────────────────

/// A short word diff: unchanged runs shortened to "...", removals as [-words-], additions as {+words+}.
pub fn short_diff(before: &str, after: &str) -> String {
    const MAX_WORDS: usize = 1500;
    const CONTEXT: usize = 3;
    const MAX_CHARS: usize = 700;
    let a: Vec<&str> = before.split_whitespace().collect();
    let b: Vec<&str> = after.split_whitespace().collect();
    if a == b {
        return String::new();
    }
    if a.len() > MAX_WORDS || b.len() > MAX_WORDS {
        return "(rewritten; texts too long to compare word by word)".into();
    }
    // longest common subsequence table
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1] + 1
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    // ops: 0 same, 1 removed, 2 added
    let mut ops: Vec<(u8, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((0, a[i]));
            i += 1;
            j += 1;
        } else if i < n && (j == m || t[i + 1][j] >= t[i][j + 1]) {
            ops.push((1, a[i]));
            i += 1;
        } else {
            ops.push((2, b[j]));
            j += 1;
        }
    }
    let mut parts: Vec<String> = Vec::new();
    let mut k = 0;
    while k < ops.len() {
        let kind = ops[k].0;
        let mut words = Vec::new();
        while k < ops.len() && ops[k].0 == kind {
            words.push(ops[k].1);
            k += 1;
        }
        match kind {
            0 => {
                let first = parts.is_empty();
                let last = k >= ops.len();
                if words.len() > CONTEXT * 2
                    || (first && words.len() > CONTEXT)
                    || (last && words.len() > CONTEXT)
                {
                    let head = if first {
                        vec![]
                    } else {
                        words[..CONTEXT].to_vec()
                    };
                    let tail = if last {
                        vec![]
                    } else {
                        words[words.len() - CONTEXT..].to_vec()
                    };
                    let mut s = head.join(" ");
                    if !s.is_empty() {
                        s.push(' ');
                    }
                    s.push_str("...");
                    if !tail.is_empty() {
                        s.push(' ');
                        s.push_str(&tail.join(" "));
                    }
                    parts.push(s);
                } else {
                    parts.push(words.join(" "));
                }
            }
            1 => parts.push(format!("[-{}-]", words.join(" "))),
            _ => parts.push(format!("{{+{}+}}", words.join(" "))),
        }
    }
    let out = parts.join(" ");
    if out.chars().count() > MAX_CHARS {
        let mut cut: String = out.chars().take(MAX_CHARS - 3).collect();
        cut.push_str("...");
        return cut;
    }
    out
}

/// The statuses list_nary_cards filters on, and what each means in agent_pending_actions.
pub const CARD_STATUSES: [&str; 9] = [
    "pending",
    "applied",
    "edited",
    "rejected",
    "expired",
    "cancelled",
    "failed",
    "shadow",
    "all",
];

fn status_sql(status: &str) -> &'static str {
    match status {
        "pending" => "a.status IN ('pending', 'scheduled')",
        "applied" => "a.status = 'applied' AND a.edited_payload IS NULL",
        "edited" => "a.edited_payload IS NOT NULL",
        "rejected" => "a.status = 'rejected'",
        "expired" => "a.status = 'expired'",
        "cancelled" => "a.status IN ('cancelled', 'superseded')",
        "failed" => "a.status = 'failed'",
        "shadow" => "a.status = 'shadow'",
        _ => "true",
    }
}

fn card_text(p: &Value) -> Option<String> {
    p.get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// `list_nary_cards`: Nary's cards (agent_pending_actions) with what was drafted, what went out and who
/// decided, newest first.
pub async fn _h_list_nary_cards(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !(user.is_superadmin
        || user.perms.contains("nary:approve")
        || user.perms.contains("nary:manage")
        || is_owner(db, user).await?)
    {
        return err("Only the Owner or a Nary admin can read the card history.");
    }
    let status = text(&args, "status")
        .unwrap_or_else(|| "all".into())
        .to_lowercase();
    if !CARD_STATUSES.contains(&status.as_str()) {
        return err(format!(
            "status must be one of {}",
            CARD_STATUSES.join(", ")
        ));
    }
    let days = int(&args, "days", 30, 1, 366);
    let limit = int(&args, "limit", 20, 1, 50);
    let offset = int(&args, "offset", 0, 0, 100_000);
    let since = Utc::now() - Duration::days(days);
    let contact = match text(&args, "contact_id") {
        Some(s) => match py_uuid(&s) {
            Some(u) => Some(u),
            None => return err("contact_id must be a contact id (search_contacts)."),
        },
        None => None,
    };
    let chat = text(&args, "chat_id");
    let thread = text(&args, "thread_id");
    // kind: an action name, a prefix ("whatsapp" = whatsapp.*) or a pattern with *
    let kind = text(&args, "kind").map(|k| {
        let k = k.replace('%', "").replace('_', "\\_");
        if k.contains('*') {
            k.replace('*', "%")
        } else if k.contains('.') && !k.ends_with('.') {
            k
        } else {
            format!("{}%", k.trim_end_matches('.'))
        }
    });
    let filter = format!(
        "a.created_at >= $1 AND {} \
         AND ($2::uuid IS NULL OR (a.entity_type = 'contact' AND a.entity_id = $2) OR a.payload->>'contact_id' = $2::text) \
         AND ($3::text IS NULL OR a.payload->>'chat_id' = $3) \
         AND ($4::text IS NULL OR a.payload->>'thread_id' = $4) \
         AND ($5::text IS NULL OR a.action LIKE $5)",
        status_sql(&status)
    );
    let (total,): (i64,) = sqlx::query_as(&format!(
        "SELECT count(*) FROM agent_pending_actions a WHERE {filter}"
    ))
    .bind(since)
    .bind(contact)
    .bind(&chat)
    .bind(&thread)
    .bind(&kind)
    .fetch_one(&mut *db)
    .await?;
    let rows: Vec<AgentPendingAction> = sqlx::query_as(&format!(
        "SELECT a.* FROM agent_pending_actions a WHERE {filter} ORDER BY a.created_at DESC, a.id LIMIT $6 OFFSET $7"
    ))
    .bind(since)
    .bind(contact)
    .bind(&chat)
    .bind(&thread)
    .bind(&kind)
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *db)
    .await?;
    let decider_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.decided_by_id).collect();
    let names: HashMap<Uuid, String> =
        sqlx::query_as::<_, (Uuid, String)>("SELECT id, name FROM users WHERE id = ANY($1)")
            .bind(&decider_ids)
            .fetch_all(&mut *db)
            .await?
            .into_iter()
            .collect();
    let mut cards = Vec::new();
    for r in &rows {
        let label = super::actions::get(&r.action)
            .map(|a| a.label.clone())
            .unwrap_or_else(|_| r.action.clone());
        let shown_status = if r.edited_payload.is_some() && r.status == "applied" {
            "edited".to_string()
        } else {
            r.status.clone()
        };
        let drafted = card_text(&r.payload);
        let final_text = match &r.edited_payload {
            Some(e) => card_text(e),
            None if r.status == "shadow" => r.actual_text.clone().filter(|t| !t.trim().is_empty()),
            None => None,
        };
        let mut c = Map::new();
        c.insert("id".into(), json!(r.id.to_string()));
        c.insert("action".into(), json!(r.action));
        c.insert("what".into(), json!(label));
        c.insert("status".into(), json!(shown_status));
        c.insert("origin".into(), json!(r.origin));
        if let Some(s) = r
            .diff
            .as_ref()
            .and_then(|d| d.get("label"))
            .filter(|v| !v.is_null())
        {
            c.insert("summary".into(), s.clone());
        }
        for k in ["chat_id", "thread_id", "to", "contact_id"] {
            if let Some(v) = r
                .payload
                .get(k)
                .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            {
                c.insert(k.into(), v.clone());
            }
        }
        c.insert("created".into(), json!(local(db, r.created_at).await?));
        if let Some(t) = r.decided_at {
            c.insert("decided".into(), json!(local(db, t).await?));
        }
        if let Some(d) = r.decided_by_id {
            c.insert(
                "decided_by".into(),
                json!(names.get(&d).cloned().unwrap_or_else(|| d.to_string())),
            );
        }
        if let Some(t) = &drafted {
            c.insert("drafted_text".into(), json!(t));
        }
        if let Some(f) = &final_text {
            if Some(f) != drafted.as_ref() {
                c.insert("final_text".into(), json!(f));
                c.insert(
                    "changes".into(),
                    json!(short_diff(drafted.as_deref().unwrap_or(""), f)),
                );
            }
        }
        if r.edited_payload.is_some() && final_text.is_none() {
            c.insert(
                "edited_payload".into(),
                r.edited_payload.clone().unwrap_or(Value::Null),
            );
        }
        if let Some(reason) = r.reject_reason.as_ref().filter(|s| !s.trim().is_empty()) {
            c.insert("reject_reason".into(), json!(reason));
        }
        if let Some(e) = r.error.as_ref().filter(|s| !s.trim().is_empty()) {
            c.insert("error".into(), json!(e));
        }
        cards.push(Value::Object(c));
    }
    Ok(json!({
        "total": total,
        "offset": offset,
        "count": cards.len(),
        "cards": cards,
        "note": "changes: [-removed-] {+added+}. Say the total and offer the next page when there are more.",
    }))
}

// ─── A9: delete a contact (Owner only) ──────────────────────────────────────────

/// Quotation statuses that are finished (a quotation in any other status is still open).
const CLOSED_QUOTATIONS: &str =
    "('rejected', 'expired', 'cancelled', 'declined', 'lost', 'superseded', 'invoiced', 'converted')";

/// The open deals and quotations of a contact: [{"type", "id", "label"}].
pub async fn open_records(db: &mut PgConnection, contact_id: Uuid) -> ApiResult<Vec<Value>> {
    let closed: Vec<&str> = crate::crm::stages::CLOSED_STAGES.to_vec();
    let deals: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, title, stage FROM deals WHERE contact_id = $1 AND is_deleted = false \
         AND (stage IS NULL OR NOT (stage = ANY($2))) ORDER BY created_at",
    )
    .bind(contact_id)
    .bind(&closed)
    .fetch_all(&mut *db)
    .await?;
    let quotes: Vec<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(&format!(
        "SELECT id, number, status FROM quotations WHERE customer_contact_id = $1 AND is_deleted = false \
         AND (status IS NULL OR status NOT IN {CLOSED_QUOTATIONS}) ORDER BY created_at"
    ))
    .bind(contact_id)
    .fetch_all(&mut *db)
    .await?;
    let mut out: Vec<Value> = deals
        .into_iter()
        .map(|(id, title, stage)| {
            json!({"type": "deal", "id": id.to_string(),
                   "label": format!("deal {title} ({})", stage.unwrap_or_default())})
        })
        .collect();
    out.extend(quotes.into_iter().map(|(id, number, status)| {
        json!({"type": "quotation", "id": id.to_string(),
               "label": format!("quotation {} ({})", number.unwrap_or_else(|| id.to_string()), status.unwrap_or_default())})
    }));
    Ok(out)
}

async fn load_contact(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Contact>> {
    Ok(sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

fn contact_name(c: &Contact) -> String {
    format!("{} {}", c.first_name, c.last_name.as_deref().unwrap_or(""))
        .trim()
        .to_string()
}

/// What stays linked to the contact (it keeps pointing at the deleted contact, as in the CRM).
async fn linked_counts(db: &mut PgConnection, id: Uuid) -> ApiResult<String> {
    let mut parts = Vec::new();
    for (sql, what) in [
        (
            "SELECT count(*) FROM leads WHERE contact_id = $1 AND is_deleted = false",
            "leads",
        ),
        (
            "SELECT count(*) FROM deals WHERE contact_id = $1 AND is_deleted = false",
            "deals",
        ),
        (
            "SELECT count(*) FROM activities WHERE contact_id = $1 AND is_deleted = false",
            "activities",
        ),
        (
            "SELECT count(*) FROM wa_chats WHERE contact_id = $1",
            "WhatsApp chats",
        ),
    ] {
        let (n,): (i64,) = sqlx::query_as(sql).bind(id).fetch_one(&mut *db).await?;
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    Ok(if parts.is_empty() {
        "nothing linked".into()
    } else {
        parts.join(", ")
    })
}

/// `propose_contact_delete`: the Owner removes a contact from the CRM (the CRM's delete), on a card.
pub async fn _h_propose_contact_delete(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !is_owner(db, user).await? {
        return err(
            "Only the Owner can delete a contact through Nary; others delete it in the CRM.",
        );
    }
    let Some(id) = text(&args, "contact_id").and_then(|s| py_uuid(&s)) else {
        return err("contact_id must be a contact id (search_contacts).");
    };
    let Some(c) = load_contact(db, id).await?.filter(|c| !c.is_deleted) else {
        return err("No such contact (or it is already deleted).");
    };
    let open = open_records(db, id).await?;
    if !open.is_empty() && !flag(&args, "include_open") {
        let list: Vec<&str> = open.iter().filter_map(|o| o["label"].as_str()).collect();
        return err(format!(
            "{} still has open records: {}. Tell the user; only if they still want it deleted, call again with \
             include_open: true (the records stay, listed on the card).",
            contact_name(&c),
            list.join("; ")
        ));
    }
    let reason = text(&args, "reason");
    let linked = linked_counts(db, id).await?;
    let mut summary = format!("Delete contact {} from the CRM", contact_name(&c));
    if let Some(r) = &reason {
        summary.push_str(&format!(" ({r})"));
    }
    summary.push_str(&format!("; still linked: {linked}"));
    if !open.is_empty() {
        let list: Vec<&str> = open.iter().filter_map(|o| o["label"].as_str()).collect();
        summary.push_str(&format!(
            "; open records kept without it: {}",
            list.join("; ")
        ));
    }
    let payload = json!({
        "contact_id": id.to_string(),
        "reason": reason,
        "open_records": open.iter().map(|o| o["id"].clone()).collect::<Vec<_>>(),
    });
    crate::ai::tools::catalog::_autonomy_card(db, user, "contact.delete", payload, summary, None)
        .await
}

async fn _apply_contact_delete(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let user = need_actor(actor)?;
    if !is_owner(db, user).await? {
        return Err(ApiError::http(
            403,
            "Only the Owner can delete a contact through Nary",
        ));
    }
    let id = to_uuid(&p["contact_id"])?;
    let Some(c) = load_contact(db, id).await?.filter(|c| !c.is_deleted) else {
        return Err(ApiError::http(
            404,
            "The contact is gone or already deleted",
        ));
    };
    let listed: HashSet<String> = p["open_records"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let new_open: Vec<String> = open_records(db, id)
        .await?
        .into_iter()
        .filter(|o| !listed.contains(o["id"].as_str().unwrap_or("")))
        .filter_map(|o| o["label"].as_str().map(str::to_string))
        .collect();
    if !new_open.is_empty() {
        return Err(ApiError::http(
            409,
            format!(
                "Not deleted: open since the card was made: {}",
                new_open.join("; ")
            ),
        ));
    }
    // DELETE /contacts/{id}: the CRM's soft delete
    sqlx::query(
        "UPDATE contacts SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(Utc::now())
    .execute(&mut *db)
    .await?;
    let name = contact_name(&c);
    crate::crm::writes::insert_audit(
        db,
        crate::crm::writes::AuditEntry {
            user_id: Some(user.id),
            user_name: Some(user.name.clone()),
            action: "delete".into(),
            entity_type: "contact".into(),
            entity_id: Some(id.to_string()),
            entity_label: Some(name.chars().take(200).collect()),
            old_values: json!({"is_deleted": false}),
            new_values: json!({"is_deleted": true}),
            changed_fields: Some(vec!["is_deleted".into()]),
            origin: "nary".into(),
            notes: p["reason"]
                .as_str()
                .map(|r| format!("Deleted through Nary: {r}")),
        },
    )
    .await?;
    super::commit(db).await?;
    Ok(json!({"label": format!("Deleted contact {name}"), "contact_id": id.to_string()}))
}

async fn _describe_contact_delete(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let id = to_uuid(&p["contact_id"])?;
    let Some(c) = load_contact(db, id).await? else {
        return Ok(json!({"label": "Delete a contact", "error": "The contact is gone"}));
    };
    let name = contact_name(&c);
    let org: Option<(String,)> = match c.organization_id {
        Some(o) => {
            sqlx::query_as("SELECT name FROM organizations WHERE id = $1")
                .bind(o)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let mut lines = vec![format!(
        "Contact: {name}; email {}; phone {}; organization {}",
        c.email
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "none".into()),
        c.phone
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "none".into()),
        org.map(|o| o.0).unwrap_or_else(|| "none".into()),
    )];
    if let Some(r) = p["reason"].as_str().filter(|r| !r.is_empty()) {
        lines.push(format!("Reason: {r}"));
    }
    lines.push(format!(
        "Still linked to it: {}",
        linked_counts(db, id).await?
    ));
    let open = open_records(db, id).await?;
    if !open.is_empty() {
        lines.push("Open records (they stay, without this contact):".into());
        for o in &open {
            lines.push(format!("  {}", o["label"].as_str().unwrap_or("")));
        }
    }
    lines.push("The CRM's delete: the contact is hidden from lists and search.".into());
    Ok(json!({
        "label": format!("Delete contact {name}"),
        "lines": lines,
        "after": {"contact": "deleted"},
    }))
}

// ─── registry ─────────────────────────────────────────────────────────────────

/// The actions this module registers (nary/actions.rs module_actions).
pub fn actions() -> Vec<Action> {
    vec![
        Action::new(
            "organization.merge",
            "Merge organizations (one company)",
            Schema::strict(
                "OrganizationMerge",
                vec![
                    Field::req("keep_id", Ty::Str),
                    Field::req("merge_ids", Ty::list(Ty::Str)),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_org_merge(db, a, p))),
        )
        .permission("crm:edit_contact")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_org_merge(db, a, p))))
        .entity(entity_fn(|p| {
            Ok((Some("organization".into()), Some(to_uuid(&p["keep_id"])?)))
        })),
        Action::new(
            "contact.delete",
            "Delete a contact",
            Schema::strict(
                "ContactDelete",
                vec![
                    Field::req("contact_id", Ty::Str),
                    Field::opt("reason", Ty::Str),
                    Field::default("open_records", Ty::list(Ty::Str), json!([])),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_contact_delete(db, a, p))),
        )
        .permission("crm:delete_contact")
        .describe(apply_fn(|db, a, p| {
            Box::pin(_describe_contact_delete(db, a, p))
        }))
        .entity(entity_fn(|p| {
            Ok((Some("contact".into()), Some(to_uuid(&p["contact_id"])?)))
        })),
    ]
}
