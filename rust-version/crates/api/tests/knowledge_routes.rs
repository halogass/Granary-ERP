//! routers/knowledge.py: the backend tests that write rows directly or fake the PDF service
//! (test_audit_high_b2.py H21, test_audit_medium_w3.py SOP revisions, test_audit_low_w2.py RFC 5987 file
//! name) and the import-pdf paths that need an attachment row. The HTTP-only tests are in
//! compat-tests/http-suite/tests/test_knowledge_routes.py.

mod support;

use std::io::{Read, Write};
use std::sync::OnceLock;

use serde_json::{json, Value};
use uuid::Uuid;

/// A PDF service stand-in (`generate_pdf` monkeypatched to return b"%PDF"): a thread that answers
/// every request with "%PDF". It outlives the per-test runtimes, like the app.
fn pdf_service() -> String {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                std::thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    // read headers, then the declared body
                    loop {
                        let n = s.read(&mut chunk).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            while buf.len() < pos + 4 + len {
                                let n = s.read(&mut chunk).unwrap_or(0);
                                if n == 0 {
                                    break;
                                }
                                buf.extend_from_slice(&chunk[..n]);
                            }
                            break;
                        }
                    }
                    let _ = s.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/pdf\r\ncontent-length: 4\r\nconnection: close\r\n\r\n%PDF",
                    );
                });
            }
        });
        url
    })
    .clone()
}

fn app() -> &'static support::TestApp {
    std::env::set_var("PDF_SERVICE_URL", pdf_service());
    support::app("knowledge")
}

/// `_article(**kw)`: a KBArticle row with content "body".
async fn article(
    status: &str,
    access_level: &str,
    is_private: bool,
    created_by: Option<Uuid>,
) -> (Uuid, String) {
    let app = app();
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    let slug = support::u("pyt-b2");
    sqlx::query(
        "INSERT INTO kb_articles (id, title, slug, content, status, access_level, is_private, created_by_id, \
         article_type, version, is_deleted) VALUES ($1, $2, $2, 'body', $3, $4, $5, $6, 'wiki', '1.0', false)",
    )
    .bind(id)
    .bind(&slug)
    .bind(status)
    .bind(access_level)
    .bind(is_private)
    .bind(created_by)
    .execute(&mut db)
    .await
    .unwrap();
    (id, slug)
}

/// test_audit_high_b2.py::test_restricted_and_draft_articles_are_hidden_from_readers.
#[tokio::test]
async fn test_restricted_and_draft_articles_are_hidden_from_readers() {
    let app = app();
    let reader = app.user_with(&["kb:view"]).await;
    let restricted = article("published", "restricted", false, None).await;
    let management = article("published", "management", false, None).await;
    let draft = article("draft", "all_staff", false, None).await;
    let someone = app.make_user(support::UserSpec::new()).await;
    let private = article("published", "all_staff", true, Some(someone.id)).await;
    let public = article("published", "all_staff", false, None).await;
    let k = |p: String| format!("/api/v1/kb/articles{p}");
    for (id, slug) in [&restricted, &management, &draft, &private] {
        let mut idents = vec![id.to_string()];
        if id == &draft.0 {
            idents.push(slug.clone());
        }
        for ident in idents {
            assert_eq!(
                app.get(&reader.token, &k(format!("/{ident}"))).await.status,
                404
            );
        }
        assert_eq!(
            app.get(&reader.token, &k(format!("/{id}/revisions")))
                .await
                .status,
            404
        );
        assert_eq!(
            app.get(&reader.token, &k(format!("/{id}/pdf")))
                .await
                .status,
            404
        );
    }
    assert_eq!(
        app.get(&reader.token, &k(format!("/{}", public.0)))
            .await
            .status,
        200
    );
    assert_eq!(
        app.get(&reader.token, &k(format!("/{}/revisions", public.0)))
            .await
            .status,
        200
    );
    let listed: Vec<String> = app
        .get(&reader.token, &k("?status=draft".into()))
        .await
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !listed.contains(&draft.0.to_string()),
        "?status=draft must not lift the published-only rule"
    );

    let restricted_reader = app.user_with(&["kb:view", "kb:view_restricted"]).await;
    assert_eq!(
        app.get(&restricted_reader.token, &k(format!("/{}", restricted.0)))
            .await
            .status,
        200
    );
    assert_eq!(
        app.get(&restricted_reader.token, &k(format!("/{}", draft.0)))
            .await
            .status,
        404
    );
    let editor = app.user_with(&["kb:view", "kb:create"]).await;
    assert_eq!(
        app.get(&editor.token, &k(format!("/{}", draft.0)))
            .await
            .status,
        200
    );
    let listed = app
        .get(&editor.token, &k("?status=draft&limit=500".into()))
        .await
        .json();
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["id"] == json!(draft.0.to_string())));
}

