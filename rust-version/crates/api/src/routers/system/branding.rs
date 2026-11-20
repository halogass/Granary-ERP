//! system.py, "Branding (logo)" and "Letterhead": `/branding`, `/branding/logo`, `/branding/favicon`,
//! `/settings/letterhead`.

use axum::response::Response;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{Cx, PyJson};
use granary_core::{ApiError, ApiResult};

use super::ai::py_prefix;
use super::require_admin;
use crate::extract::PyForm;
use crate::resp;

/// SVG is not accepted for the logo or the favicon: an SVG can carry script, and the favicon is served as a file.
pub const LOGO_ALLOWED_TYPES: &[&str] = &["image/png", "image/jpeg", "image/jpg", "image/webp"];
pub const LOGO_MAX_BYTES: usize = 2 * 1024 * 1024;

const LOGIN_FEATURE_DEFAULTS: &str = "AI-drafted quotations, proposals and replies\nLead enrichment, research and scoring\nEmail and WhatsApp triage with suggested replies\nEvery AI action approved by a person\nFinance, HR, projects and procurement in one place\nConnect your own AI client over MCP";

/// `GET "/branding"`: public (login page and sidebar).
pub async fn get_branding(db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT system_settings.key, system_settings.value FROM system_settings \
         WHERE system_settings.category = 'branding'",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut settings: Map<String, Value> = Map::new();
    for (k, v) in rows {
        settings.insert(
            k.replace("branding.", ""),
            Value::String(v.unwrap_or_default()),
        );
    }
    let get = |k: &str, d: &str| {
        settings
            .get(k)
            .cloned()
            .unwrap_or_else(|| Value::String(d.to_string()))
    };
    Ok(resp::json(json!({
        "logo_type": get("logo_type", "text"),
        "logo_text": get("logo_text", "Granary"),
        "logo_subtitle": get("logo_subtitle", "ERP"),
        "logo_image_data": get("logo_image_data", ""),
        "favicon_data": get("favicon_data", ""),
        "login_badge": get("login_badge", "GRANARY"),
        "login_headline": get("login_headline", "The ERP that thinks"),
        "login_headline_accent": get("login_headline_accent", "alongside your team."),
        "login_tagline": get("login_tagline", "Nary, the built-in AI, works across sales, finance, people and operations: it drafts the next step and waits for your approval."),
        "login_features": get("login_features", LOGIN_FEATURE_DEFAULTS),
        "login_footer": get("login_footer", "Open-source ERP with built-in AI"),
        "login_subtitle": get("login_subtitle", "Sign in to continue."),
        "login_copyright": get("login_copyright", "Open-source ERP with built-in AI"),
    })))
}

granary_core::py_model! {
    /// `class BrandingTextUpdate(BaseModel)`.
    pub struct BrandingTextUpdate {
        pub logo_type: Option<String> = None,
        pub logo_text: Option<String> = None,
        pub logo_subtitle: Option<String> = None,
        pub login_badge: Option<String> = None,
        pub login_headline: Option<String> = None,
        pub login_headline_accent: Option<String> = None,
        pub login_tagline: Option<String> = None,
        pub login_features: Option<String> = None,
        pub login_footer: Option<String> = None,
        pub login_subtitle: Option<String> = None,
        pub login_copyright: Option<String> = None,
    }
}

/// The branding upsert: an existing row gets the value and `updated_by_id`; a new one is a public
/// `branding` string setting (`label` only where Python gives one).
async fn upsert_branding(
    conn: &mut PgConnection,
    key: &str,
    value: &str,
    user_id: Uuid,
    label: Option<&str>,
) -> ApiResult<()> {
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM system_settings WHERE system_settings.key = $1")
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?;
    match found {
        Some((id,)) => {
            sqlx::query(
                "UPDATE system_settings SET value = $1, updated_by_id = $2, updated_at = now() \
                 WHERE system_settings.id = $3",
            )
            .bind(value)
            .bind(user_id)
            .bind(id)
            .execute(&mut *conn)
            .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO system_settings (key, value, value_type, category, label, is_public, is_encrypted, id) \
                 VALUES ($1, $2, 'string', 'branding', $3, true, false, $4)",
            )
            .bind(key)
            .bind(value)
            .bind(label)
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

/// `PATCH "/branding"`.
pub async fn update_branding(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<BrandingTextUpdate>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut updates: Vec<(&str, String)> = Vec::new();
    if let Some(t) = &body.logo_type {
        if t != "text" && t != "image" {
            return Err(ApiError::http(400, "logo_type must be 'text' or 'image'"));
        }
        updates.push(("branding.logo_type", t.clone()));
    }
    let fields: [(&Option<String>, &str, usize); 10] = [
        (&body.logo_text, "branding.logo_text", 100),
        (&body.logo_subtitle, "branding.logo_subtitle", 100),
        (&body.login_badge, "branding.login_badge", 100),
        (&body.login_headline, "branding.login_headline", 200),
        (
            &body.login_headline_accent,
            "branding.login_headline_accent",
            200,
        ),
        (&body.login_tagline, "branding.login_tagline", 500),
        (&body.login_features, "branding.login_features", 2000),
        (&body.login_footer, "branding.login_footer", 200),
        (&body.login_subtitle, "branding.login_subtitle", 200),
        (&body.login_copyright, "branding.login_copyright", 200),
    ];
    for (v, key, n) in fields {
        if let Some(v) = v {
            updates.push((key, py_prefix(v, n)));
        }
    }
    let mut conn = db.conn().await?;
    for (key, value) in &updates {
        upsert_branding(&mut conn, key, value, user.id, None).await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "ok"})))
}

