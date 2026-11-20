//! Ports of backend/tests/test_mail_sync.py, test_delete_mail_chats.py (service level), test_nary_p0_links.py
//! (link_open_opportunity) and test_nary_p2_web3forms.py (Junk) for imap_sync / crm_inbox. Python's tests use
//! a FakeIMAP; these run against GreenMail (a real IMAP server), with a small proxy in front of it where a
//! test needs a server that refuses or lacks MOVE.
//!
//! The poller fetches with `BODY.PEEK[]`, so polling leaves unread mail unread on a real server too.

mod greenmail;
mod support;

use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

use granary_db::rows::crm_inbox::{CrmEmailMessage, CrmEmailThread};
use granary_services::crm_inbox;
use granary_services::email::{self, Capture, Seams};
use granary_services::imap_sync::{self, pyemail};

const MAILBOX: &str = "hello@granary.test";

fn mail(
    subject: &str,
    frm: &str,
    to: &str,
    msg_id: &str,
    in_reply_to: Option<&str>,
    when: chrono::DateTime<Utc>,
) -> Vec<u8> {
    let date = pyemail::utils::format_datetime(&pyemail::utils::PyDateTime {
        naive: when.naive_utc(),
        offset: Some(0),
    });
    let mut s = format!(
        "Message-ID: {msg_id}\r\nFrom: {frm}\r\nTo: {to}\r\nSubject: {subject}\r\nDate: {date}\r\n"
    );
    if let Some(r) = in_reply_to {
        s.push_str(&format!("In-Reply-To: {r}\r\nReferences: {r}\r\n"));
    }
    s.push_str(&format!("Content-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: 7bit\r\nMIME-Version: 1.0\r\n\r\nBody of {subject}\r\n"));
    s.into_bytes()
}

fn mid(domain: &str) -> String {
    format!("<{}@{domain}>", Uuid::new_v4().simple())
}

async fn setup(tag: &str) -> (PgPool, String) {
    let pool = support::fresh_db(&format!("s5_ms_{tag}")).await;
    sqlx::query(
        "UPDATE system_settings SET value = 'false' WHERE key = 'nary.email.intake_enabled'",
    )
    .execute(&pool)
    .await
    .unwrap();
    crm_inbox::set_session_pool(pool.clone());
    greenmail::install_nary();
    greenmail::ensure().await;
    let user = greenmail::new_user(tag);
    greenmail::configure(&pool, &user).await;
    (pool, user)
}

async fn put(user: &str, mbox: &str, raw: &[u8], seen: bool) {
    let mut i = greenmail::login(user).await;
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
    let mut i = greenmail::login(user).await;
    let _ = i.create(&imap_sync::quote_mailbox(mbox)).await;
    let _ = i.logout().await;
}

/// UID -> seen in a mailbox, read with BODY.PEEK-free FLAGS only.
async fn server_flags(user: &str, mbox: &str) -> std::collections::BTreeMap<i64, bool> {
    let mut i = greenmail::login(user).await;
    imap_sync::select(&mut i, mbox).await.unwrap();
    let search = i.uid_search(&["ALL"]).await.unwrap();
    let uids = imap_sync::parse_uids(&search.lines);
    let out = imap_sync::fetch_seen(&mut i, &uids).await;
    let _ = i.logout().await;
    out
}

