//! backend/app/routers/email_signature.py, mounted at `/api/v1/email/signature` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (`InternalUser`; the asset route is public).
//!
//! Email signature: one company template (admin) + each user's field values.

use axum::extract::Path;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{str_value, PyValue};
use granary_compat::text::pystr;
use granary_compat::text::utils::py_uuid;
use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::PyJson;
use granary_core::{ApiError, ApiResult, AppState};
use granary_services::email_signature as sig;
use granary_services::email_signature::SignatureUser;

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/email/signature";

/// The routes of `router` in email_signature.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/template"),
            get(get_template).put(save_template),
        )
        .route(
            &at(PREFIX, "/me"),
            get(get_my_signature).put(save_my_signature),
        )
        .route(&at(PREFIX, "/render"), get(render_my_signature))
        .route(&at(PREFIX, "/assets/{asset_id}"), get(get_asset))
}

/// `_require_admin(user)`.
fn require_admin(user: &AuthUser) -> ApiResult<()> {
    if !(user.is_superadmin || user.is_system_admin) {
        return Err(ApiError::http(
            403,
            "Only admins can edit the signature template",
        ));
    }
    Ok(())
}

/// `_set_setting(db, key, value, label)`.
async fn set_setting(db: &mut PgConnection, key: &str, value: &str, label: &str) -> ApiResult<()> {
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM system_settings WHERE system_settings.key = $1")
            .bind(key)
            .fetch_optional(&mut *db)
            .await?;
    match found {
        Some((id,)) => {
            sqlx::query(
                "UPDATE system_settings SET value = $1, updated_at = now() WHERE system_settings.id = $2",
            )
            .bind(value)
            .bind(id)
            .execute(&mut *db)
            .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO system_settings (key, value, value_type, category, label, is_public, \
                 is_encrypted, id) VALUES ($1, $2, 'string', 'email', $3, false, false, $4)",
            )
            .bind(key)
            .bind(value)
            .bind(label)
            .bind(Uuid::new_v4())
            .execute(&mut *db)
            .await?;
        }
    }
    Ok(())
}

fn fields_json(spec: &[sig::FieldSpec]) -> Value {
    Value::Array(
        spec.iter()
            .map(|f| json!({"name": f.name, "label": f.label}))
            .collect(),
    )
}

/// The signature columns of a user, re-read from the database (the request's `current_user`).
#[allow(clippy::type_complexity)]
async fn signature_user(db: &mut PgConnection, user_id: Uuid) -> ApiResult<SignatureUser> {
    let row: (
        String,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
        Option<bool>,
        Option<Value>,
    ) = sqlx::query_as(
        "SELECT name, position, department, email, phone, email_signature_enabled, \
         email_signature_fields FROM users WHERE users.id = $1",
    )
    .bind(user_id)
    .fetch_one(&mut *db)
    .await?;
    Ok(SignatureUser {
        name: Some(row.0),
        position: row.1,
        department: row.2,
        email: Some(row.3),
        phone: row.4,
        email_signature_enabled: row.5,
        email_signature_fields: row.6,
    })
}

// ─── Template (admin) ────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class TemplateBody(BaseModel)`.
    pub struct TemplateBody {
        pub html: String,
        pub enabled: bool = true,
    }
}

/// `GET "/template"`.
pub async fn get_template(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    let html = sig::get_template(&mut conn).await?;
    let enabled = sig::is_enabled(&mut conn).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM email_signature_assets")
        .fetch_one(&mut *conn)
        .await?;
    Ok(resp::json(json!({
        "html": html,
        "enabled": enabled,
        "fields": fields_json(&sig::field_spec(&html)),
        "asset_count": count,
    })))
}

/// `PUT "/template"`: save the template, re-hosting any images it points at.
pub async fn save_template(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<TemplateBody>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let html = sig::strip_document_shell(&body.html);
    let mut conn = db.conn().await?;
    let (html, imported, problems) = sig::import_external_images(&mut conn, &html).await?;
    set_setting(
        &mut conn,
        sig::TEMPLATE_KEY,
        &html,
        "Email signature template",
    )
    .await?;
    set_setting(
        &mut conn,
        sig::ENABLED_KEY,
        if body.enabled { "true" } else { "false" },
        "Email signature enabled",
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({
        "html": html,
        "enabled": body.enabled,
        "fields": fields_json(&sig::field_spec(&html)),
        "images_imported": imported,
        "problems": problems,
    })))
}

