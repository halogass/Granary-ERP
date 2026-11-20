//! Document library, agent A1: the HTTP API through the real app (routes registered without conflicts, the
//! multipart create and upload, list filters, detail with permissions, PATCH, suggest-label, approve/reject,
//! categories, article links, the catalog endpoint).

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use granary_services::pdftools::{Document as Pdf, Point, SaveOptions, TextOptions};
use granary_services::storage::{self, ObjectStore, StoreError};
use serde_json::json;
use support::{TestApp, UserSpec};
use uuid::Uuid;

const TAG: &str = "doclib_routes";

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

fn app() -> &'static TestApp {
    support::app(TAG)
}

fn pdf(text: &str) -> Vec<u8> {
    let mut doc = Pdf::new().unwrap();
    let p = doc.new_page(595.0, 842.0).unwrap();
    doc.insert_text(p, Point::new(50.0, 80.0), text, &TextOptions::default())
        .unwrap();
    doc.tobytes(SaveOptions::default()).unwrap()
}

fn pdf_part(name: &str, text: &str) -> reqwest::multipart::Part {
    reqwest::multipart::Part::bytes(pdf(text))
        .file_name(name.to_string())
        .mime_str("application/pdf")
        .unwrap()
}

async fn create(app: &TestApp, token: &str, meta: serde_json::Value, text: &str) -> support::Resp {
    let form = reqwest::multipart::Form::new()
        .text("meta", meta.to_string())
        .part("file", pdf_part("doc.pdf", text));
    app.send(
        app.client()
            .post(app.url("/api/v1/knowledge/documents"))
            .bearer_auth(token)
            .multipart(form),
    )
    .await
}

async fn upload(
    app: &TestApp,
    token: &str,
    id: &str,
    label: Option<&str>,
    note: &str,
) -> support::Resp {
    let mut form = reqwest::multipart::Form::new()
        .text("change_note", note.to_string())
        .part("file", pdf_part("doc.pdf", note));
    if let Some(l) = label {
        form = form.text("label", l.to_string());
    }
    app.send(
        app.client()
            .post(app.url(&format!("/api/v1/knowledge/documents/{id}/versions")))
            .bearer_auth(token)
            .multipart(form),
    )
    .await
}

