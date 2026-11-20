//! 3 Nov 2026 (the MCP client as Nary while internal Nary stays paused, Stream B2, Owner decisions 3 Nov): the deal
//! close-out and meeting work Nary does by itself, as tools for the MCP client and Nary's internal chat.
//! Nary itself stays paused: nothing here runs unless a person calls the tool.
//!
//! ```text
//!     propose_contract_from_quotation   card contract.create_draft (what Nary raises on quotation.accepted): the
//!                                       quotation marked accepted, the contract draft (its own Owner approval),
//!                                       the payment schedule from the terms, then the down-payment proforma card
//!     propose_downpayment_proforma      card proforma.raise for the quotation's first on-order milestone (or the
//!                                       one named): money, so a card everywhere and nary_apply at full power
//!     propose_deal_won                  card deal.won: the Won rule's checks (deals::won_blocker: a signed
//!                                       contract, the down payment paid when nary.contract.won_needs_dp); a
//!                                       failing check refuses with its reason, unless the Owner forces it with a
//!                                       reason (force=true; nary_apply at full power, the Owner's level only)
//!     propose_kickoff                   card handoff.kickoff (project, PM, kickoff tasks); the handoff package is
//!                                       compiled first when the deal has none (the same model call as on Won)
//!     find_meeting_slots                read only: free slots (slots.rs) against the company calendar and the
//!                                       attendees' calendars (Google free/busy); propose_meeting books one
//!     transcribe_meeting_file           a meeting's recording or notes file transcribed and debriefed now, as the
//!                                       minute tick would (long work: job_id), results stored as Nary stores them
//! ```
//!
//! The pause: executor::propose refuses Nary's cards while Nary is paused. A close-out card from the chat or
//! The MCP client is applied inside executor::on_request (executor::ON_REQUEST_ACTIONS), so the card it raises
//! next (the down-payment proforma after the contract draft) is raised; the meeting file's debrief runs inside
//! it too. Nary's own work (no request) is unchanged.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::clock::local_tz;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use granary_db::rows::crm::Deal;
use granary_db::rows::finance::PaymentSchedule;
use granary_db::rows::sales::{Contract, Quotation};

use super::actions::{apply_fn, entity_fn, need_actor, to_uuid, Action, Field, Schema, Ty};
use super::{deals, meeting_files, meetings, slots};
use crate::ai::tools::catalog::long_tools;
use crate::crm::stages;

pub const CONTRACT: &str = "contract.create_draft";
pub const PROFORMA: &str = "proforma.raise";
pub const WON: &str = "deal.won";
pub const KICKOFF: &str = "handoff.kickoff";
/// Who may move a deal to Won (the deal editor's permission).
pub const WON_PERMISSION: &str = "crm:edit_deal";
pub const FORCE_REFUSED: &str =
    "Only the Owner can move a deal to Won past the Won rule's checks (force).";

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn text(args: &Value, k: &str) -> Option<String> {
    match args.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn flag(args: &Value, k: &str) -> bool {
    match args.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1"),
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        _ => false,
    }
}

fn holds(user: &AuthUser, code: &str) -> bool {
    user.is_superadmin || user.perms.contains(code)
}

fn money(v: Option<rust_decimal::Decimal>) -> String {
    v.map(|d| d.normalize().to_string())
        .unwrap_or_else(|| "?".into())
}

fn qnumber(q: &Quotation) -> String {
    q.full_number
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| q.number.clone())
}

/// "chat" in Nary's chat, "mcp" over the MCP client (the MCP router marks the act tools' arguments).
fn origin_of(args: &Value) -> &'static str {
    if super::uploads::via_of(args) == "mcp" {
        "mcp"
    } else {
        "chat"
    }
}

