//! Port of backend/app/services/nary/profiling.py.
//!
//! Progressive profiling for direct WhatsApp chats (design §4.3, Nary Phase 2 item E). Phase 3's reply
//! playbook (replies.py) uses gaps() and instructions() from here; draft_reply() is the Phase 2 reply on
//! its own, kept for callers that want only the profiling question.
//!
//! Answer first, then at most two questions per message to fill what's missing (name, company, email,
//! the need, how they found the company), using what is already known from the number's country, the WhatsApp
//! display name and the Business profile. "How did you find us?" is asked once in a conversation; the
//! answer is stored as the lead's self-reported attribution (there is no URL attribution for a direct chat).
//!
//! The draft goes through the action registry as whatsapp.reply.send, so in shadow mode it is stored and
//! later compared with what the Owner actually sent; it never reaches the customer from here unless the
//! autonomy level and shadow mode allow it.

use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::crm::{Contact, Lead};
use granary_db::rows::nary::{AgentRun, WaChat};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::PgConnection;

use crate::whatsapp::py_truthy;

/// `SCHEMA` (key order as Python's dict literal).
pub static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "reply": {"type": "string"},
            "asked": {"type": "array", "items": {"type": "string"}},
            "customer_said_how_found": {"type": ["string", "null"]},
        },
        "required": ["reply", "asked", "customer_said_how_found"],
        "additionalProperties": false,
    })
});

