//! routers/email_client.py: ports of the backend tests that exercise its routes and sync helpers with rows
//! written directly, monkeypatched internals or a fake mail server:
//! - test_inbox_unread_filter.py: test_a_personal_mailbox_can_show_unread_mail_only;
//! - test_audit_medium_w3.py: test_sync_dedupes_per_folder_and_never_stores_a_uid_twice,
//!   test_the_unique_key_rejects_a_second_row_for_a_uid, test_a_failed_sync_records_the_error_and_closes_imap,
//!   test_an_overlapping_sync_is_skipped;
//! - test_mail_sync.py: test_personal_read_flags_follow_the_server;
//! - test_delete_mail_chats.py (personal mailbox);
//! - test_inbox_bulk.py (personal email).
//!
//! FakeIMAP becomes the GreenMail container (IMAP 127.0.0.1:53143, any login accepted); the failing login
//! is a few-line fake IMAP server.

mod support;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde_json::json;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use uuid::Uuid;

use granary_api::routers::email_client;
use granary_db::rows::email_client::EmailAccount;
use granary_services::imap_sync::{self, Imap4};

const GM_HOST: &str = "127.0.0.1";
const GM_IMAP: u16 = 53143;

fn app() -> &'static support::TestApp {
    support::app("email_client")
}

fn gm_user() -> String {
    format!(
        "api4e-{}@granary.test",
        &Uuid::new_v4().simple().to_string()[..8]
    )
}

async fn gm_login(user: &str) -> Imap4 {
    let mut i = Imap4::open(GM_HOST, GM_IMAP, false)
        .await
        .expect("greenmail");
    assert!(i.login(user, "x").await.unwrap().ok());
    i
}

fn raw_mail(msg_id: &str) -> Vec<u8> {
    format!(
        "Message-ID: {msg_id}\r\nFrom: Ann <ann@example.test>\r\nTo: me@example.test\r\nSubject: Pytest W3 mail\r\n\
         Date: Thu, 08 Oct 2026 10:00:00 +0700\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n\
         Hello there, this is the body of the message.\r\n"
    )
    .into_bytes()
}

async fn put(user: &str, mbox: &str, raw: &[u8], seen: bool) {
    let mut i = gm_login(user).await;
    if mbox != "INBOX" {
        let _ = i.create(&imap_sync::quote_mailbox(mbox)).await;
    }
    let r = i
        .append(
            raw,
            &imap_sync::quote_mailbox(mbox),
            if seen { Some("(\\Seen)") } else { None },
        )
        .await
        .unwrap();
    assert!(r.ok());
    let _ = i.logout().await;
}

async fn server_uids(user: &str, mbox: &str) -> Vec<i64> {
    let mut i = gm_login(user).await;
    if !imap_sync::select(&mut i, mbox)
        .await
        .map(|r| r.ok())
        .unwrap_or(false)
    {
        return Vec::new();
    }
    let r = i.uid_search(&["ALL"]).await.unwrap();
    let _ = i.logout().await;
    imap_sync::parse_uids(&r.lines)
}

async fn server_flags(user: &str, mbox: &str) -> std::collections::BTreeMap<i64, bool> {
    let mut i = gm_login(user).await;
    imap_sync::select(&mut i, mbox).await.unwrap();
    let r = i.uid_search(&["ALL"]).await.unwrap();
    let out = imap_sync::fetch_seen(&mut i, &imap_sync::parse_uids(&r.lines)).await;
    let _ = i.logout().await;
    out
}

/// The `mailbox` fixture: a person and their account (on GreenMail when `gm` is given).
async fn mailbox(gm: Option<&str>, port: u16) -> (support::TestUser, EmailAccount) {
    let a = app();
    let person = a.user_with(&[]).await;
    let mut db = a.db().await;
    let account: EmailAccount = sqlx::query_as(
        "INSERT INTO email_accounts (id, user_id, email_address, imap_host, imap_port, imap_use_ssl, smtp_host, \
         username, password_encrypted, is_active, sync_enabled, is_default) VALUES (gen_random_uuid(), $1, $2, $3, \
         $4, false, 'smtp.test', $5, $6, true, true, false) RETURNING *",
    )
    .bind(person.id)
    .bind(format!("{}@granary.test", &Uuid::new_v4().simple().to_string()[..6]))
    .bind(if gm.is_some() { GM_HOST } else { "imap.test" })
    .bind(port as i32)
    .bind(gm.unwrap_or("u"))
    .bind(granary_services::email_crypto::encrypt("pw"))
    .fetch_one(&mut db)
    .await
    .unwrap();
    (person, account)
}

