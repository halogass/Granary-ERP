//! The routers/whatsapp.py helpers ai/tools/catalog.rs calls (`_require_inbox_access`, `fetch_conversations`,
//! `fetch_messages`). They live in the API crate (crates/api/src/routers/whatsapp.rs), which depends on this
//! one, so the router installs them here when it is built ([`install`]); these functions call through.
//! Without an installed router (a services-only test) they fail with a 500.
use std::sync::OnceLock;

use futures::future::BoxFuture;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use serde_json::Value;
use sqlx::PgConnection;

/// `fetch_conversations(db, limit=limit)`.
pub type FetchConversations =
    for<'a> fn(&'a mut PgConnection, i64) -> BoxFuture<'a, ApiResult<(Vec<Value>, bool)>>;
/// `fetch_messages(chat_id, limit)`.
pub type FetchMessages = fn(String, i64) -> BoxFuture<'static, ApiResult<(Value, bool, bool)>>;

/// The router functions (crates/api routers/whatsapp.rs).
#[derive(Clone, Copy)]
pub struct Hooks {
    pub require_inbox_access: fn(&AuthUser) -> ApiResult<()>,
    pub fetch_conversations: FetchConversations,
    pub fetch_messages: FetchMessages,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// Called by the WhatsApp router when it is built (first call wins).
pub fn install(hooks: Hooks) {
    let _ = HOOKS.set(hooks);
}

fn hooks() -> ApiResult<Hooks> {
    HOOKS.get().copied().ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!(
            "routers/whatsapp.rs is not installed (build the API app first)"
        ))
    })
}

/// routers/whatsapp.py `_require_inbox_access(user)`: superadmin passes; a non-internal user is
/// HTTPException(403, "No access to WhatsApp inbox"); otherwise crm:view_whatsapp is required.
pub fn _require_inbox_access(user: &AuthUser) -> ApiResult<()> {
    (hooks()?.require_inbox_access)(user)
}

/// routers/whatsapp.py `fetch_conversations(db, limit=limit)`: (the chat list page with CRM names, more?).
pub async fn fetch_conversations(
    db: &mut PgConnection,
    limit: i64,
) -> ApiResult<(Vec<Value>, bool)> {
    (hooks()?.fetch_conversations)(db, limit).await
}

/// routers/whatsapp.py `fetch_messages(chat_id, limit=limit)`: (messages, older known locally, history
/// exhausted). The messages are whatever the WA service returned (normally a list).
pub async fn fetch_messages(chat_id: &str, limit: i64) -> ApiResult<(Value, bool, bool)> {
    (hooks()?.fetch_messages)(chat_id.to_string(), limit).await
}
