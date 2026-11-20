//! Port of backend/app/services/ai/suggestions.py: AI reply suggestions for WhatsApp and email threads.
//! Drafts a reply the user edits and sends; never sends anything itself.

use std::time::Duration;

use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::ai::base::{AiError, ChatRequest};
use crate::ai::registry::{
    ai_ready, feature_enabled, get_ai_config, refusal_message, use_ai, UseAiOptions,
};
use crate::crm::attributes::py::{str_of as py_str, truthy};
use crate::whatsapp::{HttpxError, HttpxResponse};

pub const SYSTEM: &str = "You are a sales/support assistant for the company. Draft a reply to the \
most recent inbound message in the conversation below. Match the customer's language (English \
unless they write in another), keep the company's professional but warm tone, be concise and helpful, and move the deal \
forward. If RELEVANT COMPANY SOPs / POLICIES are provided, follow them — do not contradict company \
procedure, pricing rules, or policy; if the SOP says to escalate or gather specific info, do that. \
Output ONLY the reply text — no preamble, no quotes, no signature placeholders.";

static TAGS: Lazy<Regex> = Lazy::new(|| Regex::new("<[^>]+>").unwrap());
static SPACES: Lazy<fancy_regex::Regex> = Lazy::new(|| {
    fancy_regex::Regex::new(&format!(
        "[{}]+",
        granary_compat::text::pystr::PY_SPACE_CLASS
    ))
    .unwrap()
});

/// `_strip_html(html)`.
pub fn strip_html(html: Option<&str>) -> String {
    let no_tags = TAGS.replace_all(html.unwrap_or(""), " ");
    let squashed = SPACES.replace_all(&no_tags, " ");
    pystr::strip(&squashed).to_string()
}

/// `_WA_MEDIA_LABELS`.
fn media_label(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "image" => "an image",
        "video" => "a video",
        "audio" | "ptt" => "a voice message",
        "document" => "a document",
        "sticker" => "a sticker",
        "location" => "a location",
        "vcard" => "a contact card",
        "multi_vcard" => "contact cards",
        _ => return None,
    })
}

/// `_wa_media_note(m)`: a short stand-in for what a WhatsApp message carried besides text, e.g.
/// "[sent a document: PO-2291.pdf]". Empty for an ordinary text message.
pub fn wa_media_note(m: &Value) -> String {
    let kind = match m.get("type") {
        Some(t) if truthy(t) => pystr::lower(&py_str(t)),
        _ => String::new(),
    };
    let mut what = media_label(&kind).map(str::to_string);
    if what.is_none() && m.get("hasMedia").map(truthy).unwrap_or(false) {
        what = Some("an attachment".into());
    }
    let Some(what) = what else {
        return String::new();
    };
    let meta = m.get("mediaMeta").filter(|v| truthy(v));
    let name = match meta.and_then(|mm| mm.get("filename")) {
        Some(f) if truthy(f) => pystr::strip(&py_str(f)).to_string(),
        _ => String::new(),
    };
    if name.is_empty() {
        format!("[sent {what}]")
    } else {
        format!("[sent {what}: {name}]")
    }
}

/// routers/whatsapp.py `_wa_manager.cache_get_messages(chat_id, limit)`: the Redis copy of a chat's
/// messages, or None (any failure reads as a miss). Inlined here: the router is wave 4.
async fn cache_get_messages(chat_id: &str, limit: i64) -> Option<Value> {
    use redis::AsyncCommands;
    let mut conn = granary_core::redis_client::get_redis().conn().await.ok()?;
    let cached: Option<String> = conn
        .get(format!("wa:messages:{chat_id}:{limit}:0"))
        .await
        .ok()?;
    serde_json::from_str(&cached?).ok()
}

