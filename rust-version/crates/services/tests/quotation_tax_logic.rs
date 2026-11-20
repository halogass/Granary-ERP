//! 2 Nov 2026 (quotation tax backlog, Owner decisions 2 Nov): what the notes say about tax, the "prices exclude
//! tax" sentence added and taken out, the notes/tax conflict, the money rule for tax changes over MCP, and the
//! new tool and action wiring. No database.

use granary_services::ai::mcp_access;
use granary_services::nary::quotation_updates as qu;
use granary_services::quotation_tax::{self as qt, Claim, Rules, TaxChoice};
use rust_decimal::Decimal;
use serde_json::json;

const NOTE: &str = "Prices exclude sales tax and withholding tax.";

fn rules() -> Rules {
    // the defaults of the baseline (sales.quotation.tax_*_phrases)
    let list = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    Rules {
        note: NOTE.into(),
        exclusive: list(&[
            "exclude vat",
            "excludes vat",
            "excluding vat",
            "exclusive of vat",
            "excl. vat",
            "excl vat",
            "exclude tax",
            "excludes tax",
            "excluding tax",
            "exclusive of tax",
            "before tax",
            "exclude sales tax",
            "excluding sales tax",
            "plus vat",
            "plus tax",
        ]),
        inclusive: list(&[
            "include vat",
            "includes vat",
            "including vat",
            "inclusive of vat",
            "incl. vat",
            "incl vat",
            "include tax",
            "includes tax",
            "including tax",
            "inclusive of tax",
            "tax included",
            "vat included",
            "including sales tax",
        ]),
    }
}

#[test]
fn the_notes_say_whether_prices_exclude_or_include_tax() {
    let r = rules();
    assert_eq!(qt::claim(Some(NOTE), &r), Some(Claim::Excludes));
    assert_eq!(
        qt::claim(Some("<p>Prices exclude VAT and withholding tax.</p>"), &r),
        Some(Claim::Excludes)
    );
    assert_eq!(
        qt::claim(Some("Prices excluding VAT 20%."), &r),
        Some(Claim::Excludes)
    );
    assert_eq!(
        qt::claim(Some("Prices include VAT."), &r),
        Some(Claim::Includes)
    );
    assert_eq!(
        qt::claim(Some("All prices are including VAT."), &r),
        Some(Claim::Includes)
    );
    // "include vat" inside "exclude vat" wording is not a claim that prices include it
    assert_eq!(
        qt::claim(Some("Exclusive of VAT"), &r),
        Some(Claim::Excludes)
    );
    assert_eq!(qt::claim(Some("Delivery in 4 weeks."), &r), None);
    assert_eq!(qt::claim(None, &r), None);
}

#[test]
fn the_sentence_is_added_once_and_taken_out_again() {
    let added = qt::with_note(Some("Delivery in 4 weeks."), NOTE);
    assert_eq!(added, format!("Delivery in 4 weeks.\n\n{NOTE}"));
    assert_eq!(qt::with_note(Some(&added), NOTE), added, "not twice");
    assert_eq!(qt::with_note(None, NOTE), NOTE);
    let html = qt::with_note(Some("<p>Delivery in 4 weeks.</p>"), NOTE);
    assert_eq!(html, format!("<p>Delivery in 4 weeks.</p><p>{NOTE}</p>"));
    assert_eq!(
        qt::without_note(Some(&html), NOTE).as_deref(),
        Some("<p>Delivery in 4 weeks.</p>")
    );
    assert_eq!(
        qt::without_note(Some(&added), NOTE).as_deref(),
        Some("Delivery in 4 weeks.")
    );
    assert_eq!(qt::without_note(Some(NOTE), NOTE), None);
    // whatever the case or spacing
    assert!(qt::has_note(
        Some("prices exclude  SALES tax and withholding tax."),
        NOTE
    ));
}

#[test]
fn notes_and_line_taxes_that_disagree_are_a_conflict() {
    let r = rules();
    let why = qt::conflict(Some(NOTE), 2, 3, &r).unwrap();
    assert!(
        why.contains("exclude tax") && why.contains("2 of 3"),
        "{why}"
    );
    assert!(qt::conflict(Some(NOTE), 0, 3, &r).is_none());
    let why = qt::conflict(Some("Prices include VAT."), 0, 2, &r).unwrap();
    assert!(why.contains("include tax"), "{why}");
    assert!(qt::conflict(Some("Prices include VAT."), 1, 2, &r).is_none());
    assert!(qt::conflict(Some("Delivery in 4 weeks."), 2, 2, &r).is_none());
    assert!(!why.contains('\u{2014}'));
}

