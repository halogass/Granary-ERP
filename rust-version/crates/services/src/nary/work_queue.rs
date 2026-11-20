//! 3 Nov 2026 (Owner decisions, Stream A, migration 0154): the work queue. Internal Nary stays paused; your AI client
//! (MCP) works the business by hand, and this module says what is waiting and which tool to use next.
//!
//! ```text
//!     compute(db, now)     every waiting item, urgent first, then today, then the rest (oldest first)
//!     list(db, filter)     the same filtered by kinds / urgency / limit, as the MCP tool, the chat tool and
//!                          Nary > Work queue show it
//!     nudge_tick(db)       the worker beat (every minute): a WhatsApp nudge "N items waiting" to the Owner at
//!                          nary.queue.nudge_times on working days, and at once for new urgent items within
//!                          working hours, at most once per nary.queue.urgent_gap_minutes. It runs while
//!                          nary.paused is true (that is the point).
//! ```
//!
//! Item kinds (each reuses the computation its own module already has):
//! - whatsapp_reply / unknown_chat: chats where the customer had the last word (reply_guard::awaiting_us, the
//!   daily unanswered check's query); a chat with no lead, contact or deal is an unknown number.
//! - email_reply: CRM inbox threads whose newest message is the customer's.
//! - new_lead: leads in new or triaging, within nary.queue.new_lead_days.
//! - held_chat: chats held by the privacy gate (business or personal?).
//! - followup_due: open follow-up reminders that are due (every owner).
//! - quotation_expiring / quotation_expired: sent quotations expiring within nary.queue.quotation_expiry_days,
//!   and expired ones within sales.quotation.expired_nudge_days.
//! - accepted_no_contract / contract_no_proforma / ready_to_win: the close-out (nary::deals: a signed contract
//!   or a paid down payment on a deal that is not Won).
//! - meeting_reminder / meeting_brief: booked meetings within nary.queue.meeting_reminder_hours with no reminder
//!   sent, and within nary.queue.meeting_brief_hours with no brief.
//! - meeting_file: meeting recordings and notes files waiting to be read.
//! - duplicate_lead / duplicate_deal: crm::lead_merge and crm::deal_duplicates pairs not decided yet.
//! - card_pending / card_expiring: pending cards; those expiring within nary.queue.card_expiry_hours (of
//!   nary.pending.expire_days) are card_expiring.
//! - approval_overdue: approval requests past their step's SLA.
//!
//! An item is gone once its work is done: the customer answered (or a card to answer waits), the lead moved on,
//! the reminder is done, the quotation revised or answered, the card decided, and so on.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeDelta, Utc};
use chrono_tz::Tz;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, require_int, require_json, SettingError};
use granary_db::rows::approval::ApprovalRequest;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::notify::{notify, Notice};
use crate::crm::followups::link_for;
use crate::nary::{commit, owner_channel, py_slice, rhythm, roles};

pub const WHERE: &str = "Nary & AI → Work queue";
/// The ERP page.
pub const PAGE: &str = "/nary/queue";

/// Every kind, in the order the page lists its filter.
pub const KINDS: &[&str] = &[
    "whatsapp_reply",
    "email_reply",
    "new_lead",
    "unknown_chat",
    "held_chat",
    "followup_due",
    "quotation_expiring",
    "quotation_expired",
    "accepted_no_contract",
    "contract_no_proforma",
    "ready_to_win",
    "meeting_reminder",
    "meeting_brief",
    "meeting_file",
    "duplicate_lead",
    "duplicate_deal",
    "card_pending",
    "card_expiring",
    "approval_overdue",
];
pub const URGENCIES: [&str; 3] = ["urgent", "today", "normal"];

/// The prompt the Owner pastes into a scheduled task in their AI client (the ERP guide quotes it word for word).
pub const SCHEDULED_PROMPT: &str = "Connect to Granary. Call nary_work_queue. Work the urgent items first, then \
today's, oldest first. For each item read what it points to with its suggested tool and arguments, then do the next \
step. Every message to a customer is a card for my approval: draft it with propose_customer_message and never send \
anything directly. Never say something was sent, booked or saved until a tool result says applied. Leave meeting \
recordings alone unless I ask. When you are done, give me a short list: the cards you drafted (with their card ids), \
what needs my decision, and what you could not do and why.";

/// State markers (system settings of category nary_state).
const LAST_NUDGE_AT: &str = "nary.queue.last_nudge_at";
const URGENT_SEEN_AT: &str = "nary.queue.urgent_seen_at";
const SLOT_MARK: &str = "nary.queue.last_slot";

/// One waiting piece of work.
#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    /// Who or what, with the record ids.
    pub subject: Value,
    pub since: DateTime<Utc>,
    pub urgency: &'static str,
    pub suggested_tool: Option<&'static str>,
    pub suggested_args: Value,
    pub hint: String,
    pub link: String,
}

impl Item {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "kind": self.kind, "title": self.title, "subject": self.subject,
            "since": self.since.to_rfc3339(), "urgency": self.urgency,
            "suggested_tool": self.suggested_tool, "suggested_args": self.suggested_args,
            "hint": self.hint, "link": self.link,
        })
    }
}

/// A subject object with only the given (non-null) fields.
fn subject(pairs: &[(&str, Value)]) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        if !v.is_null() && v != &json!("") {
            m.insert((*k).to_string(), v.clone());
        }
    }
    Value::Object(m)
}

fn id_v(id: Option<Uuid>) -> Value {
    id.map(|u| json!(u.to_string())).unwrap_or(Value::Null)
}

fn s_v(s: Option<String>) -> Value {
    s.filter(|t| !t.trim().is_empty())
        .map(Value::String)
        .unwrap_or(Value::Null)
}

fn first_text(opts: &[Option<String>], fallback: &str) -> String {
    opts.iter()
        .flatten()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn clip(s: &str, n: usize) -> String {
    let t = s.trim().replace('\n', " ");
    if t.chars().count() > n {
        format!("{}...", py_slice(&t, n))
    } else {
        t
    }
}

// ─── Settings ──────────────────────────────────────────────────────────────────

async fn int(db: &mut PgConnection, key: &str) -> ApiResult<i64> {
    Ok(require_int(db, key, WHERE).await?)
}

async fn string_list(db: &mut PgConnection, key: &str) -> ApiResult<Vec<String>> {
    match require_json(db, key, WHERE).await? {
        Value::Array(a) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| SettingError::new(key, "must be a list of texts", WHERE).into())
            })
            .collect(),
        _ => Err(SettingError::new(key, "must be a list of texts", WHERE).into()),
    }
}

