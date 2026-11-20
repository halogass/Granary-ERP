//! Port of backend/app/services/ai/pdf_import.py: PDF -> Markdown for the Knowledge Base.
//!
//! Two stages, so both digital and scanned documents work:
//!   1. MuPDF (crate::pdftools, the PyMuPDF port) pulls the text layer: free, fast, no tokens.
//!   2. Page ranges with (almost) no text layer are scans/photos, so those pages are cut
//!      into a small PDF and handed to the multimodal model instead.
//!
//! Either way the text ends at the same LLM step, which returns clean Markdown.
//!
//! The caller stores the result as the article body; indexing happens later, when a human
//! publishes the article (embeddings only ever cover published, non-private articles).

use std::collections::BTreeSet;

use granary_compat::text::pystr;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::ai::base::{AiError, ChatRequest, FileRequest};
use crate::ai::registry::{self, feature_enabled, UseAiOptions};
use crate::pdftools::{Document, SaveOptions};

// Cost/latency guardrails. A document longer than the cap is converted up to the cap
// and flagged as truncated rather than failing outright.
pub const PAGE_CAP: usize = 150;
/// pages of extracted text per model call
pub const TEXT_BATCH_PAGES: usize = 12;
/// pages per multimodal call (whole pages are sent as PDF)
pub const VISION_BATCH_PAGES: usize = 6;
/// below this average, treat the range as scanned
pub const MIN_TEXT_CHARS_PER_PAGE: usize = 120;

pub const _SYSTEM: &str = concat!(
    "You convert source documents into clean, faithful Markdown for an internal company ",
    "knowledge base. Reproduce the document's own content and structure \u{2014} never summarise, ",
    "never invent, never add commentary of your own."
);

pub const _RULES: &str = concat!(
    "Rules:\n",
    "- Output Markdown only. No preamble, no ``` fences around the whole answer.\n",
    "- Keep the document's original language.\n",
    "- Use #/##/### for the document's own heading hierarchy.\n",
    "- Render tables as Markdown tables. Keep lists as lists, and keep numbering.\n",
    "- Preserve figures, dates, amounts, codes and names EXACTLY as written.\n",
    "- Drop page headers/footers, page numbers and repeated watermarks.\n",
    "- If something is unreadable, write [illegible] rather than guessing.\n",
);

/// `_TEXT_PROMPT.format(first=, last=, body=)`.
pub fn _text_prompt(first: usize, last: usize, body: &str) -> String {
    format!(
        "Below is text extracted from pages {first}\u{2013}{last} of a PDF. The extraction has lost the \
         formatting; restore it as Markdown.\n\n{_RULES}\n--- EXTRACTED TEXT ---\n{body}"
    )
}

/// `_VISION_PROMPT.format(first=, last=)`.
pub fn _vision_prompt(first: usize, last: usize) -> String {
    format!(
        "This file contains pages {first}\u{2013}{last} of a larger document. Transcribe them into \
         Markdown.\n\n{_RULES}"
    )
}

/// Anything that should be shown to the user on the article as a failure reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfImportError(pub String);

impl std::fmt::Display for PdfImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PdfImportError {}

/// What `convert_pdf_to_markdown` raises: `PdfImportError`, or another exception Python let
/// propagate (a provider SDK error, a database error).
#[derive(Debug)]
pub enum PdfImportFail {
    Import(PdfImportError),
    Other(AiError),
}

impl std::fmt::Display for PdfImportFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfImportFail::Import(e) => write!(f, "{e}"),
            PdfImportFail::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PdfImportFail {}

impl From<PdfImportError> for PdfImportFail {
    fn from(e: PdfImportError) -> Self {
        PdfImportFail::Import(e)
    }
}

impl From<AiError> for PdfImportFail {
    fn from(e: AiError) -> Self {
        PdfImportFail::Other(e)
    }
}

fn import_err(s: impl Into<String>) -> PdfImportFail {
    PdfImportFail::Import(PdfImportError(s.into()))
}

/// Text layer of each page (empty string for pages that have none).
pub fn extract_pages(pdf_bytes: &[u8]) -> Result<Vec<String>, PdfImportError> {
    let mut doc = Document::open(pdf_bytes)
        .map_err(|e| PdfImportError(format!("Could not open the PDF: {e}")))?;
    if doc.is_encrypted() && !doc.authenticate("") {
        return Err(PdfImportError("The PDF is password-protected".into()));
    }
    let mut out = vec![];
    for pno in 0..doc.page_count() {
        // Not identical: a page whose text extraction fails raised out of PyMuPDF (a 500 for the
        // caller); here that page reads as empty.
        let text = doc.get_text(pno).unwrap_or_default();
        out.push(pystr::strip(&text).to_string());
    }
    Ok(out)
}

/// A new PDF holding only pages [first, last] (0-based, inclusive).
pub fn _subset_pdf(pdf_bytes: &[u8], first: usize, last: usize) -> Result<Vec<u8>, String> {
    let src = Document::open(pdf_bytes).map_err(|e| e.to_string())?;
    let mut out = Document::new().map_err(|e| e.to_string())?;
    out.insert_pdf(&src, first, last)
        .map_err(|e| e.to_string())?;
    out.tobytes(SaveOptions::default())
        .map_err(|e| e.to_string())
}

/// Contiguous (first, last) page ranges, 0-based inclusive.
pub fn _batches(total: usize, size: usize) -> Vec<(usize, usize)> {
    (0..total)
        .step_by(size)
        .map(|start| (start, (start + size).min(total) - 1))
        .collect()
}

