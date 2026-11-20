//! Port of backend/app/services/pdf.py: the client of the Puppeteer PDF sidecar
//! (`POST {PDF_SERVICE_URL}/render`, Bearer `PDF_SERVICE_TOKEN` when set, 30 s timeout).
//! Errors are httpx's (see `crate::whatsapp::HttpxError`); Python lets them propagate (a 500 unless the
//! caller catches them).

use std::time::Duration;

use once_cell::sync::Lazy;
use serde_json::{Map, Value};

use crate::whatsapp::{
    httpx_client, httpx_json_body, httpx_url, py_truthy, HttpxError, HttpxResponse,
};

/// 10 Nov 2026: longer than the PDF service's own limits (HTML 20 s, images and fonts 30 s, printing 30 s), so the
/// service ends a stuck render itself and says why, instead of this call hanging up first (its work was then
/// cancelled without a log line). Under Cloudflare's 100 s.
const PDF_TIMEOUT: Duration = Duration::from_secs(90);

static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| httpx_client(PDF_TIMEOUT));

/// The JSON body `generate_pdf` posts: `{"template", "data", "footer"?, "header"?}` (footer/header only
/// when truthy).
pub fn render_body(
    template: &str,
    data: &Value,
    footer: Option<&Value>,
    header: Option<&Value>,
) -> Value {
    let mut body = Map::new();
    body.insert("template".into(), Value::String(template.to_string()));
    body.insert("data".into(), data.clone());
    if let Some(f) = footer.filter(|f| py_truthy(f)) {
        body.insert("footer".into(), f.clone());
    }
    if let Some(h) = header.filter(|h| py_truthy(h)) {
        body.insert("header".into(), h.clone());
    }
    Value::Object(body)
}

/// `generate_pdf(template, data, footer=None, header=None)`: the raw PDF bytes. `footer`
/// (`{qr_data_uri, title, line, code}`) prints the verification QR on every page; `header` (`{text}`) a
/// running header.
pub async fn generate_pdf(
    template: &str,
    data: &Value,
    footer: Option<&Value>,
    header: Option<&Value>,
) -> Result<Vec<u8>, HttpxError> {
    let s = granary_core::config::settings();
    let url = httpx_url(&format!("{}/render", s.PDF_SERVICE_URL), &[])?;
    let mut rb = CLIENT
        .post(url)
        .timeout(PDF_TIMEOUT)
        .header("Content-Type", "application/json")
        .body(httpx_json_body(&render_body(
            template, data, footer, header,
        )));
    if !s.PDF_SERVICE_TOKEN.is_empty() {
        rb = rb.header("Authorization", format!("Bearer {}", s.PDF_SERVICE_TOKEN));
    }
    let resp = HttpxResponse::read(rb.send().await?).await?;
    resp.raise_for_status()?;
    Ok(resp.content)
}

/// `generate_quotation_pdf(quotation_data)`.
pub async fn generate_quotation_pdf(quotation_data: &Value) -> Result<Vec<u8>, HttpxError> {
    generate_pdf("quotation", quotation_data, None, None).await
}

/// `generate_invoice_pdf(invoice_data)`.
pub async fn generate_invoice_pdf(invoice_data: &Value) -> Result<Vec<u8>, HttpxError> {
    generate_pdf("invoice", invoice_data, None, None).await
}

/// `generate_proforma_pdf(invoice_data)`.
pub async fn generate_proforma_pdf(invoice_data: &Value) -> Result<Vec<u8>, HttpxError> {
    generate_pdf("proforma", invoice_data, None, None).await
}