async fn message(pool: &PgPool, msg_id: &str) -> Option<CrmEmailMessage> {
    sqlx::query_as("SELECT * FROM crm_email_messages WHERE message_id_header = $1")
        .bind(msg_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn thread(pool: &PgPool, id: Uuid) -> Option<CrmEmailThread> {
    sqlx::query_as("SELECT * FROM crm_email_threads WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

// ─── Folder names ───────────────────────────────────────────────────────────

#[test]
fn test_the_sent_folder_is_found_whatever_the_server_calls_it() {
    for (listing, expected) in [
        (
            &b"(\\HasNoChildren \\Sent) \"/\" \"Sent Items\""[..],
            "Sent Items",
        ),
        (&b"(\\HasNoChildren) \".\" INBOX.Sent"[..], "INBOX.Sent"),
        (
            &b"(\\HasNoChildren) \"/\" \"[Gmail]/Sent Mail\""[..],
            "[Gmail]/Sent Mail",
        ),
        (
            &b"(\\HasNoChildren) \"/\" \"Sent Messages\""[..],
            "Sent Messages",
        ),
    ] {
        let folders = imap_sync::folders_from_list(&[listing.to_vec()]);
        assert_eq!(folders.get("SENT"), Some(expected));
    }
}

#[tokio::test]
async fn test_the_sent_folder_is_found_on_a_real_server() {
    let _g = greenmail::serial().await;
    greenmail::ensure().await;
    let user = greenmail::new_user("folders");
    create(&user, "INBOX.Sent").await; // GreenMail's delimiter is "."
    create(&user, "Deleted Items").await;
    let mut i = greenmail::login(&user).await;
    let folders = imap_sync::resolve_folders(&mut i).await;
    assert_eq!(folders.get("INBOX"), Some("INBOX"));
    assert_eq!(folders.get("SENT"), Some("INBOX.Sent"));
    assert_eq!(folders.get("TRASH"), Some("Deleted Items"));
}

#[test]
fn test_mailbox_names_with_spaces_are_quoted() {
    assert_eq!(imap_sync::quote_mailbox("Sent Items"), "\"Sent Items\"");
    assert_eq!(imap_sync::quote_mailbox("INBOX.Sent"), "INBOX.Sent");
    assert_eq!(imap_sync::_uid_set(&[7, 1, 2, 3, 9, 10]), "1:3,7,9:10");
}

// ─── CRM inbox ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_read_status_and_sent_mail_come_from_the_server() {
    let _g = greenmail::serial().await;
    let (pool, user) = setup("read").await;
    let now = Utc::now();
    let (asked, unread, reply) = (
        mid("customer.test"),
        mid("customer.test"),
        mid("granary.test"),
    );
    create(&user, "Sent Items").await;
    put(
        &user,
        "INBOX",
        &mail(
            "Quote please",
            "ann@customer.test",
            MAILBOX,
            &asked,
            None,
            now - Duration::hours(2),
        ),
        true,
    )
    .await;
    put(
        &user,
        "INBOX",
        &mail(
            "Another question",
            "bob@customer.test",
            MAILBOX,
            &unread,
            None,
            now,
        ),
        false,
    )
    .await;
    put(
        &user,
        "Sent Items",
        &mail(
            "Re: Quote please",
            MAILBOX,
            "ann@customer.test",
            &reply,
            Some(&asked),
            now - Duration::hours(1),
        ),
        true,
    )
    .await;
    assert_eq!(crm_inbox::poll_crm_inbox(true, false).await.unwrap(), 3);

    let first = message(&pool, &asked).await.unwrap();
    let second = message(&pool, &unread).await.unwrap();
    let sent = message(&pool, &reply).await.unwrap();
    assert!(first.is_read && !second.is_read);
    assert_eq!(
        thread(&pool, first.thread_id).await.unwrap().is_read,
        Some(true),
        "read in Thunderbird -> read here"
    );
    assert_eq!(
        thread(&pool, second.thread_id).await.unwrap().is_read,
        Some(false)
    );
    assert_eq!(sent.direction, "outbound");
    assert_eq!(sent.imap_folder.as_deref(), Some("SENT"));
    assert_eq!(
        sent.thread_id, first.thread_id,
        "a reply sent from Thunderbird joins its conversation"
    );

    // Bob's mail is then read in webmail: the next poll picks that up.
    let mut i = greenmail::login(&user).await;
    i.select("INBOX").await.unwrap();
    imap_sync::set_seen(&mut i, &[second.imap_uid.unwrap() as i64], true)
        .await
        .unwrap();
    let _ = i.logout().await;
    crm_inbox::poll_crm_inbox(true, false).await.unwrap();
    assert_eq!(
        thread(&pool, second.thread_id).await.unwrap().is_read,
        Some(true)
    );
}

#[tokio::test]
async fn test_set_server_seen_stores_the_flag() {
    let _g = greenmail::serial().await;
    let (_pool, user) = setup("seen").await;
    put(&user, "INBOX", &[b'x'; 60], false).await;
    assert_eq!(server_flags(&user, "INBOX").await.get(&1), Some(&false));
    crm_inbox::set_server_seen(&[1], true).await;
    assert_eq!(server_flags(&user, "INBOX").await.get(&1), Some(&true));
    crm_inbox::set_server_seen(&[1], false).await;
    assert_eq!(server_flags(&user, "INBOX").await.get(&1), Some(&false));
}

#[tokio::test]
async fn test_an_erp_reply_threads_and_is_saved_to_sent_once() {
    let _g = greenmail::serial().await;
    let (pool, user) = setup("reply").await;
    let asked = mid("customer.test");
    create(&user, "Sent Items").await;
    put(
        &user,
        "INBOX",
        &mail(
            "Need a quote",
            "dan@customer.test",
            MAILBOX,
            &asked,
            None,
            Utc::now(),
        ),
        true,
    )
    .await;
    crm_inbox::poll_crm_inbox(true, false).await.unwrap();
    let original = message(&pool, &asked).await.unwrap();

    let sent_with: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sw = sent_with.clone();
    let asked2 = asked.clone();
    email::set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(move |call: email::SendCall| {
            let sw = sw.clone();
            let asked = asked2.clone();
            Box::pin(async move {
                let headers = call.headers.clone().unwrap_or_default();
                *sw.lock().unwrap() = headers.clone();
                let id = headers
                    .iter()
                    .find(|(k, _)| k == "Message-ID")
                    .unwrap()
                    .1
                    .clone();
                let raw = mail(
                    &call.subject,
                    MAILBOX,
                    &call.to[0],
                    &id,
                    Some(&asked),
                    Utc::now(),
                );
                Ok((
                    true,
                    Some(Capture {
                        raw: Some(raw),
                        message_id: Some(id),
                    }),
                ))
            })
        })),
        ..Default::default()
    }));
    let mut tx = pool.begin().await.unwrap();
    let out = crm_inbox::send_crm_reply(
        &mut tx,
        original.thread_id,
        &original,
        "<p>Here it is</p>",
        "Here it is",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    email::set_global_seams(None);
    let reply_id = out.message_id_header.clone();
    let headers = sent_with.lock().unwrap().clone();
    let h = |k: &str| {
        headers
            .iter()
            .find(|(x, _)| x == k)
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_eq!(h("In-Reply-To"), asked);
    assert!(h("References").contains(&asked));
    assert_eq!(
        h("Message-ID"),
        reply_id,
        "stored and sent Message-ID are the same"
    );
    // saved to the server's Sent folder, read
    let flags = server_flags(&user, "Sent Items").await;
    assert_eq!(flags.len(), 1);
    assert_eq!(flags.get(&1), Some(&true));

    // The next poll finds that copy in Sent and must not store it a second time.
    crm_inbox::poll_crm_inbox(true, false).await.unwrap();
    let copies: Vec<CrmEmailMessage> =
        sqlx::query_as("SELECT * FROM crm_email_messages WHERE message_id_header = $1")
            .bind(&reply_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].imap_folder.as_deref(), Some("SENT"));
    assert_eq!(copies[0].imap_uid, Some(1));
}

