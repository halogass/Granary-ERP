//! Port of backend/app/services/nary/cadence.py: the follow-up cadence after a quotation is sent
//! (Nary Phase 3 item B).
//!
//! quotation.sent -> one row per step of nary.cadence.steps (day 1/3/7/14/21/28/30 by default). The tick,
//! inside working hours, takes each step that is due:
//!
//! ```text
//!     the quotation is no longer "sent" (accepted, signed, rejected, revised...)  -> the rest is cancelled
//!     the customer wrote within nary.cadence.min_gap_days                        -> skipped: follow their rhythm
//!     one of our follow-ups went out within nary.cadence.min_gap_days            -> moved to after the gap
//!     nary.cadence.max_without_news follow-ups in a row carried no new information and this one
//!     wouldn't either (a step's "news" flag)                                     -> skipped
//!     whatsapp / email  -> Opus drafts it (nary.followup_draft); proposed as cadence.followup.send
//!     call              -> a reminder for the Owner to call
//!     erp               -> the expiry decision: extend, revise or nurture; with no answer in the last
//!                          nary.cadence.at_risk_silent_days the deal is flagged At Risk
//! ```
//!
//! Drafts start at level 1 (a card); a step type earns autonomy like any other action.
//!
//! Rust shape: Python changes the ORM rows in memory and flushes; here every change is an UPDATE at the
//! point Python would flush it (the tick flushes before each step, so the order of effects is the same).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, SubsecRound, TimeDelta, Utc};
use granary_compat::pyjson::{self, PyDateTime, PyValue};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json, SettingError};
use granary_db::rows::crm::Contact;
use granary_db::rows::nary::{NaryCadence, WaChat};
use granary_db::rows::sales::Quotation;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::crm::followups::{self, NewReminder};
use crate::nary::commit;
use crate::nary::replies;

pub const WHERE: &str = "Nary & AI";
pub const CHANNELS: [&str; 4] = ["whatsapp", "email", "call", "erp"];

/// `datetime.now(timezone.utc)` (microsecond precision, as Python and Postgres keep it).
fn now_utc() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

/// `dt.isoformat()` of an aware UTC datetime.
pub fn iso(dt: &DateTime<Utc>) -> String {
    PyDateTime::from_chrono_utc(dt).isoformat()
}

/// `quotation.full_number or quotation.number`.
pub fn qnum(q: &Quotation) -> String {
    match q.full_number.as_deref() {
        Some(f) if !f.is_empty() => f.to_string(),
        _ => q.number.clone(),
    }
}

/// `str(x)` of an optional string in an f-string (None prints "None").
fn s_or_none(v: Option<&str>) -> &str {
    v.unwrap_or("None")
}

/// `isinstance(v, int)` on a decoded JSON value (a bool is an int in Python): the int value.
fn json_int(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Bool(b)) => Some(*b as i64),
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => n.as_i64(),
        _ => None,
    }
}

/// `steps(db)`: nary.cadence.steps, checked: a non-empty list of {day, channel, intent, topic[, news]}.
pub async fn steps(db: &mut PgConnection) -> ApiResult<Vec<Value>> {
    let raw = require_json(db, "nary.cadence.steps", WHERE).await?;
    check_steps(raw)
}

/// The checks of `steps(db)` on the decoded setting.
pub fn check_steps(raw: Value) -> ApiResult<Vec<Value>> {
    let list = match raw {
        Value::Array(a) if !a.is_empty() => a,
        _ => {
            return Err(SettingError::new(
                "nary.cadence.steps",
                "must be a list of {day, channel, intent, topic}",
                WHERE,
            )
            .into())
        }
    };
    for s in &list {
        let Value::Object(m) = s else {
            // `s.get(...)` on a non-dict: AttributeError (a 500).
            return Err(ApiError::internal(anyhow::anyhow!(
                "AttributeError: nary.cadence.steps item has no attribute 'get'"
            )));
        };
        let channel_ok =
            matches!(m.get("channel"), Some(Value::String(c)) if CHANNELS.contains(&c.as_str()));
        if !channel_ok || json_int(m.get("day")).is_none() {
            let shown = pyjson::repr(&PyValue::from(s)).unwrap_or_default();
            return Err(SettingError::new(
                "nary.cadence.steps",
                &format!("has a bad step: {shown}"),
                WHERE,
            )
            .into());
        }
    }
    Ok(list)
}

/// One row `start` schedules.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub day: i32,
    pub channel: String,
    pub intent: Option<String>,
    pub topic: Option<String>,
    pub due_at: DateTime<Utc>,
}

/// The rows `start` writes for checked `steps` sent at `base`: `due_at = base + timedelta(days=day)`.
pub fn schedule(steps: &[Value], base: DateTime<Utc>) -> ApiResult<Vec<Planned>> {
    let mut out = vec![];
    for s in steps {
        let day = json_int(s.get("day")).unwrap_or(0);
        // timedelta(days=...) and datetime + timedelta: OverflowError outside years 1..=9999 (a 500).
        let due_at = TimeDelta::try_days(day)
            .filter(|_| day.abs() <= 999_999_999)
            .and_then(|d| base.checked_add_signed(d))
            .filter(|d| (1..=9999).contains(&chrono::Datelike::year(d)))
            .ok_or_else(|| {
                ApiError::internal(anyhow::anyhow!("OverflowError: date value out of range"))
            })?;
        let day = i32::try_from(day).map_err(|_| {
            ApiError::internal(anyhow::anyhow!("DataError: value out of int32 range"))
        })?;
        out.push(Planned {
            day,
            channel: s
                .get("channel")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string(),
            intent: step_text(s, "intent")?,
            topic: step_text(s, "topic")?,
            due_at,
        });
    }
    Ok(out)
}

