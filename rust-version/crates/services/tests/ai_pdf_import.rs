//! Tests for crates/services/src/ai/pdf_import.rs (backend/app/services/ai/pdf_import.py). The Python
//! suite only stubs `convert_pdf_to_markdown` (test_ai_knowledge.py) and checks that it asks for the
//! verbatim provider (test_ai_style.py); these exercise the conversion itself with a stub model, and
//! the pure helpers against outputs recorded from Python.
mod support;

use std::sync::Arc;

use async_trait::async_trait;
use granary_services::ai::base::{AiError, ChatRequest, ChatResult, FileRequest, LLMProvider};
use granary_services::ai::pdf_import::{self, PdfImportFail};
use granary_services::ai::registry::{self, seam};
use granary_services::pdftools::{Document, Point, SaveOptions, TextOptions};
use serde_json::json;

#[test]
fn clean_and_batches_match_python() {
    let cases = [
        ("", ""),
        ("  hi  ", "hi"),
        ("```markdown\n# T\n```", "# T"),
        ("```md\n\n a \n```\n", "a"),
        ("```\nx\ny\n```", "x\ny"),
        ("pre\n```\nx\n```", "pre\n```\nx\n```"),
        ("```json\nx\n```", "```json\nx\n```"),
        ("```\n```", "```\n```"),
        ("```\n\n```", ""),
        ("```markdown  \n# a\n``` ", "# a"),
    ];
    for (i, o) in cases {
        assert_eq!(pdf_import::_clean(i), o, "{i:?}");
    }
    assert_eq!(pdf_import::_batches(0, 12), vec![]);
    assert_eq!(pdf_import::_batches(1, 12), vec![(0, 0)]);
    assert_eq!(pdf_import::_batches(12, 12), vec![(0, 11)]);
    assert_eq!(pdf_import::_batches(13, 12), vec![(0, 11), (12, 12)]);
    assert_eq!(pdf_import::_batches(7, 6), vec![(0, 5), (6, 6)]);
    assert_eq!(pdf_import::_batches(150, 12).last(), Some(&(144, 149)));
}

/// A PDF whose pages carry `texts` (an empty string = a page with no text layer).
fn pdf(texts: &[&str]) -> Vec<u8> {
    let mut d = Document::new().unwrap();
    for t in texts {
        let p = d.new_page(595.0, 842.0).unwrap();
        if !t.is_empty() {
            d.insert_text(p, Point::new(50.0, 72.0), t, &TextOptions::default())
                .unwrap();
        }
    }
    d.tobytes(SaveOptions::default()).unwrap()
}

struct TextOnly;

#[async_trait]
impl LLMProvider for TextOnly {
    fn name(&self) -> &str {
        "textonly"
    }
    async fn chat(&self, _m: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
        let n = req.messages[0]["content"].as_str().unwrap().len();
        Ok(ChatResult::text(format!(
            "```markdown\n# Converted {n}\n```"
        )))
    }
}

struct Vision;

#[async_trait]
impl LLMProvider for Vision {
    fn name(&self) -> &str {
        "vision"
    }
    async fn chat(&self, _m: &str, _req: &ChatRequest) -> Result<ChatResult, AiError> {
        Ok(ChatResult::text("# Text part"))
    }
    async fn analyze_file(&self, _m: &str, req: &FileRequest) -> Result<ChatResult, AiError> {
        let pages = Document::open(&req.file_bytes).unwrap().page_count();
        Ok(ChatResult::text(format!("# Scan with {pages} page(s)")))
    }
}