async fn flag(db: &mut PgConnection, key: &str) -> ApiResult<bool> {
    match get_optional(db, key).await? {
        Some(v) => Ok(v.eq_ignore_ascii_case("true")),
        None => Err(SettingError::new(key, "is missing", WHERE).into()),
    }
}

/// nary.queue.nudge_times as times of day.
pub async fn nudge_times(db: &mut PgConnection) -> ApiResult<Vec<NaiveTime>> {
    let key = "nary.queue.nudge_times";
    let mut out = vec![];
    for t in string_list(db, key).await? {
        let at = NaiveTime::parse_from_str(t.trim(), "%H:%M").map_err(|_| {
            ApiError::from(SettingError::new(
                key,
                &format!("must be times as HH:MM ('{t}')"),
                WHERE,
            ))
        })?;
        out.push(at);
    }
    out.sort();
    Ok(out)
}

/// A working day: a day of nary.working_hours and not a holiday of nary.meetings.holidays.
pub async fn is_working_day(db: &mut PgConnection, day: NaiveDate) -> ApiResult<bool> {
    let wh = rhythm::working_hours(db).await?;
    if !wh
        .days
        .contains(&(day.weekday().number_from_monday() as i64))
    {
        return Ok(false);
    }
    let holidays = require_json(db, "nary.meetings.holidays", WHERE).await?;
    let iso = day.format("%Y-%m-%d").to_string();
    Ok(!matches!(&holidays, Value::Array(a) if a.iter().any(|d| d.as_str() == Some(iso.as_str()))))
}

// ─── Computing the queue ─────────────────────────────────────────────────────

struct Ctx {
    now: DateTime<Utc>,
    tz: Tz,
    urgent: Vec<String>,
    today: Vec<String>,
}

impl Ctx {
    fn urgency(&self, kind: &str) -> &'static str {
        if self.urgent.iter().any(|k| k == kind) {
            "urgent"
        } else if self.today.iter().any(|k| k == kind) {
            "today"
        } else {
            "normal"
        }
    }
    fn local(&self, t: DateTime<Utc>) -> String {
        t.with_timezone(&self.tz)
            .format("%a %d %b %H:%M")
            .to_string()
    }
    fn today(&self) -> NaiveDate {
        self.now.with_timezone(&self.tz).date_naive()
    }
    #[allow(clippy::too_many_arguments)]
    fn item(
        &self,
        id: String,
        kind: &'static str,
        title: String,
        subject: Value,
        since: DateTime<Utc>,
        tool: Option<&'static str>,
        args: Value,
        hint: &str,
        link: String,
    ) -> Item {
        Item {
            id,
            kind,
            title,
            subject,
            since,
            urgency: self.urgency(kind),
            suggested_tool: tool,
            suggested_args: args,
            hint: hint.to_string(),
            link,
        }
    }
}

/// Every waiting item at `now`, urgent first, then today, then the rest; oldest first within each.
pub async fn compute(db: &mut PgConnection, now: DateTime<Utc>) -> ApiResult<Vec<Item>> {
    let cx = Ctx {
        now,
        tz: granary_core::clock::local_tz(db).await?,
        urgent: string_list(db, "nary.queue.urgent_kinds").await?,
        today: string_list(db, "nary.queue.today_kinds").await?,
    };
    let mut items: Vec<Item> = vec![];
    chats(db, &cx, &mut items).await?;
    emails(db, &cx, &mut items).await?;
    leads(db, &cx, &mut items).await?;
    held(db, &cx, &mut items).await?;
    followups(db, &cx, &mut items).await?;
    quotations(db, &cx, &mut items).await?;
    close_out(db, &cx, &mut items).await?;
    meetings(db, &cx, &mut items).await?;
    meeting_files(db, &cx, &mut items).await?;
    duplicates(db, &cx, &mut items).await?;
    cards(db, &cx, &mut items).await?;
    approvals(db, &cx, &mut items).await?;
    let rank = |u: &str| URGENCIES.iter().position(|x| *x == u).unwrap_or(3);
    items.sort_by(|a, b| (rank(a.urgency), a.since, &a.id).cmp(&(rank(b.urgency), b.since, &b.id)));
    Ok(items)
}

/// Chats with a card to them still waiting (pending or timed): the answer is drafted, the card is the item.
async fn chats_with_waiting_card(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT a.payload ->> 'chat_id' FROM agent_pending_actions a \
         WHERE a.status IN ('pending', 'scheduled') AND (a.payload ->> 'chat_id') IS NOT NULL",
    )
    .fetch_all(&mut *db)
    .await?)
}

async fn chats(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    let days = int(db, "nary.reply.unanswered_lookback_days").await?;
    let waiting = chats_with_waiting_card(db).await?;
    for c in crate::nary::reply_guard::awaiting_us(db, cx.now - TimeDelta::days(days)).await? {
        if waiting.contains(&c.chat_id) {
            continue;
        }
        let who = first_text(
            &[
                c.contact_name.clone(),
                c.display_name.clone(),
                c.phone.clone(),
            ],
            &c.chat_id,
        );
        let unknown = c.lead_id.is_none() && c.contact_id.is_none() && c.deal_id.is_none();
        let said = clip(&c.text, 160);
        let subj = subject(&[
            ("name", json!(who)),
            ("chat_id", json!(c.chat_id)),
            ("phone", s_v(c.phone.clone())),
            ("contact_id", id_v(c.contact_id)),
            ("lead_id", id_v(c.lead_id)),
            ("deal_id", id_v(c.deal_id)),
            ("last_message", json!(said)),
        ]);
        let link = link_for("whatsapp_chat", None, Some(&c.chat_id));
        if unknown {
            out.push(cx.item(
                format!("unknown_chat:{}", c.chat_id),
                "unknown_chat",
                format!("New WhatsApp number {who} wrote: \"{said}\""),
                subj,
                c.since,
                Some("propose_whatsapp_lead"),
                json!({"chat_id": c.chat_id}),
                "Read the chat (get_whatsapp_messages); a prospect becomes a lead with propose_whatsapp_lead, \
                 then answer with propose_customer_message (a card for the Owner).",
                link,
            ));
        } else {
            out.push(cx.item(
                format!("whatsapp_reply:{}", c.chat_id),
                "whatsapp_reply",
                format!("{who} is waiting for an answer on WhatsApp: \"{said}\""),
                subj,
                c.since,
                Some("get_whatsapp_messages"),
                json!({"chat_id": c.chat_id}),
                "Read the chat, then answer with propose_customer_message (channel whatsapp, this chat_id): a \
                 card for the Owner's approval.",
                link,
            ));
        }
    }
    Ok(())
}

