//! Port of backend/app/services/nary/playbook_actions.py: the Phase 3 actions (sales playbooks). Each goes
//! through the executor, so shadow mode, the autonomy levels (migration 085: money never past asking) and the
//! approver's own permissions apply.
//!
//! ```text
//!     price.answer.send       a list-price answer (WhatsApp or email)
//!     cadence.followup.send   a cadence follow-up
//!     lost.reply.send         a gracious close after a loss
//!     quotation.send          a quotation through document delivery (email, or WhatsApp-only customers)
//!     meeting.slots.send      the Owner-approved meeting times to the customer
//!     meeting.book            book the customer's pick (Google Calendar + Meet); automatic
//!     meeting.reminder.send   the day-before / hour-before reminder; automatic
//!     meeting.add_guests      add guests to a booked meeting (Google sends them the invitation)
//!     meeting.cancel          cancel a booked meeting (Google tells the invitees); kept, marked cancelled
//!     summary.email.send      the summary email after a call
//!     contract.create_draft   acceptance → contract draft, schedule, down-payment proforma card
//!     proforma.raise          the down-payment proforma
//!     handoff.kickoff         Won → project with the PM, kickoff tasks
//!     partner.route           a lead in a partner's territory → that partner
//! ```
//!
//! Python registers them on import; here [`actions`] returns them in that order (the registry collects it).

use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::finance::PaymentSchedule;
use granary_db::rows::nary::{NaryInsight, NaryMeeting};
use granary_db::rows::sales::Quotation;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::{
    apply_fn, apply_send, entity_fn, need_actor, opt_str, opt_uuid, py_truthy, reply_draft_schema,
    reply_entity, to_uuid, Action, Field, Schema, Ty,
};
use super::channel_actions::{email_text_schema, send_email};
use super::governance::py_str;
use super::{commit, meetings};

/// `p[key]` as a str (a KeyError / TypeError in Python otherwise; the schema has checked it).
fn s<'a>(p: &'a Value, key: &str) -> ApiResult<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{key}'")))
}

fn str_list(p: &Value, key: &str) -> Vec<String> {
    p.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(py_str).collect())
        .unwrap_or_default()
}

/// `str(decimal)` of an optional NUMERIC ("None" when NULL).
fn dec(v: Option<rust_decimal::Decimal>) -> String {
    v.map(|d| d.to_string()).unwrap_or_else(|| "None".into())
}

async fn get_quotation(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Quotation>> {
    Ok(sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

// ─── quotation.send ───────────────────────────────────────────────────────────

fn quotation_send_schema() -> Schema {
    Schema::strict(
        "QuotationSend",
        vec![
            Field::req("quotation_id", Ty::Str),
            Field::req("channel", Ty::Literal(&["email", "whatsapp"])),
            Field::req("to", Ty::list(Ty::Str)),
        ],
    )
}

async fn _apply_quotation_send(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let to = str_list(p, "to");
    super::quotes::send(db, actor, s(p, "quotation_id")?, s(p, "channel")?, &to).await
}

async fn _describe_quotation_send(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let q = get_quotation(db, to_uuid(&p["quotation_id"])?).await?;
    let num = match &q {
        Some(q) => q
            .full_number
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| q.number.clone()),
        None => py_str(&p["quotation_id"]),
    };
    Ok(json!({
        "label": format!("Send quotation {num} by {} to {}", py_str(&p["channel"]), str_list(p, "to").join(", ")),
        "after": {"total": q.as_ref().map(|q| dec(q.total)), "currency": q.as_ref().and_then(|q| q.currency.clone())},
    }))
}

// ─── Meetings ─────────────────────────────────────────────────────────────────

fn meeting_entity(p: &Value) -> ApiResult<(Option<String>, Option<Uuid>)> {
    Ok((
        Some("nary_meeting".into()),
        Some(to_uuid(&p["meeting_id"])?),
    ))
}

fn slots_send_schema() -> Schema {
    Schema::strict(
        "SlotsSend",
        vec![
            Field::req("meeting_id", Ty::Str),
            Field::req("slots", Ty::list(Ty::Dict)),
            Field::req("text", Ty::Str),
        ],
    )
}

async fn _apply_slots(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let slot_list = p["slots"].as_array().cloned().unwrap_or_default();
    meetings::send_slots(db, to_uuid(&p["meeting_id"])?, &slot_list, s(p, "text")?).await
}

async fn _describe_slots(
    _db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let empty = vec![];
    let mut parts = Vec::new();
    for slot in p["slots"].as_array().unwrap_or(&empty) {
        // (s.get("company") or s.get("start", "")) + (" (extended)" if s.get("extended") else "")
        let base = match slot.get("company") {
            Some(c) if py_truthy(c) => c.clone(),
            _ => slot.get("start").cloned().unwrap_or_else(|| json!("")),
        };
        let Value::String(base) = base else {
            return Err(ApiError::internal(anyhow::anyhow!(
                "can only concatenate str (not \"{}\") to str",
                super::actions::py_type_name(&base)
            )));
        };
        let ext = if slot.get("extended").is_some_and(py_truthy) {
            " (extended)"
        } else {
            ""
        };
        parts.push(format!("{base}{ext}"));
    }
    Ok(
        json!({"label": format!("Meeting times: {}", parts.join("; ")), "after": {"text": p["text"]}}),
    )
}

fn meeting_book_schema() -> Schema {
    Schema::strict(
        "MeetingBook",
        vec![
            Field::req("meeting_id", Ty::Str),
            // StrictInt: a bool is not a pick (lax int read True as 1); book() checks the range.
            Field::req("picked", Ty::StrictInt),
        ],
    )
}

async fn _apply_book(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let picked = p["picked"]
        .as_i64()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("picked is not an int")))?;
    meetings::book(db, to_uuid(&p["meeting_id"])?, picked).await
}

