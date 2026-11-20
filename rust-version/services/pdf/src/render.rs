//! `renderPDF` of services/pdf/src/index.js: one shared Chromium (launched on first use or after it
//! disconnected), a page per render with request interception, `setContent(html, networkidle0)` and
//! `page.pdf` with the same A4 options, margins and verification header/footer.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::cdp::{Browser, JsError};
use crate::jsval;

/// The args index.js passes to `puppeteer.launch` (headless: true).
pub const PDF_ARGS: [&str; 4] = [
    "--no-sandbox",
    "--disable-setuid-sandbox",
    "--disable-dev-shm-usage",
    "--disable-gpu",
];

pub struct PdfRenderer {
    browser: Mutex<Option<Arc<Browser>>>,
    asset_hosts: HashSet<String>,
}

/// `new URL(h.includes("://") ? h : "http://" + h).host`.
pub fn url_host(h: &str) -> Option<String> {
    let with_scheme = if h.contains("://") {
        h.to_string()
    } else {
        format!("http://{h}")
    };
    let u = url::Url::parse(&with_scheme).ok()?;
    let host = u.host_str().unwrap_or("").to_string();
    Some(match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    })
}

/// ASSET_HOSTS: MINIO_ENDPOINT (default minio:9000), MINIO_PUBLIC_URL and PDF_ASSET_HOSTS (comma list).
pub fn asset_hosts_from_env() -> HashSet<String> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut list = vec![var("MINIO_ENDPOINT").unwrap_or_else(|| "minio:9000".into())];
    list.extend(var("MINIO_PUBLIC_URL"));
    list.extend(
        std::env::var("PDF_ASSET_HOSTS")
            .unwrap_or_default()
            .split(',')
            .map(str::to_string),
    );
    list.iter()
        .map(|h| h.trim())
        .filter(|h| !h.is_empty())
        .filter_map(url_host)
        .filter(|h| !h.is_empty())
        .collect()
}

/// A URL without its query string and fragment (for logs).
pub fn without_query(u: &str) -> String {
    u.split(['?', '#'])
        .next()
        .unwrap_or("")
        .chars()
        .take(300)
        .collect()
}

/// `isAllowedRequest(url, allowAssets)`.
pub fn is_allowed_request(url: &str, allow_assets: bool, hosts: &HashSet<String>) -> bool {
    if url.starts_with("data:") || url.starts_with("about:") {
        return true;
    }
    if !allow_assets {
        return false;
    }
    match url::Url::parse(url) {
        Ok(u) => {
            (u.scheme() == "http" || u.scheme() == "https")
                && url_host(u.as_str()).is_some_and(|h| hosts.contains(&h))
        }
        Err(_) => false,
    }
}

/// `escapeHtml(value)`: String(value ?? "") with & < > " ' escaped.
pub fn escape_html(v: Option<&Value>) -> String {
    let s = match v {
        None | Some(Value::Null) => String::new(),
        other => jsval::to_js_string(other),
    };
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// `footerTemplate(footer)`: QR (data: URI only), document title/line/code, page x / y.
pub fn footer_template(footer: &Value) -> String {
    let qr = match jsval::get_prop(Some(footer), "qr_data_uri") {
        Some(Value::String(s)) if s.starts_with("data:image/png;base64,") => s.clone(),
        _ => String::new(),
    };
    let img = if qr.is_empty() {
        String::new()
    } else {
        format!("<img src=\"{qr}\" style=\"width:18mm;height:18mm;\" />")
    };
    let f = |k: &str| escape_html(jsval::get_prop(Some(footer), k));
    format!(
        "<div style=\"width:100%;margin:0 12mm;font-family:Arial,sans-serif;font-size:7px;color:#666;display:flex;align-items:center;gap:8px;-webkit-print-color-adjust:exact;\">\n    {img}\n    <div style=\"flex:1;line-height:1.5;\">\n      <div style=\"font-weight:700;color:#1a1a1a;font-size:7.5px;\">{}</div>\n      <div>{}</div>\n      <div>{}</div>\n    </div>\n    <div style=\"white-space:nowrap;\">Page <span class=\"pageNumber\"></span> / <span class=\"totalPages\"></span></div>\n  </div>",
        f("title"),
        f("line"),
        f("code")
    )
}

/// `headerTemplate(header)`: a right-aligned running header.
pub fn header_template(header: &Value) -> String {
    format!(
        "<div style=\"width:100%;margin:0 12mm;font-family:Arial,sans-serif;font-size:7.5px;color:#888;text-align:right;-webkit-print-color-adjust:exact;\">{}</div>",
        escape_html(jsval::get_prop(Some(header), "text"))
    )
}

/// Puppeteer's convertPrintParameterToInches for "<n>mm".
fn mm(v: f64) -> f64 {
    v * 3.78 / 96.0
}

/// The Page.printToPDF parameters `page.pdf({ format: "A4", printBackground: true, margin, ... })` sends.
pub fn print_params(footer: Option<&Value>, header: Option<&Value>) -> Value {
    let (top, right, bottom, left) = match footer {
        Some(_) => (if header.is_some() { 20.0 } else { 15.0 }, 12.0, 30.0, 12.0),
        None => (15.0, 15.0, 20.0, 15.0),
    };
    let (display, header_t, footer_t) = match footer {
        Some(f) => (
            true,
            header
                .map(header_template)
                .unwrap_or_else(|| "<span></span>".into()),
            footer_template(f),
        ),
        None => (false, String::new(), String::new()),
    };
    json!({
        "transferMode": "ReturnAsStream",
        "landscape": false,
        "displayHeaderFooter": display,
        "headerTemplate": header_t,
        "footerTemplate": footer_t,
        "printBackground": true,
        "scale": 1,
        "paperWidth": 8.27,
        "paperHeight": 11.7,
        "marginTop": mm(top),
        "marginBottom": mm(bottom),
        "marginLeft": mm(left),
        "marginRight": mm(right),
        "pageRanges": "",
        "preferCSSPageSize": false,
        "generateTaggedPDF": false,
        "generateDocumentOutline": false,
    })
}

pub struct RenderOptions<'a> {
    pub allow_assets: bool,
    pub footer: Option<&'a Value>,
    pub header: Option<&'a Value>,
    pub javascript: bool,
}

