//! Port of backend/app/services/ai/knowledge.py: AI knowledge, documents only the AI reads.
//!
//! Admins upload Markdown or PDF in Settings → Nary & AI → AI knowledge. Employees have no page, list or API for
//! them; every AI feature reads them through this module:
//!
//!   * `search`: the chunks nearest a question (used by the assistant's tool and MCP agents);
//!   * `context_block`: a ready prompt section for the features that build their own prompt. It never
//!     raises: a missing embedding provider or an empty library just means no extra context.

use std::collections::BTreeSet;

use granary_compat::text::pystr;
use granary_db::rows::ai::AIKnowledgeDoc;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::ai::base::AiError;
use crate::ai::embeddings::{chunk_text, to_vector};
use crate::ai::registry::{get_ai_config, get_model, get_provider};
use crate::crm::followups::py_commit;

pub const DEFAULT_K: i64 = 5;
/// For context added automatically (the model didn't ask): cosine distance above which a chunk is too far
/// from the question to be worth the tokens. A starting value.
pub const MAX_DISTANCE: f64 = 0.65;
pub const CHUNK_CHARS_IN_PROMPT: usize = 1500;
pub const PDF_MAX_BYTES: usize = 25 * 1024 * 1024;

/// `KnowledgeError`: shown to the admin, the document could not be read or indexed.
#[derive(Clone, Debug, PartialEq)]
pub struct KnowledgeError(pub String);

impl std::fmt::Display for KnowledgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KnowledgeError {}

/// What `index_doc` / `pdf_to_markdown` can raise: a KnowledgeError, or anything else (`str(exc)` kept).
#[derive(Debug)]
pub enum IndexError {
    Knowledge(KnowledgeError),
    Other(AiError),
}

