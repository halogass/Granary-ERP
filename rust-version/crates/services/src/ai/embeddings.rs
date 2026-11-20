//! Port of backend/app/services/ai/embeddings.py: KB/SOP embeddings: chunk, embed, upsert kb_embeddings,
//! and cosine retrieval for RAG.
//!
//! Retrieval is filtered to published, non-private articles only (per product decision: all staff may
//! query all SOPs except `is_private` ones).
//!
//! Vectors: Python's pgvector sends `str(float(v))` text that Postgres parses into float4; here each f64
//! goes through its shortest repr too (`to_f32`), so the stored and compared values are the same.

use std::sync::Arc;

use granary_compat::text::pystr;
use granary_db::rows::knowledge::KBArticle;
use granary_db::rows::Vector;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::ai::base::{AiError, LLMProvider};
use crate::ai::registry::{get_ai_config, get_model, get_provider};
use crate::crm::followups::py_commit;

/// `_CHUNK_CHARS`: ~700 tokens is about 2800 chars; keep chunks comfortably under model limits.
pub const CHUNK_CHARS: usize = granary_compat::text::patterns::CHUNK_CHARS;

/// `chunk_text(text)`: section-aware chunking, paragraphs accumulated up to ~CHUNK_CHARS.
pub use granary_compat::text::patterns::chunk_text;

static BACKTICKS: Lazy<Regex> = Lazy::new(|| Regex::new("`{1,3}").unwrap());
static MD_MARKS: Lazy<Regex> = Lazy::new(|| Regex::new("[*_#>]+").unwrap());
static BLANKS: Lazy<Regex> = Lazy::new(|| Regex::new("[ \t]+").unwrap());

/// `_strip_md(text)`: light cleanup so embeddings focus on meaning, not markdown syntax.
pub fn strip_md(text: Option<&str>) -> String {
    let text = text.unwrap_or("");
    if text.is_empty() {
        return String::new();
    }
    let t = BACKTICKS.replace_all(text, "");
    let t = MD_MARKS.replace_all(&t, " ");
    let t = BLANKS.replace_all(&t, " ");
    pystr::strip(&t).to_string()
}

/// `article_text(article)`: flatten an article (title + summary + content + SOP steps) into indexable text.
pub fn article_text(article: &KBArticle) -> String {
    article_text_of(
        Some(article.title.as_str()),
        article.sop_code.as_deref(),
        article.summary.as_deref(),
        article.content.as_deref(),
        article.sop_steps.as_ref(),
    )
}

