//! routers/crm_inbox.py: ports of the backend tests that exercise these routes with rows written directly,
//! monkeypatched internals or a fake mail server:
//! - test_audit_low_w2.py: test_crm_attachment_download_handles_non_latin_names,
//!   test_crm_reply_response_includes_attachments_without_lazy_load (the email seams replace SMTP);
//! - test_bug_hunt_fixes.py: test_linking_a_hidden_number_chat_links_only_that_chat;
//! - test_inbox_unread_filter.py: test_the_crm_inbox_can_show_unread_threads_only;
//! - test_mail_sync.py: test_the_sent_box_lists_conversations_we_wrote_in,
//!   test_opening_a_thread_marks_it_read_on_the_server;
//! - test_delete_mail_chats.py (CRM inbox, through the routes);
//! - test_inbox_bulk.py (CRM email).
//!
//! Python's FakeIMAP becomes the GreenMail container (granary-s5-greenmail, IMAP 127.0.0.1:53143, any login
//! accepted), with a small proxy where a test needs a server that refuses or must count connections. The
//! CRM mailbox settings are process-wide, so the mailbox tests take `app.serial()`.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

use granary_services::imap_sync::{self, Imap4};

const TAG: &str = "crm_inbox";
const GM_HOST: &str = "127.0.0.1";
const GM_IMAP: u16 = 53143;
const MAILBOX: &str = "hello@example.test";

fn app() -> &'static support::TestApp {
    support::app(TAG)
}

// ─── GreenMail ──────────────────────────────────────────────────────────────