/// A step's text field (`s.get("intent")`): None or a string (asyncpg refuses anything else for a text column).
fn step_text(s: &Value, key: &str) -> ApiResult<Option<String>> {
    match s.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) => Ok(Some(t.clone())),
        Some(other) => Err(ApiError::internal(anyhow::anyhow!(
            "DataError: invalid input for query argument (expected str, got {other})"
        ))),
    }
}

/// `@events.subscribe("quotation.sent")`: start the cadence of the quotation the event names.
pub async fn on_quotation_sent(
    db: &mut PgConnection,
    entity_id: Option<Uuid>,
    occurred_at: Option<DateTime<Utc>>,
) -> ApiResult<()> {
    let Some(id) = entity_id else { return Ok(()) };
    let q: Option<Quotation> = sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    if let Some(q) = q {
        start(db, &q, occurred_at).await?;
    }
    Ok(())
}

/// `@events.subscribe("quotation.accepted")` (cadence's handler).
pub async fn on_quotation_accepted(
    db: &mut PgConnection,
    entity_id: Option<Uuid>,
) -> ApiResult<()> {
    if let Some(id) = entity_id {
        cancel_rest(db, id, "the quotation was accepted").await?;
    }
    Ok(())
}

/// `@events.subscribe("quotation.rejected", "quotation.expired")`: `event_type` is the event's type.
pub async fn on_quotation_ended(
    db: &mut PgConnection,
    event_type: &str,
    entity_id: Option<Uuid>,
) -> ApiResult<()> {
    if let Some(id) = entity_id {
        // `event.type.split('.')[1]`
        let part = event_type.split('.').nth(1).ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!("IndexError: list index out of range"))
        })?;
        cancel_rest(db, id, &format!("the quotation was {part}")).await?;
    }
    Ok(())
}

/// `start(db, quotation, sent_at=None)`: schedule the steps of a sent quotation (once). A resend doesn't
/// restart the cadence. Flushes; the caller commits.
pub async fn start(
    db: &mut PgConnection,
    quotation: &Quotation,
    sent_at: Option<DateTime<Utc>>,
) -> ApiResult<i64> {
    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM nary_cadences WHERE quotation_id = $1 LIMIT 1")
            .bind(quotation.id)
            .fetch_optional(&mut *db)
            .await?;
    if exists.is_some() {
        return Ok(0);
    }
    let base = sent_at.unwrap_or_else(now_utc);
    let planned = schedule(&steps(db).await?, base)?;
    let mut rows = 0;
    for p in planned {
        sqlx::query(
            "INSERT INTO nary_cadences (id, quotation_id, deal_id, contact_id, day, channel, intent, topic, due_at, \
             status, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'scheduled', now(), now())",
        )
        .bind(Uuid::new_v4())
        .bind(quotation.id)
        .bind(quotation.deal_id)
        .bind(quotation.customer_contact_id)
        .bind(p.day)
        .bind(&p.channel)
        .bind(&p.intent)
        .bind(&p.topic)
        .bind(p.due_at)
        .execute(&mut *db)
        .await?;
        rows += 1;
    }
    Ok(rows)
}

/// `_last_inbound(db, contact_id)`: the customer's newest message (WhatsApp or email).
pub async fn _last_inbound(
    db: &mut PgConnection,
    contact_id: Option<Uuid>,
) -> ApiResult<Option<DateTime<Utc>>> {
    let Some(cid) = contact_id else {
        return Ok(None);
    };
    let wa: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT max(wa_ts) FROM wa_messages WHERE contact_id = $1 AND from_me = false",
    )
    .bind(cid)
    .fetch_one(&mut *db)
    .await?;
    let em: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT max(crm_email_messages.received_at) FROM crm_email_messages JOIN crm_email_threads \
         ON crm_email_threads.id = crm_email_messages.thread_id \
         WHERE crm_email_threads.linked_contact_id = $1 AND crm_email_messages.direction = 'inbound'",
    )
    .bind(cid)
    .fetch_one(&mut *db)
    .await?;
    Ok([wa, em].into_iter().flatten().max())
}

/// `_news_flags(db)`: {topic: news} (a later step with the same topic wins).
pub async fn _news_flags(db: &mut PgConnection) -> ApiResult<HashMap<Option<String>, bool>> {
    let mut out = HashMap::new();
    for s in steps(db).await? {
        let news = py_truthy(s.get("news"));
        out.insert(step_text_key(&s), news);
    }
    Ok(out)
}

/// The dict key `s.get("topic")`: a string topic or None (other JSON types never match a VARCHAR topic).
fn step_text_key(s: &Value) -> Option<String> {
    match s.get("topic") {
        Some(Value::String(t)) => Some(t.clone()),
        None | Some(Value::Null) => None,
        Some(other) => Some(format!("\u{0}{}", other)),
    }
}

fn py_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `news.get(topic)` truthiness.
fn has_news(news: &HashMap<Option<String>, bool>, topic: &Option<String>) -> bool {
    news.get(topic).copied().unwrap_or(false)
}