/// `article_text` on the fields it reads.
pub fn article_text_of(
    title: Option<&str>,
    sop_code: Option<&str>,
    summary: Option<&str>,
    content: Option<&str>,
    sop_steps: Option<&Value>,
) -> String {
    let mut parts: Vec<String> = vec![title.unwrap_or("").to_string()];
    if let Some(code) = sop_code.filter(|c| !c.is_empty()) {
        parts.push(format!("({code})"));
    }
    if let Some(s) = summary.filter(|s| !s.is_empty()) {
        parts.push(s.to_string());
    }
    if let Some(c) = content.filter(|c| !c.is_empty()) {
        parts.push(c.to_string());
    }
    if let Some(Value::Array(steps)) = sop_steps {
        for step in steps {
            if let Value::Object(m) = step {
                // str(step.get(k, "")): a key present with null prints "None".
                let seg = ["title", "description", "responsible"]
                    .iter()
                    .map(|k| match m.get(*k) {
                        None => String::new(),
                        Some(v) => crate::crm::attributes::py::str_of(v),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                if !pystr::strip(&seg).is_empty() {
                    parts.push(seg);
                }
            }
        }
    }
    let joined = parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    strip_md(Some(&joined))
}

/// f64 to the float4 Postgres stores for pgvector's text form (`str(float(v))`).
pub fn to_f32(v: f64) -> f32 {
    format!("{v}").parse::<f32>().unwrap_or(v as f32)
}

/// A Python list of floats as the pgvector value it is sent as.
pub fn to_vector(v: &[f64]) -> Vector {
    Vector(v.iter().map(|x| to_f32(*x)).collect())
}

/// `reindex_article(db, article, *, provider=None, emb_model=None)`: replace all embeddings for one
/// article. Returns the number of chunks written. Skips (and clears) articles that are not published or
/// are private.
pub async fn reindex_article(
    db: &mut PgConnection,
    article: &KBArticle,
    provider: Option<Arc<dyn LLMProvider>>,
    emb_model: Option<String>,
) -> Result<usize, AiError> {
    sqlx::query("DELETE FROM kb_embeddings WHERE kb_embeddings.article_id = $1")
        .bind(article.id)
        .execute(&mut *db)
        .await?;
    if article.is_private || article.status.as_deref() != Some("published") || article.is_deleted {
        py_commit(db).await?;
        return Ok(0);
    }
    let (provider, emb_model) = match provider {
        Some(p) => (p, emb_model.unwrap_or_default()),
        None => {
            let cfg = get_ai_config(&mut *db).await?;
            (
                get_provider(&cfg, "embedding", false)?,
                get_model(&cfg, "embedding")?,
            )
        }
    };
    let chunks = chunk_text(&article_text(article));
    if chunks.is_empty() {
        py_commit(db).await?;
        return Ok(0);
    }
    let vectors = provider.embed(&emb_model, &chunks).await?;
    for (i, (chunk, vec)) in chunks.iter().zip(vectors.iter()).enumerate() {
        sqlx::query(
            "INSERT INTO kb_embeddings (id, article_id, chunk_index, content, embedding, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, now(), now())",
        )
        .bind(Uuid::new_v4())
        .bind(article.id)
        .bind(i as i32)
        .bind(chunk)
        .bind(to_vector(vec))
        .execute(&mut *db)
        .await?;
    }
    py_commit(db).await?;
    Ok(chunks.len())
}

/// `reindex_all(db)`: re-embed every published, non-private article. Returns {"articles", "chunks"}.
pub async fn reindex_all(db: &mut PgConnection) -> Result<Value, AiError> {
    let cfg = get_ai_config(&mut *db).await?;
    let provider = get_provider(&cfg, "embedding", false)?;
    let emb_model = get_model(&cfg, "embedding")?;
    let rows: Vec<KBArticle> = sqlx::query_as(
        "SELECT * FROM kb_articles WHERE kb_articles.is_private = false AND kb_articles.status = 'published' \
         AND kb_articles.is_deleted = false",
    )
    .fetch_all(&mut *db)
    .await?;
    let mut total = 0;
    for art in &rows {
        total += reindex_article(
            &mut *db,
            art,
            Some(provider.clone()),
            Some(emb_model.clone()),
        )
        .await?;
    }
    Ok(json!({"articles": rows.len(), "chunks": total}))
}

/// One `search_sops` result.
#[derive(Clone, Debug, PartialEq)]
pub struct SopHit {
    pub article_id: String,
    pub title: String,
    pub sop_code: Option<String>,
    pub slug: String,
    pub content: String,
    pub distance: f64,
}

impl SopHit {
    /// The dict Python returns.
    pub fn to_json(&self) -> Value {
        json!({"article_id": self.article_id, "title": self.title, "sop_code": self.sop_code,
               "slug": self.slug, "content": self.content, "distance": self.distance})
    }
}

/// `search_sops(db, query, k=5)`: embed the query and return the top-k SOP chunks (non-private,
/// published).
pub async fn search_sops(
    db: &mut PgConnection,
    query: &str,
    k: i64,
) -> Result<Vec<SopHit>, AiError> {
    let cfg = get_ai_config(&mut *db).await?;
    let provider = get_provider(&cfg, "embedding", false)?;
    let emb_model = get_model(&cfg, "embedding")?;
    let q = provider.embed(&emb_model, &[query.to_string()]).await?;
    // `[0]` of an empty answer is an IndexError in Python.
    let q_vec = q
        .first()
        .ok_or_else(|| AiError::Other("list index out of range".into()))?;
    let rows: Vec<(Uuid, String, String, Option<String>, String, f64)> = sqlx::query_as(
        "SELECT kb_embeddings.article_id, kb_embeddings.content, kb_articles.title, kb_articles.sop_code, \
         kb_articles.slug, kb_embeddings.embedding <=> $1 AS dist \
         FROM kb_embeddings JOIN kb_articles ON kb_articles.id = kb_embeddings.article_id \
         WHERE kb_articles.is_private = false AND kb_articles.status = 'published' AND kb_articles.is_deleted = false \
         ORDER BY dist LIMIT $2",
    )
    .bind(to_vector(q_vec))
    .bind(k)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(article_id, content, title, sop_code, slug, dist)| SopHit {
                article_id: article_id.to_string(),
                title,
                sop_code,
                slug,
                content,
                distance: dist,
            },
        )
        .collect())
}
