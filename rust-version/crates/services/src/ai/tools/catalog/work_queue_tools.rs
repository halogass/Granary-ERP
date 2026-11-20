//! 3 Nov 2026 (Owner decisions, Stream A): `nary_work_queue`, what is waiting and which tool to use next
//! (nary::work_queue). Read only, but it shows customer data across every owner: over MCP it is listed at act
//! and full power (mcp_access::ACT_TOOLS), never at read or propose; in Nary's chat for the Owner and system admins.

use serde_json::json;

use super::{handler, Tool};
use crate::nary::work_queue::{self, KINDS};

pub fn tools() -> Vec<Tool> {
    vec![Tool::new(
        "nary_work_queue",
        concat!(
            "The work queue while internal Nary is paused: everything waiting for the company, urgent first (a customer ",
            "who wrote and has no answer, a new lead, a WhatsApp number we don't know), then today's (follow-ups ",
            "due, cards about to expire, meetings soon, close-out steps), then the rest (quotations expiring, ",
            "duplicates, pending cards, overdue approvals). Each item has id, kind, title, ",
            "subject (who or what, with record ids), since, urgency (urgent, today, normal), suggested_tool with ",
            "suggested_args (the next step) and a hint. Work the urgent items first. Every message to a customer ",
            "is a card for the Owner's approval (propose_customer_message); nothing is sent directly. An item goes ",
            "away once its work is done (answered, decided, moved on). For the Owner and system admins.",
        ),
        json!({
            "type": "object",
            "properties": {
                "kinds": {
                    "type": "array",
                    "items": {"type": "string", "enum": KINDS},
                    "description": "Only these kinds (default all).",
                },
                "urgency": {
                    "type": "string",
                    "enum": work_queue::URGENCIES,
                    "description": "Only this urgency (default all).",
                },
                "limit": {
                    "type": "integer",
                    "description": "Most items to return (default nary.queue.default_limit, at most 500).",
                },
            },
        }),
        handler!(work_queue::_h_nary_work_queue),
        &["nary:view", "nary:approve"],
    )]
}