/// The UIDs of the message bodies fetched through `proxy` (FakeIMAP's `fetched`).
fn fetched(p: &Proxy) -> Vec<i64> {
    p.commands
        .lock()
        .unwrap()
        .iter()
        .filter(|c| {
            let u = c.to_uppercase();
            u.starts_with("UID FETCH") && (u.contains("BODY") || u.contains("RFC822"))
        })
        .map(|c| c.split_whitespace().nth(2).unwrap().parse().unwrap())
        .collect()
}

#[tokio::test]
async fn test_polling_leaves_unread_mail_unread_on_the_server() {
    let _g = greenmail::serial().await;
    let (pool, user) = setup("peek").await;
    let msg_id = mid("customer.test");
    create(&user, "Sent Items").await;
    put(
        &user,
        "INBOX",
        &mail(
            "Still unread",
            "fay@customer.test",
            MAILBOX,
            &msg_id,
            None,
            Utc::now(),
        ),
        false,
    )
    .await;
    let p = proxy(&[], false).await;
    point_at(&pool, p.port).await;
    crm_inbox::poll_crm_inbox(true, false).await.unwrap();
    crm_inbox::poll_crm_inbox(true, false).await.unwrap(); // the read-status sync must not flip it either
    assert_eq!(fetched(&p), vec![1]);
    assert_eq!(
        server_flags(&user, "INBOX").await.get(&1),
        Some(&false),
        "fetching the message must not set \\Seen on the server"
    );
    let message = message(&pool, &msg_id).await.unwrap();
    assert!(!message.is_read);
    assert_eq!(
        thread(&pool, message.thread_id).await.unwrap().is_read,
        Some(false)
    );
}

fn bare_pdf(msg_id: &str) -> Vec<u8> {
    let date = pyemail::utils::format_datetime(&pyemail::utils::PyDateTime {
        naive: Utc::now().naive_utc(),
        offset: Some(0),
    });
    // What MimeMessage.set_content(b"...", maintype="application", subtype="pdf", filename="scan.pdf") writes.
    format!(
        "Message-ID: {msg_id}\r\nFrom: scanner@office.test\r\nTo: {MAILBOX}\r\nSubject: Scan\r\nDate: {date}\r\n\
         Content-Type: application/pdf\r\nContent-Transfer-Encoding: base64\r\n\
         Content-Disposition: attachment; filename=\"scan.pdf\"\r\nMIME-Version: 1.0\r\n\r\n\
         JVBERi0xLjQgc2Nhbm5lZCBwYWdl\r\n"
    )
    .into_bytes()
}

#[tokio::test]
async fn test_a_bare_pdf_is_stored_with_its_file_and_later_mail_still_arrives() {
    let _g = greenmail::serial().await;
    let (pool, user) = setup("bare_pdf").await;
    let (scan, later) = (mid("office.test"), mid("customer.test"));
    create(&user, "Sent Items").await;
    put(&user, "INBOX", &bare_pdf(&scan), false).await;
    put(
        &user,
        "INBOX",
        &mail(
            "After the scan",
            "gus@customer.test",
            MAILBOX,
            &later,
            None,
            Utc::now(),
        ),
        false,
    )
    .await;
    assert_eq!(crm_inbox::poll_crm_inbox(true, false).await.unwrap(), 2);
    let stored = message(&pool, &scan).await.unwrap();
    assert_eq!(stored.body_text.as_deref(), Some(""));
    assert_eq!(stored.subject.as_deref(), Some("Scan"));
    let files: Vec<(String, Option<String>, Vec<u8>)> = sqlx::query_as(
        "SELECT filename, content_type, data FROM crm_email_attachments WHERE message_id = $1",
    )
    .bind(stored.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        files,
        vec![(
            "scan.pdf".to_string(),
            Some("application/pdf".to_string()),
            b"%PDF-1.4 scanned page".to_vec()
        )]
    );
    assert_eq!(
        message(&pool, &later).await.unwrap().subject.as_deref(),
        Some("After the scan")
    );
}