/// `_wa_history(chat_id, limit=20)`: "Us: ..." / "Customer: ..." lines, oldest first.
pub async fn wa_history(chat_id: &str, limit: i64) -> Result<Vec<String>, HttpxError> {
    let wa_url = granary_core::config::settings()
        .WHATSAPP_SERVICE_URL
        .clone();
    if wa_url.is_empty() {
        return Ok(Vec::new());
    }
    let data = match cache_get_messages(chat_id, limit).await {
        Some(d) => d,
        None => {
            let url = crate::whatsapp::httpx_url(
                &format!("{wa_url}/messages/{chat_id}"),
                &[("limit", limit.to_string())],
            )?;
            let mut rb = crate::http::client()
                .get(url)
                .timeout(Duration::from_secs(30));
            for (k, v) in crate::whatsapp::_headers() {
                rb = rb.header(k, v);
            }
            let resp = HttpxResponse::read(rb.send().await?).await?;
            if resp.status != 200 {
                return Ok(Vec::new());
            }
            resp.json()?
        }
    };
    let msgs: Vec<Value> = match &data {
        Value::Array(a) => a.clone(),
        Value::Object(o) => match o.get("messages") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let start = msgs.len().saturating_sub(limit.max(0) as usize);
    let mut lines = Vec::new();
    for m in &msgs[start..] {
        // A photo, PDF or voice note with no caption has an empty body: say it arrived.
        let body = match m.get("body") {
            Some(b) if truthy(b) => pystr::strip(&py_str(b)).to_string(),
            _ => String::new(),
        };
        let text = [wa_media_note(m), body]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        let who = if m.get("fromMe").map(truthy).unwrap_or(false) {
            "Us"
        } else {
            "Customer"
        };
        lines.push(format!("{who}: {text}"));
    }
    Ok(lines)
}

/// `_email_history(db, thread_id, limit=15)`: newest first, then reversed, so a long thread still ends
/// with the message being replied to.
pub async fn email_history(
    db: &mut PgConnection,
    thread_id: Uuid,
    limit: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT direction, from_name, from_email, body_text, body_html FROM crm_email_messages \
         WHERE crm_email_messages.thread_id = $1 ORDER BY crm_email_messages.received_at DESC LIMIT $2",
    )
    .bind(thread_id)
    .bind(limit)
    .fetch_all(db)
    .await?;
    let mut lines = Vec::new();
    for (direction, from_name, from_email, body_text, body_html) in rows.into_iter().rev() {
        let text = match body_text.filter(|t| !t.is_empty()) {
            Some(t) => t,
            None => strip_html(body_html.as_deref()),
        };
        let text: String = text.chars().take(1500).collect();
        if text.is_empty() {
            continue;
        }
        let who = if direction == "outbound" {
            "Us".to_string()
        } else {
            from_name.filter(|n| !n.is_empty()).unwrap_or_else(|| {
                if from_email.is_empty() {
                    "Them".into()
                } else {
                    from_email
                }
            })
        };
        lines.push(format!("{who}: {text}"));
    }
    Ok(lines)
}

