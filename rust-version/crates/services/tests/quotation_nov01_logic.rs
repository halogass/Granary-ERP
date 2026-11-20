//! 1 Nov 2026, Stream A (no database): the discount arguments of the quotation tools, which cards are money at
//! MCP full power, and how the new actions and tools are wired.

use granary_services::ai::mcp_access;
use granary_services::nary::quotation_updates::{discount_from_args, Discount};
use rust_decimal::Decimal;
use serde_json::json;

#[test]
fn discount_arguments_are_normalised_and_checked() {
    assert_eq!(discount_from_args(&json!({"subject": "x"})).unwrap(), None);
    let d = discount_from_args(
        &json!({"discount_type": "percent", "discount_value": 10, "discount_note": " Owner ok "}),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        (
            d.dtype.as_str(),
            d.value,
            d.note.as_deref(),
            d.display.as_str()
        ),
        ("percentage", Decimal::from(10), Some("Owner ok"), "pdf")
    );
    assert!(d.text("JPY").starts_with("10% off"));
    let d = discount_from_args(&json!({"discount_type": "amount", "discount_value": "5,000,000"}))
        .unwrap()
        .unwrap();
    assert_eq!(
        (d.dtype.as_str(), d.value),
        ("fixed", Decimal::from(5_000_000))
    );
    assert_eq!(d.text("JPY"), "JPY 5,000,000 off, Discount line on the PDF");
    // notes only: nothing taken off, the note is required
    let d = discount_from_args(&json!({"discount_type": "percent", "discount_value": 10,
                                       "discount_display": "notes_only", "discount_note": "10% included"}))
    .unwrap()
    .unwrap();
    assert_eq!(
        (d.dtype.as_str(), d.value, d.display.as_str()),
        ("none", Decimal::ZERO, "notes_only")
    );
    assert!(discount_from_args(&json!({"discount_display": "notes_only"})).is_err());
    // none removes it; mistakes are said
    assert_eq!(
        discount_from_args(&json!({"discount_type": "none"})).unwrap(),
        Some(Discount::none())
    );
    for bad in [
        json!({"discount_type": "percent", "discount_value": 120}),
        json!({"discount_type": "percent", "discount_value": 0}),
        json!({"discount_type": "bogus", "discount_value": 5}),
        json!({"discount_value": 5}),
        json!({"discount_type": "amount", "discount_value": -1}),
        json!({"discount_note": "a note without a discount"}),
        json!({"discount_type": "percent", "discount_value": 5, "discount_display": "banner"}),
    ] {
        assert!(discount_from_args(&bad).is_err(), "{bad}");
    }
    // the payload form reads back the same
    let d = discount_from_args(&json!({"discount_type": "percent", "discount_value": 7.5}))
        .unwrap()
        .unwrap();
    let back = Discount::from_payload(&serde_json::Value::Object(d.fields()))
        .unwrap()
        .unwrap();
    assert_eq!(back, d);
    assert!(!d.text("JPY").contains('\u{2014}'));
}

#[test]
fn a_discount_is_money_and_cancelling_is_gated() {
    let quote = |p| json!({"kind": "quotation", "payload": p});
    assert!(mcp_access::carries_discount(&quote(
        json!({"discount_value": "10", "discount_type": "percentage"})
    )));
    assert!(mcp_access::carries_discount(&quote(
        json!({"discount_value": 0, "discount_note": "10% included"})
    )));
    assert!(!mcp_access::carries_discount(&quote(json!({"items": []}))));
    let upd = |p| json!({"kind": "nary_action", "action": "quotation.update", "payload": p});
    assert!(mcp_access::carries_discount(&upd(
        json!({"discount_type": "none"})
    )));
    assert!(!mcp_access::carries_discount(&upd(json!({"subject": "x"}))));
    assert!(mcp_access::GATED_ACTIONS.contains(&"quotation.cancel"));
    assert!(!mcp_access::GATED_ACTIONS.contains(&"quotation.reprice"));
}

#[test]
fn the_new_actions_and_tools_are_wired() {
    use granary_services::ai::{schema, tools::catalog};
    for (tool, action) in [
        ("propose_quotation_reprice", "quotation.reprice"),
        ("propose_quotation_withdraw", "quotation.withdraw"),
        ("propose_quotation_cancel", "quotation.cancel"),
    ] {
        let t = catalog::ALL_TOOLS
            .iter()
            .find(|t| t.name == tool)
            .unwrap_or_else(|| panic!("{tool}"));
        let sent = schema::for_anthropic(&t.parameters);
        assert!(schema::violations(&sent).is_empty(), "{tool}");
        assert!(!t.description.contains('\u{2014}'), "{tool}");
        assert!(mcp_access::PROPOSAL_TOOLS.contains(&tool));
        let a = granary_services::nary::actions::get(action).unwrap();
        assert_eq!(a.permission.as_deref(), Some("sales:create_quotation"));
        assert!(granary_services::nary::executor::CHAT_ACTIONS.contains(&action));
    }
    for tool in ["propose_quotation_draft", "propose_quotation_update"] {
        let t = catalog::ALL_TOOLS.iter().find(|t| t.name == tool).unwrap();
        assert!(
            t.parameters["properties"]["discount_display"].is_object(),
            "{tool}"
        );
        let sent = schema::for_anthropic(&t.parameters);
        assert!(schema::violations(&sent).is_empty(), "{tool}");
    }
    let guide = include_str!("../src/ai/erp_guide/nary.md");
    let after = guide
        .split("<!-- added after the Python freeze")
        .nth(1)
        .unwrap();
    assert!(after.contains("propose_quotation_reprice") && after.contains("notes_only"));
    assert!(!after.contains('\u{2014}'));
}
