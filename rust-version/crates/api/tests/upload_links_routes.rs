//! 5 Nov 2026 (Owner-approved plan, Stream B), through the real app: one-time upload links.
//!
//! - create_upload_link over MCP (act; not listed at propose), a PNG and an MP4 PUT to the public route, the drop
//!   page's status, get_upload_link listing file_ids that outbound_files::find_file resolves (what
//!   propose_customer_message attach uses), the attachment marked 'upload_link' and audited.
//! - staging: the file is a Nary file of the creator's, sendable by the creator and nobody else.
//! - refusals: unknown token 404, SVG 415, closed / expired / full 410, over the cap 413, a person who can't add
//!   files to the record can't make a link for it.

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_services::storage::{self, ObjectStore, StoreError};
use serde_json::{json, Value};
use support::{app, u, TestApp};
use uuid::Uuid;

const TAG: &str = "upload_links_routes";
const API: &str = "/api/v1";

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

impl ObjectStore for MemStore {
    fn bucket_exists<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> Fut<'c, Result<bool, StoreError>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(true) })
    }
    fn make_bucket<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(()) })
    }
    fn put_object<'a, 'b, 'd, 'e, 'f, 'c>(
        &'a self,
        _bucket: &'b str,
        key: &'d str,
        data: &'e [u8],
        _content_type: &'f str,
    ) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        'e: 'c,
        'f: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.blobs
                .lock()
                .unwrap()
                .insert(key.to_string(), data.to_vec());
            Ok(())
        })
    }
    fn get_object<'a, 'b, 'd, 'c>(
        &'a self,
        _bucket: &'b str,
        key: &'d str,
    ) -> Fut<'c, Result<Vec<u8>, StoreError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.blobs
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or(StoreError {
                    kind: "S3Error".into(),
                    message: key.into(),
                })
        })
    }
}

fn the_app() -> &'static TestApp {
    app(TAG)
}

// ─── Fixtures ────────────────────────────────────────────────────────────────

/// A contact assigned to `owner`.
async fn contact(app: &TestApp, owner: Uuid) -> Uuid {
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, is_primary, tags, is_deleted, custom, assigned_to_id) \
         VALUES ($1, 'Bob', $2, false, '{}', false, '{}', $3)",
    )
    .bind(id)
    .bind(u("Drop"))
    .bind(owner)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

fn png(n: usize) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    v.resize(n, 9);
    v
}

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// An MP4 (isom, H.264) of `seconds` and about `total` bytes (the uploads tests' fixture).
fn mp4(seconds: u32, total: usize) -> Vec<u8> {
    let mut ftyp = b"isom".to_vec();
    ftyp.extend_from_slice(&[0, 0, 2, 0]);
    for c in [b"isom", b"iso2", b"avc1", b"mp41"] {
        ftyp.extend_from_slice(c);
    }
    let mut out = boxed(b"ftyp", &ftyp);
    let mut mvhd = vec![0, 0, 0, 0];
    mvhd.extend_from_slice(&[0; 8]);
    mvhd.extend_from_slice(&1000u32.to_be_bytes());
    mvhd.extend_from_slice(&(seconds * 1000).to_be_bytes());
    mvhd.resize(mvhd.len() + 80, 0);
    let mut moov = boxed(b"mvhd", &mvhd);
    let stsd = boxed(
        b"stsd",
        &[&[0u8; 8][..], &boxed(b"avc1", &[0; 16])].concat(),
    );
    moov.extend(boxed(
        b"trak",
        &boxed(b"mdia", &boxed(b"minf", &boxed(b"stbl", &stsd))),
    ));
    out.extend(boxed(b"moov", &moov));
    let pad = total.saturating_sub(out.len() + 8);
    out.extend(boxed(b"mdat", &vec![0x11; pad]));
    out
}

