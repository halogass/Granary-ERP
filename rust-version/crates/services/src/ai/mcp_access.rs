//! Port of backend/app/services/ai/mcp_access.py: what an MCP connection (an outside agent,
//! The MCP client...) may see and do, by its access level.
//!
//! A connection always acts as one person (its token's owner) with that person's permissions, and its
//! level narrows it further (migration 099): read, propose, act, owner, full. Tokens made before 099 carry
//! ["act"]. A scope list of `None` in Python behaves exactly like an empty list everywhere here, so the
//! Rust functions take `&[String]` (pass `&[]` for None).

use serde_json::{json, Map, Value};
use sqlx::PgConnection;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::nary::AgentPendingAction;

use crate::ai::tools::catalog::Tool;
use crate::nary::staff_chat::{self, PyExc};

pub const SCOPES: [&str; 5] = ["read", "propose", "act", "owner", "full"];
pub const LEVELS: [&str; 3] = ["read", "propose", "act"];
/// Tools that change something (in the chat they produce confirm cards).
pub const PROPOSAL_TOOLS: &[&str] = &[
    "propose_quotation_draft",
    "propose_invoice_draft",
    "propose_sop_draft",
    "propose_record_change",
    "propose_followup_reminder",
    "start_research",
    "propose_customer_message",
    "propose_chat_link",
    "propose_reminder_change",
    "propose_meeting",
    "propose_meeting_guests",
    "propose_meeting_cancel",
    "propose_contact_merge",
    // batch 2 T1: Solution Proposals from the chat
    "propose_solution_proposal",
    "update_requirements",
    // batch 2 T2: CRM hygiene (merge organizations, delete a contact)
    "propose_organization_merge",
    "propose_contact_delete",
    // R&D 26 Oct 2026: merge a deal entered twice; delete an activity; link an email thread
    "propose_deal_merge",
    "propose_activity_delete",
    "propose_email_link",
    // batch 2 T3: cancel a timed customer message (a read-level connection can't)
    "cancel_scheduled_message",
    // batch 3 R1: a meeting file goes to its meeting and is read (a read-level connection can't)
    "attach_meeting_notes",
    // batch 4 D3: change a saved draft quotation
    "propose_quotation_update",
    // doclib A3: send a library document to a customer
    "propose_document_send",
    // visuals V3: proposal figures and "Ask Nary to change..." (actions proposal.visual, proposal.edit)
    "propose_proposal_visual",
    "propose_proposal_visual_change",
    "propose_proposal_visual_remove",
    "propose_proposal_edit",
    // fix-longapply: revise a proposal instead of a duplicate, reject a draft
    "propose_proposal_revision",
    "propose_proposal_reject",
    // visuals3 P3B: a picture into a proposal (action proposal.visual)
    "propose_proposal_image",
    // 22 Oct 2026: link a quotation to a proposal, or take one off (action proposal.quotation)
    "propose_proposal_quotation",
    // 23 Oct 2026: a proposal written from the given sections (action proposal.from_sections)
    "propose_proposal_from_sections",
    // 23 Oct 2026: catalog changes by the Owner (action catalog.change)
    "propose_catalog_change",
    // 1 Nov 2026: reprice a draft quotation to today, withdraw one from approval, cancel one
    "propose_quotation_reprice",
    "propose_quotation_withdraw",
    "propose_quotation_cancel",
    // 2 Nov 2026: a new revision of a sent or expired quotation (action quotation.revision)
    "propose_quotation_revision",
    // 5 Nov 2026 (Stream C): link a sent or approved quotation to a deal without a revision (quotation.link_deal)
    "propose_quotation_link_deal",
    // 3 Nov 2026 (Stream B1): Nary's WhatsApp and lead work by hand while Nary is paused
    "propose_whatsapp_lead",
    "propose_held_chat_decision",
    "propose_lead_merge",
    // 5 Nov 2026: promote a lead to a deal (lead.promote)
    "propose_lead_promote",
    // 3 Nov 2026 (Stream B2): deal close-out by hand while Nary is paused
    "propose_contract_from_quotation",
    "propose_downpayment_proforma",
    "propose_deal_won",
    "propose_kickoff",
    // 5 Nov 2026 (Stream A): a file added to a catalog product's Product files (action catalog.product_file)
    "propose_product_file",
];
/// 29 Oct 2026: tools that store something at once, with no card (upload_file): listed only at act and full
/// power, never at read or propose.
pub const ACT_TOOLS: &[&str] = &[
    "upload_file",
    // 1 Nov 2026 (Owner decisions): direct proposal authoring, applied at once on a draft or in-review proposal
    "proposal_get",
    "proposal_create",
    "proposal_set_section",
    "proposal_set_block",
    "proposal_delete_block",
    "proposal_set_requirements",
    "figure_place",
    "figure_replace",
    "figure_remove",
    "proposal_add_image",
    "proposal_check",
    // 3 Nov 2026 (Stream A): the work queue reads customer data of every owner: act and full power only
    "nary_work_queue",
    // 3 Nov 2026 (Stream B2): a meeting file transcribed and read now (results stored on the deal)
    "transcribe_meeting_file",
    // 5 Nov 2026 (E7): the Owner's pricing override, written at once (audited)
    "set_pricing_override",
    // 5 Nov 2026: one-time upload links (a link stores files with no card, like upload_file)
    "create_upload_link",
    "get_upload_link",
    "revoke_upload_link",
];
/// 29 Oct 2026: arguments that store a file (preview_visual save_to): refused below act like ACT_TOOLS.
/// 3 Nov 2026: score_lead save (stores Nary's BANT score on the lead).
pub const ACT_ARGS: [(&str, &str); 2] = [("preview_visual", "save_to"), ("score_lead", "save")];

