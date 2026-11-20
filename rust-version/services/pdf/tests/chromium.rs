//! Ports of services/pdf/src/proposal.test.js, plus browser recovery. They need Chromium
//! (PUPPETEER_EXECUTABLE_PATH or a system chromium / google-chrome) and skip when there is none. One
//! browser at a time (a lock serialises the tests).

use std::path::PathBuf;

use granary_pdf::cdp::{executable, Browser};
use granary_pdf::hbs::Templates;
use granary_pdf::render::{PdfRenderer, RenderOptions};
use once_cell::sync::Lazy;
use serde_json::json;

static ONE_BROWSER: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

fn have_chromium() -> bool {
    let exe = executable();
    let ok = std::path::Path::new(&exe).exists();
    if !ok {
        eprintln!("skipped: no Chromium at {exe} (set PUPPETEER_EXECUTABLE_PATH)");
    }
    ok
}

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

#[tokio::test]
async fn the_proposal_renders_to_a_multi_page_a4_pdf_with_every_part() {
    if !have_chromium() {
        return;
    }
    let _one = ONE_BROWSER.lock().await;
    let labels = json!({"kind": "Solution Proposal", "requirement": "Requirement", "status": "Status", "notes": "Notes",
        "approved": "APPROVED ELECTRONICALLY", "awaiting": "AWAITING APPROVAL", "verify": "Verify with the QR code on this page", "draft": "DRAFT"});
    let data = json!({
        "language": "en", "title": "Office Laptop Rollout", "subtitle": "15 laptops, monitors and support, on-site", "labels": labels,
        "draft": false, "company": {"company_trade": "Acme Ltd"},
        "cover_rows": [["Prepared for", "Globex Corp"], ["Reference", "ACME/PRP-2026-10-00001 Rev. 1"]],
        "sections": [
            {"number": 1, "title": "Executive Summary", "blocks": [{"html": "<p><strong>15 laptops</strong> with monitors and a support plan.</p>".repeat(30)}]},
            {"number": 2, "title": "Requirement Coverage", "blocks": [{"coverage": {"legend": "Included = in scope.", "rows": [
                {"requirement": "Disk encryption", "status": "included", "status_label": "Included", "notes": "set up before delivery"}]}}]},
            {"number": 3, "title": "Commercial Summary", "blocks": [{"commercial": {"columns": [{"label": "Item"}, {"label": "Amount", "num": true}],
                "rows": [{"cells": [{"text": "LT-PRO-25: Laptop Pro 25"}, {"text": "USD 44,700", "num": true}]},
                         {"total": true, "cells": [{"text": "Total"}, {"text": "USD 44,700", "num": true}]}],
                "notes": ["Validity 30 days."]}}]},
            {"number": 4, "title": "Approval", "blocks": [{"stamp": {"approval": {"name": "Robert Smith", "title": "Owner", "date": "11 October 2026"}}}]}
        ]
    });
    let html = Templates::new(assets().join("templates"))
        .render(&json!("proposal"), &data)
        .unwrap();
    for s in [
        "Office Laptop Rollout",
        "Prepared for",
        "Requirement",
        "status-included",
        "USD 44,700",
        "APPROVED ELECTRONICALLY",
    ] {
        assert!(html.contains(s), "{s}");
    }
    let renderer = PdfRenderer::new(Default::default());
    let footer = json!({"title": "SOLUTION PROPOSAL", "line": "", "code": ""});
    let header = json!({"text": "Acme Ltd | Confidential"});
    let opts = || RenderOptions {
        allow_assets: true,
        footer: Some(&footer),
        header: Some(&header),
        javascript: true,
    };
    let pdf = renderer.render(json!(html), opts()).await.unwrap();
    assert_eq!(&pdf[..4], b"%PDF");
    let pages = regex::bytes::Regex::new(r"/Type\s*/Page[^s]")
        .unwrap()
        .find_iter(&pdf)
        .count();
    assert!(pages >= 3, "pages: {pages}");

    // A crashed browser is replaced on the next render (getBrowser checks isConnected).
    renderer.browser().await.unwrap().kill();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let again = renderer
        .render(json!("<p>after a crash</p>"), opts())
        .await
        .unwrap();
    assert_eq!(&again[..4], b"%PDF");
    renderer.shutdown().await;
}