async fn personal(account: &EmailAccount, uid: i32, folder: &str, is_read: bool) -> Uuid {
    let mut db = app().db().await;
    sqlx::query_scalar(
        "INSERT INTO email_messages (id, account_id, message_id, imap_uid, folder, subject, is_read, is_starred, \
         is_deleted, to_addresses, cc_addresses, attachments) VALUES (gen_random_uuid(), $1, $2, $3, $4, 'hi', $5, \
         false, false, '{}', '{}', '[]') RETURNING id",
    )
    .bind(account.id)
    .bind(format!("<{}@x>", Uuid::new_v4().simple()))
    .bind(uid)
    .bind(folder)
    .bind(is_read)
    .fetch_one(&mut db)
    .await
    .unwrap()
}

async fn row(
    id: Uuid,
) -> (
    Option<String>,
    Option<i32>,
    Option<bool>,
    Option<bool>,
    Option<bool>,
) {
    let mut db = app().db().await;
    sqlx::query_as("SELECT folder, imap_uid, is_deleted, is_read, is_starred FROM email_messages WHERE id = $1")
        .bind(id)
        .fetch_one(&mut db)
        .await
        .unwrap()
}

// ─── test_inbox_unread_filter.py ────────────────────────────────────────────

#[tokio::test]
async fn test_a_personal_mailbox_can_show_unread_mail_only() {
    let a = app();
    let me = a.superadmin().await;
    let mut db = a.db().await;
    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO email_accounts (id, user_id, email_address, imap_host, smtp_host, username, password_encrypted) \
         VALUES (gen_random_uuid(), $1, $2, 'imap.test', 'smtp.test', 'u', 'x') RETURNING id",
    )
    .bind(me.id)
    .bind(format!("{}@granary.test", &Uuid::new_v4().simple().to_string()[..6]))
    .fetch_one(&mut db)
    .await
    .unwrap();
    let mut ids = Vec::new();
    for i in 0..3 {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO email_messages (id, account_id, message_id, imap_uid, folder, subject, is_read, is_deleted) \
             VALUES (gen_random_uuid(), $1, $2, $3, 'INBOX', $4, $5, false) RETURNING id",
        )
        .bind(account)
        .bind(format!("<{i}-{}@x>", Uuid::new_v4().simple()))
        .bind(500 + i)
        .bind(format!("m{i}"))
        .bind(i != 1)
        .fetch_one(&mut db)
        .await
        .unwrap();
        ids.push(id);
    }
    let res = a
        .get(
            &me.token,
            &format!("/api/v1/email/inbox?account_id={account}&unread_only=true"),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let got: Vec<String> = res
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(got, vec![ids[1].to_string()]);
}

// ─── test_audit_medium_w3.py ────────────────────────────────────────────────