/// Why this call is refused at this connection's level (an ACT_ARGS argument below act), if it is.
pub fn act_arg_refusal(scopes: &[String], tool: &str, args: &Value) -> Option<String> {
    if level(scopes) == "act" {
        return None;
    }
    ACT_ARGS
        .iter()
        .find(|(t, a)| {
            *t == tool
                && args.get(*a).is_some_and(|v| {
                    !v.is_null() && v != &json!({}) && v != &json!("") && v != &json!(false)
                })
        })
        .map(|(_, a)| {
            // 3 Nov 2026: score_lead save stores a score, not a file
            let what = if tool == "score_lead" {
                "stores the score on the lead"
            } else {
                "stores a file"
            };
            format!(
                "Error: {tool} with {a} {what}, which this connection's access level ({}) can't do; call it \
                 without {a}.",
                scopes.join(", ")
            )
        })
}

/// The arguments a tool runs with over MCP: a file stored by upload_file or preview_visual save_to is marked as
/// uploaded via MCP (whatever the agent sent in that reserved argument).
pub fn mcp_args(tool: &str, mut args: Value) -> Value {
    // 1 Nov 2026: every ACT tool (the proposal authoring tools log price text with it)
    if ACT_TOOLS.contains(&tool) || ACT_ARGS.iter().any(|(t, _)| *t == tool) {
        if let Some(o) = args.as_object_mut() {
            o.insert(crate::nary::uploads::VIA_ARG.into(), json!("mcp"));
        }
    }
    args
}

/// Act on a chat conversation's own cards.
pub const CHAT_ONLY_TOOLS: [&str; 1] = ["withdraw_cards"];
/// Write a proposal memory / a backlog item directly.
pub const DIRECT_WRITES: [&str; 2] = ["propose_skill", "propose_tool_idea"];
pub const CRM_WRITE_TOOLS: [&str; 2] = ["create_crm_record", "update_crm_record"];
/// Even with "act", these stay cards: they reach a customer, or bill (a meeting: Google emails the customer).
pub const ALWAYS_CARD_KINDS: [&str; 3] = ["customer_message", "invoice", "meeting"];
/// Chat actions that reach the customer too (Google emails the invitees when guests join or a meeting
/// is cancelled).
pub const CUSTOMER_ACTIONS: [&str; 3] = [
    "meeting.add_guests",
    "meeting.cancel",
    // doclib A3: a library document to a customer
    "document.send",
];
/// With full power these apply after the two-step confirm; the tools that make them take `confirm`.
pub const CONFIRM_TOOLS: [&str; 7] = [
    "propose_customer_message",
    "propose_meeting",
    "propose_invoice_draft",
    "nary_decide_card",
    "propose_meeting_guests",
    "propose_meeting_cancel",
    // doclib A3
    "propose_document_send",
];

/// 23 Oct: chat actions that never apply in one call over MCP, whatever the level: the Owner's connection (full
/// power, or act with owner) gets the two-step confirm (final version, then the confirm code), any other a
/// pending card.
/// 5 Nov 2026: catalog.product_file (a product file added by the Owner) too.
pub const CATALOG_ACTIONS: [&str; 2] = ["catalog.change", "catalog.product_file"];
/// 23 Oct: the tools that make them take `confirm` on the Owner's connection.
pub const CATALOG_CONFIRM_TOOLS: [&str; 2] = ["propose_catalog_change", "propose_product_file"];

pub const GENERIC_INSTRUCTIONS: &str = "Granary. Read the prompt granary_assistant (or the resource granary://rules) first: it says how to work for the company. Everything you change follows your access level.";
const INSTRUCTIONS_MAX: usize = 12000;
const TOPICS_MAX: usize = 4000;

fn level_index(s: &str) -> usize {
    LEVELS.iter().position(|l| *l == s).unwrap_or(0)
}

/// `max(got, key=LEVELS.index)`: the first of the highest (Python's max keeps the first maximum).
fn max_level<'a>(got: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<&str> = None;
    for s in got {
        match best {
            Some(b) if level_index(s) <= level_index(b) => {}
            _ => best = Some(s),
        }
    }
    best
}

/// The connection's level; a token without one (made before 099) acts as before ("act").
pub fn level(scopes: &[String]) -> String {
    let got = scopes
        .iter()
        .map(String::as_str)
        .filter(|s| LEVELS.contains(s));
    match max_level(got) {
        Some(l) => l.to_string(),
        None => if scopes.is_empty() { "act" } else { "read" }.to_string(),
    }
}

pub fn has_owner(scopes: &[String]) -> bool {
    scopes.iter().any(|s| s == "owner")
}

pub fn has_full(scopes: &[String]) -> bool {
    scopes.iter().any(|s| s == "full") && has_owner(scopes)
}

pub fn crm_writes_allowed(scopes: &[String]) -> bool {
    matches!(level(scopes).as_str(), "propose" | "act")
}

pub async fn is_approver(db: &mut PgConnection, user: &AuthUser) -> Result<bool, ApiError> {
    if user.is_superadmin {
        return Ok(true);
    }
    Ok(crate::nary::roles::approver_ids(db)
        .await?
        .contains(&user.id))
}

/// The levels this person may give a connection: all three, plus full power for an approver.
pub async fn grantable(db: &mut PgConnection, user: &AuthUser) -> Result<Vec<String>, ApiError> {
    let mut out: Vec<String> = LEVELS.iter().map(|s| s.to_string()).collect();
    if is_approver(db, user).await? {
        out.push("full".into());
    }
    Ok(out)
}

/// A valid scope list for this person: one level (the highest asked), or full power for an approver.
/// Full power brings act and owner with it; an agent that asks for "owner" (the old Owner powers) gets full
/// power.
pub async fn clean_scopes(
    db: &mut PgConnection,
    user: &AuthUser,
    requested: &[String],
) -> Result<Vec<String>, ApiError> {
    let requested: Vec<&str> = requested
        .iter()
        .map(String::as_str)
        .filter(|s| SCOPES.contains(s))
        .collect();
    if requested.contains(&"full") || requested.contains(&"owner") {
        if !is_approver(db, user).await? {
            return Err(ApiError::http(
                403,
                "Full power is only for Nary's approver",
            ));
        }
        return Ok(vec!["act".into(), "owner".into(), "full".into()]);
    }
    let lvl = max_level(requested.into_iter().filter(|s| LEVELS.contains(s))).unwrap_or("propose");
    Ok(vec![lvl.to_string()])
}