#[tokio::test]
async fn test_an_overlong_message_id_on_a_linked_thread_is_stored_and_threads() {
    let _g = greenmail::serial().await;
    let (pool, user) = setup("long_id").await;
    let addr = format!("{}@linked.test", &Uuid::new_v4().simple().to_string()[..10]);
    let long_id = format!(
        "<{}.{}@mailer.test>",
        "x".repeat(400),
        Uuid::new_v4().simple()
    );
    let reply = mid("linked.test");
    let contact = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, email, is_deleted) VALUES ($1, 'Hana', $2, false)",
    )
    .bind(contact)
    .bind(&addr)
    .execute(&pool)
    .await
    .unwrap();
    create(&user, "Sent Items").await;
    put(
        &user,
        "INBOX",
        &mail("Long id", &addr, MAILBOX, &long_id, None, Utc::now()),
        false,
    )
    .await;
    put(
        &user,
        "INBOX",
        &mail(
            "Re: Long id",
            &addr,
            MAILBOX,
            &reply,
            Some(&long_id),
            Utc::now(),
        ),
        false,
    )
    .await;
    assert_eq!(crm_inbox::poll_crm_inbox(true, false).await.unwrap(), 2);
    let first = message(&pool, &long_id).await.unwrap();
    let second = message(&pool, &reply).await.unwrap();
    assert_eq!(
        second.thread_id, first.thread_id,
        "the reply finds its conversation by the long id"
    );
    assert_eq!(second.in_reply_to.as_deref(), Some(long_id.as_str()));
    let mut ids: Vec<String> =
        sqlx::query_scalar("SELECT channel_message_id FROM activities WHERE contact_id = $1")
            .bind(contact)
            .fetch_all(&pool)
            .await
            .unwrap();
    ids.sort();
    let mut want = vec![
        crm_inbox::_fit_id(Some(&long_id), crm_inbox::CHANNEL_MESSAGE_ID_LEN).unwrap(),
        reply.clone(),
    ];
    want.sort();
    assert_eq!(ids, want);
    assert!(ids.iter().all(|i| i.chars().count() <= 255));
}

#[test]
fn test_reply_and_forward_prefixes_leave_the_subject() {
    for (subject, clean) in [
        ("Re: x", "x"),
        ("RE: x", "x"),
        ("re:x", "x"),
        ("Fwd: x", "x"),
        ("FWD: x", "x"),
        ("Fw: x", "x"),
        ("AW: x", "x"),
        ("Re: Fwd: x", "x"),
        ("Report for May", "Report for May"),
        ("Fwd", "Fwd"),
        ("Re:", "(No Subject)"),
    ] {
        assert_eq!(crm_inbox::_clean_subject(subject), clean, "{subject}");
    }
}

async fn insert_contact(
    pool: &PgPool,
    first: &str,
    email: &str,
    created: chrono::DateTime<Utc>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, email, is_deleted, created_at, updated_at) VALUES ($1, $2, $3, false, $4, $4)",
    )
    .bind(id)
    .bind(first)
    .bind(email)
    .bind(created)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_a_sender_links_to_the_contact_with_exactly_that_address() {
    let pool = support::fresh_db("s5_ms_exact").await;
    let tag = &Uuid::new_v4().simple().to_string()[..8];
    let now = Utc::now();
    let exact = insert_contact(&pool, "Ivy", &format!("ivy_{tag}@co.test"), now).await;
    // "_" in ILIKE matches the "x"
    insert_contact(&pool, "Ivo", &format!("ivyx{tag}@co.test"), now).await;
    let older = insert_contact(
        &pool,
        "Jo",
        &format!("JO.{tag}@co.test"),
        now - Duration::days(3),
    )
    .await;
    insert_contact(&pool, "Jo", &format!("jo.{tag}@co.test"), now).await;

    let mut conn = pool.acquire().await.unwrap();
    let mut thread = crm_inbox::new_thread("x".into());
    crm_inbox::_auto_link(&mut conn, &mut thread, &format!("IVY_{tag}@co.test"))
        .await
        .unwrap();
    assert_eq!(thread.linked_contact_id, Some(exact));
    let mut thread = crm_inbox::new_thread("x".into());
    crm_inbox::_auto_link(&mut conn, &mut thread, &format!("jo.{tag}@co.test"))
        .await
        .unwrap();
    assert_eq!(
        thread.linked_contact_id,
        Some(older),
        "the same address on two contacts: the oldest one"
    );
}

/// Python makes `message_from_bytes` raise for one message; the email package cannot be patched here, so
/// the message fails where Postgres refuses it (a NUL in its text), which is skipped the same way.
#[tokio::test]
async fn test_a_message_that_cannot_be_parsed_is_skipped_once_not_every_poll() {
    let _g = greenmail::serial().await;
    let (pool, user) = setup("skip_once").await;
    let good = mid("customer.test");
    create(&user, "Sent Items").await;
    put(
        &user,
        "INBOX",
        &mail(
            "Fine",
            "kim@customer.test",
            MAILBOX,
            &good,
            None,
            Utc::now(),
        ),
        false,
    )
    .await;
    let mut bad = mail(
        "Broken",
        "kim@customer.test",
        MAILBOX,
        &mid("customer.test"),
        None,
        Utc::now(),
    );
    bad.extend_from_slice(b"bad\x00byte\r\n");
    put(&user, "INBOX", &bad, false).await;
    let p = proxy(&[], false).await;
    point_at(&pool, p.port).await;
    assert_eq!(crm_inbox::poll_crm_inbox(true, false).await.unwrap(), 1);
    crm_inbox::poll_crm_inbox(true, false).await.unwrap();
    assert_eq!(
        fetched(&p),
        vec![1, 2],
        "the bad message is not fetched again"
    );
    let mark: Option<String> = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'crm_inbox.skipped_uid.inbox'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(mark.as_deref(), Some("2"));
    assert!(message(&pool, &good).await.is_some());
}