async fn set_step(
    db: &mut PgConnection,
    id: Uuid,
    status: &str,
    reason: Option<&str>,
    done_at: Option<DateTime<Utc>>,
) -> ApiResult<()> {
    // Only the columns Python assigns: reason is kept when the branch doesn't set it.
    match reason {
        Some(r) => {
            sqlx::query(
                "UPDATE nary_cadences SET status = $1, reason = $2, done_at = $3, updated_at = now() WHERE id = $4",
            )
            .bind(status)
            .bind(r)
            .bind(done_at)
            .bind(id)
            .execute(&mut *db)
            .await?;
        }
        None => {
            sqlx::query("UPDATE nary_cadences SET status = $1, done_at = $2, updated_at = now() WHERE id = $3")
                .bind(status)
                .bind(done_at)
                .bind(id)
                .execute(&mut *db)
                .await?;
        }
    }
    Ok(())
}

/// `tick(db)`: {"drafted", "skipped", "cancelled", "reminders", "moved"}. Commits.
pub async fn tick(db: &mut PgConnection) -> ApiResult<Value> {
    let mut drafted = 0i64;
    let mut skipped = 0i64;
    let mut cancelled = 0i64;
    let mut reminders = 0i64;
    let mut moved = 0i64;
    macro_rules! out {
        () => {
            json!({"drafted": drafted, "skipped": skipped, "cancelled": cancelled, "reminders": reminders, "moved": moved})
        };
    }
    if !crate::nary::rhythm::is_working_time(db, None).await? {
        return Ok(out!());
    }
    let now = now_utc();
    let gap = TimeDelta::days(require_int(db, "nary.cadence.min_gap_days", WHERE).await?);
    let max_plain = require_int(db, "nary.cadence.max_without_news", WHERE).await?;
    let news = _news_flags(db).await?;
    let rules = StepRules {
        now,
        gap,
        max_plain,
        news,
    };
    let due: Vec<NaryCadence> = sqlx::query_as(
        "SELECT * FROM nary_cadences WHERE status = 'scheduled' AND due_at <= $1 ORDER BY due_at, id LIMIT 50",
    )
    .bind(now)
    .fetch_all(&mut *db)
    .await?;
    // R&D 26 Oct (double sends): the WhatsApp and email steps that pass their checks are drafted together
    // after the pass, one message per customer (two quotations sent together used to get two follow-ups).
    let mut ready: Vec<(NaryCadence, Quotation)> = vec![];
    let mut ready_quotes: HashSet<Uuid> = HashSet::new();
    for mut step in due {
        let quotation: Option<Quotation> = sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
            .bind(step.quotation_id)
            .fetch_optional(&mut *db)
            .await?;
        let quotation = match quotation {
            Some(q) if q.status.as_deref() == Some("sent") => q,
            other => {
                let what = match &other {
                    Some(q) => s_or_none(q.status.as_deref()).to_string(),
                    None => "gone".to_string(),
                };
                cancelled +=
                    cancel_rest(db, step.quotation_id, &format!("the quotation is {what}")).await?;
                continue;
            }
        };
        if step.channel == "erp" {
            _expiry_decision(db, &mut step, &quotation, now).await?;
            reminders += 1;
            continue;
        }
        // held deal, customer in touch, gap, a second step of a quotation in this pass, no news
        let checked =
            check_step(db, &step, &rules, ready_quotes.contains(&step.quotation_id)).await?;
        match checked {
            Checked::Ready => {}
            Checked::Skipped => {
                skipped += 1;
                continue;
            }
            Checked::Moved => {
                moved += 1;
                continue;
            }
        }
        if step.channel == "call" {
            _call_reminder(db, &mut step, &quotation).await?;
            reminders += 1;
            continue;
        }
        ready_quotes.insert(step.quotation_id);
        ready.push((step, quotation));
    }
    for mut group in group_by_customer(ready) {
        // security 27 Oct (#22b): the steps pulled in pass the same checks as the due ones
        let (s, m) = pull_in_today(db, &mut group, &rules).await?;
        skipped += s;
        moved += m;
        if draft_group(db, &mut group).await? {
            drafted += 1;
        } else {
            skipped += 1;
        }
    }
    commit(db).await?;
    Ok(out!())
}

/// The numbers and news a pass of the cadence checks its steps with.
pub struct StepRules {
    pub now: DateTime<Utc>,
    pub gap: TimeDelta,
    pub max_plain: i64,
    pub news: HashMap<Option<String>, bool>,
}

/// What the checks of one WhatsApp, email or call step decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    /// it goes out (drafted, or a call reminder)
    Ready,
    /// skipped (the customer is in touch, no news)
    Skipped,
    /// due later (a held deal, the gap after the last follow-up, a second step of a quotation in this pass)
    Moved,
}