async fn emails(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    let days = int(db, "nary.reply.unanswered_lookback_days").await?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        Uuid,
        Option<String>,
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
        String,
        Option<String>,
        DateTime<Utc>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT t.id, t.subject, t.linked_contact_id, t.linked_lead_id, t.linked_deal_id, m.from_email, m.from_name, \
                COALESCE(m.received_at, m.created_at), m.body_text \
         FROM crm_email_threads t \
         JOIN LATERAL (SELECT * FROM crm_email_messages WHERE thread_id = t.id \
                       ORDER BY COALESCE(received_at, created_at) DESC LIMIT 1) m ON true \
         WHERE m.direction = 'inbound' AND COALESCE(m.received_at, m.created_at) >= $1 \
           AND COALESCE(t.status, 'open') NOT IN ('closed', 'archived', 'spam', 'done', 'trash') \
           AND NOT EXISTS (SELECT 1 FROM agent_pending_actions a WHERE a.status IN ('pending', 'scheduled') \
                           AND (a.payload ->> 'thread_id') = t.id::text)",
    )
    .bind(cx.now - TimeDelta::days(days))
    .fetch_all(&mut *db)
    .await?;
    for (id, subj, contact, lead, deal, from, from_name, at, body) in rows {
        let who = first_text(&[from_name], &from);
        let topic = first_text(&[subj], "(no subject)");
        out.push(cx.item(
            format!("email_reply:{id}"),
            "email_reply",
            format!("{who} is waiting for an answer by email: {}", clip(&topic, 100)),
            subject(&[
                ("name", json!(who)),
                ("email", json!(from)),
                ("thread_id", json!(id.to_string())),
                ("subject", json!(topic)),
                ("contact_id", id_v(contact)),
                ("lead_id", id_v(lead)),
                ("deal_id", id_v(deal)),
                ("last_message", json!(clip(body.as_deref().unwrap_or(""), 160))),
            ]),
            at,
            Some("get_email_thread"),
            json!({"thread_id": id.to_string()}),
            "Read the thread, then answer with propose_customer_message (channel email, this thread_id): a card \
             for the Owner's approval.",
            link_for("email_thread", Some(id), None),
        ));
    }
    Ok(())
}

/// The name a lead goes by: its organization, its contact, or its title.
const LEAD_NAME: &str =
    "COALESCE(NULLIF(o.name, ''), NULLIF(trim(concat_ws(' ', c.first_name, c.last_name)), ''), \
                         NULLIF(l.title, ''), 'Lead')";

async fn leads(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    let days = int(db, "nary.queue.new_lead_days").await?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, String, Option<String>, String, Option<String>, DateTime<Utc>, Option<Uuid>, Option<Uuid>)> =
        sqlx::query_as(&format!(
            "SELECT l.id, {LEAD_NAME}, l.status, l.source, l.title, l.created_at, l.contact_id, l.organization_id \
             FROM leads l LEFT JOIN contacts c ON c.id = l.contact_id \
             LEFT JOIN organizations o ON o.id = l.organization_id \
             WHERE l.is_deleted = false AND COALESCE(l.status, 'new') IN ('new', 'triaging') \
               AND l.created_at >= $1"
        ))
        .bind(cx.now - TimeDelta::days(days))
        .fetch_all(&mut *db)
        .await?;
    for (id, name, status, source, title, at, contact, org) in rows {
        let status = status.unwrap_or_else(|| "new".into());
        out.push(cx.item(
            format!("new_lead:{id}"),
            "new_lead",
            format!(
                "New lead {name} ({status}, from {source}){}",
                title
                    .filter(|t| !t.is_empty() && t != &name)
                    .map(|t| format!(": {}", clip(&t, 80)))
                    .unwrap_or_default()
            ),
            subject(&[
                ("name", json!(name)),
                ("lead_id", json!(id.to_string())),
                ("status", json!(status)),
                ("source", json!(source)),
                ("contact_id", id_v(contact)),
                ("organization_id", id_v(org)),
            ]),
            at,
            Some("score_lead"),
            json!({"lead_id": id.to_string()}),
            "Read it (get_lead), score it (BANT), acknowledge the customer with propose_customer_message (a card) \
             and move it to qualifying or disqualify it.",
            link_for("lead", Some(id), None),
        ));
    }
    Ok(())
}

async fn held(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    let rows: Vec<(String, Option<String>, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT chat_id, display_name, phone, COALESCE(last_inbound_at, updated_at) FROM wa_chats \
         WHERE status = 'held' AND staff_user_id IS NULL",
    )
    .fetch_all(&mut *db)
    .await?;
    for (chat_id, name, phone, at) in rows {
        let who = first_text(&[name, phone.clone()], &chat_id);
        out.push(cx.item(
            format!("held_chat:{chat_id}"),
            "held_chat",
            format!("Held chat {who}: business or personal?"),
            subject(&[("name", json!(who)), ("chat_id", json!(chat_id)), ("phone", s_v(phone))]),
            at,
            Some("propose_held_chat_decision"),
            json!({"chat_id": chat_id}),
            "Ask the Owner whether the chat is business (released to Nary) or personal (never read); then \
             propose_held_chat_decision.",
            "/nary/intake?tab=held".to_string(),
        ));
    }
    Ok(())
}