fn gm_user() -> String {
    format!(
        "api4-{}@example.test",
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

fn mail(subject: &str, frm: &str, to: &str, msg_id: &str, in_reply_to: Option<&str>) -> Vec<u8> {
    let mut s = format!(
        "Message-ID: {msg_id}\r\nFrom: {frm}\r\nTo: {to}\r\nSubject: {subject}\r\nDate: Thu, 15 Oct 2026 10:00:00 +0000\r\n"
    );
    if let Some(r) = in_reply_to {
        s.push_str(&format!("In-Reply-To: {r}\r\nReferences: {r}\r\n"));
    }
    s.push_str(&format!(
        "Content-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: 7bit\r\nMIME-Version: 1.0\r\n\r\nBody of {subject}\r\n"
    ));
    s.into_bytes()
}

fn mid(domain: &str) -> String {
    format!("<{}@{domain}>", Uuid::new_v4().simple())
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
    assert!(r.ok(), "{r:?}");
    let _ = i.logout().await;
}

async fn create(user: &str, mbox: &str) {
    let mut i = gm_login(user).await;
    let _ = i.create(&imap_sync::quote_mailbox(mbox)).await;
    let _ = i.logout().await;
}

async fn server_flags(user: &str, mbox: &str) -> std::collections::BTreeMap<i64, bool> {
    let mut i = gm_login(user).await;
    imap_sync::select(&mut i, mbox).await.unwrap();
    let search = i.uid_search(&["ALL"]).await.unwrap();
    let uids = imap_sync::parse_uids(&search.lines);
    let out = imap_sync::fetch_seen(&mut i, &uids).await;
    let _ = i.logout().await;
    out
}

async fn ensure_mailbox() {
    app().ensure_crm_mailbox(MAILBOX).await;
}

/// Points the CRM mailbox at a GreenMail user (through `port`: GreenMail or a proxy in front of it).
async fn configure(user: &str, port: u16) {
    ensure_mailbox().await;
    let a = app();
    // A fresh GreenMail user numbers its UIDs from 1 again: forget the UIDs (the poller's cursor) and the
    // skipped-UID marks earlier tests' mailboxes left (Python's tests use ever-growing `_uid_base()` UIDs).
    let mut db = a.db().await;
    sqlx::query("UPDATE crm_email_messages SET imap_uid = NULL WHERE imap_uid IS NOT NULL")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM system_settings WHERE key LIKE 'crm_inbox.skipped_uid.%' OR key = 'nary.junk_last_uid'")
        .execute(&mut db)
        .await
        .unwrap();
    a.set_setting("nary.email.intake_enabled", "false").await;
    a.set_setting("crm_imap_host", GM_HOST).await;
    a.set_setting("crm_imap_port", &port.to_string()).await;
    a.set_setting("crm_imap_user", user).await;
    a.set_setting(
        "crm_imap_password",
        &granary_services::email_crypto::encrypt("x"),
    )
    .await;
    a.set_setting("crm_imap_ssl", "false").await;
}

async fn poll() -> i64 {
    app()
        .on_server(async {
            granary_services::crm_inbox::poll_crm_inbox(true, false)
                .await
                .map_err(|e| e.to_string())
        })
        .await
        .expect("poll")
}

/// A customer email and our reply, synced into one CRM conversation (test_delete_mail_chats `_conversation`).
async fn conversation(port_of: impl FnOnce(&str) -> u16) -> (String, String, String) {
    let user = gm_user();
    let (asked, reply) = (mid("c.test"), mid("example.test"));
    create(&user, "Sent").await;
    create(&user, "Trash").await;
    put(
        &user,
        "INBOX",
        &mail("Price?", "ann@c.test", MAILBOX, &asked, None),
        true,
    )
    .await;
    put(
        &user,
        "Sent",
        &mail("Re: Price?", MAILBOX, "ann@c.test", &reply, Some(&asked)),
        true,
    )
    .await;
    configure(&user, port_of(&user)).await;
    poll().await;
    (user, asked, reply)
}

async fn msg_row(header: &str) -> Option<(Uuid, Uuid, bool)> {
    let mut db = app().db().await;
    sqlx::query_as(
        "SELECT id, thread_id, is_read FROM crm_email_messages WHERE message_id_header = $1",
    )
    .bind(header)
    .fetch_optional(&mut db)
    .await
    .unwrap()
}

async fn thread_row(id: Uuid) -> Option<(Option<bool>, Option<String>, Option<i32>)> {
    let mut db = app().db().await;
    sqlx::query_as("SELECT is_read, status, message_count FROM crm_email_threads WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut db)
        .await
        .unwrap()
}

async fn sysadmin() -> support::TestUser {
    app()
        .make_user(
            support::UserSpec::new()
                .perms(&["crm:view_all", "crm:view_whatsapp"])
                .system_admin(),
        )
        .await
}

/// A TCP proxy in front of GreenMail: counts connections, can refuse UID MOVE/STORE.
struct Proxy {
    port: u16,
    connections: Arc<AtomicUsize>,
}

async fn proxy(refuse: bool) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let connections = Arc::new(AtomicUsize::new(0));
    let count = connections.clone();
    // Runs on the server runtime: the test runtime ends with the test, the app keeps connecting.
    app()
        .on_server(async move {
            tokio::spawn(async move {
                loop {
                    let (client, _) = match listener.accept().await {
                        Ok(c) => c,
                        Err(_) => return,
                    };
                    count.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(async move {
                        let server = TcpStream::connect((GM_HOST, GM_IMAP)).await.unwrap();
                        let (cr, cw) = client.into_split();
                        let (mut sr, mut sw) = server.into_split();
                        let cw = Arc::new(tokio::sync::Mutex::new(cw));
                        let cw2 = cw.clone();
                        tokio::spawn(async move {
                            let mut buf = [0u8; 8192];
                            loop {
                                use tokio::io::AsyncReadExt;
                                let n = match sr.read(&mut buf).await {
                                    Ok(0) | Err(_) => return,
                                    Ok(n) => n,
                                };
                                if cw2.lock().await.write_all(&buf[..n]).await.is_err() {
                                    return;
                                }
                            }
                        });
                        let mut cr = BufReader::new(cr);
                        let mut line = Vec::new();
                        loop {
                            line.clear();
                            if cr.read_until(b'\n', &mut line).await.unwrap_or(0) == 0 {
                                return;
                            }
                            let s = String::from_utf8_lossy(&line).to_string();
                            let mut parts = s.trim_end().splitn(2, ' ');
                            let tag = parts.next().unwrap_or("").to_string();
                            let rest = parts.next().unwrap_or("").to_uppercase();
                            if refuse
                                && (rest.starts_with("UID MOVE") || rest.starts_with("UID STORE"))
                            {
                                let _ = cw
                                    .lock()
                                    .await
                                    .write_all(format!("{tag} NO refused\r\n").as_bytes())
                                    .await;
                                continue;
                            }
                            if sw.write_all(&line).await.is_err() {
                                return;
                            }
                        }
                    });
                }
            });
        })
        .await;
    Proxy { port, connections }
}

async fn wait_for<F, Fut>(mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..100 {
        if f().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

// ─── Rows ───────────────────────────────────────────────────────────────────

async fn add_thread(subject: &str, is_read: bool) -> Uuid {
    let mut db = app().db().await;
    sqlx::query_scalar(
        "INSERT INTO crm_email_threads (id, subject, status, is_read, message_count, last_message_at, last_sender_email) \
         VALUES (gen_random_uuid(), $1, 'open', $2, 0, now(), 'buyer@example.com') RETURNING id",
    )
    .bind(subject)
    .bind(is_read)
    .fetch_one(&mut db)
    .await
    .unwrap()
}

async fn add_message(thread: Uuid, direction: &str) -> Uuid {
    let mut db = app().db().await;
    sqlx::query_scalar(
        "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, to_emails, subject, \
         body_text, received_at, is_read, sent_by_nary) VALUES (gen_random_uuid(), $1, $2, $3, 'x@y.test', \
         ARRAY['sales@example.com'], 'pytest', 'hello', now(), true, false) RETURNING id",
    )
    .bind(thread)
    .bind(format!("<{}@pytest>", Uuid::new_v4()))
    .bind(direction)
    .fetch_one(&mut db)
    .await
    .unwrap()
}

// ─── test_audit_low_w2.py ───────────────────────────────────────────────────

#[tokio::test]
async fn test_crm_attachment_download_handles_non_latin_names() {
    let a = app();
    let admin = a.superadmin().await;
    let t = add_thread("pytest", false).await;
    let m = add_message(t, "inbound").await;
    let mut db = a.db().await;
    let att: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_email_attachments (id, message_id, filename, data) VALUES (gen_random_uuid(), $1, $2, $3) \
         RETURNING id",
    )
    .bind(m)
    .bind("Quotation\u{2014}\u{65e5}\u{672c} \"q\".pdf")
    .bind(b"%PDF".to_vec())
    .fetch_one(&mut db)
    .await
    .unwrap();
    let r = a
        .get(
            &admin.token,
            &format!("/api/v1/crm/inbox/attachments/{att}"),
        )
        .await;
    assert_eq!(r.status, 200);
    let cd = r.header("content-disposition").unwrap();
    assert!(cd.contains("filename*=UTF-8''") && cd.contains("%E6%97%A5"));
    assert!(cd.is_ascii());
}

#[tokio::test]
async fn test_crm_reply_response_includes_attachments_without_lazy_load() {
    use granary_services::email::{self, Capture, Seams, SmtpConfig};
    let a = app();
    let _g = a.serial().await;
    let admin = a.superadmin().await;
    let t = add_thread("pytest", false).await;
    add_message(t, "inbound").await;
    ensure_mailbox().await;
    a.set_setting("crm_imap_user", "").await; // no Sent copy to save
    email::set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(|_call| {
            Box::pin(async {
                Ok((
                    true,
                    Some(Capture {
                        raw: Some(b"raw".to_vec()),
                        message_id: None,
                    }),
                ))
            })
        })),
        load_system_smtp: None,
        load_crm_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    from_email: "me@example.test".into(),
                    from_name: "Your Company".into(),
                    ..Default::default()
                })
            })
        })),
    }));
    let form = reqwest::multipart::Form::new()
        .text("body_html", "<p>hi</p>")
        .part(
            "files",
            reqwest::multipart::Part::bytes(b"hello".to_vec())
                .file_name("a.txt")
                .mime_str("text/plain")
                .unwrap(),
        );
    let r = a
        .send(
            a.client()
                .post(a.url(&format!("/api/v1/crm/inbox/threads/{t}/reply")))
                .bearer_auth(&admin.token)
                .multipart(form),
        )
        .await;
    email::set_global_seams(None);
    assert_eq!(r.status, 201, "{}", r.text());
    let names: Vec<String> = r.json()["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["filename"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["a.txt"]);
    assert_eq!(r.json()["direction"], "outbound");
}

