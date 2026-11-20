//! Port of backend/app/services/ai/scoring.py.
//!
//! On-demand AI lead scoring. Pulls the lead + its WA/email/activity history and asks the model to
//! rate it hot/warm/cold with a rationale and a recommended next action, using a tunable rubric.

use chrono::{DateTime, Utc};
use granary_compat::pyjson::{str_value, PyDateTime, PyDecimal, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_db::rows::crm::Lead;
use serde_json::{json, Value};
use sqlx::PgConnection;

use crate::ai::base::{AiError, ChatRequest};
use crate::ai::drafting::truthy;
use crate::ai::registry::{self, extract_json, refusal_message, UseAiOptions};

pub const _DEFAULT_RUBRIC: &str = concat!(
    "HOT: explicit budget, timeline, and an engaged decision-maker; clear buying intent.\n",
    "WARM: genuine interest and fit but missing budget/timeline/authority; needs nurturing.\n",
    "COLD: vague, info-gathering only, poor fit, or unresponsive."
);

pub const _SYSTEM: &str = concat!(
    "You are a B2B sales analyst for the company. Score the lead from the ",
    "given CRM data and conversation history. Be objective: base the score only on the evidence. ",
    "Respond with ONLY a JSON object: ",
    "{\"score\": <0-100 int>, \"temperature\": \"hot|warm|cold\", \"rationale\": \"<2-3 sentences>\", ",
    "\"recommended_next_action\": \"<one concrete next step>\", \"signals\": [\"<short evidence point>\", ...]}"
);

fn none_or(v: Option<&str>) -> String {
    v.unwrap_or("None").to_string()
}

fn non_empty(v: Option<&str>) -> Option<&str> {
    v.filter(|s| !s.is_empty())
}

fn take(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[derive(sqlx::FromRow)]
struct Act {
    activity_type: String,
    direction: Option<String>,
    subject: Option<String>,
    content: Option<String>,
    outcome: Option<String>,
    created_at: DateTime<Utc>,
}

/// `_gather(db, lead)`: the lead data and recent conversation as prompt text.
/// Quirk kept: the activity query has no is_deleted filter, and ties on created_at come back in
/// whatever order Postgres returns them.
pub async fn _gather(db: &mut PgConnection, lead: &Lead) -> Result<String, AiError> {
    let mut parts: Vec<String> = vec![];
    let title = non_empty(lead.title.as_deref())
        .or(non_empty(lead.source_detail.as_deref()))
        .unwrap_or("(no title)");
    parts.push(format!("Lead inquiry: {title}"));
    parts.push(format!(
        "Status: {} \u{b7} Priority: {} \u{b7} Source: {}",
        none_or(lead.status.as_deref()),
        none_or(lead.priority.as_deref()),
        lead.source
    ));
    if let Some(v) = lead.estimated_value.filter(|v| !v.is_zero()) {
        // `{lead.currency + ' ' if lead.currency else ''}`: a lead may have no currency (migration 105).
        let cur = non_empty(lead.currency.as_deref())
            .map(|c| format!("{c} "))
            .unwrap_or_default();
        parts.push(format!("Estimated value: {cur}{}", PyDecimal::from(v)));
    }
    if let Some(p) = lead.interested_products.as_ref().filter(|p| !p.is_empty()) {
        parts.push(format!("Interested in: {}", p.join(", ")));
    }
    if let Some(d) = non_empty(lead.description.as_deref()) {
        parts.push(format!("Notes: {}", take(d, 1000)));
    }

    if let Some(org_id) = lead.organization_id {
        let name: Option<String> =
            sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
                .bind(org_id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some(name) = name {
            parts.push(format!("Organization: {name}"));
        }
    }
    if let Some(cid) = lead.contact_id {
        let c: Option<(String, Option<String>, Option<String>)> =
            sqlx::query_as("SELECT first_name, last_name, email FROM contacts WHERE id = $1")
                .bind(cid)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((first, last, email)) = c {
            parts.push(format!(
                "Contact: {first} {} <{}>",
                last.as_deref().unwrap_or(""),
                email.as_deref().unwrap_or("")
            ));
        }
    }

    // Recent activity / conversation history (WA, email, calls, notes).
    let acts: Vec<Act> = match lead.contact_id {
        Some(cid) => {
            sqlx::query_as(
                "SELECT activity_type, direction, subject, content, outcome, created_at FROM activities \
                 WHERE lead_id = $1 OR contact_id = $2 ORDER BY created_at DESC LIMIT 25",
            )
            .bind(lead.id)
            .bind(cid)
            .fetch_all(&mut *db)
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT activity_type, direction, subject, content, outcome, created_at FROM activities \
                 WHERE lead_id = $1 OR false ORDER BY created_at DESC LIMIT 25",
            )
            .bind(lead.id)
            .fetch_all(&mut *db)
            .await?
        }
    };
    if !acts.is_empty() {
        parts.push("\nConversation & activity history (most recent first):".into());
        for a in &acts {
            // asyncpg hands back UTC datetimes: the date is the UTC one.
            let when = a.created_at.format("%Y-%m-%d").to_string();
            let mut line = format!(
                "- [{when}] {}/{}",
                a.activity_type,
                none_or(a.direction.as_deref())
            );
            if let Some(s) = non_empty(a.subject.as_deref()) {
                line += &format!(" \u{2014} {s}");
            }
            if let Some(c) = non_empty(a.content.as_deref()) {
                line += &format!(": {}", take(c, 400));
            }
            if let Some(o) = non_empty(a.outcome.as_deref()) {
                line += &format!(" (outcome: {o})");
            }
            parts.push(line);
        }
    }
    Ok(parts.join("\n"))
}

fn detail_of(e: &AiError) -> String {
    match e {
        AiError::Ai { detail, .. } => detail.clone(),
        AiError::Api(granary_core::errors::ApiError::Http { detail, .. }) => match detail {
            Value::String(s) => s.clone(),
            other => str_value(&PyValue::from(other)),
        },
        other => other.to_string(),
    }
}

fn py_type(v: &Value) -> &'static str {
    match PyValue::from(v) {
        PyValue::Bool(_) => "bool",
        PyValue::Int(_) => "int",
        PyValue::Float(_) => "float",
        PyValue::Str(_) => "str",
        PyValue::List(_) => "list",
        PyValue::None => "NoneType",
        _ => "dict",
    }
}

/// `int(x)` of a decoded JSON value (ValueError / TypeError propagate in Python: an error here).
fn py_int(v: &Value) -> Result<i64, AiError> {
    match PyValue::from(v) {
        PyValue::Bool(b) => Ok(b as i64),
        PyValue::Int(i) => i64::try_from(i).map_err(|_| AiError::Other("int too large".into())),
        PyValue::Float(f) if f.is_nan() => {
            Err(AiError::Value("cannot convert float NaN to integer".into()))
        }
        PyValue::Float(f) if f.is_infinite() => Err(AiError::Other(
            "cannot convert float infinity to integer".into(),
        )),
        PyValue::Float(f) => Ok(f.trunc() as i64),
        PyValue::Str(s) => granary_compat::crypto::mcp::py_int_str(&s).ok_or_else(|| {
            AiError::Value(format!(
                "invalid literal for int() with base 10: {}",
                granary_compat::pyjson::repr_str(&s)
            ))
        }),
        _ => Err(AiError::Other(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py_type(v)
        ))),
    }
}