async fn followups(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        Uuid,
        String,
        String,
        Option<Uuid>,
        Option<String>,
        DateTime<Utc>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT r.id, r.title, r.entity_type, r.entity_id, r.chat_id, r.due_at, u.name \
             FROM crm_reminders r LEFT JOIN users u ON u.id = r.owner_user_id \
             WHERE r.status = 'open' AND r.due_at <= $1",
    )
    .bind(cx.now)
    .fetch_all(&mut *db)
    .await?;
    for (id, title, etype, eid, chat, due, owner) in rows {
        let mut subj = vec![
            ("reminder_id", json!(id.to_string())),
            ("entity_type", json!(etype)),
            ("entity_id", id_v(eid)),
            ("chat_id", s_v(chat.clone())),
            ("owner", s_v(owner.clone())),
            ("due_at", json!(due.to_rfc3339())),
        ];
        subj.insert(0, ("name", json!(title)));
        out.push(cx.item(
            format!("followup_due:{id}"),
            "followup_due",
            format!(
                "Follow-up due {}: {}{}",
                cx.local(due),
                clip(&title, 120),
                owner.map(|o| format!(" ({o})")).unwrap_or_default()
            ),
            subject(&subj),
            due,
            Some("propose_reminder_change"),
            json!({"reminder_id": id.to_string(), "action": "done"}),
            "Do the follow-up (usually propose_customer_message, a card), then mark it done; or snooze it with \
             propose_reminder_change action snooze.",
            link_for(&etype, eid, chat.as_deref()),
        ));
    }
    Ok(())
}

async fn quotations(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    let ahead = int(db, "nary.queue.quotation_expiry_days").await?;
    let back = int(db, "sales.quotation.expired_nudge_days").await?;
    let today = cx.today();
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, String, Option<String>, Option<String>, NaiveDate, Option<Uuid>, Option<String>, DateTime<Utc>)> =
        sqlx::query_as(
            "SELECT q.id, q.number, q.full_number, q.status, q.valid_until, q.deal_id, \
                    COALESCE(NULLIF(o.name, ''), NULLIF(q.subject, '')), q.updated_at \
             FROM quotations q LEFT JOIN organizations o ON o.id = q.customer_org_id \
             LEFT JOIN deals d ON d.id = q.deal_id \
             WHERE q.is_deleted = false \
               AND ((q.status = 'sent' AND q.valid_until <= $1) \
                    OR (q.status = 'expired' AND q.valid_until >= $2)) \
               AND (d.id IS NULL OR NOT (d.stage = ANY($3))) \
               AND NOT EXISTS (SELECT 1 FROM quotations n WHERE n.previous_version_id = q.id AND n.is_deleted = false)",
        )
        .bind(today + TimeDelta::days(ahead))
        .bind(today - TimeDelta::days(back))
        .bind(
            crate::crm::stages::CLOSED_STAGES
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
        .fetch_all(&mut *db)
        .await?;
    for (id, number, full, status, valid, deal, customer, updated) in rows {
        let no = full.filter(|f| !f.is_empty()).unwrap_or(number);
        let who = customer.unwrap_or_else(|| "the customer".into());
        let expired = status.as_deref() == Some("expired") || valid < today;
        let subj = subject(&[
            ("name", json!(who)),
            ("quotation_id", json!(id.to_string())),
            ("number", json!(no)),
            ("valid_until", json!(valid.to_string())),
            ("deal_id", id_v(deal)),
        ]);
        let link = link_for("quotation", Some(id), None);
        if expired {
            out.push(cx.item(
                format!("quotation_expired:{id}"),
                "quotation_expired",
                format!("Quotation {no} for {who} expired on {valid}"),
                subj,
                updated.min(cx.now),
                Some("propose_quotation_revision"),
                json!({"quotation_id": id.to_string()}),
                "Ask the customer where they stand (propose_customer_message, a card) and, if they still want it, \
                 a new revision with propose_quotation_revision.",
                link,
            ));
        } else {
            let days_left = (valid - today).num_days();
            out.push(cx.item(
                format!("quotation_expiring:{id}"),
                "quotation_expiring",
                format!(
                    "Quotation {no} for {who} expires {}",
                    if days_left == 0 { "today".to_string() } else { format!("in {days_left} day(s), {valid}") }
                ),
                subj,
                updated.min(cx.now),
                Some("propose_customer_message"),
                json!({"quotation_id": id.to_string()}),
                "A follow-up before it expires (propose_customer_message, a card), or a new validity with \
                 propose_quotation_revision.",
                link,
            ));
        }
    }
    Ok(())
}

