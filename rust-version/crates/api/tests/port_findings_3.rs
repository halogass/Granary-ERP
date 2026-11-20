//! Port findings batch 3 (Python main a68fc84): the parts of backend/tests/test_port_findings_3_{a,b,c}.py
//! that need a fake mail server, rows written directly or the core helpers. The HTTP-only parts are in
//! compat-tests/http-suite/tests/test_port_findings_3.py; the WebSocket ones in messaging_ws.rs.
//!
//! - test_other_value_errors_are_not_masked -> `the_bad_path_parameter_is_named` (the field rule);
//! - test_bcc_recipients_get_the_mail_without_a_bcc_header (SMTP sink + GreenMail Sent folder);
//! - test_personal_sync_records_a_clear_login_error, test_imap_connect_raises_and_closes_on_a_refused_login;
//! - test_leave_delete_by_owner_and_by_manager (the owner through the employee record).

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use uuid::Uuid;

use granary_api::routers::email_client;
use granary_core::errors::{bad_uuid_field, ApiError};
use granary_services::imap_sync::{self, ConnectError, Imap4};

const GM_HOST: &str = "127.0.0.1";
const GM_IMAP: u16 = 53143;

fn app() -> &'static support::TestApp {
    support::app("port_findings_3")
}

// ─── the central 422 ─────────────────────────────────────────────────────────

#[test]
fn the_bad_path_parameter_is_named() {
    let good = Uuid::new_v4().to_string();
    assert_eq!(bad_uuid_field([("lead_id", "nope")]), "lead_id");
    assert_eq!(
        bad_uuid_field([("rfq_id", good.as_str()), ("line_id", "x")]),
        "line_id"
    );
    // only `id` and `*_id` parameters count; uuid.UUID's own acceptance (braces, no hyphens) is a UUID
    assert_eq!(bad_uuid_field([("token", "nope")]), "id");
    assert_eq!(bad_uuid_field([("id", "nope")]), "id");
    let braced = format!("{{{good}}}");
    assert_eq!(bad_uuid_field([("doc_id", braced.as_str())]), "id");
    assert_eq!(bad_uuid_field(std::iter::empty::<(&str, &str)>()), "id");
    let body = ApiError::invalid_date().body();
    assert_eq!(
        body,
        json!({"detail": "Validation failed", "errors": [{"field": "date",
               "message": "Invalid date format, expected YYYY-MM-DD (ISO 8601)"}], "code": "VALIDATION_ERROR"})
    );
    assert_eq!(ApiError::invalid_id().status().as_u16(), 422);
}

#[tokio::test]
async fn a_server_side_uuid_cast_is_the_central_422() {
    // "invalid input syntax for type uuid" from Postgres (a text literal compared with a UUID column).
    let a = app();
    let mut db = a.db().await;
    let err = sqlx::query("SELECT $1::text::uuid")
        .bind("nope")
        .execute(&mut db)
        .await
        .unwrap_err();
    let api: ApiError = err.into();
    assert_eq!(api.status().as_u16(), 422);
    assert_eq!(
        api.body(),
        json!({"detail": "Validation failed", "errors": [{"field": "id", "message": "Invalid id format"}],
               "code": "VALIDATION_ERROR"})
    );
}

// ─── Bcc ─────────────────────────────────────────────────────────────────────

