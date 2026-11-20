//! Wire-up checks: what Python registers at import time (the actions registry, the event subscribers, the
//! Owner tools in the catalog) is registered here too, in the same order. Expected values recorded from Python
//! (main 8626c2e): `list(actions.REGISTRY)` and `events._SUBSCRIBERS` after `import app.main` plus the import
//! line of `events.dispatch_pending`.

use granary_services::ai::tools::{catalog, nary_owner};
use granary_services::nary::{actions, events};

#[test]
fn the_action_registry_matches_python_in_order() {
    let names: Vec<String> = actions::all().iter().map(|a| a.name.clone()).collect();
    let python = [
        "crm.record_change",
        "mcp.crm_write",
        "lead.promote",
        "lead.merge",
        "reminder.add",
        "reply.draft",
        "email.reply.send",
        "whatsapp.reply.send",
        "quotation.create_draft",
        "whatsapp.ack.send",
        "reengagement.send",
        "email.ack.send",
        "identity.link",
        "contact.merge",
        "lead.open_returning",
        "mcp.proposal",
        "price.answer.send",
        "cadence.followup.send",
        "lost.reply.send",
        "quotation.send",
        "meeting.slots.send",
        "meeting.book",
        "meeting.reminder.send",
        "meeting.add_guests",
        "meeting.cancel",
        "summary.email.send",
        "contract.create_draft",
        "proforma.raise",
        "handoff.kickoff",
        "partner.route",
        "proposal.discovery.start",
        "proposal.create_draft",
        "rates.propose_update",
        // batch 2 T1: Rust only (after Python froze), registered by proposal_actions
        "proposal.from_chat",
        "proposal.update_requirements",
        // batch 2 T3: Rust only (after Python froze)
        "message.cancel_scheduled",
        // batch 2 T2: Rust only (after Python froze)
        "organization.merge",
        "contact.delete",
        // batch 4 D3: Rust only (after Python froze)
        "quotation.update",
        // doclib A3: Rust only
        "document.send",
        // visuals V3: Rust only
        "proposal.visual",
        "proposal.edit",
        // fix-longapply: Rust only
        "proposal.revise",
        "proposal.reject",
        // 22 Oct 2026: Rust only
        "proposal.quotation",
        // 23 Oct 2026: Rust only
        "proposal.from_sections",
        "catalog.change",
        // R&D 26 Oct 2026: Rust only
        "deal.merge",
        "activity.void",
        "email.link",
        // 1 Nov 2026: Rust only
        "quotation.reprice",
        "quotation.withdraw",
        "quotation.cancel",
        // 2 Nov 2026: Rust only
        "quotation.revision",
        // 3 Nov 2026 (Stream B1): Rust only
        "lead.from_whatsapp",
        "chat.held_decide",
        // 3 Nov 2026 (Stream B2): Rust only
        "deal.won",
        // 5 Nov 2026 (Stream A): Rust only (a product file added by the Owner's card)
        "catalog.product_file",
        // 5 Nov 2026 (Stream C): Rust only
        "quotation.link_deal",
    ];
    assert_eq!(names, python);
}

#[test]
fn every_python_subscriber_is_registered() {
    // (type, how many subscribers Python has for it, "*" not counted)
    let python: &[(&str, usize)] = &[
        ("quotation.sent", 1),     // cadence.on_quotation_sent
        ("quotation.accepted", 2), // cadence.on_quotation_accepted, deals.on_quotation_accepted
        ("quotation.rejected", 1), // cadence.on_quotation_ended
        ("quotation.expired", 1),  // cadence.on_quotation_ended
        ("message.inbound", 1),    // channels.on_inbound
        ("message.outbound", 1),   // channels.on_outbound
        ("contract.signed", 1),    // deals.on_contract_or_payment
        ("payment.recorded", 1),   // deals.on_contract_or_payment
        ("email.new", 1),          // email_intake.on_new_email
        ("deal.staged", 2),        // handoff.on_deal_staged, labels.on_deal
        ("lead.created", 2),       // handoff.on_lead_created, labels.on_lead
        ("intake.form", 1),        // intake.process_form
        // labels.on_deal; Rust adds crm::deal_duplicates (R&D 26 Oct 2026)
        ("deal.created", 2),
        ("email.inbound", 1),    // playbooks.on_email
        ("approval.decided", 1), // quotes.on_approval
        ("quotation.signed", 0),
        ("quotation.invoiced", 0),
        ("arrival.missed", 0),
        ("chat.held", 0),
        ("meeting.booked", 0),
    ];
    for (t, n) in python {
        // subscribers_for adds the "*" subscriber (subscribers.record_event) last
        assert_eq!(events::subscribers_for(t).len(), n + 1, "{t}");
    }
}

#[test]
fn the_owner_tools_reach_the_catalog_once_used() {
    // Python: `register_extra(OWNER_TOOLS)` when nary_owner is first imported (staff_chat / mcp_access do).
    let names: Vec<String> = nary_owner::OWNER_TOOLS
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(!names.is_empty());
    for n in &names {
        assert!(catalog::tool_by_name(n).is_some(), "{n}");
    }
}