fn meeting_reminder_schema() -> Schema {
    Schema::strict(
        "MeetingReminder",
        vec![
            Field::req("meeting_id", Ty::Str),
            Field::req("text", Ty::Str),
        ],
    )
}

async fn _apply_reminder(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    meetings::send_reminder(db, to_uuid(&p["meeting_id"])?, s(p, "text")?).await
}

fn meeting_guests_schema() -> Schema {
    Schema::strict(
        "MeetingGuests",
        vec![
            Field::req("meeting_id", Ty::Str),
            Field::req("emails", Ty::list(Ty::Str)),
            Field::default("save_on_contact", Ty::Bool, Value::Bool(false)),
        ],
    )
}

/// `_meeting(db, p)`: the meeting, or 404 "Meeting not found".
async fn _meeting(db: &mut PgConnection, p: &Value) -> ApiResult<NaryMeeting> {
    let id = to_uuid(&p["meeting_id"])?;
    let m: Option<NaryMeeting> = sqlx::query_as("SELECT * FROM nary_meetings WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    m.ok_or_else(|| ApiError::http(404, "Meeting not found"))
}

fn title_or(m: &NaryMeeting, default: &str) -> String {
    m.title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| default.to_string())
}

async fn _apply_guests(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let m = _meeting(db, p).await?;
    let event_id = match m.event_id.clone().filter(|e| !e.is_empty()) {
        Some(e) if m.status == "booked" => e,
        _ => {
            return Err(ApiError::http(
                409,
                format!(
                    "Guests can only join a booked meeting (this one is {})",
                    m.status
                ),
            ))
        }
    };
    // [e.strip().lower() for e in p["emails"] if e and "@" in e]
    let emails: Vec<String> = str_list(p, "emails")
        .iter()
        .filter(|e| !e.is_empty() && e.contains('@'))
        .map(|e| pystr::lower(pystr::strip(e)))
        .collect();
    if emails.is_empty() {
        return Err(ApiError::http(400, "Give at least one email address"));
    }
    let invited = crate::google_calendar::add_attendees(db, &event_id, &emails).await?;
    // list(dict.fromkeys([*(m.attendees or []), *emails]))
    let mut attendees: Vec<Value> = Vec::new();
    let have = m.attendees.as_array().cloned().unwrap_or_default();
    for v in have.into_iter().chain(emails.iter().map(|e| json!(e))) {
        if !attendees.contains(&v) {
            attendees.push(v);
        }
    }
    let attendee_email = m
        .attendee_email
        .clone()
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| emails[0].clone());
    sqlx::query("UPDATE nary_meetings SET attendees = $2, attendee_email = $3, updated_at = now() WHERE id = $1")
        .bind(m.id)
        .bind(Value::Array(attendees))
        .bind(&attendee_email)
        .execute(&mut *db)
        .await?;
    let acts: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, content FROM activities WHERE activity_type = 'meeting' AND (channel_data ->> 'event_id') = $1",
    )
    .bind(&event_id)
    .fetch_all(&mut *db)
    .await?;
    for (id, content) in acts {
        let new = pystr::strip(&format!(
            "{}\nAdded to the invitation: {}.",
            content.unwrap_or_default(),
            emails.join(", ")
        ))
        .to_string();
        sqlx::query("UPDATE activities SET content = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(new)
            .execute(&mut *db)
            .await?;
    }
    if p.get("save_on_contact").is_some_and(py_truthy) {
        if let Some(cid) = m.contact_id {
            // a contact without an email gets the first guest's
            sqlx::query(
                "UPDATE contacts SET email = $2, updated_at = now() WHERE id = $1 AND (email IS NULL OR email = '')",
            )
            .bind(cid)
            .bind(&emails[0])
            .execute(&mut *db)
            .await?;
        }
    }
    commit(db).await?;
    Ok(json!({
        "id": m.id.to_string(),
        "label": format!("Invited {} to {}", emails.join(", "), title_or(&m, "the meeting")),
        "invited": invited,
    }))
}

