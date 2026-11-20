//! Port of backend/app/services/ai/records.py: AI record brief, a short read on where a CRM relationship
//! stands and what to do next.
//!
//! Same shape as `scoring.py`: gather the record plus its timeline into one prompt, ask for strict JSON,
//! parse with the shared `extract_json`. Runs only on explicit request (the Refresh button).
//!
//! The success dict's "model" is the model the provider says it used, else the configured one (batch 2: it
//! named an undefined `model` before, a NameError on every brief).

use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{str_value, PyValue};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_db::rows::crm::{Contact, Deal, Lead, Organization};

use crate::ai::base::{AiError, ChatRequest};
use crate::ai::registry::{
    extract_json, feature_enabled, get_ai_config, refusal_message, use_ai, UseAiOptions,
};

pub const _SYSTEM: &str = concat!(
    "You are a B2B sales analyst for the company. ",
    "Read the CRM record and its interaction history, then brief the account owner. ",
    "Base every statement on the evidence given; never invent facts, numbers or names. ",
    "If the history is thin, say so rather than padding. Reply in the language the ",
    "conversation history mostly uses (English unless it is clearly another language).\n",
    "Respond with ONLY a JSON object: ",
    "{\"summary\": \"<2-4 sentences on where this relationship stands>\", ",
    "\"next_best_action\": \"<one concrete next step the owner should take>\", ",
    "\"signals\": [\"<short positive evidence point>\", ...], ",
    "\"risks\": [\"<short risk or blocker>\", ...]}",
);

/// One CRM record as `_load` returns it (`_MODELS[object_type]`).
#[derive(Debug, Clone)]
pub enum Record {
    Organization(Organization),
    Contact(Contact),
    Lead(Lead),
    Deal(Deal),
}

impl Record {
    pub fn id(&self) -> Uuid {
        match self {
            Record::Organization(r) => r.id,
            Record::Contact(r) => r.id,
            Record::Lead(r) => r.id,
            Record::Deal(r) => r.id,
        }
    }

    /// `getattr(rec, "notes", None)` (leads have no notes column).
    pub fn notes(&self) -> Option<&str> {
        match self {
            Record::Organization(r) => r.notes.as_deref(),
            Record::Contact(r) => r.notes.as_deref(),
            Record::Lead(_) => None,
            Record::Deal(r) => r.notes.as_deref(),
        }
    }

    pub fn custom(&self) -> &Value {
        match self {
            Record::Organization(r) => &r.custom,
            Record::Contact(r) => &r.custom,
            Record::Lead(r) => &r.custom,
            Record::Deal(r) => &r.custom,
        }
    }

    /// The table of this record.
    pub fn table(&self) -> &'static str {
        match self {
            Record::Organization(_) => "organizations",
            Record::Contact(_) => "contacts",
            Record::Lead(_) => "leads",
            Record::Deal(_) => "deals",
        }
    }
}

fn opt(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "None".into())
}