// ─── test_bug_hunt_fixes.py ─────────────────────────────────────────────────

fn lid() -> String {
    format!("1{:013}@lid", Uuid::new_v4().as_u128() % 10u128.pow(13))
}

#[tokio::test]
async fn test_linking_a_hidden_number_chat_links_only_that_chat() {
    let a = app();
    let admin = a.superadmin().await;
    let the_lid = lid();
    let strays = [lid(), lid()];
    let mut db = a.db().await;
    for cid in [&the_lid, &strays[0], &strays[1]] {
        sqlx::query(
            "INSERT INTO wa_chats (id, chat_id, status) VALUES (gen_random_uuid(), $1, 'active')",
        )
        .bind(cid)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let contact: Uuid = sqlx::query_scalar(
        "INSERT INTO contacts (id, first_name, last_name, custom, is_deleted) VALUES (gen_random_uuid(), 'Pytest', $1, \
         '{}', false) RETURNING id",
    )
    .bind(support::u("BH"))
    .fetch_one(&mut db)
    .await
    .unwrap();
    let res = a
        .post(
            &admin.token,
            "/api/v1/crm/inbox/wa-link",
            json!({"chat_id": the_lid, "contact_id": contact.to_string()}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let linked: Vec<(String, Option<Uuid>)> =
        sqlx::query_as("SELECT chat_id, contact_id FROM wa_chats WHERE chat_id = ANY($1)")
            .bind(vec![the_lid.clone(), strays[0].clone(), strays[1].clone()])
            .fetch_all(&mut db)
            .await
            .unwrap();
    for (cid, c) in &linked {
        if *cid == the_lid {
            assert_eq!(*c, Some(contact));
        } else {
            assert_eq!(*c, None);
        }
    }
    let hidden: Vec<String> =
        sqlx::query_scalar("SELECT phone FROM wa_contact_links WHERE contact_id = $1")
            .bind(contact)
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert_eq!(hidden, vec![the_lid.split('@').next().unwrap().to_string()]);
    let wa: Option<String> = sqlx::query_scalar("SELECT whatsapp FROM contacts WHERE id = $1")
        .bind(contact)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert!(wa.unwrap_or_default().is_empty(), "the id stays hidden");
}

// ─── test_inbox_unread_filter.py ────────────────────────────────────────────

#[tokio::test]
async fn test_the_crm_inbox_can_show_unread_threads_only() {
    let a = app();
    let admin = a.superadmin().await;
    let tag = support::u("pytest-unread");
    let read = add_thread(&format!("{tag} read"), true).await;
    add_message(read, "inbound").await;
    let unread = add_thread(&format!("{tag} unread"), false).await;
    add_message(unread, "inbound").await;
    let everything = a
        .get(
            &admin.token,
            &format!("/api/v1/crm/inbox/threads?search={tag}"),
        )
        .await;
    let only = a
        .get(
            &admin.token,
            &format!("/api/v1/crm/inbox/threads?search={tag}&unread=true"),
        )
        .await;
    assert_eq!(
        (everything.status, only.status),
        (200, 200),
        "{}",
        only.text()
    );
    let mut ids: Vec<String> = everything.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    let mut want = vec![read.to_string(), unread.to_string()];
    want.sort();
    assert_eq!(ids, want);
    let only_ids: Vec<String> = only.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(only_ids, vec![unread.to_string()]);
    assert_eq!(only.json()["total"], 1);
}

// ─── test_mail_sync.py ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_the_sent_box_lists_conversations_we_wrote_in() {
    let a = app();
    let _g = a.serial().await;
    let user = gm_user();
    let (asked, reply, other) = (mid("x.test"), mid("x.test"), mid("x.test"));
    let tag = &Uuid::new_v4().simple().to_string()[..8];
    put(
        &user,
        "INBOX",
        &mail(
            &format!("Answered {tag}"),
            "ann@customer.test",
            MAILBOX,
            &asked,
            None,
        ),
        true,
    )
    .await;
    put(
        &user,
        "INBOX",
        &mail(
            &format!("Unanswered {tag}"),
            "bob@customer.test",
            MAILBOX,
            &other,
            None,
        ),
        true,
    )
    .await;
    put(
        &user,
        "Sent Items",
        &mail(
            &format!("Re: Answered {tag}"),
            MAILBOX,
            "ann@customer.test",
            &reply,
            Some(&asked),
        ),
        true,
    )
    .await;
    configure(&user, GM_IMAP).await;
    let person = a.user_with(&["crm:view_all", "crm:send_email"]).await;
    // the route runs the same poll
    let r = a
        .post(&person.token, "/api/v1/crm/inbox/sync", json!({}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"status": "synced", "new": 3}));
    let sent = a
        .get(
            &person.token,
            &format!("/api/v1/crm/inbox/threads?box=sent&search={tag}"),
        )
        .await
        .json();
    let inbox = a
        .get(
            &person.token,
            &format!("/api/v1/crm/inbox/threads?box=inbox&search={tag}"),
        )
        .await
        .json();
    let subjects = |v: &Value| -> Vec<String> {
        let mut s: Vec<String> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["subject"].as_str().unwrap().to_string())
            .collect();
        s.sort();
        s
    };
    assert_eq!(subjects(&sent), vec![format!("Answered {tag}")]);
    assert_eq!(
        subjects(&inbox),
        vec![format!("Answered {tag}"), format!("Unanswered {tag}")]
    );
}

