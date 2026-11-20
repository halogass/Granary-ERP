//! 3 Nov 2026 (Owner decisions 3 Nov, Stream B1): the MCP client and the chat do internal Nary's WhatsApp and lead
//! work by hand while Nary is paused (nary::wa_leads): propose_whatsapp_lead, propose_held_chat_decision,
//! propose_lead_merge (cards, mcp_access::PROPOSAL_TOOLS) and score_lead (a read; save=true stores, at act and
//! full power only: mcp_access::ACT_ARGS).

use serde_json::json;

use super::{handler, Tool};
use crate::nary::wa_leads;

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "propose_whatsapp_lead",
            concat!(
                "Make a CRM lead from a WhatsApp chat, the way Nary does for a new prospect, for the user to ",
                "confirm (works while Nary is paused). Identity first: a number that belongs to a known contact ",
                "(or a certain identity match) gets a lead for that contact; a new number gets a new contact and ",
                "a lead (source WhatsApp, qualifying) plus a card to promote it to a deal. Refused when the chat ",
                "or its person already has an open lead or deal (the answer names it; link with ",
                "propose_chat_link instead), and for held, personal or staff chats (decide a held chat first ",
                "with propose_held_chat_decision). Nothing is sent to the customer.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "chat_id": {
                        "type": "string",
                        "description": "The WhatsApp chat id (list_recent_whatsapp or nary_work_queue). Give this or phone.",
                    },
                    "phone": {
                        "type": "string",
                        "description": "The chat's phone number, e.g. +1 555 0100...; used when chat_id is not given.",
                    },
                    "name": {
                        "type": "string",
                        "description": "The person's name for a new contact (default: the WhatsApp name).",
                    },
                    "organization": {
                        "type": "string",
                        "description": "Their company: the organization with that name, else a new one.",
                    },
                    "notes": {
                        "type": "string",
                        "description": "A note put on the lead (what they want, where they came from).",
                    },
                },
            }),
            handler!(wa_leads::_h_propose_whatsapp_lead),
            &[wa_leads::LEAD_PERMISSION],
        ),
        Tool::new(
            "propose_held_chat_decision",
            concat!(
                "Decide a held WhatsApp chat (one Nary does not read until the Owner says business or personal), ",
                "for the user to confirm. business: released, its messages are processed (while Nary is paused ",
                "they wait; make the lead with propose_whatsapp_lead). personal: excluded for good and ",
                "everything stored from it is deleted; in the MCP client at full power it waits for nary_apply.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "chat_id": {
                        "type": "string",
                        "description": "The held chat's id (nary_work_queue lists held chats).",
                    },
                    "decision": {
                        "type": "string",
                        "enum": ["business", "personal"],
                    },
                    "reason": {
                        "type": "string",
                        "description": "Why (shown on the card).",
                    },
                },
                "required": ["chat_id", "decision"],
            }),
            handler!(wa_leads::_h_propose_held_chat_decision),
            &[wa_leads::HELD_PERMISSION],
        ),
        Tool::new(
            "propose_lead_merge",
            concat!(
                "Merge two open leads of the same person into the one kept, for the user to confirm (Nary's ",
                "lead merge): the merged lead's activities, chats, emails and form submissions move to the one ",
                "kept and it is closed as disqualified (duplicate). In the MCP client at full power it waits for ",
                "nary_apply.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "keep_lead_id": {
                        "type": "string",
                        "description": "The lead that stays (usually the older or richer one).",
                    },
                    "merge_lead_id": {
                        "type": "string",
                        "description": "The duplicate that is merged into it.",
                    },
                },
                "required": ["keep_lead_id", "merge_lead_id"],
            }),
            handler!(wa_leads::_h_propose_lead_merge),
            &[wa_leads::LEAD_PERMISSION],
        ),
        // 5 Nov 2026: promote a lead from the MCP client (the lead.promote card)
        Tool::new(
            "propose_lead_promote",
            concat!(
                "Promote an open lead (new, triaging, qualifying or nurture) to a deal, for the user to confirm: ",
                "the deal starts at Qualifying with the lead's contact, organization, title and segment, ",
                "and the lead becomes promoted. A lead that is a deal already answers with its deal (nothing to ",
                "do). Then propose_quotation_link_deal links its quotation to the new deal. In the MCP client at ",
                "full power it applies at once.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "lead_id": {
                        "type": "string",
                        "description": "The lead's id (search_records, list_leads or nary_work_queue).",
                    },
                },
                "required": ["lead_id"],
            }),
            handler!(wa_leads::_h_propose_lead_promote),
            &[wa_leads::PROMOTE_PERMISSION],
        ),
        Tool::new(
            "score_lead",
            concat!(
                "Score a lead with Nary's BANT rubric (Budget, Authority, Need, Timeline, 0-3 each, total out ",
                "of 12, class from Settings) from its WhatsApp chats and emails; also the red and green flags, ",
                "what the customer's latest message asks for and the product family. Nothing is stored unless ",
                "save is true (then it is saved on the lead as Nary saves it; a new Hot lead alerts the Owner; ",
                "in the MCP client only at act or full power).",
            ),
            json!({
                "type": "object",
                "properties": {
                    "lead_id": {
                        "type": "string",
                    },
                    "save": {
                        "type": "boolean",
                        "description": "Store the score on the lead (default false).",
                    },
                },
                "required": ["lead_id"],
            }),
            handler!(wa_leads::_h_score_lead),
            &["crm:view_all", "crm:view_own"],
        ),
    ]
}