/// An SMTP sink that records the envelope commands and the DATA.
async fn smtp_sink() -> (u16, Arc<Mutex<(Vec<String>, String)>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let rec = Arc::new(Mutex::new((Vec::new(), String::new())));
    let r2 = rec.clone();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = sock.into_split();
        let mut rd = BufReader::new(rd);
        wr.write_all(b"220 sink ESMTP\r\n").await.unwrap();
        let mut in_data = false;
        loop {
            let mut line = String::new();
            if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            if in_data {
                if line == ".\r\n" {
                    in_data = false;
                    wr.write_all(b"250 2.0.0 queued\r\n").await.unwrap();
                } else {
                    r2.lock().unwrap().1.push_str(&line);
                }
                continue;
            }
            let cmd = line.trim_end_matches("\r\n").to_string();
            r2.lock().unwrap().0.push(cmd.clone());
            let up = cmd.to_uppercase();
            let reply: &[u8] = if up.starts_with("EHLO") {
                b"250-sink\r\n250-8BITMIME\r\n250-AUTH PLAIN LOGIN\r\n250 SIZE 10000000\r\n"
            } else if up.starts_with("AUTH") {
                b"235 2.7.0 ok\r\n"
            } else if up.starts_with("MAIL") || up.starts_with("RCPT") || up.starts_with("RSET") {
                b"250 ok\r\n"
            } else if up == "DATA" {
                in_data = true;
                b"354 go\r\n"
            } else if up == "QUIT" {
                let _ = wr.write_all(b"221 bye\r\n").await;
                break;
            } else {
                b"502 no\r\n"
            };
            wr.write_all(reply).await.unwrap();
        }
    });
    (port, rec)
}

async fn gm_login(user: &str) -> Imap4 {
    let mut i = Imap4::open(GM_HOST, GM_IMAP, false)
        .await
        .expect("greenmail");
    assert!(i.login(user, "x").await.unwrap().ok());
    i
}

/// Everything in the user's Sent folder on GreenMail, as one text.
async fn sent_folder(user: &str) -> String {
    let mut i = gm_login(user).await;
    let mut out = String::new();
    if imap_sync::select(&mut i, "Sent")
        .await
        .map(|r| r.ok())
        .unwrap_or(false)
    {
        let r = i.uid("fetch", &["1:*", "(BODY.PEEK[])"]).await.unwrap();
        for l in r.lines {
            out.push_str(&String::from_utf8_lossy(&l));
        }
    }
    let _ = i.logout().await;
    out
}

#[tokio::test]
async fn test_bcc_recipients_get_the_mail_without_a_bcc_header() {
    let a = app();
    let me = a.user_with(&[]).await;
    let gm = format!(
        "pf3c-{}@granary.test",
        &Uuid::new_v4().simple().to_string()[..8]
    );
    {
        let mut i = gm_login(&gm).await;
        let _ = i.create("Sent").await;
        let _ = i.logout().await;
    }
    let (port, rec) = smtp_sink().await;
    let mut db = a.db().await;
    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO email_accounts (id, user_id, email_address, display_name, imap_host, imap_port, imap_use_ssl, \
         smtp_host, smtp_port, smtp_use_tls, username, password_encrypted, is_active, sync_enabled, is_default) \
         VALUES (gen_random_uuid(), $1, $2, 'Pytest Sender', $3, $4, false, '127.0.0.1', $5, false, $6, $7, true, \
         false, false) RETURNING id",
    )
    .bind(me.id)
    .bind(format!("{}@example.test", &Uuid::new_v4().simple().to_string()[..8]))
    .bind(GM_HOST)
    .bind(GM_IMAP as i32)
    .bind(port as i32)
    .bind(&gm)
    .bind(granary_services::email_crypto::encrypt("p"))
    .fetch_one(&mut db)
    .await
    .unwrap();
    let res = a
        .post(
            &me.token,
            "/api/v1/email/send",
            json!({"account_id": account.to_string(), "to": ["to@example.test"], "cc": ["cc@example.test"],
                   "bcc": ["hidden@example.test"], "subject": "Pytest bcc", "body_html": "<p>Hi</p>"}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let (cmds, data) = rec.lock().unwrap().clone();
    let mut rcpt: Vec<String> = cmds
        .iter()
        .filter(|c| c.to_uppercase().starts_with("RCPT TO:"))
        .map(|c| {
            c[8..]
                .trim_matches(|ch| ch == '<' || ch == '>' || ch == ' ')
                .to_string()
        })
        .collect();
    rcpt.sort();
    assert_eq!(
        rcpt,
        ["cc@example.test", "hidden@example.test", "to@example.test"]
    );
    assert!(!data.contains("hidden@example.test"), "{data}");
    assert!(!data.to_lowercase().contains("\r\nbcc:"), "{data}");
    // The sender's own Sent copy keeps the Bcc line (saved off the request: wait for it).
    let mut copy = String::new();
    for _ in 0..100 {
        copy = sent_folder(&gm).await;
        if copy.contains("Pytest bcc") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(copy.contains("Bcc: hidden@example.test"), "{copy}");
}

// ─── a refused IMAP login ────────────────────────────────────────────────────

/// An IMAP server that answers LOGIN with NO and records the commands it gets.
async fn refusing_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let _ = w.write_all(b"* OK fake IMAP ready\r\n").await;
                let mut r = BufReader::new(r);
                let mut line = String::new();
                loop {
                    line.clear();
                    if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let mut parts = line.trim_end().splitn(3, ' ');
                    let tag = parts.next().unwrap_or("").to_string();
                    let cmd = parts.next().unwrap_or("").to_uppercase();
                    log.lock().unwrap().push(cmd.clone());
                    let reply = match cmd.as_str() {
                        "CAPABILITY" => format!("* CAPABILITY IMAP4rev1\r\n{tag} OK done\r\n"),
                        "LOGIN" => format!("{tag} NO [AUTHENTICATIONFAILED] login refused\r\n"),
                        "LOGOUT" => format!("* BYE\r\n{tag} OK bye\r\n"),
                        _ => format!("{tag} BAD command {cmd} illegal in state NONAUTH\r\n"),
                    };
                    let _ = w.write_all(reply.as_bytes()).await;
                    if cmd == "LOGOUT" {
                        return;
                    }
                }
            });
        }
    });
    (port, seen)
}