async fn close_out(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    let closed: Vec<String> = crate::crm::stages::CLOSED_STAGES
        .iter()
        .map(|s| s.to_string())
        .collect();
    // Accepted quotations with no contract yet.
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, String, Option<String>, Option<Uuid>, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT q.id, q.number, q.full_number, q.deal_id, COALESCE(NULLIF(o.name, ''), NULLIF(q.subject, '')), \
                q.updated_at \
         FROM quotations q LEFT JOIN organizations o ON o.id = q.customer_org_id \
         LEFT JOIN deals d ON d.id = q.deal_id \
         WHERE q.is_deleted = false AND q.status = 'accepted' AND (d.id IS NULL OR NOT (d.stage = ANY($1))) \
           AND NOT EXISTS (SELECT 1 FROM contracts c WHERE c.is_deleted = false \
                           AND (c.quotation_id = q.id OR (q.deal_id IS NOT NULL AND c.deal_id = q.deal_id)))",
    )
    .bind(&closed)
    .fetch_all(&mut *db)
    .await?;
    for (id, number, full, deal, customer, at) in rows {
        let no = full.filter(|f| !f.is_empty()).unwrap_or(number);
        let who = customer.unwrap_or_else(|| "the customer".into());
        out.push(cx.item(
            format!("accepted_no_contract:{id}"),
            "accepted_no_contract",
            format!("{who} accepted quotation {no}: no contract yet"),
            subject(&[
                ("name", json!(who)),
                ("quotation_id", json!(id.to_string())),
                ("number", json!(no)),
                ("deal_id", id_v(deal)),
            ]),
            at,
            Some("propose_contract_from_quotation"),
            json!({"quotation_id": id.to_string()}),
            "Draft the contract from the accepted quotation (a card), then the down-payment proforma.",
            link_for("quotation", Some(id), None),
        ));
    }
    // A contract (draft or signed) whose quotation has no proforma yet.
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, String, Option<String>, Uuid, Option<Uuid>, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT c.id, c.number, c.status, c.quotation_id, c.deal_id, NULLIF(o.name, ''), c.created_at \
         FROM contracts c LEFT JOIN organizations o ON o.id = c.customer_org_id \
         LEFT JOIN deals d ON d.id = c.deal_id \
         WHERE c.is_deleted = false AND c.quotation_id IS NOT NULL \
           AND COALESCE(c.status, 'draft') NOT IN ('cancelled', 'terminated', 'expired', 'void') \
           AND (d.id IS NULL OR NOT (d.stage = ANY($1))) \
           AND NOT EXISTS (SELECT 1 FROM invoices i WHERE i.quotation_id = c.quotation_id \
                           AND i.invoice_type = 'proforma' AND COALESCE(i.status, '') NOT IN ('cancelled', 'void'))",
    )
    .bind(&closed)
    .fetch_all(&mut *db)
    .await?;
    for (id, number, status, quotation, deal, customer, at) in rows {
        let who = customer.unwrap_or_else(|| "the customer".into());
        out.push(cx.item(
            format!("contract_no_proforma:{id}"),
            "contract_no_proforma",
            format!(
                "Contract {number} for {who} ({}): no down-payment proforma yet",
                status.unwrap_or_else(|| "draft".into())
            ),
            subject(&[
                ("name", json!(who)),
                ("contract_id", json!(id.to_string())),
                ("number", json!(number)),
                ("quotation_id", json!(quotation.to_string())),
                ("deal_id", id_v(deal)),
            ]),
            at,
            Some("propose_downpayment_proforma"),
            json!({"quotation_id": quotation.to_string()}),
            "Raise the down-payment proforma from the quotation's payment terms (a card).",
            "/sales/contracts".to_string(),
        ));
    }
    // Deals with a signed contract or a paid down payment that are not Won.
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, String, String, bool, bool, DateTime<Utc>)> = sqlx::query_as(
        "SELECT d.id, d.title, d.stage, \
                EXISTS (SELECT 1 FROM contracts c WHERE c.is_deleted = false AND c.status = 'signed' \
                        AND (c.deal_id = d.id OR c.quotation_id IN (SELECT id FROM quotations WHERE deal_id = d.id))), \
                EXISTS (SELECT 1 FROM invoices i WHERE i.invoice_type = 'proforma' AND i.amount_paid > 0 \
                        AND i.quotation_id IN (SELECT id FROM quotations WHERE deal_id = d.id)), \
                d.updated_at \
         FROM deals d WHERE d.is_deleted = false AND NOT (d.stage = ANY($1))",
    )
    .bind(&closed)
    .fetch_all(&mut *db)
    .await?;
    for (id, title, stage, signed, paid, at) in rows {
        if !signed && !paid {
            continue;
        }
        let what = match (signed, paid) {
            (true, true) => "contract signed and down payment paid",
            (true, false) => "contract signed",
            _ => "down payment paid",
        };
        out.push(cx.item(
            format!("ready_to_win:{id}"),
            "ready_to_win",
            format!("{title}: {what}, the deal is still {stage}"),
            subject(&[
                ("name", json!(title)),
                ("deal_id", json!(id.to_string())),
                ("stage", json!(stage)),
                ("contract_signed", json!(signed)),
                ("down_payment_paid", json!(paid)),
            ]),
            at,
            Some("propose_deal_won"),
            json!({"deal_id": id.to_string()}),
            "Move the deal to Won when the rule is met (signed contract, and the down payment when \
             nary.contract.won_needs_dp is on), then the kickoff (propose_kickoff).",
            link_for("deal", Some(id), None),
        ));
    }
    Ok(())
}

async fn meetings(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    let remind_h = int(db, "nary.queue.meeting_reminder_hours").await?;
    let brief_h = int(db, "nary.queue.meeting_brief_hours").await?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        Uuid,
        Option<String>,
        DateTime<Utc>,
        Option<String>,
        String,
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, title, starts_at, chat_id, channel, deal_id, lead_id, contact_id, day_reminder_at, \
                hour_reminder_at, brief_at, created_at \
         FROM nary_meetings WHERE status = 'booked' AND starts_at > $1 AND starts_at <= $2",
    )
    .bind(cx.now)
    .bind(cx.now + TimeDelta::hours(remind_h.max(brief_h)))
    .fetch_all(&mut *db)
    .await?;
    for (
        id,
        title,
        starts,
        chat,
        channel,
        deal,
        lead,
        contact,
        day_rem,
        hour_rem,
        brief,
        _created,
    ) in rows
    {
        let name = first_text(&[title], "Meeting");
        let subj = subject(&[
            ("name", json!(name)),
            ("meeting_id", json!(id.to_string())),
            ("starts_at", json!(starts.to_rfc3339())),
            ("chat_id", s_v(chat.clone())),
            ("deal_id", id_v(deal)),
            ("lead_id", id_v(lead)),
            ("contact_id", id_v(contact)),
        ]);
        if day_rem.is_none() && hour_rem.is_none() && starts <= cx.now + TimeDelta::hours(remind_h)
        {
            out.push(cx.item(
                format!("meeting_reminder:{id}"),
                "meeting_reminder",
                format!("{name} on {}: no reminder sent to the customer", cx.local(starts)),
                subj.clone(),
                starts - TimeDelta::hours(remind_h),
                Some("propose_customer_message"),
                json!({"channel": if channel == "email" { "email" } else { "whatsapp" }, "chat_id": chat,
                       "meeting_id": id.to_string()}),
                "A short reminder with the meeting link (propose_customer_message with meeting_id, a card).",
                "/nary/meetings".to_string(),
            ));
        }
        if brief.is_none() && starts <= cx.now + TimeDelta::hours(brief_h) {
            let (otype, rid) = match (deal, lead, contact) {
                (Some(d), _, _) => ("deal", d),
                (None, Some(l), _) => ("lead", l),
                (None, None, Some(c)) => ("contact", c),
                _ => ("", Uuid::nil()),
            };
            out.push(cx.item(
                format!("meeting_brief:{id}"),
                "meeting_brief",
                format!("{name} on {}: the Owner has no brief yet", cx.local(starts)),
                subj,
                starts - TimeDelta::hours(brief_h),
                if otype.is_empty() { Some("list_meetings") } else { Some("get_activity_history") },
                if otype.is_empty() {
                    json!({})
                } else {
                    json!({"object_type": otype, "record_id": rid.to_string()})
                },
                "Give the Owner a short brief before the meeting: who they are, what they asked, open quotations and \
                 what to agree.",
                "/nary/meetings".to_string(),
            ));
        }
    }
    Ok(())
}

