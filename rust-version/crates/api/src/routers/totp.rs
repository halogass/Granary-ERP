//! Rust-only (2 Nov 2026): TOTP two-factor sign-in, the person's own setup (Profile > Security) and the admin
//! reset (Settings > Users & roles). The logic is `granary_services::totp`; the login's second step is
//! `POST /api/v1/auth/login/totp` in auth.rs.
//!
//! Not behind INTERNAL_ONLY: portal users can protect their accounts too. The admin routes check the caller.

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::validation::PyJson;
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::totp;

use super::at;
use crate::extract::pg_uuid;
use crate::resp;

/// The prefix of the person's own routes (auth.py's).
pub const PREFIX: &str = "/api/v1/auth";
const USERS_PREFIX: &str = "/api/v1/users";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/totp"), get(my_status))
        .route(&at(PREFIX, "/totp/setup"), post(setup))
        .route(&at(PREFIX, "/totp/confirm"), post(confirm))
        .route(&at(PREFIX, "/totp/disable"), post(disable))
        .route(&at(PREFIX, "/totp/recovery-codes"), post(regenerate))
        .route(
            &at(USERS_PREFIX, "/{user_id}/totp"),
            get(user_status).delete(reset_user),
        )
}

granary_core::py_model! {
    /// A code from the authenticator app (or, to turn TOTP off, a recovery code).
    pub struct CodeBody {
        pub code: String,
    }
}

/// Secrets and recovery codes must not be cached anywhere on the way.
fn no_store(body: Value) -> Response {
    resp::with_headers(
        resp::json(body),
        &[("cache-control", "no-store".to_string())],
    )
}

/// `GET /auth/totp`: the caller's status; `required` when their role must have two-factor sign-in.
pub async fn my_status(CurrentUser(u): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let st = totp::status(&mut conn, u.id).await?;
    let mut body = st.to_json();
    body["required"] = Value::Bool(u.perms.contains(totp::REQUIRED_PERMISSION));
    Ok(resp::json(body))
}

granary_core::py_model! {
    /// Stage 8: the account password, asked again before a new secret is made.
    pub struct SetupBody {
        pub password: String = String::new(),
    }
}

/// `POST /auth/totp/setup`: a new secret as a QR and text; on only after `/confirm`. Stage 8: needs the current
/// password (a stolen session must not bind its own authenticator to the account).
pub async fn setup(
    CurrentUser(u): CurrentUser,
    db: Db,
    PyJson(body): PyJson<SetupBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(u.id)
        .fetch_one(&mut *conn)
        .await?;
    let ok = !body.password.is_empty()
        && granary_compat::crypto::password::verify_password(&body.password, &hash)
            .unwrap_or(false);
    if !ok {
        return Err(ApiError::app(
            403,
            "Enter your current password to set up two-factor sign-in.",
            Some("password_required"),
        ));
    }
    let s = totp::begin_setup(&mut conn, u.id, &u.email).await?;
    drop(conn);
    db.commit().await?;
    Ok(no_store(s.to_json()))
}

/// `POST /auth/totp/confirm`: a valid code turns TOTP on; the recovery codes are returned this once.
pub async fn confirm(
    CurrentUser(u): CurrentUser,
    db: Db,
    PyJson(body): PyJson<CodeBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let codes = totp::confirm_setup(&mut conn, u.id, &body.code).await?;
    drop(conn);
    db.commit().await?;
    Ok(no_store(json!({"enabled": true, "recovery_codes": codes})))
}

/// `POST /auth/totp/disable`: needs a current code or a recovery code.
pub async fn disable(
    CurrentUser(u): CurrentUser,
    db: Db,
    PyJson(body): PyJson<CodeBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    totp::disable(&mut conn, u.id, &body.code).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"enabled": false})))
}

/// `POST /auth/totp/recovery-codes`: new recovery codes (the old ones stop working); needs a current code.
pub async fn regenerate(
    CurrentUser(u): CurrentUser,
    db: Db,
    PyJson(body): PyJson<CodeBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let codes = totp::regenerate_recovery_codes(&mut conn, u.id, &body.code).await?;
    drop(conn);
    db.commit().await?;
    Ok(no_store(json!({"recovery_codes": codes})))
}

/// The Owner (superadmin) or a system admin; only the superadmin touches the superadmin's TOTP.
async fn admin_target(
    me: &granary_core::auth::AuthUser,
    conn: &mut sqlx::PgConnection,
    user_id: &str,
) -> ApiResult<uuid::Uuid> {
    if !(me.is_superadmin || me.is_system_admin) {
        return Err(ApiError::http(403, "System admin required"));
    }
    let id = pg_uuid(user_id)?;
    let target: Option<bool> = sqlx::query_scalar("SELECT is_superadmin FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(target_is_superadmin) = target else {
        return Err(ApiError::not_found("User", Some(user_id)));
    };
    if target_is_superadmin && !me.is_superadmin {
        return Err(ApiError::http(
            403,
            "Only the superadmin can reset the superadmin's two-factor sign-in",
        ));
    }
    Ok(id)
}

/// `GET /users/{user_id}/totp`: whether the user has TOTP on (admins).
pub async fn user_status(
    Path(user_id): Path<String>,
    CurrentUser(me): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    if !(me.is_superadmin || me.is_system_admin) {
        return Err(ApiError::http(403, "System admin required"));
    }
    let id = pg_uuid(&user_id)?;
    let mut conn = db.conn().await?;
    let st = totp::status(&mut conn, id).await?;
    Ok(resp::json(json!({
        "enabled": st.enabled,
        "confirmed_at": st.confirmed_at.map(|t| t.to_rfc3339()),
        "recovery_codes_left": st.recovery_codes_left,
    })))
}

/// `DELETE /users/{user_id}/totp`: resets the user's TOTP (audit-logged); they must set it up again.
pub async fn reset_user(
    Path(user_id): Path<String>,
    CurrentUser(me): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let id = admin_target(&me, &mut conn, &user_id).await?;
    let had = totp::admin_reset(&mut conn, me.id, id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"reset": had, "enabled": false})))
}