// ─── Per-user values ─────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class MySignatureBody(BaseModel)`.
    pub struct MySignatureBody {
        pub fields: Map<String, Value> = Map::new(),
        pub enabled: bool = true,
    }
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(v)`.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => str_value(&PyValue::from(other)),
    }
}

/// `GET "/me"`: the form to show this user, their saved values, and the rendered preview.
pub async fn get_my_signature(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let template = sig::get_template(&mut conn).await?;
    let me = signature_user(&mut conn, user.id).await?;
    let saved: Map<String, Value> = match &me.email_signature_fields {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    let company = granary_core::company::get_company_info(&mut conn)
        .await?
        .to_map();
    let defaults = sig::defaults_for_user(&me, Some(&company));
    let spec = sig::field_spec(&template);

    let mut fields: Vec<Value> = Vec::new();
    let mut missing: Vec<Value> = Vec::new();
    for f in &spec {
        let value = saved
            .get(&f.name)
            .cloned()
            .or_else(|| defaults.get(&f.name).cloned())
            .unwrap_or_else(|| Value::String(String::new()));
        // str(f["value"] or "").strip()
        let text = if truthy(&value) {
            py_str(&value)
        } else {
            String::new()
        };
        if pystr::strip(&text).is_empty() {
            missing.push(Value::String(f.label.clone()));
        }
        fields.push(json!({"name": f.name, "label": f.label, "value": value}));
    }
    let company_enabled = sig::is_enabled(&mut conn).await?;
    let html = sig::render_for_user(&mut conn, &me).await?;
    Ok(resp::json(json!({
        "configured": !pystr::strip(&template).is_empty(),
        "company_enabled": company_enabled,
        "enabled": me.email_signature_enabled != Some(false),
        "fields": fields,
        "complete": missing.is_empty(),
        "missing": missing,
        "html": html,
    })))
}

/// Python `s[:n]` in code points.
fn py_prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `PUT "/me"`.
pub async fn save_my_signature(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<MySignatureBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let template = sig::get_template(&mut conn).await?;
    let allowed: Vec<String> = sig::field_spec(&template)
        .into_iter()
        .map(|f| f.name)
        .collect();
    let mut clean = Map::new();
    for (k, v) in &body.fields {
        if allowed.contains(k) {
            clean.insert(k.clone(), Value::String(py_prefix(&py_str(v), 300)));
        }
    }
    sqlx::query(
        "UPDATE users SET email_signature_fields = $1, email_signature_enabled = $2, updated_at = now() \
         WHERE users.id = $3",
    )
    .bind(Value::Object(clean))
    .bind(body.enabled)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    // await db.refresh(user)
    let mut conn = db.conn().await?;
    let me = signature_user(&mut conn, user.id).await?;
    let html = sig::render_for_user(&mut conn, &me).await?;
    Ok(resp::json(json!({
        "enabled": me.email_signature_enabled,
        "html": html,
    })))
}

/// `GET "/render"`: what the composer inserts. Empty string when nothing is configured.
pub async fn render_my_signature(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let me = signature_user(&mut conn, user.id).await?;
    let html = sig::render_for_user(&mut conn, &me).await?;
    Ok(resp::json(json!({"html": html})))
}

// ─── Assets ──────────────────────────────────────────────────────────────────

/// `GET "/assets/{asset_id}"`: signature images, deliberately unauthenticated (public brand assets that
/// <img> tags load while composing).
pub async fn get_asset(db: Db, Path(asset_id): Path<String>) -> ApiResult<Response> {
    let not_found = || ApiError::http(404, "Asset not found");
    // A malformed id fails the query in Python, which the `except Exception` turns into "not found".
    let Some(id) = py_uuid(&asset_id) else {
        return Err(not_found());
    };
    let mut conn = db.conn().await?;
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT mime_type, object_key FROM email_signature_assets WHERE email_signature_assets.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .unwrap_or(None);
    drop(conn);
    let Some((mime_type, object_key)) = row else {
        return Err(not_found());
    };
    // sig.read_asset_bytes(asset)
    let bucket = granary_core::config::settings().MINIO_BUCKET_ASSETS.clone();
    let data = granary_services::storage::minio()
        .get_object(&bucket, &object_key)
        .await
        .map_err(|_| ApiError::http(404, "Asset file missing from storage"))?;
    let media = if mime_type.is_empty() {
        "image/png".to_string()
    } else {
        mime_type
    };
    // security 26 Oct: an imported image's type came from another server; SVG and the like download instead
    let name = object_key.rsplit('/').next().unwrap_or("image");
    Ok(resp::with_headers(
        crate::files::safe_file_response(data, &media, name, true),
        &[("cache-control", "public, max-age=86400".to_string())],
    ))
}