/// `datetime.now(timezone.utc).isoformat()`.
fn now_iso() -> String {
    PyDateTime::from_chrono_utc(&Utc::now()).isoformat()
}

/// Score a lead. Returns the score dict (also saved on the lead and committed), or `{"error": ...}`.
pub async fn score_lead(
    db: &mut PgConnection,
    user: &AuthUser,
    lead: &mut Lead,
) -> Result<Value, AiError> {
    let cfg = registry::get_ai_config(db).await?;
    let ai = match registry::use_ai(
        db,
        "scoring",
        UseAiOptions::user(Some(user.id)).with_cfg(cfg.clone()),
    )
    .await
    {
        Ok(ai) => ai,
        Err(e) if e.is_http_exception() => return Ok(json!({"error": detail_of(&e)})),
        Err(e) => return Err(e),
    };
    let rubric = cfg
        .get("ai.scoring_rubric")
        .filter(|r| !r.is_empty())
        .cloned()
        .unwrap_or_else(|| _DEFAULT_RUBRIC.to_string());

    let context = _gather(db, lead).await?;
    let guidance = crate::ai::knowledge::context_block(
        db,
        &format!("lead qualification scoring\n{}", take(&context, 1500)),
        3,
    )
    .await;
    let prompt = format!(
        "SCORING RUBRIC:\n{rubric}\n\nLEAD DATA:\n{context}{guidance}\n\nReturn the JSON score now."
    );
    let result = match ai
        .chat(
            db,
            ChatRequest::new(_SYSTEM, vec![json!({"role": "user", "content": prompt})])
                .temperature(0.2),
        )
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_http_exception() => return Ok(json!({"error": detail_of(&e)})),
        Err(e) => return Err(e),
    };
    if result.refused() {
        return Ok(json!({"error": refusal_message(&result)}));
    }
    let data = extract_json(&result.text);
    if !truthy(Some(&data)) {
        return Ok(json!({"error": "The model did not return a valid score. Try again."}));
    }
    let Value::Object(data) = data else {
        return Err(AiError::Other(format!(
            "'{}' object has no attribute 'get'",
            py_type(&data)
        )));
    };
    let or = |k: &str, default: Value| -> Value {
        match data.get(k) {
            v if truthy(v) => v.unwrap().clone(),
            _ => default,
        }
    };
    let score_v = or("score", json!(0));
    let temp_v = or("temperature", json!("cold"));
    let temperature = match &temp_v {
        Value::String(s) => pystr::lower(s),
        other => {
            return Err(AiError::Other(format!(
                "'{}' object has no attribute 'lower'",
                py_type(other)
            )))
        }
    };
    let temperature = if matches!(temperature.as_str(), "hot" | "warm" | "cold") {
        temperature
    } else {
        "warm".to_string()
    };
    let score = json!({
        "score": py_int(&score_v)?,
        "temperature": temperature,
        "rationale": or("rationale", json!("")),
        "recommended_next_action": or("recommended_next_action", json!("")),
        "signals": or("signals", json!([])),
        "scored_at": now_iso(),
        "scored_by": user.id.to_string(),
    });
    sqlx::query("UPDATE leads SET ai_score = $2, updated_at = now() WHERE id = $1")
        .bind(lead.id)
        .bind(&score)
        .execute(&mut *db)
        .await?;
    lead.ai_score = Some(score.clone());
    commit(db).await?;
    Ok(score)
}

/// `await db.commit()` on the caller's connection: inside a transaction it commits and opens a new one
/// (the SAVEPOINT probe of google_calendar.rs); in autocommit there is nothing to do.
async fn commit(db: &mut PgConnection) -> Result<(), AiError> {
    if sqlx::query("SAVEPOINT granary_commit_probe")
        .execute(&mut *db)
        .await
        .is_ok()
    {
        sqlx::query("RELEASE SAVEPOINT granary_commit_probe")
            .execute(&mut *db)
            .await?;
        sqlx::query("COMMIT").execute(&mut *db).await?;
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    Ok(())
}
