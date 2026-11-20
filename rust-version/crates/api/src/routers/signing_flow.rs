//! backend/app/routers/signing_flow.py, mounted at `/api/v1/sign` (backend/app/main.py) next to signing.py.
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (`InternalUser`).
//!
//!   POST /sign/from-document                       render + QR + signers + invite, for an approved document
//!   GET  /sign/from-document/{entity_type}/{id}    latest signature request for a document
//!   POST /sign/documents/{doc_id}/extend           push the counterparty deadline (capped by the policy)

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use uuid::Uuid;

use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::validation::{v, PyJson};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::signing::SignDocument;
use granary_services::verification::signing_flow::{self, PendingInvitations};

use super::at;
use super::signing::{require_sign_manage, require_sign_view};
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/sign";

/// The routes of `router` in signing_flow.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/from-document"), post(sign_from_document))
        .route(
            &at(PREFIX, "/from-document/{entity_type}/{entity_id}"),
            get(signature_for_document),
        )
        .route(
            &at(PREFIX, "/documents/{doc_id}/extend"),
            post(extend_signing_deadline),
        )
}

granary_core::py_model! {
    /// `class FromDocumentBody(BaseModel)`.
    pub struct FromDocumentBody {
        pub entity_type: String,
        pub entity_id: String,
        pub message: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(2000))),
    }
}

/// `POST /from-document` (201): starts the signature request of an approved document.
pub async fn sign_from_document(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<FromDocumentBody>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    let mut invites = PendingInvitations::default();
    let mut conn = db.conn().await?;
    signing_flow::start_from_document(
        &mut conn,
        &body.entity_type,
        &body.entity_id,
        &user,
        body.message.as_deref(),
        &mut invites,
    )
    .await?;
    drop(conn);
    db.commit().await?;
    signing_flow::dispatch_invitations(&mut invites);
    let mut conn = db.conn().await?;
    let out =
        signing_flow::status_for_document(&mut conn, &body.entity_type, &body.entity_id).await?;
    Ok(resp::json_status(201, out))
}

/// `GET /from-document/{entity_type}/{entity_id}`.
pub async fn signature_for_document(
    InternalUser(user): InternalUser,
    db: Db,
    Path((entity_type, entity_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require_sign_view(&user)?;
    let mut conn = db.conn().await?;
    let out = signing_flow::status_for_document(&mut conn, &entity_type, &entity_id).await?;
    Ok(resp::json(out))
}

granary_core::py_model! {
    /// `class ExtendBody(BaseModel)`.
    pub struct ExtendBody {
        pub days: i64 => v::int_c(v::NumC::new().ge("1").le("30")),
    }
}

/// `POST /documents/{doc_id}/extend`: a later counterparty deadline, capped by the policy.
pub async fn extend_signing_deadline(
    InternalUser(user): InternalUser,
    db: Db,
    Path(doc_id): Path<String>,
    PyJson(body): PyJson<ExtendBody>,
) -> ApiResult<Response> {
    require_sign_manage(&user)?;
    // uuid.UUID(doc_id): ValueError -> 404.
    let Ok(uid) = granary_compat::text::utils::parse_uuid(&doc_id, "doc_id") else {
        return Err(ApiError::http(404, "Document not found"));
    };
    let uid: Uuid = uid;
    let mut conn = db.conn().await?;
    let doc: Option<SignDocument> =
        sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1 AND is_deleted = false")
            .bind(uid)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(mut doc) = doc else {
        return Err(ApiError::http(404, "Document not found"));
    };
    signing_flow::extend_deadline(&mut conn, &mut doc, body.days, &user).await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let source_type = doc.source_type.clone().unwrap_or_else(|| "None".into());
    let source_id = doc
        .source_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| "None".into());
    let out = signing_flow::status_for_document(&mut conn, &source_type, &source_id).await?;
    Ok(resp::json(out))
}