fn nonempty(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `describe_record(object_type, rec)`.
pub fn describe_record(object_type: &str, rec: &Record) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    match (object_type, rec) {
        ("organization", Record::Organization(r)) => {
            parts.push(format!("Organization: {}", r.name));
            let emp = r.employee_count.filter(|n| *n != 0).map(|n| n.to_string());
            for (label, val) in [
                ("Industry", nonempty(&r.industry).map(str::to_string)),
                ("Country", nonempty(&r.country).map(str::to_string)),
                ("City", nonempty(&r.city).map(str::to_string)),
                ("Website", nonempty(&r.website).map(str::to_string)),
                ("Employees", emp),
            ] {
                if let Some(v) = val {
                    parts.push(format!("{label}: {v}"));
                }
            }
            if let Some(rev) = r.annual_revenue.filter(|d| !d.is_zero()) {
                parts.push(format!(
                    "Annual revenue: {} {}",
                    r.revenue_currency
                        .clone()
                        .filter(|c| !c.is_empty())
                        .unwrap_or_default(),
                    rev
                ));
            }
            if let Some(t) = r.type_.as_ref().filter(|t| !t.is_empty()) {
                parts.push(format!("Relationship: {}", t.join(", ")));
            }
        }
        ("contact", Record::Contact(r)) => {
            let line = format!(
                "Contact: {} {}",
                r.first_name,
                r.last_name.clone().unwrap_or_default()
            );
            parts.push(granary_compat::text::pystr::strip(&line).to_string());
            for (label, val) in [
                ("Position", nonempty(&r.position)),
                ("Department", nonempty(&r.department)),
                ("Email", nonempty(&r.email)),
                ("Phone", nonempty(&r.phone)),
            ] {
                if let Some(v) = val {
                    parts.push(format!("{label}: {v}"));
                }
            }
        }
        ("lead", Record::Lead(r)) => {
            parts.push(format!(
                "Lead: {}",
                nonempty(&r.title).unwrap_or("(no title)")
            ));
            parts.push(format!(
                "Status: {} · Priority: {} · Source: {}",
                opt(&r.status),
                opt(&r.priority),
                r.source
            ));
            if let Some(v) = r.estimated_value.filter(|d| !d.is_zero()) {
                parts.push(format!("Estimated value: {} {v}", opt(&r.currency)));
            }
            if let Some(p) = r.interested_products.as_ref().filter(|p| !p.is_empty()) {
                parts.push(format!("Interested in: {}", p.join(", ")));
            }
        }
        ("deal", Record::Deal(r)) => {
            parts.push(format!("Deal: {}", r.title));
            parts.push(format!(
                "Stage: {} · Probability: {}%",
                r.stage,
                r.probability
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "None".into())
            ));
            if let Some(v) = r.value.filter(|d| !d.is_zero()) {
                parts.push(format!("Value: {} {v}", opt(&r.currency)));
            }
            if let Some(d) = r.expected_close_date {
                parts.push(format!("Expected close: {}", d.format("%Y-%m-%d")));
            }
        }
        _ => {}
    }
    if let Some(n) = rec.notes().filter(|n| !n.is_empty()) {
        parts.push(format!("Notes: {}", take_chars(n, 800)));
    }
    if let Some(custom) = rec.custom().as_object().filter(|m| !m.is_empty()) {
        // `v not in (None, "", [])`
        let kv: Vec<String> = custom
            .iter()
            .filter(|(_, v)| {
                !(v.is_null()
                    || v.as_str() == Some("")
                    || v.as_array().is_some_and(|a| a.is_empty()))
            })
            .map(|(k, v)| format!("{k}={}", str_value(&PyValue::from(v))))
            .collect();
        if !kv.is_empty() {
            parts.push(format!("Custom attributes: {}", kv.join(", ")));
        }
    }
    parts
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(crate::crm::attributes::py::truthy)
}

fn s(v: Option<&Value>) -> String {
    str_value(&PyValue::from(v.unwrap_or(&Value::Null)))
}

/// `describe_timeline(entries)`.
pub fn describe_timeline(entries: &[Value]) -> Vec<String> {
    if entries.is_empty() {
        return vec!["\nNo recorded interactions yet.".into()];
    }
    let mut out = vec!["\nInteraction history (most recent first):".to_string()];
    for e in entries.iter().take(40) {
        let at = if truthy(e.get("at")) {
            s(e.get("at"))
        } else {
            String::new()
        };
        let mut line = format!("- [{}] {}", take_chars(&at, 10), s(e.get("kind")));
        if truthy(e.get("sub_kind")) {
            line += &format!("/{}", s(e.get("sub_kind")));
        }
        if truthy(e.get("title")) {
            line += &format!(" \u{2014} {}", s(e.get("title")));
        }
        if truthy(e.get("preview")) {
            line += &format!(": {}", take_chars(&s(e.get("preview")), 300));
        }
        out.push(line);
    }
    out
}

/// `_load(db, object_type, record_id)`.
pub async fn _load(
    db: &mut PgConnection,
    object_type: &str,
    record_id: Uuid,
) -> Result<Option<Record>, ApiError> {
    Ok(match object_type {
        "organization" => {
            sqlx::query_as::<_, Organization>("SELECT * FROM organizations WHERE id = $1")
                .bind(record_id)
                .fetch_optional(&mut *db)
                .await?
                .map(Record::Organization)
        }
        "contact" => sqlx::query_as::<_, Contact>("SELECT * FROM contacts WHERE id = $1")
            .bind(record_id)
            .fetch_optional(&mut *db)
            .await?
            .map(Record::Contact),
        "lead" => sqlx::query_as::<_, Lead>("SELECT * FROM leads WHERE id = $1")
            .bind(record_id)
            .fetch_optional(&mut *db)
            .await?
            .map(Record::Lead),
        "deal" => sqlx::query_as::<_, Deal>("SELECT * FROM deals WHERE id = $1")
            .bind(record_id)
            .fetch_optional(&mut *db)
            .await?
            .map(Record::Deal),
        _ => None,
    })
}

