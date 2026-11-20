//! 3 Nov 2026, Stream B1 (no database): how propose_whatsapp_lead, propose_held_chat_decision, propose_lead_merge
//! and score_lead are wired (catalog, MCP levels, chat actions, gating, the ERP guide).

use granary_services::ai::{mcp_access, schema, tools::catalog};
use granary_services::nary::{actions, executor, wa_leads};
use serde_json::json;

#[test]
fn the_new_tools_and_actions_are_wired() {
    for (tool, action) in [
        ("propose_whatsapp_lead", "lead.from_whatsapp"),
        ("propose_held_chat_decision", "chat.held_decide"),
        ("propose_lead_merge", "lead.merge"),
        ("propose_lead_promote", "lead.promote"),
    ] {
        let t = catalog::ALL_TOOLS
            .iter()
            .find(|t| t.name == tool)
            .unwrap_or_else(|| panic!("{tool}"));
        assert!(
            schema::violations(&schema::for_anthropic(&t.parameters)).is_empty(),
            "{tool}"
        );
        assert!(!t.description.contains('\u{2014}'), "{tool}");
        assert!(mcp_access::PROPOSAL_TOOLS.contains(&tool), "{tool}");
        assert!(!mcp_access::ACT_TOOLS.contains(&tool), "{tool}");
        assert!(actions::get(action).is_ok(), "{action}");
        assert!(executor::CHAT_ACTIONS.contains(&action), "{action}");
    }
    assert_eq!(
        actions::get("lead.from_whatsapp")
            .unwrap()
            .permission
            .as_deref(),
        Some("crm:edit_lead")
    );
    assert_eq!(
        actions::get("chat.held_decide")
            .unwrap()
            .permission
            .as_deref(),
        Some("nary:manage")
    );
    // score_lead reads (every level); its save is an act argument
    let t = catalog::ALL_TOOLS
        .iter()
        .find(|t| t.name == "score_lead")
        .unwrap();
    assert!(schema::violations(&schema::for_anthropic(&t.parameters)).is_empty());
    assert!(!mcp_access::PROPOSAL_TOOLS.contains(&"score_lead"));
    assert!(mcp_access::ACT_ARGS.contains(&("score_lead", "save")));
}

#[test]
fn saving_a_score_needs_act_and_personal_is_gated() {
    let propose = vec!["propose".to_string()];
    let read = vec!["read".to_string()];
    let act = vec!["act".to_string()];
    let full = vec!["act".to_string(), "owner".to_string(), "full".to_string()];
    let save = json!({"lead_id": "x", "save": true});
    for lvl in [&propose, &read] {
        let why = mcp_access::act_arg_refusal(lvl, "score_lead", &save).unwrap();
        assert!(why.contains("stores the score"), "{why}");
        assert!(mcp_access::act_arg_refusal(
            lvl,
            "score_lead",
            &json!({"lead_id": "x", "save": false})
        )
        .is_none());
        assert!(mcp_access::act_arg_refusal(lvl, "score_lead", &json!({"lead_id": "x"})).is_none());
    }
    for lvl in [&act, &full] {
        assert!(mcp_access::act_arg_refusal(lvl, "score_lead", &save).is_none());
    }
    // a held chat decided personal deletes what was stored: gated at full power; business is not
    let card = |d: &str| {
        json!({"kind": "nary_action", "action": "chat.held_decide",
               "payload": {"chat_id": "1@c.us", "decision": d}})
    };
    assert!(wa_leads::deletes_chat(&card("personal")));
    assert!(!wa_leads::deletes_chat(&card("business")));
    assert!(!wa_leads::deletes_chat(
        &json!({"action": "lead.from_whatsapp", "payload": {}})
    ));
    assert!(mcp_access::GATED_ACTIONS.contains(&"lead.merge"));
    assert!(!mcp_access::GATED_ACTIONS.contains(&"lead.from_whatsapp"));
}

#[test]
fn the_erp_guide_says_how_after_the_freeze() {
    let guide = include_str!("../src/ai/erp_guide/nary.md");
    let after = guide
        .split("<!-- added after the Python freeze")
        .nth(1)
        .unwrap();
    for t in [
        "propose_whatsapp_lead",
        "propose_held_chat_decision",
        "propose_lead_merge",
        "score_lead",
    ] {
        assert!(after.contains(t), "{t}");
    }
    assert!(!after.contains('\u{2014}'));
}
