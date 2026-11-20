//! GreenMail (docker greenmail/standalone:2.1.2) for the IMAP tests of imap_sync / crm_inbox, plus the
//! wave-3 replacements crm_inbox's tests install (Python's tests run the real nary code, which with
//! nary.email.intake_enabled=false does exactly this).
#![allow(dead_code)]

use std::sync::Arc;

use granary_services::crm_inbox::{self, NaryCalls};
use granary_services::imap_sync::{pyemail::Message, Imap4};
use serde_json::{json, Value};
use sqlx::PgPool;

pub const HOST: &str = "127.0.0.1";
pub const IMAP_PORT: u16 = 53143;
pub const SMTP_PORT: u16 = 53025;
const NAME: &str = "granary-s5-greenmail";

/// Starts the shared GreenMail container unless it is already listening.
pub async fn ensure() {
    if tokio::net::TcpStream::connect((HOST, IMAP_PORT))
        .await
        .is_ok()
    {
        return;
    }
    let _ = std::process::Command::new("docker")
        .args([
            "run", "-d", "--rm", "--name", NAME,
            "-p", &format!("{HOST}:{IMAP_PORT}:3143"), "-p", &format!("{HOST}:{SMTP_PORT}:3025"),
            "-e", "GREENMAIL_OPTS=-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled",
            "greenmail/standalone:2.1.2",
        ])
        .status();
    for _ in 0..60 {
        if let Ok(mut i) = Imap4::open(HOST, IMAP_PORT, false).await {
            let _ = i.logout().await;
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("GreenMail did not start");
}

/// A fresh mailbox user (GreenMail creates it on first login).
pub fn new_user(tag: &str) -> String {
    format!(
        "s5-{tag}-{}@granary.test",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

pub async fn login(user: &str) -> Imap4 {
    let mut i = Imap4::open(HOST, IMAP_PORT, false).await.expect("open");
    let r = i.login(user, "x").await.expect("login");
    assert!(r.ok(), "{r:?}");
    i
}

/// The CRM mailbox settings pointing at GreenMail.
pub async fn configure(pool: &PgPool, user: &str) {
    let password = granary_services::email_crypto::encrypt("x");
    let port = IMAP_PORT.to_string();
    for (k, v) in [
        ("crm_imap_host", HOST),
        ("crm_imap_port", port.as_str()),
        ("crm_imap_user", user),
        ("crm_imap_password", password.as_str()),
        ("crm_imap_ssl", "false"),
        ("crm_smtp_host", "smtp.test"),
        ("crm_smtp_from_email", "hello@granary.test"),
    ] {
        sqlx::query(
            "INSERT INTO system_settings (id, key, value, category) VALUES (gen_random_uuid(), $1, $2, 'crm_email') \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(k)
        .bind(v)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// services/nary/web3forms.py header_auth, as the test double.
pub fn header_auth(parsed: &Message) -> Value {
    let dkim_d = regex::Regex::new(r"(?i)(?:^|;)\s*d\s*=\s*([^;\s]+)").unwrap();
    let ar =
        fancy_regex::Regex::new(r"(?i)dkim\s*=\s*(\w+)[^;]*?header\.d\s*=\s*([^\s;]+)").unwrap();
    let mut dkim = Vec::new();
    for sig in parsed.get_all("DKIM-Signature").unwrap_or_default() {
        let s = sig.replace(['\n', '\t'], " ");
        if let Some(m) = dkim_d.captures(&s) {
            dkim.push(m[1].to_lowercase().trim_end_matches('.').to_string());
        }
    }
    let mut results = Vec::new();
    for a in parsed.get_all("Authentication-Results").unwrap_or_default() {
        for c in ar.captures_iter(&a).flatten() {
            results.push(json!({"dkim": c[1].to_lowercase(), "domain": c[2].to_lowercase().trim_end_matches('.')}));
        }
    }
    json!({"dkim_domains": dkim, "dkim_results": results})
}

/// Installs the nary doubles: header_auth, email intake switched off, events.emit writing the row.
pub fn install_nary() {
    crm_inbox::set_nary_calls(NaryCalls {
        header_auth: Some(Arc::new(header_auth)),
        detect_and_queue: None,
        email_intake_queue: Some(Arc::new(|db, _t, _m, _p, _a| {
            Box::pin(async move {
                let on =
                    granary_core::settings_store::get_optional(db, "nary.email.intake_enabled")
                        .await?;
                assert_ne!(
                    on.as_deref().map(|s| s.to_lowercase()),
                    Some("true".into()),
                    "intake needs wave 3"
                );
                Ok(Some("off".to_string()))
            })
        })),
        emit: Some(Arc::new(|db, type_, entity_type, entity_id, payload| {
            let (t, et) = (type_.to_string(), entity_type.map(|s| s.to_string()));
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO domain_events (id, type, entity_type, entity_id, payload) VALUES (gen_random_uuid(), $1, $2, $3, $4)",
                )
                .bind(t)
                .bind(et)
                .bind(entity_id)
                .bind(payload.unwrap_or(json!({})))
                .execute(&mut *db)
                .await?;
                Ok(())
            })
        })),
    });
}

static SERIAL: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

/// The poller's session pool and the nary/email seams are process-wide: tests using them run one at a time.
pub async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    SERIAL.lock().await
}
