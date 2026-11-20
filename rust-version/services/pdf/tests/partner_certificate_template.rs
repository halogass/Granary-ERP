//! Partner training: the partner_certificate template renders the data portal_certificate_pdf builds.

use std::path::PathBuf;

use granary_pdf::hbs::Templates;
use serde_json::json;

#[test]
fn partner_certificate_renders_holder_module_and_number() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let t = Templates::new(root.join("assets/templates"));
    let data = json!({
        "certificate_number": "CERT-3F2A9C1E",
        "holder_name": "Aiko <Tanaka>",
        "partner_company": "Kanda Logistics KK",
        "module_title": "Product fundamentals",
        "issued_date": "20 November 2026",
        "expires_date": "",
        "letterhead_url": "",
        "company_name": "Acme Trading",
        "company_website": "example.com",
    });
    let html = t.render(&json!("partner_certificate"), &data).unwrap();
    for want in [
        "Aiko &lt;Tanaka&gt;",
        "Kanda Logistics KK",
        "Product fundamentals",
        "20 November 2026",
        "CERT-3F2A9C1E",
        "Acme Trading",
    ] {
        assert!(html.contains(want), "missing {want}");
    }
    assert!(!html.contains("Valid until"));
    assert!(!html.contains("letterhead-img\" src"));
}
