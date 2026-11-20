//! Port of backend/app/services/nary/proposals.py (agent N6).
//!
//! Requirement discovery and Solution Proposal drafting (Nary Phase 4 item B; design §11).
//!
//! ```text
//!     trigger        the requirement fit check (Phase 3) says custom, tender, complex or asked → a card:
//!                    "start requirement discovery?" (proposal.discovery.start, level 1)
//!     discovery      a live requirements sheet on the deal (an internal note, kind "requirements"): each
//!                    checklist field (proposal.discovery.fields) known or not, with the customer's words. Each
//!                    new message updates it (Sonnet), and the reply asks the next one or two unknown fields. At
//!                    proposal.discovery.min_coverage_pct the rest become assumptions and Nary proposes the draft.
//!     draft          Opus (nary.proposal_draft) writes the proposal from the template's sections and guidance,
//!                    the sheet, the fit check, approved company facts and customer-safe knowledge, in the
//!                    customer's language. An accuracy figure never survives: sentences stating one are
//!                    removed and the Owner is told. Then the figures and the internal cost sheet.
//!     confirm        the Owner edits and confirms it; Nary drafts the matching quotation (catalog lines from the
//!                    proposal, manual lines from the Owner's final prices on the cost sheet), linked to it.
//!     send           once both are approved, proposal.send sends the proposal and its quotations together.
//! ```

use granary_compat::pyjson::{self, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json, SettingError};
use granary_db::rows::crm::{Contact, Deal, Organization};
use granary_db::rows::documents::SalesDocument;
use granary_db::rows::nary::{AgentRun, NaryInsight};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::py_truthy;
use super::negotiation::dumps_unicode;

use super::replies::Conversation;

pub const WHERE: &str = "Sales & CRM → Proposals";

/// "95% accuracy", "accuracy of 99.5 %", "precision above 90 %": never before the customer validates it.
static ACCURACY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)[^.\n]*\b(accura\w*|precision|success rate|error rate)\b[^.\n]*\d+(?:[.,]\d+)?\s?%[^.\n]*\.?",
        r"|[^.\n]*\d+(?:[.,]\d+)?\s?%[^.\n]*\b(accura\w*|precision|success rate|error rate)\b[^.\n]*\.?"
    ))
    .unwrap()
});

/// `strip_accuracy(text)`: remove every sentence that states an accuracy-like figure; (text, removed).
pub fn strip_accuracy(text: &str) -> (String, Vec<String>) {
    let removed = ACCURACY
        .find_iter(text)
        .map(|m| pystr::strip(m.as_str()).to_string())
        .collect();
    let rest = ACCURACY.replace_all(text, "");
    (pystr::strip(&rest).to_string(), removed)
}

/// `checklist(db)`: the discovery fields (proposal.discovery.fields).
pub async fn checklist(db: &mut PgConnection) -> ApiResult<Vec<Value>> {
    let fields = require_json(db, "proposal.discovery.fields", WHERE).await?;
    match fields {
        Value::Array(a) if !a.is_empty() => Ok(a),
        _ => Err(SettingError::new(
            "proposal.discovery.fields",
            "must be a list of {key, label, ask}",
            WHERE,
        )
        .into()),
    }
}

/// `sheet(db, deal_id)`: the newest requirements sheet of a deal.
pub async fn sheet(db: &mut PgConnection, deal_id: Uuid) -> ApiResult<Option<NaryInsight>> {
    Ok(sqlx::query_as(
        "SELECT * FROM nary_insights WHERE deal_id = $1 AND kind = 'requirements' ORDER BY created_at DESC LIMIT 1",
    )
    .bind(deal_id)
    .fetch_optional(&mut *db)
    .await?)
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(t)) => !t.is_empty(),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Bool(true)) => true,
    }
}

/// `x or {}` then `.get(k)`.
fn get_or_empty<'a>(v: Option<&'a Value>, k: &str) -> Option<&'a Value> {
    if truthy(v) {
        v.and_then(|v| v.get(k))
    } else {
        None
    }
}

/// `coverage(content, fields)`: the percentage of checklist fields known, `round()` half to even.
pub fn coverage(content: &Value, fields: &[Value]) -> i64 {
    if fields.is_empty() {
        return 100;
    }
    let known = fields
        .iter()
        .filter(|f| {
            let key = f.get("key").and_then(|k| k.as_str()).unwrap_or_default();
            truthy(get_or_empty(
                get_or_empty(content.get("fields"), key),
                "known",
            ))
        })
        .count() as i64;
    py_round_div(100 * known, fields.len() as i64)
}

/// Python `round(a / b)` for non-negative ints: banker's rounding of the float quotient.
fn py_round_div(a: i64, b: i64) -> i64 {
    let q = a / b;
    let r = a % b;
    match (2 * r).cmp(&b) {
        std::cmp::Ordering::Less => q,
        std::cmp::Ordering::Greater => q + 1,
        std::cmp::Ordering::Equal => {
            if q % 2 == 0 {
                q
            } else {
                q + 1
            }
        }
    }
}

fn internal(msg: impl Into<String>) -> ApiError {
    ApiError::internal(anyhow::anyhow!(msg.into()))
}

/// `str(x)` in an f-string.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        other => pyjson::str_value(&PyValue::from(other)),
    }
}

fn key<'a>(d: &'a Value, k: &str) -> ApiResult<&'a Value> {
    d.get(k).ok_or_else(|| internal(format!("KeyError: '{k}'")))
}

/// `x in (seq or [])` for a template's `blocks` (a list by membership, a string by substring).
fn listed(seq: Option<&Value>, x: &str) -> bool {
    match seq {
        Some(Value::Array(a)) => a.iter().any(|v| v.as_str() == Some(x)),
        Some(Value::String(t)) => t.contains(x),
        Some(Value::Object(o)) => o.contains_key(x),
        _ => false,
    }
}