/// The checks every WhatsApp, email or call step passes before it goes out: a deal held as a likely duplicate,
/// the customer in touch, the gap after the quotation's last follow-up, a second step of a quotation already
/// going out (`quotation_in_pass`), nothing new to say. A skipped or moved step is written. Security 27 Oct
/// (#22b): the same checks run on the steps a same-day group pulls in.
pub async fn check_step(
    db: &mut PgConnection,
    step: &NaryCadence,
    rules: &StepRules,
    quotation_in_pass: bool,
) -> ApiResult<Checked> {
    // a deal held as a likely duplicate waits for the Owner's merge decision (its twin carries on)
    if deal_on_hold(db, step.deal_id).await? {
        sqlx::query("UPDATE nary_cadences SET due_at = $1, updated_at = now() WHERE id = $2")
            .bind(rules.now + TimeDelta::days(1))
            .bind(step.id)
            .execute(&mut *db)
            .await?;
        return Ok(Checked::Moved);
    }
    let last_in = _last_inbound(db, step.contact_id).await?;
    if let Some(li) = last_in {
        if rules.now - li < rules.gap {
            set_step(
                db,
                step.id,
                "skipped",
                Some("the customer is in touch; following their rhythm"),
                Some(rules.now),
            )
            .await?;
            return Ok(Checked::Skipped);
        }
    }
    // ORDER BY done_at DESC: Postgres puts NULL done_at first, as in Python.
    let done: Vec<NaryCadence> = sqlx::query_as(
        "SELECT * FROM nary_cadences WHERE quotation_id = $1 AND status IN ('drafted', 'done') \
         ORDER BY done_at DESC",
    )
    .bind(step.quotation_id)
    .fetch_all(&mut *db)
    .await?;
    if let Some(Some(last_done)) = done.first().map(|d| d.done_at) {
        if rules.now - last_done < rules.gap {
            sqlx::query("UPDATE nary_cadences SET due_at = $1, updated_at = now() WHERE id = $2")
                .bind(last_done + rules.gap)
                .bind(step.id)
                .execute(&mut *db)
                .await?;
            return Ok(Checked::Moved);
        }
    }
    // a second step of a quotation already going out in this pass follows the gap after it
    if quotation_in_pass {
        sqlx::query("UPDATE nary_cadences SET due_at = $1, updated_at = now() WHERE id = $2")
            .bind(rules.now + rules.gap)
            .bind(step.id)
            .execute(&mut *db)
            .await?;
        return Ok(Checked::Moved);
    }
    let chain: Vec<&NaryCadence> = done
        .iter()
        .filter(|d| !matches!((last_in, d.done_at), (Some(li), Some(da)) if da < li))
        .collect();
    // `chain[:max_plain]` with Python slice rules (a negative bound counts from the end).
    let n = chain.len() as i64;
    let take = if rules.max_plain >= 0 {
        rules.max_plain.min(n)
    } else {
        (n + rules.max_plain).max(0)
    } as usize;
    if n >= rules.max_plain
        && chain[..take]
            .iter()
            .all(|d| !has_news(&rules.news, &d.topic))
        && !has_news(&rules.news, &step.topic)
    {
        set_step(
            db,
            step.id,
            "skipped",
            Some("no new information to share"),
            Some(rules.now),
        )
        .await?;
        return Ok(Checked::Skipped);
    }
    Ok(Checked::Ready)
}