#[tokio::test]
async fn test_opening_a_thread_marks_it_read_on_the_server() {
    let a = app();
    let _g = a.serial().await;
    let user = gm_user();
    let msg_id = mid("customer.test");
    put(
        &user,
        "INBOX",
        &mail("Hello", "c@customer.test", MAILBOX, &msg_id, None),
        false,
    )
    .await;
    configure(&user, GM_IMAP).await;
    poll().await;
    let (_, tid, _) = msg_row(&msg_id).await.unwrap();
    assert_eq!(
        server_flags(&user, "INBOX")
            .await
            .values()
            .copied()
            .collect::<Vec<_>>(),
        vec![false]
    );
    let person = a.user_with(&["crm:view_all"]).await;
    let r = a
        .get(&person.token, &format!("/api/v1/crm/inbox/threads/{tid}"))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(thread_row(tid).await.unwrap().0, Some(true));
    assert!(
        wait_for(|| async { server_flags(&user, "INBOX").await.values().all(|s| *s) }).await,
        "\\Seen set on the server"
    );
    let r = a
        .post(
            &person.token,
            &format!("/api/v1/crm/inbox/threads/{tid}/unread"),
            json!({}),
        )
        .await;
    assert_eq!(r.json(), json!({"id": tid.to_string(), "is_read": false}));
    assert!(wait_for(|| async { server_flags(&user, "INBOX").await.values().all(|s| !*s) }).await);
    assert_eq!(thread_row(tid).await.unwrap().0, Some(false));
    assert!(!msg_row(&msg_id).await.unwrap().2);
}