async fn get_deal(db: &mut PgConnection, deal_id: &str) -> ApiResult<Option<Deal>> {
    let id = super::actions::to_uuid(&Value::String(deal_id.to_string()))?;
    Ok(sqlx::query_as("SELECT * FROM deals WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

/// `maybe_offer(db, conv, fit, *, run=None)`: after a fit check, a custom, tender, complex or requested scope
/// offers discovery (once per deal).
pub async fn maybe_offer(
    db: &mut PgConnection,
    conv: &Conversation,
    fit: Option<&Value>,
    run: Option<&AgentRun>,
) -> ApiResult<Option<Value>> {
    let Some(fit) = fit.filter(|f| py_truthy(f)) else {
        return Ok(None);
    };
    let Some(trigger) = fit.get("proposal_trigger").filter(|t| py_truthy(t)) else {
        return Ok(None);
    };
    let Some(deal) = conv.deal.as_ref() else {
        return Ok(None);
    };
    if sheet(db, deal.id).await?.is_some() {
        return Ok(None);
    }
    let open_card: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM agent_pending_actions WHERE action = 'proposal.discovery.start' \
         AND status IN ('pending', 'shadow') AND (payload ->> 'deal_id') = $1 LIMIT 1",
    )
    .bind(deal.id.to_string())
    .fetch_optional(&mut *db)
    .await?;
    if open_card.is_some() {
        return Ok(None);
    }
    let card = super::replies::_propose(
        db,
        "proposal.discovery.start",
        json!({"deal_id": deal.id.to_string(), "reason": trigger}),
        run,
        None,
        Some(&format!(
            "{}: the request looks {}. Start requirement discovery for a Solution Proposal?",
            deal.title,
            py_str(trigger)
        )),
        None,
    )
    .await?;
    Ok(card.map(|c| json!({"card_id": c.id.to_string(), "status": c.status})))
}

/// Applies proposal.discovery.start. Commits.
pub async fn start_discovery(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    deal_id: &str,
    reason: &str,
) -> ApiResult<Value> {
    let _ = actor;
    let Some(deal) = get_deal(db, deal_id).await? else {
        return Err(ApiError::http(404, "Deal not found"));
    };
    if let Some(existing) = sheet(db, deal.id).await? {
        return Ok(
            json!({"label": "Discovery already running", "insight_id": existing.id.to_string()}),
        );
    }
    let fields = checklist(db).await?;
    let mut fmap = Map::new();
    for f in &fields {
        fmap.insert(
            py_str(key(f, "key")?),
            json!({"known": false, "value": null}),
        );
    }
    let content = json!({"reason": reason, "fields": fmap, "assumptions": [], "coverage": 0});
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO nary_insights (id, kind, deal_id, content, status) VALUES ($1, 'requirements', $2, $3, 'discovery')")
        .bind(id)
        .bind(deal.id)
        .bind(&content)
        .execute(&mut *db)
        .await?;
    super::commit(db).await?;
    Ok(
        json!({"label": format!("Requirement discovery started for {}", deal.title), "insight_id": id.to_string()}),
    )
}

pub static UPDATE_SCHEMA: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(r#"{"type": "object", "properties": {"fields": {"type": "array", "items": {"type": "object", "properties": {"key": {"type": "string"}, "value": {"type": ["string", "null"]}}, "required": ["key", "value"], "additionalProperties": false}}, "language": {"type": "string", "enum": ["en"]}}, "required": ["fields", "language"], "additionalProperties": false}"#).unwrap()
});

pub const UPDATE_RULES: &str =
    "Fill a requirements checklist from the conversation. For each key, the \
customer's answer in a short phrase, or null when the conversation doesn't \
say. Never guess. language: the language the customer writes in.";

/// `update_sheet(db, conv, *, run=None)`: fill the requirements sheet from the conversation (Sonnet).
/// Returns what the reply should ask next, or None when no discovery is running for this deal. Commits.
pub async fn update_sheet(
    db: &mut PgConnection,
    conv: &Conversation,
    mut run: Option<&mut AgentRun>,
) -> ApiResult<Option<Value>> {
    let Some(deal) = conv.deal.as_ref() else {
        return Ok(None);
    };
    let Some(row) = sheet(db, deal.id).await? else {
        return Ok(None);
    };
    if row.status != "discovery" {
        return Ok(None);
    }
    let fields = checklist(db).await?;
    let mut listing = Vec::new();
    for f in &fields {
        listing.push(json!({"key": key(f, "key")?, "label": key(f, "label")?}));
    }
    let content_in = format!(
        "CHECKLIST:\n{}\n\nCONVERSATION:\n{}",
        dumps_unicode(&Value::Array(listing))?,
        super::llm::information_default("customer", &conv.transcript)
    );
    let system = super::persona::system_prompt("staff", UPDATE_RULES, "", "");
    let data = super::llm::ask_json(
        db,
        "nary.fit_check",
        &system,
        &content_in,
        &UPDATE_SCHEMA,
        run.as_deref_mut(),
        None,
        None,
    )
    .await?
    .unwrap_or_else(|| json!({}));
    let mut content: Map<String, Value> = row.content.as_object().cloned().unwrap_or_default();
    let mut current: Map<String, Value> = match content.get("fields") {
        Some(Value::Object(o)) => o.clone(),
        _ => Map::new(),
    };
    if let Some(Value::Array(updates)) = data.get("fields") {
        for f in updates {
            let Some(k) = f.get("key").and_then(|k| k.as_str()) else {
                continue;
            };
            if current.contains_key(k) && f.get("value").is_some_and(py_truthy) {
                let v = py_str(&f["value"]);
                current.insert(
                    k.to_string(),
                    json!({"known": true, "value": super::py_slice(&v, 500)}),
                );
            }
        }
    }
    content.insert("fields".into(), Value::Object(current.clone()));
    let lang_new = data.get("language").filter(|l| py_truthy(l)).cloned();
    let language =
        lang_new.unwrap_or_else(|| content.get("language").cloned().unwrap_or(Value::Null));
    content.insert("language".into(), language);
    let cov = coverage(&Value::Object(content.clone()), &fields);
    content.insert("coverage".into(), json!(cov));
    sqlx::query("UPDATE nary_insights SET content = $2, updated_at = now() WHERE id = $1")
        .bind(row.id)
        .bind(Value::Object(content.clone()))
        .execute(&mut *db)
        .await?;
    let lang = "en";
    let mut unknown = Vec::new();
    for f in &fields {
        let k = py_str(key(f, "key")?);
        if !current
            .get(&k)
            .and_then(|c| c.get("known"))
            .is_some_and(py_truthy)
        {
            unknown.push(f.clone());
        }
    }
    let min_cov = require_int(db, "proposal.discovery.min_coverage_pct", WHERE).await?;
    let ready = cov >= min_cov;
    if ready {
        let mut assumptions = Vec::new();
        for f in &unknown {
            assumptions.push(Value::String(format!(
                "{}: not known yet; to be confirmed",
                py_str(key(f, "label")?)
            )));
        }
        content.insert("assumptions".into(), Value::Array(assumptions));
        sqlx::query("UPDATE nary_insights SET content = $2, status = 'ready', updated_at = now() WHERE id = $1")
            .bind(row.id)
            .bind(Value::Object(content.clone()))
            .execute(&mut *db)
            .await?;
        super::commit(db).await?;
        super::replies::_propose(
            db,
            "proposal.create_draft",
            json!({"deal_id": deal.id.to_string()}),
            run.as_deref(),
            None,
            Some(&format!(
                "Requirements for {} are {}% known. Draft the Solution Proposal?",
                deal.title, cov
            )),
            None,
        )
        .await?;
    } else {
        super::commit(db).await?;
    }
    let mut ask = Vec::new();
    for f in unknown.iter().take(2) {
        // the question is English text; an older stored {"en": ..} object reads as its English text
        let a = key(f, "ask")?;
        ask.push(match a {
            Value::Object(_) => key(a, lang)?.clone(),
            _ => a.clone(),
        });
    }
    Ok(Some(json!({"coverage": cov, "ask": ask, "ready": ready})))
}

// ─── Drafting ─────────────────────────────────────────────────────────────────

pub static DRAFT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(r#"{"type": "object", "properties": {"title": {"type": "string"}, "subtitle": {"type": "string"}, "sections": {"type": "array", "items": {"type": "object", "properties": {"key": {"type": "string"}, "blocks": {"type": "array", "items": {"type": "object", "properties": {"type": {"type": "string", "enum": ["text", "kv", "table", "coverage"]}, "heading": {"type": ["string", "null"]}, "md": {"type": ["string", "null"]}, "rows": {"type": ["array", "null"], "items": {}}, "columns": {"type": ["array", "null"], "items": {"type": "string"}}}, "required": ["type"], "additionalProperties": false}}}, "required": ["key", "blocks"], "additionalProperties": false}}}, "required": ["title", "subtitle", "sections"], "additionalProperties": false}"#).unwrap()
});

/// The drafting rules (the f-string of `draft`, for a language).
pub fn draft_rules(language: &str) -> String {
    format!(
        "Write a Solution Proposal in the house style of the company's proposals: factual, \
terse, no superlatives, obligations stated plainly. Language: \
{}. \
Fill each section in the order given, following its guidance. Blocks: text (Markdown), kv \
(rows of [label, value]), table (columns and rows), coverage (rows of {{requirement, status: \
included|optional_custom|not_in_scope|to_confirm, notes}}). Never state an accuracy, precision or success-rate figure. Never state a price: \
the commercial section is generated from the quotation. Only facts from the brief, the \
knowledge and the company facts; anything else is an assumption, said as one.",
        if language == "en" || language.is_empty() {
            "English"
        } else {
            "the customer's language (formal, use the customer company name)"
        }
    )
}