#[tokio::test]
async fn test_personal_sync_records_a_clear_login_error() {
    let a = app();
    let _g = a.serial().await;
    a.set_setting("mail.sync_lock_ttl_seconds", "60").await;
    let (port, _) = refusing_server().await;
    let me = a.user_with(&[]).await;
    let pool = PgPool::connect(&a.db_url).await.unwrap();
    let acct: Uuid = sqlx::query_scalar(
        "INSERT INTO email_accounts (id, user_id, email_address, imap_host, imap_port, imap_use_ssl, smtp_host, \
         username, password_encrypted, is_active, sync_enabled, is_default) VALUES (gen_random_uuid(), $1, $2, \
         '127.0.0.1', $3, false, 'smtp.test', 'u', $4, true, true, false) RETURNING id",
    )
    .bind(me.id)
    .bind(format!("{}@granary.test", &Uuid::new_v4().simple().to_string()[..6]))
    .bind(port as i32)
    .bind(granary_services::email_crypto::encrypt("p"))
    .fetch_one(&pool)
    .await
    .unwrap();
    email_client::_sync_account_locked(pool.clone(), acct, "INBOX".into()).await;
    let stored = email_client::_backfill_account(pool.clone(), acct, "INBOX".into()).await;
    assert_eq!(stored, (0, false));
    let error: Option<String> =
        sqlx::query_scalar("SELECT last_sync_error FROM email_accounts WHERE id = $1")
            .bind(acct)
            .fetch_one(&pool)
            .await
            .unwrap();
    let error = error.unwrap_or_default();
    assert!(error.starts_with("IMAP login failed"), "{error}");
    assert!(!error.contains("NONAUTH"), "{error}");
}