#[test]
fn tax_choices_set_the_line_as_the_editor_does() {
    let mut l = granary_services::pricing::LineIn::new("a");
    TaxChoice::None.apply(&mut l);
    assert_eq!((l.tax_id.clone(), l.tax_pct), (None, Some(Decimal::ZERO)));
    TaxChoice::Custom(Decimal::from(5)).apply(&mut l);
    assert_eq!(l.tax_pct, Some(Decimal::from(5)));
    TaxChoice::Default.apply(&mut l);
    assert_eq!((l.tax_id.clone(), l.tax_pct), (None, None));
    assert_eq!(TaxChoice::None.text(), "no tax");
    assert_eq!(TaxChoice::None.item_fields(), (json!(null), json!("0")));
}

#[test]
fn a_tax_change_is_money_over_mcp() {
    let upd = |p| json!({"kind": "nary_action", "action": "quotation.update", "payload": p});
    assert!(mcp_access::carries_tax(&upd(json!({"tax": "none"}))));
    assert!(mcp_access::carries_tax(&upd(
        json!({"change_items": [{"item_id": "x", "tax_code": null}]})
    )));
    assert!(mcp_access::carries_tax(&upd(
        json!({"add_items": [{"sku": "BOX", "tax_code": "VAT20"}]})
    )));
    assert!(!mcp_access::carries_tax(&upd(
        json!({"change_items": [{"item_id": "x", "quantity": 2}]})
    )));
    assert!(mcp_access::carries_tax(
        &json!({"kind": "quotation", "tax": "no tax", "payload": {}})
    ));
    assert!(!mcp_access::carries_tax(
        &json!({"kind": "quotation", "payload": {}})
    ));
    assert!(qu::sets_tax(&json!({"tax": "VAT20"})));
    assert!(!qu::sets_tax(&json!({"tax": ""})));
    assert!(!mcp_access::GATED_ACTIONS.contains(&"quotation.revision"));
}

#[test]
fn the_new_tool_action_and_arguments_are_wired() {
    use granary_services::ai::{schema, tools::catalog};
    let t = catalog::ALL_TOOLS
        .iter()
        .find(|t| t.name == "propose_quotation_revision")
        .unwrap();
    let sent = schema::for_anthropic(&t.parameters);
    assert!(schema::violations(&sent).is_empty());
    assert!(!t.description.contains('\u{2014}'));
    assert!(mcp_access::PROPOSAL_TOOLS.contains(&"propose_quotation_revision"));
    let a = granary_services::nary::actions::get("quotation.revision").unwrap();
    assert_eq!(a.permission.as_deref(), Some("sales:create_quotation"));
    assert!(granary_services::nary::executor::CHAT_ACTIONS.contains(&"quotation.revision"));
    for tool in ["propose_quotation_draft", "propose_quotation_update"] {
        let t = catalog::ALL_TOOLS.iter().find(|t| t.name == tool).unwrap();
        assert!(t.parameters["properties"]["tax"].is_object(), "{tool}");
        let sent = schema::for_anthropic(&t.parameters);
        assert!(schema::violations(&sent).is_empty(), "{tool}");
    }
    let t = catalog::ALL_TOOLS
        .iter()
        .find(|t| t.name == "propose_quotation_update")
        .unwrap();
    assert!(
        t.parameters["properties"]["change_lines_json"]["description"]
            .as_str()
            .unwrap()
            .contains("tax_code")
    );
    let guide = include_str!("../src/ai/erp_guide/nary.md");
    let after = guide
        .split("<!-- added after the Python freeze")
        .nth(1)
        .unwrap();
    assert!(after.contains("propose_quotation_revision") && after.contains("quote_without_tax"));
    assert!(!after.contains('\u{2014}'));
    assert!(granary_services::crm::writes::may_write(
        "organization",
        "quote_without_tax"
    ));
    assert!(!granary_services::crm::writes::may_write(
        "contact",
        "quote_without_tax"
    ));
}