/// `_knowledge(db, query)`: customer-safe knowledge and the trusted company facts, as prompt text.
pub async fn _knowledge(db: &mut PgConnection, query: &str) -> ApiResult<String> {
    let hits = match crate::ai::knowledge::search(db, query, 4, None, true).await {
        Ok(h) => h,
        Err(e) => {
            // embeddings down or not configured: draft from the facts alone
            let msg = e.to_string();
            tracing::warn!(error = %super::py_slice(&msg, 200), "nary_proposal_knowledge_unavailable");
            Vec::new()
        }
    };
    let facts = super::memory::recall(
        db,
        super::memory::Recall {
            layers: vec!["company".into()],
            trusted_only: true,
            limit: 20,
            ..Default::default()
        },
    )
    .await?;
    let knowledge = hits
        .iter()
        .map(|h| format!("{}: {}", h.title, super::py_slice(&h.content, 1500)))
        .collect::<Vec<_>>()
        .join("\n---\n");
    let company = facts
        .iter()
        .map(|r| {
            format!(
                "- {}: {}",
                r.title.clone().unwrap_or_else(|| "None".into()),
                r.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "KNOWLEDGE:\n{knowledge}\n\nCOMPANY FACTS:\n{company}"
    ))
}

static FAIL_AFTER_DOCUMENT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Test seam: make drafting stop right after the document exists (as a cut connection or a crash would).
pub fn set_fail_after_document(fail: bool) {
    FAIL_AFTER_DOCUMENT.store(fail, std::sync::atomic::Ordering::SeqCst);
}

async fn latest_insight(
    db: &mut PgConnection,
    deal_id: Uuid,
    kind: &str,
) -> ApiResult<Option<NaryInsight>> {
    Ok(sqlx::query_as("SELECT * FROM nary_insights WHERE deal_id = $1 AND kind = $2 ORDER BY created_at DESC LIMIT 1")
        .bind(deal_id)
        .bind(kind)
        .fetch_optional(&mut *db)
        .await?)
}

/// `(x.content or {}).get(k)` of an optional insight.
fn content_get(row: Option<&NaryInsight>, k: &str) -> Option<Value> {
    let row = row?;
    if !py_truthy(&row.content) {
        return Some(Value::Null);
    }
    Some(row.content.get(k).cloned().unwrap_or(Value::Null))
}

const COVERAGE_STATUSES: [&str; 4] = ["included", "optional_custom", "not_in_scope", "to_confirm"];

/// Applies proposal.create_draft: write the proposal, draw its figures, cost it. Returns the document. Commits.
pub async fn draft(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    deal_id: &str,
) -> ApiResult<Value> {
    draft_with(db, actor, deal_id, DraftInto::default()).await
}

/// fix-longapply: where [`draft_with`] writes.
#[derive(Debug, Clone, Default)]
pub struct DraftInto {
    /// Redraft this proposal (a draft) instead of creating one: the retry of a cut apply, or a revision.
    pub reuse: Option<Uuid>,
    /// The card being applied: the document id is stored on it as soon as the document exists, so a retry
    /// finds it.
    pub record_on: Option<Uuid>,
    /// What the person asked for (a revision): goes into the brief.
    pub instruction: Option<String>,
}

/// 23 Oct: what a proposal is written for (the drafter and proposal.from_sections share it): the deal, its
/// customer, the language, the template and the brief (sheet, fit check, a revision's instruction).
pub struct DraftContext {
    pub deal: Deal,
    pub org: Option<Organization>,
    pub contact: Option<Contact>,
    pub language: String,
    pub tmpl: Vec<Value>,
    pub brief: Value,
}

/// [`draft`], into an existing draft or recording the new document on a card (fix-longapply).
pub async fn draft_with(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    deal_id: &str,
    into: DraftInto,
) -> ApiResult<Value> {
    let ctx = draft_context(db, deal_id, into.instruction.as_deref()).await?;
    let deal_ref = ctx.deal.id.to_string();
    let mut run = super::runs::start_run(
        db,
        "manual",
        "nary.proposal_draft",
        Some(&deal_ref),
        "internal",
        None,
    )
    .await?;
    let (language, deal, brief, tmpl) = (&ctx.language, &ctx.deal, &ctx.brief, &ctx.tmpl);
    let mut sections = Vec::new();
    for s in tmpl {
        let blocks = s.get("blocks").filter(|b| py_truthy(b));
        if listed(blocks, "commercial") || listed(blocks, "stamp") {
            continue;
        }
        let t = match crate::documents::core_::section_title_text(s.get("title")) {
            Some(t) => Value::String(t),
            None => key(s, "key")?.clone(),
        };
        sections.push(json!({"key": key(s, "key")?, "title": t, "blocks": s.get("blocks").cloned().unwrap_or(Value::Null),
                             "guidance": s.get("guidance").cloned().unwrap_or(Value::Null)}));
    }
    let knowledge = _knowledge(db, &format!("{} requirements", deal.title)).await?;
    let content = format!(
        "SECTIONS:\n{}\n\nBRIEF (JSON):\n{}\n\n{}",
        dumps_unicode(&Value::Array(sections))?,
        dumps_unicode(brief)?,
        knowledge
    );
    // visuals V2: the draft also plans its figures (chart / diagram blocks, the phases for the timeline)
    let rules = format!(
        "{}{}",
        draft_rules(language),
        super::proposal_visuals::DRAFT_FIGURE_RULES
    );
    let system = super::persona::system_prompt("customer", &rules, "", "");
    let draft_schema = super::proposal_visuals::extend_draft_schema(&DRAFT_SCHEMA);
    // 23 Oct: an unusable answer says why (cut at the output limit, not JSON, the provider's error)
    let data = super::llm::ask_json_why(
        db,
        "nary.proposal_draft",
        &system,
        &content,
        &draft_schema,
        Some(&mut run),
        None,
        None,
    )
    .await?;
    let data = match data {
        Ok(d) => d,
        Err(why) => {
            let error = format!("no usable draft: {why}");
            super::runs::finish_run(db, &mut run, "failed", None, Some(&error)).await?;
            super::commit(db).await?;
            return Err(ApiError::http(
                502,
                format!(
                    "Nary couldn't write the proposal text: {why}. No proposal text was saved."
                ),
            ));
        }
    };
    build_from_draft(db, actor, &ctx, data, into, &mut run, true).await
}

/// 23 Oct: the deal, customer, language, template and brief a proposal is written from.
pub async fn draft_context(
    db: &mut PgConnection,
    deal_id: &str,
    instruction: Option<&str>,
) -> ApiResult<DraftContext> {
    use crate::documents::core_ as core;
    let Some(deal) = get_deal(db, deal_id).await? else {
        return Err(ApiError::http(404, "Deal not found"));
    };
    let req = sheet(db, deal.id).await?;
    let fit = latest_insight(db, deal.id, "fit_check").await?;
    let org: Option<Organization> = match deal.organization_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let contact: Option<Contact> = match deal.contact_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let language = content_get(req.as_ref(), "language")
        .filter(py_truthy)
        .map(|l| py_str(&l))
        .unwrap_or_else(|| "en".into());
    let tmpl = core::template(db).await?;
    let customer = match (&org, &contact) {
        (Some(o), _) => o.name.clone(),
        (None, Some(c)) => c.first_name.clone(),
        (None, None) => deal.title.clone(),
    };
    let mut brief = Map::new();
    brief.insert("customer".into(), json!(customer));
    brief.insert("deal".into(), json!(deal.title));
    brief.insert(
        "requirements".into(),
        content_get(req.as_ref(), "fields").unwrap_or(Value::Null),
    );
    brief.insert(
        "assumptions".into(),
        content_get(req.as_ref(), "assumptions").unwrap_or_else(|| json!([])),
    );
    // batch 2 T1: what the Owner gave in the chat (items, options A/B, notes), only when the sheet has it
    for k in ["scope", "notes"] {
        if let Some(v) = content_get(req.as_ref(), k).filter(py_truthy) {
            brief.insert(k.into(), v);
        }
    }
    // fix-longapply: a revision's instruction
    if let Some(i) = instruction.filter(|i| !i.trim().is_empty()) {
        brief.insert("revision_instruction".into(), json!(i));
    }
    brief.insert(
        "fit_check".into(),
        fit.as_ref()
            .map(|f| f.content.clone())
            .unwrap_or(Value::Null),
    );
    Ok(DraftContext {
        deal,
        org,
        contact,
        language,
        tmpl,
        brief: Value::Object(brief),
    })
}

/// 23 Oct: what failed after the document existed: {stage: figures | cost_sheet | quotations,
/// error}. The text is kept; the person runs the missing part again from the proposal page.
pub fn partial_entry(stage: &str, error: &str) -> Value {
    json!({"stage": stage, "error": super::py_slice(error.trim(), 500)})
}

/// 23 Oct: the reason of a failure in plain words (the detail of an HTTP error, else the error itself).
pub fn stage_error(e: &ApiError) -> String {
    if super::governance::is_http_exception(e) {
        super::governance::http_detail(e).unwrap_or_default()
    } else {
        super::governance::plain_error(e)
    }
}

/// 23 Oct: "the cost sheet failed (reason); a figure failed (reason). Run it again from the proposal page.",
/// or None when nothing is missing.
pub fn partial_text(partial: &[Value]) -> Option<String> {
    if partial.is_empty() {
        return None;
    }
    let parts: Vec<String> = partial
        .iter()
        .map(|p| {
            let what = match p.get("stage").and_then(Value::as_str) {
                Some("figures") => "a figure",
                Some("cost_sheet") => "the cost sheet",
                Some("quotations") => "linking a quotation",
                _ => "a step",
            };
            let why = p.get("error").and_then(Value::as_str).unwrap_or("");
            format!("{what} failed ({why})")
        })
        .collect();
    Some(format!(
        "{}. Run it again from the proposal page.",
        parts.join("; ")
    ))
}

/// 23 Oct: the second half of [`draft_with`], also run by proposal.from_sections with the sections the chat or
/// The MCP client wrote. `data` has the draft's shape ({title, subtitle, sections: [{key, blocks}], phases});
/// the blocks become the document (created, or `into.reuse` drafted again), then the figures and (with
/// `cost_sheet`) the cost sheet are made. What fails after the document
/// exists is listed in `partial` and the text is kept. Commits.
pub async fn build_from_draft(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    ctx: &DraftContext,
    data: Value,
    into: DraftInto,
    run: &mut AgentRun,
    cost_sheet: bool,
) -> ApiResult<Value> {
    use crate::documents::core_ as core;
    let (language, deal, brief, tmpl) = (&ctx.language, &ctx.deal, &ctx.brief, &ctx.tmpl);
    let mut partial: Vec<Value> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut blocks: Vec<Value> = Vec::new();
    let mut figure_plan = super::proposal_visuals::DraftPlan::default(); // visuals V2
    let keys: Vec<Value> = tmpl
        .iter()
        .map(|s| s.get("key").cloned().unwrap_or(Value::Null))
        .collect();
    let data_sections: Vec<Value> = key(&data, "sections")?
        .as_array()
        .cloned()
        .unwrap_or_default();
    for s in &data_sections {
        let skey = key(s, "key")?;
        if !keys.contains(skey) {
            continue;
        }
        for b in key(s, "blocks")?.as_array().cloned().unwrap_or_default() {
            let heading = b.get("heading").cloned().unwrap_or(Value::Null);
            // rows arrive as JSON text since 22 Oct (an open "items" let the grammar fill rows with numbers);
            // a list is taken as it is
            let or_list = |k: &str| {
                let v = b.get(k).filter(|v| py_truthy(v)).cloned();
                match v {
                    Some(Value::String(t)) => serde_json::from_str::<Value>(&t)
                        .ok()
                        .filter(Value::is_array)
                        .unwrap_or_else(|| json!([])),
                    Some(v) => v,
                    None => json!([]),
                }
            };
            match key(&b, "type")?.as_str() {
                Some("text") => {
                    let md_in = b
                        .get("md")
                        .filter(|v| py_truthy(v))
                        .map(py_str)
                        .unwrap_or_default();
                    let (md, cut) = strip_accuracy(&md_in);
                    removed.extend(cut);
                    blocks.push(json!({"section": skey, "block_type": "text", "data": {"md": md, "heading": heading}}));
                }
                Some("kv") => {
                    blocks.push(json!({"section": skey, "block_type": "kv", "data": {"rows": or_list("rows"), "heading": heading}}));
                }
                Some("table") => {
                    blocks.push(json!({"section": skey, "block_type": "table",
                                       "data": {"columns": or_list("columns"), "rows": or_list("rows"), "heading": heading}}));
                }
                Some("coverage") => {
                    let mut rows = Vec::new();
                    for r in or_list("rows").as_array().cloned().unwrap_or_default() {
                        let Value::Object(mut r) = r else { continue };
                        let ok = r
                            .get("status")
                            .and_then(|v| v.as_str())
                            .is_some_and(|st| COVERAGE_STATUSES.contains(&st));
                        if !ok {
                            continue;
                        }
                        let notes_in = r
                            .get("notes")
                            .filter(|v| py_truthy(v))
                            .map(py_str)
                            .unwrap_or_default();
                        let (notes, cut) = strip_accuracy(&notes_in);
                        removed.extend(cut);
                        r.insert("notes".into(), Value::String(notes));
                        rows.push(Value::Object(r));
                    }
                    blocks.push(json!({"section": skey, "block_type": "coverage", "data": {"rows": rows, "heading": heading}}));
                }
                Some(bt @ ("chart" | "diagram")) if b.get("figure").is_some_and(py_truthy) => {
                    // visuals V2: drawn after the document exists (proposal_visuals::draft_figures)
                    let in_section = blocks.iter().filter(|x| &x["section"] == skey).count() as i64;
                    let size = if b.get("size").and_then(|v| v.as_str()) == Some("half") {
                        crate::visuals::spec::FigureSize::Half
                    } else {
                        crate::visuals::spec::FigureSize::Full
                    };
                    figure_plan
                        .figures
                        .push(super::proposal_visuals::DraftFigure {
                            section: py_str(skey),
                            position: in_section,
                            block_type: bt.to_string(),
                            brief: py_str(&b["figure"]),
                            caption: b.get("caption").filter(|v| py_truthy(v)).map(py_str),
                            size,
                        });
                }
                _ => {}
            }
        }
    }
    for s in tmpl {
        // the generated sections always exist
        for bt in ["commercial", "stamp"] {
            if listed(s.get("blocks").filter(|b| py_truthy(b)), bt) {
                blocks.push(json!({"section": s.get("key").cloned().unwrap_or(Value::Null), "block_type": bt, "data": {}}));
            }
        }
    }
    // order = {s["key"]: i}: a duplicated key keeps its LAST index. Stable sort, as Python's.
    let order_of = |k: &Value| -> usize { keys.iter().rposition(|x| x == k).unwrap_or(0) };
    blocks.sort_by_key(|b| order_of(&b["section"]));
    let mut cover = base_cover(db, ctx).await?;
    cover["accuracy_removed"] = json!(removed);
    // visuals V2: the phases behind the timeline (source erp proposal.phases)
    // (both arrive as JSON text since 22 Oct: see proposal_visuals::extend_draft_schema)
    if let Some(ph) = super::proposal_visuals::json_text_object(data.get("phases")).filter(|p| {
        p.get("tasks")
            .and_then(|t| t.as_array())
            .is_some_and(|t| !t.is_empty())
    }) {
        cover["phases"] = ph.clone();
        figure_plan.phases = Some(ph.clone());
    }
    let doc = match into.reuse {
        // fix-longapply: the same document again (a retry, a revision), never a second one
        Some(id) => {
            core::redraft(
                db,
                id,
                &py_str(key(&data, "title")?),
                &py_str(key(&data, "subtitle")?),
                language,
                cover,
                &blocks,
            )
            .await?
        }
        None => {
            core::create(
                db,
                core::CreateArgs {
                    user_id: actor.map(|a| a.id),
                    title: py_str(key(&data, "title")?),
                    subtitle: Some(py_str(key(&data, "subtitle")?)),
                    language: language.clone(),
                    deal: Some(deal),
                    blocks: Some(blocks),
                    origin: "nary".into(),
                    cover: Some(cover),
                    ..Default::default()
                },
            )
            .await?
        }
    };
    if let Some(card) = into.record_on {
        record_document(db, card, &doc).await?;
    }
    super::commit(db).await?;
    if FAIL_AFTER_DOCUMENT.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(ApiError::internal(anyhow::anyhow!(
            "the connection was cut"
        )));
    }
    // visuals V2: the timeline, the architecture diagram and the figures the draft asked for
    figure_plan.context = format!(
        "CUSTOMER BRIEF (JSON):\n{}\n\nDRAFTED SECTIONS (JSON):\n{}\n\nDEAL PRODUCTS:\n{}",
        super::py_slice(&dumps_unicode(brief)?, 6000),
        super::py_slice(&dumps_unicode(&Value::Array(data_sections.clone()))?, 12000),
        super::proposal_visuals::deal_products(db, Some(deal.id)).await?
    );
    // 23 Oct: a figure or the cost sheet that fails (any error) leaves the text as it is: listed in `partial`
    match super::proposal_visuals::draft_figures(db, &doc, &figure_plan, Some(&mut *run)).await {
        Ok(figure_lines) => {
            tracing::info!(doc = %doc.id, figures = ?figure_lines, "nary_proposal_figures");
            for l in &figure_lines {
                if let Some(label) = l.strip_suffix(": failed") {
                    partial.push(partial_entry(
                        "figures",
                        &format!("{label} could not be drawn"),
                    ));
                }
            }
        }
        Err(e) => {
            super::rollback(db).await?;
            tracing::warn!(doc = %doc.id, error = %stage_error(&e), "nary_proposal_figures_failed");
            partial.push(partial_entry("figures", &stage_error(&e)));
        }
    }
    if cost_sheet {
        let scope =
            dumps_unicode(&json!({"title": doc.title, "brief": brief, "sections": data_sections}))?;
        if let Err(e) = super::costing::estimate(db, &doc, &scope, Some(&mut *run)).await {
            if !super::governance::is_http_exception(&e) {
                super::rollback(db).await?;
            }
            tracing::warn!(doc = %doc.id, error = %stage_error(&e), "nary_cost_estimate_failed");
            partial.push(partial_entry("cost_sheet", &stage_error(&e)));
        }
    }
    let missing = partial_text(&partial);
    let full_number = doc.full_number.clone().unwrap_or_else(|| "None".into());
    let mut outcome = format!("proposal {full_number} drafted");
    if !removed.is_empty() {
        outcome += &format!("; {} accuracy statement(s) removed", removed.len());
    }
    if let Some(m) = &missing {
        outcome += &format!("; {m}");
    }
    super::runs::finish_run(db, run, "done", Some(&outcome), None).await?;
    super::commit(db).await?;
    let mut body =
        "Review and edit it, set the final prices on the cost sheet, then confirm.".to_string();
    if !removed.is_empty() {
        body += &format!(
            " Nary removed {} sentence(s) that stated an accuracy figure.",
            removed.len()
        );
    }
    let review = super::owner_channel::consult_proposals(db).await?;
    let mut facts = None;
    if review {
        let cost_sheet = super::costing::latest(db, doc.id).await?;
        let likely = cost_sheet.as_ref().map(|c| {
            let summary = if py_truthy(&c.content) {
                c.content.get("summary").cloned().unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            if py_truthy(&summary) {
                summary.get("likely").cloned().unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        });
        let likely = likely.unwrap_or(Value::Null);
        let assumptions: Vec<Value> = match brief.get("assumptions") {
            Some(Value::Array(a)) => a.clone(),
            _ => vec![],
        };
        facts = Some(
            json!({"document_id": doc.id.to_string(), "number": doc.full_number, "title": doc.title,
                            "assumptions": assumptions, "cost_likely": likely,
                            "cost_currency": cost_sheet.as_ref().map(|c| c.currency.clone())}),
        );
        let price = if py_truthy(&likely) {
            let g = |k: &str| py_str(likely.get(k).unwrap_or(&Value::Null));
            format!(
                "\nCost (likely): {} {}; at target margin {}, floor {}.",
                g("cost"),
                cost_sheet
                    .as_ref()
                    .map(|c| c.currency.clone())
                    .unwrap_or_else(|| "None".into()),
                g("price_at_target"),
                g("price_at_floor")
            )
        } else {
            String::new()
        };
        body = format!(
            "{full_number} for {}.{price}",
            py_str(brief.get("customer").unwrap_or(&Value::Null))
        );
        if !assumptions.is_empty() {
            let first: Vec<String> = assumptions.iter().take(6).map(py_str).collect();
            body += &format!("\nAssumptions: {}", first.join("; "));
        }
        body += "\nTell me what to change, or give me the final prices to prepare the quotation.";
    }
    // 23 Oct: what is missing, so the Owner knows what to run again
    if let Some(m) = &missing {
        body += &format!("\nNot done: {m}");
    }
    let title = format!("Proposal drafted: {}", doc.title);
    let link = format!("/sales/proposals/{}", doc.id);
    let doc_id = doc.id.to_string();
    let mut tell = super::owner_channel::Tell::new(
        &title,
        &body,
        &link,
        "sales_document",
        &doc_id,
        "nary_done",
    );
    tell.kind = if review { "proposal_review" } else { "alert" };
    tell.facts = facts;
    tell.ref_type = Some("sales_document");
    tell.ref_id = Some(doc.id);
    tell.resume = if review {
        Some(json!({"handler": "proposal.review", "document_id": doc_id}))
    } else {
        None
    };
    super::owner_channel::tell(db, tell).await?;
    super::commit(db).await?;
    let label = match &missing {
        Some(m) => format!("Proposal {full_number} drafted, but {m}"),
        None => format!("Proposal {full_number} drafted"),
    };
    Ok(
        json!({"id": doc.id.to_string(), "number": doc.full_number, "label": label, "partial": partial}),
    )
}

/// `confirm(db, doc, user, *, lines)`: someone who may create quotations (the route checks
/// sales:create_quotation) confirmed the proposal: Nary drafts the matching quotation (catalog lines and manual
/// lines at the confirmed prices) and links it. The quotation's own approval decides the price.
/// `lines`: [{product_id | name, quantity, unit_price?}]. Commits.
pub async fn confirm(
    db: &mut PgConnection,
    doc: &SalesDocument,
    user: Option<&AuthUser>,
    lines: &[Value],
) -> ApiResult<Value> {
    let _ = user;
    if !matches!(doc.status.as_str(), "draft" | "review") {
        return Err(ApiError::http(
            409,
            format!("A {} proposal can't be confirmed again", doc.status),
        ));
    }
    if lines.is_empty() {
        return Err(ApiError::http(
            400,
            "Give the quotation lines (catalog items and your final prices)",
        ));
    }
    let deal: Option<Deal> = match doc.deal_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM deals WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let Some(deal) = deal else {
        return Err(ApiError::http(
            400,
            "A proposal needs a deal before its quotation",
        ));
    };
    let currency = deal
        .currency
        .clone()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "USD".into());
    let mut items = Vec::new();
    for ln in lines {
        let get = |k: &str| ln.get(k).filter(|v| py_truthy(v));
        let quantity = get("quantity").map(py_str).unwrap_or_else(|| "1".into());
        let mut item = Map::new();
        item.insert("quantity".into(), json!(quantity));
        item.insert("discount_percentage".into(), json!("0"));
        item.insert("is_optional".into(), json!(get("optional").is_some()));
        if let Some(pid) = get("product_id") {
            item.insert("product_id".into(), pid.clone());
        } else {
            let name = get("name").map(py_str).unwrap_or_else(|| "Service".into());
            item.insert("name".into(), json!(super::py_slice(&name, 255)));
            item.insert("unit_price".into(), json!(py_str(key(ln, "unit_price")?)));
            item.insert(
                "description".into(),
                ln.get("description").cloned().unwrap_or(Value::Null),
            );
        }
        items.push(Value::Object(item));
    }
    let full_number = doc.full_number.clone().unwrap_or_else(|| "None".into());
    let card = super::replies::_propose(
        db,
        "quotation.create_draft",
        json!({
            "subject": super::py_slice(&doc.title, 200), "deal_id": deal.id.to_string(), "currency": currency,
            "customer_contact_id": doc.contact_id.map(|c| c.to_string()),
            "customer_org_id": doc.organization_id.map(|o| o.to_string()),
            "items": items, "internal_notes": format!("{} For proposal {full_number}.", super::quotes::NARY_NOTE),
            "proposal_id": doc.id.to_string(),
        }),
        None,
        None,
        Some(&format!("The quotation for proposal {full_number}: catalog lines and your final prices.")),
        None,
    )
    .await?;
    sqlx::query("UPDATE sales_documents SET status = 'review', updated_at = now() WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *db)
        .await?;
    super::commit(db).await?;
    Ok(
        json!({"card_id": card.as_ref().map(|c| c.id.to_string()), "status": card.as_ref().map(|c| c.status.clone())}),
    )
}

// ─── batch 2 T1: Solution Proposals from the chat (A1) ───────────────────────
//
// The Owner (WhatsApp, web chat or MCP) asks for a proposal with what they already know. The tools
// propose_solution_proposal and update_requirements make a card that shows the deal and the requirement sheet
// it will write; applying proposal.from_chat writes the sheet and runs the same `draft` as the automatic flow
// (proposal, figures, cost sheet); proposal.update_requirements only edits the sheet.

/// The keys a chat request may carry next to the checklist `fields`.
pub const SCOPE_LIST_KEYS: [&str; 2] = ["standard_items", "custom_items"];

/// Who may ask for a proposal from the chat: sales:create_quotation, or the Owner (Nary's approver).
pub async fn may_request(db: &mut PgConnection, user: &AuthUser) -> ApiResult<bool> {
    if user.is_superadmin || user.perms.contains("sales:create_quotation") {
        return Ok(true);
    }
    Ok(super::roles::approver_ids(db).await?.contains(&user.id))
}

fn clean_text(v: &Value, max: usize) -> Option<String> {
    let t = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if t.is_empty() {
        None
    } else {
        Some(super::py_slice(&t, max).to_string())
    }
}

fn clean_list(v: &Value, what: &str) -> Result<Vec<Value>, String> {
    match v {
        Value::Null => Ok(vec![]),
        Value::String(_) => Ok(clean_text(v, 500).map(Value::String).into_iter().collect()),
        Value::Array(a) => {
            let mut out = vec![];
            for x in a {
                match clean_text(x, 500) {
                    Some(t) => out.push(Value::String(t)),
                    None if x.is_null() => {}
                    None => return Err(format!("{what} must be a list of texts")),
                }
            }
            Ok(out)
        }
        _ => Err(format!("{what} must be a list of texts")),
    }
}

/// `clean_requirements(input, checklist)`: what a chat request may write on the sheet, checked. Keys:
/// fields {checklist key: text}, standard_items, custom_items, options [{name, description}],
/// assumptions, notes, language (en | id). Only the keys given are returned; Err says what is wrong.
pub fn clean_requirements(input: &Value, checklist: &[Value]) -> Result<Value, String> {
    let Value::Object(m) = input else {
        return Err("requirements must be an object".into());
    };
    let keys: Vec<String> = checklist
        .iter()
        .filter_map(|f| f.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut out = Map::new();
    for (k, v) in m {
        match k.as_str() {
            "fields" => {
                let Value::Object(f) = v else {
                    return Err("fields must be an object {checklist key: what is known}".into());
                };
                let mut fields = Map::new();
                for (fk, fv) in f {
                    if !keys.contains(fk) {
                        return Err(format!(
                            "Unknown requirement '{fk}'. The checklist keys are: {}",
                            keys.join(", ")
                        ));
                    }
                    if let Some(t) = clean_text(fv, 500) {
                        fields.insert(fk.clone(), Value::String(t));
                    }
                }
                out.insert("fields".into(), Value::Object(fields));
            }
            "standard_items" | "custom_items" | "assumptions" => {
                out.insert(k.clone(), Value::Array(clean_list(v, k)?));
            }
            // 1 Nov 2026: requirement flags (proposal.required_figures_by_requirement), e.g. network, process_flow
            "flags" => {
                let flags: Vec<Value> = clean_list(v, k)?
                    .into_iter()
                    .filter_map(|f| f.as_str().map(|s| json!(s.trim().to_lowercase())))
                    .collect();
                out.insert("flags".into(), Value::Array(flags));
            }
            "options" => {
                let Value::Array(a) = v else {
                    return Err("options must be a list of {name, description}".into());
                };
                let mut opts = vec![];
                for (i, o) in a.iter().enumerate() {
                    let default_name = format!("Option {}", (b'A' + (i as u8 % 26)) as char);
                    let (name, desc) = match o {
                        Value::String(_) => (default_name, clean_text(o, 1000)),
                        Value::Object(om) => (
                            om.get("name")
                                .and_then(|n| clean_text(n, 120))
                                .unwrap_or(default_name),
                            om.get("description").and_then(|d| clean_text(d, 1000)),
                        ),
                        _ => return Err("options must be a list of {name, description}".into()),
                    };
                    let Some(desc) = desc else {
                        return Err(format!("{name} needs a description"));
                    };
                    opts.push(json!({"name": name, "description": desc}));
                }
                out.insert("options".into(), Value::Array(opts));
            }
            "notes" => {
                out.insert(
                    "notes".into(),
                    clean_text(v, 2000).map(Value::String).unwrap_or(Value::Null),
                );
            }
            "language" => match v.as_str().map(str::trim) {
                Some(l @ "en") => {
                    out.insert("language".into(), json!(l));
                }
                _ => return Err("language must be en".into()),
            },
            // batch 4 D1: the customer files the requirements come from ([{file_id, name}], checked by the tool)
            "sources" => {
                let Value::Array(a) = v else {
                    return Err("sources must be a list of {file_id, name}".into());
                };
                let mut list = vec![];
                for x in a {
                    let id = x.get("file_id").and_then(|i| clean_text(i, 64));
                    let name = x.get("name").and_then(|n| clean_text(n, 255));
                    if let (Some(id), Some(name)) = (id, name) {
                        list.push(json!({"file_id": id, "name": name}));
                    }
                }
                out.insert("sources".into(), Value::Array(list));
            }
            other => {
                return Err(format!(
                    "Unknown key '{other}' in requirements. Use fields, standard_items, custom_items, options, assumptions, notes, language"
                ))
            }
        }
    }
    Ok(Value::Object(out))
}

/// `merged_sheet(current, edits, checklist)`: the sheet content after the chat's edits: checklist fields
/// set as known, scope keys (items, options) and assumptions replaced when given, coverage
/// recomputed. A deal without a sheet starts from an empty one.
pub fn merged_sheet(current: Option<&Value>, edits: &Value, checklist: &[Value]) -> Value {
    let mut content: Map<String, Value> = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    content
        .entry("reason")
        .or_insert_with(|| json!("asked in the chat"));
    let mut fields: Map<String, Value> = match content.get("fields") {
        Some(Value::Object(o)) => o.clone(),
        _ => Map::new(),
    };
    for f in checklist {
        if let Some(k) = f.get("key").and_then(Value::as_str) {
            fields
                .entry(k.to_string())
                .or_insert_with(|| json!({"known": false, "value": null}));
        }
    }
    if let Some(Value::Object(given)) = edits.get("fields") {
        for (k, v) in given {
            fields.insert(k.clone(), json!({"known": true, "value": v}));
        }
    }
    content.insert("fields".into(), Value::Object(fields));
    let mut scope: Map<String, Value> = match content.get("scope") {
        Some(Value::Object(o)) => o.clone(),
        _ => Map::new(),
    };
    for k in ["standard_items", "custom_items", "options"] {
        if let Some(v) = edits.get(k) {
            scope.insert(k.into(), v.clone());
        }
    }
    if !scope.is_empty() {
        content.insert("scope".into(), Value::Object(scope));
    }
    if let Some(a) = edits.get("assumptions") {
        content.insert("assumptions".into(), a.clone());
    }
    content.entry("assumptions").or_insert_with(|| json!([]));
    // 1 Nov 2026: flags (required figures) are replaced when given
    for k in ["notes", "language", "flags"] {
        if let Some(v) = edits.get(k) {
            content.insert(k.into(), v.clone());
        }
    }
    // batch 4 D1: the files cited are kept and added to (never dropped by a later edit)
    if let Some(Value::Array(given)) = edits.get("sources") {
        let mut list: Vec<Value> = match content.get("sources") {
            Some(Value::Array(a)) => a.clone(),
            _ => vec![],
        };
        for g in given {
            if !list.iter().any(|x| x.get("file_id") == g.get("file_id")) {
                list.push(g.clone());
            }
        }
        content.insert("sources".into(), Value::Array(list));
    }
    let cov = coverage(&Value::Object(content.clone()), checklist);
    content.insert("coverage".into(), json!(cov));
    Value::Object(content)
}

fn joined(v: Option<&Value>) -> Option<String> {
    let a = v?.as_array()?;
    let parts: Vec<String> = a.iter().map(py_str).filter(|t| !t.is_empty()).collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

/// `sheet_lines(content, checklist)`: the whole sheet as card lines, never cut.
pub fn sheet_lines(content: &Value, checklist: &[Value]) -> Vec<String> {
    let cov = content.get("coverage").cloned().unwrap_or(json!(0));
    let mut out = vec![format!("Requirement sheet ({}% known):", py_str(&cov))];
    for f in checklist {
        let k = f.get("key").and_then(Value::as_str).unwrap_or_default();
        let label = f.get("label").map(py_str).unwrap_or_else(|| k.to_string());
        let field = content.get("fields").and_then(|fs| fs.get(k));
        let known = field.and_then(|x| x.get("known")).is_some_and(py_truthy);
        let value = field
            .and_then(|x| x.get("value"))
            .filter(|v| py_truthy(v))
            .map(py_str);
        out.push(match (known, value) {
            (true, Some(v)) => format!("- {label}: {v}"),
            _ => format!("- {label}: not known yet (to be confirmed)"),
        });
    }
    let scope = content.get("scope");
    if let Some(t) = joined(scope.and_then(|s| s.get("standard_items"))) {
        out.push(format!("Standard items: {t}"));
    }
    if let Some(t) = joined(scope.and_then(|s| s.get("custom_items"))) {
        out.push(format!("Custom items: {t}"));
    }
    if let Some(Value::Array(opts)) = scope.and_then(|s| s.get("options")) {
        for o in opts {
            out.push(format!(
                "{}: {}",
                o.get("name").map(py_str).unwrap_or_default(),
                o.get("description").map(py_str).unwrap_or_default()
            ));
        }
    }
    if let Some(t) = joined(content.get("assumptions")) {
        out.push(format!("Assumptions: {t}"));
    }
    if let Some(n) = content.get("notes").filter(|v| py_truthy(v)) {
        out.push(format!("Notes: {}", py_str(n)));
    }
    // batch 4 D1: the customer files the sheet comes from
    if let Some(Value::Array(src)) = content.get("sources") {
        let names: Vec<String> = src
            .iter()
            .filter_map(|x| x.get("name").and_then(Value::as_str))
            .map(|n| format!("{n} (customer file)"))
            .collect();
        if !names.is_empty() {
            out.push(format!("Sources: {}", names.join("; ")));
        }
    }
    let lang = "English";
    out.push(format!("Language: {lang}"));
    out
}

/// The deal a chat request is about, with its organization's name ("" when none).
pub(crate) async fn deal_and_org(
    db: &mut PgConnection,
    deal_id: &str,
) -> ApiResult<(Deal, String)> {
    let Some(deal) = get_deal(db, deal_id).await? else {
        return Err(ApiError::http(404, "Deal not found"));
    };
    let org: Option<(String,)> = match deal.organization_id {
        Some(id) => {
            sqlx::query_as("SELECT name FROM organizations WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    Ok((deal, org.map(|o| o.0).unwrap_or_default()))
}

pub(crate) fn deal_label(deal: &Deal, org: &str) -> String {
    if org.is_empty() || deal.title.contains(org) {
        deal.title.clone()
    } else {
        format!("{} ({org})", deal.title)
    }
}

/// Writes the sheet: the newest one is updated, or a new one is made. Does not commit.
pub(crate) async fn write_sheet(
    // fix-longapply: pub(crate) for proposal_lifecycle
    db: &mut PgConnection,
    deal_id: Uuid,
    content: &Value,
    status: Option<&str>,
) -> ApiResult<Uuid> {
    if let Some(row) = sheet(db, deal_id).await? {
        sqlx::query(
            "UPDATE nary_insights SET content = $2, status = COALESCE($3, status), updated_at = now() WHERE id = $1",
        )
        .bind(row.id)
        .bind(content)
        .bind(status)
        .execute(&mut *db)
        .await?;
        return Ok(row.id);
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO nary_insights (id, kind, deal_id, content, status) VALUES ($1, 'requirements', $2, $3, $4)")
        .bind(id)
        .bind(deal_id)
        .bind(content)
        .bind(status.unwrap_or("ready"))
        .execute(&mut *db)
        .await?;
    Ok(id)
}

pub(crate) fn requirements_of(p: &Value) -> Value {
    p.get("requirements")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}

pub(crate) async fn check_requester(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
) -> ApiResult<()> {
    let actor = super::actions::need_actor(actor)?;
    if !may_request(db, actor).await? {
        return Err(ApiError::http(
            403,
            "Permission required: sales:create_quotation",
        ));
    }
    Ok(())
}

/// The sheet the card will write: (deal, org name, content after the edits). Err when the edits don't fit
/// the checklist.
pub async fn preview_sheet(
    db: &mut PgConnection,
    deal_id: &str,
    requirements: &Value,
) -> ApiResult<(Deal, String, Value)> {
    let fields = checklist(db).await?;
    let clean = clean_requirements(requirements, &fields).map_err(|e| ApiError::http(422, e))?;
    let (deal, org) = deal_and_org(db, deal_id).await?;
    let current = sheet(db, deal.id).await?.map(|r| r.content);
    let merged = merged_sheet(current.as_ref(), &clean, &fields);
    Ok((deal, org, merged))
}

/// The proposals already made for a deal (newest first): "PRP-001 (draft)".
async fn existing_proposals(db: &mut PgConnection, deal_id: Uuid) -> ApiResult<Vec<String>> {
    let rows: Vec<(Option<String>, String, String)> = sqlx::query_as(
        "SELECT full_number, number, status FROM sales_documents WHERE deal_id = $1 AND doc_type = 'proposal' \
         AND is_deleted = false ORDER BY created_at DESC",
    )
    .bind(deal_id)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(full, number, status)| format!("{} ({status})", full.unwrap_or(number)))
        .collect())
}

/// Describes proposal.from_chat: the deal, the whole sheet it writes, then what follows.
pub async fn describe_from_chat(db: &mut PgConnection, p: &Value) -> ApiResult<Value> {
    let deal_id = p.get("deal_id").map(py_str).unwrap_or_default();
    let (deal, org, merged) = preview_sheet(db, &deal_id, &requirements_of(p)).await?;
    let fields = checklist(db).await?;
    let mut lines = sheet_lines(&merged, &fields);
    let made = existing_proposals(db, deal.id).await?;
    if !made.is_empty() {
        lines.push(format!(
            "This deal already has: {}. A separate new proposal is drafted next to it (asked for).",
            made.join(", ")
        ));
    }
    lines.push(
        "Then: Nary drafts the proposal from this sheet, makes its figures and the internal cost sheet; you review it in Sales > Proposals.".into(),
    );
    Ok(
        json!({"label": format!("Solution Proposal for {}", deal_label(&deal, &org)), "lines": lines}),
    )
}

/// Describes proposal.update_requirements: the deal and the whole sheet after the change.
pub async fn describe_update(db: &mut PgConnection, p: &Value) -> ApiResult<Value> {
    let deal_id = p.get("deal_id").map(py_str).unwrap_or_default();
    let (deal, org, merged) = preview_sheet(db, &deal_id, &requirements_of(p)).await?;
    let fields = checklist(db).await?;
    let mut lines = sheet_lines(&merged, &fields);
    lines.push("Only the sheet changes: the proposal text stays as it is until it is drafted again or edited.".into());
    Ok(
        json!({"label": format!("Update the requirement sheet of {}", deal_label(&deal, &org)), "lines": lines}),
    )
}

// fix-longapply: one document per card. The id is stored on the card as soon as the document exists.

/// Stores the document a card made on it (`result.document_id`), committed with the document.
pub async fn record_document(
    db: &mut PgConnection,
    card_id: Uuid,
    doc: &SalesDocument,
) -> ApiResult<()> {
    sqlx::query("UPDATE agent_pending_actions SET result = $2, updated_at = now() WHERE id = $1")
        .bind(card_id)
        .bind(json!({"document_id": doc.id.to_string(), "number": doc.full_number}))
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// The proposal an earlier try made for this deal and never finished: the card's own (a retried card), else
/// the newest failed proposal.from_chat card's ("ask Nary to try again" makes a new card). Only while that
/// proposal is still a draft.
pub async fn unfinished_document(
    db: &mut PgConnection,
    card_id: Option<Uuid>,
    deal_id: Uuid,
) -> ApiResult<Option<Uuid>> {
    let found: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT result::jsonb ->> 'document_id' FROM agent_pending_actions \
         WHERE action IN ('proposal.from_chat', 'proposal.from_sections') AND entity_id = $2 \
           AND (id = $1 OR status = 'failed') \
           AND result IS NOT NULL AND jsonb_typeof(result::jsonb) = 'object' AND result::jsonb ? 'document_id' \
         ORDER BY (id IS NOT DISTINCT FROM $1) DESC, updated_at DESC LIMIT 1",
    )
    .bind(card_id)
    .bind(deal_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(id) = found
        .and_then(|r| r.0)
        .and_then(|s| Uuid::parse_str(&s).ok())
    else {
        return Ok(None);
    };
    let live: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM sales_documents WHERE id = $1 AND deal_id = $2 AND is_deleted = false \
         AND status IN ('draft', 'review')",
    )
    .bind(id)
    .bind(deal_id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(live.map(|r| r.0))
}

/// The deal's current proposal (not rejected, superseded or deleted), newest first: a second one is not made
/// for it unless asked (revise it instead).
pub async fn live_proposal(
    db: &mut PgConnection,
    deal_id: Uuid,
) -> ApiResult<Option<SalesDocument>> {
    Ok(sqlx::query_as(
        "SELECT * FROM sales_documents WHERE deal_id = $1 AND is_deleted = false \
         AND status NOT IN ('rejected', 'superseded') ORDER BY created_at DESC LIMIT 1",
    )
    .bind(deal_id)
    .fetch_optional(&mut *db)
    .await?)
}

/// The refusal a second proposal for a deal gets.
pub fn exists_refusal(d: &SalesDocument) -> String {
    format!(
        "{} exists for this deal ({}); use propose_proposal_revision to change it (document_id {}), or pass \
         new_proposal: true only when the person asked for a separate new proposal.",
        d.full_number.clone().unwrap_or_else(|| d.number.clone()),
        d.status,
        d.id
    )
}

/// Applies proposal.from_chat: write the sheet, then draft (proposal, figures, cost sheet) exactly as
/// the automatic flow. Returns the document id and its preview link; the cost only to cost viewers. Commits.
pub async fn apply_from_chat(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    check_requester(db, actor).await?;
    let deal_id = p.get("deal_id").map(py_str).unwrap_or_default();
    let (deal, _, merged) = preview_sheet(db, &deal_id, &requirements_of(p)).await?;
    // fix-longapply: a retry redrafts what the cut try made; a deal with a proposal is revised, not doubled
    let card = super::wa_send::current_card();
    let reuse = unfinished_document(db, card, deal.id).await?;
    if reuse.is_none() && !p.get("new_proposal").is_some_and(py_truthy) {
        if let Some(d) = live_proposal(db, deal.id).await? {
            return Err(ApiError::http(409, exists_refusal(&d)));
        }
    }
    write_sheet(db, deal.id, &merged, Some("ready")).await?;
    super::commit(db).await?;
    let out = draft_with(
        db,
        actor,
        &deal.id.to_string(),
        DraftInto {
            reuse,
            record_on: card,
            instruction: None,
        },
    )
    .await?;
    let doc_id = out.get("id").map(py_str).unwrap_or_default();
    let number = out
        .get("number")
        .filter(|v| py_truthy(v))
        .map(py_str)
        .unwrap_or_else(|| doc_id.clone());
    let link = format!("/sales/proposals/{doc_id}");
    let mut result = Map::new();
    // 23 Oct: the label never claims what failed
    let partial: Vec<Value> = out
        .get("partial")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let label = match partial_text(&partial) {
        Some(m) => format!("Proposal {number} drafted: {link} (document_id {doc_id}); but {m}"),
        None => format!(
            "Proposal {number} drafted with its figures and cost sheet: {link} (document_id {doc_id})"
        ),
    };
    result.insert("label".into(), json!(label));
    result.insert("partial".into(), Value::Array(partial));
    result.insert("document_id".into(), json!(doc_id));
    result.insert(
        "number".into(),
        out.get("number").cloned().unwrap_or(Value::Null),
    );
    result.insert("link".into(), json!(link));
    if actor.is_some_and(crate::ai::tools::catalog::can_see_costs) {
        if let Ok(id) = Uuid::parse_str(&doc_id) {
            if let Some(c) = super::costing::latest(db, id).await? {
                let summary = c
                    .content
                    .get("summary")
                    .and_then(|s| s.get("likely"))
                    .cloned()
                    .unwrap_or(Value::Null);
                result.insert(
                    "cost_sheet".into(),
                    json!({"currency": c.currency, "likely": summary}),
                );
            }
        }
    }
    Ok(Value::Object(result))
}

/// Applies proposal.update_requirements: only the sheet. Commits.
pub async fn apply_update(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    check_requester(db, actor).await?;
    let deal_id = p.get("deal_id").map(py_str).unwrap_or_default();
    let (deal, org, merged) = preview_sheet(db, &deal_id, &requirements_of(p)).await?;
    let id = write_sheet(db, deal.id, &merged, None).await?;
    super::commit(db).await?;
    Ok(json!({
        "label": format!("Requirement sheet of {} updated ({}% known)", deal_label(&deal, &org),
                         py_str(merged.get("coverage").unwrap_or(&json!(0)))),
        "insight_id": id.to_string(),
    }))
}

// ─── 1 Nov 2026: direct authoring (cover, requirement sheet replaced as given, required figures) ─────────────

/// The cover a new proposal gets (Nary's drafter and proposal_create): prepared for the organization (else the
/// contact's first name), attention of the contact with their email, prepared by documents.proposal.prepared_by
/// in the proposal's language.
pub async fn base_cover(db: &mut PgConnection, ctx: &DraftContext) -> ApiResult<Value> {
    let (language, org, contact) = (&ctx.language, &ctx.org, &ctx.contact);
    let prepared_all = require_json(db, "documents.proposal.prepared_by", WHERE).await?;
    let prepared_by = if py_truthy(&prepared_all) {
        prepared_all
            .get(language.as_str())
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let attention = match contact {
        Some(c) => {
            let names: Vec<String> = [Some(c.first_name.clone()), c.last_name.clone()]
                .into_iter()
                .flatten()
                .filter(|x| !x.is_empty())
                .collect();
            let mut a = names.join(" ");
            if let Some(e) = c.email.as_ref().filter(|e| !e.is_empty()) {
                a += &format!(" ({e})");
            }
            a
        }
        None => String::new(),
    };
    let prepared_for = match (org, contact) {
        (Some(o), _) => o.name.clone(),
        (None, Some(c)) => c.first_name.clone(),
        (None, None) => String::new(),
    };
    Ok(json!({"prepared_for": prepared_for, "attention": attention, "prepared_by": prepared_by}))
}

/// The sheet content written as given (proposal_set_requirements replace=true): no earlier field, scope item,
/// assumption, note or flag survives unless given again (a stale sheet never leaks into the proposal); the
/// checklist fields not given are unknown; coverage is recomputed.
pub fn replaced_sheet(given: &Value, checklist: &[Value]) -> Value {
    let mut content = Map::new();
    content.insert("reason".into(), json!("written as given (replaced)"));
    let mut fields = Map::new();
    for f in checklist {
        if let Some(k) = f.get("key").and_then(Value::as_str) {
            fields.insert(k.to_string(), json!({"known": false, "value": null}));
        }
    }
    if let Some(Value::Object(g)) = given.get("fields") {
        for (k, v) in g {
            fields.insert(k.clone(), json!({"known": true, "value": v}));
        }
    }
    content.insert("fields".into(), Value::Object(fields));
    let mut scope = Map::new();
    for k in ["standard_items", "custom_items", "options"] {
        if let Some(v) = given.get(k) {
            scope.insert(k.into(), v.clone());
        }
    }
    if !scope.is_empty() {
        content.insert("scope".into(), Value::Object(scope));
    }
    content.insert(
        "assumptions".into(),
        given
            .get("assumptions")
            .cloned()
            .unwrap_or_else(|| json!([])),
    );
    for k in ["notes", "language", "sources", "flags"] {
        if let Some(v) = given.get(k) {
            content.insert(k.into(), v.clone());
        }
    }
    let cov = coverage(&Value::Object(content.clone()), checklist);
    content.insert("coverage".into(), json!(cov));
    Value::Object(content)
}

/// Writes a deal's requirement sheet: replaced as given (`replace`) or merged with the current one (the
/// update_requirements card's rule; given flags replace the earlier ones). Returns (insight id, content).
/// Err 422 when the sheet does not fit the checklist. Does not commit.
pub async fn set_requirements(
    db: &mut PgConnection,
    deal_id: Uuid,
    given: &Value,
    replace: bool,
) -> ApiResult<(Uuid, Value)> {
    let fields = checklist(db).await?;
    let clean = clean_requirements(given, &fields).map_err(|e| ApiError::http(422, e))?;
    let content = if replace {
        replaced_sheet(&clean, &fields)
    } else {
        let current = sheet(db, deal_id).await?.map(|r| r.content);
        merged_sheet(current.as_ref(), &clean, &fields)
    };
    let id = write_sheet(db, deal_id, &content, None).await?;
    Ok((id, content))
}

/// The figure kinds a required-figures rule may name.
pub const FIGURE_KINDS: [&str; 4] = ["timeline", "architecture", "network", "process"];

/// Whether a sheet raises a requirement flag: its `flags` list, a true `scope.<flag>`, or a known checklist
/// field `fields.<flag>`.
fn sheet_raises(content: &Value, flag: &str) -> bool {
    let listed = content
        .get("flags")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|f| f.as_str() == Some(flag)));
    let scoped = content
        .get("scope")
        .and_then(|s| s.get(flag))
        .is_some_and(py_truthy);
    let field = content
        .get("fields")
        .and_then(|f| f.get(flag))
        .and_then(|f| f.get("known"))
        .is_some_and(py_truthy);
    listed || scoped || field
}

/// The figure kinds a proposal must have before it can be submitted for approval:
/// proposal.visuals.default_figures (always) plus the kinds proposal.required_figures_by_requirement names for
/// the flags the deal's requirement sheet raises. A setting naming an unknown kind is a clear error.
pub async fn required_kinds(db: &mut PgConnection, doc: &SalesDocument) -> ApiResult<Vec<String>> {
    let kind_ok = |k: &str| FIGURE_KINDS.contains(&k);
    let defaults = require_json(db, "proposal.visuals.default_figures", WHERE).await?;
    let mut out: Vec<String> = Vec::new();
    for v in defaults.as_array().cloned().unwrap_or_default() {
        let Some(k) = v.as_str() else { continue };
        if !kind_ok(k) {
            return Err(SettingError::new(
                "proposal.visuals.default_figures",
                &format!("names '{k}': the figures are {}", FIGURE_KINDS.join(", ")),
                WHERE,
            )
            .into());
        }
        if !out.iter().any(|x| x == k) {
            out.push(k.to_string());
        }
    }
    let map = require_json(db, "proposal.required_figures_by_requirement", WHERE).await?;
    let Some(map) = map.as_object() else {
        return Err(SettingError::new(
            "proposal.required_figures_by_requirement",
            "must be an object {requirement flag: figure kind}",
            WHERE,
        )
        .into());
    };
    let content = match doc.deal_id {
        Some(d) => sheet(db, d).await?.map(|r| r.content),
        None => None,
    };
    for (flag, kind) in map {
        let Some(k) = kind.as_str().filter(|k| kind_ok(k)) else {
            return Err(SettingError::new(
                "proposal.required_figures_by_requirement",
                &format!(
                    "maps '{flag}' to {kind}: the figures are {}",
                    FIGURE_KINDS.join(", ")
                ),
                WHERE,
            )
            .into());
        };
        if content.as_ref().is_some_and(|c| sheet_raises(c, flag)) && !out.iter().any(|x| x == k) {
            out.push(k.to_string());
        }
    }
    Ok(out)
}

/// The required-figures kind a block shows, if any: a chart's gantt or milestones is the timeline, a diagram
/// its kind, a picture placed by your AI client or Nary the `figure_kind` it was given.
pub fn block_figure_kind(block_type: &str, data: &Value) -> Option<String> {
    let spec_kind = data
        .get("spec")
        .and_then(|s| s.get("kind"))
        .and_then(Value::as_str);
    match block_type {
        "chart" => match spec_kind {
            Some("gantt" | "milestones") => Some("timeline".into()),
            Some(k) => Some(k.to_string()),
            None => None,
        },
        "diagram" => spec_kind.map(str::to_string),
        "image" => match data.get("figure_kind").and_then(Value::as_str) {
            Some(k) if !k.is_empty() => Some(k.to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a figure or picture block is usable: Ok(()) when its image is stored (a ready asset) and its check
/// did not fail; Err(reason) otherwise.
pub async fn figure_state(
    db: &mut PgConnection,
    block_type: &str,
    data: &Value,
) -> ApiResult<Result<(), String>> {
    let key = if block_type == "image" {
        "asset_id"
    } else {
        "png_asset_id"
    };
    let id = data
        .get(key)
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok());
    let Some(id) = id else {
        return Ok(Err("no image stored".into()));
    };
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT status, storage_key FROM sales_document_assets WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    match row {
        Some((s, Some(k))) if s == "ready" && !k.is_empty() => {}
        Some((s, _)) => return Ok(Err(format!("its image is not stored ({s})"))),
        None => return Ok(Err("its image is missing".into())),
    }
    if let Some(c) = data.get("checked").filter(|c| c.is_object()) {
        if c.get("ok") == Some(&json!(false)) {
            let notes = match c.get("notes") {
                Some(Value::Array(a)) => a.iter().map(py_str).collect::<Vec<_>>().join("; "),
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            return Ok(Err(if notes.is_empty() {
                "its vision check failed".into()
            } else {
                format!("its vision check failed: {notes}")
            }));
        }
    }
    Ok(Ok(()))
}

/// `required_figures(db, doc)`: {required, present: [{kind, block_id}], missing: [kind], failed: [{kind,
/// block_id, reason}], ok}. A kind is present when one of its blocks has its image stored and a check that
/// did not fail; failed when it has blocks but none usable; missing when it has none.
pub async fn required_figures(db: &mut PgConnection, doc: &SalesDocument) -> ApiResult<Value> {
    let required = required_kinds(db, doc).await?;
    let blocks = crate::documents::core_::blocks_of(db, doc.id).await?;
    let mut kinds: Vec<(String, Uuid, Result<(), String>)> = Vec::new();
    for b in &blocks {
        if !matches!(b.block_type.as_str(), "chart" | "diagram" | "image") {
            continue;
        }
        let Some(kind) = block_figure_kind(&b.block_type, &b.data) else {
            continue;
        };
        let state = figure_state(db, &b.block_type, &b.data).await?;
        kinds.push((kind, b.id, state));
    }
    let (mut present, mut missing, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    for k in &required {
        let mine: Vec<&(String, Uuid, Result<(), String>)> =
            kinds.iter().filter(|(kk, _, _)| kk == k).collect();
        if let Some((_, id, _)) = mine.iter().find(|(_, _, s)| s.is_ok()) {
            present.push(json!({"kind": k, "block_id": id.to_string()}));
        } else if mine.is_empty() {
            missing.push(json!(k));
        } else {
            for (_, id, s) in mine {
                failed.push(json!({"kind": k, "block_id": id.to_string(),
                                   "reason": s.clone().err().unwrap_or_default()}));
            }
        }
    }
    let ok = missing.is_empty() && failed.is_empty();
    Ok(
        json!({"required": required, "present": present, "missing": missing, "failed": failed, "ok": ok}),
    )
}

/// The words of a figure kind ("network diagram").
pub fn figure_kind_words(kind: &str) -> String {
    match kind {
        "timeline" => "timeline (gantt)".into(),
        k => format!("{k} diagram"),
    }
}

/// The approval gate (approvals::engine::submit): why a Solution Proposal can't be submitted yet, one line per
/// missing or failed required figure; empty when it can.
pub async fn figures_refusal(db: &mut PgConnection, doc_id: Uuid) -> ApiResult<Vec<String>> {
    let doc: Option<SalesDocument> =
        sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1 AND is_deleted = false")
            .bind(doc_id)
            .fetch_optional(&mut *db)
            .await?;
    let Some(doc) = doc.filter(|d| d.doc_type == "solution_proposal") else {
        return Ok(vec![]);
    };
    let r = required_figures(db, &doc).await?;
    let mut out = Vec::new();
    for m in r["missing"].as_array().cloned().unwrap_or_default() {
        out.push(format!(
            "Required figure missing: {} (add it before submitting)",
            figure_kind_words(m.as_str().unwrap_or(""))
        ));
    }
    for f in r["failed"].as_array().cloned().unwrap_or_default() {
        out.push(format!(
            "Required figure not usable: {} (block {}): {}",
            figure_kind_words(f["kind"].as_str().unwrap_or("")),
            f["block_id"].as_str().unwrap_or(""),
            f["reason"].as_str().unwrap_or("")
        ));
    }
    Ok(out)
}
