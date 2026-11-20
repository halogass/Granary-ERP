//! Port of backend/app/services/ai/documents.py.
//!
//! Document intelligence: analyze a stored attachment (PDF/image) with a multimodal model.
//!
//! Reads the same MinIO objects the attachment endpoints serve, and goes through the same access
//! check (`attachments_access::assert_can_access`). It has to: handing an attachment id to
//! the model is a read of that file, so "may I analyze this?" is exactly "may I download this?".
//! An access failure is a 403 rather than an {"error": ...} dict, so the caller can tell
//! "not allowed" apart from "the model failed".

use granary_compat::pyjson::{str_value, PyValue};
use granary_compat::text::pystr;
use granary_compat::text::utils::py_uuid;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::ai::base::{AiError, FileRequest};
use crate::ai::registry::{
    self, ai_ready, feature_enabled, friendly_error, refusal_message, UseAiOptions,
};

const _SUPPORTED: &[&str] = &["application/pdf"]; // + any image/*

pub const _DEFAULT_PROMPT: &str = concat!(
    "Summarize this document in a few sentences, then list the key fields/figures (parties, dates, ",
    "amounts, totals, reference numbers) as concise bullet points. Reply in the document's language."
);

pub const _SYSTEM: &str = concat!(
    "You are a precise document analyst. Use ONLY what is visible in the document. If a detail isn't ",
    "present, say so. Do not invent numbers."
);

fn _supported(mime: &str) -> bool {
    _SUPPORTED.contains(&mime) || mime.starts_with("image/")
}

/// The UUID bind of a string id (SQLAlchemy's Uuid type raises ValueError on a malformed one: a 500).
fn uuid_arg(v: &str) -> ApiResult<Uuid> {
    py_uuid(v).ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!(
            "ValueError: badly formed hexadecimal UUID string"
        ))
    })
}

async fn _read_object(object_key: &str) -> Result<Vec<u8>, crate::storage::StoreError> {
    // batch 4 D1: a Nary customer file listed as an attachment is in the documents bucket
    let bucket = crate::storage::attachment_bucket(object_key);
    crate::storage::minio()
        .get_object(&bucket, object_key)
        .await
}

async fn _load_attachment(
    db: &mut PgConnection,
    user: &AuthUser,
    attachment_id: Option<&str>,
) -> ApiResult<(String, String, String)> {
    let Some(aid) = attachment_id else {
        return Ok((String::new(), String::new(), String::new()));
    };
    let att: Option<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT entity_type, entity_id, object_key, mime_type, file_name FROM attachments WHERE id = $1",
    )
    .bind(uuid_arg(aid)?)
    .fetch_optional(&mut *db)
    .await?;
    let Some((entity_type, entity_id, object_key, mime, file_name)) = att else {
        return Ok((String::new(), String::new(), String::new()));
    };
    // Same gate as GET /attachments/{id}/download: 403 if this user may not read it.
    crate::attachments_access::assert_can_access(db, user, &entity_type, &entity_id).await?;
    Ok((
        object_key,
        if mime.is_empty() {
            "application/octet-stream".into()
        } else {
            mime
        },
        if file_name.is_empty() {
            "document".into()
        } else {
            file_name
        },
    ))
}

fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        other => str_value(&PyValue::from(other)),
    }
}

/// `a.get(k) or default` as a string.
fn s_or(a: &Value, k: &str, default: &str) -> String {
    match a.get(k) {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        v if crate::ai::drafting::truthy(v) => py_str(v.unwrap()),
        _ => default.to_string(),
    }
}

async fn _load_email_attachment(
    db: &mut PgConnection,
    user: &AuthUser,
    message_id: Option<&str>,
    att_id: Option<&str>,
) -> ApiResult<(String, String, String)> {
    let empty = || (String::new(), String::new(), String::new());
    let Some(mid) = message_id else {
        return Ok(empty());
    };
    // Ownership: the message's account must belong to the requesting user.
    let row: Option<Option<Value>> = sqlx::query_scalar(
        "SELECT m.attachments FROM email_messages m JOIN email_accounts a ON a.id = m.account_id \
         WHERE m.id = $1 AND a.user_id = $2",
    )
    .bind(uuid_arg(mid)?)
    .bind(user.id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(atts) = row else {
        return Ok(empty());
    };
    let want = match att_id {
        None => "None".to_string(),
        Some(s) => s.to_string(),
    };
    if let Some(Value::Array(list)) = atts {
        for a in &list {
            if py_str(a.get("id").unwrap_or(&Value::Null)) == want {
                // a.get("object_key", "")
                let key = match a.get("object_key") {
                    None => String::new(),
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Null) => String::new(),
                    Some(other) => py_str(other),
                };
                return Ok((
                    key,
                    s_or(a, "content_type", "application/octet-stream"),
                    s_or(a, "filename", "document"),
                ));
            }
        }
    }
    Ok(empty())
}

