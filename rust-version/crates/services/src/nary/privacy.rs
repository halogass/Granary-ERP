//! Port of backend/app/services/nary/privacy.py: the privacy gate. Personal WhatsApp chats never reach
//! the AI (design 4.5, Owner rule).
//!
//! Two kinds of chat are blocked:
//! - excluded: ticked as personal on the history review screen, or answered "personal" on a held-chat
//!   card. The webhook drops their messages before anything is stored (wa_ingest::ingest).
//! - held: a number saved on the company phone that the CRM doesn't know. Stored, but nothing reads it
//!   until the Owner says business or personal.
//!
//! Every code path that hands WhatsApp content to a model calls `assert_ai_allowed` first.
//!
//! Port note: `PrivateChat` is a plain Python `Exception` (a 500 when it escapes). Converted into
//! `ApiError::Internal` it keeps its type, so a caller several modules up (channels::on_inbound) finds it
//! again with [`private_chat_of`].

use granary_core::errors::ApiError;
use sqlx::PgConnection;

pub const BLOCKED: [&str; 2] = ["held", "excluded"];

/// Raised instead of sending a personal chat's content to a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateChat {
    pub chat_id: String,
    pub status: String,
}

impl std::fmt::Display for PrivateChat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "WhatsApp chat {} is {}: its content never reaches the AI",
            self.chat_id, self.status
        )
    }
}

impl std::error::Error for PrivateChat {}

impl From<PrivateChat> for ApiError {
    fn from(e: PrivateChat) -> ApiError {
        ApiError::Internal(anyhow::Error::new(e))
    }
}

/// The `PrivateChat` inside an error that came from [`assert_ai_allowed`], if it is one.
pub fn private_chat_of(e: &ApiError) -> Option<&PrivateChat> {
    match e {
        ApiError::Internal(inner) => inner.downcast_ref::<PrivateChat>(),
        _ => None,
    }
}

/// Errors of [`assert_ai_allowed`]: the gate itself, or the database.
#[derive(Debug)]
pub enum PrivacyError {
    Private(PrivateChat),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for PrivacyError {
    fn from(e: sqlx::Error) -> Self {
        PrivacyError::Db(e)
    }
}

impl From<PrivacyError> for ApiError {
    fn from(e: PrivacyError) -> ApiError {
        match e {
            PrivacyError::Private(p) => p.into(),
            PrivacyError::Db(d) => d.into(),
        }
    }
}

pub async fn chat_status(
    db: &mut PgConnection,
    chat_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT status FROM wa_chats WHERE chat_id = $1")
        .bind(chat_id)
        .fetch_optional(db)
        .await
}

/// True only for a chat Nary knows and that is active (an unknown chat id is refused too).
pub async fn ai_allowed(db: &mut PgConnection, chat_id: &str) -> Result<bool, sqlx::Error> {
    Ok(chat_status(db, chat_id).await?.as_deref() == Some("active"))
}

pub async fn assert_ai_allowed(db: &mut PgConnection, chat_id: &str) -> Result<(), PrivacyError> {
    let status = chat_status(db, chat_id).await?;
    if status.as_deref() != Some("active") {
        return Err(PrivacyError::Private(PrivateChat {
            chat_id: chat_id.to_string(),
            status: status.unwrap_or_else(|| "unknown".to_string()),
        }));
    }
    Ok(())
}
