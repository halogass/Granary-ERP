//! TOTP two-factor sign-in (2 Nov 2026, Owner decisions; migration 0148).
//!
//! Any user can turn it on (Profile > Security). RFC 6238: HMAC-SHA1, 6 digits, 30 s steps, a code from the
//! step before or after is accepted. The secret is kept encrypted with the ERP's Fernet key (the one used for
//! encrypted settings, [`crate::email_crypto`]); a time step is accepted once per user (`last_used_step`), so a
//! code can't be replayed. Ten one-time recovery codes are made at setup, shown once and stored as bcrypt
//! hashes; they work at login instead of a code, never for a step-up. Codes and secrets are never logged.
//!
//! Wrong codes are counted per user in `user_totp_failures` (written on [`crate::nary::session_pool`] in their
//! own transaction, so they count even though the caller's transaction rolls back on the error); more than
//! `auth.totp.max_failed_attempts` in `auth.totp.failed_window_seconds` gives a 429. At login the attempts also
//! count toward the login limits (auth.login_*), done by the login route.
//!
//! # Step-up API (for other features, e.g. approving a sensitive action)
//!
//! - [`is_enrolled`]`(db, user_id) -> ApiResult<bool>`: TOTP is on for the user.
//! - [`verify_for_action`]`(db, user_id, code, action) -> ApiResult<DateTime<Utc>>`: checks a CURRENT code
//!   (recovery codes are refused), single use, rate-limited; writes an audit row (`totp_step_up`, the action in
//!   `notes` and `new_values.action`) on the caller's connection and returns the verification time. Errors (the
//!   usual `{"detail", "code"}` body): 403 `totp_not_enrolled` (no TOTP: the user must set it up in Profile >
//!   Security), 403 `totp_invalid` (wrong, malformed or already used code), 429 `totp_rate_limited`, 400 naming
//!   a missing setting.
//!
//! # Test helpers (usable from any crate's tests)
//!
//! - [`enrol_for_test`]`(db, user_id) -> ApiResult<String>`: turns TOTP on for the user (no recovery codes) and
//!   returns the base32 secret.
//! - [`code_at`]`(secret_b32, t) -> String`: the code for time `t`. Each step is accepted once: a second
//!   step-up in the same test needs another step, e.g. `code_at(&s, Utc::now() + Duration::seconds(30))` (the
//!   next step is accepted too).
//! - Failures are written through `granary_services::nary::session_pool()`: tests on a fresh database call
//!   `granary_services::nary::set_session_pool(pool)` first (a failed write there is logged and ignored).

use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rand::{Rng, RngCore};
use serde_json::{json, Map, Value};
use sha1::Sha1;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::settings_store::{self, SettingError};
use granary_core::{ApiError, ApiResult};

/// Where the settings live in the Settings UI.
pub const WHERE: &str = "System & security \u{2192} Login security";

pub const ISSUER_KEY: &str = "auth.totp.issuer";
pub const CHALLENGE_SECONDS_KEY: &str = "auth.totp.challenge_seconds";
pub const MAX_FAILED_KEY: &str = "auth.totp.max_failed_attempts";
pub const FAILED_WINDOW_KEY: &str = "auth.totp.failed_window_seconds";
pub const RECOVERY_CODES_KEY: &str = "auth.totp.recovery_codes";

/// RFC 6238 parameters (fixed by the Owner's decision, and by what authenticator apps support).
pub const DIGITS: u32 = 6;
pub const STEP_SECONDS: i64 = 30;
/// Steps accepted either side of now.
pub const SKEW: i64 = 1;

/// The JWT `type` of a login challenge (never an access token: the auth extractor wants "access").
pub const CHALLENGE_TYPE: &str = "totp_challenge";

/// The permission whose holders must have TOTP (shown as "required" on the profile): they approve production.
pub const REQUIRED_PERMISSION: &str = "manufacturing:manage_production";

const RECOVERY_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const RECOVERY_LEN: usize = 10;
/// Recovery codes carry ~50 random bits; cost 10 keeps checking ten of them quick.
const RECOVERY_BCRYPT_COST: u32 = 10;

