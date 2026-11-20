//! Port of backend/app/services/nary/negotiation.py (agent N6).
//!
//! Negotiation hints for the Owner only (Nary Phase 3 item B).
//!
//! When a customer pushes back, Opus (job nary.negotiation) writes private hints from: the value-first
//! answers per objection (nary.negotiation.scripts), the volume tier the deal's lines would allow
//! (a private lever, never offered by Nary), the margin of each line against the target
//! and floor (margins.rs), and the competitor checklist. Internal context only: the hints are a
//! notification to the Owner and an internal note on the deal, never part of a customer prompt.

use granary_compat::pyjson::{self, DumpOpts, PyValue};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_json;
use granary_db::rows::crm::Deal;
use granary_db::rows::nary::AgentRun;
use once_cell::sync::Lazy;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::margins::{self, s};

pub const WHERE: &str = "Nary & AI → Negotiation & contract";

pub static SCHEMA: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(r#"{"type": "object", "properties": {"objection": {"type": "string"}, "hints": {"type": "array", "items": {"type": "string"}}, "room": {"type": "string"}, "watch_out": {"type": "array", "items": {"type": "string"}}}, "required": ["objection", "hints", "room", "watch_out"], "additionalProperties": false}"#).unwrap()
});

pub const REQUIRED: [&str; 4] = ["objection", "hints", "room", "watch_out"];

pub const HINT_RULES: &str =
    "The customer pushed back. Write private hints for the Owner: name the \
objection, 2 to 4 value-first moves (from the scripts), how much room \
there is (the tier available and the margin above the floor, per line), \
and what to watch out for (a line under its floor, a comparison that \
isn't like for like). These hints never go to the customer.";

/// `json.dumps(v, ensure_ascii=False, default=str)`.
pub fn dumps_unicode(v: &Value) -> ApiResult<String> {
    let opts = DumpOpts {
        ensure_ascii: false,
        default_str: true,
        ..DumpOpts::default()
    };
    pyjson::dumps(&PyValue::from(v), &opts).map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))
}

/// `str(x or "")` for an optional Decimal column (0 is falsy too).
fn str_or_empty(d: Option<rust_decimal::Decimal>) -> String {
    match d {
        Some(d) if !d.is_zero() => s(d),
        _ => String::new(),
    }
}

/// `facts(db, deal)`: the internal facts the hints are written from.
pub async fn facts(db: &mut PgConnection, deal: &Deal) -> ApiResult<Value> {
    let quotation =
        super::deals::latest_quotation(db, deal.id, &["draft", "sent", "accepted"]).await?;
    let mut out = Map::new();
    out.insert(
        "deal".into(),
        json!({"title": deal.title, "stage": deal.stage, "value": str_or_empty(deal.value), "currency": deal.currency}),
    );
    out.insert(
        "scripts".into(),
        require_json(db, "nary.negotiation.scripts", WHERE).await?,
    );
    out.insert(
        "competitor_checklist".into(),
        require_json(db, "nary.negotiation.competitor_checklist", WHERE).await?,
    );
    if let Some(q) = quotation {
        let number = q
            .full_number
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| q.number.clone());
        out.insert(
            "quotation".into(),
            json!({"number": number, "total": str_or_empty(q.total), "currency": q.currency}),
        );
        out.insert("margins".into(), margins::quotation_report(db, &q).await?);
    }
    Ok(Value::Object(out))
}

/// `hint(db, deal, transcript, *, run=None)`: {"insight_id"} or None. Commits.
pub async fn hint(
    db: &mut PgConnection,
    deal: &Deal,
    transcript: &str,
    mut run: Option<&mut AgentRun>,
) -> ApiResult<Option<Value>> {
    let system = super::persona::system_prompt("staff", HINT_RULES, "", "");
    let content = format!(
        "FACTS (JSON, internal):\n{}\n\nCONVERSATION:\n{}",
        dumps_unicode(&facts(db, deal).await?)?,
        super::llm::information_default("customer", transcript)
    );
    let data = super::llm::ask_json(
        db,
        "nary.negotiation",
        &system,
        &content,
        &SCHEMA,
        run.as_deref_mut(),
        None,
        None,
    )
    .await?;
    let Some(data) = data else {
        return Ok(None);
    };
    let mut content = Map::new();
    for k in REQUIRED {
        content.insert(k.into(), data.get(k).cloned().unwrap_or(Value::Null));
    }
    let content = Value::Object(content);
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO nary_insights (id, kind, deal_id, content, status, model, run_id) \
         VALUES ($1, 'negotiation', $2, $3, 'draft', $4, $5)",
    )
    .bind(id)
    .bind(deal.id)
    .bind(&content)
    .bind(data.get("_model").and_then(|m| m.as_str()))
    .bind(run.as_ref().map(|r| r.id))
    .execute(&mut *db)
    .await?;
    let hints: Vec<String> = match &content["hints"] {
        Value::Array(a) => a.iter().map(py_str).collect(),
        Value::Null => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "TypeError: 'NoneType' object is not iterable"
            )))
        }
        // Iterating a str gives its characters; any other non-list is a TypeError in Python.
        Value::String(t) => t.chars().map(|c| c.to_string()).collect(),
        _ => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "TypeError: object is not iterable"
            )))
        }
    };
    let body = format!(
        "{}\n{}\nRoom: {}",
        py_str(&content["objection"]),
        hints
            .iter()
            .map(|h| format!("- {h}"))
            .collect::<Vec<_>>()
            .join("\n"),
        py_str(&content["room"])
    );
    let title = format!("Negotiation hint: {}", deal.title);
    let link = format!("/crm/deals/{}", deal.id);
    let entity_id = deal.id.to_string();
    let mut tell =
        super::owner_channel::Tell::new(&title, &body, &link, "deal", &entity_id, "nary_hint");
    tell.facts = Some(content.clone());
    tell.ref_type = Some("deal");
    tell.ref_id = Some(deal.id);
    super::owner_channel::tell(db, tell).await?;
    super::commit(db).await?;
    Ok(Some(json!({"insight_id": id.to_string()})))
}

/// `str(x)` in an f-string.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        other => pyjson::str_value(&PyValue::from(other)),
    }
}
