//! routers/ai_knowledge.py through the real app. Ports the HTTP parts of backend/tests/test_ai_knowledge.py
//! (toy bag-of-words embeddings behind the registry seam, the stored originals in a fake object store, no
//! Celery worker so indexing runs inline) and of test_ai_erp_guide.py::test_guide_documents_are_read_only_in_settings
//! (the services test crates/services/tests/ai_erp_guide.rs marks it `wave 4`). Permission and validation
//! checks are in the HTTP suite (compat-tests/http-suite/tests/test_ai_routes.py).
//!
//! Not ported: test_a_pdf_falls_back_to_its_text_when_ai_conversion_is_off (it builds the PDF with PyMuPDF;
//! the conversion itself is covered by the services tests of ai::pdf_import).

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};

use granary_services::ai::base::{AiError, ChatRequest, ChatResult, LLMProvider};
use granary_services::ai::registry::seam::Seam;
use granary_services::storage::{ObjectStore, StoreError};
use serde_json::{json, Value};
use support::{app, UserSpec};
use uuid::Uuid;

const TAG: &str = "ai_knowledge_routes";
const K: &str = "/api/v1/ai/knowledge";

const PRICING: &str = "# Discount rules\nDistributors get 12% off list price on IoT gateways. Discounts above 20% need Owner approval.\nNever quote the Granary Server below JPY 45,000,000.";

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ─── fakes ────────────────────────────────────────────────────────────────────

/// The private bucket (`_put` / `_remove` monkeypatched to a dict in Python).
#[derive(Default)]
struct FakeStore(Mutex<HashMap<String, Vec<u8>>>);

impl ObjectStore for FakeStore {
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
            self.0
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
            self.0
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or_else(|| StoreError {
                    kind: "NoSuchKey".into(),
                    message: key.into(),
                })
        })
    }
    fn remove_object<'a, 'b, 'd, 'c>(
        &'a self,
        _bucket: &'b str,
        key: &'d str,
    ) -> Fut<'c, Result<(), StoreError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            self.0.lock().unwrap().remove(key);
            Ok(())
        })
    }
}

fn store() -> Arc<FakeStore> {
    static STORE: OnceLock<Arc<FakeStore>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let s = Arc::new(FakeStore::default());
            granary_services::storage::set_store(Some(s.clone() as Arc<dyn ObjectStore>));
            s
        })
        .clone()
}

fn stored_keys(doc_id: &str) -> Vec<String> {
    store()
        .0
        .lock()
        .unwrap()
        .keys()
        .filter(|k| k.contains(doc_id))
        .cloned()
        .collect()
}

/// `_vector(text)`: a toy bag of words (any stable hash serves; Python uses md5).
fn toy_vector(text: &str) -> Vec<f64> {
    let mut v = vec![0.0; 768];
    let lower = text.to_lowercase();
    for w in lower
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        .filter(|w| w.len() > 2)
    {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in w.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        v[(h % 768) as usize] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm = if norm == 0.0 { 1.0 } else { norm };
    v.iter().map(|x| x / norm).collect()
}

struct ToyEmbedder;

impl LLMProvider for ToyEmbedder {
    fn name(&self) -> &str {
        "stub"
    }
    fn chat<'a, 'b, 'd, 'c>(
        &'a self,
        _model: &'b str,
        _req: &'d ChatRequest,
    ) -> Fut<'c, Result<ChatResult, AiError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        Box::pin(async { Err(AiError::Other("no chat".into())) })
    }
    fn embed<'a, 'b, 'd, 'c>(
        &'a self,
        _model: &'b str,
        texts: &'d [String],
    ) -> Fut<'c, Result<Vec<Vec<f64>>, AiError>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        Box::pin(async move { Ok(texts.iter().map(|t| toy_vector(t)).collect()) })
    }
}

/// The `toy_embeddings` fixture: config with a key, the toy embedder, the fake store.
async fn toy_embeddings() -> Seam {
    store();
    let seam = Seam::install().await;
    seam.get_ai_config(|| {
        let mut c = HashMap::new();
        c.insert("ai.api_key".to_string(), "k".to_string());
        c.insert(
            "ai.routing".to_string(),
            r#"{"default": {"provider": "ollama", "model": "m"}, "embedding": {"provider": "ollama", "model": "e"}}"#
                .to_string(),
        );
        Ok(c)
    });
    seam.get_provider(|_, _| Ok(Arc::new(ToyEmbedder) as Arc<dyn LLMProvider>));
    seam
}