static FENCED: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"(?s)\A```(?:markdown|md)?\s*\n(.*)\n```\z").unwrap());

/// Strip a whole-answer code fence and any 'here is the markdown' preamble.
/// (`re.match(r"^```(?:markdown|md)?\s*\n(.*)\n```$", text, re.DOTALL)`: `$` also matches before a
/// final newline, but the text is stripped first so there is none; `\s` is Unicode in both.)
pub fn _clean(md: &str) -> String {
    let text = pystr::strip(md).to_string();
    if let Some(c) = FENCED.captures(&text) {
        return pystr::strip(c.get(1).map(|m| m.as_str()).unwrap_or("")).to_string();
    }
    text
}

/// `str(e.detail)` of an HTTPException-like AiError, or None when it is not one.
fn http_detail(e: &AiError) -> Option<String> {
    if !e.is_http_exception() {
        return None;
    }
    Some(match e {
        AiError::Ai { detail, .. } => detail.clone(),
        AiError::Api(granary_core::errors::ApiError::Http { detail, .. }) => match detail {
            Value::String(s) => s.clone(),
            other => {
                granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(other))
            }
        },
        other => other.to_string(),
    })
}

/// Convert a PDF into Markdown. Fails with PdfImportError carrying a user-facing message.
pub async fn convert_pdf_to_markdown(
    db: &mut PgConnection,
    pdf_bytes: &[u8],
    user_id: Option<Uuid>,
) -> Result<Value, PdfImportFail> {
    let cfg = registry::get_ai_config(db).await?;
    if !feature_enabled(&cfg, "doc_intel") {
        return Err(import_err(
            "AI document intelligence is disabled in settings",
        ));
    }

    // a transcription keeps the author's punctuation: the verbatim (unstyled) provider
    let provider = match registry::use_ai(
        db,
        "pdf_import",
        UseAiOptions::user(user_id).with_cfg(cfg.clone()).verbatim(),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => match http_detail(&e) {
            Some(d) => return Err(import_err(d)),
            None => return Err(e.into()),
        },
    };

    let pages = extract_pages(pdf_bytes)?;
    let total_pages = pages.len();
    if total_pages == 0 {
        return Err(import_err("The PDF has no pages"));
    }

    let converted = total_pages.min(PAGE_CAP);
    let truncated = total_pages > PAGE_CAP;

    let mut parts: Vec<String> = vec![];
    let mut modes: BTreeSet<&str> = BTreeSet::new();

    for (first, last) in _batches(converted, TEXT_BATCH_PAGES) {
        let window = &pages[first..=last];
        // len() of a Python str counts code points
        let chars: usize = window.iter().map(|p| p.chars().count()).sum();

        if chars >= MIN_TEXT_CHARS_PER_PAGE * window.len() {
            let body = window.join("\n\n");
            let req = ChatRequest::new(
                _SYSTEM,
                vec![json!({"role": "user", "content": _text_prompt(first + 1, last + 1, &body)})],
            )
            .temperature(0.1);
            let result = match provider.chat(db, req).await {
                Ok(r) => r,
                Err(e) => match http_detail(&e) {
                    Some(d) => return Err(import_err(d)),
                    None => return Err(e.into()),
                },
            };
            modes.insert("text");
            parts.push(_clean(&result.text));
            continue;
        }

        // No usable text layer: these pages are images. Send them to the model as PDF.
        for (vfirst, vlast) in _batches(last - first + 1, VISION_BATCH_PAGES) {
            let (afirst, alast) = (first + vfirst, first + vlast);
            // Not identical: a MuPDF failure while cutting the pages propagated from PyMuPDF.
            let subset = _subset_pdf(pdf_bytes, afirst, alast)
                .map_err(|e| PdfImportFail::Other(AiError::Other(e)))?;
            let req = FileRequest::new(
                _vision_prompt(afirst + 1, alast + 1),
                subset,
                "application/pdf",
            )
            .system(_SYSTEM);
            let result = match provider.analyze_file(db, req).await {
                Ok(r) => r,
                Err(AiError::NotImplemented(why)) if !why.is_empty() => {
                    return Err(import_err(format!(
                        "Pages {}\u{2013}{} are scanned images: {why}",
                        afirst + 1,
                        alast + 1
                    )))
                }
                Err(AiError::NotImplemented(_)) => {
                    return Err(import_err(format!(
                        "Pages {}\u{2013}{} are scanned images, and the active AI \
                         provider ({}) cannot read files. Switch the provider in AI \
                         settings, or paste the text in manually.",
                        afirst + 1,
                        alast + 1,
                        provider.name()
                    )))
                }
                Err(e) => match http_detail(&e) {
                    Some(d) => return Err(import_err(d)),
                    None => return Err(e.into()),
                },
            };
            modes.insert("vision");
            parts.push(_clean(&result.text));
        }
    }

    let joined = parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut markdown = pystr::strip(&joined).to_string();
    if markdown.is_empty() {
        return Err(import_err("The converter produced no text from this PDF"));
    }

    if truncated {
        markdown += &format!(
            "\n\n---\n\n> **Note:** only the first {PAGE_CAP} of {total_pages} pages were \
             converted. Split the source document to import the rest.\n"
        );
    }

    let mode = if modes.len() > 1 {
        "mixed"
    } else {
        modes.iter().next().copied().unwrap_or("text")
    };
    Ok(json!({
        "markdown": markdown,
        "pages_converted": converted,
        "total_pages": total_pages,
        "truncated": truncated,
        "mode": mode,
    }))
}
