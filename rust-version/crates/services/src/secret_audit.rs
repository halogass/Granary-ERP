//! Placeholder secrets at startup (security triage #23).
//!
//! When the API or the worker starts, the secrets it was given (environment) and the secrets stored in
//! Settings (encrypted system settings) are checked for values left from `.env.example` or a test setup:
//! a value starting with "change-this", "change_me", "changeme", "test", "secret"... and, for the keys this
//! deployment generates itself (signing keys, service tokens), one shorter than 32 characters.
//!
//! Stage 8: one exception, [`startup_refusal`]: JWT_SECRET_KEY, APP_SECRET_KEY or NEXTAUTH_SECRET still holding
//! the `.env.example` placeholder ("change-this...") stops the API and the worker before they serve anything.
//!
//! Owner decision 26 Oct 2026: for everything else the process does NOT refuse to start. Each finding is logged at ERROR, naming
//! the variable or setting and never its value, and the Owner is told once per boot (the Owner alert path,
//! `nary::owner_channel::tell`: a bell notification and email, plus the Nary Owner chat when it is on). The API
//! and the worker boot together on a deploy: a short Redis marker keeps that to one alert.

use std::time::Duration;

use sqlx::PgPool;

/// The shortest acceptable length of a generated key or token.
pub const MIN_KEY_CHARS: usize = 32;

/// Value prefixes that mark a placeholder (compared lower-case, after trimming).
pub const PLACEHOLDER_PREFIXES: &[&str] = &[
    "change-this",
    "change_this",
    "changethis",
    "change-me",
    "change_me",
    "changeme",
    "test",
    "secret",
    "password",
    "placeholder",
    "example",
    "dummy",
];

/// How a secret is judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretKind {
    /// Generated here (signing key, service token): placeholder prefixes and the minimum length.
    Key,
    /// A password or a key issued by someone else (its length is theirs): placeholder prefixes only.
    Password,
}

/// The environment secrets the API and the worker see, with how each is judged. Unset or empty ones are
/// skipped (an optional service left off; required ones are enforced when the settings load).
pub const ENV_SECRETS: &[(&str, SecretKind)] = &[
    ("APP_SECRET_KEY", SecretKind::Key),
    ("JWT_SECRET_KEY", SecretKind::Key),
    ("SIGNAL_TICKET_KEY", SecretKind::Key),
    ("INTERNAL_AUTH_TOKEN", SecretKind::Key),
    ("NEXTAUTH_SECRET", SecretKind::Key),
    ("WHATSAPP_SERVICE_TOKEN", SecretKind::Key),
    ("PDF_SERVICE_TOKEN", SecretKind::Key),
    ("FACE_SERVICE_TOKEN", SecretKind::Key),
    ("RESEARCH_SERVICE_TOKEN", SecretKind::Key),
    ("STT_SERVICE_TOKEN", SecretKind::Key),
    ("SEARXNG_SECRET", SecretKind::Key),
    ("TURN_SECRET", SecretKind::Key),
    ("POSTGRES_PASSWORD", SecretKind::Password),
    ("REDIS_PASSWORD", SecretKind::Password),
    ("SUPERADMIN_PASSWORD", SecretKind::Password),
    ("MINIO_SECRET_KEY", SecretKind::Password),
    ("SIGN_CERT_P12_PASSWORD", SecretKind::Password),
    ("SIGN_QTSP_API_KEY", SecretKind::Password),
    ("EXCHANGERATE_API_KEY", SecretKind::Password),
];

/// One weak secret: where it is (an environment variable or a setting key) and why, never the value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub name: String,
    pub source: &'static str,
    pub reason: String,
}

/// Why a value is a placeholder, or None when it looks real. Empty values are not judged.
pub fn weakness(value: &str, kind: SecretKind) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    let low = v.to_lowercase();
    if let Some(p) = PLACEHOLDER_PREFIXES.iter().find(|p| low.starts_with(**p)) {
        return Some(format!("starts with the placeholder \"{p}\""));
    }
    if kind == SecretKind::Key && v.chars().count() < MIN_KEY_CHARS {
        return Some(format!("shorter than {MIN_KEY_CHARS} characters"));
    }
    None
}

/// The environment findings, reading each variable through `get` (the process environment in production).
pub fn env_findings(get: impl Fn(&str) -> Option<String>) -> Vec<Finding> {
    ENV_SECRETS
        .iter()
        .filter_map(|(name, kind)| {
            let value = get(name)?;
            weakness(&value, *kind).map(|reason| Finding {
                name: (*name).to_string(),
                source: "environment variable",
                reason,
            })
        })
        .collect()
}

