//! Rust-only router (27 Oct 2026, security triage item 18), mounted at `/api/v1/email/image-senders`.
//!
//! Received email bodies are shown with remote images hidden (they are read tracking). A user may choose
//! "Always show images from <sender>"; that list is kept per user here so it holds on every device.
//!
//! - `GET ""`: `{"senders": ["a@b.com", ...]}`, the logged-in user's senders, sorted.
//! - `POST ""` `{"email": "A@B.com"}`: adds the sender (trimmed, lower-cased); 201 `{"email": "a@b.com"}`.
//!   Adding one already on the list is not an error.
//! - `DELETE "?email=a@b.com"`: removes it (204, also when it was not on the list).
//!
//! Mounted INTERNAL_ONLY: portal users have no mailbox.

use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde_json::json;
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};

use super::at;
use crate::resp;

/// The prefix this router is mounted at.
pub const PREFIX: &str = "/api/v1/email/image-senders";

/// The routes of this module.
pub fn router() -> Router<AppState> {
    Router::new().route(
        &at(PREFIX, ""),
        get(list_senders).post(add_sender).delete(remove_sender),
    )
}

/// A sender address as stored: trimmed and lower-cased; a bare `local@domain` with no spaces or brackets.
fn normalize(email: &str) -> ApiResult<String> {
    let e = email.trim().to_lowercase();
    let ok = e.len() <= 320
        && !e
            .chars()
            .any(|c| c.is_whitespace() || "<>\"(),;:".contains(c))
        && matches!(e.split_once('@'), Some((l, d)) if !l.is_empty() && !d.is_empty() && !d.contains('@'));
    if ok {
        Ok(e)
    } else {
        Err(ApiError::http(422, "Enter a valid sender email address"))
    }
}

/// `GET ""`.
pub async fn list_senders(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT sender_email FROM email_image_senders WHERE user_id = $1 ORDER BY sender_email",
    )
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    let senders: Vec<String> = rows.into_iter().map(|r| r.0).collect();
    Ok(resp::json(json!({ "senders": senders })))
}

granary_core::py_model! {
    /// The body of `POST ""` and the query of `DELETE ""`.
    pub struct SenderBody {
        pub email: String,
    }
}

/// `POST ""` (201).
pub async fn add_sender(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<SenderBody>,
) -> ApiResult<Response> {
    let email = normalize(&body.email)?;
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO email_image_senders (id, user_id, sender_email) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, sender_email) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(&email)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({ "email": email })))
}

/// `DELETE "?email=..."` (204).
pub async fn remove_sender(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<SenderBody>,
) -> ApiResult<Response> {
    let email = normalize(&q.email)?;
    let mut conn = db.conn().await?;
    sqlx::query("DELETE FROM email_image_senders WHERE user_id = $1 AND sender_email = $2")
        .bind(user.id)
        .bind(&email)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