impl From<KnowledgeError> for IndexError {
    fn from(e: KnowledgeError) -> Self {
        IndexError::Knowledge(e)
    }
}
impl From<AiError> for IndexError {
    fn from(e: AiError) -> Self {
        IndexError::Other(e)
    }
}
impl From<sqlx::Error> for IndexError {
    fn from(e: sqlx::Error) -> Self {
        IndexError::Other(AiError::from(e))
    }
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ─── Indexing ────────────────────────────────────────────────────────────────

/// `pdf_to_markdown(db, pdf_bytes, *, user_id=None)`: (markdown, how). The AI converter keeps tables and
/// headings; when it is off or fails, the plain text layer is used instead.
pub async fn pdf_to_markdown(
    db: &mut PgConnection,
    pdf_bytes: &[u8],
    user_id: Option<Uuid>,
) -> Result<(String, String), IndexError> {
    use crate::ai::pdf_import::{convert_pdf_to_markdown, extract_pages, PdfImportFail};
    match convert_pdf_to_markdown(&mut *db, pdf_bytes, user_id).await {
        Ok(out) => {
            if let Some(md) = out.get("markdown").and_then(Value::as_str) {
                if !pystr::strip(md).is_empty() {
                    return Ok((md.to_string(), "converted".into()));
                }
            }
        }
        Err(PdfImportFail::Import(exc)) => {
            tracing::info!(reason = %exc, "ai_knowledge.pdf_convert_fallback")
        }
        Err(exc) => {
            tracing::warn!(error = %cut(&exc.to_string(), 200), "ai_knowledge.pdf_convert_failed")
        }
    }
    let pages = extract_pages(pdf_bytes)
        .map_err(|exc| KnowledgeError(format!("This PDF could not be read: {exc}")))?;
    let text = pages
        .iter()
        .map(|p| pystr::strip(p))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() {
        return Err(KnowledgeError(
            "This PDF has no readable text (a scan needs AI document conversion switched on in Settings → Nary & AI → AI features).".into(),
        )
        .into());
    }
    Ok((text, "text".into()))
}

/// `index_doc(db, doc)`: (re)build a document's chunks and embeddings. Returns the chunk count. Sets
/// `doc.chunk_count` (written with the document by `process`).
pub async fn index_doc(
    db: &mut PgConnection,
    doc: &mut AIKnowledgeDoc,
) -> Result<usize, IndexError> {
    sqlx::query("DELETE FROM ai_knowledge_chunks WHERE ai_knowledge_chunks.doc_id = $1")
        .bind(doc.id)
        .execute(&mut *db)
        .await?;
    let text = format!(
        "# {}\n\n{}\n\n{}",
        doc.title,
        doc.description.as_deref().unwrap_or(""),
        doc.content.as_deref().unwrap_or("")
    );
    let chunks = chunk_text(pystr::strip(&text));
    if chunks.is_empty() {
        return Err(KnowledgeError("The document is empty.".into()).into());
    }
    let cfg = get_ai_config(&mut *db).await?;
    let provider = get_provider(&cfg, "embedding", false)?;
    let model = get_model(&cfg, "embedding")?;
    let vectors = match provider.embed(&model, &chunks).await {
        Ok(v) => v,
        Err(exc) => {
            return Err(KnowledgeError(format!(
                "The embedding provider failed: {}",
                cut(&exc.to_string(), 200)
            ))
            .into())
        }
    };
    for (i, (chunk, vec)) in chunks.iter().zip(vectors.iter()).enumerate() {
        sqlx::query(
            "INSERT INTO ai_knowledge_chunks (id, doc_id, chunk_index, content, embedding, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, now(), now())",
        )
        .bind(Uuid::new_v4())
        .bind(doc.id)
        .bind(i as i32)
        .bind(chunk)
        .bind(to_vector(vec))
        .execute(&mut *db)
        .await?;
    }
    doc.chunk_count = chunks.len() as i32;
    Ok(chunks.len())
}

/// `process(db, doc, *, pdf_bytes=None, user_id=None)`: convert (PDF) and index a document, recording the
/// outcome on it (content, chunk_count, status, error are written to its row). Never fails on the
/// document; only a failing write of the outcome itself is returned.
pub async fn process(
    db: &mut PgConnection,
    doc: &mut AIKnowledgeDoc,
    pdf_bytes: Option<&[u8]>,
    user_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    let outcome: Result<(), IndexError> = async {
        if doc.source_type == "pdf" {
            if let Some(bytes) = pdf_bytes {
                let (content, _how) = pdf_to_markdown(&mut *db, bytes, user_id).await?;
                doc.content = Some(content);
            }
        }
        index_doc(&mut *db, doc).await?;
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => {
            doc.status = "ready".into();
            doc.error = None;
        }
        Err(IndexError::Knowledge(e)) => {
            doc.status = "failed".into();
            doc.error = Some(e.0);
        }
        Err(IndexError::Other(e)) => {
            tracing::error!(doc = %doc.id, error = %e, "ai_knowledge.process_failed");
            doc.status = "failed".into();
            doc.error = Some(format!("Indexing failed: {}", cut(&e.to_string(), 300)));
        }
    }
    // The unit of work flushes the document's changed attributes (onupdate: updated_at).
    sqlx::query(
        "UPDATE ai_knowledge_docs SET content = $2, chunk_count = $3, status = $4, error = $5, updated_at = now() \
         WHERE ai_knowledge_docs.id = $1",
    )
    .bind(doc.id)
    .bind(&doc.content)
    .bind(doc.chunk_count)
    .bind(&doc.status)
    .bind(&doc.error)
    .execute(&mut *db)
    .await?;
    py_commit(db).await
}

// ─── Retrieval ───────────────────────────────────────────────────────────────

/// One `search` result: {doc_id, title, content, distance}.
#[derive(Clone, Debug, PartialEq)]
pub struct KnowledgeHit {
    pub doc_id: String,
    pub title: String,
    pub content: String,
    pub distance: f64,
}

impl KnowledgeHit {
    pub fn to_json(&self) -> Value {
        json!({"doc_id": self.doc_id, "title": self.title, "content": self.content, "distance": self.distance})
    }
}

/// `search(db, query, k=DEFAULT_K, *, max_distance=None, customer_safe_only=False)`: nearest chunks from
/// active, ready documents. With `max_distance`, farther chunks are dropped; without it the model judges
/// relevance. `customer_safe_only`: just the documents marked safe for a customer conversation (Nary).
pub async fn search(
    db: &mut PgConnection,
    query: &str,
    k: i64,
    max_distance: Option<f64>,
    customer_safe_only: bool,
) -> Result<Vec<KnowledgeHit>, AiError> {
    let query = pystr::strip(query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let cfg = get_ai_config(&mut *db).await?;
    let provider = get_provider(&cfg, "embedding", false)?;
    let model = get_model(&cfg, "embedding")?;
    let q = provider.embed(&model, &[cut(query, 4000)]).await?;
    let q_vec = q
        .first()
        .ok_or_else(|| AiError::Other("list index out of range".into()))?;
    let safe = if customer_safe_only {
        " AND ai_knowledge_docs.customer_safe = true"
    } else {
        ""
    };
    let rows: Vec<(Uuid, String, String, f64)> = sqlx::query_as(&format!(
        "SELECT ai_knowledge_chunks.doc_id, ai_knowledge_chunks.content, ai_knowledge_docs.title, \
         ai_knowledge_chunks.embedding <=> $1 AS dist \
         FROM ai_knowledge_chunks JOIN ai_knowledge_docs ON ai_knowledge_docs.id = ai_knowledge_chunks.doc_id \
         WHERE ai_knowledge_docs.is_active = true AND ai_knowledge_docs.status = 'ready'{safe} \
         ORDER BY dist LIMIT $2"
    ))
    .bind(to_vector(q_vec))
    .bind(k)
    .fetch_all(&mut *db)
    .await?;
    let hits: Vec<KnowledgeHit> = rows
        .into_iter()
        .filter(|r| max_distance.map(|m| r.3 <= m).unwrap_or(true))
        .map(|(doc_id, content, title, dist)| KnowledgeHit {
            doc_id: doc_id.to_string(),
            title,
            content,
            distance: dist,
        })
        .collect();
    let ids: BTreeSet<Uuid> = hits
        .iter()
        .filter_map(|h| Uuid::parse_str(&h.doc_id).ok())
        .collect();
    mark_used(&ids, &mut *db).await;
    Ok(hits)
}

/// `_mark_used(doc_ids, db)`: a counter must never break an answer.
async fn mark_used(doc_ids: &BTreeSet<Uuid>, db: &mut PgConnection) {
    if doc_ids.is_empty() {
        return;
    }
    let ids: Vec<Uuid> = doc_ids.iter().copied().collect();
    let res = sqlx::query(
        "UPDATE ai_knowledge_docs SET use_count = ai_knowledge_docs.use_count + 1, last_used_at = $2, \
         updated_at = now() WHERE ai_knowledge_docs.id = ANY($1)",
    )
    .bind(&ids)
    .bind(chrono::Utc::now())
    .execute(&mut *db)
    .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, "ai_knowledge.mark_used_failed");
    }
}