/// A variable as the API reads it: the process environment, then the `.env` file (the settings loader's
/// order), so a secret only in `.env` is judged too.
fn env_value(name: &str) -> Option<String> {
    if let Ok(v) = std::env::var(name) {
        return Some(v);
    }
    let s = granary_core::config::settings();
    let v = match name {
        "APP_SECRET_KEY" => &s.APP_SECRET_KEY,
        "JWT_SECRET_KEY" => &s.JWT_SECRET_KEY,
        "WHATSAPP_SERVICE_TOKEN" => &s.WHATSAPP_SERVICE_TOKEN,
        "PDF_SERVICE_TOKEN" => &s.PDF_SERVICE_TOKEN,
        "FACE_SERVICE_TOKEN" => &s.FACE_SERVICE_TOKEN,
        "RESEARCH_SERVICE_TOKEN" => &s.RESEARCH_SERVICE_TOKEN,
        "STT_SERVICE_TOKEN" => &s.STT_SERVICE_TOKEN,
        "TURN_SECRET" => &s.TURN_SECRET,
        "POSTGRES_PASSWORD" => &s.POSTGRES_PASSWORD,
        "SUPERADMIN_PASSWORD" => &s.SUPERADMIN_PASSWORD,
        "MINIO_SECRET_KEY" => &s.MINIO_SECRET_KEY,
        "SIGN_CERT_P12_PASSWORD" => &s.SIGN_CERT_P12_PASSWORD,
        "SIGN_QTSP_API_KEY" => &s.SIGN_QTSP_API_KEY,
        "EXCHANGERATE_API_KEY" => &s.EXCHANGERATE_API_KEY,
        _ => return None,
    };
    Some(v.clone())
}

/// The encrypted settings (API keys kept in Settings) whose decrypted value is a placeholder. A value that
/// does not decrypt is logged and skipped: it is not a placeholder question.
pub async fn settings_findings(pool: &PgPool) -> Result<Vec<Finding>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM system_settings WHERE is_encrypted = true \
         AND value IS NOT NULL AND value <> '' ORDER BY key",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for (key, cipher) in rows {
        let plain = match crate::email_crypto::decrypt(&cipher) {
            Ok(p) => p,
            Err(_) => {
                tracing::error!(setting = %key, "secret audit: the encrypted setting does not decrypt with APP_SECRET_KEY");
                continue;
            }
        };
        if let Some(reason) = weakness(&plain, SecretKind::Password) {
            out.push(Finding {
                name: key,
                source: "setting",
                reason,
            });
        }
    }
    Ok(out)
}

/// Stage 8: the signing secrets the process refuses to start with while they hold the `.env.example`
/// placeholder (anyone who has read that file could sign sessions, tokens or encrypted settings with them).
pub const FATAL_SECRETS: &[&str] = &["JWT_SECRET_KEY", "APP_SECRET_KEY", "NEXTAUTH_SECRET"];

/// The `.env.example` placeholder, in the spellings people type (compared lower-case, after trimming).
pub const FATAL_PREFIXES: &[&str] = &["change-this", "change_this", "changethis"];

/// Stage 8: why the process must not start, or None. Reads each of [`FATAL_SECRETS`] through `get`; a
/// value that starts with a [`FATAL_PREFIXES`] placeholder is refused with a message that says how to make a
/// real one. Other weak secrets keep the startup alert ([`run_at_startup`]).
pub fn startup_refusal(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let bad: Vec<&str> = FATAL_SECRETS
        .iter()
        .copied()
        .filter(|name| {
            get(name).is_some_and(|v| {
                let low = v.trim().to_lowercase();
                FATAL_PREFIXES.iter().any(|p| low.starts_with(p))
            })
        })
        .collect();
    if bad.is_empty() {
        return None;
    }
    Some(format!(
        "Refusing to start: {} still {} the placeholder from .env.example. Generate a value for each with \
         `openssl rand -hex 64` (a different one per variable), put it in .env and start again. Changing \
         JWT_SECRET_KEY signs everyone out; changing APP_SECRET_KEY on a server with data makes encrypted \
         settings unreadable, so set it once, before first use.",
        bad.join(", "),
        if bad.len() == 1 { "holds" } else { "hold" }
    ))
}

/// [`startup_refusal`] on this process's environment (then `.env`, as the settings load it).
pub fn startup_refusal_from_env() -> Option<String> {
    startup_refusal(env_value)
}

/// The alert text: names and reasons, never values.
pub fn alert_text(findings: &[Finding]) -> (String, String) {
    let title = "Placeholder secrets on the server".to_string();
    let mut lines: Vec<String> = findings
        .iter()
        .map(|f| format!("- {} ({}): {}", f.name, f.source, f.reason))
        .collect();
    lines.insert(
        0,
        "These secrets still look like placeholders. Replace each with a long random value \
         (openssl rand -base64 48) and restart the services:"
            .to_string(),
    );
    (title, lines.join("\n"))
}

/// The Redis marker that keeps one alert per deploy (the API and the worker start together).
const ALERT_MARKER: &str = "security:secret_audit:alerted";
const ALERT_MARKER_SECONDS: u64 = 600;