/// `_how_over_mcp(scopes)`.
/// 10 Nov 2026 (Owner: AI clients follow the house formats and rules strictly).
pub const QUOTATION_RULE: &str = "- Quotations follow the house formats strictly: before a quotation card (draft, update, revision) call quotation_notes_template and pass the deal clauses as notes_deal; never write free notes when the template applies. When a tool or the quotation says the notes, tax or pricing check fails, fix it in the same turn (notes_deal, propose_quotation_update) before you say it is ready; never leave a quotation that the approval will refuse.";

pub fn how_over_mcp(scopes: &[String]) -> String {
    let mut lines: Vec<String> = vec![
        "HOW YOU WORK HERE (over MCP, for the person who connected you):".into(),
        "- You are working as Nary for this person. The rules above apply to everything you write.".into(),
        "- Look things up with the tools; the ERP guide is in granary://erp-guide/{topic}, the catalog in granary://catalog.".into(),
        "- Never say something was sent, booked or saved until a tool result says applied.".into(),
        "- Answer the person's newest message first; when a tool can't do something, say the exact reason it gave.".into(),
        QUOTATION_RULE.into(),
        "- Meetings: list_meetings for the booked ones; add guests (propose_meeting_guests) or cancel (propose_meeting_cancel) instead of booking a new event. Duplicate contacts: propose_contact_merge.".into(),
    ];
    let lvl = level(scopes);
    if lvl == "read" {
        lines.push("- This connection only reads: you can't change anything.".into());
    } else if lvl == "propose" {
        lines.push(
            "- Every change becomes an approval card in the ERP; say it waits for approval.".into(),
        );
    } else {
        lines.push(format!(
            "- Changes apply at once as this person. Messages to customers, meetings (booking, guests, cancelling) and invoices {}",
            if has_full(scopes) {
                "need the person's yes: the first call returns the final version and an apply_id; show the final version word for word, and only after a clear yes call nary_apply with that apply_id (your AI client (MCP) asks the person to Allow it; their click is the approval)."
            } else {
                "become approval cards in the ERP."
            }
        ));
    }
    if has_owner(scopes) {
        lines.push("- You speak with the Owner: nary_owner_context shows what Nary's WhatsApp chat with them shows (open topics, waiting cards, customer replies that need us, the autonomy levels, the last messages); decide cards with nary_decide_card.".into());
    }
    lines.join("\n")
}

/// Security 26 Oct: the Owner's connection also gets nary_apply (the step the person approves with the AI client's Allow
/// button); no tool takes a confirm code any more.
pub fn with_confirm(mut specs: Vec<Value>, scopes: &[String]) -> Vec<Value> {
    if has_owner(scopes) || has_full(scopes) {
        specs.push(apply_tool_spec());
    }
    specs
}

/// `_INSTRUCTIONS_MAX` cut: `text[:max - 1] + "..."` (U+2026) by code points.
fn cut_instructions(text: String) -> String {
    if text.chars().count() <= INSTRUCTIONS_MAX {
        return text;
    }
    let mut out: String = text.chars().take(INSTRUCTIONS_MAX - 1).collect();
    out.push('\u{2026}');
    out
}

#[doc(hidden)]
pub fn _cut_for_tests(text: String) -> String {
    cut_instructions(text)
}

/// `_owner_tools()`: nary_owner.OWNER_TOOLS.
pub fn _owner_tools() -> &'static [Tool] {
    &crate::ai::tools::nary_owner::OWNER_TOOLS
}

/// The tools this connection lists and may call (person's permissions and level).
pub async fn allowed_tools(
    db: &mut PgConnection,
    user: &AuthUser,
    scopes: &[String],
) -> ApiResult<Vec<Tool>> {
    use crate::ai::tools::catalog::{tools_for_user, user_can_use};
    let mut tools: Vec<Tool> = tools_for_user(user)
        .into_iter()
        .filter(|t| !CHAT_ONLY_TOOLS.contains(&t.name.as_str()))
        .collect();
    if level(scopes) == "read" {
        tools.retain(|t| {
            !PROPOSAL_TOOLS.contains(&t.name.as_str()) && !DIRECT_WRITES.contains(&t.name.as_str())
        });
    }
    // 29 Oct 2026: upload_file stores at once: act and full power only
    if level(scopes) != "act" {
        tools.retain(|t| !ACT_TOOLS.contains(&t.name.as_str()));
    }
    if has_owner(scopes) && is_approver(db, user).await? {
        tools.extend(
            _owner_tools()
                .iter()
                .filter(|t| user_can_use(user, t))
                .cloned(),
        );
    }
    // 23 Oct: catalog changes are listed for the Owner only
    crate::ai::tools::catalog::catalog_change_tools::keep_for_owner(db, user, &mut tools).await?;
    crate::nary::skills::drop_unconfigured_tools(db, &mut tools).await?;
    Ok(tools)
}

fn jwt_secret() -> String {
    granary_core::config::settings().JWT_SECRET_KEY.clone()
}

/// The value a second call must echo to apply a money card (binds the person, the card and the exact
/// edits).
pub fn confirm_value(
    user_id: uuid::Uuid,
    card_id: &str,
    decision: &str,
    edited: Option<&Value>,
) -> String {
    granary_compat::crypto::mcp::confirm_value(
        &jwt_secret(),
        &user_id.to_string(),
        card_id,
        decision,
        edited,
    )
}

