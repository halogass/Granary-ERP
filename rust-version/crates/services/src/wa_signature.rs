//! Port of backend/app/services/wa_signature.py.
//!
//! The WhatsApp sign-off: every message sent from the ERP goes out from the one company number,
//! so it is signed with who at the company wrote it, and the sender is logged for the inbox's "sent by".
//!
//! Signed: text replies and media captions from the inbox, and scheduled messages. Not signed:
//! forwards, documents sent with the document "Send" button, and staff notifications.

use std::collections::HashMap;

use granary_compat::text::pystr;
use granary_db::rows::auth::User;
use once_cell::sync::Lazy;
use sqlx::PgConnection;

pub const CUSTOM_MAX: usize = 300;

/// The sign-off prefix `_` + EM DASH (U+2014). Kept as an escape so the source carries no literal dash.
const SIGNOFF_START: &str = "_\u{2014}";

static PY_ALPHA: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"^\p{L}$").expect("regex"));

/// Python `str.isalpha()` for one character: general category L* (Lu, Ll, Lt, Lm, Lo).
fn py_isalpha(c: char) -> bool {
    let mut buf = [0u8; 4];
    PY_ALPHA.is_match(c.encode_utf8(&mut buf))
}

/// First name plus the initials of the rest: "Diana Rose Prince" -> "Diana R. P.".
/// A degree after a comma ("Diana Prince, MBA") is dropped; a single name stays as it is.
pub fn short_name(full_name: Option<&str>) -> String {
    let full = full_name.unwrap_or("");
    let head = full.split_once(',').map(|(h, _)| h).unwrap_or(full);
    let parts = pystr::split(head);
    if parts.is_empty() {
        return String::new();
    }
    let mut out: Vec<String> = vec![parts[0].to_string()];
    for p in &parts[1..] {
        let first = p.chars().next().expect("split() parts are non-empty");
        if py_isalpha(first) {
            // `p[0].upper()`: full case mapping (one char may become several, as in Python).
            out.push(format!("{}.", first.to_uppercase()));
        }
    }
    out.join(" ")
}

/// The name this user signs with and is shown as: their override, else the automatic one.
pub fn display_name(user: &User) -> String {
    display_name_of(
        user.wa_signature_name.as_deref(),
        Some(&user.name),
        &user.email,
    )
}

/// `display_name` on the three fields it reads.
pub fn display_name_of(wa_signature_name: Option<&str>, name: Option<&str>, email: &str) -> String {
    let override_ = pystr::strip(wa_signature_name.unwrap_or(""));
    if !override_.is_empty() {
        return override_.to_string();
    }
    let auto = short_name(name);
    if !auto.is_empty() {
        return auto;
    }
    email.to_string()
}

/// The superadmin's own text is used exactly as written; everyone else gets the standard line.
pub async fn signature_for(
    db: &mut PgConnection,
    user: Option<&User>,
) -> Result<String, sqlx::Error> {
    let custom = pystr::strip(
        user.and_then(|u| u.wa_signature_custom.as_deref())
            .unwrap_or(""),
    );
    if let Some(u) = user {
        if u.is_superadmin && !custom.is_empty() {
            return Ok(custom.to_string());
        }
    }
    let company = crate::email_marketing::get_company_info(db)
        .await?
        .company_name;
    Ok(match user {
        None => format!("{SIGNOFF_START} {company}_"),
        Some(u) => format!("{SIGNOFF_START} {}, {company}_", display_name(u)),
    })
}

/// Append the sign-off once. Text that already ends with it (a retry, an AI draft that copied
/// it) is left alone; empty text (a file sent without a caption) becomes just the sign-off.
pub fn sign(text: Option<&str>, signature: &str) -> String {
    let body = pystr::rstrip(text.unwrap_or(""));
    if body.is_empty() {
        return signature.to_string();
    }
    if body.ends_with(signature) {
        return body.to_string();
    }
    format!("{body}\n\n{signature}")
}