async fn _describe_guests(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let m = _meeting(db, p).await?;
    Ok(json!({
        "label": format!("Invite {} to {}", str_list(p, "emails").join(", "), title_or(&m, "the meeting")),
        "after": {"emails": p["emails"]},
    }))
}

fn meeting_cancel_schema() -> Schema {
    Schema::strict(
        "MeetingCancel",
        vec![
            Field::req("meeting_id", Ty::Str),
            Field::opt("reason", Ty::Str),
        ],
    )
}

async fn _apply_cancel(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let mut m = _meeting(db, p).await?;
    let out = meetings::cancel(db, &mut m, opt_str(p, "reason"), actor, true).await?;
    commit(db).await?;
    Ok(out)
}

async fn _describe_cancel(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let m = _meeting(db, p).await?;
    let reason = match p.get("reason") {
        Some(r) if py_truthy(r) => format!(" ({})", py_str(r)),
        _ => String::new(),
    };
    Ok(
        json!({"label": format!("Cancel {}{reason}", title_or(&m, "the meeting")), "after": {"status": "cancelled"}}),
    )
}

// ─── summary.email.send ───────────────────────────────────────────────────────

async fn _apply_summary(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let out = send_email(
        db,
        s(p, "to")?,
        s(p, "subject")?,
        s(p, "text")?,
        opt_uuid(p, "contact_id")?,
        opt_str(p, "lead_id"),
        None,
        actor,
        None,
        None,
    )
    .await?;
    let mut m: Map<String, Value> = match out {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    m.insert(
        "label".into(),
        json!(format!("Summary email to {}", py_str(&p["to"]))),
    );
    Ok(Value::Object(m))
}

async fn _describe_email(
    _db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    Ok(json!({
        "label": format!("Email to {}: {}", py_str(&p["to"]), py_str(&p["subject"])),
        "after": {"subject": p["subject"], "text": p["text"]},
    }))
}

// ─── Deals, handoff, partners ─────────────────────────────────────────────────

async fn _apply_contract(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    super::deals::create_contract_draft(db, need_actor(actor)?, s(p, "quotation_id")?).await
}

async fn _describe_contract(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let q = get_quotation(db, to_uuid(&p["quotation_id"])?).await?;
    let label = match &q {
        Some(q) => format!(
            "Contract draft from quotation {} ({} {})",
            q.full_number
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| q.number.clone()),
            q.currency.clone().unwrap_or_else(|| "None".into()),
            dec(q.total)
        ),
        None => "Contract draft from quotation ?".to_string(),
    };
    Ok(json!({"label": label}))
}

async fn _apply_proforma(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    super::deals::raise_proforma(db, need_actor(actor)?, s(p, "schedule_id")?).await
}

async fn _describe_proforma(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let sched: Option<PaymentSchedule> =
        sqlx::query_as("SELECT * FROM payment_schedules WHERE id = $1")
            .bind(to_uuid(&p["schedule_id"])?)
            .fetch_optional(&mut *db)
            .await?;
    let label = match sched {
        Some(sc) => format!(
            "Proforma for '{}' ({}% {})",
            sc.name,
            sc.percentage
                .filter(|d| !d.is_zero())
                .map(|d| d.to_string())
                .unwrap_or_default(),
            sc.currency.unwrap_or_else(|| "None".into())
        ),
        None => "Proforma".to_string(),
    };
    Ok(json!({"label": label}))
}

async fn _apply_kickoff(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    super::handoff::kickoff(db, need_actor(actor)?.id, s(p, "deal_id")?).await
}