/// `_approve_pending_sop(article_id, token)`: approve the newest sop_article request when pending.
async fn approve_pending_sop(article_id: &str, user_id: Uuid) {
    let app = app();
    let mut db = app.db().await;
    let req: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, status FROM approval_requests WHERE entity_type = 'sop_article' AND entity_id = $1 \
         ORDER BY submitted_at DESC LIMIT 1",
    )
    .bind(Uuid::parse_str(article_id).unwrap())
    .fetch_optional(&mut db)
    .await
    .unwrap();
    let (req_id, status) = req.expect("the countersignature submits the SOP for approval");
    if status == "pending" {
        let actor = granary_services::nary::roles::load_actor(&mut db, user_id)
            .await
            .unwrap()
            .unwrap();
        granary_services::approvals::engine::act(
            &mut db,
            &req_id.to_string(),
            &actor,
            "approved",
            None,
            None,
            None,
        )
        .await
        .unwrap();
    } else {
        assert_eq!(status, "approved");
    }
}

/// test_audit_medium_w3.py::test_sop_signatures_submit_for_approval_and_edits_open_a_revision.
#[tokio::test]
async fn test_sop_signatures_submit_for_approval_and_edits_open_a_revision() {
    let app = app();
    let sa = app.superadmin().await;
    let k = |p: &str| format!("/api/v1/kb/articles{p}");
    let res = app
        .post(
            &sa.token,
            &k(""),
            json!({"title": support::u("Pytest W3 SOP"), "article_type": "sop", "content": "v1 body", "sop_code": ""}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let aid = res.json()["id"].as_str().unwrap().to_string();
    let mut db = app.db().await;
    let code: Option<String> = sqlx::query_scalar("SELECT sop_code FROM kb_articles WHERE id = $1")
        .bind(Uuid::parse_str(&aid).unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(code, None); // "" stored as NULL

    assert_eq!(
        app.post(&sa.token, &k(&format!("/{aid}/sign")), json!({}))
            .await
            .status,
        200
    );
    let res = app
        .post(&sa.token, &k(&format!("/{aid}/sign/owner")), json!({}))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    approve_pending_sop(&aid, sa.id).await;
    let live = app.get(&sa.token, &k(&format!("/{aid}"))).await.json();
    assert_eq!(live["status"], "published");

    // Editing the published SOP opens a draft revision; the live one is untouched
    let res = app
        .patch(
            &sa.token,
            &k(&format!("/{aid}")),
            json!({"content": "v2 body", "sop_code": ""}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(res.json()["created_revision"], true);
    let rid = res.json()["id"].as_str().unwrap().to_string();
    assert!(rid != aid && res.json()["revision_of"] == json!(aid));
    let live = app.get(&sa.token, &k(&format!("/{aid}"))).await.json();
    assert!(live["content"] == "v1 body" && live["status"] == "published");
    assert_eq!(live["pending_revision_id"], json!(rid));
    assert_eq!(
        app.patch(&sa.token, &k(&format!("/{aid}")), json!({"content": "x"}))
            .await
            .status,
        409
    );

    // The revision is signed anew and approved; then it replaces the live content
    assert_eq!(
        app.post(&sa.token, &k(&format!("/{rid}/sign")), json!({}))
            .await
            .status,
        200
    );
    assert_eq!(
        app.post(&sa.token, &k(&format!("/{rid}/sign/owner")), json!({}))
            .await
            .status,
        200
    );
    approve_pending_sop(&rid, sa.id).await;
    let live = app.get(&sa.token, &k(&format!("/{aid}"))).await.json();
    assert!(live["content"] == "v2 body" && live["status"] == "published");
    assert!(live["version"] == "2.0" && live["pending_revision_id"] == Value::Null);
    let rev = app.get(&sa.token, &k(&format!("/{rid}"))).await.json();
    assert_eq!(rev["status"], "archived");
    assert!(
        live["author_signed_at"] == rev["author_signed_at"]
            && live["owner_signed_at"] == rev["owner_signed_at"]
    );
}

/// test_audit_low_w2.py::test_kb_pdf_filename_is_rfc5987.
#[tokio::test]
async fn test_kb_pdf_filename_is_rfc5987() {
    let app = app();
    let admin = app.make_user(support::UserSpec::new().superadmin()).await;
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO kb_articles (id, title, slug, content, status, is_private, is_deleted) \
         VALUES ($1, 'SOP \u{fc}', $2, 'x', 'published', false, false)",
    )
    .bind(id)
    .bind(support::u("sop-\u{65e5}\u{672c}"))
    .execute(&mut db)
    .await
    .unwrap();
    let r = app
        .get(&admin.token, &format!("/api/v1/kb/articles/{id}/pdf"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.body, b"%PDF");
    assert_eq!(r.header("content-type").as_deref(), Some("application/pdf"));
    let cd = r.header("content-disposition").unwrap();
    assert!(cd.contains("filename*=UTF-8''"), "{cd}");
    assert!(cd.starts_with("attachment; filename=\"sop-__-"), "{cd}");
    assert!(
        cd.contains("filename*=UTF-8''sop-%E6%97%A5%E6%9C%AC-"),
        "{cd}"
    );
}

async fn attachment(article_id: Uuid, mime: &str) -> Uuid {
    let app = app();
    let mut db = app.db().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO attachments (id, entity_type, entity_id, file_name, file_size, mime_type, object_key, url, file_type) \
         VALUES ($1, 'kb_article', $2, 'manual.pdf', 1234, $3, 'kb/manual.pdf', 'http://x/manual.pdf', 'document')",
    )
    .bind(id)
    .bind(article_id.to_string())
    .bind(mime)
    .execute(&mut db)
    .await
    .unwrap();
    id
}

/// import-pdf with an attachment on the article: only PDFs, never over content unless forced, and
/// (no worker in tests) the article is marked failed with the queue's message.
#[tokio::test]
async fn test_import_pdf_queues_or_reports_why_not() {
    let app = app();
    let sa = app.superadmin().await;
    let res = app
        .post(
            &sa.token,
            "/api/v1/kb/articles",
            json!({"title": support::u("Pdf"), "content": ""}),
        )
        .await;
    let aid = Uuid::parse_str(res.json()["id"].as_str().unwrap()).unwrap();
    let png = attachment(aid, "image/png").await;
    let url = format!("/api/v1/kb/articles/{aid}/import-pdf");
    let res = app
        .post(&sa.token, &url, json!({"attachment_id": png.to_string()}))
        .await;
    assert_eq!(res.status, 422);
    assert_eq!(
        res.json(),
        json!({"detail": "Only PDF files can be converted"})
    );
    let pdf = attachment(aid, "application/pdf").await;
    let res = app
        .post(&sa.token, &url, json!({"attachment_id": pdf.to_string()}))
        .await;
    assert_eq!(res.status, 202, "{}", res.text());
    assert_eq!(
        res.json(),
        json!({"id": aid.to_string(), "pdf_import_status": "failed",
               "pdf_import_error": "No background worker responded, so the conversion cannot start. Check that the background worker is running and up to date.",
               "source_pdf": {"id": pdf.to_string(), "file_name": "manual.pdf"}})
    );
    let detail = app
        .get(&sa.token, &format!("/api/v1/kb/articles/{aid}"))
        .await
        .json();
    assert_eq!(detail["pdf_import_status"], "failed");
    assert_eq!(
        detail["source_pdf"],
        json!({"id": pdf.to_string(), "file_name": "manual.pdf", "file_size": 1234})
    );
    // the article's own source PDF is the default; content blocks a re-run unless forced
    app.patch(
        &sa.token,
        &format!("/api/v1/kb/articles/{aid}"),
        json!({"content": "converted"}),
    )
    .await;
    let res = app.post(&sa.token, &url, json!({})).await;
    assert_eq!(res.status, 409);
    assert_eq!(
        res.json(),
        json!({"detail": "This article already has content. Re-run with force to replace it."})
    );
    assert_eq!(
        app.post(&sa.token, &url, json!({"force": true}))
            .await
            .status,
        202
    );
    // an attachment of another article is not this article's
    let other = attachment(Uuid::new_v4(), "application/pdf").await;
    let res = app
        .post(
            &sa.token,
            &url,
            json!({"attachment_id": other.to_string(), "force": true}),
        )
        .await;
    assert_eq!(res.status, 404);
}

/// Editing an article enqueues its re-embedding (reindex_kb_embeddings).
#[tokio::test]
async fn test_edits_enqueue_a_reindex() {
    let app = app();
    let sa = app.superadmin().await;
    let _g = app.serial().await;
    let res = app
        .post(
            &sa.token,
            "/api/v1/kb/articles",
            json!({"title": support::u("Re")}),
        )
        .await;
    let aid = res.json()["id"].as_str().unwrap().to_string();
    app.enqueued();
    app.patch(
        &sa.token,
        &format!("/api/v1/kb/articles/{aid}"),
        json!({"content": "x"}),
    )
    .await;
    app.delete(&sa.token, &format!("/api/v1/kb/articles/{aid}"))
        .await;
    let jobs: Vec<(String, Vec<Value>)> = app
        .enqueued()
        .into_iter()
        .map(|j| (j.task, j.args))
        .filter(|(_, a)| a == &vec![json!(aid)])
        .collect();
    assert_eq!(
        jobs,
        vec![
            (
                "app.tasks.reindex_kb_embeddings".to_string(),
                vec![json!(aid)]
            ),
            (
                "app.tasks.reindex_kb_embeddings".to_string(),
                vec![json!(aid)]
            ),
        ]
    );
}

/// The categories page shows each category's description and orders by sort_order, so the
/// list returns both (it used to drop them). It sends no active flag: the page must not
/// read a missing one as "Inactive".
#[tokio::test]
async fn test_category_list_returns_description_and_sort_order() {
    let app = app();
    let sa = app.superadmin().await;
    let slug = support::u("cat-desc");
    let res = app
        .post(
            &sa.token,
            "/api/v1/kb/categories",
            json!({"name": slug, "slug": slug, "description": "Warehouse know-how"}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let list = app.get(&sa.token, "/api/v1/kb/categories").await.json();
    let cat = list
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["slug"] == json!(slug))
        .cloned()
        .expect("created category listed");
    assert_eq!(cat["description"], json!("Warehouse know-how"));
    assert_eq!(cat["sort_order"], json!(0));
    assert!(cat.get("is_active").is_none(), "{cat}");
}

/// Batch E: the Sort order typed on the categories page is saved (create_category used to store 0).
#[tokio::test]
async fn test_create_category_saves_sort_order() {
    let app = app();
    let sa = app.superadmin().await;
    let slug = support::u("cat-sort");
    let res = app
        .post(
            &sa.token,
            "/api/v1/kb/categories",
            json!({"name": slug, "slug": slug, "sort_order": 7}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let list = app.get(&sa.token, "/api/v1/kb/categories").await.json();
    let cat = list
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["slug"] == json!(slug))
        .cloned()
        .expect("created category listed");
    assert_eq!(cat["sort_order"], json!(7));

    let bad = support::u("cat-sort-bad");
    let res = app
        .post(
            &sa.token,
            "/api/v1/kb/categories",
            json!({"name": bad, "slug": bad, "sort_order": 10_000_000}),
        )
        .await;
    assert_eq!(res.status, 422, "{}", res.text());
}

/// Batch E: kb:view was seeded but never checked. The knowledge read routes now need it (or an editing
/// permission, which includes reading), like the other modules' `:view` permissions.
#[tokio::test]
async fn test_knowledge_reads_need_kb_view() {
    let app = app();
    let nobody = app.user_with(&[]).await;
    let reader = app.user_with(&["kb:view"]).await;
    let editor = app.user_with(&["kb:create"]).await;
    let public = article("published", "all_staff", false, None).await;
    let k = |p: String| format!("/api/v1/kb{p}");
    let reads = [
        k("/categories".into()),
        k("/articles".into()),
        k(format!("/articles/{}", public.0)),
        k(format!("/articles/{}", public.1)),
        k(format!("/articles/{}/revisions", public.0)),
        k(format!("/articles/{}/pdf", public.0)),
    ];
    for path in &reads {
        let res = app.get(&nobody.token, path).await;
        assert_eq!(res.status, 403, "{path}: {}", res.text());
        assert!(res.text().contains("kb:view"), "{path}: {}", res.text());
    }
    for path in &reads {
        for who in [&reader, &editor] {
            let res = app.get(&who.token, path).await;
            assert_eq!(res.status, 200, "{path}: {}", res.text());
        }
    }
}