fn err(s: impl Into<String>) -> Value {
    json!({"error": s.into()})
}

/// `str(e.detail)` of an HTTPException-like AiError.
fn http_detail(e: &AiError) -> Option<String> {
    if !e.is_http_exception() {
        return None;
    }
    Some(match e {
        AiError::Ai { detail, .. } => detail.clone(),
        AiError::Api(ApiError::Http { detail, .. }) => match detail {
            Value::String(s) => s.clone(),
            other => py_str(other),
        },
        other => other.to_string(),
    })
}

/// `analyze_document(db, user, *, source, attachment_id=None, message_id=None, att_id=None, question=None)`.
#[allow(clippy::too_many_arguments)]
pub async fn analyze_document(
    db: &mut PgConnection,
    user: &AuthUser,
    source: &str,
    attachment_id: Option<&str>,
    message_id: Option<&str>,
    att_id: Option<&str>,
    question: Option<&str>,
) -> ApiResult<Value> {
    let cfg = registry::get_ai_config(db).await?;
    if !feature_enabled(&cfg, "doc_intel") {
        return Ok(err(
            "Document intelligence is disabled in Settings \u{2192} Nary & AI \u{2192} AI features.",
        ));
    }
    if !ai_ready(&cfg, "doc_intel") {
        return Ok(err("No API key for the AI provider document analysis uses. Set it in Settings \u{2192} Nary & AI \u{2192} Providers & keys."));
    }

    let (file_bytes, mime, filename): (Vec<u8>, String, String);
    if source == "crm_email" {
        // The shared CRM inbox is CRM-members only, exactly as search_emails treats it.
        if !(user.is_superadmin
            || user.perms.contains("crm:view_all")
            || user.perms.contains("crm:view_own"))
        {
            return Err(ApiError::http(403, "Permission required: crm:view_own"));
        }
        // CRM inbox stores attachment bytes in the DB row directly.
        let att: Option<(Option<String>, String, Vec<u8>)> =
            match attachment_id {
                None => None,
                Some(aid) => sqlx::query_as(
                    "SELECT content_type, filename, data FROM crm_email_attachments WHERE id = $1",
                )
                .bind(uuid_arg(aid)?)
                .fetch_optional(&mut *db)
                .await?,
            };
        let Some((ct, fname, data)) = att else {
            return Ok(err("Document not found or not accessible to you."));
        };
        mime = ct
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "application/octet-stream".into());
        filename = if fname.is_empty() {
            "document".into()
        } else {
            fname
        };
        if !_supported(&mime) {
            return Ok(err(format!(
                "Unsupported file type ({mime}). Only PDF and images can be analyzed."
            )));
        }
        file_bytes = data;
    } else {
        let (object_key, m, f) = match source {
            "attachment" => _load_attachment(db, user, attachment_id).await?,
            "email" => _load_email_attachment(db, user, message_id, att_id).await?,
            _ => return Ok(err("Unsupported source")),
        };
        if object_key.is_empty() {
            return Ok(err("Document not found or not accessible to you."));
        }
        if !_supported(&m) {
            return Ok(err(format!(
                "Unsupported file type ({m}). Only PDF and images can be analyzed."
            )));
        }
        mime = m;
        filename = f;
        file_bytes = match _read_object(&object_key).await {
            Ok(b) => b,
            Err(_) => return Ok(err("Could not read the file from storage.")),
        };
    }

    if file_bytes.is_empty() {
        return Ok(err("The document appears to be empty."));
    }

    let q = pystr::strip(question.unwrap_or("")).to_string();
    let prompt = if q.is_empty() {
        _DEFAULT_PROMPT.to_string()
    } else {
        q
    };
    let call = async {
        let ai = registry::use_ai(
            db,
            "doc_intel",
            UseAiOptions::user(Some(user.id)).with_cfg(cfg.clone()),
        )
        .await?;
        ai.analyze_file(
            db,
            FileRequest::new(prompt, file_bytes, mime).system(_SYSTEM),
        )
        .await
    };
    let result = match call.await {
        Ok(r) => r,
        Err(e) => {
            if let Some(d) = http_detail(&e) {
                return Ok(err(d));
            }
            if let AiError::NotImplemented(why) = &e {
                // the adapter's own reason when it has one (OpenAI-compatible: images yes, PDFs no)
                if !why.is_empty() {
                    return Ok(err(why.clone()));
                }
                return Ok(err("The AI provider routed for doc_intel can't read files. Change it in Settings \u{2192} Nary & AI \u{2192} Model per job."));
            }
            return Ok(err(format!("Analysis failed: {}", friendly_error(&e, ""))));
        }
    };
    if result.refused() {
        return Ok(err(refusal_message(&result)));
    }
    let answer = pystr::strip(&result.text).to_string();
    Ok(json!({
        "answer": if answer.is_empty() { "No answer produced.".to_string() } else { answer },
        "filename": filename,
    }))
}