#[tokio::test]
async fn test_sync_dedupes_per_folder_and_never_stores_a_uid_twice() {
    let gm = gm_user();
    let msg_id = format!("<{}@example.test>", Uuid::new_v4().simple());
    put(&gm, "INBOX", &raw_mail(&msg_id), true).await;
    put(&gm, "Sent", &raw_mail(&msg_id), true).await; // mail to yourself: also in Sent
    let (_p, acct) = mailbox(Some(&gm), GM_IMAP).await;
    let mut imap = gm_login(&gm).await;
    let mut db = app().db().await;
    imap_sync::select(&mut imap, "INBOX").await.unwrap();
    email_client::_store_one_uid(&mut imap, &mut db, &acct, "INBOX", "1")
        .await
        .unwrap();
    email_client::_store_one_uid(&mut imap, &mut db, &acct, "INBOX", "1")
        .await
        .unwrap();
    imap_sync::select(&mut imap, "Sent").await.unwrap();
    email_client::_store_one_uid(&mut imap, &mut db, &acct, "SENT", "1")
        .await
        .unwrap();
    let _ = imap.logout().await;
    let mut rows: Vec<(Option<String>, Option<i32>)> =
        sqlx::query_as("SELECT folder, imap_uid FROM email_messages WHERE account_id = $1")
            .bind(acct.id)
            .fetch_all(&mut db)
            .await
            .unwrap();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            (Some("INBOX".into()), Some(1)),
            (Some("SENT".into()), Some(1))
        ]
    );
    let (subject, from, body, read): (Option<String>, Option<String>, Option<String>, Option<bool>) = sqlx::query_as(
        "SELECT subject, from_address, body_text, is_read FROM email_messages WHERE account_id = $1 AND folder = 'INBOX'",
    )
    .bind(acct.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(subject.as_deref(), Some("Pytest W3 mail"));
    assert_eq!(from.as_deref(), Some("Ann <ann@example.test>"));
    assert!(body.unwrap().starts_with("Hello there"));
    assert_eq!(read, Some(true));
}

#[tokio::test]
async fn test_the_unique_key_rejects_a_second_row_for_a_uid() {
    let (_p, acct) = mailbox(None, 993).await;
    let mut db = app().db().await;
    let mut tx = sqlx::Connection::begin(&mut db).await.unwrap();
    let mut errs = 0;
    for n in 0..2 {
        if sqlx::query(
            "INSERT INTO email_messages (id, account_id, message_id, folder, imap_uid, is_deleted) VALUES \
             (gen_random_uuid(), $1, $2, 'INBOX', 5, false)",
        )
        .bind(acct.id)
        .bind(format!("<{n}@x>"))
        .execute(&mut *tx)
        .await
        .is_err()
        {
            errs += 1;
        }
    }
    assert_eq!(errs, 1, "IntegrityError");
}

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
                let _ = w.write_all(b"* OK IMAP4rev1 ready\r\n").await;
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
                        "LOGIN" => format!("{tag} NO login refused\r\n"),
                        "LOGOUT" => format!("* BYE\r\n{tag} OK bye\r\n"),
                        _ => format!("{tag} BAD what\r\n"),
                    };
                    let _ = w.write_all(reply.as_bytes()).await;
                }
            });
        }
    });
    (port, seen)
}

#[tokio::test]
async fn test_a_failed_sync_records_the_error_and_closes_imap() {
    let a = app();
    let _g = a.serial().await;
    a.set_setting("mail.sync_lock_ttl_seconds", "60").await;
    let (port, commands) = refusing_server().await;
    let (_p, acct) = mailbox(Some("u@x.test"), port).await;
    let pool = PgPool::connect(&a.db_url).await.unwrap();
    email_client::_sync_account(pool.clone(), acct.id, "INBOX".into()).await;
    let err: Option<String> =
        sqlx::query_scalar("SELECT last_sync_error FROM email_accounts WHERE id = $1")
            .bind(acct.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    // imap_sync.connect checks the LOGIN reply (port findings batch 3): a clear error, not "NONAUTH".
    assert_eq!(
        err.as_deref(),
        Some("IMAP login failed: check the username and password.")
    );
    assert!(
        commands.lock().unwrap().contains(&"LOGOUT".to_string()),
        "imap closed"
    );
}

#[tokio::test]
async fn test_an_overlapping_sync_is_skipped() {
    let a = app();
    let _g = a.serial().await;
    a.set_setting("mail.sync_lock_ttl_seconds", "60").await;
    let (_p, acct) = mailbox(None, 993).await;
    let pool = PgPool::connect(&a.db_url).await.unwrap();
    let ran: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let mut r = granary_services::redis_lease::client().await.unwrap();
    let key = format!("mail:sync:{}:INBOX", acct.id);
    assert!(
        granary_services::redis_lease::acquire(&mut r, &key, "someone-else", 60)
            .await
            .unwrap()
    );
    let fake = |ran: Arc<Mutex<Vec<String>>>| {
        move |_p: PgPool, _a: Uuid, folder: String| -> futures::future::BoxFuture<'static, ()> {
            Box::pin(async move {
                ran.lock().unwrap().push(folder);
            })
        }
    };
    email_client::_sync_account_with(pool.clone(), acct.id, "INBOX".into(), fake(ran.clone()))
        .await;
    assert!(ran.lock().unwrap().is_empty());
    let _: () = redis::cmd("DEL")
        .arg(&key)
        .query_async(&mut r)
        .await
        .unwrap();
    email_client::_sync_account_with(pool.clone(), acct.id, "INBOX".into(), fake(ran.clone()))
        .await;
    assert_eq!(*ran.lock().unwrap(), vec!["INBOX".to_string()]);
    let left: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut r)
        .await
        .unwrap();
    assert!(left.is_none(), "lease released");
}