async fn token(app: &TestApp, bearer: &str, scopes: &[&str]) -> String {
    let r = app
        .post(
            bearer,
            &format!("{API}/mcp/tokens"),
            json!({"name": u("drop"), "scopes": scopes, "password": "pytest-pass"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()["token"].as_str().unwrap().to_string()
}

async fn rpc(app: &TestApp, h: &str, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let r = app.post(h, &format!("{API}/mcp"), body).await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

async fn call(app: &TestApp, h: &str, name: &str, args: Value) -> (Value, bool) {
    let res = rpc(
        app,
        h,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
    .await["result"]
        .clone();
    let text = res["content"][0]["text"].as_str().unwrap().to_string();
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "text": text }));
    (body, res.get("isError") == Some(&json!(true)))
}

async fn tool_names(app: &TestApp, h: &str) -> Vec<String> {
    rpc(app, h, "tools/list", json!({})).await["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

/// The token in a page_url.
fn token_of(page_url: &str) -> String {
    page_url.rsplit('/').next().unwrap().to_string()
}

async fn put_file(app: &TestApp, tok: &str, name: &str, data: Vec<u8>) -> support::Resp {
    let rb = app
        .client()
        .put(app.url(&format!("{API}/public/drop/{tok}/files?name={name}")))
        .body(data);
    app.send(rb).await
}

async fn get_status(app: &TestApp, tok: &str) -> support::Resp {
    app.get("", &format!("{API}/public/drop/{tok}")).await
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_link_on_a_contact_png_and_mp4_then_attach_and_close() {
    let app = the_app();
    let _g = app.serial().await;
    let objects = Arc::new(MemStore::default());
    storage::set_store(Some(objects.clone()));
    let sa = app.superadmin().await;
    let who = contact(app, sa.id).await;

    // propose: not listed; act: listed with the hand-over
    let propose = token(app, &sa.token, &["propose"]).await;
    assert!(!tool_names(app, &propose)
        .await
        .iter()
        .any(|n| n == "create_upload_link"));
    let act = token(app, &sa.token, &["act"]).await;
    let names = tool_names(app, &act).await;
    for t in [
        "create_upload_link",
        "get_upload_link",
        "revoke_upload_link",
    ] {
        assert!(names.iter().any(|n| n == t), "{t} at act");
    }

    let (link, err) = call(
        app,
        &act,
        "create_upload_link",
        json!({"entity_type": "contact", "entity_id": who.to_string(), "note": "site video"}),
    )
    .await;
    assert!(!err, "{link}");
    let page = link["page_url"].as_str().unwrap();
    assert!(page.contains("/drop/"), "{page}");
    assert!(link["upload_url"]
        .as_str()
        .unwrap()
        .contains("/api/v1/public/drop/"));
    assert!(link["how_to"]["curl"]
        .as_str()
        .unwrap()
        .starts_with("curl -T"));
    assert_eq!(link["max_files"], 10);
    let tok = token_of(page);
    assert!(tok.len() >= 43, "{tok}");
    // only the hash is kept
    let mut db = app.db().await;
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM upload_links WHERE token_hash = $1")
        .bind(&tok)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(kept, 0);

    // the drop page's status: the record's kind and name, nothing else
    let r = get_status(app, &tok).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let st = r.json();
    assert!(st["target_label"]
        .as_str()
        .unwrap()
        .starts_with("Contact: Bob Drop-"));
    assert_eq!(st["state"], "open");
    assert_eq!(st["files_used"], 0);
    assert!(st.get("entity_id").is_none() && st.get("link_id").is_none());

    // a PNG and an MP4
    let r = put_file(app, &tok, "site%20photo.png", png(20 * 1024)).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["name"], "site photo.png");
    assert_eq!(r.json()["type"], "PNG image");
    assert!(r.json().get("file_id").is_none());
    let r = put_file(app, &tok, "walkthrough.mp4", mp4(42, 64 * 1024)).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["type"], "MP4 video");
    assert_eq!(r.json()["duration_label"], "0:42");
    assert_eq!(r.json()["files_used"], 2);
    // an SVG is refused by its content
    let r = put_file(
        app,
        &tok,
        "plan.svg",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".to_vec(),
    )
    .await;
    assert_eq!(r.status, 415, "{}", r.text());
    assert!(r.text().contains("SVG"), "{}", r.text());

    let st = get_status(app, &tok).await.json();
    assert_eq!(st["files_used"], 2);
    assert_eq!(st["files"][0]["name"], "site photo.png");

    // get_upload_link: file_ids that a customer message can attach
    let (got, err) = call(
        app,
        &act,
        "get_upload_link",
        json!({"link_id": link["link_id"]}),
    )
    .await;
    assert!(!err, "{got}");
    let files = got["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[1]["duration_label"], "0:42");
    let actor = granary_services::nary::roles::load_actor(&mut db, sa.id)
        .await
        .unwrap()
        .unwrap();
    for f in files {
        let id = f["file_id"].as_str().unwrap();
        let found = granary_services::nary::outbound_files::find_file(&mut db, &actor, id)
            .await
            .unwrap()
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(found.filename, f["name"].as_str().unwrap());
        let (via, by, etype): (Option<String>, Uuid, String) = sqlx::query_as(
            "SELECT uploaded_via, uploaded_by_id, entity_type FROM attachments WHERE id = $1",
        )
        .bind(Uuid::parse_str(id).unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
        assert_eq!(
            (via.as_deref(), by, etype.as_str()),
            (Some("upload_link"), sa.id, "crm_contact")
        );
    }
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE action = 'attachment.upload_via_link' AND entity_id = $1",
    )
    .bind(files[0]["file_id"].as_str().unwrap())
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(audited, 1);

    // Done closes it: 410 with the reason afterwards; the status still shows why
    let r = app
        .post("", &format!("{API}/public/drop/{tok}/close"), json!({}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = put_file(app, &tok, "late.png", png(1024)).await;
    assert_eq!(r.status, 410, "{}", r.text());
    assert!(r.text().contains("closed"), "{}", r.text());
    assert_eq!(get_status(app, &tok).await.json()["state"], "closed");

    // unknown token: 404 on every route
    let bogus = "x".repeat(43);
    assert_eq!(get_status(app, &bogus).await.status, 404);
    assert_eq!(put_file(app, &bogus, "a.png", png(100)).await.status, 404);
    storage::set_store(None);
}

#[tokio::test]
async fn test_staging_link_expired_full_and_revoked() {
    let app = the_app();
    let _g = app.serial().await;
    let objects = Arc::new(MemStore::default());
    storage::set_store(Some(objects.clone()));
    let sa = app.superadmin().await;
    let act = token(app, &sa.token, &["act"]).await;
    let mut db = app.db().await;

    // staging: no entity_id; a Nary file of the creator's
    let (bad, err) = call(
        app,
        &act,
        "create_upload_link",
        json!({"entity_type": "staging", "entity_id": Uuid::new_v4().to_string()}),
    )
    .await;
    assert!(err || bad.get("error").is_some(), "{bad}");
    let (link, err) = call(
        app,
        &act,
        "create_upload_link",
        json!({"entity_type": "staging"}),
    )
    .await;
    assert!(!err, "{link}");
    let tok = token_of(link["page_url"].as_str().unwrap());
    assert!(get_status(app, &tok).await.json()["target_label"]
        .as_str()
        .unwrap()
        .starts_with("Staging"));
    // unique content per run (nary_files keeps one row per content)
    let mut data = png(4096);
    data.extend_from_slice(Uuid::new_v4().as_bytes());
    let r = put_file(app, &tok, "brief.png", data).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (got, _) = call(
        app,
        &act,
        "get_upload_link",
        json!({"link_id": link["link_id"]}),
    )
    .await;
    let fid = got["files"][0]["file_id"].as_str().unwrap().to_string();
    assert_eq!(got["files"][0]["stored_as"], "staging (Nary file)");
    let actor = granary_services::nary::roles::load_actor(&mut db, sa.id)
        .await
        .unwrap()
        .unwrap();
    let found = granary_services::nary::outbound_files::find_file(&mut db, &actor, &fid)
        .await
        .unwrap()
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(found.filename, "brief.png");
    // someone else can't send it (nor see the link)
    let other = app.user_with(&["crm:view_all"]).await;
    let other_actor = granary_services::nary::roles::load_actor(&mut db, other.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        granary_services::nary::outbound_files::find_file(&mut db, &other_actor, &fid)
            .await
            .unwrap()
            .is_err()
    );
    assert!(granary_services::upload_links::get(
        &mut db,
        &other_actor,
        link["link_id"].as_str().unwrap()
    )
    .await
    .unwrap()
    .is_err());

    // full: a link of one file takes no second
    sqlx::query("UPDATE upload_links SET max_files = 1 WHERE id = $1")
        .bind(Uuid::parse_str(link["link_id"].as_str().unwrap()).unwrap())
        .execute(&mut db)
        .await
        .unwrap();
    let r = put_file(app, &tok, "second.png", png(2048)).await;
    assert_eq!(r.status, 410, "{}", r.text());
    assert!(r.text().contains("taken its 1 files"), "{}", r.text());

    // expired
    let (link2, _) = call(
        app,
        &act,
        "create_upload_link",
        json!({"entity_type": "staging"}),
    )
    .await;
    let tok2 = token_of(link2["page_url"].as_str().unwrap());
    sqlx::query("UPDATE upload_links SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(Uuid::parse_str(link2["link_id"].as_str().unwrap()).unwrap())
        .execute(&mut db)
        .await
        .unwrap();
    let r = put_file(app, &tok2, "x.png", png(2048)).await;
    assert_eq!(r.status, 410, "{}", r.text());
    assert!(r.text().contains("expired"), "{}", r.text());
    assert_eq!(get_status(app, &tok2).await.json()["state"], "expired");

    // revoked by the tool
    let (link3, _) = call(
        app,
        &act,
        "create_upload_link",
        json!({"entity_type": "staging"}),
    )
    .await;
    let tok3 = token_of(link3["page_url"].as_str().unwrap());
    let (rev, err) = call(
        app,
        &act,
        "revoke_upload_link",
        json!({"link_id": link3["link_id"]}),
    )
    .await;
    assert!(!err && rev["revoked"] == json!(true), "{rev}");
    assert_eq!(put_file(app, &tok3, "x.png", png(2048)).await.status, 410);
    storage::set_store(None);
}

#[tokio::test]
async fn test_no_link_without_access_and_the_size_cap() {
    let app = the_app();
    let _g = app.serial().await;
    let objects = Arc::new(MemStore::default());
    storage::set_store(Some(objects.clone()));
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let theirs = contact(app, sa.id).await;

    // view_own on someone else's contact: no link
    let own = app.user_with(&["crm:view_own"]).await;
    let own_actor = granary_services::nary::roles::load_actor(&mut db, own.id)
        .await
        .unwrap()
        .unwrap();
    let refused = granary_services::upload_links::create(
        &mut db,
        &own_actor,
        "contact",
        &theirs.to_string(),
        "",
        "mcp",
    )
    .await
    .unwrap();
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.contains("can't add files")),
        "{refused:?}"
    );

    // over the read cap (the larger of the two caps): 413 before the file is stored
    app.set_setting("nary.upload.max_mb", "1").await;
    app.set_setting("nary.upload.video_max_mb", "1").await;
    let actor = granary_services::nary::roles::load_actor(&mut db, sa.id)
        .await
        .unwrap()
        .unwrap();
    let link = granary_services::upload_links::create(
        &mut db,
        &actor,
        "contact",
        &theirs.to_string(),
        "",
        "mcp",
    )
    .await
    .unwrap()
    .unwrap();
    let tok = token_of(link["page_url"].as_str().unwrap());
    let r = put_file(app, &tok, "big.png", png(1024 * 1024 + 10)).await;
    app.set_setting("nary.upload.max_mb", "10").await;
    app.set_setting("nary.upload.video_max_mb", "100").await;
    assert_eq!(r.status, 413, "{}", r.text());
    assert_eq!(get_status(app, &tok).await.json()["files_used"], 0);
    storage::set_store(None);
}
