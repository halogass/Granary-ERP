//! 3 Nov 2026 (Stream B2, Owner decisions 3 Nov): deal close-out and meeting tools for the MCP client and Nary's
//! chat, while internal Nary stays paused. The handlers are in nary::closeout.

use serde_json::json;

use super::{handler, Tool};
use crate::nary::closeout;

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "propose_contract_from_quotation",
            concat!(
                "Draft the contract from a quotation the customer accepted, for the user to confirm (what Nary does ",
                "on an accepted quotation). On confirm: a sent quotation is marked accepted (its follow-ups stop), ",
                "the contract draft is made and goes to the Owner's approval, the payment schedule is set from the ",
                "quotation's payment terms, the deal moves to Contract, and the down-payment proforma card follows ",
                "(or use propose_downpayment_proforma). Refused when the quotation already has a contract.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "quotation_id": {
                        "type": "string",
                        "description": "The quotation's id or its number (list_quotations).",
                    },
                },
                "required": ["quotation_id"],
            }),
            handler!(closeout::_h_propose_contract_from_quotation),
            &["sales:create_contract"],
        ),
        Tool::new(
            "propose_downpayment_proforma",
            concat!(
                "Raise the down-payment proforma invoice, for the user to confirm: for the quotation's first ",
                "milestone due on order without a proforma (or the payment_schedule_id given). Give quotation_id, or ",
                "deal_id for the deal's accepted quotation. The payment schedule comes from the contract draft ",
                "(propose_contract_from_quotation). Money: always a card for approval.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "deal_id": {
                        "type": "string",
                        "description": "The deal: its latest accepted or signed quotation.",
                    },
                    "quotation_id": {
                        "type": "string",
                        "description": "Or the quotation's id or number.",
                    },
                    "payment_schedule_id": {
                        "type": "string",
                        "description": "A specific payment milestone of that quotation (default: the down payment).",
                    },
                },
            }),
            handler!(closeout::_h_propose_downpayment_proforma),
            &["finance:create_invoice"],
        ),
        Tool::new(
            "propose_deal_won",
            concat!(
                "Move a deal to Won, for the user to confirm, with the checks of Nary's Won rule: a signed contract ",
                "on the deal or its quotations, and the down payment paid (a proforma with a payment) while ",
                "nary.contract.won_needs_dp is on. When a check fails it is refused with the reason. Only the Owner ",
                "can move it anyway: force=true with a reason (in the MCP client it waits for nary_apply). After ",
                "Won, start the project with propose_kickoff.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "deal_id": {
                        "type": "string",
                    },
                    "reason": {
                        "type": "string",
                        "description": "Why (kept with the change; required with force).",
                    },
                    "force": {
                        "type": "boolean",
                        "description": "The Owner only: move it although a check fails.",
                    },
                },
                "required": ["deal_id"],
            }),
            handler!(closeout::_h_propose_deal_won),
            &[closeout::WON_PERMISSION],
        ),
        Tool::new(
            "propose_kickoff",
            concat!(
                "Start the project of a WON deal, for the user to confirm (Nary's handoff): the ",
                "handoff package (scope sold and not sold, success criteria, commitments, red flags) is compiled ",
                "from the deal's history first when it has none (this can take a minute: the answer may be ",
                "still_working with a job_id; call again with only job_id). On confirm: a project for the deal, ",
                "its PM from the role table, the kickoff tasks, and the deal moves to Handover.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "deal_id": {
                        "type": "string",
                    },
                    "job_id": {
                        "type": "string",
                        "description": "Collect a result that was still being worked on.",
                    },
                },
            }),
            handler!(closeout::_h_propose_kickoff),
            &["projects:create"],
        ),
        Tool::new(
            "find_meeting_slots",
            concat!(
                "Find free meeting times (read only, nothing is booked) with Nary's meeting rules (working days, ",
                "start window, lunch and other blocks, buffer, minimum notice, meetings per day) against ",
                "the company calendar and the attendees' Google calendars (Google free/busy). Returns slots in ",
                "company time (and the customer's, with country). Book the chosen one with propose_meeting.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "attendee_user_ids": {
                        "type": "array",
                        "items": {
                            "type": "string",
                        },
                        "description": "ERP users who must be free too (their calendars are read when shared).",
                    },
                    "duration_minutes": {
                        "type": "integer",
                        "description": "Default: the meeting length in Nary's rules.",
                    },
                    "within_days": {
                        "type": "integer",
                        "description": "How many days ahead (default: Nary's rules; at most nary.meetings.find_max_days).",
                    },
                    "prefer": {
                        "type": "string",
                        "description": "morning, afternoon (company time) or a day YYYY-MM-DD to list first.",
                    },
                    "country": {
                        "type": "string",
                        "description": "The customer's country code (e.g. SA): their working hours and time zone count.",
                    },
                },
            }),
            handler!(closeout::_h_find_meeting_slots),
            &["nary:approve", "crm:edit_deal", "crm:edit_lead"],
        ),
        Tool::new(
            "transcribe_meeting_file",
            concat!(
                "Transcribe and read a meeting's recording or notes file now (what Nary does by itself every ",
                "minute): meeting_id (its newest file not read yet) or file_id. A recording goes to the server's ",
                "speech-to-text first; this can take many minutes: the answer may be still_working with a job_id, ",
                "call again with only job_id. When read, the results are stored as Nary stores them: the results ",
                "note and follow-ups on the deal, a card for a stage or value change, the summary email card, and a ",
                "message to the Owner. A chat file without its meeting needs meeting_id.",
            ),
            json!({
                "type": "object",
                "properties": {
                    "meeting_id": {
                        "type": "string",
                        "description": "The meeting (Nary > Meetings, list_meetings).",
                    },
                    "file_id": {
                        "type": "string",
                        "description": "A specific meeting file.",
                    },
                    "job_id": {
                        "type": "string",
                        "description": "Collect a result that was still being worked on.",
                    },
                },
            }),
            handler!(closeout::_h_transcribe_meeting_file),
            &["nary:approve"],
        ),
    ]
}