async fn load_deal(db: &mut PgConnection, args: &Value) -> ApiResult<Result<Deal, String>> {
    let Some(raw) = text(args, "deal_id") else {
        return Ok(Err(
            "Give deal_id (search_records or get_record find it).".into()
        ));
    };
    let Ok(id) = Uuid::parse_str(&raw) else {
        return Ok(Err(format!("deal_id {raw} is not a valid id.")));
    };
    let deal: Option<Deal> =
        sqlx::query_as("SELECT * FROM deals WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    Ok(deal.ok_or_else(|| "Deal not found.".to_string()))
}

async fn open_card(db: &mut PgConnection, action: &str, key: &str, id: Uuid) -> ApiResult<bool> {
    deals::_open_card(db, action, key, &id.to_string()).await
}

// ─── propose_contract_from_quotation ──────────────────────────────────────────

/// `propose_contract_from_quotation`: the contract.create_draft card for a sent, accepted or signed quotation.
pub async fn _h_propose_contract_from_quotation(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !holds(user, "sales:create_contract") {
        return err(
            "Drafting a contract needs the permission to create contracts (sales:create_contract).",
        );
    }
    let Some(q) = super::quotation_updates::find(db, &args).await? else {
        return err("Quotation not found: give quotation_id or its number (list_quotations).");
    };
    let n = qnumber(&q);
    let status = q.status.clone().unwrap_or_default();
    if !matches!(status.as_str(), "sent" | "accepted" | "signed") {
        return err(format!(
            "Quotation {n} is {status}: only a sent, accepted or signed quotation becomes a contract."
        ));
    }
    let has: Option<Contract> = sqlx::query_as(
        "SELECT * FROM contracts WHERE quotation_id = $1 AND is_deleted = false LIMIT 1",
    )
    .bind(q.id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(c) = has {
        return err(format!(
            "Quotation {n} already has contract {} ({}). For the down payment use propose_downpayment_proforma.",
            c.number,
            c.status.unwrap_or_default()
        ));
    }
    if open_card(db, CONTRACT, "quotation_id", q.id).await? {
        return err(format!(
            "A contract draft card for quotation {n} already waits for approval (Nary > Approvals)."
        ));
    }
    let mut summary = format!(
        "Contract draft from quotation {n} ({} {})",
        q.currency.clone().unwrap_or_default(),
        money(q.total)
    );
    if status == "sent" {
        summary.push_str(": the quotation is marked accepted");
    }
    summary.push_str(
        ". The contract goes to the Owner's approval, the payment schedule comes from the quotation's terms, and \
         the down-payment proforma card follows.",
    );
    crate::ai::tools::catalog::_autonomy_card(
        db,
        user,
        CONTRACT,
        json!({"quotation_id": q.id.to_string()}),
        summary,
        None,
    )
    .await
}

// ─── propose_downpayment_proforma ─────────────────────────────────────────────

/// The quotation the proforma is for: quotation_id (or number), else the deal's latest accepted or signed one.
async fn proforma_quotation(
    db: &mut PgConnection,
    args: &Value,
) -> ApiResult<Result<Quotation, String>> {
    if text(args, "quotation_id").is_some() || text(args, "number").is_some() {
        return Ok(super::quotation_updates::find(db, args)
            .await?
            .ok_or_else(|| {
                "Quotation not found: give quotation_id or its number (list_quotations)."
                    .to_string()
            }));
    }
    let deal = match load_deal(db, args).await? {
        Ok(d) => d,
        Err(_) if text(args, "deal_id").is_none() => {
            return Ok(Err("Give deal_id or quotation_id.".into()))
        }
        Err(why) => return Ok(Err(why)),
    };
    Ok(
        deals::latest_quotation(db, deal.id, &["accepted", "signed"])
            .await?
            .ok_or_else(|| {
                format!(
                    "{} has no accepted or signed quotation: draft the contract first \
                     (propose_contract_from_quotation), it marks the quotation accepted and sets the payment schedule.",
                    deal.title
                )
            }),
    )
}

/// `propose_downpayment_proforma`: the proforma.raise card for the quotation's down-payment milestone.
pub async fn _h_propose_downpayment_proforma(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !holds(user, "finance:create_invoice") {
        return err(
            "Raising a proforma needs the permission to create invoices (finance:create_invoice).",
        );
    }
    let q = match proforma_quotation(db, &args).await? {
        Ok(q) => q,
        Err(why) => return err(why),
    };
    let n = qnumber(&q);
    let schedule: Vec<PaymentSchedule> = sqlx::query_as(
        "SELECT * FROM payment_schedules WHERE quotation_id = $1 AND is_deleted = false ORDER BY sequence",
    )
    .bind(q.id)
    .fetch_all(&mut *db)
    .await?;
    let chosen = match text(&args, "payment_schedule_id") {
        Some(raw) => {
            let Some(m) = schedule.iter().find(|s| s.id.to_string() == raw) else {
                return err(format!(
                    "Payment milestone {raw} is not one of quotation {n}'s milestones."
                ));
            };
            m.clone()
        }
        None => {
            if schedule.is_empty() {
                return err(format!(
                    "Quotation {n} has no payment schedule yet: draft the contract first \
                     (propose_contract_from_quotation), which sets it from the quotation's payment terms."
                ));
            }
            match schedule.iter().find(|s| {
                s.due_trigger.as_deref() == Some("on_order") && s.proforma_invoice_id.is_none()
            }) {
                Some(m) => m.clone(),
                None => {
                    return err(format!(
                        "Quotation {n} has no down-payment milestone (due on order) without a proforma: give \
                         payment_schedule_id for another milestone."
                    ))
                }
            }
        }
    };
    if let Some(inv) = chosen.proforma_invoice_id {
        let number: Option<String> =
            sqlx::query_scalar("SELECT COALESCE(full_number, number) FROM invoices WHERE id = $1")
                .bind(inv)
                .fetch_optional(&mut *db)
                .await?
                .flatten();
        return err(format!(
            "Milestone '{}' already has proforma {}.",
            chosen.name,
            number.unwrap_or_else(|| inv.to_string())
        ));
    }
    if open_card(db, PROFORMA, "schedule_id", chosen.id).await? {
        return err(format!(
            "A proforma card for milestone '{}' already waits for approval (Nary > Approvals).",
            chosen.name
        ));
    }
    let share = match chosen.percentage.filter(|p| !p.is_zero()) {
        Some(p) => format!("{}% of ", p.normalize()),
        None => String::new(),
    };
    let summary = format!(
        "Proforma for '{}' ({share}{} {}) on quotation {n}",
        chosen.name,
        q.currency.clone().unwrap_or_default(),
        money(q.total)
    );
    crate::ai::tools::catalog::_autonomy_card(
        db,
        user,
        PROFORMA,
        json!({"schedule_id": chosen.id.to_string()}),
        summary,
        None,
    )
    .await
}

// ─── propose_deal_won / deal.won ──────────────────────────────────────────────

/// `propose_deal_won`: the deal.won card when the Won rule's checks pass; refused with the reason when they don't,
/// unless the Owner forces it (force=true with a reason).
pub async fn _h_propose_deal_won(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !holds(user, WON_PERMISSION) {
        return err("Moving a deal to Won needs the permission to edit deals (crm:edit_deal).");
    }
    let deal = match load_deal(db, &args).await? {
        Ok(d) => d,
        Err(why) => return err(why),
    };
    if stages::WON_STAGES.contains(&deal.stage.as_str()) {
        return err(format!(
            "{} is already won ({}). Start the project with propose_kickoff.",
            deal.title, deal.stage
        ));
    }
    let reason = text(&args, "reason");
    let force = flag(&args, "force");
    let blocker = deals::won_blocker(db, &deal).await?;
    let mut payload = json!({"deal_id": deal.id.to_string()});
    if let Some(r) = &reason {
        payload["reason"] = json!(r);
    }
    let summary = match &blocker {
        None => format!(
            "Move {} to Won: the contract is signed and the down payment checks pass{}",
            deal.title,
            reason.as_ref().map(|r| format!(". {r}")).unwrap_or_default()
        ),
        Some(why) if !force => {
            return err(format!(
                "{} can't move to Won yet: {why} Not moved. When it must be Won anyway, only the Owner can force it \
                 (force=true with a reason).",
                deal.title
            ))
        }
        Some(why) => {
            if !super::quotation_updates::is_owner(db, user).await? {
                return err(FORCE_REFUSED);
            }
            let Some(r) = &reason else {
                return err("Say why (reason) when moving a deal to Won past its checks.");
            };
            payload["force"] = json!(true);
            format!(
                "Move {} to Won PAST THE CHECKS (forced by the Owner). Failing check: {why} Reason: {r}",
                deal.title
            )
        }
    };
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, WON, payload, summary, None).await?;
    if let Some(why) = blocker {
        out["failing_check"] = json!(why);
    }
    Ok(out)
}

async fn _apply_won(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    let id = to_uuid(&p["deal_id"])?;
    let deal: Option<Deal> =
        sqlx::query_as("SELECT * FROM deals WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let Some(mut deal) = deal else {
        return Err(ApiError::http(404, "Deal not found"));
    };
    if stages::WON_STAGES.contains(&deal.stage.as_str()) {
        return Err(ApiError::http(
            409,
            format!("{} is already won ({}).", deal.title, deal.stage),
        ));
    }
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let reason = text(p, "reason");
    let blocker = deals::won_blocker(db, &deal).await?;
    if let Some(why) = &blocker {
        if !force {
            return Err(ApiError::http(409, format!("Not moved to Won: {why}")));
        }
        if !super::quotation_updates::is_owner(db, actor).await? {
            return Err(ApiError::http(403, FORCE_REFUSED));
        }
    }
    let note = match (&blocker, &reason) {
        (Some(why), r) => format!(
            "Moved to Won by {} past the checks ({why}) Reason: {}",
            actor.name,
            r.clone().unwrap_or_default()
        ),
        (None, Some(r)) => r.clone(),
        (None, None) => "Contract signed and down payment received.".to_string(),
    };
    let from = deal.stage.clone();
    stages::change_deal_stage(
        db,
        &mut deal,
        "won",
        Some(actor.id),
        "chat",
        None,
        Some(&note),
        None,
    )
    .await?;
    super::commit(db).await?;
    Ok(json!({
        "deal_id": deal.id.to_string(), "from": from, "stage": "won", "forced": blocker.is_some(),
        "label": format!("{} moved to Won", deal.title),
        "next": "Start the project with propose_kickoff (handoff package, project, PM and kickoff tasks).",
    }))
}

async fn _describe_won(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let title: Option<String> = sqlx::query_scalar("SELECT title FROM deals WHERE id = $1")
        .bind(to_uuid(&p["deal_id"])?)
        .fetch_optional(&mut *db)
        .await?;
    let forced = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    Ok(json!({"label": format!(
        "Move {} to Won{}",
        title.unwrap_or_else(|| "?".into()),
        if forced { " (forced past the checks)" } else { "" }
    )}))
}

/// The actions of this module (registered by actions::module_actions).
pub fn actions() -> Vec<Action> {
    vec![Action::new(
        WON,
        "Move a deal to Won",
        Schema::strict(
            "DealWon",
            vec![
                Field::req("deal_id", Ty::Str),
                Field::opt("reason", Ty::Str),
                Field::opt("force", Ty::Bool),
            ],
        ),
        apply_fn(|db, a, p| Box::pin(_apply_won(db, a, p))),
    )
    .permission(WON_PERMISSION)
    .describe(apply_fn(|db, a, p| Box::pin(_describe_won(db, a, p))))
    .entity(entity_fn(|p| {
        Ok((Some("deal".into()), Some(to_uuid(&p["deal_id"])?)))
    }))]
}

/// Whether this card moves a deal to Won past its checks (on the Owner's full-power connection it waits for
/// nary_apply; refused on any connection without the Owner's level).
pub fn forces_won(proposal: &Value) -> bool {
    proposal.get("kind").and_then(Value::as_str) == Some("nary_action")
        && proposal.get("action").and_then(Value::as_str) == Some(WON)
        && proposal
            .get("payload")
            .and_then(|p| p.get("force"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

// ─── propose_kickoff ──────────────────────────────────────────────────────────

fn still_working_note(out: &mut Value, tool: &str, what: &str) {
    if out.get("still_working") == Some(&json!(true)) {
        let job = out["job_id"].as_str().unwrap_or("").to_string();
        out["note"] = json!(format!(
            "Still working ({what}). Nothing is finished yet. Call {tool} again with only job_id {job} in a minute \
             to get the result."
        ));
    }
}

/// `propose_kickoff`: the handoff.kickoff card for a won deal (its handoff package compiled first when missing).
pub async fn _h_propose_kickoff(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !holds(user, "projects:create") {
        return err(
            "Starting the project needs the permission to create projects (projects:create).",
        );
    }
    if let Some(job) = text(&args, "job_id") {
        return long_tools::collect(db, user, "propose_kickoff", &job).await;
    }
    let deal = match load_deal(db, &args).await? {
        Ok(d) => d,
        Err(why) => return err(why),
    };
    if !stages::WON_STAGES.contains(&deal.stage.as_str()) {
        return err(format!(
            "{} is {}, not won: move it to Won first (propose_deal_won).",
            deal.title, deal.stage
        ));
    }
    let project: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT project_number FROM projects WHERE deal_id = $1 AND is_deleted = false LIMIT 1",
    )
    .bind(deal.id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((number,)) = project {
        return err(format!(
            "Project {} already exists for {}.",
            number.unwrap_or_default(),
            deal.title
        ));
    }
    if open_card(db, KICKOFF, "deal_id", deal.id).await? {
        return err(format!(
            "A kickoff card for {} already waits for approval (Nary > Approvals).",
            deal.title
        ));
    }
    let has_package: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM nary_insights WHERE deal_id = $1 AND kind = 'handoff')",
    )
    .bind(deal.id)
    .fetch_one(&mut *db)
    .await?;
    if has_package {
        return kickoff_now(db, user, args).await;
    }
    // the package is a model call over the whole history: it may outlast the proxy
    let mut out = long_tools::run_long(
        db,
        user,
        "propose_kickoff",
        args,
        crate::ai_tool_handler!(kickoff_now),
    )
    .await?;
    still_working_note(&mut out, "propose_kickoff", "compiling the handoff package");
    Ok(out)
}

/// The package (compiled now when the deal has none) and the kickoff card.
async fn kickoff_now(db: &mut PgConnection, user: &AuthUser, args: Value) -> ApiResult<Value> {
    let deal = match load_deal(db, &args).await? {
        Ok(d) => d,
        Err(why) => return err(why),
    };
    let package = super::handoff::package_on_request(db, &deal).await?;
    let pm: Option<String> = match super::roles::holder(db, "PM").await? {
        Some(id) => {
            sqlx::query_scalar("SELECT name FROM users WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let tasks = granary_core::settings_store::require_json(
        db,
        "nary.handoff.kickoff_tasks",
        super::handoff::WHERE,
    )
    .await?;
    let n_tasks = tasks.as_array().map(Vec::len).unwrap_or(0);
    let summary = format!(
        "Start the project for {}: PM {}, {n_tasks} kickoff task(s){}",
        deal.title,
        pm.unwrap_or_else(|| "you (no PM in the role table)".into()),
        if package.is_some() {
            ", with the handoff package"
        } else {
            " (no handoff package could be compiled)"
        }
    );
    let mut out = crate::ai::tools::catalog::_autonomy_card(
        db,
        user,
        KICKOFF,
        json!({"deal_id": deal.id.to_string()}),
        summary,
        None,
    )
    .await?;
    if let Some(p) = package {
        out["handoff_package"] = p.content;
    }
    Ok(out)
}

// ─── find_meeting_slots ───────────────────────────────────────────────────────

/// `find_meeting_slots`: free slots (read only). Nary's meeting rules (working days, start window, blocks, buffer,
/// minimum notice, meetings per day) against the company calendar and the attendees' calendars.
pub async fn _h_find_meeting_slots(
    db: &mut PgConnection,
    _user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let max_days = require_int(db, "nary.meetings.find_max_days", meeting_files::WHERE).await?;
    let max_slots = require_int(db, "nary.meetings.find_max_slots", meeting_files::WHERE).await?;
    let mut rules = meetings::rules(db).await?;
    let int_arg = |k: &str| -> Result<Option<i64>, String> {
        match args.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => slots::py_int(v)
                .map(Some)
                .map_err(|_| format!("{k} must be a whole number.")),
        }
    };
    let duration = match int_arg("duration_minutes") {
        Ok(d) => d.unwrap_or(slots::py_int(&rules["duration_minutes"])?),
        Err(e) => return err(e),
    };
    if duration <= 0 || duration > 24 * 60 {
        return err("duration_minutes must be between 1 and 1440.");
    }
    let days = match int_arg("within_days") {
        Ok(d) => d.unwrap_or(slots::py_int(&rules["horizon_days"])?),
        Err(e) => return err(e),
    };
    if days < 0 || days > max_days {
        return err(format!(
            "within_days must be between 0 and {max_days} (nary.meetings.find_max_days)."
        ));
    }
    // attendees: ERP users by id, their (Google Workspace) calendars are read too
    let mut emails: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    if let Some(list) = args.get("attendee_user_ids").filter(|v| !v.is_null()) {
        let Some(items) = list.as_array() else {
            return err("attendee_user_ids must be a list of user ids.");
        };
        for it in items {
            let raw = it.as_str().unwrap_or("").trim().to_string();
            let Ok(id) = Uuid::parse_str(&raw) else {
                return err(format!("attendee_user_ids: {raw} is not a valid user id."));
            };
            let row: Option<(String, String)> =
                sqlx::query_as("SELECT name, email FROM users WHERE id = $1 AND is_active = true")
                    .bind(id)
                    .fetch_optional(&mut *db)
                    .await?;
            let Some((name, email)) = row else {
                return err(format!("attendee_user_ids: no active user {raw}."));
            };
            names.push(name);
            emails.push(email);
        }
    }
    let tz = local_tz(db).await?;
    let company_tz = tz.name().to_string();
    let country = text(&args, "country");
    let cust_tz = meetings::customer_tz(country.as_deref(), &company_tz);
    let now = Utc::now();
    let mut warning: Option<String> = None;
    let busy = match crate::google_calendar::free_busy_with(
        db,
        now,
        now + Duration::days(days + 1),
        &emails,
    )
    .await
    {
        Ok((busy, unread)) => {
            if !unread.is_empty() {
                warning = Some(format!(
                    "These calendars could not be read (not shared with the company's Google account): {}. \
                     Check them before booking.",
                    unread.join(", ")
                ));
            }
            busy
        }
        Err(e) if super::governance::is_http_exception(&e) => {
            warning = Some(format!(
                "Google Calendar wasn't read ({}); only meetings booked in the ERP are counted. Check the calendars \
                 before booking.",
                super::governance::http_detail(&e).unwrap_or_default()
            ));
            Vec::new()
        }
        Err(e) => return Err(e),
    };
    let mut booked: std::collections::HashMap<NaiveDate, i64> = std::collections::HashMap::new();
    let mut busy = busy;
    let rows: Vec<(DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT starts_at, ends_at FROM nary_meetings WHERE status = 'booked' AND starts_at >= $1",
    )
    .bind(now)
    .fetch_all(&mut *db)
    .await?;
    for (s, e) in rows {
        *booked.entry(s.with_timezone(&tz).date_naive()).or_insert(0) += 1;
        // a meeting booked in the ERP is busy even when the calendar wasn't read
        let end = e.unwrap_or(s + Duration::minutes(duration));
        busy.push((s.fixed_offset(), end.fixed_offset()));
    }
    let wanted = (max_slots * 4).max(1);
    if let Some(o) = rules.as_object_mut() {
        o.insert("duration_minutes".into(), json!(duration));
        o.insert("horizon_days".into(), json!(days));
        o.insert("slots_offered".into(), json!(wanted));
    }
    let holidays =
        granary_core::settings_store::require_json(db, "nary.meetings.holidays", meetings::WHERE)
            .await?;
    let reduced_hours = granary_core::settings_store::require_json(
        db,
        "nary.meetings.reduced_hours_periods",
        meetings::WHERE,
    )
    .await?;
    let customer_hours = granary_core::settings_store::require_json(
        db,
        "nary.meetings.customer_hours",
        meetings::WHERE,
    )
    .await?;
    let found = slots::find(
        &rules,
        &holidays,
        &reduced_hours,
        &customer_hours,
        &company_tz,
        Some(&cust_tz),
        country.as_deref(),
        &busy,
        now,
        Some(&booked),
    )?;
    // the preference: morning / afternoon (company time), or a day (YYYY-MM-DD) first
    let prefer = text(&args, "prefer").map(|p| p.to_lowercase());
    let local_hour = |s: &slots::Slot| s.start.with_timezone(&tz).format("%H").to_string();
    let mut chosen: Vec<slots::Slot> = found.clone();
    let mut note_pref: Option<String> = None;
    match prefer.as_deref() {
        Some("morning") | Some("afternoon") => {
            let morning = prefer.as_deref() == Some("morning");
            let kept: Vec<slots::Slot> = found
                .iter()
                .filter(|s| (local_hour(s).as_str() < "12") == morning)
                .copied()
                .collect();
            if kept.is_empty() {
                note_pref = Some(format!(
                    "No {} slot fits; these are the free slots at other times.",
                    prefer.as_deref().unwrap_or("")
                ));
            } else {
                chosen = kept;
            }
        }
        Some(p) => match NaiveDate::parse_from_str(p, "%Y-%m-%d") {
            Ok(day) => {
                chosen.sort_by_key(|s| (s.start.with_timezone(&tz).date_naive() != day, s.start));
                if !chosen
                    .first()
                    .is_some_and(|s| s.start.with_timezone(&tz).date_naive() == day)
                {
                    note_pref = Some(format!(
                        "Nothing is free on {day}; these are the nearest free slots."
                    ));
                }
            }
            Err(_) => {
                return err("prefer is morning, afternoon or a day (YYYY-MM-DD).");
            }
        },
        None => {}
    }
    chosen.truncate(max_slots.max(0) as usize);
    let list = chosen
        .iter()
        .map(|s| s.as_dict(&company_tz, Some(&cust_tz)))
        .collect::<ApiResult<Vec<Value>>>()?;
    let mut out = Map::new();
    out.insert("slots".into(), json!(list));
    out.insert("duration_minutes".into(), json!(duration));
    out.insert("within_days".into(), json!(days));
    out.insert("company_time_zone".into(), json!(company_tz));
    if country.is_some() {
        out.insert("customer_time_zone".into(), json!(cust_tz));
    }
    if !names.is_empty() {
        out.insert("attendees".into(), json!(names));
    }
    if let Some(w) = warning {
        out.insert("warning".into(), json!(w));
    }
    if let Some(n) = note_pref {
        out.insert("preference".into(), json!(n));
    }
    out.insert(
        "note".into(),
        json!(if list.is_empty() {
            "No free slot fits Nary's meeting rules in that window: try more days or a shorter meeting."
        } else {
            "Nothing is booked. Offer these times, or book one with propose_meeting (start = the slot's start)."
        }),
    );
    Ok(Value::Object(out))
}

// ─── transcribe_meeting_file ──────────────────────────────────────────────────

/// `transcribe_meeting_file`: a meeting's recording or notes file transcribed and debriefed now (meeting_id: its
/// newest file not read yet; or file_id). Long work: the answer may be still_working with a job_id.
pub async fn _h_transcribe_meeting_file(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !holds(user, "nary:approve") {
        return err(
            "Only people who may give Nary a meeting transcript (nary:approve) can do this.",
        );
    }
    if let Some(job) = text(&args, "job_id") {
        return long_tools::collect(db, user, "transcribe_meeting_file", &job).await;
    }
    let meeting_id = match text(&args, "meeting_id") {
        Some(raw) => match Uuid::parse_str(&raw) {
            Ok(id) => Some(id),
            Err(_) => return err("meeting_id is not a valid id."),
        },
        None => None,
    };
    let file = match text(&args, "file_id") {
        Some(raw) => {
            let Ok(id) = Uuid::parse_str(&raw) else {
                return err("file_id is not a valid id.");
            };
            match meeting_files::load(db, id).await? {
                Some(f) => f,
                None => return err("No meeting file has that file_id."),
            }
        }
        None => {
            let Some(mid) = meeting_id else {
                return err("Give meeting_id (Nary > Meetings, list_meetings) or file_id.");
            };
            let files = meeting_files::list(db, Some(mid), 50).await?;
            if files.is_empty() {
                return err(
                    "No recording or notes file is attached to this meeting: upload one on Nary > Meetings or \
                     attach it in the chat (attach_meeting_notes).",
                );
            }
            match files.iter().find(|f| f.status != "done") {
                Some(f) => f.clone(),
                None => {
                    return Ok(
                        json!({"note": "Every file of this meeting was already read; the results are on \
                                              the deal.", "file": meeting_files::view(&files[0]),
                                     "result": files[0].result.clone()}),
                    )
                }
            }
        }
    };
    let is_approver =
        user.is_superadmin || super::roles::approver_ids(db).await?.contains(&user.id);
    if file.requested_by_id != Some(user.id) && !is_approver {
        return err("That file was sent by someone else.");
    }
    if file.status == "done" {
        return Ok(
            json!({"note": "This file was already read; its results are on the deal.",
                         "file": meeting_files::view(&file), "result": file.result.clone()}),
        );
    }
    // a chat file without its meeting, or a failed one tried again: (re)queued for its meeting
    if file.status == "stored" || file.status == "failed" {
        let target = meeting_id.or(file.meeting_id);
        let meeting = match meeting_files::resolve_meeting(db, target, None).await? {
            Ok(m) => m,
            Err(why) => return Ok(why),
        };
        meeting_files::attach(db, &file, &meeting).await?;
        super::commit(db).await?;
    }
    let mut work_args = json!({"file_id": file.id.to_string()});
    work_args[super::uploads::VIA_ARG] = json!(super::uploads::via_of(&args));
    let mut out = long_tools::run_long(
        db,
        user,
        "transcribe_meeting_file",
        work_args,
        crate::ai_tool_handler!(transcribe_now),
    )
    .await?;
    still_working_note(
        &mut out,
        "transcribe_meeting_file",
        "transcribing and reading the meeting file; a long recording can take many minutes",
    );
    Ok(out)
}

/// The file stepped through to its end, as the request of the person (the debrief's cards are raised while
/// Nary is paused).
async fn transcribe_now(db: &mut PgConnection, _user: &AuthUser, args: Value) -> ApiResult<Value> {
    let id = to_uuid(&args["file_id"])?;
    let origin = origin_of(&args);
    let mut out = super::executor::on_request(origin, async {
        meeting_files::process_on_request(db, id).await
    })
    .await?;
    let note = match out["status"].as_str() {
        Some("done") => "Read: the results note and follow-ups are on the deal; a stage or value change and the \
                         summary email wait as cards for approval.",
        Some("failed") => "It could not be read: the reason is in error (the Owner was told too).",
        _ => "Not finished.",
    };
    out["note"] = json!(note);
    Ok(out)
}
