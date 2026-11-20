//! system.py, "Email Configuration": `/email-config` (GET/PATCH), `/email-config/test`,
//! `/email-config/test-imap`.

use std::time::Duration;

use axum::response::Response;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::validation::PyJson;
use granary_core::{ApiError, ApiResult};
use granary_services::email::{send_test_email, SmtpConfig, ENCRYPTED_EMAIL_KEYS};
use granary_services::email_crypto::{decrypt, encrypt};
use granary_services::imap_sync::{Imap4, ImapError};

use super::require_admin;
use crate::resp;

pub const ALL_EMAIL_KEYS: &[&str] = &[
    "email_smtp_host",
    "email_smtp_port",
    "email_smtp_user",
    "email_smtp_password",
    "email_smtp_from_name",
    "email_smtp_from_email",
    "email_smtp_tls",
    "crm_smtp_host",
    "crm_smtp_port",
    "crm_smtp_user",
    "crm_smtp_password",
    "crm_smtp_from_name",
    "crm_smtp_from_email",
    "crm_smtp_tls",
    "crm_imap_host",
    "crm_imap_port",
    "crm_imap_user",
    "crm_imap_password",
    "crm_imap_ssl",
];

pub const EMAIL_MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

fn encrypted(key: &str) -> bool {
    ENCRYPTED_EMAIL_KEYS.contains(&key) || granary_services::email::is_mail_password_key(key)
}

/// R&D 26 Oct: the email keys: the fixed ones and each extra CRM inbox's (crm_sales_smtp_host...).
async fn all_email_keys(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    let mut keys: Vec<String> = ALL_EMAIL_KEYS.iter().map(|k| k.to_string()).collect();
    for m in granary_services::crm_mailboxes::all(db).await? {
        if m.prefix != granary_services::crm_mailboxes::DEFAULT_PREFIX {
            keys.extend(granary_services::email::mailbox_keys(&m.prefix));
        }
    }
    Ok(keys)
}

/// `_mask(key, value)`.
fn mask(key: &str, value: Option<&str>) -> String {
    let v = value.unwrap_or("");
    if encrypted(key) {
        return if v.is_empty() { "" } else { EMAIL_MASK }.to_string();
    }
    v.to_string()
}

/// `_stored_email_value(db, key)`: the saved value, decrypted for a password key; "" when unset or
/// undecryptable.
async fn stored_email_value(db: &mut PgConnection, key: &str) -> ApiResult<String> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM system_settings WHERE system_settings.key = $1")
            .bind(key)
            .fetch_optional(&mut *db)
            .await?;
    let Some(v) = row.and_then(|r| r.0).filter(|v| !v.is_empty()) else {
        return Ok(String::new());
    };
    if encrypted(key) {
        return Ok(decrypt(&v).unwrap_or_default());
    }
    Ok(v)
}

/// `GET "/email-config"`: password fields are masked.
pub async fn get_email_config(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    let keys = all_email_keys(&mut conn).await?;
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT system_settings.key, system_settings.value FROM system_settings \
         WHERE system_settings.key = ANY($1)",
    )
    .bind(&keys)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Map::new();
    for k in &keys {
        let v = rows
            .iter()
            .rev()
            .find(|(key, _)| key == k)
            .and_then(|(_, v)| v.as_deref());
        out.insert(k.to_string(), Value::String(mask(k, v)));
    }
    Ok(resp::json(Value::Object(out)))
}

granary_core::py_model! {
    /// `class EmailConfigPatch(BaseModel)`.
    pub struct EmailConfigPatch {
        pub key: String,
        pub value: String,
    }
}

/// `PATCH "/email-config"`: one key; password fields are encrypted, a blank or masked password is kept.
pub async fn update_email_config(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<EmailConfigPatch>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let known = {
        let mut conn = db.conn().await?;
        all_email_keys(&mut conn).await?
    };
    if !known.contains(&body.key) {
        return Err(ApiError::http(
            422,
            format!("Unknown email config key: {}", body.key),
        ));
    }
    let enc = encrypted(&body.key);
    if enc && (body.value.is_empty() || body.value == EMAIL_MASK) {
        return Ok(resp::json(
            json!({"key": body.key, "saved": true, "unchanged": true}),
        ));
    }
    let stored = if enc && !body.value.is_empty() {
        encrypt(&body.value)
    } else {
        body.value.clone()
    };
    let mut conn = db.conn().await?;
    let n = sqlx::query(
        "UPDATE system_settings SET value = $1, updated_by_id = $2, updated_at = now() \
         WHERE system_settings.key = $3",
    )
    .bind(&stored)
    .bind(user.id)
    .bind(&body.key)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if n == 0 {
        let category = if body.key.starts_with("crm_") {
            "crm_email"
        } else {
            "email"
        };
        sqlx::query(
            "INSERT INTO system_settings (key, value, value_type, category, label, is_public, is_encrypted, \
             updated_by_id, id) VALUES ($1, $2, 'string', $3, $4, false, $5, $6, $7)",
        )
        .bind(&body.key)
        .bind(&stored)
        .bind(category)
        .bind(pystr::title(&body.key.replace('_', " ")))
        .bind(enc)
        .bind(user.id)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"key": body.key, "saved": true})))
}