#[tokio::test]
async fn text_pages_are_converted_in_one_call_with_the_python_prompt() {
    let pool = support::fresh_db("pdf_import_text").await;
    let mut db = pool.acquire().await.unwrap();
    let line = "This page has a real text layer with plenty of characters on it, enough for the threshold.";
    let long = format!("{line}\n{line}");
    let bytes = pdf(&[&long, &long]);
    let pages = pdf_import::extract_pages(&bytes).unwrap();
    assert_eq!(pages.len(), 2);
    assert!(pages[0].starts_with("This page has a real text layer"));

    let stub = seam::fake_use_ai(Arc::new(TextOnly), "stub-model");
    let s = seam::Seam::install().await;
    let st = stub.clone();
    s.use_ai(move |job, opts| {
        // a transcription keeps the author's punctuation: the verbatim provider
        assert!(
            opts.verbatim,
            "pdf_import must ask for the verbatim provider"
        );
        Ok(st.client(job))
    });
    let out = pdf_import::convert_pdf_to_markdown(&mut db, &bytes, None)
        .await
        .unwrap();
    drop(s);
    assert_eq!(stub.job().as_deref(), Some("pdf_import"));
    let calls = stub.calls();
    assert_eq!(calls.len(), 1);
    let req = calls[0].chat().unwrap();
    assert_eq!(req.system, pdf_import::_SYSTEM);
    assert_eq!(req.temperature, Some(0.1));
    let body = pages.join("\n\n");
    let prompt = pdf_import::_text_prompt(1, 2, &body);
    assert_eq!(
        req.messages,
        vec![json!({"role": "user", "content": prompt})]
    );
    assert_eq!(
        out,
        json!({"markdown": format!("# Converted {}", prompt.len()), "pages_converted": 2, "total_pages": 2,
               "truncated": false, "mode": "text"})
    );
}

#[tokio::test]
async fn scanned_pages_need_a_provider_that_reads_files() {
    let pool = support::fresh_db("pdf_import_scan").await;
    let mut db = pool.acquire().await.unwrap();
    let bytes = pdf(&["", ""]);
    let stub = seam::fake_use_ai(Arc::new(TextOnly), "stub-model");
    let _s = stub.install().await;
    match pdf_import::convert_pdf_to_markdown(&mut db, &bytes, None).await {
        Err(PdfImportFail::Import(e)) => assert_eq!(
            e.0,
            "Pages 1\u{2013}2 are scanned images, and the active AI provider (textonly) cannot read files. \
             Switch the provider in AI settings, or paste the text in manually."
        ),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn mixed_documents_send_the_scanned_pages_as_a_pdf() {
    let pool = support::fresh_db("pdf_import_mixed").await;
    let mut db = pool.acquire().await.unwrap();
    let line =
        "A text page with enough characters to count as a real text layer for the importer here.";
    let long = format!("{line}\n{line}");
    let mut texts: Vec<&str> = vec![long.as_str(); 12];
    texts.push("");
    texts.push("");
    let bytes = pdf(&texts);
    let stub = seam::fake_use_ai(Arc::new(Vision), "stub-model");
    let _s = stub.install().await;
    let out = pdf_import::convert_pdf_to_markdown(&mut db, &bytes, None)
        .await
        .unwrap();
    assert_eq!(out["mode"], json!("mixed"));
    assert_eq!(
        out["markdown"],
        json!("# Text part\n\n# Scan with 2 page(s)")
    );
    let calls = stub.calls();
    let file = calls[1].file().unwrap();
    assert_eq!(file.prompt, pdf_import::_vision_prompt(13, 14));
    assert_eq!(file.system, pdf_import::_SYSTEM);
    assert_eq!(file.mime_type, "application/pdf");
}

#[tokio::test]
async fn disabled_and_unreadable_pdfs_fail_with_python_texts() {
    let pool = support::fresh_db("pdf_import_fail").await;
    let mut db = pool.acquire().await.unwrap();
    let mut cfg = registry::get_ai_config(&mut db).await.unwrap();
    cfg.insert("ai.feature.doc_intel".into(), "false".into());
    let s = seam::Seam::install().await;
    s.get_ai_config(move || Ok(cfg.clone()));
    match pdf_import::convert_pdf_to_markdown(&mut db, b"%PDF", None).await {
        Err(PdfImportFail::Import(e)) => {
            assert_eq!(e.0, "AI document intelligence is disabled in settings")
        }
        other => panic!("{other:?}"),
    }
    drop(s);
    assert_eq!(
        pdf_import::extract_pages(b"not a pdf").unwrap_err().0,
        "Could not open the PDF: Failed to open stream"
    );
}