/// `has_documents(db)`.
pub async fn has_documents(db: &mut PgConnection) -> Result<bool, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT ai_knowledge_docs.id FROM ai_knowledge_docs WHERE ai_knowledge_docs.is_active = true \
         AND ai_knowledge_docs.status = 'ready' LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// The prompt section for `hits` (the end of `context_block`).
pub fn format_block(hits: &[KnowledgeHit]) -> String {
    if hits.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = hits
        .iter()
        .map(|h| format!("[{}]\n{}", h.title, cut(&h.content, CHUNK_CHARS_IN_PROMPT)))
        .collect();
    format!(
        "\n\nCOMPANY KNOWLEDGE FOR AI (internal guidance, price rules, product facts — follow it \
         where it applies; it outranks general knowledge):\n{}",
        parts.join("\n\n")
    )
}

/// `context_block(db, query, k=4)`: a prompt section with the company's AI knowledge relevant to `query`,
/// or "". Never fails.
pub async fn context_block(db: &mut PgConnection, query: &str, k: i64) -> String {
    let hits = async {
        if !has_documents(&mut *db).await? {
            return Ok::<_, AiError>(Vec::new());
        }
        search(&mut *db, query, k, Some(MAX_DISTANCE), false).await
    }
    .await;
    match hits {
        Ok(h) => format_block(&h),
        Err(e) => {
            tracing::warn!(error = %e, "ai_knowledge.context_failed");
            String::new()
        }
    }
}