#[tokio::test]
async fn test_two_organisations_on_the_sender_domain_pick_the_oldest() {
    let pool = support::fresh_db("s5_ms_two_orgs").await;
    let dom = format!("pytest-{}.test", &Uuid::new_v4().simple().to_string()[..8]);
    let (first, newer) = (Uuid::new_v4(), Uuid::new_v4());
    let now = Utc::now();
    for (id, name, site, at) in [
        (
            first,
            "Old Ltd",
            format!("https://{dom}"),
            now - Duration::seconds(1),
        ),
        (newer, "New Ltd", format!("https://www.{dom}"), now),
    ] {
        sqlx::query(
            "INSERT INTO organizations (id, name, website, is_deleted, created_at, updated_at) VALUES ($1, $2, $3, false, $4, $4)",
        )
        .bind(id)
        .bind(name)
        .bind(site)
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();
    }
    let mut conn = pool.acquire().await.unwrap();
    let mut thread = crm_inbox::new_thread("Hello".into());
    thread.last_sender_email = Some(format!("bob@{dom}"));
    crm_inbox::_auto_link(&mut conn, &mut thread, &format!("bob@{dom}"))
        .await
        .unwrap();
    assert_eq!(thread.linked_org_id, Some(first));
}

#[test]
fn test_inline_cid_images_are_embedded_and_oversized_ones_left_alone() {
    // The message test_mail_sync.py builds (recorded by gen_crm_inbox.py; the "big" image there is 3000 bytes,
    // so here the cap is exercised by an over-cap copy of it).
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../compat-tests/golden/crm_inbox/messages/inline_cid_test.eml");
    let raw = std::fs::read(path).unwrap();
    let parsed = pyemail::message_from_bytes(&raw).unwrap();
    let (_, html, _) = crm_inbox::_extract_body_and_attachments(&parsed, &raw).unwrap();
    assert!(html.contains("src=\"data:image/png;base64,") && !html.contains("cid:_Foxmail"));
    assert!(html.contains("src=\"cid:missing@x\""));
}

// ─── Deleting ───────────────────────────────────────────────────────────────

/// A proxy in front of GreenMail: hides capabilities and refuses UID MOVE / UID STORE when asked.
struct Proxy {
    port: u16,
    commands: Arc<Mutex<Vec<String>>>,
}