// ─── test_mail_sync.py ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_personal_read_flags_follow_the_server() {
    let gm = gm_user();
    for seen in [true, false, true] {
        put(
            &gm,
            "INBOX",
            &raw_mail(&format!("<{}@x>", Uuid::new_v4().simple())),
            seen,
        )
        .await;
    }
    let (_p, account) = mailbox(Some(&gm), GM_IMAP).await;
    let mut rows = Vec::new();
    for uid in 1..=3 {
        rows.push(personal(&account, uid, "INBOX", false).await);
    }
    let mut imap = gm_login(&gm).await;
    imap_sync::select(&mut imap, "INBOX").await.unwrap();
    let mut db = app().db().await;
    let changed = email_client::_refresh_read_flags(&mut imap, &mut db, &account, "INBOX")
        .await
        .unwrap();
    let _ = imap.logout().await;
    let mut states = Vec::new();
    for r in &rows {
        states.push(row(*r).await.3);
    }
    assert_eq!(changed, 2);
    assert_eq!(states, vec![Some(true), Some(false), Some(true)]);
}

// ─── test_delete_mail_chats.py (personal mailbox) ───────────────────────────

#[tokio::test]
async fn test_a_personal_email_goes_to_trash_then_can_be_deleted_for_good() {
    let a = app();
    let gm = gm_user();
    put(
        &gm,
        "INBOX",
        &raw_mail(&format!("<{}@x>", Uuid::new_v4().simple())),
        true,
    )
    .await;
    let mut i = gm_login(&gm).await;
    let _ = i.create("Trash").await;
    let _ = i.logout().await;
    let (person, account) = mailbox(Some(&gm), GM_IMAP).await;
    let m = personal(&account, 1, "INBOX", true).await;
    let first = a
        .delete(&person.token, &format!("/api/v1/email/messages/{m}"))
        .await;
    assert_eq!(first.json()["deleted"], "trash", "{}", first.text());
    let (folder, uid, deleted, _, _) = row(m).await;
    assert_eq!(
        (folder.as_deref(), uid, deleted),
        (Some("TRASH"), None, Some(false))
    );
    let trash_uid = server_uids(&gm, "Trash").await;
    assert_eq!(trash_uid.len(), 1);
    // what the next Trash sync would record
    let mut db = a.db().await;
    sqlx::query("UPDATE email_messages SET imap_uid = $1 WHERE id = $2")
        .bind(trash_uid[0] as i32)
        .bind(m)
        .execute(&mut db)
        .await
        .unwrap();
    let second = a
        .delete(&person.token, &format!("/api/v1/email/messages/{m}"))
        .await;
    assert_eq!(second.json()["deleted"], "forever");
    assert!(
        server_uids(&gm, "Trash").await.is_empty(),
        "expunged from Trash on the server"
    );
    assert_eq!(row(m).await.2, Some(true));
}