/// What an uploaded logo or favicon really is, from its first bytes (never from its name or the type the browser
/// claims): PNG, JPEG, WebP or ICO.
pub fn sniff_image(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if data.len() >= 6 && data[..4] == [0, 0, 1, 0] {
        return Some("image/x-icon");
    }
    None
}

/// The image kinds (as `sniff_image` names them) each stored branding image may hold.
fn sniffed_kinds_for(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "branding.logo_image_data" => Some(&["image/png", "image/jpeg", "image/webp"]),
        "branding.favicon_data" => Some(&["image/png", "image/jpeg", "image/x-icon"]),
        _ => None,
    }
}

/// The checked type of an uploaded branding image: its bytes must be one of the kinds `key` may hold.
fn checked_image(key: &str, data: &[u8], kinds_text: &str) -> ApiResult<&'static str> {
    let allowed = sniffed_kinds_for(key).unwrap_or(&[]);
    match sniff_image(data) {
        Some(kind) if allowed.contains(&kind) => Ok(kind),
        _ => Err(ApiError::http(
            400,
            format!("The file is not a {kinds_text} image. Choose another file."),
        )),
    }
}

/// The generic `PATCH /settings/{key}` for a `branding.*_data` key: only an empty value or a base64 data URL of
/// a PNG, JPEG, WebP (logo) or ICO (favicon) image whose bytes match its declared type. None for other keys.
pub fn validate_image_setting(key: &str, value: &str) -> Option<Result<(), String>> {
    if !(key.starts_with("branding.") && key.ends_with("_data")) {
        return None;
    }
    let Some(allowed) = sniffed_kinds_for(key) else {
        return Some(Err(format!(
            "{key} can't be set here: upload images in Settings, Company & branding."
        )));
    };
    if value.is_empty() {
        return Some(Ok(()));
    }
    let refused = || {
        Err(format!(
            "{key} must be a base64 data URL of a PNG, JPEG, WebP or ICO image (upload it in Settings, Company & \
             branding)."
        ))
    };
    let Some((mime, b64)) = value
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(";base64,"))
    else {
        return Some(refused());
    };
    let mime = match mime {
        "image/vnd.microsoft.icon" => "image/x-icon",
        "image/jpg" => "image/jpeg",
        m => m,
    };
    let Ok(bytes) = granary_compat::crypto::pyb64::b64decode_str(b64) else {
        return Some(refused());
    };
    match sniff_image(&bytes) {
        Some(kind) if kind == mime && allowed.contains(&kind) => Some(Ok(())),
        _ => Some(refused()),
    }
}

fn data_url(content_type: &str, data: &[u8]) -> String {
    format!(
        "data:{content_type};base64,{}",
        granary_compat::crypto::b64_std(data)
    )
}

/// `str(x)` of an optional content type (`None` prints as "None").
fn ct_str(ct: &Option<String>) -> String {
    ct.clone().unwrap_or_else(|| "None".to_string())
}

/// `POST "/branding/logo"`.
pub async fn upload_logo(
    CurrentUser(user): CurrentUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = Cx::new();
    let file = form.req_file(&mut cx, "file");
    form.check(cx)?;
    let file = file.unwrap();
    require_admin(&user)?;
    let ct = file.content_type.clone();
    if !ct
        .as_deref()
        .is_some_and(|c| LOGO_ALLOWED_TYPES.contains(&c))
    {
        return Err(ApiError::http(
            400,
            format!("Unsupported type: {}", ct_str(&ct)),
        ));
    }
    if file.data.len() > LOGO_MAX_BYTES {
        return Err(ApiError::http(400, "Logo image must be under 2 MB"));
    }
    let kind = checked_image("branding.logo_image_data", &file.data, "PNG, JPEG or WebP")?;
    let url = data_url(kind, &file.data);
    let mut conn = db.conn().await?;
    for (key, value) in [
        ("branding.logo_image_data", url.as_str()),
        ("branding.logo_type", "image"),
    ] {
        upsert_branding(&mut conn, key, value, user.id, None).await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"data_url": url})))
}

