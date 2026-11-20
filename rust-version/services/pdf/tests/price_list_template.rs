//! doclib A2: the price_list template renders the data price_list.rs builds.

use std::path::PathBuf;

use granary_pdf::hbs::Templates;
use serde_json::json;

#[test]
fn price_list_renders_sections_families_and_the_internal_mark() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let t = Templates::new(root.join("assets/templates"));
    let data = json!({
        "title": "ACME Price List",
        "generated_on": "21 Oct 2026",
        "effective_date": "2026-10-21",
        "internal_mark": "Internal",
        "company": {"trade_name": "Acme Ltd", "legal_name": "Acme Ltd", "address": "Tokyo",
                    "email": "hello@example.com", "website": "example.com", "letterhead_url": ""},
        "sections": [{"currency": "JPY", "families": [{"family": "Laptop", "items": [
            {"sku": "LAPTOP-S25", "name": "Laptop Server 25", "description": null, "terms": "per unit",
             "price": "JPY 25,000,000", "note": "Incl. <1 year> care"}]}]}]
    });
    let html = t.render(&json!("price_list"), &data).unwrap();
    for want in [
        "ACME Price List",
        "Internal",
        "Prices in JPY",
        "Laptop",
        "LAPTOP-S25",
        "JPY 25,000,000",
        "Generated 21 Oct 2026",
        "Incl. &lt;1 year&gt; care",
    ] {
        assert!(html.contains(want), "missing {want}");
    }
    assert!(!html.contains("letterhead\" src"));
}