#[tokio::test]
async fn test_someone_elses_mail_cannot_be_deleted() {
    let a = app();
    let (_p, account) = mailbox(None, 993).await;
    let m = personal(&account, 502, "INBOX", true).await;
    let stranger = a.user_with(&[]).await;
    assert_eq!(
        a.delete(&stranger.token, &format!("/api/v1/email/messages/{m}"))
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn test_mail_deleted_in_another_client_leaves_but_a_mass_vanish_is_ignored() {
    let (_p, account) = mailbox(None, 993).await;
    let mut rows = Vec::new();
    for i in 0..30 {
        rows.push(personal(&account, 600 + i, "INBOX", true).await);
    }
    let mut db = app().db().await;
    let server: HashSet<i64> = (0..30)
        .map(|i| 600 + i)
        .filter(|u| *u != 600 && *u != 601)
        .collect();
    let dropped = email_client::_drop_vanished(&mut db, &account, "INBOX", &server)
        .await
        .unwrap();
    assert_eq!(dropped, 2);
    assert_eq!(row(rows[0]).await.2, Some(true));
    // The server suddenly reports an empty folder (renumbered, or a bad answer): keep everything.
    assert_eq!(
        email_client::_drop_vanished(&mut db, &account, "INBOX", &HashSet::new())
            .await
            .unwrap(),
        0
    );
}

// ─── test_inbox_bulk.py (personal email) ────────────────────────────────────

#[tokio::test]
async fn test_personal_bulk_delete_read_and_star() {
    let a = app();
    let gm = gm_user();
    for _ in 0..2 {
        put(
            &gm,
            "INBOX",
            &raw_mail(&format!("<{}@x>", Uuid::new_v4().simple())),
            true,
        )
        .await;
    }
    let mut i = gm_login(&gm).await;
    let _ = i.create("Trash").await;
    let _ = i.logout().await;
    let (person, account) = mailbox(Some(&gm), GM_IMAP).await;
    let ma = personal(&account, 1, "INBOX", true).await;
    let mb = personal(&account, 2, "INBOX", true).await;
    let ids = json!([ma.to_string(), mb.to_string()]);
    let counted = a
        .get(
            &person.token,
            &format!(
                "/api/v1/email/inbox/count?account_id={}&folder=INBOX",
                account.id
            ),
        )
        .await;
    assert_eq!(counted.json(), json!({"total": 2}), "{}", counted.text());
    let r = a
        .post(
            &person.token,
            "/api/v1/email/messages/bulk",
            json!({"action": "unread", "ids": ids}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["affected"].clone()),
        (200, json!(2)),
        "{}",
        r.text()
    );
    let mut unseen = false;
    for _ in 0..100 {
        if server_flags(&gm, "INBOX").await.values().all(|s| !*s) {
            unseen = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(unseen, "-FLAGS \\Seen on the server");
    a.post(
        &person.token,
        "/api/v1/email/messages/bulk",
        json!({"action": "star", "ids": ids}),
    )
    .await;
    assert_eq!((row(ma).await.4, row(mb).await.4), (Some(true), Some(true)));
    let r = a
        .post(
            &person.token,
            "/api/v1/email/messages/bulk",
            json!({"action": "delete", "filter": {"account_id": account.id.to_string(), "folder": "INBOX"}}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["affected"].clone()),
        (200, json!(2)),
        "{}",
        r.text()
    );
    assert_eq!(
        server_uids(&gm, "Trash").await.len(),
        2,
        "moved to Trash on the server"
    );
    assert_eq!(
        (row(ma).await.0, row(mb).await.0),
        (Some("TRASH".into()), Some("TRASH".into()))
    );
}

#[tokio::test]
async fn test_someone_elses_mail_is_not_touched_in_bulk() {
    let a = app();
    let (_p, account) = mailbox(None, 993).await;
    let m = personal(&account, 703, "INBOX", true).await;
    let stranger = a.user_with(&[]).await;
    let r = a
        .post(
            &stranger.token,
            "/api/v1/email/messages/bulk",
            json!({"action": "star", "ids": [m.to_string()]}),
        )
        .await;
    assert_eq!(r.json()["affected"], 0);
    assert_eq!(row(m).await.4, Some(false));
}