/// Runs the check once at startup: logs every finding at ERROR and alerts the Owner (once per boot). Never
/// fails the process; errors are logged. `process` names the caller in the log ("api", "worker").
pub async fn run_at_startup(pool: PgPool, process: &'static str) {
    // let the pool and Redis come up with the rest of the process
    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut findings = env_findings(env_value);
    match settings_findings(&pool).await {
        Ok(f) => findings.extend(f),
        Err(e) => tracing::error!(error = %e, process, "secret audit: could not read the settings"),
    }
    for f in &findings {
        tracing::error!(
            process,
            name = %f.name,
            source = f.source,
            reason = %f.reason,
            "placeholder secret: {} {} {}", f.source, f.name, f.reason
        );
    }
    if findings.is_empty() {
        return;
    }
    if let Err(e) = alert_once(&pool, &findings).await {
        tracing::error!(error = %e, process, "secret audit: the Owner alert failed");
    }
}

async fn alert_once(pool: &PgPool, findings: &[Finding]) -> anyhow::Result<()> {
    // SET NX: the first process of this boot alerts; Redis down means each process alerts (better twice
    // than never).
    let first = async {
        let mut r = crate::redis_lease::client().await?;
        crate::redis_lease::acquire(
            &mut r,
            ALERT_MARKER,
            &crate::redis_lease::new_token(),
            ALERT_MARKER_SECONDS as i64,
        )
        .await
    }
    .await
    .unwrap_or(true);
    if !first {
        return Ok(());
    }
    let (title, body) = alert_text(findings);
    let mut s = pool.begin().await?;
    let mut tell = crate::nary::owner_channel::Tell::new(
        &title,
        &body,
        "/settings",
        "system",
        "secrets",
        "security",
    );
    tell.priority = "high";
    tell.email = true;
    tell.anytime = true;
    crate::nary::owner_channel::tell(&mut s, tell)
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    s.commit().await?;
    crate::approvals::notify::deliver_outbox_now().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stage 8: the signing secrets left at the .env.example placeholder stop the start; others don't.
    #[test]
    fn placeholder_signing_secrets_refuse_to_start() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let msg = startup_refusal(env(&[
            (
                "JWT_SECRET_KEY",
                "change-this-to-another-long-random-secret-key",
            ),
            (
                "APP_SECRET_KEY",
                "a3f9c0d1e2b4a5f6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2",
            ),
        ]))
        .expect("refused");
        assert!(msg.contains("JWT_SECRET_KEY") && !msg.contains("APP_SECRET_KEY still"));
        assert!(msg.contains("openssl rand"));
        assert!(!msg.contains("change-this-to-another"), "never the value");
        let msg = startup_refusal(env(&[
            (
                "APP_SECRET_KEY",
                "  Change-This-to-a-long-random-secret-min-64-chars",
            ),
            ("NEXTAUTH_SECRET", "change_this"),
        ]))
        .expect("refused");
        assert!(msg.contains("APP_SECRET_KEY, NEXTAUTH_SECRET still hold"));
        // real values, unset ones and other placeholders (alerted, not refused) start
        assert_eq!(
            startup_refusal(env(&[
                (
                    "JWT_SECRET_KEY",
                    "test-jwt-secret-key-at-least-32-characters-long"
                ),
                (
                    "APP_SECRET_KEY",
                    "a3f9c0d1e2b4a5f6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2"
                ),
                ("POSTGRES_PASSWORD", "change-this-db-password"),
            ])),
            None
        );
    }

    #[test]
    fn placeholders_and_short_keys_are_found() {
        assert!(weakness(
            "change-this-to-a-long-random-secret-min-64-chars",
            SecretKind::Key
        )
        .is_some());
        assert!(weakness("change_me_turn_secret", SecretKind::Key).is_some());
        assert!(weakness("CHANGEME", SecretKind::Password).is_some());
        assert!(weakness("  test-key-123  ", SecretKind::Password).is_some());
        assert!(weakness("secret", SecretKind::Password).is_some());
        assert_eq!(
            weakness("abc123", SecretKind::Key).as_deref(),
            Some("shorter than 32 characters")
        );
        // a real-looking value passes; a short third-party key is not judged by length
        assert!(weakness("q8Zr2vN0xL4pT7mK1sYc9bWd3fGh6jE5aR0uI2oP", SecretKind::Key).is_none());
        assert!(weakness("a1b2c3d4e5f6a7b8c9d0e1f2", SecretKind::Password).is_none());
        assert!(weakness("", SecretKind::Key).is_none());
    }

    #[test]
    fn env_findings_name_the_variable_never_the_value() {
        let value = "change-this-db-password";
        let found = env_findings(|n| match n {
            "POSTGRES_PASSWORD" => Some(value.to_string()),
            "JWT_SECRET_KEY" => Some("q1w2e3".to_string()),
            "APP_SECRET_KEY" => Some("Zx9".repeat(20)),
            "PDF_SERVICE_TOKEN" => Some(String::new()),
            _ => None,
        });
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["JWT_SECRET_KEY", "POSTGRES_PASSWORD"]);
        let (title, body) = alert_text(&found);
        assert!(!title.is_empty());
        assert!(body.contains("POSTGRES_PASSWORD") && body.contains("JWT_SECRET_KEY"));
        assert!(!body.contains(value) && !body.contains("q1w2e3"));
        assert!(!body.contains('\u{2014}'));
    }
}