/// Drop a trailing standard sign-off line (`_<em dash> ..._`), e.g. one an AI draft imitated from history.
pub fn strip_signature(text: Option<&str>) -> String {
    let trimmed = pystr::rstrip(text.unwrap_or(""));
    let mut lines: Vec<&str> = trimmed.split('\n').collect();
    if let Some(last) = lines.last() {
        let l = pystr::strip(last);
        if l.starts_with(SIGNOFF_START) && l.ends_with('_') {
            lines.pop();
        }
    }
    pystr::rstrip(&lines.join("\n")).to_string()
}

/// The stable part of a WhatsApp message id. Ids look like `true_<remote>_<KEY>[_participant]`
/// (services/whatsapp/src/compat.js serializedMessageId); the remote can switch between the
/// `@lid` and `@c.us` forms of the same contact, the key doesn't.
pub fn message_key(serialized_id: Option<&str>) -> Option<String> {
    let sid = serialized_id.filter(|s| !s.is_empty())?;
    let parts: Vec<&str> = sid.split('_').collect();
    if parts.len() >= 3 && !parts[2].is_empty() {
        Some(parts[2].to_string())
    } else {
        Some(sid.to_string())
    }
}

/// Log who sent this message. Best effort: a missing id just means no "sent by" label.
/// Python commits here: inside the caller's transaction this commits it (`COMMIT; BEGIN`).
pub async fn record_sent(
    db: &mut PgConnection,
    serialized_id: Option<&str>,
    chat_id: &str,
    user: Option<&User>,
) -> Result<(), sqlx::Error> {
    let Some(key) = message_key(serialized_id) else {
        return Ok(());
    };
    let exists: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT wa_sent_messages.id FROM wa_sent_messages WHERE wa_sent_messages.message_key = $1",
    )
    .bind(&key)
    .fetch_optional(&mut *db)
    .await?;
    if exists.is_some() {
        return Ok(());
    }
    let sender_name = match user {
        Some(u) => display_name(u),
        None => "System".to_string(),
    };
    let chat_id = chat_id.to_string();
    let user_id = user.map(|u| u.id);
    crate::email_marketing::commit_unit(db, move |db| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO wa_sent_messages (message_key, chat_id, user_id, sender_name, id) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(&key)
            .bind(&chat_id)
            .bind(user_id)
            .bind(&sender_name)
            .bind(uuid::Uuid::new_v4())
            .execute(&mut *db)
            .await?;
            Ok(())
        })
    })
    .await?;
    Ok(())
}

/// {serialized id: sender name} for the given messages that were sent from the ERP.
/// `None` entries stand for messages without an id (`m.get("id")` in the router).
pub async fn sent_by(
    db: &mut PgConnection,
    serialized_ids: &[Option<&str>],
) -> Result<HashMap<String, String>, sqlx::Error> {
    // {sid: message_key(sid) for sid in serialized_ids if sid}
    let mut keys: Vec<(String, Option<String>)> = Vec::new();
    for sid in serialized_ids.iter().flatten() {
        if sid.is_empty() {
            continue;
        }
        let k = message_key(Some(sid));
        if let Some(slot) = keys.iter_mut().find(|(s, _)| s == sid) {
            slot.1 = k;
        } else {
            keys.push((sid.to_string(), k));
        }
    }
    let mut wanted: Vec<String> = Vec::new();
    for (_, k) in &keys {
        if let Some(k) = k {
            if !wanted.contains(k) {
                wanted.push(k.clone());
            }
        }
    }
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT wa_sent_messages.message_key, wa_sent_messages.sender_name FROM wa_sent_messages \
         WHERE wa_sent_messages.message_key = ANY($1)",
    )
    .bind(&wanted)
    .fetch_all(&mut *db)
    .await?;
    let by_key: HashMap<String, String> = rows.into_iter().collect();
    Ok(keys
        .into_iter()
        .filter_map(|(sid, k)| {
            k.and_then(|k| by_key.get(&k).cloned())
                .map(|name| (sid, name))
        })
        .collect())
}