async fn meeting_files(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, String, String, String, Option<Uuid>, Option<Uuid>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, filename, kind, status, meeting_id, deal_id, created_at FROM nary_meeting_files \
         WHERE status IN ('stored', 'queued')",
    )
    .fetch_all(&mut *db)
    .await?;
    for (id, filename, kind, status, meeting, deal, at) in rows {
        out.push(cx.item(
            format!("meeting_file:{id}"),
            "meeting_file",
            format!(
                "Meeting {kind} {filename} waiting to be read{}",
                if status == "stored" {
                    " (no meeting chosen yet)"
                } else {
                    ""
                }
            ),
            subject(&[
                ("name", json!(filename)),
                ("file_id", json!(id.to_string())),
                ("kind", json!(kind)),
                ("status", json!(status)),
                ("meeting_id", id_v(meeting)),
                ("deal_id", id_v(deal)),
            ]),
            at,
            Some("transcribe_meeting_file"),
            json!({"file_id": id.to_string()}),
            "Only when the Owner asks: transcribe it and debrief the meeting (results on the deal).",
            "/nary/meetings".to_string(),
        ));
    }
    Ok(())
}

async fn pending_trigger_keys(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT trigger_key FROM agent_pending_actions WHERE status = 'pending' AND trigger_key IS NOT NULL",
    )
    .fetch_all(&mut *db)
    .await?)
}

async fn duplicates(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    for (keep, drop, why) in crate::crm::lead_merge::find_pairs(db).await? {
        let name = first_text(&[keep.title.clone(), drop.title.clone()], "Lead");
        out.push(cx.item(
            format!("duplicate_lead:{}:{}", keep.id, drop.id),
            "duplicate_lead",
            format!("Two open leads look like one ({why}): {name}"),
            subject(&[
                ("name", json!(name)),
                ("keep_id", json!(keep.id.to_string())),
                ("merge_id", json!(drop.id.to_string())),
                ("why", json!(why)),
            ]),
            drop.created_at,
            Some("propose_lead_merge"),
            json!({"keep_lead_id": keep.id.to_string(), "merge_lead_id": drop.id.to_string()}),
            "Check both leads (get_lead); merge them with propose_lead_merge (a card), or leave them when they \
             are two different needs.",
            link_for("lead", Some(keep.id), None),
        ));
    }
    let raised = pending_trigger_keys(db).await?;
    for p in crate::crm::deal_duplicates::find_pairs(db, None).await? {
        if raised.contains(&format!("deal_dup:{}:{}", p.keep.id, p.merge.id)) {
            continue; // the merge card is the item
        }
        out.push(cx.item(
            format!("duplicate_deal:{}:{}", p.keep.id, p.merge.id),
            "duplicate_deal",
            format!("Two open deals look like one ({}): {} and {}", p.why, p.keep.title, p.merge.title),
            subject(&[
                ("name", json!(p.keep.title)),
                ("keep_id", json!(p.keep.id.to_string())),
                ("merge_id", json!(p.merge.id.to_string())),
                ("why", json!(p.why)),
            ]),
            p.merge.created_at,
            Some("propose_deal_merge"),
            json!({"keep_id": p.keep.id.to_string(), "merge_id": p.merge.id.to_string()}),
            "Check both deals; merge them with propose_deal_merge (a card), or reject the card when they are \
             two deals.",
            link_for("deal", Some(p.keep.id), None),
        ));
    }
    Ok(())
}

async fn cards(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    let days = int(db, "nary.pending.expire_days").await?;
    let soon = int(db, "nary.queue.card_expiry_hours").await?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        Uuid,
        String,
        String,
        Option<Value>,
        Option<String>,
        Option<String>,
        Option<Uuid>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, action, origin, diff, rationale, entity_type, entity_id, created_at \
             FROM agent_pending_actions WHERE status = 'pending'",
    )
    .fetch_all(&mut *db)
    .await?;
    for (id, action, origin, diff, rationale, etype, eid, created) in rows {
        let label = crate::nary::actions::get(&action)
            .map(|a| a.label.clone())
            .unwrap_or_else(|_| action.clone());
        let what = diff
            .as_ref()
            .and_then(|d| d.get("label"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or(rationale)
            .unwrap_or_default();
        let expires = created + TimeDelta::days(days);
        let expiring = expires - TimeDelta::hours(soon) <= cx.now;
        let kind = if expiring {
            "card_expiring"
        } else {
            "card_pending"
        };
        out.push(cx.item(
            format!("card:{id}"),
            kind,
            format!(
                "Card waiting for the Owner: {label}{}{}",
                if what.is_empty() {
                    String::new()
                } else {
                    format!(": {}", clip(&what, 120))
                },
                if expiring {
                    format!(" (expires {})", cx.local(expires))
                } else {
                    String::new()
                }
            ),
            subject(&[
                ("name", json!(label)),
                ("card_id", json!(id.to_string())),
                ("action", json!(action)),
                ("origin", json!(origin)),
                ("entity_type", s_v(etype)),
                ("entity_id", id_v(eid)),
                ("expires_at", json!(expires.to_rfc3339())),
            ]),
            created,
            Some("nary_decide_card"),
            json!({"card_id": id.to_string(), "decision": "view"}),
            "Show the Owner the card word for word; apply or reject it only as the Owner says.",
            format!("/nary/approvals?card={id}"),
        ));
    }
    Ok(())
}

async fn approvals(db: &mut PgConnection, cx: &Ctx, out: &mut Vec<Item>) -> ApiResult<()> {
    let rows: Vec<ApprovalRequest> =
        sqlx::query_as("SELECT * FROM approval_requests WHERE status = 'pending'")
            .fetch_all(&mut *db)
            .await?;
    for r in rows {
        let Some(step) = crate::approvals::engine::current_step(&r) else {
            continue;
        };
        let Some(due) = step
            .get("due_at")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
        else {
            continue;
        };
        if due > cx.now {
            continue;
        }
        let label = r
            .document_label
            .clone()
            .unwrap_or_else(|| r.entity_type.clone());
        let step_name = step
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("approval");
        out.push(cx.item(
            format!("approval_overdue:{}", r.id),
            "approval_overdue",
            format!(
                "Overdue approval: {label} ({step_name}, due {})",
                cx.local(due)
            ),
            subject(&[
                ("name", json!(label)),
                ("request_id", json!(r.id.to_string())),
                ("entity_type", json!(r.entity_type)),
                ("entity_id", json!(r.entity_id.to_string())),
            ]),
            due,
            None,
            json!({}),
            "Remind the Owner to approve or reject it on the Approvals page.",
            format!("/approvals?request={}", r.id),
        ));
    }
    Ok(())
}

// ─── Listing (MCP tool, chat tool, ERP page) ────────────────────────────────

/// Filters of the list.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub kinds: Vec<String>,
    pub urgency: Option<String>,
    pub limit: Option<i64>,
}