/// Sets `value` and `updated_by_id` on an existing setting row; nothing when it is absent.
async fn update_if_exists(
    conn: &mut PgConnection,
    key: &str,
    value: &str,
    user_id: Uuid,
) -> ApiResult<bool> {
    let n = sqlx::query(
        "UPDATE system_settings SET value = $1, updated_by_id = $2, updated_at = now() \
         WHERE system_settings.key = $3",
    )
    .bind(value)
    .bind(user_id)
    .bind(key)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// `DELETE "/branding/logo"` (204).
pub async fn delete_logo(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    for (key, empty) in [
        ("branding.logo_image_data", ""),
        ("branding.logo_type", "text"),
    ] {
        update_if_exists(&mut conn, key, empty, user.id).await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

pub const FAVICON_ALLOWED_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/jpg",
    "image/x-icon",
    "image/vnd.microsoft.icon",
];
pub const FAVICON_MAX_BYTES: usize = 512 * 1024;

/// `POST "/branding/favicon"`.
pub async fn upload_favicon(
    CurrentUser(user): CurrentUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = Cx::new();
    let file = form.req_file(&mut cx, "file");
    form.check(cx)?;
    let file = file.unwrap();
    require_admin(&user)?;
    let ct = file.content_type.clone().unwrap_or_default();
    if !FAVICON_ALLOWED_TYPES.contains(&ct.as_str()) {
        return Err(ApiError::http(
            400,
            format!("Unsupported type: {ct}. Use PNG, JPEG or ICO."),
        ));
    }
    if file.data.len() > FAVICON_MAX_BYTES {
        return Err(ApiError::http(400, "Favicon must be under 512 KB"));
    }
    let kind = checked_image("branding.favicon_data", &file.data, "PNG, JPEG or ICO")?;
    let url = data_url(kind, &file.data);
    let mut conn = db.conn().await?;
    upsert_branding(
        &mut conn,
        "branding.favicon_data",
        &url,
        user.id,
        Some("Favicon (base64 data URL)"),
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"data_url": url})))
}

/// `DELETE "/branding/favicon"` (204).
pub async fn delete_favicon(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    if update_if_exists(&mut conn, "branding.favicon_data", "", user.id).await? {
        drop(conn);
        db.commit().await?;
    }
    Ok(resp::no_content())
}

// ─── Letterhead ───────────────────────────────────────────────────────────────

pub const LETTERHEAD_KEY: &str = "company.letterhead_image";
pub const LETTERHEAD_ALLOWED_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/jpg",
    "image/svg+xml",
    "image/webp",
];
pub const LETTERHEAD_MAX_BYTES: usize = 5 * 1024 * 1024;

/// `_require_branding_permission(user)`: superadmin or system_admin.
fn require_branding_permission(user: &AuthUser) -> ApiResult<()> {
    if !(user.is_superadmin || user.is_system_admin) {
        return Err(ApiError::http(
            403,
            "Executive / system admin required to manage company branding",
        ));
    }
    Ok(())
}

/// `GET "/settings/letterhead"`: public.
pub async fn get_letterhead(db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM system_settings WHERE system_settings.key = $1")
            .bind(LETTERHEAD_KEY)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(resp::json(json!({"data_url": row.and_then(|r| r.0)})))
}

/// `POST "/settings/letterhead"`: upload or replace the letterhead image.
pub async fn upload_letterhead(
    CurrentUser(user): CurrentUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = Cx::new();
    let file = form.req_file(&mut cx, "file");
    form.check(cx)?;
    let file = file.unwrap();
    require_branding_permission(&user)?;
    let ct = file.content_type.clone().unwrap_or_default().to_lowercase();
    if !LETTERHEAD_ALLOWED_TYPES.contains(&ct.as_str()) {
        return Err(ApiError::http(
            422,
            "Only PNG, JPG, SVG, or WebP images are accepted",
        ));
    }
    if file.data.len() > LETTERHEAD_MAX_BYTES {
        return Err(ApiError::http(422, "Image must be smaller than 5 MB"));
    }
    let url = data_url(&ct, &file.data);
    let mut conn = db.conn().await?;
    if !update_if_exists(&mut conn, LETTERHEAD_KEY, &url, user.id).await? {
        sqlx::query(
            "INSERT INTO system_settings (key, value, value_type, category, label, description, is_public, \
             is_encrypted, updated_by_id, id) VALUES ($1, $2, 'string', 'company', 'Company Letterhead Image', \
             'Header image used in quotations, invoices, and other documents', true, false, $3, $4)",
        )
        .bind(LETTERHEAD_KEY)
        .bind(&url)
        .bind(user.id)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"message": "Letterhead updated"})))
}

/// `DELETE "/settings/letterhead"` (204).
pub async fn delete_letterhead(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_branding_permission(&user)?;
    let mut conn = db.conn().await?;
    let n = sqlx::query("DELETE FROM system_settings WHERE system_settings.key = $1")
        .bind(LETTERHEAD_KEY)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    drop(conn);
    if n > 0 {
        db.commit().await?;
    }
    Ok(resp::no_content())
}