thread_local! {
    /// Test seam for `_clock` (the Python tests monkeypatch it): seconds added to the wall clock.
    static CLOCK_SHIFT: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// Test seam: `monkeypatch.setattr(mcp_access, "_clock", lambda: real() + shift)` for this thread.
pub fn set_clock_shift(seconds: f64) {
    CLOCK_SHIFT.with(|c| c.set(seconds));
}

/// `_clock()`: `time.time()`.
pub fn _clock() -> f64 {
    let now = chrono::Utc::now();
    now.timestamp() as f64
        + f64::from(now.timestamp_subsec_micros()) / 1e6
        + CLOCK_SHIFT.with(|c| c.get())
}

fn as_map(proposal: &Value) -> Map<String, Value> {
    proposal.as_object().cloned().unwrap_or_default()
}

/// The one-time code that lets this person apply this exact version ("<issued>.<signature>").
pub fn card_code(user_id: uuid::Uuid, proposal: &Value, issued: Option<i64>) -> String {
    // int(time.time()) truncates toward zero
    let issued = issued.unwrap_or_else(|| _clock().trunc() as i64);
    granary_compat::crypto::mcp::card_code(
        &jwt_secret(),
        &user_id.to_string(),
        &as_map(proposal),
        issued,
    )
}

fn py_exc(e: ApiError) -> PyExc {
    PyExc::from(e)
}

/// None when the code is good (and now used); else why not: "changed", "expired" or "used".
pub async fn _check_code(
    db: &mut PgConnection,
    user: &AuthUser,
    proposal: &Value,
    code: &str,
) -> Result<Option<&'static str>, PyExc> {
    use granary_compat::crypto::mcp::{check_code, CodeCheck};
    let minutes = granary_core::settings_store::require_int(
        db,
        "mcp.owner.confirm_minutes",
        "Integrations & access → MCP access",
    )
    .await
    .map_err(|e| py_exc(e.into()))?;
    let checked = check_code(
        &jwt_secret(),
        &user.id.to_string(),
        &as_map(proposal),
        Some(code),
        _clock(),
        minutes,
    )
    .map_err(|_| {
        PyExc::new(
            "TypeError",
            "comparing strings with non-ASCII characters is not supported",
        )
    })?;
    let sig = match checked {
        CodeCheck::Changed => return Ok(Some("changed")),
        CodeCheck::Expired => return Ok(Some("expired")),
        CodeCheck::Good { sig } => sig,
    };
    let mut r = crate::redis_lease::client()
        .await
        .map_err(|e| py_exc(ApiError::from(e)))?;
    let fresh = crate::redis_lease::acquire(
        &mut r,
        &format!("mcp:confirm:{sig}"),
        "1",
        minutes * 60 + 60,
    )
    .await
    .map_err(|e| py_exc(ApiError::from(e)))?;
    Ok(if fresh { None } else { Some("used") })
}

async fn commit(db: &mut PgConnection) -> Result<(), PyExc> {
    crate::crm::followups::py_commit(db)
        .await
        .map_err(|e| py_exc(ApiError::from(e)))
}

/// `proposal.get("result") or {"label": label}`.
fn result_or_label(proposal: &Value, label: &str) -> Value {
    match proposal.get("result") {
        Some(v) if crate::crm::attributes::py::truthy(v) => v.clone(),
        _ => json!({"label": label}),
    }
}

/// Security 26 Oct: the tool that applies what a full-power call parked. AI clients (MCP) pause on it
/// (it is marked destructive) and the person clicks Allow: the AI can't apply a send, delete, merge or money
/// action by itself, even if text it read told it to.
pub const APPLY_TOOL: &str = "nary_apply";

/// Actions that, on the Owner's full-power connection, wait for nary_apply (deletes and merges; sends, money
/// and catalog changes are parked through their own paths).
pub const GATED_ACTIONS: &[&str] = &[
    "deal.merge",
    "contact.merge",
    "organization.merge",
    "lead.merge",
    "contact.delete",
    "activity.void",
    // 1 Nov 2026: cancelling a quotation
    "quotation.cancel",
];

/// 3 Nov 2026 (Stream B2): chat actions that are money (the down-payment proforma): a card on any connection, on
/// the Owner's full-power connection they wait for nary_apply (like an invoice).
pub const MONEY_ACTIONS: &[&str] = &["proforma.raise"];

/// 3 Nov 2026: a deal moved to Won past the Won rule's checks is the Owner's call: refused on a connection without
/// the Owner's level, and on the Owner's it waits for nary_apply.
pub const FORCE_WON_REFUSED: &str =
    "Moving a deal to Won past its checks (force) needs the Owner's connection \
(full power); without force it is refused with the failing check.";