async fn _describe_kickoff(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let deal_id = to_uuid(&p["deal_id"])?;
    let deal: Option<(String, Option<Uuid>, Option<Uuid>)> =
        sqlx::query_as("SELECT title, organization_id, contact_id FROM deals WHERE id = $1")
            .bind(deal_id)
            .fetch_optional(&mut *db)
            .await?;
    // scalar_one_or_none(): more than one handoff package is MultipleResultsFound
    let pkgs: Vec<NaryInsight> = sqlx::query_as(
        "SELECT * FROM nary_insights WHERE deal_id = $1 AND kind = 'handoff' LIMIT 2",
    )
    .bind(deal_id)
    .fetch_all(&mut *db)
    .await?;
    if pkgs.len() > 1 {
        return Err(ApiError::internal(super::governance::PyException::new(
            "MultipleResultsFound",
            "Multiple rows were found when one or none was required",
        )));
    }
    let title = deal.as_ref().map(|d| d.0.clone());
    // 21 Nov 2026: what the kickoff makes, readable (it showed the raw package with its nulls)
    let mut after = Map::new();
    if let Some(t) = &title {
        after.insert("project".into(), json!(t));
    }
    if let Some(name) = kickoff_customer(
        db,
        deal.as_ref().and_then(|d| d.1),
        deal.as_ref().and_then(|d| d.2),
    )
    .await?
    {
        after.insert("customer".into(), json!(name));
    }
    let pm: Option<String> = match super::roles::holder(db, "PM").await? {
        Some(id) => {
            sqlx::query_scalar("SELECT name FROM users WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    after.insert(
        "pm".into(),
        json!(pm
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Whoever applies this card (no PM in the role table)".into())),
    );
    // read softly: a missing setting fails the apply with its own message, not the card
    let tasks: Vec<String> =
        granary_core::settings_store::get_optional(db, "nary.handoff.kickoff_tasks")
            .await?
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                t.as_str()
                    .map(str::to_string)
                    .filter(|t| !t.trim().is_empty())
            })
            .collect();
    if !tasks.is_empty() {
        after.insert("kickoff_tasks".into(), json!(tasks));
    }
    if let Some(pkg) = pkgs.first() {
        if let Some(sum) = pkg
            .content
            .get("summary")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            after.insert("handoff_summary".into(), json!(sum));
        }
        let mut sections = Map::new();
        for k in super::handoff::SECTIONS {
            let items = handoff_lines(pkg.content.get(k));
            if !items.is_empty() {
                sections.insert(k.to_string(), json!(items));
            }
        }
        if !sections.is_empty() {
            after.insert("handoff".into(), Value::Object(sections));
        }
    }
    Ok(json!({
        "label": format!("Start the project for {}", title.unwrap_or_else(|| "?".into())),
        "after": after,
    }))
}

/// The deal's customer: its organization, else its contact.
async fn kickoff_customer(
    db: &mut PgConnection,
    org: Option<Uuid>,
    contact: Option<Uuid>,
) -> ApiResult<Option<String>> {
    if let Some(id) = org {
        let name: Option<String> =
            sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
            return Ok(Some(n));
        }
    }
    if let Some(id) = contact {
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT first_name, last_name FROM contacts WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((f, l)) = row {
            let full = format!("{f} {}", l.unwrap_or_default()).trim().to_string();
            if !full.is_empty() {
                return Ok(Some(full));
            }
        }
    }
    Ok(None)
}

/// One handoff section as lines: strings as they are, a success criterion as "quote (source)"; empty and null
/// entries left out.
fn handoff_lines(v: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = v else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|it| match it {
            Value::String(s) => Some(s.trim().to_string()),
            Value::Object(m) => {
                let quote = m.get("quote").and_then(Value::as_str).unwrap_or("").trim();
                let source = m.get("source").and_then(Value::as_str).unwrap_or("").trim();
                match (quote.is_empty(), source.is_empty()) {
                    (true, _) => None,
                    (false, true) => Some(quote.to_string()),
                    (false, false) => Some(format!("{quote} ({source})")),
                }
            }
            _ => None,
        })
        .filter(|s| !s.is_empty())
        .collect()
}

async fn _apply_partner(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    super::handoff::route_to_partner(
        db,
        need_actor(actor)?.id,
        s(p, "lead_id")?,
        s(p, "partner_account_id")?,
    )
    .await
}

