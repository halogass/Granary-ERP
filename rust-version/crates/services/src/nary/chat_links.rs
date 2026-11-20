//! Port of backend/app/services/nary/chat_links.py: linking a WhatsApp chat to its CRM contact, by hand or
//! as soon as the CRM learns the number.
//!
//! A chat only links itself when its first message arrives and the number is already on a contact. When
//! the number is added to the contact later (for example a customer's chat came in as an @lid id, his phone was
//! saved on his contact afterwards), the chat stayed unlinked and his replies never reached his contact or
//! deal. So:
//! - `link(db, chat, contact)`: a person (or Nary's card, on the Owner's yes) links one chat
//!   (identity_match.link_chat, which also points the stored messages at the contact and its open lead or
//!   deal, and can be undone);
//! - `link_by_number(db, contact)`: when a contact's phone or WhatsApp number is saved, every unlinked
//!   customer chat with that number is linked to it (the number of an @lid chat comes from its name, as on
//!   intake).

use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::errors::ApiError;
use granary_db::rows::crm::Contact;
use granary_db::rows::nary::WaChat;

pub type Result<T> = std::result::Result<T, ApiError>;

/// `LinkRefused(msg)`: the router turns it into a 400 with `str(e)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRefused(pub String);

impl std::fmt::Display for LinkRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LinkRefused {}

/// What `link` can raise: a refusal, or anything else (database, link_chat).
#[derive(Debug)]
pub enum LinkError {
    Refused(LinkRefused),
    Api(ApiError),
}

impl From<ApiError> for LinkError {
    fn from(e: ApiError) -> Self {
        LinkError::Api(e)
    }
}

impl From<sqlx::Error> for LinkError {
    fn from(e: sqlx::Error) -> Self {
        LinkError::Api(e.into())
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Refused(r) => write!(f, "{r}"),
            LinkError::Api(a) => write!(f, "{a}"),
        }
    }
}

/// `_name(contact)`: `f"{first_name} {last_name or ''}".strip()`.
pub fn _name(contact: &Contact) -> String {
    let raw = format!(
        "{} {}",
        contact.first_name,
        contact.last_name.as_deref().unwrap_or("")
    );
    granary_compat::text::pystr::strip(&raw).to_string()
}

/// `link(db, chat, contact, *, by)`: link this chat to this contact (a person's decision). Flushes; the
/// caller commits.
pub async fn link(
    db: &mut PgConnection,
    chat: &mut WaChat,
    contact: &Contact,
    by: &str,
) -> std::result::Result<(), LinkError> {
    if chat.staff_user_id.is_some() {
        return Err(LinkError::Refused(LinkRefused(
            "This is a staff chat; it belongs to a member of staff, not a contact.".into(),
        )));
    }
    if chat.status == "excluded" {
        return Err(LinkError::Refused(LinkRefused(
            "This chat is marked personal; Nary keeps nothing from it.".into(),
        )));
    }
    let evidence = json!([{"signal": "manual", "score": 100, "detail": format!("linked by {by}")}]);
    deps::link_chat(db, chat, contact.id, &_name(contact), evidence, "manual").await?;
    Ok(())
}

/// `link_by_number(db, contact)`: link the unlinked customer chats that carry this contact's numbers.
/// Flushes; returns how many. (The wave-2 stub in later/crm_writes.rs takes the contact id; this loads the
/// contact.)
pub async fn link_by_number(db: &mut PgConnection, contact_id: Uuid) -> Result<i64> {
    let contact: Option<Contact> = sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
        .bind(contact_id)
        .fetch_optional(&mut *db)
        .await?;
    let Some(contact) = contact else {
        // `contact.phone` on None: AttributeError in Python.
        return Err(ApiError::internal(anyhow::anyhow!(
            "'NoneType' object has no attribute 'phone'"
        )));
    };
    link_by_number_contact(db, &contact).await
}

/// `link_by_number(db, contact)` with the contact row in hand.
pub async fn link_by_number_contact(db: &mut PgConnection, contact: &Contact) -> Result<i64> {
    let country = deps::default_country(db).await?;
    let mut numbers: Vec<String> = Vec::new();
    for n in [
        deps::to_e164(contact.phone.as_deref(), &country),
        deps::to_e164(contact.whatsapp.as_deref(), &country),
    ]
    .into_iter()
    .flatten()
    {
        if !n.is_empty() && !numbers.contains(&n) {
            numbers.push(n);
        }
    }
    if numbers.is_empty() {
        return Ok(0);
    }
    // No ORDER BY in Python either: the database's order.
    let chats: Vec<WaChat> = sqlx::query_as(
        "SELECT * FROM wa_chats WHERE contact_id IS NULL AND staff_user_id IS NULL AND status != 'excluded' \
         AND (phone = ANY($1) OR phone IS NULL)",
    )
    .bind(&numbers)
    .fetch_all(&mut *db)
    .await?;
    let mut linked = 0;
    for mut chat in chats {
        let number = match chat.phone.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => Some(p.to_string()),
            None => deps::from_display_name(chat.display_name.as_deref(), &country),
        };
        let Some(number) = number.filter(|n| numbers.contains(n)) else {
            continue;
        };
        if chat.phone.as_deref().is_none_or(str::is_empty) {
            sqlx::query("UPDATE wa_chats SET phone = $2, updated_at = now() WHERE id = $1")
                .bind(chat.id)
                .bind(&number)
                .execute(&mut *db)
                .await?;
            chat.phone = Some(number.clone());
        }
        let evidence = json!([{"signal": "phone", "score": 100, "detail": format!("{number} saved on the contact")}]);
        deps::link_chat(
            db,
            &mut chat,
            contact.id,
            &_name(contact),
            evidence,
            "certain",
        )
        .await?;
        linked += 1;
    }
    Ok(linked)
}

/// Adapters over the other Nary modules (kept in one place so the wiring is easy to audit).
mod deps {
    use super::*;

    pub async fn default_country(db: &mut PgConnection) -> Result<String> {
        Ok(crate::nary::phones::default_country(db).await?)
    }
    pub fn to_e164(raw: Option<&str>, country: &str) -> Option<String> {
        crate::nary::phones::to_e164(raw, country)
    }
    pub fn from_display_name(name: Option<&str>, country: &str) -> Option<String> {
        crate::nary::phones::from_display_name(name, country)
    }
    /// `link_chat(db, chat, Candidate("contact", contact_id, label, 100, evidence), tier=tier)`.
    pub async fn link_chat(
        db: &mut PgConnection,
        chat: &mut WaChat,
        contact_id: Uuid,
        label: &str,
        evidence: serde_json::Value,
        tier: &str,
    ) -> Result<()> {
        let mut cand = crate::nary::identity_match::Candidate::new("contact", contact_id, label);
        cand.score = 100;
        cand.signals = evidence.as_array().cloned().unwrap_or_default();
        crate::nary::identity_match::link_chat(db, chat, &cand, tier, false, None, "linked")
            .await?;
        Ok(())
    }
}