#[tokio::test]
async fn pages_render_concurrently_on_one_browser() {
    if !have_chromium() {
        return;
    }
    let _one = ONE_BROWSER.lock().await;
    let renderer = PdfRenderer::new(Default::default());
    let jobs = (0..4).map(|i| {
        let r = &renderer;
        async move {
            r.render(
                json!(format!("<h1>Page {i}</h1>")),
                RenderOptions {
                    allow_assets: false,
                    footer: None,
                    header: None,
                    javascript: false,
                },
            )
            .await
        }
    });
    for pdf in futures_util::future::join_all(jobs).await {
        assert_eq!(&pdf.unwrap()[..4], b"%PDF");
    }
    renderer.shutdown().await;
    // The headless "new" launch mode (own profile) starts and closes cleanly.
    let b = Browser::launch(true, &["--no-sandbox"]).await.unwrap();
    b.close().await;
}

/// 23 Oct 2026: the proposal cover's letterhead has the quotation's width and place (Owner): 169 mm wide, 20.5 mm
/// from the left edge (edge to edge clipped the letterhead's own text), with the verified footer and the running
/// header still printed; page 2 keeps its 12 mm side margins.
#[tokio::test]
async fn the_proposal_cover_letterhead_matches_the_quotation() {
    use base64::Engine as _;
    if !have_chromium() {
        return;
    }
    let _one = ONE_BROWSER.lock().await;
    // a wide letterhead in green (the cover's divider line is maroon), as a data URI (as the backend sends it)
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="2480" height="350"><rect width="2480" height="350" fill="#1F7A2B"/></svg>"##;
    let uri = format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    );
    let labels = json!({"kind": "Solution Proposal", "draft": "DRAFT"});
    let data = json!({
        "language": "en", "title": "Letterhead width", "labels": labels, "draft": false,
        "company": {"company_trade": "Acme Ltd", "letterhead_url": uri},
        "cover_rows": [["Prepared for", "Globex Corp"]],
        "sections": [{"number": 1, "title": "Executive Summary",
                      "blocks": [{"html": "<p>Body text on the second page.</p>"}]}]
    });
    let html = Templates::new(assets().join("templates"))
        .render(&json!("proposal"), &data)
        .unwrap();
    let renderer = PdfRenderer::new(Default::default());
    let footer =
        json!({"title": "SOLUTION PROPOSAL", "line": "Scan the QR code", "code": "ABC123"});
    let header = json!({"text": "Acme Ltd | Confidential"});
    let pdf = renderer
        .render(
            json!(html),
            RenderOptions {
                allow_assets: true,
                footer: Some(&footer),
                header: Some(&header),
                javascript: false,
            },
        )
        .await
        .unwrap();
    renderer.shutdown().await;

    let doc = granary_services::pdftools::Document::open(&pdf).unwrap();
    assert!(doc.page_count() >= 2);
    // 10 px per mm
    let zoom = 10.0 * 25.4 / 72.0;
    let green = |p: &[u8]| p[1] > 90 && p[1] < 150 && p[0] < 60 && p[2] < 70;
    let page = |n: usize| {
        let text = doc.get_text(n).unwrap();
        let pix = doc.render_page(n, zoom).unwrap();
        (pix, text)
    };
    let span = |pix: &granary_services::pdftools::RgbImage| {
        // the widest run of letterhead pixels on any row: (first x, last x) in mm
        let (w, h) = (pix.width as usize, pix.height as usize);
        let s = &pix.samples;
        let mut best: Option<(usize, usize)> = None;
        for y in 0..h {
            let row = &s[y * w * 3..(y + 1) * w * 3];
            let xs: Vec<usize> = (0..w).filter(|x| green(&row[x * 3..x * 3 + 3])).collect();
            if let (Some(a), Some(b)) = (xs.first(), xs.last()) {
                if best.is_none_or(|(x0, x1)| b - a > x1 - x0) {
                    best = Some((*a, *b));
                }
            }
        }
        best.map(|(a, b)| (a as f64 / 10.0, b as f64 / 10.0, w as f64 / 10.0))
    };
    let (p1, text1) = page(0);
    let (left, right, width) = span(&p1).expect("the letterhead is drawn");
    assert!((width - 210.0).abs() < 1.0, "A4 width: {width}");
    assert!(
        (left - 20.5).abs() < 1.0,
        "letterhead starts {left} mm from the left edge (quotation: 20.5)"
    );
    assert!(
        (right - left - 169.0).abs() < 1.0,
        "letterhead is {} mm wide (quotation: 169)",
        right - left
    );
    assert!(
        text1.contains("ABC123") && text1.contains("Confidential"),
        "{text1}"
    );
    assert!(text1.contains("Letterhead width"));
    // page 2: the body keeps its side margins (no letterhead-coloured run near an edge)
    let (p2, text2) = page(1);
    assert!(text2.contains("Body text on the second page"), "{text2}");
    if let Some((l, r, w)) = span(&p2) {
        assert!(l >= 10.0 && r <= w - 10.0, "page 2 content at {l}..{r} mm");
    }
}