granary_core::py_model! {
    /// `class TestEmailBody(BaseModel)`.
    pub struct TestEmailBody {
        pub to: String,
        pub smtp_host: String,
        pub smtp_port: i64 = 587,
        pub smtp_user: String = String::new(),
        pub smtp_password: String = String::new(),
        pub smtp_from_name: String = "Granary".to_string(),
        pub smtp_from_email: String = String::new(),
        pub smtp_tls: bool = true,
        /// "system" | "crm": which saved password to fall back to
        pub scope: String = "system".to_string(),
    }
}

/// Security 26 Oct: a test that reuses the saved password must target the saved server, port, user and
/// encryption; otherwise the password has to be typed again.
async fn same_server(
    conn: &mut PgConnection,
    prefix: &str,
    host: &str,
    port: i64,
    user: &str,
    secure: bool,
    secure_key: &str,
) -> ApiResult<()> {
    let saved_host = stored_email_value(conn, &format!("{prefix}_host")).await?;
    let mut saved_port = stored_email_value(conn, &format!("{prefix}_port")).await?;
    if saved_port.trim().is_empty() {
        // the loaders' defaults
        saved_port = if prefix.ends_with("_imap") {
            "993"
        } else {
            "587"
        }
        .to_string();
    }
    let saved_user = stored_email_value(conn, &format!("{prefix}_user")).await?;
    let saved_secure = stored_email_value(conn, &format!("{prefix}_{secure_key}")).await?;
    let saved_secure =
        saved_secure.trim().is_empty() || saved_secure.trim().eq_ignore_ascii_case("true");
    let same = saved_host.trim().eq_ignore_ascii_case(host.trim())
        && saved_port.trim() == port.to_string()
        && saved_user.trim().eq_ignore_ascii_case(user.trim())
        && saved_secure == secure;
    if same {
        Ok(())
    } else {
        Err(ApiError::http(
            400,
            "The server details differ from the saved ones: type the password to test them (the saved password \
             is only used with the saved server).",
        ))
    }
}

/// `POST "/email-config/test"`: send a test email with the given SMTP parameters (not necessarily saved).
pub async fn test_email_config(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<TestEmailBody>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    if body.smtp_host.is_empty() || body.smtp_from_email.is_empty() {
        return Err(ApiError::http(
            422,
            "smtp_host and smtp_from_email are required",
        ));
    }
    let mut password = body.smtp_password.clone();
    if password.is_empty() || password == EMAIL_MASK {
        // "crm" is hello@; another CRM inbox is its prefix (crm_sales)
        let prefix = if body.scope.starts_with("crm") {
            format!("{}_smtp", body.scope)
        } else {
            "email_smtp".to_string()
        };
        let mut conn = db.conn().await?;
        // Security 26 Oct: the saved password only goes to the saved server (else a "test" to any host could
        // capture it)
        same_server(
            &mut conn,
            &prefix,
            &body.smtp_host,
            body.smtp_port,
            &body.smtp_user,
            body.smtp_tls,
            "tls",
        )
        .await?;
        password = stored_email_value(&mut conn, &format!("{prefix}_password")).await?;
    }
    let cfg = SmtpConfig {
        host: body.smtp_host.clone(),
        port: body.smtp_port,
        user: body.smtp_user.clone(),
        password,
        from_name: body.smtp_from_name.clone(),
        from_email: body.smtp_from_email.clone(),
        tls: body.smtp_tls,
    };
    match tokio::time::timeout(Duration::from_secs(20), send_test_email(&cfg, &body.to)).await {
        Ok(Ok(())) => Ok(resp::json(json!({"status": "sent", "to": body.to}))),
        Err(_) => Err(ApiError::http(
            400,
            "SMTP connection timed out: check the host, port, and that the server is reachable.",
        )),
        Ok(Err(e)) => Err(ApiError::http(400, format!("SMTP error: {e}"))),
    }
}