/// 1 Nov 2026: a quotation card that gives a discount is money: on the Owner's full-power connection it waits for
/// nary_apply, on any other connection it is a card (like an invoice).
pub fn carries_discount(proposal: &Value) -> bool {
    let payload = proposal.get("payload").unwrap_or(&Value::Null);
    match proposal.get("kind").and_then(Value::as_str) {
        Some("quotation") => {
            let value = match payload.get("discount_value") {
                Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
                Some(Value::String(s)) => s.trim().parse::<f64>().unwrap_or(0.0),
                _ => 0.0,
            };
            value > 0.0
                || payload
                    .get("discount_note")
                    .and_then(Value::as_str)
                    .is_some_and(|n| !n.trim().is_empty())
        }
        Some("nary_action") => {
            proposal.get("action").and_then(Value::as_str) == Some("quotation.update")
                && payload
                    .get("discount_type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| !t.is_empty())
        }
        _ => false,
    }
}

/// 2 Nov 2026: a quotation card that sets a tax (a draft's `tax`, an update's `tax` or a line's `tax_code`) is money
/// like a discount: on the Owner's full-power connection it waits for nary_apply, on any other connection it is a
/// card.
pub fn carries_tax(proposal: &Value) -> bool {
    match proposal.get("kind").and_then(Value::as_str) {
        Some("quotation") => proposal
            .get("tax")
            .and_then(Value::as_str)
            .is_some_and(|t| !t.is_empty()),
        Some("nary_action") => {
            proposal.get("action").and_then(Value::as_str) == Some("quotation.update")
                && crate::nary::quotation_updates::sets_tax(
                    proposal.get("payload").unwrap_or(&Value::Null),
                )
        }
        _ => false,
    }
}

/// Stage 8: the setting that makes nary_apply ask for a TOTP code typed by the person (customer messages,
/// invoices, meetings, money, deletes and merges) when their account has TOTP on.
pub const APPLY_REQUIRE_TOTP_KEY: &str = "mcp.apply.require_totp";

/// Stage 8: the actions whose nary_apply needs the person's TOTP code (with [`APPLY_REQUIRE_TOTP_KEY`] on): the
/// always-card kinds (a customer message, an invoice, a meeting, a discount, a tax, money) and
/// [`GATED_ACTIONS`] (deletes, merges, a cancelled quotation, a held chat's stored data).
pub fn needs_step_up(proposal: &Value) -> bool {
    let action = proposal.get("action").and_then(Value::as_str).unwrap_or("");
    is_always_card(proposal)
        || GATED_ACTIONS.contains(&action)
        || crate::nary::wa_leads::deletes_chat(proposal)
}

/// Whether a proposal stays a card even with "act": it reaches a customer, bills, or is money.
fn is_always_card(proposal: &Value) -> bool {
    let mut kind = proposal.get("kind").cloned().unwrap_or(Value::Null);
    let action = proposal.get("action").and_then(Value::as_str).unwrap_or("");
    let has_message = proposal
        .get("message")
        .is_some_and(crate::crm::attributes::py::truthy);
    if kind.as_str() == Some("nary_action") && (CUSTOMER_ACTIONS.contains(&action) || has_message) {
        kind = json!("customer_message"); // reaches the customer: a card, or the two-step at full power
    }
    kind.as_str()
        .is_some_and(|k| ALWAYS_CARD_KINDS.contains(&k))
        || carries_discount(proposal)
        || carries_tax(proposal)
        || (kind.as_str() == Some("nary_action") && MONEY_ACTIONS.contains(&action))
}

/// The tool spec of nary_apply (for the Owner's connection).
pub fn apply_tool_spec() -> Value {
    json!({
        "name": APPLY_TOOL,
        "description": "Apply ONE action the user has just approved: a customer message, a delete, a merge, a money or \
    catalog change that an earlier call parked (it returned approval_required and an apply_id). Call it only after showing \
    the user the final version word for word; the user's Allow click on this call is the approval. Never call it twice \
    for one apply_id, and never ask the user to set it to Always allow. When the earlier call said totp_required, ask \
    the user to type the current 6-digit code from their authenticator app and pass exactly what they typed as \
    totp_code: the person types it, never you; never guess, reuse or make up a code.",
        "inputSchema": {"type": "object", "properties": {
            "apply_id": {"type": "string", "description": "The apply_id the earlier call returned."},
            "totp_code": {"type": "string", "description": "The current code from the user's authenticator app, \
    exactly as the user typed it in this chat (only when the parked action said totp_required)."}},
            "required": ["apply_id"]},
        "annotations": {"title": "Apply an approved action", "readOnlyHint": false,
                        "destructiveHint": true, "idempotentHint": false, "openWorldHint": true},
    })
}

/// Parks the exact final version for nary_apply (single use, mcp.owner.confirm_minutes) and tells the agent so.
async fn park(
    db: &mut PgConnection,
    user: &AuthUser,
    tool: &str,
    proposal: &Value,
    final_: &str,
) -> Result<Value, PyExc> {
    let minutes = granary_core::settings_store::require_int(
        db,
        "mcp.owner.confirm_minutes",
        "Integrations & access → MCP access",
    )
    .await
    .map_err(|e| py_exc(e.into()))?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    // Stage 8: whether nary_apply will need the person's TOTP code for this one
    let step_up = needs_step_up(proposal) && apply_needs_totp(db, user).await?;
    let stored = json!({"user_id": user.id.to_string(), "tool": tool, "proposal": proposal,
                        "final": final_, "step_up": step_up});
    let mut r = crate::redis_lease::client()
        .await
        .map_err(|e| py_exc(ApiError::from(e)))?;
    let _: () = redis::cmd("SET")
        .arg(format!("mcp:apply:{id}"))
        .arg(stored.to_string())
        .arg("EX")
        .arg((minutes * 60).max(1))
        .query_async(&mut r)
        .await
        .map_err(|e| py_exc(ApiError::from(e)))?;
    let totp_note = if step_up {
        " This one also needs the current code from the user's authenticator app: ask them to type it in this chat \
and pass it as totp_code (the user types it, never you)."
    } else {
        ""
    };
    Ok(
        json!({"ok": true, "applied": false, "approval_required": true, "apply_id": id,
              "final_version": final_, "expires_in_minutes": minutes, "totp_required": step_up,
              "note": format!("Nothing has happened yet. Show the user this final version word for word. If they want \
it, call {APPLY_TOOL} with this apply_id: the AI client asks them to Allow that call, and only their click applies it. If they \
want changes, call {tool} again with the changes.{totp_note}")}),
    )
}

/// Stage 8: `mcp.apply.require_totp` is on (a missing or unreadable value counts as on) and the person has TOTP.
async fn apply_needs_totp(db: &mut PgConnection, user: &AuthUser) -> Result<bool, PyExc> {
    let raw = granary_core::settings_store::get_optional(db, APPLY_REQUIRE_TOTP_KEY)
        .await
        .map_err(|e| py_exc(ApiError::from(e)))?;
    let on = raw.is_none_or(|v| v.trim().to_lowercase() != "false");
    if !on {
        return Ok(false);
    }
    crate::totp::is_enrolled(db, user.id).await.map_err(py_exc)
}

/// `nary_apply`: applies the parked action once (it must be this person's, and not expired).
pub async fn apply_parked(
    db: &mut PgConnection,
    user: &AuthUser,
    scopes: &[String],
    args: &Value,
) -> Result<Value, PyExc> {
    if !has_owner(scopes) && !has_full(scopes) {
        return Ok(json!({"ok": false, "applied": false,
                         "error": "This connection can't apply actions; approve the card in the ERP."}));
    }
    let id = args
        .get("apply_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let mut r = crate::redis_lease::client()
        .await
        .map_err(|e| py_exc(ApiError::from(e)))?;
    let key = format!("mcp:apply:{id}");
    // Stage 8: read first and use it up only once the checks pass (a missing code leaves it waiting)
    let raw: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut r)
        .await
        .map_err(|e| py_exc(ApiError::from(e)))?;
    let gone = || {
        json!({"ok": false, "applied": false,
               "error": "Nothing waits under this apply_id: it was applied already or it expired. Prepare it again."})
    };
    let Some(stored) = raw.and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
        return Ok(gone());
    };
    if stored["user_id"].as_str() != Some(user.id.to_string().as_str()) {
        return Ok(
            json!({"ok": false, "applied": false, "error": "This apply_id belongs to someone else."}),
        );
    }
    // Stage 8: customer messages, invoices, meetings, money, deletes and merges need the person's TOTP code
    // (mcp.apply.require_totp): an AI that read a planted instruction can click nothing in the person's app
    if stored["step_up"].as_bool() == Some(true) && apply_needs_totp(db, user).await? {
        let code = args
            .get("totp_code")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if code.is_empty() {
            return Ok(json!({"ok": false, "applied": false, "totp_required": true,
                             "error": "This action needs the current code from your authenticator app. Ask the user \
to type it here, then call nary_apply again with the same apply_id and totp_code. Nothing has happened yet."}));
        }
        if let Err(e) = crate::totp::verify_for_action(db, user.id, &code, "mcp.apply").await {
            let detail = e.body()["detail"]
                .as_str()
                .unwrap_or("That code is not valid.")
                .to_string();
            return Ok(json!({"ok": false, "applied": false, "totp_required": true,
                             "error": format!("{detail} Nothing has happened yet.")}));
        }
    }
    let taken: Option<String> = redis::cmd("GETDEL")
        .arg(&key)
        .query_async(&mut r)
        .await
        .map_err(|e| py_exc(ApiError::from(e)))?;
    if taken.is_none() {
        return Ok(gone());
    }
    let mut proposal = stored["proposal"].clone();
    let final_ = stored["final"].clone();
    commit(db).await?;
    let label = staff_chat::_apply_one(user, &mut proposal, None).await?;
    tracing::info!(tool = APPLY_TOOL, parked_by = ?stored["tool"].as_str(), kind = ?proposal.get("kind"),
                   user_id = %user.id, "mcp_owner_apply");
    if let Some(w) = still_working(&proposal) {
        return Ok(w);
    }
    Ok(
        json!({"ok": true, "applied": true, "result": result_or_label(&proposal, &label), "summary": final_}),
    )
}