// ─── RFC 4648 base32 ─────────────────────────────────────────────────────────

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Base32 without padding (what otpauth URIs carry).
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// Base32 decode; spaces, dashes, padding and lower case are accepted. None for any other character.
pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in text.chars() {
        if matches!(c, ' ' | '-' | '=') {
            continue;
        }
        let up = c.to_ascii_uppercase() as u8;
        let v = B32.iter().position(|&x| x == up)? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

// ─── RFC 4226 / 6238 ─────────────────────────────────────────────────────────

fn hotp(key: &[u8], counter: u64) -> u32 {
    let mut mac = <Hmac<Sha1>>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = (h[h.len() - 1] & 0x0f) as usize;
    let bin = ((h[off] as u32 & 0x7f) << 24)
        | ((h[off + 1] as u32) << 16)
        | ((h[off + 2] as u32) << 8)
        | (h[off + 3] as u32);
    bin % 10u32.pow(DIGITS)
}

/// The time step of `t`.
pub fn step_of(t: DateTime<Utc>) -> i64 {
    t.timestamp().div_euclid(STEP_SECONDS)
}

fn code_for_step(key: &[u8], step: i64) -> String {
    format!(
        "{:0width$}",
        hotp(key, step as u64),
        width = DIGITS as usize
    )
}

/// The code for time `t` (test helper; also what an authenticator app shows). Panics on a secret that isn't
/// base32.
pub fn code_at(secret_b32: &str, t: DateTime<Utc>) -> String {
    let key = base32_decode(secret_b32).expect("a base32 TOTP secret");
    code_for_step(&key, step_of(t))
}

/// The digits of a typed code (spaces dropped), or None when it isn't exactly six digits.
fn clean_code(code: &str) -> Option<String> {
    let c: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    (c.len() == DIGITS as usize && c.bytes().all(|b| b.is_ascii_digit())).then_some(c)
}

/// The step (now, or one either side) whose code is `code`. The comparison is constant-time.
fn matching_step(key: &[u8], code: &str, now: DateTime<Utc>) -> Option<i64> {
    let code = clean_code(code)?;
    let now_step = step_of(now);
    let mut found = None;
    for d in -SKEW..=SKEW {
        let s = now_step + d;
        if granary_compat::crypto::compare_digest_bytes(
            code_for_step(key, s).as_bytes(),
            code.as_bytes(),
        ) && found.is_none()
        {
            found = Some(s);
        }
    }
    found
}

/// `otpauth://totp/{issuer}:{account}?secret=...&issuer=...&algorithm=SHA1&digits=6&period=30`.
pub fn otpauth_uri(issuer: &str, account: &str, secret_b32: &str) -> String {
    fn enc(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'@' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
    format!(
        "otpauth://totp/{}:{}?secret={secret_b32}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        enc(issuer),
        enc(account),
        enc(issuer)
    )
}

/// A new 160-bit secret, base32.
pub fn new_secret() -> String {
    let mut raw = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    base32_encode(&raw)
}

// ─── Errors ──────────────────────────────────────────────────────────────────

fn not_enrolled() -> ApiError {
    ApiError::app(
        403,
        "Two-factor sign-in (TOTP) is not set up for this account. Set it up in Profile > Security first.",
        Some("totp_not_enrolled"),
    )
}

fn invalid(status: u16) -> ApiError {
    ApiError::app(
        status,
        "That code is not valid or was already used. Wait for the next code and try again.",
        Some("totp_invalid"),
    )
}

fn invalid_recovery() -> ApiError {
    ApiError::app(
        401,
        "That recovery code is not valid or was already used.",
        Some("totp_invalid"),
    )
}

fn rate_limited(window: i64) -> ApiError {
    ApiError::app(
        429,
        format!(
            "Too many wrong codes. Try again in {} minutes.",
            (window + 59) / 60
        ),
        Some("totp_rate_limited"),
    )
}

fn decrypt_error() -> ApiError {
    ApiError::internal(anyhow::anyhow!(
        "a TOTP secret does not decrypt (APP_SECRET_KEY changed?)"
    ))
}

// ─── Rows ────────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct Row {
    secret_encrypted: String,
    enabled: bool,
    confirmed_at: Option<DateTime<Utc>>,
}

async fn row(db: &mut PgConnection, user_id: Uuid) -> ApiResult<Option<Row>> {
    Ok(sqlx::query_as::<_, Row>(
        "SELECT secret_encrypted, enabled, confirmed_at FROM user_totp WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&mut *db)
    .await?)
}

fn secret_key(r: &Row) -> ApiResult<Vec<u8>> {
    let b32 = crate::email_crypto::decrypt(&r.secret_encrypted).map_err(|_| decrypt_error())?;
    base32_decode(&b32).ok_or_else(decrypt_error)
}

/// Marks `step` used when it is later than the last one used (race-safe: one UPDATE). False: replayed.
async fn consume_step(db: &mut PgConnection, user_id: Uuid, step: i64) -> ApiResult<bool> {
    let n = sqlx::query(
        "UPDATE user_totp SET last_used_step = $2, updated_at = now() \
         WHERE user_id = $1 AND (last_used_step IS NULL OR last_used_step < $2)",
    )
    .bind(user_id)
    .bind(step)
    .execute(&mut *db)
    .await?
    .rows_affected();
    Ok(n == 1)
}

// ─── Settings and the failure limit ──────────────────────────────────────────

async fn require_int(db: &mut PgConnection, key: &str) -> ApiResult<i64> {
    Ok(settings_store::require_int(db, key, WHERE).await?)
}

async fn require_str(db: &mut PgConnection, key: &str) -> ApiResult<String> {
    settings_store::get_optional(db, key)
        .await?
        .ok_or_else(|| SettingError::new(key, "is not set", WHERE).into())
}

/// 429 when the user has too many wrong codes in the window.
async fn check_rate(db: &mut PgConnection, user_id: Uuid) -> ApiResult<()> {
    let max = require_int(db, MAX_FAILED_KEY).await?;
    let window = require_int(db, FAILED_WINDOW_KEY).await?;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_totp_failures WHERE user_id = $1 \
         AND created_at > now() - make_interval(secs => $2)",
    )
    .bind(user_id)
    .bind(window as f64)
    .fetch_one(&mut *db)
    .await?;
    if n >= max {
        return Err(rate_limited(window));
    }
    Ok(())
}

/// A wrong code, written in its own transaction (the caller's rolls back on the error), with an audit row for
/// a step-up. Best effort: a failure here is logged (never the code) and the caller still gets its error.
async fn record_failure(user_id: Uuid, kind: &str, action: Option<&str>) {
    let res: ApiResult<()> = async {
        let pool = crate::nary::session_pool()?;
        let mut conn = pool.acquire().await?;
        let mut tx = sqlx::Connection::begin(&mut *conn).await?;
        sqlx::query(
            "INSERT INTO user_totp_failures (id, user_id, kind, created_at) VALUES ($1, $2, $3, now())",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(kind)
        .execute(&mut *tx)
        .await?;
        if let Some(action) = action {
            audit(
                &mut tx,
                user_id,
                user_id,
                "totp_step_up_failed",
                json!({"action": action}),
                Some(action),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        tracing::error!(
            "TOTP: could not record a failed attempt for user {}: {:?}",
            user_id,
            e
        );
    }
}

async fn clear_failures(db: &mut PgConnection, user_id: Uuid) -> ApiResult<()> {
    sqlx::query("DELETE FROM user_totp_failures WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// One audit row (entity user_totp, the target user). `actor` is who did it.
async fn audit(
    db: &mut PgConnection,
    actor: Uuid,
    target: Uuid,
    action: &str,
    new_values: Value,
    notes: Option<&str>,
) -> ApiResult<()> {
    let names: Vec<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, name, email FROM users WHERE id = ANY($1)")
            .bind(vec![actor, target])
            .fetch_all(&mut *db)
            .await?;
    let find = |id: Uuid| names.iter().find(|n| n.0 == id);
    let actor_name = find(actor).map(|n| n.1.clone());
    let label: String = find(target)
        .map(|n| format!("{} <{}>", n.1, n.2))
        .unwrap_or_else(|| target.to_string())
        .chars()
        .take(200)
        .collect();
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
         new_values, notes, origin) VALUES ($1, $2, $3, $4, 'user_totp', $5, $6, $7::json, $8, 'ui')",
    )
    .bind(Uuid::new_v4())
    .bind(actor)
    .bind(actor_name)
    .bind(action)
    .bind(target.to_string())
    .bind(label)
    .bind(new_values.to_string())
    .bind(notes)
    .execute(&mut *db)
    .await?;
    Ok(())
}

// ─── Recovery codes ──────────────────────────────────────────────────────────

fn clean_recovery(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Makes `n` new codes for the user (the old ones are deleted) and returns them for display, "xxxxx-xxxxx".
async fn new_recovery_codes(db: &mut PgConnection, user_id: Uuid) -> ApiResult<Vec<String>> {
    let n = require_int(db, RECOVERY_CODES_KEY).await?;
    if !(1..=50).contains(&n) {
        return Err(
            SettingError::new(RECOVERY_CODES_KEY, "must be between 1 and 50", WHERE).into(),
        );
    }
    let codes: Vec<String> = (0..n)
        .map(|_| {
            let mut rng = rand::rngs::OsRng;
            (0..RECOVERY_LEN)
                .map(|_| RECOVERY_ALPHABET[rng.gen_range(0..RECOVERY_ALPHABET.len())] as char)
                .collect()
        })
        .collect();
    let to_hash = codes.clone();
    let hashes = tokio::task::spawn_blocking(move || {
        to_hash
            .iter()
            .map(|c| {
                let mut salt = [0u8; 16];
                rand::rngs::OsRng.fill_bytes(&mut salt);
                granary_compat::crypto::password::hash_password_with_salt(
                    c,
                    RECOVERY_BCRYPT_COST,
                    salt,
                )
            })
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(|e| ApiError::internal(anyhow::anyhow!("hashing recovery codes: {e:?}")))?;
    sqlx::query("DELETE FROM user_totp_recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *db)
        .await?;
    for h in hashes {
        sqlx::query(
            "INSERT INTO user_totp_recovery_codes (id, user_id, code_hash, created_at) VALUES ($1, $2, $3, now())",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(h)
        .execute(&mut *db)
        .await?;
    }
    Ok(codes
        .into_iter()
        .map(|c| format!("{}-{}", &c[..5], &c[5..]))
        .collect())
}

/// Uses one recovery code: true when it matched an unused one (now used).
async fn use_recovery_code(db: &mut PgConnection, user_id: Uuid, code: &str) -> ApiResult<bool> {
    let given = clean_recovery(code);
    if given.len() != RECOVERY_LEN {
        return Ok(false);
    }
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, code_hash FROM user_totp_recovery_codes WHERE user_id = $1 AND used_at IS NULL \
         ORDER BY created_at, id",
    )
    .bind(user_id)
    .fetch_all(&mut *db)
    .await?;
    let found = tokio::task::spawn_blocking(move || {
        rows.into_iter().find_map(|(id, h)| {
            granary_compat::crypto::password::verify_password(&given, &h)
                .unwrap_or(false)
                .then_some(id)
        })
    })
    .await
    .map_err(ApiError::internal)?;
    let Some(id) = found else { return Ok(false) };
    let n = sqlx::query(
        "UPDATE user_totp_recovery_codes SET used_at = now() WHERE id = $1 AND used_at IS NULL",
    )
    .bind(id)
    .execute(&mut *db)
    .await?
    .rows_affected();
    Ok(n == 1)
}

// ─── Status and setup ────────────────────────────────────────────────────────

/// What the profile shows.
#[derive(Debug, Clone)]
pub struct Status {
    pub enabled: bool,
    /// A setup was started and not confirmed yet.
    pub pending: bool,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub recovery_codes_left: i64,
}

impl Status {
    pub fn to_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "pending": self.pending,
            "confirmed_at": self.confirmed_at.map(|t| t.to_rfc3339()),
            "recovery_codes_left": self.recovery_codes_left,
        })
    }
}

pub async fn status(db: &mut PgConnection, user_id: Uuid) -> ApiResult<Status> {
    let r = row(db, user_id).await?;
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_totp_recovery_codes WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(&mut *db)
    .await?;
    Ok(Status {
        enabled: r.as_ref().is_some_and(|r| r.enabled),
        pending: r.as_ref().is_some_and(|r| !r.enabled),
        confirmed_at: r.as_ref().and_then(|r| r.confirmed_at),
        recovery_codes_left: if r.as_ref().is_some_and(|r| r.enabled) {
            left
        } else {
            0
        },
    })
}

/// TOTP is on for the user (step-up API).
pub async fn is_enrolled(db: &mut PgConnection, user_id: Uuid) -> ApiResult<bool> {
    let on: Option<bool> = sqlx::query_scalar("SELECT enabled FROM user_totp WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(&mut *db)
        .await?;
    Ok(on.unwrap_or(false))
}

/// A setup in progress: shown once, as a QR and as text.
#[derive(Debug, Clone)]
pub struct Setup {
    pub secret: String,
    pub otpauth_uri: String,
    pub issuer: String,
    pub account: String,
    /// `data:image/png;base64,...` of the URI.
    pub qr_data_uri: String,
}

impl Setup {
    pub fn to_json(&self) -> Value {
        json!({
            "secret": self.secret,
            "otpauth_uri": self.otpauth_uri,
            "issuer": self.issuer,
            "account": self.account,
            "qr_data_uri": self.qr_data_uri,
        })
    }
}

/// Starts (or restarts) a setup: a new secret, not active until [`confirm_setup`]. 409 when TOTP is on.
pub async fn begin_setup(db: &mut PgConnection, user_id: Uuid, account: &str) -> ApiResult<Setup> {
    if is_enrolled(db, user_id).await? {
        return Err(ApiError::conflict(
            "Two-factor sign-in is already on. Turn it off first to set it up again.",
        ));
    }
    let issuer = require_str(db, ISSUER_KEY).await?;
    let secret = new_secret();
    sqlx::query(
        "INSERT INTO user_totp (user_id, secret_encrypted, enabled, confirmed_at, last_used_step, created_at, \
         updated_at) VALUES ($1, $2, false, NULL, NULL, now(), now()) \
         ON CONFLICT (user_id) DO UPDATE SET secret_encrypted = EXCLUDED.secret_encrypted, enabled = false, \
         confirmed_at = NULL, last_used_step = NULL, updated_at = now()",
    )
    .bind(user_id)
    .bind(crate::email_crypto::encrypt(&secret))
    .execute(&mut *db)
    .await?;
    sqlx::query("DELETE FROM user_totp_recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *db)
        .await?;
    let uri = otpauth_uri(&issuer, account, &secret);
    let qr = crate::verification::service::qr_data_uri(&uri)?;
    Ok(Setup {
        secret,
        otpauth_uri: uri,
        issuer,
        account: account.to_string(),
        qr_data_uri: qr,
    })
}

/// Confirms a setup with a code from the app: TOTP is on, and the recovery codes (shown once) are returned.
pub async fn confirm_setup(
    db: &mut PgConnection,
    user_id: Uuid,
    code: &str,
) -> ApiResult<Vec<String>> {
    let Some(r) = row(db, user_id).await? else {
        return Err(ApiError::http(
            400,
            "Start the setup first (scan the QR code, then enter a code).",
        ));
    };
    if r.enabled {
        return Err(ApiError::conflict("Two-factor sign-in is already on."));
    }
    check_rate(db, user_id).await?;
    let key = secret_key(&r)?;
    let ok = match matching_step(&key, code, Utc::now()) {
        Some(s) => consume_step(db, user_id, s).await?,
        None => false,
    };
    if !ok {
        record_failure(user_id, "setup", None).await;
        return Err(invalid(403));
    }
    sqlx::query(
        "UPDATE user_totp SET enabled = true, confirmed_at = now(), updated_at = now() WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(&mut *db)
    .await?;
    clear_failures(db, user_id).await?;
    let codes = new_recovery_codes(db, user_id).await?;
    audit(db, user_id, user_id, "totp_enabled", json!({}), None).await?;
    Ok(codes)
}

/// Checks a current code for an enrolled user: Ok(()) or the error (failure recorded as `kind`).
async fn check_code(
    db: &mut PgConnection,
    user_id: Uuid,
    code: &str,
    kind: &str,
    action: Option<&str>,
    status_on_fail: u16,
) -> ApiResult<DateTime<Utc>> {
    let Some(r) = row(db, user_id).await?.filter(|r| r.enabled) else {
        return Err(not_enrolled());
    };
    check_rate(db, user_id).await?;
    let key = secret_key(&r)?;
    let now = Utc::now();
    match matching_step(&key, code, now) {
        Some(s) if consume_step(db, user_id, s).await? => {
            clear_failures(db, user_id).await?;
            Ok(now)
        }
        _ => {
            record_failure(user_id, kind, action).await;
            Err(invalid(status_on_fail))
        }
    }
}

/// Step-up for another feature: a CURRENT code (no recovery codes), single use, rate-limited, audited.
/// See the module docs for the errors.
pub async fn verify_for_action(
    db: &mut PgConnection,
    user_id: Uuid,
    code: &str,
    action: &str,
) -> ApiResult<DateTime<Utc>> {
    let at = check_code(db, user_id, code, "step_up", Some(action), 403).await?;
    audit(
        db,
        user_id,
        user_id,
        "totp_step_up",
        json!({"action": action, "verified_at": at.to_rfc3339()}),
        Some(action),
    )
    .await?;
    Ok(at)
}

/// What the person typed at the login's second step.
#[derive(Debug, Clone, Copy)]
pub enum LoginFactor<'a> {
    Code(&'a str),
    Recovery(&'a str),
}

/// The login's second step: Ok when the code (or an unused recovery code, now used) is right; 401
/// `totp_invalid` otherwise, 429 over the limit, 403 `totp_not_enrolled` when TOTP is off.
pub async fn verify_login(
    db: &mut PgConnection,
    user_id: Uuid,
    factor: LoginFactor<'_>,
) -> ApiResult<()> {
    match factor {
        LoginFactor::Code(code) => {
            check_code(db, user_id, code, "login", None, 401).await?;
            Ok(())
        }
        LoginFactor::Recovery(code) => {
            if !is_enrolled(db, user_id).await? {
                return Err(not_enrolled());
            }
            check_rate(db, user_id).await?;
            if use_recovery_code(db, user_id, code).await? {
                clear_failures(db, user_id).await?;
                audit(
                    db,
                    user_id,
                    user_id,
                    "totp_recovery_code_used",
                    json!({}),
                    None,
                )
                .await?;
                Ok(())
            } else {
                record_failure(user_id, "login", None).await;
                Err(invalid_recovery())
            }
        }
    }
}

/// Turns TOTP off: needs a current code or an unused recovery code.
pub async fn disable(db: &mut PgConnection, user_id: Uuid, code: &str) -> ApiResult<()> {
    if clean_code(code).is_some() {
        check_code(db, user_id, code, "manage", None, 403).await?;
    } else {
        if !is_enrolled(db, user_id).await? {
            return Err(not_enrolled());
        }
        check_rate(db, user_id).await?;
        if !use_recovery_code(db, user_id, code).await? {
            record_failure(user_id, "manage", None).await;
            return Err(invalid(403));
        }
    }
    remove(db, user_id).await?;
    audit(db, user_id, user_id, "totp_disabled", json!({}), None).await
}

/// New recovery codes (the old ones stop working): needs a current code.
pub async fn regenerate_recovery_codes(
    db: &mut PgConnection,
    user_id: Uuid,
    code: &str,
) -> ApiResult<Vec<String>> {
    check_code(db, user_id, code, "manage", None, 403).await?;
    let codes = new_recovery_codes(db, user_id).await?;
    audit(
        db,
        user_id,
        user_id,
        "totp_recovery_regenerated",
        json!({}),
        None,
    )
    .await?;
    Ok(codes)
}

async fn remove(db: &mut PgConnection, user_id: Uuid) -> ApiResult<bool> {
    sqlx::query("DELETE FROM user_totp_recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *db)
        .await?;
    sqlx::query("DELETE FROM user_totp_failures WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *db)
        .await?;
    let n = sqlx::query("DELETE FROM user_totp WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *db)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// An admin's reset (the caller checks who may): TOTP and recovery codes removed, audit-logged; the user
/// sets it up again. Returns whether there was anything to remove.
pub async fn admin_reset(db: &mut PgConnection, actor: Uuid, target: Uuid) -> ApiResult<bool> {
    let had = remove(db, target).await?;
    audit(
        db,
        actor,
        target,
        "totp_reset",
        json!({"had_totp": had}),
        Some("Two-factor sign-in reset by an administrator"),
    )
    .await?;
    Ok(had)
}

// ─── The login challenge ─────────────────────────────────────────────────────

/// A short tag of the password hash: a challenge stops working when the password changes.
pub fn password_tag(password_hash: &str) -> String {
    granary_compat::crypto::hashes::sha256_hex(password_hash.as_bytes())[..16].to_string()
}

/// The signed, short-lived proof that the password step passed (`auth.totp.challenge_seconds`).
pub async fn issue_challenge(
    db: &mut PgConnection,
    user_id: Uuid,
    password_hash: &str,
) -> ApiResult<(String, i64)> {
    let seconds = require_int(db, CHALLENGE_SECONDS_KEY).await?;
    let cfg = granary_core::config::settings().jwt();
    let now = Utc::now();
    let mut claims = Map::new();
    claims.insert("sub".into(), Value::String(user_id.to_string()));
    claims.insert("type".into(), Value::String(CHALLENGE_TYPE.into()));
    claims.insert("pw".into(), Value::String(password_tag(password_hash)));
    claims.insert("iat".into(), Value::from(now.timestamp()));
    claims.insert(
        "exp".into(),
        Value::from((now + Duration::seconds(seconds)).timestamp()),
    );
    claims.insert("jti".into(), Value::String(Uuid::new_v4().to_string()));
    let token = granary_compat::crypto::jwt::encode(&claims, &cfg.secret, &cfg.algorithm)
        .map_err(|e| ApiError::internal(anyhow::anyhow!("challenge: {e:?}")))?;
    Ok((token, seconds))
}

/// The user and password tag of a valid, unexpired challenge.
pub fn read_challenge(token: &str) -> Option<(Uuid, String)> {
    let cfg = granary_core::config::settings().jwt();
    let claims = granary_compat::crypto::jwt::decode_token(&cfg, token);
    if claims.get("type").and_then(Value::as_str) != Some(CHALLENGE_TYPE) {
        return None;
    }
    let uid = Uuid::parse_str(claims.get("sub")?.as_str()?).ok()?;
    let pw = claims.get("pw")?.as_str()?.to_string();
    Some((uid, pw))
}

// ─── Test helper ─────────────────────────────────────────────────────────────

/// Turns TOTP on for `user_id` directly (tests only; no recovery codes) and returns the base32 secret.
pub async fn enrol_for_test(db: &mut PgConnection, user_id: Uuid) -> ApiResult<String> {
    let secret = new_secret();
    sqlx::query(
        "INSERT INTO user_totp (user_id, secret_encrypted, enabled, confirmed_at, last_used_step, created_at, \
         updated_at) VALUES ($1, $2, true, now(), NULL, now(), now()) \
         ON CONFLICT (user_id) DO UPDATE SET secret_encrypted = EXCLUDED.secret_encrypted, enabled = true, \
         confirmed_at = now(), last_used_step = NULL, updated_at = now()",
    )
    .bind(user_id)
    .bind(crate::email_crypto::encrypt(&secret))
    .execute(&mut *db)
    .await?;
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// RFC 6238 Appendix B, SHA1 (the 8-digit vectors' last six digits).
    #[test]
    fn rfc6238_vectors() {
        let key = base32_encode(b"12345678901234567890");
        for (t, want) in [
            (59i64, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ] {
            let got = code_at(&key, Utc.timestamp_opt(t, 0).unwrap());
            assert_eq!(got, want[2..], "t={t}");
        }
    }

    #[test]
    fn base32_round_trip_and_skew() {
        for n in 0..25 {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base32_decode(&base32_encode(&data)).unwrap(), data);
        }
        assert_eq!(base32_decode("mzxw6==="), Some(b"foo".to_vec()));
        assert!(base32_decode("not base32!").is_none());
        let s = new_secret();
        assert_eq!(s.len(), 32);
        let key = base32_decode(&s).unwrap();
        let now = Utc::now();
        let c = code_at(&s, now);
        assert_eq!(matching_step(&key, &c, now), Some(step_of(now)));
        let next = code_at(&s, now + Duration::seconds(30));
        assert_eq!(matching_step(&key, &next, now), Some(step_of(now) + 1));
        let far = code_at(&s, now + Duration::seconds(90));
        if far != c && far != next && far != code_at(&s, now - Duration::seconds(30)) {
            assert_eq!(matching_step(&key, &far, now), None);
        }
        assert_eq!(matching_step(&key, "12345", now), None);
        assert_eq!(matching_step(&key, "abcdef", now), None);
        assert_eq!(
            matching_step(&key, &format!("{} {}", &c[..3], &c[3..]), now),
            Some(step_of(now))
        );
    }

    #[test]
    fn uri_shape() {
        let u = otpauth_uri("Granary", "a.b@example.com", "ABC");
        assert_eq!(
            u,
            "otpauth://totp/Granary:a.b@example.com?secret=ABC&issuer=Granary&algorithm=SHA1&digits=6&period=30"
        );
        assert_eq!(clean_recovery("ABCDE-fghjk "), "abcdefghjk");
    }
}