/// `suggest_reply(db, user, *, channel, chat_id=None, thread_id=None, instruction=None)`:
/// {"draft", "sops"} or {"error"}.
pub async fn suggest_reply(
    db: &mut PgConnection,
    user: &AuthUser,
    channel: &str,
    chat_id: Option<&str>,
    thread_id: Option<&str>,
    instruction: Option<&str>,
) -> Result<Value, ApiError> {
    let cfg = get_ai_config(&mut *db).await.map_err(ApiError::from)?;
    if !feature_enabled(&cfg, "suggestions") {
        return Ok(
            json!({"error": "Reply suggestions are disabled in Settings → Nary & AI → AI features."}),
        );
    }
    if !ai_ready(&cfg, "suggestion") {
        return Ok(
            json!({"error": "No API key for the AI provider reply suggestions use. Set it in Settings → Nary & AI → Providers & keys."}),
        );
    }
    let history = match channel {
        "whatsapp" => {
            let Some(chat_id) = chat_id.filter(|c| !c.is_empty()) else {
                return Ok(json!({"error": "chat_id is required"}));
            };
            wa_history(chat_id, 20)
                .await
                .map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))?
        }
        "email" => {
            let Some(thread_id) = thread_id.filter(|t| !t.is_empty()) else {
                return Ok(json!({"error": "thread_id is required"}));
            };
            // Python compares the column with the raw string: asyncpg rejects a malformed id.
            let tid = Uuid::parse_str(thread_id).map_err(|e| {
                ApiError::internal(anyhow::anyhow!("invalid UUID '{thread_id}': {e}"))
            })?;
            email_history(&mut *db, tid, 15).await?
        }
        _ => return Ok(json!({"error": "Unsupported channel"})),
    };
    if history.is_empty() {
        return Ok(json!({"error": "No conversation history found to base a reply on."}));
    }
    let convo = history.join("\n");
    let tail: String = {
        let n = convo.chars().count();
        convo.chars().skip(n.saturating_sub(1500)).collect()
    };
    let instruction_s = instruction.unwrap_or("");

    // Ground the reply in relevant company SOPs/policies (best-effort).
    let mut sop_block = String::new();
    let mut sop_cites: Vec<Value> = Vec::new();
    let sop_query = format!("{instruction_s}\n{tail}");
    if let Ok(sops) =
        crate::ai::embeddings::search_sops(&mut *db, pystr::strip(&sop_query), 3).await
    {
        if !sops.is_empty() {
            let body: Vec<String> = sops
                .iter()
                .map(|s| {
                    format!(
                        "[{}]\n{}",
                        s.title,
                        s.content.chars().take(800).collect::<String>()
                    )
                })
                .collect();
            sop_block = format!(
                "\n\nRELEVANT COMPANY SOPs / POLICIES (follow these; do not contradict them):\n{}",
                body.join("\n\n")
            );
            sop_cites = sops
                .iter()
                .map(|s| json!({"title": s.title, "article_id": s.article_id, "slug": s.slug}))
                .collect();
        }
    }

    let mut prompt = format!("CONVERSATION (oldest → newest):\n{convo}\n");
    if !instruction_s.is_empty() {
        prompt += &format!("\nAdditional instruction for the reply: {instruction_s}\n");
    }
    prompt += &sop_block;
    prompt += &crate::ai::knowledge::context_block(
        &mut *db,
        pystr::strip(&format!("{instruction_s}\n{tail}")),
        3,
    )
    .await;
    prompt += "\n\nWrite the best reply now:";

    let result = async {
        let ai = use_ai(
            &mut *db,
            "suggestion",
            UseAiOptions::user(Some(user.id)).with_cfg(cfg.clone()),
        )
        .await?;
        ai.chat(&mut *db, ChatRequest::user(SYSTEM, prompt).temperature(0.4))
            .await
    }
    .await;
    let result = match result {
        Ok(r) => r,
        Err(e) if e.is_http_exception() => return Ok(json!({"error": http_detail(&e)})),
        Err(e) => return Err(ApiError::from(e)),
    };
    if result.refused() {
        return Ok(json!({"error": refusal_message(&result)}));
    }
    let mut draft = pystr::strip(&result.text).to_string();
    if channel == "whatsapp" {
        // Sending adds the sender's own sign-off, so a copied one would sign the message twice.
        draft = crate::wa_signature::strip_signature(Some(&draft));
    }
    if draft.is_empty() {
        return Ok(json!({"error": "Couldn't generate a suggestion. Try again."}));
    }
    Ok(json!({"draft": draft, "sops": sop_cites}))
}

/// `str(e.detail)` of an HTTPException-like AI error.
pub(crate) fn http_detail(e: &AiError) -> String {
    match e {
        AiError::Ai { detail, .. } => detail.clone(),
        AiError::Api(api) => {
            crate::ai::catalog_digest::http_detail(api).unwrap_or_else(|| e.to_string())
        }
        other => other.to_string(),
    }
}
