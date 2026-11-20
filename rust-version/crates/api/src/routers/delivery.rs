//! backend/app/routers/delivery.py, mounted at `/api/v1/documents` (backend/app/main.py).
//!
//! Send an approved document to its client / vendor.
//!
//!   GET  /documents/{entity_type}/{id}/send-defaults   pre-filled recipient (email, WhatsApp) for the dialog
//!   POST /documents/{entity_type}/{id}/prepare-send    "Send" clicked: submit for approval if needed (409 while
//!                                                      it waits), else return the pre-filled recipient
//!   POST /documents/{entity_type}/{id}/send            deliver the verified PDF, refused (409) until approved
//!
//! Not behind INTERNAL_ONLY: every route takes `InternalUser`.

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{Map, Value};

use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::{v, PyJson};
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::approvals::adapters::Entity;
use granary_services::approvals::{engine, notify};
use granary_services::verification::delivery;
use sqlx::PgConnection;

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/documents";

/// The routes of `router` in delivery.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/{entity_type}/{entity_id}/send-defaults"),
            get(send_defaults),
        )
        .route(
            &at(PREFIX, "/{entity_type}/{entity_id}/prepare-send"),
            post(prepare_send),
        )
        .route(
            &at(PREFIX, "/{entity_type}/{entity_id}/send"),
            post(send_document),
        )
}

/// `_require_send(user, entity_type)`.
fn _require_send(user: &AuthUser, entity_type: &str) -> ApiResult<()> {
    let Some(perm) = delivery::SENDABLE
        .iter()
        .find(|(t, _)| *t == entity_type)
        .map(|(_, p)| *p)
    else {
        return Err(ApiError::http(
            400,
            "This document type is not sent from here",
        ));
    };
    if !user.is_superadmin && !user.perms.contains(perm) {
        return Err(ApiError::http(403, format!("Permission required: {perm}")));
    }
    Ok(())
}

/// `_load(db, entity_type, entity_id)`.
async fn _load(db: &mut PgConnection, entity_type: &str, entity_id: &str) -> ApiResult<Entity> {
    let adapter = delivery::adapter_for(entity_type)?;
    match adapter.load(db, entity_id).await? {
        Some(e) => Ok(e),
        None => Err(ApiError::http(404, format!("{} not found", adapter.label))),
    }
}

/// `{**recipient_defaults, "approved": ..., "number": ...}`.
fn merged(defaults: Value, approved: bool, number: String) -> Value {
    let mut out = match defaults {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    out.insert("approved".into(), Value::Bool(approved));
    out.insert("number".into(), Value::String(number));
    Value::Object(out)
}

/// `GET /{entity_type}/{entity_id}/send-defaults`.
pub async fn send_defaults(
    Path((entity_type, entity_id)): Path<(String, String)>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    _require_send(&user, &entity_type)?;
    let mut conn = db.conn().await?;
    let entity = _load(&mut conn, &entity_type, &entity_id).await?;
    let defaults = delivery::recipient_defaults(&mut conn, &entity_type, &entity).await?;
    let approved = engine::is_approved(&mut conn, &entity_type, &entity).await?;
    let number = delivery::adapter_for(&entity_type)?.number(&entity);
    Ok(resp::json(merged(defaults, approved, number)))
}

/// `POST /{entity_type}/{entity_id}/prepare-send`: the first half of Send. An unapproved document is
/// submitted (one within standing authority, or the Owner's own, is approved on the spot) and 409 is returned
/// while it waits; an approved one gets its recipient.
pub async fn prepare_send(
    Path((entity_type, entity_id)): Path<(String, String)>,
    InternalUser(user): InternalUser,
    db: Db,
) -> ApiResult<Response> {
    _require_send(&user, &entity_type)?;
    let mut conn = db.conn().await?;
    let entity = _load(&mut conn, &entity_type, &entity_id).await?;
    engine::ensure_or_submit(&mut conn, &entity_type, &entity, &user, "sent", None).await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    let mut conn = db.conn().await?;
    // Python keeps using the entity it loaded before the commit (expired and refreshed on access): reload.
    let entity = match delivery::adapter_for(&entity_type)?
        .load(&mut conn, &entity_id)
        .await?
    {
        Some(e) => e,
        None => entity,
    };
    let defaults = delivery::recipient_defaults(&mut conn, &entity_type, &entity).await?;
    let number = delivery::adapter_for(&entity_type)?.number(&entity);
    Ok(resp::json(merged(defaults, true, number)))
}

granary_core::py_model! {
    /// `class SendBody(BaseModel)`.
    pub struct SendBody {
        pub channel: String,
        pub to: Vec<String> => v::list_c::<String>(v::ListC::new().min_length(1).max_length(10)),
        pub cc: Vec<String> = Vec::new() => v::list_c::<String>(v::ListC::new().max_length(10)),
        pub to_name: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(255))),
        pub message: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(4000))),
        /// Send again although the same document just went to the same recipient (see delivery._guard).
        pub force: bool = false,
        /// R&D 26 Oct: email blind copies (nobody sees them) and the CRM inbox it goes from
        pub bcc: Vec<String> = Vec::new() => v::list_c::<String>(v::ListC::new().max_length(10)),
        pub from_inbox: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(255))),
    }
}

/// `POST /{entity_type}/{entity_id}/send`.
pub async fn send_document(
    Path((entity_type, entity_id)): Path<(String, String)>,
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<SendBody>,
) -> ApiResult<Response> {
    _require_send(&user, &entity_type)?;
    let mut conn = db.conn().await?;
    let mut entity = _load(&mut conn, &entity_type, &entity_id).await?;
    let result = delivery::with_mail_options(
        body.from_inbox.clone(),
        body.bcc.clone(),
        delivery::send(
            &mut conn,
            &entity_type,
            &mut entity,
            &user,
            &body.channel,
            &body.to,
            Some(&body.cc),
            body.message.as_deref(),
            body.to_name.as_deref(),
            body.force,
        ),
    )
    .await?;
    drop(conn);
    db.commit().await?;
    notify::dispatch_outbox();
    Ok(resp::json(result))
}