/// A mail server that accepts the connection and never answers: the sync gives up after the IMAP timeout,
/// records a readable timeout as the account's error and returns, so the loop goes on with the next account.
#[tokio::test]
async fn test_personal_sync_records_a_clear_timeout_and_moves_on() {
    let a = app();
    let _g = a.serial().await;
    a.set_setting("mail.sync_lock_ttl_seconds", "60").await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let silent = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock); // never written to, never closed
        }
    });
    let me = a.user_with(&[]).await;
    let pool = PgPool::connect(&a.db_url).await.unwrap();
    let acct: Uuid = sqlx::query_scalar(
        "INSERT INTO email_accounts (id, user_id, email_address, imap_host, imap_port, imap_use_ssl, smtp_host, \
         username, password_encrypted, is_active, sync_enabled, is_default) VALUES (gen_random_uuid(), $1, $2, \
         '127.0.0.1', $3, false, 'smtp.test', 'u', $4, true, true, false) RETURNING id",
    )
    .bind(me.id)
    .bind(format!("{}@granary.test", &Uuid::new_v4().simple().to_string()[..6]))
    .bind(port as i32)
    .bind(granary_services::email_crypto::encrypt("p"))
    .fetch_one(&pool)
    .await
    .unwrap();
    tokio::time::timeout(
        imap_sync::aioimap::TIMEOUT * 3,
        email_client::_sync_account_locked(pool.clone(), acct, "INBOX".into()),
    )
    .await
    .expect("the sync must give up on a silent server by itself");
    silent.abort();
    let error: Option<String> =
        sqlx::query_scalar("SELECT last_sync_error FROM email_accounts WHERE id = $1")
            .bind(acct)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(error.as_deref(), Some(imap_sync::aioimap::TIMEOUT_TEXT));
}

#[tokio::test]
async fn test_imap_connect_raises_and_closes_on_a_refused_login() {
    let (port, seen) = refusing_server().await;
    let res = tokio::time::timeout(
        Duration::from_secs(10),
        imap_sync::connect("127.0.0.1", port as i64, false, "u", "wrong"),
    )
    .await
    .unwrap();
    match res {
        Err(ConnectError::Login(m)) => {
            assert_eq!(m, "IMAP login failed: check the username and password.")
        }
        other => panic!("expected a refused login, got {:?}", other.map(|_| ())),
    }
    // The refused connection is closed, not leaked.
    for _ in 0..50 {
        if seen.lock().unwrap().last().map(String::as_str) == Some("LOGOUT") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let seen = seen.lock().unwrap().clone();
    assert!(seen.contains(&"LOGIN".to_string()), "{seen:?}");
    assert_eq!(seen.last().map(String::as_str), Some("LOGOUT"), "{seen:?}");
}

// ─── leave delete: the owner through the employee record ─────────────────────

#[tokio::test]
async fn test_leave_owner_through_the_employee_record_may_delete() {
    let a = app();
    let me = a.user_with(&["hr:submit_leave"]).await;
    let other = a.user_with(&["hr:submit_leave"]).await;
    let mut db = a.db().await;
    let emp: Uuid = sqlx::query_scalar(
        "INSERT INTO employees (id, employee_number, first_name, last_name, email, start_date, status, user_id, \
         is_deleted) VALUES (gen_random_uuid(), $1, 'Pf3a', 'Tester', $2, '2026-01-01', 'active', $3, false) \
         RETURNING id",
    )
    .bind(support::u("PF3A"))
    .bind(format!("{}@granary.test", support::u("pf3a")))
    .bind(me.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    // Submitted by someone else (HR on the employee's behalf), for my employee record.
    let mut leaves = Vec::new();
    for _ in 0..2 {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO leave_requests (id, employee_id, submitted_by_id, leave_type, start_date, end_date, days, \
             status, is_deleted) VALUES (gen_random_uuid(), $1, $2, 'annual', '2026-11-02', '2026-11-03', 2, \
             'pending', false) RETURNING id",
        )
        .bind(emp)
        .bind(other.id)
        .fetch_one(&mut db)
        .await
        .unwrap();
        leaves.push(id);
    }
    let r = a
        .delete(&me.token, &format!("/api/v1/hr/leave/{}", leaves[0]))
        .await;
    assert_eq!(r.status, 204, "{}", r.text());
    // The submitter may delete too; a third person may not.
    let third = a.user_with(&["hr:submit_leave"]).await;
    let r = a
        .delete(&third.token, &format!("/api/v1/hr/leave/{}", leaves[1]))
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        r.json(),
        json!({"detail": "You can only delete your own leave requests"})
    );
    let r = a
        .delete(&other.token, &format!("/api/v1/hr/leave/{}", leaves[1]))
        .await;
    assert_eq!(r.status, 204, "{}", r.text());
}