async fn _guidance(db: &mut PgConnection, context: &str) -> String {
    crate::ai::knowledge::context_block(db, &take_chars(context, 1500), 3).await
}

/// `str(e.detail)` when the error is an HTTPException (what `except HTTPException` catches), else None.
pub(crate) fn http_detail(e: &AiError) -> Option<String> {
    match e {
        AiError::Ai { detail, .. } => Some(detail.clone()),
        AiError::Api(ApiError::Http { detail, .. }) => Some(match detail {
            Value::String(s) => s.clone(),
            other => str_value(&PyValue::from(other)),
        }),
        _ => None,
    }
}

/// Generate a fresh brief. Returns {'error': ...} rather than raising, so the panel can render the reason
/// inline.
pub async fn build_brief(
    db: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    record_id: Uuid,
) -> Result<Value, ApiError> {
    let cfg = get_ai_config(db).await?;
    if !feature_enabled(&cfg, "scoring") {
        return Ok(
            json!({"error": "AI briefs are disabled. Enable AI in Settings → Nary & AI → AI features."}),
        );
    }
    let ai = match use_ai(
        db,
        "record_brief",
        UseAiOptions::user(Some(user.id)).with_cfg(cfg),
    )
    .await
    {
        Ok(ai) => ai,
        Err(e) => match http_detail(&e) {
            Some(d) => return Ok(json!({"error": d})),
            None => return Err(e.into()),
        },
    };
    let Some(rec) = _load(db, object_type, record_id).await? else {
        return Ok(json!({"error": "Record not found."}));
    };
    let timeline =
        crate::crm::timeline::build_timeline(db, user, object_type, record_id, 40).await?;
    let mut lines = describe_record(object_type, &rec);
    lines.extend(describe_timeline(&timeline));
    let context = lines.join("\n");
    let guidance = _guidance(db, &context).await;
    let mut req = ChatRequest::user(
        _SYSTEM,
        format!("{context}{guidance}\n\nReturn the JSON brief now."),
    );
    req.temperature = Some(0.2);
    let result = match ai.chat(db, req).await {
        Ok(r) => r,
        Err(e) => match http_detail(&e) {
            Some(d) => return Ok(json!({"error": d})),
            None => return Err(e.into()),
        },
    };
    if result.refused() {
        return Ok(json!({"error": refusal_message(&result)}));
    }
    let data = extract_json(&result.text);
    if !crate::crm::attributes::py::truthy(&data) {
        return Ok(json!({"error": "The model did not return a usable brief. Try again."}));
    }
    let Value::Object(data) = data else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "'{}' object has no attribute 'get'",
            crate::crm::attributes::py::type_name(&data)
        )));
    };
    use crate::crm::attributes::py::{iterate, str_of, truthy};
    // `data.get(k) or default`
    let or = |k: &str, default: Value| -> Value {
        match data.get(k) {
            Some(v) if truthy(v) => v.clone(),
            _ => default,
        }
    };
    // `[str(s) for s in (data.get(k) or [])][:6]`
    let strs = |k: &str| -> Result<Value, ApiError> {
        Ok(Value::Array(
            iterate(&or(k, json!([])))?
                .iter()
                .map(|v| Value::String(str_of(v)))
                .take(6)
                .collect(),
        ))
    };
    let model = if result.model_used.is_empty() {
        ai.model().to_string()
    } else {
        result.model_used.clone()
    };
    Ok(json!({
        "summary": or("summary", json!("")),
        "next_best_action": or("next_best_action", json!("")),
        "signals": strs("signals")?,
        "risks": strs("risks")?,
        "generated_at": granary_compat::pyjson::PyDateTime::from_chrono_utc(&chrono::Utc::now()).isoformat(),
        "model": model,
        "timeline_entries": timeline.len(),
    }))
}