// ─── test_delete_mail_chats.py (CRM inbox) ──────────────────────────────────

#[tokio::test]
async fn test_deleting_one_email_moves_it_to_trash_and_keeps_the_conversation() {
    let a = app();
    let _g = a.serial().await;
    let (user, asked, reply) = conversation(|_| GM_IMAP).await;
    let tid = msg_row(&asked).await.unwrap().1;
    let person = sysadmin().await;
    let r = a
        .delete(
            &person.token,
            &format!(
                "/api/v1/crm/inbox/messages/{}",
                msg_row(&reply).await.unwrap().0
            ),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(
        server_flags(&user, "Sent").await.is_empty(),
        "moved on the server, from the folder it lives in"
    );
    assert_eq!(server_flags(&user, "Trash").await.len(), 1);
    assert!(msg_row(&reply).await.is_none() && msg_row(&asked).await.is_some());
    assert_eq!(thread_row(tid).await.unwrap().2, Some(1));
}

#[tokio::test]
async fn test_deleting_a_conversation_removes_every_email() {
    let a = app();
    let _g = a.serial().await;
    let (user, asked, reply) = conversation(|_| GM_IMAP).await;
    let tid = msg_row(&asked).await.unwrap().1;
    let person = sysadmin().await;
    let r = a
        .delete(&person.token, &format!("/api/v1/crm/inbox/threads/{tid}"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        (
            r.json()["deleted"].clone(),
            r.json()["threads_removed"].clone()
        ),
        (json!(2), json!(1))
    );
    assert!(
        server_flags(&user, "INBOX").await.is_empty()
            && server_flags(&user, "Sent").await.is_empty()
    );
    assert_eq!(server_flags(&user, "Trash").await.len(), 2);
    assert!(thread_row(tid).await.is_none());
    assert!(msg_row(&asked).await.is_none() && msg_row(&reply).await.is_none());
}

#[tokio::test]
async fn test_an_email_with_attachments_can_be_deleted() {
    let a = app();
    let _g = a.serial().await;
    let (_user, asked, _reply) = conversation(|_| GM_IMAP).await;
    let (m, tid, _) = msg_row(&asked).await.unwrap();
    let mut db = a.db().await;
    sqlx::query(
        "INSERT INTO crm_email_attachments (id, message_id, filename, content_type, size_bytes, data) VALUES \
         (gen_random_uuid(), $1, 'image001.png', 'image/png', 3, 'png')",
    )
    .bind(m)
    .execute(&mut db)
    .await
    .unwrap();
    let person = sysadmin().await;
    let r = a
        .delete(&person.token, &format!("/api/v1/crm/inbox/threads/{tid}"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM crm_email_attachments WHERE message_id = $1")
            .bind(m)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(left, 0);
    assert!(msg_row(&asked).await.is_none());
}

#[tokio::test]
async fn test_a_refusing_server_changes_nothing() {
    let a = app();
    let _g = a.serial().await;
    let p = proxy(true).await;
    let (_user, asked, _) = conversation(|_| p.port).await;
    let person = sysadmin().await;
    let r = a
        .delete(
            &person.token,
            &format!(
                "/api/v1/crm/inbox/messages/{}",
                msg_row(&asked).await.unwrap().0
            ),
        )
        .await;
    assert_eq!(r.status, 502, "{}", r.text());
    assert!(r.json()["detail"].as_str().unwrap().contains("refused"));
    assert!(
        msg_row(&asked).await.is_some(),
        "nothing deleted in the ERP when the server says no"
    );
}

#[tokio::test]
async fn test_only_the_owner_and_system_admins_delete_crm_mail() {
    let a = app();
    let _g = a.serial().await;
    let (_user, asked, _) = conversation(|_| GM_IMAP).await;
    // Even a user still holding the retired permission code is refused.
    let person = a.user_with(&["crm:view_all", "crm:delete_messages"]).await;
    let (m, tid, _) = msg_row(&asked).await.unwrap();
    let r = a
        .delete(&person.token, &format!("/api/v1/crm/inbox/messages/{m}"))
        .await;
    let r2 = a
        .delete(&person.token, &format!("/api/v1/crm/inbox/threads/{tid}"))
        .await;
    assert_eq!((r.status, r2.status), (403, 403));
    assert!(msg_row(&asked).await.is_some());
}

// ─── test_inbox_bulk.py (CRM email) ─────────────────────────────────────────

/// Two customer emails on different subjects, synced as two CRM conversations.
async fn two_conversations(port_of: impl FnOnce(&str) -> u16) -> (String, Vec<String>) {
    let user = gm_user();
    let ids = [mid("c.test"), mid("c.test")];
    create(&user, "Trash").await;
    for (i, m) in ids.iter().enumerate() {
        let subject = format!("Question {i} {}", &Uuid::new_v4().simple().to_string()[..6]);
        put(
            &user,
            "INBOX",
            &mail(&subject, &format!("c{i}@c.test"), MAILBOX, m, None),
            true,
        )
        .await;
    }
    configure(&user, port_of(&user)).await;
    poll().await;
    let mut tids = Vec::new();
    for m in &ids {
        tids.push(msg_row(m).await.unwrap().1.to_string());
    }
    (user, tids)
}

#[tokio::test]
async fn test_bulk_delete_is_for_the_owner_and_system_admins() {
    let a = app();
    let _g = a.serial().await;
    let (_user, asked, _) = conversation(|_| GM_IMAP).await;
    let tid = msg_row(&asked).await.unwrap().1;
    let staff = a.user_with(&["crm:view_all", "crm:delete_messages"]).await;
    let r = a
        .post(
            &staff.token,
            "/api/v1/crm/inbox/threads/bulk",
            json!({"action": "delete", "ids": [tid.to_string()]}),
        )
        .await;
    assert_eq!(r.status, 403);
    assert!(thread_row(tid).await.is_some());
}

#[tokio::test]
async fn test_bulk_delete_removes_every_ticked_conversation_in_one_server_session() {
    let a = app();
    let _g = a.serial().await;
    let p = proxy(false).await;
    let (_user, tids) = two_conversations(|_| p.port).await;
    let before = p.connections.load(Ordering::SeqCst);
    let admin = sysadmin().await;
    let r = a
        .post(
            &admin.token,
            "/api/v1/crm/inbox/threads/bulk",
            json!({"action": "delete", "ids": tids}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        (r.json()["affected"].clone(), r.json()["deleted"].clone()),
        (json!(2), json!(2))
    );
    assert_eq!(
        p.connections.load(Ordering::SeqCst) - before,
        1,
        "one mail-server session for the whole batch"
    );
    for t in &tids {
        assert!(thread_row(t.parse().unwrap()).await.is_none());
    }
}

#[tokio::test]
async fn test_bulk_read_unread_and_status() {
    let a = app();
    let _g = a.serial().await;
    let (user, asked, _) = conversation(|_| GM_IMAP).await;
    let tid = msg_row(&asked).await.unwrap().1;
    let person = a.user_with(&["crm:view_all"]).await;
    let bulk = "/api/v1/crm/inbox/threads/bulk";
    let r = a
        .post(
            &person.token,
            bulk,
            json!({"action": "unread", "ids": [tid.to_string()]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(thread_row(tid).await.unwrap().0, Some(false));
    assert!(!msg_row(&asked).await.unwrap().2);
    assert!(wait_for(|| async { server_flags(&user, "INBOX").await.values().all(|s| !*s) }).await);
    a.post(
        &person.token,
        bulk,
        json!({"action": "read", "ids": [tid.to_string()]}),
    )
    .await;
    assert_eq!(thread_row(tid).await.unwrap().0, Some(true));
    assert!(msg_row(&asked).await.unwrap().2);
    assert!(
        wait_for(|| async { server_flags(&user, "INBOX").await.values().all(|s| *s) }).await,
        "read/unread mirrored on the mail server"
    );
    let r = a
        .post(
            &person.token,
            bulk,
            json!({"action": "status", "status": "resolved", "ids": [tid.to_string()]}),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        thread_row(tid).await.unwrap().1.as_deref(),
        Some("resolved")
    );
    let bad = a
        .post(
            &person.token,
            bulk,
            json!({"action": "status", "status": "archived", "ids": [tid.to_string()]}),
        )
        .await;
    assert_eq!(bad.status, 400);
}

#[tokio::test]
async fn test_assigning_notifies_the_assignee_once() {
    let a = app();
    let _g = a.serial().await;
    let (_user, tids) = two_conversations(|_| GM_IMAP).await;
    let boss = a.user_with(&["crm:view_all"]).await;
    let bob = a.user_with(&["crm:view_all"]).await;
    let bulk = "/api/v1/crm/inbox/threads/bulk";
    let r = a
        .post(
            &boss.token,
            bulk,
            json!({"action": "assign", "assignee_id": bob.id.to_string(), "ids": tids}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["affected"].clone()),
        (200, json!(2)),
        "{}",
        r.text()
    );
    let mine = a
        .post(
            &boss.token,
            bulk,
            json!({"action": "assign", "assignee_id": boss.id.to_string(), "ids": [tids[0]]}),
        )
        .await;
    assert_eq!(mine.status, 200);
    let listed = a
        .get(&bob.token, "/api/v1/crm/inbox/threads?assigned=me&box=all")
        .await
        .json();
    let ids: Vec<&str> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![tids[1].as_str()]);
    let mut db = a.db().await;
    let to_bob: Vec<String> = sqlx::query_scalar(
        "SELECT title FROM notifications WHERE user_id = $1 AND type = 'crm_assignment'",
    )
    .bind(bob.id)
    .fetch_all(&mut db)
    .await
    .unwrap();
    let to_boss: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND type = 'crm_assignment'",
    )
    .bind(boss.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert!(
        to_bob.len() == 1 && to_bob[0].contains('2'),
        "one notification for the whole batch"
    );
    assert_eq!(to_boss, 0, "assigning to yourself doesn't notify");
    let (by, at): (Option<Uuid>, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT assigned_by_id, assigned_at FROM crm_email_threads WHERE id = $1")
            .bind(tids[1].parse::<Uuid>().unwrap())
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(by, Some(boss.id));
    assert!(at.is_some());
}

#[tokio::test]
async fn test_assigning_to_someone_outside_the_company_is_refused() {
    let a = app();
    let _g = a.serial().await;
    let (_user, asked, _) = conversation(|_| GM_IMAP).await;
    let partner = a
        .make_user(support::UserSpec::new().portal("partner"))
        .await;
    let person = a.user_with(&["crm:view_all"]).await;
    let r = a
        .post(
            &person.token,
            "/api/v1/crm/inbox/threads/bulk",
            json!({"action": "assign", "assignee_id": partner.id.to_string(),
                   "ids": [msg_row(&asked).await.unwrap().1.to_string()]}),
        )
        .await;
    assert_eq!(r.status, 400);
}

#[tokio::test]
async fn test_all_matching_uses_the_list_filters_and_is_capped() {
    let a = app();
    let _g = a.serial().await;
    let tag = support::u("pytest-bulk");
    let mut ids = Vec::new();
    for i in 0..3 {
        ids.push(add_thread(&format!("{tag} {i}"), false).await);
    }
    let other = add_thread("unrelated", false).await;
    let person = a.user_with(&["crm:view_all"]).await;
    let bulk = "/api/v1/crm/inbox/threads/bulk";
    let r = a
        .post(&person.token, bulk, json!({"action": "status", "status": "resolved", "filter": {"search": tag, "box": "all"}}))
        .await;
    assert_eq!(
        (r.status, r.json()["affected"].clone()),
        (200, json!(3)),
        "{}",
        r.text()
    );
    granary_api::routers::crm_inbox::set_bulk_max(2);
    let capped = a
        .post(
            &person.token,
            bulk,
            json!({"action": "read", "filter": {"search": tag, "box": "all"}}),
        )
        .await;
    granary_api::routers::crm_inbox::set_bulk_max(granary_api::routers::crm_inbox::BULK_MAX);
    assert_eq!(capped.status, 400);
    assert_eq!(
        capped.json()["detail"],
        "More than 2 conversations match. Narrow the filter and try again."
    );
    let both = a
        .post(&person.token, bulk, json!({"action": "read", "ids": []}))
        .await;
    let neither = a.post(&person.token, bulk, json!({"action": "read"})).await;
    assert_eq!((both.status, neither.status), (200, 400));
    assert_eq!(thread_row(other).await.unwrap().1.as_deref(), Some("open"));
    for t in ids {
        assert_eq!(thread_row(t).await.unwrap().1.as_deref(), Some("resolved"));
    }
}

/// Security 26 Oct: the CRM inbox needs CRM view rights to read, crm:send_email to send or sync, and
/// crm:view_own sees only the threads of their own records.
#[tokio::test]
async fn test_the_inbox_needs_crm_rights_and_view_own_sees_only_its_threads() {
    let a = app();
    let nobody = a.user_with(&[]).await;
    let r = a.get(&nobody.token, "/api/v1/crm/inbox/threads").await;
    assert_eq!(r.status, 403, "{}", r.text());
    let r = a
        .post(&nobody.token, "/api/v1/crm/inbox/sync", json!({}))
        .await;
    assert_eq!(r.status, 403);
    // a thread linked to someone else's contact, and one linked to the reader's own
    let own = a.user_with(&["crm:view_own"]).await;
    let mut db = a.db().await;
    let thread_for = |owner: Option<Uuid>| {
        let (cid, tid) = (Uuid::new_v4(), Uuid::new_v4());
        (cid, tid, owner)
    };
    let (theirs_c, theirs_t, _) = thread_for(None);
    let (mine_c, mine_t, _) = thread_for(Some(own.id));
    for (cid, tid, owner) in [(theirs_c, theirs_t, None), (mine_c, mine_t, Some(own.id))] {
        sqlx::query(
            "INSERT INTO contacts (id, first_name, email, assigned_to_id, is_deleted, custom) \
             VALUES ($1, 'Perm', $2, $3, false, '{}')",
        )
        .bind(cid)
        .bind(format!("{cid}@perm.test"))
        .bind(owner)
        .execute(&mut db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO crm_email_threads (id, subject, status, is_read, message_count, linked_contact_id, \
             last_message_at) VALUES ($1, 'Perm test', 'open', true, 0, $2, now())",
        )
        .bind(tid)
        .bind(cid)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let r = a
        .get(
            &own.token,
            "/api/v1/crm/inbox/threads?search=Perm%20test&limit=50&box=all",
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let ids: Vec<String> = r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        ids.contains(&mine_t.to_string()) && !ids.contains(&theirs_t.to_string()),
        "{ids:?}"
    );
    let r = a
        .get(&own.token, &format!("/api/v1/crm/inbox/threads/{theirs_t}"))
        .await;
    assert_eq!(r.status, 404);
}