/// The Owner's full power: show the final version with a code; with the code, apply it once.
async fn _full_apply(
    db: &mut PgConnection,
    user: &AuthUser,
    _args: &Value,
    proposal: &mut Value,
    tool: &str,
    _client: Option<&str>,
) -> Result<Value, PyExc> {
    // Security 26 Oct: never applied by calling the same tool again with a code (the AI could do that alone):
    // parked for nary_apply, which the person approves with the client's Allow button
    let final_ = staff_chat::describe(proposal);
    park(db, user, tool, proposal, &final_).await
}

/// A change from an agent as an approval card (mcp.proposal), applied as `user` once approved.
pub async fn as_card(
    db: &mut PgConnection,
    user: &AuthUser,
    proposal: &Value,
    client: Option<&str>,
    args: &Value,
) -> ApiResult<AgentPendingAction> {
    let who = if !user.name.is_empty() {
        user.name.clone()
    } else {
        user.email.clone()
    };
    let mut rationale = format!(
        "Proposed by {} for {who} (MCP).",
        client.filter(|c| !c.is_empty()).unwrap_or("an agent")
    );
    // 21 Nov 2026: the reason the agent gave goes on the card, as Nary's own cards say why
    if let Some(why) = client_reason(args, proposal) {
        rationale = format!("{why}\n{rationale}");
    }
    crate::nary::executor::propose(
        db,
        "mcp.proposal",
        json!({"proposal": proposal, "client": client, "proposer_id": user.id.to_string()}),
        user,
        crate::nary::executor::ProposeOpts {
            origin: "mcp",
            rationale: Some(&rationale),
            ..Default::default()
        },
    )
    .await
}