impl Filter {
    /// From a tool's or a query's arguments: kinds (a list or comma-separated text), urgency, limit.
    pub fn from_args(args: &Value) -> Result<Filter, String> {
        let kinds: Vec<String> = match args.get("kinds") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            Some(Value::String(s)) => s.split(',').map(|k| k.trim().to_string()).collect(),
            _ => vec![],
        }
        .into_iter()
        .filter(|k| !k.is_empty())
        .collect();
        if let Some(bad) = kinds.iter().find(|k| !KINDS.contains(&k.as_str())) {
            return Err(format!("Unknown kind '{bad}'. Kinds: {}", KINDS.join(", ")));
        }
        let urgency = args
            .get("urgency")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        if let Some(u) = &urgency {
            if !URGENCIES.contains(&u.as_str()) {
                return Err(format!(
                    "Unknown urgency '{u}'. Use urgent, today or normal."
                ));
            }
        }
        let limit = match args.get("limit") {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) if !s.trim().is_empty() => Some(
                s.trim()
                    .parse()
                    .map_err(|_| "limit must be a whole number".to_string())?,
            ),
            _ => None,
        };
        Ok(Filter {
            kinds,
            urgency,
            limit,
        })
    }
}

/// The queue as the tool and the page show it: counts of everything waiting, then the filtered items.
pub async fn list(db: &mut PgConnection, filter: &Filter) -> ApiResult<Value> {
    let all = compute(db, Utc::now()).await?;
    let default_limit = int(db, "nary.queue.default_limit").await?;
    let limit = filter.limit.unwrap_or(default_limit).clamp(1, 500) as usize;
    let mut by_kind: BTreeMap<&str, i64> = BTreeMap::new();
    let mut by_urgency: Map<String, Value> = URGENCIES
        .iter()
        .map(|u| (u.to_string(), json!(0)))
        .collect();
    for i in &all {
        *by_kind.entry(i.kind).or_default() += 1;
        if let Some(n) = by_urgency.get_mut(i.urgency) {
            *n = json!(n.as_i64().unwrap_or(0) + 1);
        }
    }
    let matched: Vec<&Item> = all
        .iter()
        .filter(|i| filter.kinds.is_empty() || filter.kinds.iter().any(|k| k == i.kind))
        .filter(|i| filter.urgency.as_deref().is_none_or(|u| u == i.urgency))
        .collect();
    let paused = crate::nary::governance::is_paused(db).await?;
    Ok(json!({
        "total": all.len(),
        "matching": matched.len(),
        "shown": matched.len().min(limit),
        "by_urgency": by_urgency,
        "by_kind": by_kind,
        "nary_paused": paused,
        "scheduled_task_prompt": SCHEDULED_PROMPT,
        "items": matched.iter().take(limit).map(|i| i.to_json()).collect::<Vec<_>>(),
        "how": "Work the urgent items first, oldest first. Every message to a customer is a card for the Owner's \
                approval (propose_customer_message); never say something was sent until a tool result says applied.",
    }))
}

/// Who may read the queue: the Owner (Nary's approver), a superadmin or a system admin.
pub async fn may_read(db: &mut PgConnection, user: &AuthUser) -> ApiResult<bool> {
    if user.is_superadmin || user.is_system_admin {
        return Ok(true);
    }
    Ok(roles::approver_ids(db).await?.contains(&user.id))
}

/// The `nary_work_queue` tool (MCP act and above, Nary's chat).
pub async fn _h_nary_work_queue(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !may_read(db, user).await? {
        return Ok(json!({"error": "The work queue is for the Owner and system admins."}));
    }
    let filter = match Filter::from_args(&args) {
        Ok(f) => f,
        Err(e) => return Ok(json!({"error": e})),
    };
    list(db, &filter).await
}

// ─── The nudge ─────────────────────────────────────────────────────────────────

async fn state(db: &mut PgConnection, key: &str) -> ApiResult<Option<String>> {
    rhythm::_stamp(db, key).await
}

async fn state_time(db: &mut PgConnection, key: &str) -> ApiResult<Option<DateTime<Utc>>> {
    Ok(state(db, key)
        .await?
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc)))
}

/// The text of a nudge: the count, then the top items.
fn nudge_text(items: &[Item], first: &[&Item], top: usize) -> (String, String) {
    let urgent = items.iter().filter(|i| i.urgency == "urgent").count();
    let tag = if urgent > 0 {
        format!("{} item(s) waiting, {urgent} urgent", items.len())
    } else {
        format!("{} item(s) waiting", items.len())
    };
    let mut shown: Vec<&Item> = first.to_vec();
    for i in items {
        if shown.len() >= top {
            break;
        }
        if !shown.iter().any(|s| s.id == i.id) {
            shown.push(i);
        }
    }
    let mut lines: Vec<String> = shown
        .iter()
        .take(top.max(first.len()))
        .enumerate()
        .map(|(n, i)| {
            let mark = match i.urgency {
                "urgent" => "[urgent] ",
                "today" => "[today] ",
                _ => "",
            };
            format!("{}. {mark}{}", n + 1, clip(&i.title, 160))
        })
        .collect();
    if items.len() > lines.len() {
        lines.push(format!("...and {} more.", items.len() - lines.len()));
    }
    lines.push("Work them in your AI client (nary_work_queue) or on Nary > Work queue.".into());
    (tag, lines.join("\n"))
}

/// Tell the Owner: their WhatsApp chat with Nary when it is on, else the bell and WhatsApp.
async fn tell_owner(db: &mut PgConnection, tag: &str, text: &str, urgent: bool) -> ApiResult<()> {
    if owner_channel::enabled(db).await? {
        let mut t = owner_channel::Topic::new("alert", tag, text);
        t.ref_type = Some("work_queue".into());
        t.urgent = urgent;
        t.anytime = true;
        owner_channel::raise_topic(db, t).await?;
    } else {
        let approvers = roles::approver_ids(db).await?;
        notify(
            db,
            &approvers,
            Notice::new(tag, py_slice(text, 4000), PAGE, "work_queue", "queue")
                .type_("nary_work_queue")
                .priority(if urgent { "high" } else { "normal" })
                .email(false)
                .whatsapp(true),
        )
        .await?;
    }
    Ok(())
}