#[tokio::test]
async fn test_the_document_library_api() {
    let app = app();
    let _serial = app.serial().await;
    let objects = Arc::new(MemStore::default());
    storage::set_store(Some(objects.clone()));
    let mut db = app.db().await;
    let marketer = app.make_user(UserSpec::new().roles(&["marketing"])).await;
    let engineer = app.make_user(UserSpec::new().roles(&["engineer"])).await;
    let exec = app.make_user(UserSpec::new().roles(&["executive"])).await;
    let sysadmin = app
        .make_user(UserSpec::new().roles(&["system_admin"]))
        .await;
    let partner = app
        .make_user(UserSpec::new().roles(&["partner_portal"]).portal("partner"))
        .await;

    // create (multipart) an internal document: live at once
    let r = create(
        app,
        &marketer.token,
        json!({"title": "Company profile", "category": "company",
        "audiences": ["internal"], "label": "v1", "change_note": "First"}),
        "profile",
    )
    .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let d = r.json();
    let id = d["id"].as_str().unwrap().to_string();
    assert_eq!(d["current_version"]["label"], "v1");
    assert_eq!(d["permissions"]["edit"], true);
    assert!(d["versions"].is_null());
    // a bad meta, no file
    let r = create(
        app,
        &marketer.token,
        json!({"title": "", "category": "company", "audiences": ["internal"]}),
        "x",
    )
    .await;
    assert_eq!(r.status, 400, "{}", r.text());
    let r = create(
        app,
        &engineer.token,
        json!({"title": "E", "category": "company", "audiences": ["internal"]}),
        "x",
    )
    .await;
    assert_eq!(r.status, 403);

    // suggest-label, upload a version, list
    let r = app
        .get(
            &marketer.token,
            &format!("/api/v1/knowledge/documents/{id}/suggest-label"),
        )
        .await;
    assert_eq!(r.json(), json!({"label": "v2"}));
    let r = upload(app, &marketer.token, &id, None, "Second").await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(
        (r.json()["label"].clone(), r.json()["status"].clone()),
        (json!("v2"), json!("live"))
    );
    let r = upload(app, &engineer.token, &id, None, "nope").await;
    assert_eq!(r.status, 403);
    let r = app
        .get(
            &engineer.token,
            "/api/v1/knowledge/documents?view=internal&q=company",
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["total"], 1);
    assert_eq!(r.json()["items"][0]["current_version"]["label"], "v2");
    assert_eq!(r.json()["items"][0]["can_edit"], false);
    // a partner user is not internal staff
    let r = app.get(&partner.token, "/api/v1/knowledge/documents").await;
    assert_eq!(r.status, 403, "{}", r.text());

    // a customer document goes through approval; approve and reject endpoints
    let pid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, family, product_type, unit, is_active, sale_model, price_metric, \
         modules, pricing_mode, addon_for, is_whitelabel, is_orderable, is_deleted) \
         VALUES ($1, $2, 'Office laptop', 'laptop', 'physical', 'Unit', true, 'one_time', 'unit', '{}', 'list', '{}', false, true, false)",
    )
    .bind(pid)
    .bind(format!("TM-{}", &pid.simple().to_string()[..6]))
    .execute(&mut db)
    .await
    .unwrap();
    let r = create(
        app,
        &marketer.token,
        json!({"title": "TM brochure", "category": "products",
        "audiences": ["customer"], "products": [{"product_id": pid}]}),
        "brochure",
    )
    .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let b = r.json();
    let bid = b["id"].as_str().unwrap().to_string();
    assert!(b["current_version"].is_null());
    let v1 = b["pending_version"]["id"].as_str().unwrap().to_string();
    let r = app
        .get(&engineer.token, "/api/v1/knowledge/documents?view=pending")
        .await;
    assert_eq!(r.json()["total"], 1);
    // the approval link (?version=) resolves to the document
    let r = app
        .get(
            &exec.token,
            &format!("/api/v1/knowledge/documents?version={v1}"),
        )
        .await;
    assert_eq!(r.json()["items"][0]["id"], bid.as_str());
    let r = app
        .post(
            &marketer.token,
            &format!("/api/v1/knowledge/documents/{bid}/versions/{v1}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 403);
    let r = app
        .post(
            &exec.token,
            &format!("/api/v1/knowledge/documents/{bid}/versions/{v1}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["status"], "live");
    let v2 = upload(app, &marketer.token, &bid, None, "Typo")
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = app
        .post(
            &exec.token,
            &format!("/api/v1/knowledge/documents/{bid}/versions/{v2}/reject"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 400);
    let r = app
        .post(
            &exec.token,
            &format!("/api/v1/knowledge/documents/{bid}/versions/{v2}/reject"),
            json!({"comment": "Wrong logo"}),
        )
        .await;
    assert_eq!(r.json()["rejected_reason"], "Wrong logo");
    // the catalog endpoint
    let r = app
        .get(
            &engineer.token,
            &format!("/api/v1/sales/catalog/{pid}/documents"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["items"][0]["title"], "TM brochure");

    // detail: history for doc admins only; restore
    let r = app
        .get(
            &sysadmin.token,
            &format!("/api/v1/knowledge/documents/{bid}"),
        )
        .await;
    assert_eq!(r.json()["versions"].as_array().unwrap().len(), 2);
    assert_eq!(r.json()["permissions"]["approve"], false);
    let r = app
        .get(
            &engineer.token,
            &format!("/api/v1/knowledge/documents/{bid}"),
        )
        .await;
    assert!(r.json()["versions"].is_null());
    let r = app
        .post(
            &sysadmin.token,
            &format!(
                "/api/v1/knowledge/documents/{id}/versions/{}/restore",
                app.get(
                    &sysadmin.token,
                    &format!("/api/v1/knowledge/documents/{id}")
                )
                .await
                .json()["versions"][1]["id"]
                    .as_str()
                    .unwrap()
            ),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["change_note"], "Restored from v1");

    // PATCH: owner change only for doc admins; widening to customers only for directors
    let r = app
        .patch(
            &marketer.token,
            &format!("/api/v1/knowledge/documents/{id}"),
            json!({"title": "ACME company profile", "review_by": "2030-01-31", "language": "en"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["title"], "ACME company profile");
    assert_eq!(r.json()["review_by"], "2030-01-31");
    let r = app
        .patch(
            &marketer.token,
            &format!("/api/v1/knowledge/documents/{id}"),
            json!({"owner_id": engineer.id}),
        )
        .await;
    assert_eq!(r.status, 403);
    let r = app
        .patch(
            &marketer.token,
            &format!("/api/v1/knowledge/documents/{id}"),
            json!({"audiences": ["internal", "customer"]}),
        )
        .await;
    assert_eq!(r.status, 403);
    let r = app
        .patch(
            &sysadmin.token,
            &format!("/api/v1/knowledge/documents/{id}"),
            json!({"owner_id": engineer.id}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["owner"]["id"], json!(engineer.id));

    // categories: doc admins set role visibility
    let r = app
        .put(
            &engineer.token,
            "/api/v1/knowledge/document-categories",
            json!([{"key": "pricing", "visible_roles": ["executive"]}]),
        )
        .await;
    assert_eq!(r.status, 403);
    let r = app
        .put(
            &sysadmin.token,
            "/api/v1/knowledge/document-categories",
            json!([{"key": "pricing", "visible_roles": ["executive"]}]),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = app
        .get(&engineer.token, "/api/v1/knowledge/document-categories")
        .await;
    let pricing = r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "pricing")
        .unwrap()
        .clone();
    assert_eq!(pricing["can_view"], false);
    assert_eq!(pricing["visible_roles"], json!(["executive"]));

    // article links
    let aid = Uuid::new_v4();
    sqlx::query("INSERT INTO kb_articles (id, title, slug, status, is_deleted) VALUES ($1, 'Install guide', $2, 'published', false)")
        .bind(aid)
        .bind(format!("install-{}", &aid.simple().to_string()[..6]))
        .execute(&mut db)
        .await
        .unwrap();
    let r = app
        .put(
            &sysadmin.token,
            &format!("/api/v1/knowledge/articles/{aid}/documents"),
            json!({"document_ids": [bid]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = app
        .get(
            &engineer.token,
            &format!("/api/v1/knowledge/articles/{aid}/documents"),
        )
        .await;
    assert_eq!(r.json()["items"][0]["id"], json!(bid));

    // delete: directors only
    let r = app
        .delete(
            &marketer.token,
            &format!("/api/v1/knowledge/documents/{bid}"),
        )
        .await;
    assert_eq!(r.status, 403);
    let r = app
        .delete(&exec.token, &format!("/api/v1/knowledge/documents/{bid}"))
        .await;
    assert_eq!(r.status, 204);
    let r = app
        .get(&exec.token, &format!("/api/v1/knowledge/documents/{bid}"))
        .await;
    assert_eq!(r.status, 404);
}