/// The reason an agent passed with a proposal: `rationale` / `reason` of the tool call, else of the proposal or
/// its payload.
fn client_reason(args: &Value, proposal: &Value) -> Option<String> {
    let places = [
        args,
        proposal,
        proposal.get("payload").unwrap_or(&Value::Null),
    ];
    places.iter().find_map(|v| {
        ["rationale", "reason"].iter().find_map(|k| {
            v.get(*k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(str::to_string)
        })
    })
}

/// What happens to a proposal a tool returned, by level. Returns the tool's answer to the agent.
/// `Err` is what Python raises (the MCP router turns a ValueError / HTTPException into an error result).
pub async fn handle_proposal(
    db: &mut PgConnection,
    user: &AuthUser,
    scopes: &[String],
    tool: &str,
    args: &Value,
    proposal: &mut Value,
    client: Option<&str>,
) -> Result<Value, PyExc> {
    let mut kind = proposal.get("kind").cloned().unwrap_or(Value::Null);
    let action = proposal.get("action").and_then(Value::as_str).unwrap_or("");
    let has_message = proposal
        .get("message")
        .is_some_and(crate::crm::attributes::py::truthy);
    if kind.as_str() == Some("nary_action") && (CUSTOMER_ACTIONS.contains(&action) || has_message) {
        kind = json!("customer_message"); // reaches the customer: a card, or the two-step at full power
    }
    let always_card = is_always_card(proposal);
    if _owner_tools().iter().any(|t| t.name == tool) {
        return _owner_decision(db, user, args, proposal).await;
    }
    // 3 Nov 2026: Won past the checks: the Owner's connection only, and only on nary_apply (the Allow click)
    if crate::nary::closeout::forces_won(proposal) {
        if !has_owner(scopes) || !is_approver(db, user).await.map_err(py_exc)? {
            return Ok(json!({"ok": false, "applied": false, "error": FORCE_WON_REFUSED}));
        }
        let final_ = staff_chat::describe(proposal);
        return park(db, user, tool, proposal, &final_).await;
    }
    // 23 Oct: a catalog change never applies in one call: the Owner's connection confirms the final version
    // with the code, any other connection makes a pending card
    if kind.as_str() == Some("nary_action") && CATALOG_ACTIONS.contains(&action) {
        if has_owner(scopes)
            && level(scopes) != "read"
            && is_approver(db, user).await.map_err(py_exc)?
        {
            return _full_apply(db, user, args, proposal, tool, client).await;
        }
        let card = as_card(db, user, proposal, client, args)
            .await
            .map_err(py_exc)?;
        return Ok(
            json!({"ok": true, "applied": false, "card_id": card.id.to_string(), "status": card.status,
                  "summary": staff_chat::describe(proposal),
                  "note": "Waiting for approval in the ERP (Approvals page, or 1 on WhatsApp); nothing has changed yet."}),
        );
    }
    if always_card && has_full(scopes) && is_approver(db, user).await.map_err(py_exc)? {
        return _full_apply(db, user, args, proposal, tool, client).await;
    }
    // Security 26 Oct: a delete or merge on the Owner's full-power connection waits for nary_apply
    // 3 Nov 2026: a held chat decided personal deletes what was stored from it
    if has_full(scopes)
        && (GATED_ACTIONS.contains(&action) || crate::nary::wa_leads::deletes_chat(proposal))
    {
        let final_ = staff_chat::describe(proposal);
        return park(db, user, tool, proposal, &final_).await;
    }
    if level(scopes) == "act" && !always_card {
        commit(db).await?;
        let label = staff_chat::_apply_one(user, proposal, None).await?;
        if let Some(w) = still_working(proposal) {
            return Ok(w);
        }
        return Ok(
            json!({"ok": true, "applied": true, "result": result_or_label(proposal, &label),
                         "summary": staff_chat::describe(proposal)}),
        );
    }
    let card = as_card(db, user, proposal, client, args)
        .await
        .map_err(py_exc)?;
    Ok(
        json!({"ok": true, "applied": false, "card_id": card.id.to_string(), "status": card.status,
              "summary": staff_chat::describe(proposal),
              "note": "Waiting for approval in the ERP (Approvals page, or 1 on WhatsApp); nothing has changed yet."}),
    )
}

/// An Owner tool's decision applies at once, except applying a money card, which needs the second call.
async fn _owner_decision(
    db: &mut PgConnection,
    user: &AuthUser,
    args: &Value,
    proposal: &mut Value,
) -> Result<Value, PyExc> {
    let auto = proposal
        .get("auto")
        .is_some_and(crate::crm::attributes::py::truthy);
    if proposal.get("kind").and_then(Value::as_str) == Some("nary_decision") && !auto {
        // proposal["card_id"], proposal["decision"]: str() of the values
        let s =
            |k: &str| crate::crm::attributes::py::str_of(proposal.get(k).unwrap_or(&Value::Null));
        let want = confirm_value(
            user.id,
            &s("card_id"),
            &s("decision"),
            proposal.get("edited_payload"),
        );
        // Security 26 Oct: money waits for nary_apply (the person's Allow click)
        let _ = (want, args);
        let final_ = proposal
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        return park(db, user, "nary_decide_card", proposal, &final_).await;
    }
    commit(db).await?;
    let label = staff_chat::_apply_one(user, proposal, None).await?;
    if let Some(w) = still_working(proposal) {
        return Ok(w);
    }
    Ok(
        json!({"ok": true, "applied": true, "result": {"label": label}, "summary": staff_chat::describe(proposal)}),
    )
}

/// 23 Oct: one log line per tools/call: the tool, who, the connection's level, the card it made or applied, and
/// how long it took. No arguments (they hold customer data). A later "I rejected it but it ran" is checked
/// against what the ERP received and when.
pub fn log_call(
    tool: &str,
    user: &AuthUser,
    scopes: &[String],
    client: &str,
    out: &Value,
    started: std::time::Instant,
) {
    let lvl = if has_full(scopes) {
        "full".to_string()
    } else {
        level(scopes)
    };
    let text = |v: Option<&Value>| match v {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let card_id = text(out.get("card_id"));
    let result_id = text(out.get("result").and_then(|r| r.get("id")));
    let applied = out.get("applied") == Some(&Value::Bool(true));
    let still_working = out.get("still_working") == Some(&Value::Bool(true));
    let error = out.get("isError") == Some(&Value::Bool(true))
        || out
            .get("error")
            .is_some_and(crate::crm::attributes::py::truthy);
    tracing::info!(tool, user_id = %user.id, level = %lvl, client, card_id = %card_id,
                   result_id = %result_id, applied, still_working, error,
                   ms = started.elapsed().as_millis() as u64, "mcp_tool_call");
}

/// fix-longapply: the answer when the work is still running after the wait (a normal result, not an error).
fn still_working(proposal: &Value) -> Option<Value> {
    if proposal.get("still_working") != Some(&json!(true)) {
        return None;
    }
    let id = proposal
        .get("result")
        .and_then(|r| r.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    let (note, key) = match proposal.get("kind").and_then(Value::as_str) {
        Some("nary_decision") | Some("nary_action") => {
            (crate::nary::long_work::STILL_WORKING_MCP, "card_id")
        }
        _ => (crate::nary::long_work::STILL_WORKING_DOC, "job_id"),
    };
    let mut out = json!({"ok": true, "applied": false, "still_working": true,
                         "summary": staff_chat::describe(proposal), "note": note});
    out[key] = id;
    Some(out)
}

// ─── Context for the agent: resources and the prompt ─────────────────────────

pub async fn resources(
    db: &mut PgConnection,
    user: &AuthUser,
    scopes: &[String],
) -> ApiResult<Vec<Value>> {
    let mut items = vec![
        json!({"uri": "granary://rules", "name": "How to work for the company", "mimeType": "text/plain",
               "description": "Nary's identity and rules: list prices only, tiers private, no em dashes, cards for changes."}),
        json!({"uri": "granary://erp-guide", "name": "ERP guide (sections)", "mimeType": "application/json",
               "description": "How the ERP works; read a section with granary://erp-guide/{topic}."}),
        json!({"uri": "granary://catalog", "name": "Product catalog", "mimeType": "text/plain",
               "description": "Every active product, how it is sold and its list price."}),
        json!({"uri": "granary://skills", "name": "Learnt skills", "mimeType": "text/plain",
               "description": "Procedures the Owner approved."}),
    ];
    if has_owner(scopes) && is_approver(db, user).await? {
        items.push(json!({"uri": "granary://style-guide", "name": "The Owner's writing style", "mimeType": "text/plain",
                          "description": "Write like the Owner (WhatsApp and email sections)."}));
    }
    Ok(items)
}

/// `json.dumps(value, ensure_ascii=False)`.
fn dumps_unicode(v: &Value) -> ApiResult<String> {
    use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
    dumps(&PyValue::from(v), &DumpOpts::unicode())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))
}

/// (mime type, text) of one resource.
pub async fn read_resource(
    db: &mut PgConnection,
    user: &AuthUser,
    scopes: &[String],
    uri: &str,
) -> ApiResult<(String, String)> {
    use crate::ai::tools::catalog::can_see_costs;
    if uri == "granary://rules" {
        let cfg = crate::ai::registry::get_ai_config(db).await?;
        return Ok((
            "text/plain".into(),
            crate::ai::assistant::build_system_prompt(&cfg),
        ));
    }
    if uri == "granary://erp-guide" {
        let out = crate::ai::erp_guide::section(db, "").await?;
        return Ok(("application/json".into(), dumps_unicode(&out)?));
    }
    if uri.starts_with("granary://erp-guide/") {
        // uri.split("/", 3)[-1]
        let topic = uri.splitn(4, '/').last().unwrap_or("");
        let out = crate::ai::erp_guide::section(db, topic).await?;
        let text = match out.get("text") {
            Some(v) if crate::crm::attributes::py::truthy(v) => {
                crate::crm::attributes::py::str_of(v)
            }
            _ => dumps_unicode(&out)?,
        };
        return Ok(("text/plain".into(), text));
    }
    if uri == "granary://catalog" {
        let detail = if can_see_costs(user) { "full" } else { "list" };
        return Ok((
            "text/plain".into(),
            crate::ai::catalog_digest::digest(db, detail).await?,
        ));
    }
    if uri == "granary://skills" {
        let text = crate::nary::skills::prompt_list(db).await?;
        return Ok((
            "text/plain".into(),
            if text.is_empty() {
                "No approved skills yet.".into()
            } else {
                text
            },
        ));
    }
    if uri == "granary://style-guide" && has_owner(scopes) && is_approver(db, user).await? {
        let text = crate::nary::persona::style_guide_text(db).await?;
        return Ok((
            "text/plain".into(),
            if text.is_empty() {
                "No approved style guide yet.".into()
            } else {
                text
            },
        ));
    }
    Err(ApiError::http(404, format!("Unknown resource {uri}")))
}

/// The handshake's instructions for a logged-in connection: Nary's rules, how to work over MCP, and for
/// the Owner the open topics, so any client starts working like Nary without choosing a prompt.
pub async fn instructions(
    db: &mut PgConnection,
    user: &AuthUser,
    scopes: &[String],
) -> ApiResult<String> {
    // How to work here first: a client that cuts long instructions keeps it (the rules are also granary://rules).
    let mut parts: Vec<String> = vec![
        how_over_mcp(scopes),
        crate::ai::assistant::now_line(db).await?
            + " (when this connection started; nary_owner_context gives the time now)",
    ];
    let cfg = crate::ai::registry::get_ai_config(db).await?;
    parts.push(crate::ai::assistant::build_system_prompt(&cfg));
    if has_owner(scopes) && is_approver(db, user).await? {
        let topics = crate::nary::owner_channel::context_block(db, None).await?;
        if !topics.is_empty() {
            parts.push(topics.chars().take(TOPICS_MAX).collect());
        }
    }
    let text = parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(cut_instructions(text))
}

/// The prompt Nary itself works with (rules, ERP map, skills, the catalog the person may see).
pub async fn assistant_prompt(db: &mut PgConnection, user: &AuthUser) -> ApiResult<String> {
    use crate::ai::tools::catalog::{can_see_costs, tool_by_name, user_can_use};
    let cfg = crate::ai::registry::get_ai_config(db).await?;
    let search = tool_by_name("search_catalog")
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: 'search_catalog'")))?;
    let detail = if can_see_costs(user) { "full" } else { "list" };
    Ok(crate::ai::assistant::stable_prompt(db, &cfg, user_can_use(user, &search), detail).await)
}