impl PdfRenderer {
    pub fn new(asset_hosts: HashSet<String>) -> Self {
        PdfRenderer {
            browser: Mutex::new(None),
            asset_hosts,
        }
    }

    /// `getBrowser()`.
    pub async fn browser(&self) -> Result<Arc<Browser>, JsError> {
        let mut guard = self.browser.lock().await;
        if let Some(b) = guard.as_ref() {
            if b.is_connected() {
                return Ok(b.clone());
            }
        }
        if let Some(old) = guard.take() {
            old.kill();
        }
        let b = Arc::new(
            Browser::launch(false, &PDF_ARGS)
                .await
                .map_err(JsError::msg)?,
        );
        *guard = Some(b.clone());
        Ok(b)
    }

    pub async fn shutdown(&self) {
        if let Some(b) = self.browser.lock().await.take() {
            b.close().await;
        }
    }

    /// `renderPDF(html, { allowAssets, footer, header, javaScript })`.
    pub async fn render(&self, html: Value, opts: RenderOptions<'_>) -> Result<Vec<u8>, JsError> {
        let browser = self.browser().await?;
        let page = browser.new_page().await?;
        let result = async {
            if !opts.javascript {
                page.disable_javascript().await?;
            }
            let hosts = self.asset_hosts.clone();
            let allow_assets = opts.allow_assets;
            // 10 Nov 2026: every outside request is logged (without its query string: presigned links carry
            // secrets), so a render that waits on an image says which one
            page.intercept(move |u| {
                let ok = is_allowed_request(u, allow_assets, &hosts);
                if !u.starts_with("data:") && !u.starts_with("about:") {
                    crate::log::info(
                        "PDF asset requested",
                        json!({"url": without_query(u), "allowed": ok}),
                    );
                }
                ok
            })
            .await?;
            page.set_content(html, "networkIdle").await.map_err(|e| {
                JsError::msg(format!(
                    "{} (while loading the page's images and fonts; see the \"PDF asset requested\" lines)",
                    e.0.unwrap_or_default()
                ))
            })?;
            page.pdf(print_params(opts.footer, opts.header)).await
        }
        .await;
        page.close().await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_requests() {
        assert_eq!(url_host("minio:9000").as_deref(), Some("minio:9000"));
        assert_eq!(url_host("http://MinIO:80").as_deref(), Some("minio"));
        assert_eq!(
            url_host("https://files.example.com/bucket").as_deref(),
            Some("files.example.com")
        );
        let hosts: HashSet<String> = ["minio:9000".to_string()].into_iter().collect();
        assert!(is_allowed_request(
            "data:image/png;base64,xx",
            false,
            &hosts
        ));
        assert!(is_allowed_request("about:blank", false, &hosts));
        assert!(!is_allowed_request(
            "http://minio:9000/a.png",
            false,
            &hosts
        ));
        assert!(is_allowed_request("http://minio:9000/a.png", true, &hosts));
        assert!(!is_allowed_request("http://minio:9001/a.png", true, &hosts));
        assert!(!is_allowed_request("ftp://minio:9000/a.png", true, &hosts));
    }

    #[test]
    fn margins_match_puppeteer() {
        let p = print_params(None, None);
        assert_eq!(p["marginTop"].to_string(), "0.590625");
        assert_eq!(p["marginBottom"].to_string(), "0.7875");
        let f = json!({"title": "<T>", "line": "a & b", "code": 5, "qr_data_uri": "data:image/png;base64,AAA"});
        let p = print_params(
            Some(&f),
            Some(&json!({"text": "Your Company | Confidential"})),
        );
        assert_eq!(p["marginTop"].to_string(), "0.7875");
        assert!(p["footerTemplate"].as_str().unwrap().contains("&lt;T&gt;"));
        assert!(p["footerTemplate"]
            .as_str()
            .unwrap()
            .contains("<img src=\"data:image/png;base64,AAA\""));
        assert!(p["headerTemplate"]
            .as_str()
            .unwrap()
            .ends_with(">Your Company | Confidential</div>"));
    }
}