async fn admin() -> String {
    app(TAG)
        .make_user(UserSpec::new().system_admin())
        .await
        .token
}

async fn upload(token: &str, form: reqwest::multipart::Form) -> Value {
    let app = app(TAG);
    let r = app
        .send(
            app.client()
                .post(app.url(K))
                .bearer_auth(token)
                .multipart(form),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()
}

fn text_form(title: &str, content: &str) -> reqwest::multipart::Form {
    reqwest::multipart::Form::new()
        .text("title", title.to_string())
        .text("content", content.to_string())
}

fn file_form(title: &str, name: &str, data: &[u8], mime: &str) -> reqwest::multipart::Form {
    reqwest::multipart::Form::new()
        .text("title", title.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(data.to_vec())
                .file_name(name.to_string())
                .mime_str(mime)
                .unwrap(),
        )
}

// ─── Managing ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_markdown_is_indexed_and_searchable() {
    let app = app(TAG);
    let _seam = toy_embeddings().await;
    // Python's autouse `_cleanup` deletes every "PYTEST %" document after each test; the search ranks
    // all documents, so the ones the other tests left must go first.
    sqlx::query("DELETE FROM ai_knowledge_docs WHERE title LIKE 'PYTEST %'")
        .execute(&mut app.db().await)
        .await
        .unwrap();
    let token = admin().await;
    let doc = upload(&token, text_form("PYTEST pricing", PRICING)).await;
    assert_eq!(doc["status"], "ready", "{doc}");
    assert!(doc["chunk_count"].as_i64().unwrap() >= 1 && doc["source_type"] == "markdown");

    let r = app
        .post(
            &token,
            &format!("{K}/search"),
            json!({"query": "what discount do distributors get on gateways"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let results = r.json()["results"].as_array().unwrap().clone();
    let first = &results[0];
    assert_eq!(first["title"], "PYTEST pricing");
    let keys: Vec<&str> = first
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["doc_id", "title", "content", "distance", "used"]);
    assert_eq!(
        first["used"],
        json!(first["distance"].as_f64().unwrap() <= 0.65)
    );
    let mut db = app.db().await;
    let used: i32 = sqlx::query_scalar("SELECT use_count FROM ai_knowledge_docs WHERE id = $1")
        .bind(Uuid::parse_str(doc["id"].as_str().unwrap()).unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert!(used >= 1);
}

#[tokio::test]
async fn test_a_markdown_file_upload_is_stored_privately() {
    let _seam = toy_embeddings().await;
    let token = admin().await;
    let doc = upload(
        &token,
        file_form(
            "PYTEST playbook",
            "pytest-playbook.md",
            PRICING.as_bytes(),
            "text/markdown",
        ),
    )
    .await;
    assert_eq!(doc["original_filename"], "pytest-playbook.md");
    assert_eq!(doc["status"], "ready");
    let id = doc["id"].as_str().unwrap();
    assert_eq!(
        stored_keys(id),
        vec![format!("ai-knowledge/{id}/pytest-playbook.md")]
    );
}

#[tokio::test]
async fn test_an_untitled_file_takes_its_name() {
    let _seam = toy_embeddings().await;
    let token = admin().await;
    let doc = upload(
        &token,
        file_form("", "dir/PYTEST notes.v2.md", b"# Notes", "text/markdown"),
    )
    .await;
    assert_eq!(doc["title"], "PYTEST notes.v2");
    assert_eq!(doc["original_filename"], "PYTEST notes.v2.md");
    assert_eq!(doc["original_size"], 7);
}

#[tokio::test]
async fn test_a_switched_off_document_is_not_used_and_an_edit_reindexes() {
    let app = app(TAG);
    let _seam = toy_embeddings().await;
    let token = admin().await;
    let doc = upload(&token, text_form("PYTEST off switch", PRICING)).await;
    let id = doc["id"].as_str().unwrap().to_string();
    let off = app
        .patch(&token, &format!("{K}/{id}"), json!({"is_active": false}))
        .await;
    assert_eq!(off.status, 200);
    let hits = app
        .post(
            &token,
            &format!("{K}/search"),
            json!({"query": "distributors discount gateways"}),
        )
        .await
        .json();
    assert!(!hits["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h["doc_id"] == json!(id)));

    let r = app
        .patch(
            &token,
            &format!("{K}/{id}"),
            json!({"is_active": true, "content": "Support hours are 08:00 to 17:00 JST on weekdays."}),
        )
        .await;
    assert_eq!(r.json()["status"], "ready", "{}", r.text());
    let mut db = app.db().await;
    let chunks: Vec<String> =
        sqlx::query_scalar("SELECT content FROM ai_knowledge_chunks WHERE doc_id = $1")
            .bind(Uuid::parse_str(&id).unwrap())
            .fetch_all(&mut db)
            .await
            .unwrap();
    assert!(chunks.iter().any(|c| c.contains("Support hours")));
    assert!(!chunks.iter().any(|c| c.contains("Distributors")));
}

#[tokio::test]
async fn test_an_unchanged_patch_writes_nothing() {
    let app = app(TAG);
    let _seam = toy_embeddings().await;
    let token = admin().await;
    let doc = upload(&token, text_form("PYTEST same", PRICING)).await;
    let id = doc["id"].as_str().unwrap();
    let r = app
        .patch(
            &token,
            &format!("{K}/{id}"),
            json!({"title": "PYTEST same", "content": PRICING}),
        )
        .await
        .json();
    assert_eq!(r["updated_at"], doc["updated_at"]);
}

#[tokio::test]
async fn test_deleting_removes_the_chunks_and_the_file() {
    let app = app(TAG);
    let _seam = toy_embeddings().await;
    let token = admin().await;
    let doc = upload(
        &token,
        file_form(
            "PYTEST delete me",
            "pytest-del.md",
            PRICING.as_bytes(),
            "text/markdown",
        ),
    )
    .await;
    let id = doc["id"].as_str().unwrap();
    assert_eq!(stored_keys(id).len(), 1);
    let r = app.delete(&token, &format!("{K}/{id}")).await;
    assert_eq!(r.status, 204);
    let mut db = app.db().await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_knowledge_chunks WHERE doc_id = $1")
        .bind(Uuid::parse_str(id).unwrap())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert!(
        stored_keys(id).is_empty(),
        "the stored original is removed too"
    );
}

#[tokio::test]
async fn test_reindex_rebuilds_the_chunks() {
    let app = app(TAG);
    let _seam = toy_embeddings().await;
    let token = admin().await;
    let doc = upload(&token, text_form("PYTEST reindex", PRICING)).await;
    let id = doc["id"].as_str().unwrap();
    let r = app
        .post(&token, &format!("{K}/{id}/reindex"), json!({}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    assert_eq!(body["status"], "ready");
    assert!(body.get("content").is_none());
}

// ─── test_ai_erp_guide.py ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_guide_documents_are_read_only_in_settings() {
    let app = app(TAG);
    let _seam = toy_embeddings().await;
    let token = app.superadmin().await.token;
    let doc_id: Uuid = app
        .on_server(async {
            let mut db = app.db().await;
            granary_services::ai::erp_guide::sync(&mut db)
                .await
                .unwrap();
            sqlx::query_scalar(
                "SELECT id FROM ai_knowledge_docs WHERE system_key = 'erp_guide:sales'",
            )
            .fetch_one(&mut db)
            .await
            .unwrap()
        })
        .await;
    let listed = app.get(&token, K).await.json();
    let item = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == json!(doc_id.to_string()))
        .unwrap()
        .clone();
    assert_eq!(item["system"], true);
    let r = app
        .patch(&token, &format!("{K}/{doc_id}"), json!({"content": "x"}))
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["detail"], "This is the ERP guide, built from the code: it can only be switched on or off. Add your own document for anything else.");
    let r = app.delete(&token, &format!("{K}/{doc_id}")).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["detail"], "This is the ERP guide, built from the code: switch it off instead (or turn off ai.erp_guide.enabled).");
    let off = app
        .patch(
            &token,
            &format!("{K}/{doc_id}"),
            json!({"is_active": false}),
        )
        .await;
    assert_eq!(off.status, 200);
    assert_eq!(off.json()["is_active"], false);
}
