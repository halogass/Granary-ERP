//! 4 Nov 2026: a quotation's optional lines print in their own section under the totals of the document
//! template, and only when there are any (templates_html.rs keeps every golden byte-identical without them).

use std::path::PathBuf;

use granary_pdf::hbs::Templates;
use serde_json::{json, Value};

fn fixture() -> Value {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let p = root.join("tests/fixtures/document_quotation.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn templates() -> Templates {
    Templates::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/templates"))
}

#[test]
fn the_optional_section_prints_under_the_totals_only_when_there_are_optional_lines() {
    std::env::set_var("TZ", "UTC");
    let t = templates();
    let body = fixture();
    let without = t.render(&body["template"], &body["data"]).unwrap();
    assert!(!without.contains("Optional items"));
    assert!(!without.contains("optional-items"));

    let mut data = body["data"].clone();
    data["optional_items"] = json!({
        "title": "Optional items (not included in the total)",
        "columns": [{"label": "Description", "right": false}, {"label": "Qty", "right": true},
                    {"label": "Unit Price", "right": true}, {"label": "Amount", "right": true}],
        "rows": [[{"title": "Analytics dashboard", "sku": "", "sub": "Web dashboard, 12 months", "term": "",
                   "note": "", "indent": false},
                  {"text": "1", "right": true, "strong": false},
                  {"text": "USD 2,400.00", "right": true, "strong": false},
                  {"text": "USD 2,400.00", "right": true, "strong": true}]],
        "totals": [{"label": "Optional items subtotal", "value": "USD 2,400.00"}],
        "note": "Not included in the total above. Tax on these items if taken: USD 480.00 (with tax: USD 2,880.00).",
    });
    let with = t.render(&body["template"], &data).unwrap();
    let at = |s: &str| with.find(s).unwrap_or_else(|| panic!("{s:?} missing"));
    // under the totals: the lines, their subtotal, then the tax note
    assert!(at("class=\"totals\"") < at("Optional items (not included in the total)"));
    assert!(at("Optional items (not included in the total)") < at("Analytics dashboard"));
    assert!(at("Analytics dashboard") < at("Web dashboard, 12 months"));
    assert!(at("<strong>USD 2,400.00</strong>") < at("Optional items subtotal"));
    assert!(at("Optional items subtotal") < at("Tax on these items if taken: USD 480.00"));
    // the rest of the page is unchanged: only the section was added, right after the totals
    let start = at("<div class=\"keep-together optional-items\">");
    assert!(with[..start].trim_end().ends_with("</div>"));
    let note = at("Tax on these items if taken");
    let end = note + with[note..].find("\n</div>\n").unwrap() + "\n</div>\n".len();
    assert_eq!(format!("{}{}", &with[..start], &with[end..]), without);
}