granary_core::py_model! {
    /// `class TestCrmImapBody(BaseModel)`.
    pub struct TestCrmImapBody {
        pub imap_host: String,
        pub imap_port: i64 = 993,
        pub imap_user: String,
        pub imap_password: String = String::new(),
        pub imap_ssl: bool = true,
        /// R&D 26 Oct: which CRM inbox's saved password to fall back to ("crm" = hello@, else its prefix)
        pub scope: String = "crm".to_string(),
    }
}

/// `POST "/email-config/test-imap"`: LOGIN then LOGOUT, bounded by a timeout.
pub async fn test_crm_imap(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<TestCrmImapBody>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut password = body.imap_password.clone();
    if password.is_empty() || password == EMAIL_MASK {
        let mut conn = db.conn().await?;
        let scope = if body.scope.starts_with("crm") {
            body.scope.clone()
        } else {
            "crm".to_string()
        };
        same_server(
            &mut conn,
            &format!("{scope}_imap"),
            &body.imap_host,
            body.imap_port,
            &body.imap_user,
            body.imap_ssl,
            "ssl",
        )
        .await?;
        password = stored_email_value(&mut conn, &format!("{scope}_imap_password")).await?;
    }
    let attempt = async {
        let port = u16::try_from(body.imap_port)
            .map_err(|_| ImapError::Error("port must be 0-65535.".into()))?;
        let mut imap = Imap4::open(&body.imap_host, port, body.imap_ssl).await?;
        let resp = imap.login(&body.imap_user, &password).await?;
        let _ = imap.logout().await;
        Ok::<_, ImapError>(resp)
    };
    let timed_out = || {
        ApiError::http(
            400,
            "IMAP connection timed out: check the host, port, and that the server is reachable.",
        )
    };
    let resp = match tokio::time::timeout(Duration::from_secs(15), attempt).await {
        Err(_) | Ok(Err(ImapError::Timeout)) => return Err(timed_out()),
        Ok(Err(e)) => return Err(ApiError::http(400, format!("Could not connect: {e}"))),
        Ok(Ok(r)) => r,
    };
    if resp.result != "OK" {
        return Err(ApiError::http(
            400,
            "IMAP login failed: check the username and password.",
        ));
    }
    Ok(resp::json(
        json!({"status": "connected", "host": body.imap_host}),
    ))
}

// ─── R&D 26 Oct: the CRM inboxes ───────────────────────────────────────────────

granary_core::py_model! {
    /// A new CRM inbox.
    pub struct NewMailbox {
        pub address: String,
        pub display_name: String = String::new(),
        /// general | sales
        pub purpose: String = "general".to_string(),
        pub backfill_days: i64 = 90,
    }
}

granary_core::py_model! {
    /// A change to a CRM inbox (only what is given).
    pub struct MailboxPatch {
        pub display_name: Option<String> = None,
        pub purpose: Option<String> = None,
        pub active: Option<bool> = None,
        pub is_default: Option<bool> = None,
        pub backfill_days: Option<i64> = None,
        pub signature_html: Option<String> = None,
        pub sort: Option<i64> = None,
    }
}

/// `GET "/crm-mailboxes"`: every CRM inbox with the prefix of its server settings.
pub async fn list_crm_mailboxes(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    let boxes = granary_services::crm_mailboxes::all(&mut conn).await?;
    Ok(resp::json(json!({"mailboxes": boxes})))
}

/// The settings prefix of a new inbox: crm_ + its address's local part (sales@ -> crm_sales).
fn prefix_for(address: &str) -> String {
    let local: String = address
        .split('@')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("crm_{}", local.trim_matches('_'))
}