/// What we still don't know about this person.
pub async fn gaps(db: &mut PgConnection, chat: &WaChat) -> ApiResult<Vec<String>> {
    let contact = match chat.contact_id {
        Some(id) => {
            sqlx::query_as::<_, Contact>("SELECT * FROM contacts WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let lead = match chat.lead_id {
        Some(id) => {
            sqlx::query_as::<_, Lead>("SELECT * FROM leads WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let mut missing = Vec::new();
    let has_name = contact.as_ref().is_some_and(|c| {
        !granary_compat::text::pystr::strip(&c.first_name).is_empty()
            && Some(c.first_name.as_str()) != chat.phone.as_deref()
    });
    if !has_name {
        missing.push("their name".to_string());
    }
    let profile_description = chat
        .business_profile
        .as_ref()
        .filter(|p| py_truthy(p))
        .and_then(|p| p.get("description"))
        .is_some_and(py_truthy);
    if !contact
        .as_ref()
        .is_some_and(|c| c.organization_id.is_some())
        && !profile_description
    {
        missing.push("their company".to_string());
    }
    if !contact
        .as_ref()
        .is_some_and(|c| c.email.as_deref().is_some_and(|e| !e.is_empty()))
    {
        missing.push("a business email (useful for documents and meeting invites)".to_string());
    }
    let knows_need = lead.as_ref().is_some_and(|l| {
        l.description.as_deref().is_some_and(|d| !d.is_empty())
            || l.interested_products
                .as_ref()
                .is_some_and(|p| !p.is_empty())
    });
    if !knows_need {
        missing.push("what they need".to_string());
    }
    let self_reported = lead
        .as_ref()
        .and_then(|l| l.attribution.as_ref())
        .filter(|a| py_truthy(a))
        .and_then(|a| a.get("self_reported"))
        .is_some_and(py_truthy);
    if !self_reported && !chat.attribution_asked {
        missing.push("how they found the company (ask once, naturally)".to_string());
    }
    Ok(missing)
}

pub fn instructions(missing: &[String]) -> String {
    let asks = if missing.is_empty() {
        "nothing, the profile is complete".to_string()
    } else {
        missing.join("; ")
    };
    format!(
        "Write the next WhatsApp reply to this customer. Answer what they asked first. Then ask at most two \
short questions, picked from what is still unknown: {asks}. Never ask something the conversation already \
answers, and never ask how they found the company if anyone in this conversation already asked. Keep it under 5 \
lines. No prices unless they asked, and then only list prices. In 'asked', list the topics you asked \
about. In customer_said_how_found, copy the customer's own words if they said how they found the company, \
else null."
    )
}

/// `draft_reply(db, chat, transcript, *, run=None)`: draft the next reply (Opus, job nary.reply_draft) and
/// propose it as whatsapp.reply.send. Returns {card_id, status, asked} or None. Commits the caller's
/// transaction before proposing, as Python does.
pub async fn draft_reply(
    db: &mut PgConnection,
    chat: &WaChat,
    transcript: &str,
    run: Option<&mut AgentRun>,
) -> ApiResult<Option<Value>> {
    use crate::ai::base::ChatRequest;
    use crate::ai::registry::{use_ai, UseAiOptions};
    use crate::nary::{executor, governance, memory, privacy, roles, runs};
    use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
    privacy::assert_ai_allowed(db, &chat.chat_id).await?;
    let missing = gaps(db, chat).await?;
    let known = json!({"number_country": chat.phone, "whatsapp_name": chat.display_name,
                       "business_profile": chat.business_profile, "language": chat.language});
    let mut notes = String::new();
    if let Some(contact_id) = chat.contact_id {
        let entries = memory::recall(
            db,
            memory::Recall {
                layers: vec!["customer".into()],
                subject_type: Some("contact".into()),
                subject_id: Some(contact_id.to_string()),
                limit: 10,
                ..Default::default()
            },
        )
        .await?;
        notes = memory::as_context(&entries);
    }
    let run_id = run.as_ref().map(|r| r.id);
    let ai = use_ai(db, "nary.reply_draft", UseAiOptions::run(run_id)).await?;
    let known = dumps(&PyValue::from(&known), &DumpOpts::unicode_default_str())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
    let content = format!(
        "WHAT WE KNOW (JSON):\n{known}{}\n\nCONVERSATION:\n<information source=\"customer\">\n{}\n</information>",
        if notes.is_empty() { String::new() } else { format!("\n\nNOTES:\n{notes}") },
        crate::nary::py_slice_tail(transcript, 8000)
    );
    let system =
        super::writing::customer_prompt(db, Some("whatsapp"), "", &instructions(&missing)).await?;
    let mut req = ChatRequest::user(system, content);
    req.response_schema = Some(SCHEMA.clone());
    let result = ai.chat(db, req).await?;
    if let Some(run) = run {
        let output = json!({"text": result.text});
        runs::add_step(
            db,
            run,
            "llm",
            Some("nary.reply_draft"),
            runs::StepOpts {
                output: Some(&output),
                result: Some(&result),
                cost: ai.last_cost(),
                model: Some(ai.model()),
                ..Default::default()
            },
        )
        .await?;
    }
    if result.refused() {
        return Ok(None);
    }
    let raw = if result.text.is_empty() {
        "{}"
    } else {
        result.text.as_str()
    };
    let Ok(out) = granary_core::validation::json_loads(raw.as_bytes()).map(|o| o.to_json()) else {
        return Ok(None);
    };
    let Value::Object(out) = out else {
        // `out.get` on a non-object: AttributeError in Python.
        return Err(ApiError::internal(anyhow::anyhow!(
            "AttributeError: the draft is not an object"
        )));
    };
    if let (Some(found), Some(lead_id)) = (
        out.get("customer_said_how_found").filter(|v| py_truthy(v)),
        chat.lead_id,
    ) {
        let found = found.as_str().unwrap_or_default();
        let attribution: Option<Value> =
            sqlx::query_scalar("SELECT attribution FROM leads WHERE id = $1")
                .bind(lead_id)
                .fetch_one(&mut *db)
                .await?;
        let mut attr = match attribution {
            Some(Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        attr.insert(
            "self_reported".into(),
            json!(crate::nary::py_slice(found, 300)),
        );
        sqlx::query("UPDATE leads SET attribution = $2, updated_at = now() WHERE id = $1")
            .bind(lead_id)
            .bind(Value::Object(attr))
            .execute(&mut *db)
            .await?;
    }
    let reply = match out.get("reply") {
        Some(Value::String(s)) => granary_compat::text::pystr::strip(s).to_string(),
        _ => String::new(),
    };
    if reply.is_empty() {
        return Ok(None);
    }
    let asked: Vec<String> = match out.get("asked") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => vec![],
    };
    let actor = roles::nary_actor(db).await?;
    let (chat_id, contact_id) = (chat.chat_id.clone(), chat.contact_id);
    crate::nary::commit(db).await?;
    let rationale = format!(
        "Reply with progressive profiling: {}",
        if asked.is_empty() {
            "no questions".to_string()
        } else {
            asked.join(", ")
        }
    );
    let payload = json!({"channel": "whatsapp", "chat_id": chat_id, "text": reply,
                         "contact_id": contact_id.map(|c| c.to_string())});
    let card = match executor::propose(
        db,
        "whatsapp.reply.send",
        payload,
        actor.as_ref(),
        executor::ProposeOpts {
            run_id,
            sop_ref: None,
            rationale: Some(&rationale),
            ..Default::default()
        },
    )
    .await
    {
        Ok(card) => card,
        Err(e) if governance::is_http_exception(&e) => {
            tracing::info!(chat_id = %chat_id, reason = %governance::http_detail(&e).unwrap_or_default(), "nary_reply_not_proposed");
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    if card.status == "applied"
        && asked
            .iter()
            .any(|a| granary_compat::text::pystr::lower(a).contains("found"))
    {
        sqlx::query("UPDATE wa_chats SET attribution_asked = true, updated_at = now() WHERE chat_id = $1 AND NOT attribution_asked")
            .bind(&chat_id)
            .execute(&mut *db)
            .await?;
        crate::nary::commit(db).await?;
    }
    let asked_json = out.get("asked").cloned().unwrap_or(Value::Null);
    Ok(Some(
        json!({"card_id": card.id.to_string(), "status": card.status, "asked": asked_json}),
    ))
}
