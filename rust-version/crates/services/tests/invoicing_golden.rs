//! Golden cases for invoicing's money maths (specs -> _line_inputs -> compute_lines -> apply_totals),
//! recorded from the real Python (compat-tests/golden/gen_invoicing.py). Decimals must match in value
//! and scale (Python's str), tax_lines floats bit for bit.
#![recursion_limit = "256"]

use std::str::FromStr;

use granary_core::money::compute_lines;
use granary_db::rows::finance::Invoice;
use granary_services::invoicing::{_line_inputs, apply_totals, ItemSpec};
use rust_decimal::Decimal;
use serde_json::{json, Value};

fn dec(v: &Value) -> Option<Decimal> {
    v.as_str().map(|s| Decimal::from_str(s).unwrap())
}

fn same(rust: Decimal, py: &str) {
    let e = Decimal::from_str(py)
        .or_else(|_| Decimal::from_scientific(py))
        .unwrap();
    assert!(
        rust == e && rust.scale() == e.scale(),
        "rust {rust} python {py}"
    );
}

pub fn blank_invoice() -> Invoice {
    serde_json::from_value(json!({
        "number": "INV-G", "invoice_type": "commercial", "revision": 0, "full_number": null,
        "revision_group": null, "previous_version_id": null, "revision_reason": null,
        "credit_of_invoice_id": null, "deal_id": null, "project_id": null, "quotation_id": null,
        "ref_pi_number": null, "customer_org_id": null, "customer_contact_id": null, "customer_name": null,
        "customer_address": null, "customer_tax_id": null, "customer_tel": null, "customer_email": null,
        "customer_contact_person": null, "customer_contact_email": null, "status": "draft",
        "currency": "JPY", "exchange_rate": null, "subtotal": null, "tax_lines": null, "tax_total": null,
        "total": null, "amount_paid": null, "amount_due": null, "refund_due": "0",
        "issue_date": "2031-01-15", "due_date": null, "paid_at": null, "payment_method": null,
        "payment_reference": null, "payment_terms_text": null, "incoterms": null,
        "incoterms_destination": null, "carrier": null, "tracking_number": null, "hs_code": null,
        "country_of_origin": null, "shipped_from": null, "pdf_url": null, "authorized_by_id": null,
        "authorized_by_name": null, "authorized_by_title": null, "notes": null, "internal_notes": null,
        "created_by_id": null, "id": "00000000-0000-0000-0000-000000000001",
        "created_at": "2031-01-15T00:00:00Z", "updated_at": "2031-01-15T00:00:00Z", "is_deleted": false,
        "deleted_at": null
    }))
    .unwrap()
}

#[test]
fn golden_totals() {
    let path = format!(
        "{}/../../compat-tests/golden/invoicing/totals.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(cases.len() >= 700);
    for (i, c) in cases.iter().enumerate() {
        let places = c["places"].as_u64().unwrap() as u32;
        let specs: Vec<ItemSpec> = c["specs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| ItemSpec {
                name: "x".into(),
                quantity: dec(&s["quantity"]).unwrap(),
                unit_price: dec(&s["unit_price"]).unwrap(),
                discount_pct: dec(&s["discount_pct"]),
                tax_pct: dec(&s["tax_pct"]),
                tax_code: s["tax_code"].as_str().map(String::from),
                tax_name: s["tax_name"].as_str().map(String::from),
                ..Default::default()
            })
            .collect();
        let totals = compute_lines(&_line_inputs(&specs), places);
        let mut inv = blank_invoice();
        inv.amount_paid = dec(&c["amount_paid"]);
        apply_totals(&mut inv, &totals, dec(&c["amount_due_in"]));
        for (r, exp) in totals.lines.iter().zip(c["lines"].as_array().unwrap()) {
            let exp: Vec<&str> = exp
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            same(r.gross, exp[0]);
            same(r.discount_amount, exp[1]);
            same(r.amount, exp[2]);
            same(r.tax_amount, exp[3]);
        }
        same(inv.subtotal.unwrap(), c["subtotal"].as_str().unwrap());
        same(totals.discount_total, c["discount_total"].as_str().unwrap());
        same(inv.tax_total.unwrap(), c["tax_total"].as_str().unwrap());
        same(inv.total.unwrap(), c["total"].as_str().unwrap());
        same(inv.amount_due.unwrap(), c["amount_due"].as_str().unwrap());
        let got = inv.tax_lines.as_ref().unwrap().as_array().unwrap();
        let exp = c["tax_lines"].as_array().unwrap();
        assert_eq!(got.len(), exp.len(), "case {i}");
        for (g, e) in got.iter().zip(exp) {
            assert_eq!(g["code"], e["code"], "case {i}");
            assert_eq!(g["name"], e["name"], "case {i}");
            for k in ["rate", "base", "amount"] {
                let gv: f64 = g[k].to_string().parse().unwrap();
                let ev: f64 = e[k].as_str().unwrap().parse().unwrap();
                assert_eq!(gv.to_bits(), ev.to_bits(), "case {i} {k}");
            }
            // key order as Python's dict
            let keys: Vec<&String> = g.as_object().unwrap().keys().collect();
            assert_eq!(keys, ["code", "name", "rate", "base", "amount"]);
        }
    }
}