/// The nudge tick at `now` (tests pass the time). Returns what it did.
pub async fn nudge_tick_at(db: &mut PgConnection, now: DateTime<Utc>) -> ApiResult<Value> {
    if !flag(db, "nary.queue.nudge_enabled").await? {
        return Ok(json!({"skipped": "off"}));
    }
    let tz = granary_core::clock::local_tz(db).await?;
    let local = now.with_timezone(&tz);
    let day = local.date_naive();
    if !is_working_day(db, day).await? {
        return Ok(json!({"skipped": "not a working day"}));
    }
    let today = day.format("%Y-%m-%d").to_string();
    // the latest nudge time already passed today that has not gone out yet
    let mut due_slot: Option<NaiveTime> = None;
    for at in nudge_times(db).await? {
        if local.time() >= at
            && state(db, &format!("{SLOT_MARK}.{}", at.format("%H:%M")))
                .await?
                .as_deref()
                != Some(today.as_str())
        {
            due_slot = Some(at);
        }
    }
    let gap = int(db, "nary.queue.urgent_gap_minutes").await?;
    let top = int(db, "nary.queue.nudge_items").await?.max(1) as usize;
    let items = compute(db, now).await?;
    let newest_urgent = items
        .iter()
        .filter(|i| i.urgency == "urgent")
        .map(|i| i.since)
        .max();
    if let Some(slot) = due_slot {
        // every slot up to this one is done for today (a worker that was down sends one nudge, not three)
        for at in nudge_times(db).await? {
            if at <= slot {
                rhythm::_mark(db, &format!("{SLOT_MARK}.{}", at.format("%H:%M")), &today).await?;
            }
        }
        if items.is_empty() {
            commit(db).await?;
            return Ok(
                json!({"slot": slot.format("%H:%M").to_string(), "sent": false, "items": 0}),
            );
        }
        let (tag, text) = nudge_text(&items, &[], top);
        tell_owner(db, &tag, &text, false).await?;
        rhythm::_mark(db, LAST_NUDGE_AT, &now.to_rfc3339()).await?;
        if let Some(t) = newest_urgent {
            rhythm::_mark(db, URGENT_SEEN_AT, &t.to_rfc3339()).await?;
        }
        commit(db).await?;
        return Ok(
            json!({"slot": slot.format("%H:%M").to_string(), "sent": true, "items": items.len()}),
        );
    }
    // urgent: new urgent items since the last told one, in working hours, at most once per gap
    let seen = state_time(db, URGENT_SEEN_AT).await?;
    let fresh: Vec<&Item> = items
        .iter()
        .filter(|i| i.urgency == "urgent" && seen.is_none_or(|s| i.since > s))
        .collect();
    if fresh.is_empty() {
        return Ok(json!({"sent": false}));
    }
    let wh = rhythm::working_hours(db).await?;
    if !rhythm::in_hours(&local, &wh) {
        return Ok(json!({"sent": false, "waiting": "outside working hours"}));
    }
    if let Some(last) = state_time(db, LAST_NUDGE_AT).await? {
        if now - last < TimeDelta::minutes(gap) {
            return Ok(
                json!({"sent": false, "waiting": "the urgent nudge gap", "new_urgent": fresh.len()}),
            );
        }
    }
    let (tag, text) = nudge_text(&items, &fresh, top);
    let tag = format!("Urgent: {tag}");
    tell_owner(db, &tag, &text, true).await?;
    rhythm::_mark(db, LAST_NUDGE_AT, &now.to_rfc3339()).await?;
    if let Some(t) = newest_urgent {
        rhythm::_mark(db, URGENT_SEEN_AT, &t.to_rfc3339()).await?;
    }
    commit(db).await?;
    Ok(json!({"urgent": true, "sent": true, "new_urgent": fresh.len(), "items": items.len()}))
}

/// The worker beat's tick (every minute). Runs while Nary is paused.
pub async fn nudge_tick(db: &mut PgConnection) -> ApiResult<Value> {
    nudge_tick_at(db, Utc::now()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, urgency: &'static str, title: &str) -> Item {
        Item {
            id: id.into(),
            kind: "new_lead",
            title: title.into(),
            subject: json!({}),
            since: Utc::now(),
            urgency,
            suggested_tool: None,
            suggested_args: json!({}),
            hint: String::new(),
            link: String::new(),
        }
    }

    #[test]
    fn nudge_text_counts_and_lists_the_top() {
        let items = vec![
            item("a", "urgent", "Robert wrote"),
            item("b", "today", "Follow-up due"),
            item("c", "normal", "A card"),
        ];
        let (tag, text) = nudge_text(&items, &[], 2);
        assert_eq!(tag, "3 item(s) waiting, 1 urgent");
        assert!(
            text.starts_with("1. [urgent] Robert wrote\n2. [today] Follow-up due\n...and 1 more.")
        );
        let (_, text) = nudge_text(&items, &[&items[2]], 1);
        assert!(text.starts_with("1. A card\n"), "{text}");
        assert!(!text.contains('\u{2014}'));
    }

    #[test]
    fn the_erp_guide_quotes_the_scheduled_prompt_word_for_word() {
        let guide = include_str!("../ai/erp_guide/nary.md");
        assert!(guide.contains(&format!("\"{SCHEDULED_PROMPT}\"")));
        assert!(!SCHEDULED_PROMPT.contains('\u{2014}'));
    }

    #[test]
    fn filters_refuse_unknown_kinds_and_urgencies() {
        let f = Filter::from_args(
            &json!({"kinds": "new_lead, card_pending", "urgency": "urgent", "limit": 5}),
        )
        .unwrap();
        assert_eq!(f.kinds, vec!["new_lead", "card_pending"]);
        assert_eq!(f.limit, Some(5));
        assert!(Filter::from_args(&json!({"kinds": ["nope"]}))
            .unwrap_err()
            .contains("Unknown kind 'nope'"));
        assert!(Filter::from_args(&json!({"urgency": "soon"})).is_err());
    }
}