/// `POST "/crm-mailboxes"`: a new CRM inbox, switched off until its servers are set, tested and it is turned
/// on (then its history is imported quietly).
pub async fn create_crm_mailbox(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<NewMailbox>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let address = body.address.trim().to_lowercase();
    if !address.contains('@') || address.len() > 255 {
        return Err(ApiError::http(422, "address must be an email address"));
    }
    if !matches!(body.purpose.as_str(), "general" | "sales") {
        return Err(ApiError::http(422, "purpose must be general or sales"));
    }
    let prefix = prefix_for(&address);
    if prefix == "crm_" || prefix.len() > 40 {
        return Err(ApiError::http(422, "That address can't name an inbox."));
    }
    let mut conn = db.conn().await?;
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM crm_mailboxes WHERE address = $1 OR prefix = $2)",
    )
    .bind(&address)
    .bind(&prefix)
    .fetch_one(&mut *conn)
    .await?;
    if taken {
        return Err(ApiError::http(
            409,
            format!("{address} is already a CRM inbox."),
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO crm_mailboxes (id, prefix, address, display_name, purpose, active, is_default, sort, \
         backfill_days) VALUES ($1, $2, $3, $4, $5, false, false, \
         (SELECT coalesce(max(sort), 0) + 1 FROM crm_mailboxes), $6)",
    )
    .bind(id)
    .bind(&prefix)
    .bind(&address)
    .bind(body.display_name.trim())
    .bind(&body.purpose)
    .bind(body.backfill_days.clamp(0, 3650) as i32)
    .execute(&mut *conn)
    .await?;
    // its server settings, empty, with the inbox's address as the From
    for key in granary_services::email::mailbox_keys(&prefix) {
        let value = if key.ends_with("_smtp_from_email") {
            address.clone()
        } else if key.ends_with("_smtp_from_name") {
            body.display_name.trim().to_string()
        } else if key.ends_with("_imap_ssl") || key.ends_with("_smtp_tls") {
            // on, as the Settings switches show them
            "true".to_string()
        } else {
            String::new()
        };
        sqlx::query(
            "INSERT INTO system_settings (id, key, value, value_type, category, label, is_public, is_encrypted, \
             updated_by_id) VALUES ($1, $2, $3, 'string', 'crm_email', $4, false, $5, $6) \
             ON CONFLICT (key) DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(&key)
        .bind(&value)
        .bind(pystr::title(&key.replace('_', " ")))
        .bind(encrypted(&key))
        .bind(user.id)
        .execute(&mut *conn)
        .await?;
    }
    let made = granary_services::crm_mailboxes::get(&mut conn, id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({"mailbox": made})))
}

/// `PATCH "/crm-mailboxes/{id}"`: name, purpose, on/off, the default, history days, signature. Turning an
/// inbox on the first time starts its quiet history import.
pub async fn update_crm_mailbox(
    CurrentUser(user): CurrentUser,
    db: Db,
    axum::extract::Path(id): axum::extract::Path<String>,
    PyJson(body): PyJson<MailboxPatch>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let id = Uuid::parse_str(id.trim()).map_err(|_| ApiError::http(404, "Inbox not found"))?;
    let mut conn = db.conn().await?;
    let Some(current) = granary_services::crm_mailboxes::get(&mut conn, id).await? else {
        return Err(ApiError::http(404, "Inbox not found"));
    };
    if let Some(p) = body.purpose.as_deref() {
        if !matches!(p, "general" | "sales") {
            return Err(ApiError::http(422, "purpose must be general or sales"));
        }
    }
    if body.active == Some(false) && current.is_default {
        return Err(ApiError::http(
            409,
            "The default inbox can't be switched off; make another the default first.",
        ));
    }
    if body.active == Some(true) && !current.active {
        // its servers must be set (the Test buttons check they work)
        let cfg = granary_services::crm_mailboxes::imap(&mut conn, &current)
            .await
            .map_err(|e| ApiError::http(400, e.to_string()))?;
        if cfg.host.is_empty() || cfg.user.is_empty() || cfg.password.is_empty() {
            return Err(ApiError::http(
                400,
                "Set the inbox's IMAP host, username and password first.",
            ));
        }
    }
    if body.is_default == Some(true) {
        sqlx::query("UPDATE crm_mailboxes SET is_default = false, updated_at = now() WHERE is_default AND id <> $1")
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query(
        "UPDATE crm_mailboxes SET display_name = coalesce($2, display_name), purpose = coalesce($3, purpose), \
         active = coalesce($4, active), is_default = coalesce($5, is_default), \
         backfill_days = coalesce($6, backfill_days), signature_html = coalesce($7, signature_html), \
         sort = coalesce($8, sort), \
         connected_at = CASE WHEN coalesce($4, active) AND connected_at IS NULL THEN now() ELSE connected_at END, \
         updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(body.display_name.as_deref().map(str::trim))
    .bind(body.purpose.as_deref())
    .bind(body.active)
    .bind(body.is_default)
    .bind(body.backfill_days.map(|d| d.clamp(0, 3650) as i32))
    .bind(body.signature_html.as_deref())
    .bind(body.sort.map(|s| s as i32))
    .execute(&mut *conn)
    .await?;
    let after = granary_services::crm_mailboxes::get(&mut conn, id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"mailbox": after})))
}