/// The actions playbook_actions.py registers on import, in its order.
pub fn actions() -> Vec<Action> {
    let mut out = Vec::new();
    for (name, label) in [
        ("price.answer.send", "Answer a price question"),
        ("cadence.followup.send", "Send a quotation follow-up"),
        ("lost.reply.send", "Close a lost deal graciously"),
    ] {
        out.push(
            Action::new(name, label, reply_draft_schema(), apply_send())
                .permission("crm:log_activity")
                .customer_facing(true)
                .entity(reply_entity()),
        );
    }
    let meeting = || entity_fn(meeting_entity);
    out.extend([
        Action::new(
            "quotation.send",
            "Send a quotation",
            quotation_send_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_quotation_send(db, a, p))),
        )
        .permission("sales:send_quotation")
        .customer_facing(true)
        .describe(apply_fn(|db, a, p| {
            Box::pin(_describe_quotation_send(db, a, p))
        }))
        .entity(entity_fn(|p| {
            Ok((Some("quotation".into()), Some(to_uuid(&p["quotation_id"])?)))
        })),
        Action::new(
            "meeting.slots.send",
            "Offer meeting times",
            slots_send_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_slots(db, a, p))),
        )
        .permission("crm:send_whatsapp")
        .customer_facing(true)
        .describe(apply_fn(|db, a, p| Box::pin(_describe_slots(db, a, p))))
        .entity(meeting()),
        Action::new(
            "meeting.book",
            "Book the customer's chosen meeting time",
            meeting_book_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_book(db, a, p))),
        )
        .permission("crm:log_activity")
        .customer_facing(true)
        .entity(meeting()),
        Action::new(
            "meeting.reminder.send",
            "Remind the customer of a meeting",
            meeting_reminder_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_reminder(db, a, p))),
        )
        .permission("crm:log_activity")
        .customer_facing(true)
        .entity(meeting()),
        Action::new(
            "meeting.add_guests",
            "Add guests to a booked meeting",
            meeting_guests_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_guests(db, a, p))),
        )
        .permission("crm:log_activity")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_guests(db, a, p))))
        .entity(meeting()),
        Action::new(
            "meeting.cancel",
            "Cancel a booked meeting",
            meeting_cancel_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_cancel(db, a, p))),
        )
        .permission("crm:log_activity")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_cancel(db, a, p))))
        .entity(meeting()),
        Action::new(
            "summary.email.send",
            "Send the summary email after a call",
            email_text_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_summary(db, a, p))),
        )
        .permission("crm:log_activity")
        .customer_facing(true)
        .describe(apply_fn(|db, a, p| Box::pin(_describe_email(db, a, p))))
        .entity(entity_fn(|p| match opt_uuid(p, "contact_id")? {
            Some(id) => Ok((Some("contact".into()), Some(id))),
            None => Ok((None, None)),
        })),
        Action::new(
            "contract.create_draft",
            "Draft the contract from an accepted quotation",
            Schema::strict("QuotationRef", vec![Field::req("quotation_id", Ty::Str)]),
            apply_fn(|db, a, p| Box::pin(_apply_contract(db, a, p))),
        )
        .permission("sales:create_contract")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_contract(db, a, p))))
        .entity(entity_fn(|p| {
            Ok((Some("quotation".into()), Some(to_uuid(&p["quotation_id"])?)))
        })),
        Action::new(
            "proforma.raise",
            "Raise the down-payment proforma",
            Schema::strict("ScheduleRef", vec![Field::req("schedule_id", Ty::Str)]),
            apply_fn(|db, a, p| Box::pin(_apply_proforma(db, a, p))),
        )
        .permission("finance:create_invoice")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_proforma(db, a, p)))),
        Action::new(
            "handoff.kickoff",
            "Start the project from the handoff package",
            Schema::strict("DealRef", vec![Field::req("deal_id", Ty::Str)]),
            apply_fn(|db, a, p| Box::pin(_apply_kickoff(db, a, p))),
        )
        .permission("projects:create")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_kickoff(db, a, p))))
        .entity(entity_fn(|p| {
            Ok((Some("deal".into()), Some(to_uuid(&p["deal_id"])?)))
        })),
        Action::new(
            "partner.route",
            "Route a lead to a partner",
            Schema::strict(
                "PartnerRoute",
                vec![
                    Field::req("lead_id", Ty::Str),
                    Field::req("partner_account_id", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_partner(db, a, p))),
        )
        .permission("partner_mgmt:manage")
        .entity(entity_fn(|p| {
            Ok((Some("lead".into()), Some(to_uuid(&p["lead_id"])?)))
        })),
    ]);
    out
}