async fn proxy(hide: &'static [&'static str], refuse: bool) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let log = commands.clone();
    tokio::spawn(async move {
        loop {
            let (client, _) = match listener.accept().await {
                Ok(c) => c,
                Err(_) => return,
            };
            let log = log.clone();
            tokio::spawn(async move {
                let server = TcpStream::connect((greenmail::HOST, greenmail::IMAP_PORT))
                    .await
                    .unwrap();
                let (cr, cw) = client.into_split();
                let (sr, mut sw) = server.into_split();
                let cw2 = Arc::new(tokio::sync::Mutex::new(cw));
                let cw_s = cw2.clone();
                // server -> client: drop hidden capabilities
                tokio::spawn(async move {
                    let mut sr = BufReader::new(sr);
                    let mut buf = Vec::new();
                    loop {
                        buf.clear();
                        if sr.read_until(b'\n', &mut buf).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let mut line = buf.clone();
                        if line.starts_with(b"* CAPABILITY") {
                            let s = String::from_utf8_lossy(&line).to_string();
                            let kept: Vec<&str> = s
                                .trim_end()
                                .split(' ')
                                .filter(|w| !hide.contains(w))
                                .collect();
                            line = format!("{}\r\n", kept.join(" ")).into_bytes();
                        }
                        if cw_s.lock().await.write_all(&line).await.is_err() {
                            return;
                        }
                    }
                });
                let mut cr = BufReader::new(cr);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    if cr.read_until(b'\n', &mut buf).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let s = String::from_utf8_lossy(&buf).to_string();
                    let mut parts = s.trim_end().splitn(2, ' ');
                    let tag = parts.next().unwrap_or("").to_string();
                    let rest = parts.next().unwrap_or("").to_string();
                    log.lock().unwrap().push(rest.clone());
                    if refuse && (rest.starts_with("UID MOVE") || rest.starts_with("UID STORE")) {
                        let mut w = cw2.lock().await;
                        let _ = w
                            .write_all(format!("{tag} NO refused\r\n").as_bytes())
                            .await;
                        continue;
                    }
                    if sw.write_all(&buf).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    Proxy { port, commands }
}

async fn point_at(pool: &PgPool, port: u16) {
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'crm_imap_port'")
        .bind(port.to_string())
        .execute(pool)
        .await
        .unwrap();
}

/// A customer email and our reply, synced into one CRM conversation.
async fn conversation(tag: &str) -> (PgPool, String, String, String) {
    let (pool, user) = setup(tag).await;
    let (asked, reply) = (mid("c.test"), mid("granary.test"));
    create(&user, "Sent").await;
    create(&user, "Trash").await;
    put(
        &user,
        "INBOX",
        &mail("Price?", "ann@c.test", MAILBOX, &asked, None, Utc::now()),
        true,
    )
    .await;
    put(
        &user,
        "Sent",
        &mail(
            "Re: Price?",
            MAILBOX,
            "ann@c.test",
            &reply,
            Some(&asked),
            Utc::now(),
        ),
        true,
    )
    .await;
    crm_inbox::poll_crm_inbox(true, false).await.unwrap();
    (pool, user, asked, reply)
}

async fn delete(
    pool: &PgPool,
    msgs: Vec<CrmEmailMessage>,
) -> Result<serde_json::Value, crm_inbox::InboxError> {
    let mut tx = pool.begin().await.unwrap();
    let r = crm_inbox::delete_messages(&mut tx, &msgs).await;
    if r.is_ok() {
        tx.commit().await.unwrap();
    }
    r
}

#[tokio::test]
async fn test_deleting_one_email_moves_it_to_trash_and_keeps_the_conversation() {
    let _g = greenmail::serial().await;
    let (pool, user, asked, reply) = conversation("del_one").await;
    let tid = message(&pool, &asked).await.unwrap().thread_id;
    let out = delete(&pool, vec![message(&pool, &reply).await.unwrap()])
        .await
        .unwrap();
    assert_eq!(
        out,
        serde_json::json!({"deleted": 1, "threads_removed": 0, "server": "trash"})
    );
    assert!(
        server_flags(&user, "Sent").await.is_empty(),
        "moved on the server, from the folder it lives in"
    );
    assert_eq!(server_flags(&user, "Trash").await.len(), 1);
    assert!(message(&pool, &reply).await.is_none() && message(&pool, &asked).await.is_some());
    assert_eq!(thread(&pool, tid).await.unwrap().message_count, Some(1));
}

#[tokio::test]
async fn test_deleting_a_conversation_removes_every_email() {
    let _g = greenmail::serial().await;
    let (pool, user, asked, reply) = conversation("del_all").await;
    let tid = message(&pool, &asked).await.unwrap().thread_id;
    let msgs: Vec<CrmEmailMessage> =
        sqlx::query_as("SELECT * FROM crm_email_messages WHERE thread_id = $1")
            .bind(tid)
            .fetch_all(&pool)
            .await
            .unwrap();
    let out = delete(&pool, msgs).await.unwrap();
    assert_eq!(out["deleted"], 2);
    assert_eq!(out["threads_removed"], 1);
    assert!(
        server_flags(&user, "INBOX").await.is_empty()
            && server_flags(&user, "Sent").await.is_empty()
    );
    assert_eq!(server_flags(&user, "Trash").await.len(), 2);
    assert!(thread(&pool, tid).await.is_none());
    assert!(message(&pool, &asked).await.is_none() && message(&pool, &reply).await.is_none());
}

#[tokio::test]
async fn test_an_email_with_attachments_can_be_deleted() {
    let _g = greenmail::serial().await;
    let (pool, _user, asked, _) = conversation("del_att").await;
    let msg = message(&pool, &asked).await.unwrap();
    sqlx::query(
        "INSERT INTO crm_email_attachments (id, message_id, filename, content_type, size_bytes, data) \
         VALUES (gen_random_uuid(), $1, 'image001.png', 'image/png', 3, 'png')",
    )
    .bind(msg.id)
    .execute(&pool)
    .await
    .unwrap();
    let msgs: Vec<CrmEmailMessage> =
        sqlx::query_as("SELECT * FROM crm_email_messages WHERE thread_id = $1")
            .bind(msg.thread_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    delete(&pool, msgs).await.unwrap();
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM crm_email_attachments WHERE message_id = $1")
            .bind(msg.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(left, 0);
    assert!(message(&pool, &asked).await.is_none());
}

#[tokio::test]
async fn test_a_refusing_server_changes_nothing() {
    let _g = greenmail::serial().await;
    let (pool, _user, asked, _) = conversation("del_refuse").await;
    let p = proxy(&[], true).await;
    point_at(&pool, p.port).await;
    let err = delete(&pool, vec![message(&pool, &asked).await.unwrap()])
        .await
        .unwrap_err();
    match err {
        crm_inbox::InboxError::Delete(m) => assert!(m.contains("refused"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(
        message(&pool, &asked).await.is_some(),
        "nothing deleted in the ERP when the server says no"
    );
}

#[tokio::test]
async fn test_without_move_support_it_copies_then_expunges() {
    let _g = greenmail::serial().await;
    let (pool, user) = setup("del_nomove").await;
    let msg_id = mid("c.test");
    create(&user, "Trash").await;
    put(
        &user,
        "INBOX",
        &mail("Old server", "x@c.test", MAILBOX, &msg_id, None, Utc::now()),
        true,
    )
    .await;
    crm_inbox::poll_crm_inbox(true, false).await.unwrap();
    let p = proxy(&["MOVE"], false).await;
    point_at(&pool, p.port).await;
    delete(&pool, vec![message(&pool, &msg_id).await.unwrap()])
        .await
        .unwrap();
    let cmds = p.commands.lock().unwrap().clone();
    assert!(cmds.iter().any(|c| c == "UID COPY 1 Trash"), "{cmds:?}");
    assert!(!cmds.iter().any(|c| c.starts_with("UID MOVE")));
    assert!(cmds.iter().any(|c| c == "UID EXPUNGE 1"), "{cmds:?}");
    assert!(server_flags(&user, "INBOX").await.is_empty());
    assert_eq!(server_flags(&user, "Trash").await.len(), 1);
}

// ─── Links and Junk ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_an_inbox_thread_picks_up_the_contacts_open_lead_and_deal() {
    let _g = greenmail::serial().await;
    let pool = support::fresh_db("s5_ms_links").await;
    let contact = Uuid::new_v4();
    sqlx::query("INSERT INTO contacts (id, first_name, is_deleted, custom) VALUES ($1, 'Sarah', false, '{}')")
        .bind(contact)
        .execute(&pool)
        .await
        .unwrap();
    let lead = |status: &'static str, when: &'static str| {
        let pool = pool.clone();
        async move {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO leads (id, title, source, status, contact_id, is_deleted, custom, created_at) VALUES ($1, $2, 'email', $2, $3, false, '{}', $4::timestamptz)")
                .bind(id).bind(status).bind(contact).bind(when).execute(&pool).await.unwrap();
            id
        }
    };
    let deal = |stage: &'static str, when: &'static str| {
        let pool = pool.clone();
        async move {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO deals (id, title, stage, contact_id, is_deleted, custom, created_at) VALUES ($1, $2, $2, $3, false, '{}', $4::timestamptz)")
                .bind(id).bind(stage).bind(contact).bind(when).execute(&pool).await.unwrap();
            id
        }
    };
    lead("promoted", "2026-01-03").await;
    let open_lead = lead("qualifying", "2026-01-02").await;
    deal("lost", "2026-01-03").await;
    let open_deal = deal("new", "2026-01-02").await;
    let tid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO crm_email_threads (id, subject, linked_contact_id) VALUES ($1, 'Pilot', $2)",
    )
    .bind(tid)
    .bind(contact)
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let mut t =
        sqlx::query_as::<_, CrmEmailThread>("SELECT * FROM crm_email_threads WHERE id = $1")
            .bind(tid)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    crm_inbox::link_open_opportunity(&mut tx, &mut t)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let t = thread(&pool, tid).await.unwrap();
    assert_eq!(
        (t.linked_lead_id, t.linked_deal_id),
        (Some(open_lead), Some(open_deal))
    );
}

#[tokio::test]
async fn test_junk_only_lets_form_emails_through() {
    let _g = greenmail::serial().await;
    let pool = support::fresh_db("s5_ms_junk").await;
    greenmail::install_nary();
    let other =
        b"From: promo@shop.example\r\nSubject: SALE\r\nMessage-ID: <junk-1@shop>\r\n\r\nbuy now";
    let mut tx = pool.begin().await.unwrap();
    assert!(
        !crm_inbox::_ingest(&mut tx, other, 5, "SPAM", false, "hello@example.com")
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM crm_email_messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn test_junk_lets_a_form_email_through() {
    let _g = greenmail::serial().await;
    let pool = support::fresh_db("s5_ms_junk_form").await;
    let raw = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/web3forms/web3forms_laptops.eml"),
    )
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert!(
        crm_inbox::_ingest(&mut tx, &raw, 6, "SPAM", false, "hello@example.com")
            .await
            .unwrap()
    );
}

// ─── R&D 26 Oct 2026: several CRM inboxes ──────────────────────────────────────

/// A second CRM inbox (sales@) on its own GreenMail user, active, connected a day ago.
async fn add_sales_inbox(pool: &PgPool, user: &str) -> Uuid {
    let id = Uuid::new_v4();
    let password = granary_services::email_crypto::encrypt("x");
    let port = greenmail::IMAP_PORT.to_string();
    for (k, v) in [
        ("crm_sales_imap_host", greenmail::HOST),
        ("crm_sales_imap_port", port.as_str()),
        ("crm_sales_imap_user", user),
        ("crm_sales_imap_password", password.as_str()),
        ("crm_sales_imap_ssl", "false"),
        ("crm_sales_smtp_host", "smtp.test"),
        ("crm_sales_smtp_from_email", "sales@granary.test"),
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
    sqlx::query(
        "INSERT INTO crm_mailboxes (id, prefix, address, display_name, purpose, active, is_default, sort, \
         connected_at, backfill_days, backfill_done) \
         VALUES ($1, 'crm_sales', 'sales@granary.test', 'ACME Sales', 'sales', true, false, 1, now() - interval '1 day', \
         90, true)",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_two_inboxes_store_a_shared_email_once_and_history_quietly() {
    let _g = greenmail::serial().await;
    let (pool, hello) = setup("two_inboxes").await;
    let sales = greenmail::new_user("sales");
    let sales_id = add_sales_inbox(&pool, &sales).await;
    let now = Utc::now();
    // the customer wrote to hello@ and sales@ at once
    let shared = mid("globex.test");
    let raw = mail(
        "Tender documents",
        "Rachel Green <rachel@globex.test>",
        "hello@granary.test, sales@granary.test",
        &shared,
        None,
        now - Duration::minutes(10),
    );
    put(&hello, "INBOX", &raw, false).await;
    put(&sales, "INBOX", &raw, false).await;
    // mail only to sales@, live, and from before sales@ was connected (history)
    let live = mid("acme.test");
    put(
        &sales,
        "INBOX",
        &mail(
            "Quotation question",
            "buyer@acme.test",
            "sales@granary.test",
            &live,
            None,
            now - Duration::minutes(5),
        ),
        false,
    )
    .await;
    let old = mid("acme.test");
    put(
        &sales,
        "INBOX",
        &mail(
            "Old thread",
            "buyer@acme.test",
            "sales@granary.test",
            &old,
            None,
            now - Duration::days(10),
        ),
        false,
    )
    .await;
    // mail sales@ sent to hello@ is ours, not a customer's
    let internal = mid("granary.test");
    put(
        &hello,
        "INBOX",
        &mail(
            "Internal note",
            "sales@granary.test",
            "hello@granary.test",
            &internal,
            None,
            now - Duration::minutes(1),
        ),
        false,
    )
    .await;

    crm_inbox::poll_crm_inbox(true, false).await.unwrap();

    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM crm_email_messages WHERE message_id_header = $1")
            .bind(&shared)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 1, "one email, not two");
    let copies: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM crm_message_copies c JOIN crm_email_messages m ON m.id = c.message_id \
         WHERE m.message_id_header = $1 AND c.mailbox_id = $2",
    )
    .bind(&shared)
    .bind(sales_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(copies, 1, "linked to sales@ too");

    let row: (Option<Uuid>, String) = sqlx::query_as(
        "SELECT mailbox_id, origin FROM crm_email_messages WHERE message_id_header = $1",
    )
    .bind(&live)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (Some(sales_id), "live".to_string()));
    let thread_box: Option<Uuid> = sqlx::query_scalar(
        "SELECT t.mailbox_id FROM crm_email_threads t JOIN crm_email_messages m ON m.thread_id = t.id \
         WHERE m.message_id_header = $1",
    )
    .bind(&live)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        thread_box,
        Some(sales_id),
        "the conversation lives in sales@"
    );

    let quiet: (String, bool) = sqlx::query_as(
        "SELECT origin, is_read FROM crm_email_messages WHERE message_id_header = $1",
    )
    .bind(&old)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        quiet,
        ("backfill".to_string(), true),
        "history: no alerts, not unread"
    );

    let ours: String =
        sqlx::query_scalar("SELECT direction FROM crm_email_messages WHERE message_id_header = $1")
            .bind(&internal)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(ours, "outbound");

    // a reply in the sales@ thread goes from sales@ (the inbox the customer wrote to)
    let tid: Uuid =
        sqlx::query_scalar("SELECT thread_id FROM crm_email_messages WHERE message_id_header = $1")
            .bind(&live)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let from = granary_services::crm_mailboxes::for_reply(&mut conn, tid)
        .await
        .unwrap();
    assert_eq!(from.address, "sales@granary.test");

    // a second poll stores nothing again
    assert_eq!(crm_inbox::poll_crm_inbox(true, false).await.unwrap(), 0);
}

// ─── Security 27 Oct (#22a): a new inbox's history import runs to its end ─────

#[tokio::test]
async fn test_a_new_inbox_history_import_goes_past_a_batch_of_copies() {
    let _g = greenmail::serial().await;
    let (pool, _hello) = setup("backfill_resume").await;
    let sales = greenmail::new_user("sales");
    let sales_id = add_sales_inbox(&pool, &sales).await;
    sqlx::query("UPDATE crm_mailboxes SET backfill_done = false WHERE id = $1")
        .bind(sales_id)
        .execute(&pool)
        .await
        .unwrap();
    let now = Utc::now();
    // the oldest mail only sales@ has
    let old = mid("acme.test");
    put(
        &sales,
        "INBOX",
        &mail(
            "Old thread",
            "buyer@acme.test",
            "sales@granary.test",
            &old,
            None,
            now - Duration::days(10),
        ),
        false,
    )
    .await;
    // then two batches of mail hello@ already has (the customer wrote to both): copies, nothing new stored
    let mut tx = pool.begin().await.unwrap();
    for i in 0..(2 * crm_inbox::POLL_BATCH) {
        let raw = mail(
            &format!("Shared {i}"),
            "rachel@globex.test",
            "hello@granary.test, sales@granary.test",
            &mid("globex.test"),
            None,
            now - Duration::days(5),
        );
        assert!(crm_inbox::_ingest(
            &mut tx,
            &raw,
            i as i64 + 1,
            "INBOX",
            true,
            "hello@granary.test"
        )
        .await
        .unwrap());
        put(&sales, "INBOX", &raw, true).await;
    }
    tx.commit().await.unwrap();
    let done = |pool: PgPool| async move {
        sqlx::query_scalar::<_, bool>("SELECT backfill_done FROM crm_mailboxes WHERE id = $1")
            .bind(sales_id)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    // the first poll's history batch is all copies: the import goes on (it used to stop here)
    crm_inbox::poll_crm_inbox(true, false).await.unwrap();
    assert!(!done(pool.clone()).await, "history left: not done");
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM crm_email_messages WHERE message_id_header = $1")
            .bind(&old)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, 0);
    // the next poll reaches the oldest mail and ends the import
    crm_inbox::poll_crm_inbox(true, false).await.unwrap();
    let origin: String =
        sqlx::query_scalar("SELECT origin FROM crm_email_messages WHERE message_id_header = $1")
            .bind(&old)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(origin, "backfill");
    assert!(done(pool.clone()).await);
    let marks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM system_settings WHERE key LIKE 'crm_inbox.backfill_low.%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(marks, 0, "the import's marks go when it is done");
}