/// Whether the deal is held as a likely duplicate of another (`custom.duplicate_of`, set by
/// crm::deal_duplicates until the Owner decides the merge card).
pub async fn deal_on_hold(db: &mut PgConnection, deal_id: Option<Uuid>) -> ApiResult<bool> {
    let Some(id) = deal_id else {
        return Ok(false);
    };
    let held: Option<bool> =
        sqlx::query_scalar("SELECT custom ? 'duplicate_of' FROM deals WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    Ok(held.unwrap_or(false))
}

/// The steps ready to draft, one group per customer (their contact; a step without one stands alone), in
/// the order the first of each was due.
pub fn group_by_customer(
    ready: Vec<(NaryCadence, Quotation)>,
) -> Vec<Vec<(NaryCadence, Quotation)>> {
    let mut groups: Vec<Vec<(NaryCadence, Quotation)>> = vec![];
    for (step, q) in ready {
        let at = step
            .contact_id
            .and_then(|c| groups.iter().position(|g| g[0].0.contact_id == Some(c)));
        match at {
            Some(i) => groups[i].push((step, q)),
            None => groups.push(vec![(step, q)]),
        }
    }
    groups
}

/// The customer's other WhatsApp or email follow-ups due later today join this message (one automatic
/// message a day): steps still scheduled, of other quotations still sent, due before local midnight. Each
/// one passes the checks of a due step first ([`check_step`], security 27 Oct #22b); one that fails them is
/// skipped or moved as it would be when due, and stays out of the message. Returns (skipped, moved).
async fn pull_in_today(
    db: &mut PgConnection,
    group: &mut Vec<(NaryCadence, Quotation)>,
    rules: &StepRules,
) -> ApiResult<(i64, i64)> {
    let Some(contact_id) = group[0].0.contact_id else {
        return Ok((0, 0));
    };
    let tz = granary_core::clock::local_tz(db).await?;
    let local = rules.now.with_timezone(&tz);
    let Some(end) = (local.date_naive() + TimeDelta::days(1))
        .and_hms_opt(0, 0, 0)
        .and_then(|m| m.and_local_timezone(tz).earliest())
    else {
        return Ok((0, 0));
    };
    let quotes: Vec<Uuid> = group.iter().map(|(s, _)| s.quotation_id).collect();
    let later: Vec<NaryCadence> = sqlx::query_as(
        "SELECT DISTINCT ON (c.quotation_id) c.* FROM nary_cadences c \
         JOIN quotations q ON q.id = c.quotation_id AND q.status = 'sent' \
         WHERE c.contact_id = $1 AND c.status = 'scheduled' AND c.channel IN ('whatsapp', 'email') \
         AND c.due_at < $2 AND NOT (c.quotation_id = ANY($3)) ORDER BY c.quotation_id, c.due_at",
    )
    .bind(contact_id)
    .bind(end.with_timezone(&Utc))
    .bind(&quotes)
    .fetch_all(&mut *db)
    .await?;
    let (mut skipped, mut moved) = (0, 0);
    for step in later {
        match check_step(db, &step, rules, false).await? {
            Checked::Ready => {}
            Checked::Skipped => {
                skipped += 1;
                continue;
            }
            Checked::Moved => {
                moved += 1;
                continue;
            }
        }
        let q: Quotation = sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
            .bind(step.quotation_id)
            .fetch_one(&mut *db)
            .await?;
        group.push((step, q));
    }
    Ok((skipped, moved))
}

/// `cancel_rest(db, quotation_id, reason=...)`: every step still scheduled is cancelled. The caller commits.
pub async fn cancel_rest(
    db: &mut PgConnection,
    quotation_id: Uuid,
    reason: &str,
) -> ApiResult<i64> {
    let res = sqlx::query(
        "UPDATE nary_cadences SET status = 'cancelled', reason = $1, updated_at = now() \
         WHERE quotation_id = $2 AND status = 'scheduled'",
    )
    .bind(reason)
    .bind(quotation_id)
    .execute(&mut *db)
    .await?;
    Ok(res.rows_affected() as i64)
}

/// `_target(db, step)`: where this step can go: its channel when possible, else the other one.
pub async fn _target(
    db: &mut PgConnection,
    step: &NaryCadence,
) -> ApiResult<(Option<&'static str>, Option<WaChat>, Option<Contact>)> {
    let contact: Option<Contact> = match step.contact_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let mut chat: Option<WaChat> = None;
    if let Some(c) = &contact {
        chat = sqlx::query_as(
            "SELECT * FROM wa_chats WHERE contact_id = $1 AND status = 'active' AND staff_user_id IS NULL \
             ORDER BY last_inbound_at DESC NULLS LAST LIMIT 1",
        )
        .bind(c.id)
        .fetch_optional(&mut *db)
        .await?;
    }
    let has_email = contact
        .as_ref()
        .map(|c| c.email.as_deref().is_some_and(|e| !e.is_empty()))
        .unwrap_or(false);
    Ok(if step.channel == "whatsapp" {
        if chat.is_some() {
            (Some("whatsapp"), chat, contact)
        } else if has_email {
            (Some("email"), None, contact)
        } else {
            (None, None, contact)
        }
    } else if has_email {
        (Some("email"), None, contact)
    } else if chat.is_some() {
        (Some("whatsapp"), chat, contact)
    } else {
        (None, None, contact)
    })
}

/// `DRAFT_SCHEMA`.
pub fn draft_schema() -> Value {
    json!({"type": "object", "properties": {"text": {"type": "string"}, "subject": {"type": ["string", "null"]}},
           "required": ["text", "subject"], "additionalProperties": false})
}

/// `conv.language or ((conv.lead.ai_score or {}).get("language") if conv.lead else None) or "en"`: true for
/// English (the default; another language code writes in the customer's language).
pub fn draft_english(conv_language: Option<&str>, lead_ai_score: Option<&Value>) -> bool {
    if let Some(l) = conv_language.filter(|l| !l.is_empty()) {
        return l == "en";
    }
    // `(ai_score or {}).get("language")`: a falsy ai_score is {}; a non-dict one has no .get (500 in
    // Python; here no language).
    let lang = lead_ai_score
        .and_then(|s| s.get("language"))
        .filter(|v| py_truthy(Some(v)));
    match lang {
        Some(Value::String(l)) => l == "en",
        Some(_) => true,
        None => true,
    }
}

/// The per-call instructions of a follow-up draft (the `context` after the cache break).
pub fn draft_rules(
    day: i32,
    intent: Option<&str>,
    number: &str,
    valid_until: chrono::NaiveDate,
    english: bool,
    channel: &str,
    max_lines: i64,
) -> String {
    format!(
        "Write follow-up step 'day {day}' after quotation {number} was sent (valid until {}). Goal: {}. In {}. \
         Add value; never ask 'did you decide?'; ask where they are in the process instead. No new prices, \
         no discounts, no revised quotation. {}",
        pyjson::date_isoformat(valid_until),
        s_or_none(intent),
        if english { "English" } else { "the customer's language" },
        if channel == "whatsapp" {
            format!("WhatsApp: under {max_lines} lines, subject null.")
        } else {
            "Email: a short subject and plain paragraphs, no sign-off.".to_string()
        }
    )
}

/// The user message of a follow-up draft: the facts (json.dumps, ensure_ascii=False) and the conversation.
pub fn draft_content(
    number: &str,
    total: Option<rust_decimal::Decimal>,
    currency: Option<&str>,
    valid_until: chrono::NaiveDate,
    customer: Option<&str>,
    transcript: &str,
) -> ApiResult<String> {
    // `str(quotation.total or "")`: a zero total is falsy.
    let total = match total {
        Some(t) if !t.is_zero() => t.to_string(),
        _ => String::new(),
    };
    let opt = |v: Option<&str>| {
        v.map(|s| PyValue::Str(s.to_string()))
            .unwrap_or(PyValue::None)
    };
    let facts = PyValue::Dict(vec![
        (
            "quotation".into(),
            PyValue::Dict(vec![
                ("number".into(), PyValue::Str(number.to_string())),
                ("total".into(), PyValue::Str(total)),
                ("currency".into(), opt(currency)),
                (
                    "valid_until".into(),
                    PyValue::Str(pyjson::date_isoformat(valid_until)),
                ),
            ]),
        ),
        ("customer".into(), opt(customer)),
    ]);
    let facts_json = pyjson::dumps(&facts, &pyjson::DumpOpts::unicode())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
    let mut content = format!("FACTS:\n{facts_json}");
    if !transcript.is_empty() {
        content.push_str("\n\nCONVERSATION:\n");
        content.push_str(&super::llm::information_default("customer", transcript));
    }
    Ok(content)
}

/// `draft(db, step, quotation)`: Opus drafts the follow-up; proposed as cadence.followup.send.
pub async fn draft(
    db: &mut PgConnection,
    step: &mut NaryCadence,
    quotation: &Quotation,
) -> ApiResult<bool> {
    let mut group = vec![(step.clone(), quotation.clone())];
    let ok = draft_group(db, &mut group).await?;
    *step = group.swap_remove(0).0;
    Ok(ok)
}

/// R&D 26 Oct (double sends): one follow-up for a customer's steps due today (`group`, the first one leads):
/// one draft naming every quotation, one card, keyed `cadence:{lead step}` so a rerun returns that card. The
/// draft is kept on the lead step before the card is made: a rerun after a failure reuses it and never asks
/// the model again. Every step of the group is marked drafted with the card; the others `merged_into` the lead.
pub async fn draft_group(
    db: &mut PgConnection,
    group: &mut [(NaryCadence, Quotation)],
) -> ApiResult<bool> {
    let lead = group[0].0.clone();
    let quotation = group[0].1.clone();
    let (channel, chat, contact) = _target(db, &lead).await?;
    let Some(channel) = channel else {
        for (step, _) in group.iter_mut() {
            set_step(
                db,
                step.id,
                "skipped",
                Some("no WhatsApp chat or email for the customer"),
                Some(now_utc()),
            )
            .await?;
            step.status = "skipped".into();
        }
        return Ok(false);
    };
    let mut conv = if let Some(chat) = &chat {
        let transcript = super::channels::conversation_text(db, chat, None, 20).await?;
        replies::for_chat(db, chat, &transcript).await?
    } else {
        let mut conv = replies::Conversation::new("email", "");
        conv.contact = contact.clone();
        if let Some(did) = quotation.deal_id {
            conv.deal = sqlx::query_as("SELECT * FROM deals WHERE id = $1")
                .bind(did)
                .fetch_optional(&mut *db)
                .await?;
        }
        // the follow-up answers in the customer's latest email thread (so it threads, to everyone on it)
        if let Some(c) = &contact {
            conv.thread_id = sqlx::query_scalar(
                "SELECT id FROM crm_email_threads WHERE linked_contact_id = $1 \
                 ORDER BY last_message_at DESC NULLS LAST LIMIT 1",
            )
            .bind(c.id)
            .fetch_optional(&mut *db)
            .await?;
        }
        replies::_locale(db, &mut conv).await?;
        conv
    };
    let numbers: Vec<String> = group.iter().map(|(_, q)| qnum(q)).collect();
    let kept = lead.draft_text.clone().filter(|t| !t.trim().is_empty());
    let text = match kept {
        Some(t) => t,
        None => {
            let english = draft_english(
                conv.language.as_deref(),
                conv.lead.as_ref().and_then(|l| l.ai_score.as_ref()),
            );
            let max_lines = require_int(db, "nary.reply.max_lines", WHERE).await?;
            let (rules, content) = if group.len() == 1 {
                (
                    draft_rules(
                        lead.day,
                        lead.intent.as_deref(),
                        &numbers[0],
                        quotation.valid_until,
                        english,
                        channel,
                        max_lines,
                    ),
                    draft_content(
                        &numbers[0],
                        quotation.total,
                        quotation.currency.as_deref(),
                        quotation.valid_until,
                        contact.as_ref().map(|c| c.first_name.as_str()),
                        &conv.transcript,
                    )?,
                )
            } else {
                let quotes: Vec<FollowedQuote> = group
                    .iter()
                    .map(|(_, q)| FollowedQuote {
                        number: qnum(q),
                        total: q.total,
                        currency: q.currency.clone(),
                        valid_until: q.valid_until,
                    })
                    .collect();
                (
                    draft_rules_many(
                        lead.day,
                        lead.intent.as_deref(),
                        &quotes,
                        english,
                        channel,
                        max_lines,
                    ),
                    draft_content_many(
                        &quotes,
                        contact.as_ref().map(|c| c.first_name.as_str()),
                        &conv.transcript,
                    )?,
                )
            };
            let system = super::writing::customer_prompt(db, Some(channel), "", &rules).await?;
            let data = super::llm::ask_json(
                db,
                "nary.followup_draft",
                &system,
                &content,
                &draft_schema(),
                None,
                None,
                None,
            )
            .await?;
            data.as_ref()
                .and_then(|d| d.get("text"))
                .filter(|t| py_truthy(Some(t)))
                .and_then(|t| t.as_str())
                .map(|t| granary_compat::text::pystr::strip(t).to_string())
                .unwrap_or_default()
        }
    };
    if text.is_empty() {
        // A failed draft is not retried every minute: the steps are skipped, with the reason.
        for (step, _) in group.iter_mut() {
            set_step(
                db,
                step.id,
                "skipped",
                Some("the follow-up couldn't be drafted"),
                Some(now_utc()),
            )
            .await?;
            step.status = "skipped".into();
        }
        return Ok(false);
    }
    // kept before the card is made (the propose commits it first)
    sqlx::query("UPDATE nary_cadences SET draft_text = $1, updated_at = now() WHERE id = $2")
        .bind(&text)
        .bind(lead.id)
        .execute(&mut *db)
        .await?;
    conv.channel = channel.to_string();
    let payload = conv.reply_payload(&text);
    let rationale = format!(
        "Follow-up day {} for {}: {}",
        lead.day,
        numbers.join(", "),
        s_or_none(lead.intent.as_deref())
    );
    let card = replies::_propose_keyed(
        db,
        "cadence.followup.send",
        payload,
        None,
        None,
        Some(&rationale),
        None,
        Some(format!("cadence:{}", lead.id)),
    )
    .await?;
    let now = now_utc();
    for (i, (step, _)) in group.iter_mut().enumerate() {
        let merged_into = (i > 0).then_some(lead.id);
        sqlx::query(
            "UPDATE nary_cadences SET status = 'drafted', done_at = $1, card_id = $2, merged_into = $3, \
             updated_at = now() WHERE id = $4",
        )
        .bind(now)
        .bind(card.as_ref().map(|c| c.id))
        .bind(merged_into)
        .bind(step.id)
        .execute(&mut *db)
        .await?;
        step.status = "drafted".into();
        step.done_at = Some(now);
        step.card_id = card.as_ref().map(|c| c.id);
        step.merged_into = merged_into;
    }
    Ok(card.is_some())
}

/// One quotation a merged follow-up is about.
pub struct FollowedQuote {
    pub number: String,
    pub total: Option<rust_decimal::Decimal>,
    pub currency: Option<String>,
    pub valid_until: chrono::NaiveDate,
}

/// [`draft_rules`] for one message about several quotations (sent together, or due the same day).
pub fn draft_rules_many(
    day: i32,
    intent: Option<&str>,
    quotes: &[FollowedQuote],
    english: bool,
    channel: &str,
    max_lines: i64,
) -> String {
    let list = quotes
        .iter()
        .map(|q| {
            format!(
                "{} (valid until {})",
                q.number,
                pyjson::date_isoformat(q.valid_until)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Write ONE follow-up message, step 'day {day}', about all of these quotations together: {list}. Name \
         each quotation number once; never write a separate message per quotation. Goal: {}. In {}. Add \
         value; never ask 'did you decide?'; ask where they are in the process instead. No new prices, no \
         discounts, no revised quotation. {}",
        s_or_none(intent),
        if english { "English" } else { "the customer's language" },
        if channel == "whatsapp" {
            format!("WhatsApp: under {max_lines} lines, subject null.")
        } else {
            "Email: a short subject and plain paragraphs, no sign-off.".to_string()
        }
    )
}

/// [`draft_content`] for several quotations: FACTS carry `quotations`, a list.
pub fn draft_content_many(
    quotes: &[FollowedQuote],
    customer: Option<&str>,
    transcript: &str,
) -> ApiResult<String> {
    let opt = |v: Option<&str>| {
        v.map(|s| PyValue::Str(s.to_string()))
            .unwrap_or(PyValue::None)
    };
    let list = quotes
        .iter()
        .map(|q| {
            let total = match q.total {
                Some(t) if !t.is_zero() => t.to_string(),
                _ => String::new(),
            };
            PyValue::Dict(vec![
                ("number".into(), PyValue::Str(q.number.clone())),
                ("total".into(), PyValue::Str(total)),
                ("currency".into(), opt(q.currency.as_deref())),
                (
                    "valid_until".into(),
                    PyValue::Str(pyjson::date_isoformat(q.valid_until)),
                ),
            ])
        })
        .collect();
    let facts = PyValue::Dict(vec![
        ("quotations".into(), PyValue::List(list)),
        ("customer".into(), opt(customer)),
    ]);
    let facts_json = pyjson::dumps(&facts, &pyjson::DumpOpts::unicode())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
    let mut content = format!("FACTS:\n{facts_json}");
    if !transcript.is_empty() {
        content.push_str("\n\nCONVERSATION:\n");
        content.push_str(&super::llm::information_default("customer", transcript));
    }
    Ok(content)
}

/// The first of `roles.approver_ids(db)`, or None.
async fn first_approver(db: &mut PgConnection) -> ApiResult<Option<Uuid>> {
    Ok(super::roles::approver_ids(db).await?.into_iter().next())
}

/// `_call_reminder(db, step, quotation)`: a reminder for the Owner to call.
pub async fn _call_reminder(
    db: &mut PgConnection,
    step: &mut NaryCadence,
    quotation: &Quotation,
) -> ApiResult<()> {
    let owner = first_approver(db).await?;
    let contact: Option<(String,)> = match step.contact_id {
        Some(id) => {
            sqlx::query_as("SELECT first_name FROM contacts WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    if let Some(owner) = owner {
        let who = contact
            .map(|c| c.0)
            .unwrap_or_else(|| "the customer".into());
        let mut r = NewReminder::new(
            owner,
            if step.deal_id.is_some() {
                "deal"
            } else {
                "quotation"
            },
            &format!("Call {who} about {}", qnum(quotation)),
        );
        r.entity_id = Some(step.deal_id.unwrap_or(step.quotation_id));
        r.note = step.intent.clone();
        r.source = "quotation".into();
        r.rule_key = Some(format!("cadence:{}", step.id));
        followups::create(db, r).await?;
    }
    let now = now_utc();
    set_step(db, step.id, "done", None, Some(now)).await?;
    step.status = "done".into();
    step.done_at = Some(now);
    Ok(())
}

/// `_expiry_decision(db, step, quotation, now)`: Day 30: the quotation expires. Ask the Owner to extend,
/// revise or move to nurture; flag At Risk when the customer has been silent through the last days.
pub async fn _expiry_decision(
    db: &mut PgConnection,
    step: &mut NaryCadence,
    quotation: &Quotation,
    now: DateTime<Utc>,
) -> ApiResult<()> {
    let silent_days = require_int(db, "nary.cadence.at_risk_silent_days", WHERE).await?;
    let nurture_days = require_int(db, "nary.cadence.nurture_silent_days", WHERE).await?;
    let last_in = _last_inbound(db, step.contact_id).await?;
    let silent = match last_in {
        None => true,
        Some(li) => now - li > TimeDelta::days(silent_days),
    };
    let mut note = "Extend the validity, revise, or move the deal to nurture.".to_string();
    if let (Some(deal_id), true) = (step.deal_id, silent) {
        let custom: Option<(Value,)> = sqlx::query_as("SELECT custom FROM deals WHERE id = $1")
            .bind(deal_id)
            .fetch_optional(&mut *db)
            .await?;
        if let Some((custom,)) = custom {
            // `{**(deal.custom or {}), "at_risk": True, "at_risk_since": now.isoformat()}`
            let mut m = match custom {
                Value::Object(m) => m,
                Value::Null => serde_json::Map::new(),
                v if !py_truthy(Some(&v)) => serde_json::Map::new(),
                _ => {
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "TypeError: 'deal.custom' is not a mapping"
                    )))
                }
            };
            m.insert("at_risk".into(), Value::Bool(true));
            m.insert("at_risk_since".into(), Value::String(iso(&now)));
            sqlx::query("UPDATE deals SET custom = $1, updated_at = now() WHERE id = $2")
                .bind(Value::Object(m))
                .bind(deal_id)
                .execute(&mut *db)
                .await?;
            note = format!("At risk: no answer in the last {silent_days} days. {note}");
        }
    }
    let nurture = match last_in {
        None => true,
        Some(li) => now - li > TimeDelta::days(nurture_days),
    };
    if nurture {
        note.push_str(&format!(
            " Silent for {nurture_days}+ days: nurture is suggested."
        ));
    }
    if let Some(owner) = first_approver(db).await? {
        let mut r = NewReminder::new(
            owner,
            "quotation",
            &format!("Quotation {} expires: decide", qnum(quotation)),
        );
        r.entity_id = Some(quotation.id);
        r.note = Some(note.clone());
        r.source = "quotation".into();
        r.rule_key = Some(format!("cadence:{}", step.id));
        followups::create(db, r).await?;
    }
    set_step(db, step.id, "done", Some(&note), Some(now)).await?;
    step.status = "done".into();
    step.done_at = Some(now);
    step.reason = Some(note);
    Ok(())
}

/// `timeline(db, deal_id=None, quotation_id=None)`: the steps, oldest due first.
pub async fn timeline(
    db: &mut PgConnection,
    deal_id: Option<Uuid>,
    quotation_id: Option<Uuid>,
) -> ApiResult<Vec<Value>> {
    let rows: Vec<NaryCadence> = match quotation_id {
        Some(q) => {
            sqlx::query_as("SELECT * FROM nary_cadences WHERE quotation_id = $1 ORDER BY due_at")
                .bind(q)
                .fetch_all(&mut *db)
                .await?
        }
        // `NaryCadence.deal_id == None` renders IS NULL.
        None => {
            sqlx::query_as(
                "SELECT * FROM nary_cadences WHERE deal_id IS NOT DISTINCT FROM $1 ORDER BY due_at",
            )
            .bind(deal_id)
            .fetch_all(&mut *db)
            .await?
        }
    };
    Ok(rows
        .iter()
        .map(|r| {
            json!({"id": r.id.to_string(), "day": r.day, "channel": r.channel, "topic": r.topic, "intent": r.intent,
                   "due_at": iso(&r.due_at), "status": r.status, "reason": r.reason,
                   "card_id": r.card_id.map(|c| c.to_string())})
        })
        .collect())
}

/// The `@events.subscribe` handlers of cadence.py, in its order (collected by events.rs).
pub fn subscriptions() -> Vec<super::events::Subscription> {
    use super::events::{subscriber, Subscription};
    vec![
        Subscription::new(
            &["quotation.sent"],
            subscriber(|db, e| Box::pin(on_quotation_sent(db, e.entity_id, Some(e.occurred_at)))),
        ),
        Subscription::new(
            &["quotation.accepted"],
            subscriber(|db, e| Box::pin(on_quotation_accepted(db, e.entity_id))),
        ),
        Subscription::new(
            &["quotation.rejected", "quotation.expired"],
            subscriber(|db, e| Box::pin(on_quotation_ended(db, &e.type_, e.entity_id))),
        ),
    ]
}
