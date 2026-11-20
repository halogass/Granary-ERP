//! Port of backend/app/services/crm/followups.py: CRM follow-up reminders (no AI: dates, rules, an owner).
//!
//! A reminder is a private to-do for one of our people: set by hand ("remind me about this chat on
//! Thursday") or raised by a rule (the customer never replied, a lead went quiet, a deal is stuck, a
//! quotation got no answer, a next action fell due). When it falls due the owner gets the bell and one
//! WhatsApp digest; if it is still open `escalate_after_hours` later, an email follows. Everything overdue
//! lands in the Owner's weekly digest. Thresholds, quiet hours and the digest schedule live in Settings
//! (`crm.followup.*`).
//!
//! Python's session runs with autoflush off; every write here is executed when Python flushes or commits
//! it, and `db.commit()` / `db.rollback()` are [`py_commit`] / [`py_rollback`].

use std::cell::Cell;

use chrono::{
    DateTime, Datelike, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone, Utc,
};
use chrono_tz::Tz;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_compat::text::utils::add_business_days;
use granary_core::clock;
use granary_core::errors::ApiError;
use granary_db::rows::crm::CrmReminder;

use crate::crm::stages::{ACTIVE_LEAD_STATUSES, ACTIVE_STAGES};

pub type Result<T> = std::result::Result<T, ApiError>;

pub const SOURCES: [&str; 7] = [
    "manual",
    "no_reply",
    "stale_lead",
    "stale_deal",
    "quotation",
    "recontact",
    "next_action",
];
pub const ENTITY_TYPES: [&str; 6] = [
    "contact",
    "lead",
    "deal",
    "quotation",
    "email_thread",
    "whatsapp_chat",
];

/// `LINKS`.
pub const LINKS: [(&str, &str); 6] = [
    ("lead", "/crm/leads/{id}"),
    ("deal", "/crm/deals?deal={id}"),
    ("contact", "/crm/contacts?contact={id}"),
    ("quotation", "/sales/quotations/{id}"),
    ("email_thread", "/crm/email?thread={id}"),
    ("whatsapp_chat", "/crm/whatsapp?chat={id}"),
];

// ─── Session helpers (shared by the crm modules of this port) ────────────────

/// `await db.commit()` on a connection the caller may hold inside a transaction: COMMIT and begin the
/// next one (SQLAlchemy autobegin). Outside a transaction every statement has already committed.
pub(crate) async fn py_commit(db: &mut PgConnection) -> std::result::Result<(), sqlx::Error> {
    if in_transaction(db) {
        // Executor::execute (a boxed Send future): `raw_sql(..).execute(..)` is not "Send general enough"
        // for callers in higher-ranked contexts (the Nary action registry).
        sqlx::Executor::execute(&mut *db, sqlx::raw_sql("COMMIT; BEGIN")).await?;
    }
    Ok(())
}

/// `await db.rollback()`: ROLLBACK and begin the next transaction. Outside a transaction there is nothing
/// to undo (every statement has already committed), which is the one place the port can differ.
pub(crate) async fn py_rollback(db: &mut PgConnection) -> std::result::Result<(), sqlx::Error> {
    if in_transaction(db) {
        sqlx::Executor::execute(&mut *db, sqlx::raw_sql("ROLLBACK; BEGIN")).await?;
    }
    Ok(())
}

fn in_transaction(db: &PgConnection) -> bool {
    use sqlx::TransactionManager;
    <sqlx::postgres::PgTransactionManager as TransactionManager>::get_transaction_depth(db) > 0
}

// ─── Clock ───────────────────────────────────────────────────────────────────

thread_local! {
    static NOW_OVERRIDE: Cell<Option<DateTime<Utc>>> = const { Cell::new(None) };
}

/// Tests' `monkeypatch.setattr(followups, "_now", ...)`: fix `_now()` on this thread (None restores it).
pub fn set_now_override(now: Option<DateTime<Utc>>) {
    NOW_OVERRIDE.with(|c| c.set(now));
}

/// `_now()`.
pub fn _now() -> DateTime<Utc> {
    NOW_OVERRIDE.with(|c| c.get()).unwrap_or_else(Utc::now)
}

/// face/attendance.py `local(tz, dt)`: `(dt or now).astimezone(tz)` (its `now` is the real clock).
fn local(tz: Tz, at: Option<DateTime<Utc>>) -> DateTime<Tz> {
    at.unwrap_or_else(Utc::now).with_timezone(&tz)
}

/// `(a - b).days` for aware datetimes (floor division of the difference by a day).
fn days_between(a: DateTime<Utc>, b: DateTime<Utc>) -> i64 {
    let us = (a - b).num_microseconds().unwrap_or(i64::MAX);
    us.div_euclid(86_400_000_000)
}

/// A naive wall-clock time in `tz` as Python resolves it (`fold=0`): the earlier of two ambiguous times;
/// inside a gap, the offset in force before the gap.
fn resolve_local(tz: Tz, naive: NaiveDateTime) -> DateTime<Utc> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(d) => d.with_timezone(&Utc),
        LocalResult::Ambiguous(a, b) => a.min(b).with_timezone(&Utc),
        LocalResult::None => {
            let before = tz.offset_from_utc_datetime(&(naive - TimeDelta::days(1)));
            use chrono::Offset;
            let secs = before.fix().local_minus_utc() as i64;
            Utc.from_utc_datetime(&(naive - TimeDelta::seconds(secs)))
        }
    }
}

// ─── Settings ────────────────────────────────────────────────────────────────

/// `settings(db)` / `DEFAULTS`: the `crm.followup.*` values (Python passes a dict around).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FollowupSettings {
    pub default_delay_days: i64,
    pub no_reply_days: i64,
    pub lead_idle_days: i64,
    pub max_per_rule_pass: i64,
    pub deal_stage_idle_days: i64,
    pub quotation_followup_days: i64,
    pub quiet_from: String,
    pub quiet_to: String,
    pub digest_weekday: i64,
    pub digest_time: String,
    pub due_hour: i64,
    pub escalate_after_hours: i64,
    pub rule_no_reply: bool,
    pub rule_stale_lead: bool,
    pub rule_stale_deal: bool,
    pub rule_quotation: bool,
    pub rule_next_action: bool,
    pub digest_max_items: i64,
}

impl Default for FollowupSettings {
    /// `DEFAULTS`.
    fn default() -> Self {
        FollowupSettings {
            default_delay_days: 3,
            no_reply_days: 3,
            lead_idle_days: 7,
            max_per_rule_pass: 25,
            deal_stage_idle_days: 14,
            quotation_followup_days: 5,
            quiet_from: "08:00".into(),
            quiet_to: "20:00".into(),
            digest_weekday: 1,
            digest_time: "08:00".into(),
            due_hour: 9,
            escalate_after_hours: 24,
            rule_no_reply: true,
            rule_stale_lead: true,
            rule_stale_deal: true,
            rule_quotation: true,
            rule_next_action: true,
            digest_max_items: 10,
        }
    }
}

enum Slot<'a> {
    Int(&'a mut i64),
    Str(&'a mut String),
    Bool(&'a mut bool),
}

impl FollowupSettings {
    /// The DEFAULTS keys in their dict order, with where each value lives.
    fn slots(&mut self) -> Vec<(&'static str, Slot<'_>)> {
        vec![
            (
                "default_delay_days",
                Slot::Int(&mut self.default_delay_days),
            ),
            ("no_reply_days", Slot::Int(&mut self.no_reply_days)),
            ("lead_idle_days", Slot::Int(&mut self.lead_idle_days)),
            ("max_per_rule_pass", Slot::Int(&mut self.max_per_rule_pass)),
            (
                "deal_stage_idle_days",
                Slot::Int(&mut self.deal_stage_idle_days),
            ),
            (
                "quotation_followup_days",
                Slot::Int(&mut self.quotation_followup_days),
            ),
            ("quiet_from", Slot::Str(&mut self.quiet_from)),
            ("quiet_to", Slot::Str(&mut self.quiet_to)),
            ("digest_weekday", Slot::Int(&mut self.digest_weekday)),
            ("digest_time", Slot::Str(&mut self.digest_time)),
            ("due_hour", Slot::Int(&mut self.due_hour)),
            (
                "escalate_after_hours",
                Slot::Int(&mut self.escalate_after_hours),
            ),
            ("rule_no_reply", Slot::Bool(&mut self.rule_no_reply)),
            ("rule_stale_lead", Slot::Bool(&mut self.rule_stale_lead)),
            ("rule_stale_deal", Slot::Bool(&mut self.rule_stale_deal)),
            ("rule_quotation", Slot::Bool(&mut self.rule_quotation)),
            ("rule_next_action", Slot::Bool(&mut self.rule_next_action)),
            ("digest_max_items", Slot::Int(&mut self.digest_max_items)),
        ]
    }

    /// `cfg.get("rule_...")` for the RULES keys.
    fn rule_enabled(&self, key: &str) -> bool {
        match key {
            "rule_no_reply" => self.rule_no_reply,
            "rule_stale_lead" => self.rule_stale_lead,
            "rule_stale_deal" => self.rule_stale_deal,
            "rule_quotation" => self.rule_quotation,
            "rule_next_action" => self.rule_next_action,
            _ => false,
        }
    }

    /// Apply raw `crm.followup.*` values (key without the prefix -> value) as `settings()` does.
    pub fn apply_raw(&mut self, raw: &std::collections::HashMap<String, String>) -> Result<()> {
        for (key, slot) in self.slots() {
            let value = match raw.get(key) {
                Some(v) if !v.is_empty() => v,
                _ => continue,
            };
            match slot {
                Slot::Bool(b) => {
                    let v = pystr::lower(pystr::strip(value));
                    *b = !matches!(v.as_str(), "false" | "0" | "no" | "off");
                }
                Slot::Int(i) => match py_float(value) {
                    // int(float(value)): NaN is a ValueError (default kept); infinity an OverflowError
                    // that Python does not catch.
                    Some(f) if f.is_nan() => {}
                    Some(f) if f.is_infinite() => {
                        return Err(ApiError::internal(anyhow::anyhow!(
                            "OverflowError: cannot convert float infinity to integer"
                        )))
                    }
                    Some(f) => *i = f.trunc() as i64,
                    None => {}
                },
                Slot::Str(s) => *s = value.clone(),
            }
        }
        Ok(())
    }
}

/// Python `float(s)` for a str: None where Python raises ValueError.
pub(crate) fn py_float(s: &str) -> Option<f64> {
    // Python maps every Unicode decimal digit to its ASCII digit first.
    let mapped: String = pystr::strip(s)
        .chars()
        .map(|c| match pystr::decimal_value(c) {
            Some(d) if !c.is_ascii() => char::from(b'0' + d as u8),
            _ => c,
        })
        .collect();
    let t = mapped.as_str();
    if t.is_empty() {
        return None;
    }
    // Underscores only between two digits.
    let b = t.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c == b'_' {
            let ok =
                i > 0 && i + 1 < b.len() && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit();
            if !ok {
                return None;
            }
        }
    }
    let cleaned: String = t.chars().filter(|c| *c != '_').collect();
    let body = cleaned.trim_start_matches(['+', '-']);
    if cleaned.len() - body.len() > 1 {
        return None;
    }
    let lower = body.to_ascii_lowercase();
    if matches!(lower.as_str(), "inf" | "infinity" | "nan") {
        return cleaned.to_ascii_lowercase().parse::<f64>().ok();
    }
    // Digits, an optional point, an optional exponent; at least one mantissa digit.
    let (mant, exp) = match lower.find('e') {
        Some(p) => (&lower[..p], Some(&lower[p + 1..])),
        None => (lower.as_str(), None),
    };
    let mut parts = mant.splitn(2, '.');
    let int_part = parts.next().unwrap_or("");
    let frac_part = parts.next().unwrap_or("");
    if !int_part.bytes().all(|c| c.is_ascii_digit())
        || !frac_part.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if let Some(e) = exp {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        if e.is_empty() || !e.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    cleaned.parse::<f64>().ok()
}

/// Python `int(s)` for a str (base 10): None where Python raises ValueError.
fn py_int(s: &str) -> Option<i64> {
    let t = pystr::strip(s);
    let (neg, body) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut n: i64 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = pystr::decimal_value(c)? as i64;
        n = n.checked_mul(10)?.checked_add(d)?;
    }
    Some(if neg { -n } else { n })
}

/// `settings(db)`: the `crm.followup.*` settings, with defaults for anything unset or malformed.
pub async fn settings(db: &mut PgConnection) -> Result<FollowupSettings> {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT key, value FROM system_settings WHERE key LIKE 'crm.followup.%'")
            .fetch_all(&mut *db)
            .await?;
    let raw = rows
        .into_iter()
        .map(|(k, v)| {
            let k = k
                .strip_prefix("crm.followup.")
                .map(str::to_string)
                .unwrap_or(k);
            (k, v.unwrap_or_default())
        })
        .collect();
    let mut out = FollowupSettings::default();
    out.apply_raw(&raw)?;
    Ok(out)
}

/// `_hhmm(value, fallback)`.
fn _hhmm(value: &str, fallback: NaiveTime) -> NaiveTime {
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() != 2 {
        return fallback;
    }
    match (py_int(parts[0]), py_int(parts[1])) {
        (Some(h), Some(m)) if (0..24).contains(&h) && (0..60).contains(&m) => {
            NaiveTime::from_hms_opt(h as u32, m as u32, 0).unwrap_or(fallback)
        }
        _ => fallback,
    }
}

/// `in_quiet_hours(cfg, tz, at)`: True when a reminder may be delivered (inside the working window, company
/// time `tz`). `at` None is the real clock.
pub fn in_quiet_hours(cfg: &FollowupSettings, tz: Tz, at: Option<DateTime<Utc>>) -> bool {
    let now = local(tz, at).time();
    let from = _hhmm(&cfg.quiet_from, NaiveTime::from_hms_opt(8, 0, 0).unwrap());
    let to = _hhmm(&cfg.quiet_to, NaiveTime::from_hms_opt(20, 0, 0).unwrap());
    from <= now && now <= to
}

/// `default_due(cfg, tz, at)`: N business days ahead at the due hour, company time. A due hour outside
/// 0..23 is Python's ValueError from `.replace(hour=...)` (a 500).
pub fn default_due(
    cfg: &FollowupSettings,
    tz: Tz,
    at: Option<DateTime<Utc>>,
) -> Result<DateTime<Utc>> {
    let start = local(tz, at).naive_local();
    let day = add_business_days(start, cfg.default_delay_days);
    if !(0..24).contains(&cfg.due_hour) {
        return Err(ApiError::internal(anyhow::anyhow!(
            "ValueError: hour must be in 0..23"
        )));
    }
    let at_hour = day
        .date()
        .and_hms_opt(cfg.due_hour as u32, 0, 0)
        .expect("valid hour");
    Ok(resolve_local(tz, at_hour))
}

/// `link_for(entity_type, entity_id, chat_id)`.
pub fn link_for(entity_type: &str, entity_id: Option<Uuid>, chat_id: Option<&str>) -> String {
    let template = LINKS
        .iter()
        .find(|(k, _)| *k == entity_type)
        .map(|(_, t)| *t);
    let key: Option<String> = if entity_type == "whatsapp_chat" {
        chat_id.filter(|c| !c.is_empty()).map(str::to_string)
    } else {
        entity_id.map(|u| u.hyphenated().to_string())
    };
    match (template, key) {
        (Some(t), Some(k)) => t.replace("{id}", &k),
        _ => "/crm/followups".to_string(),
    }
}

// ─── Creating ────────────────────────────────────────────────────────────────

/// The keyword arguments of `create(...)`.
#[derive(Debug, Clone)]
pub struct NewReminder {
    pub owner_user_id: Uuid,
    pub entity_type: String,
    pub title: String,
    pub entity_id: Option<Uuid>,
    pub chat_id: Option<String>,
    pub note: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    /// default "manual"
    pub source: String,
    pub rule_key: Option<String>,
    pub created_by_id: Option<Uuid>,
}

impl NewReminder {
    pub fn new(owner_user_id: Uuid, entity_type: &str, title: &str) -> NewReminder {
        NewReminder {
            owner_user_id,
            entity_type: entity_type.to_string(),
            title: title.to_string(),
            entity_id: None,
            chat_id: None,
            note: None,
            due_at: None,
            source: "manual".to_string(),
            rule_key: None,
            created_by_id: None,
        }
    }
}

/// A reminder for a lead that was promoted to a deal is made on the deal (batch 2 T3, A6: follow-ups of a
/// meeting went to the old lead and nobody saw them on the deal).
pub async fn to_promoted_deal(db: &mut PgConnection, mut new: NewReminder) -> Result<NewReminder> {
    if new.entity_type != "lead" {
        return Ok(new);
    }
    let Some(lead_id) = new.entity_id else {
        return Ok(new);
    };
    let deal: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT leads.converted_to_deal_id FROM leads WHERE leads.id = $1")
            .bind(lead_id)
            .fetch_optional(&mut *db)
            .await?;
    if let Some(Some(deal_id)) = deal {
        new.entity_type = "deal".into();
        new.entity_id = Some(deal_id);
    }
    Ok(new)
}

/// `create(db, **kw)`: make a reminder (flushed). A rule's reminder is skipped (None) when that rule already
/// has an open one (rule_key), or when a "gone quiet" rule already has an open reminder on the same record.
pub async fn create(db: &mut PgConnection, new: NewReminder) -> Result<Option<CrmReminder>> {
    // batch 2 T3 (A6): a lead promoted to a deal lives on as the deal; its follow-ups go there.
    let new = to_promoted_deal(db, new).await?;
    // R&D 26 Oct: a deal held as a likely duplicate gets no rule reminders (its twin has them)
    if new.entity_type == "deal" && new.source != "manual" {
        if let Some(id) = new.entity_id {
            if crate::crm::deal_duplicates::on_hold(db, id).await? {
                return Ok(None);
            }
        }
    }
    if let Some(rule_key) = &new.rule_key {
        if !rule_key.is_empty() {
            let existing: Option<(Uuid,)> = sqlx::query_as(
                "SELECT id FROM crm_reminders WHERE rule_key = $1 AND status = 'open' LIMIT 1",
            )
            .bind(rule_key)
            .fetch_optional(&mut *db)
            .await?;
            if existing.is_some() {
                return Ok(None);
            }
            if let Some(entity_id) = new.entity_id {
                if matches!(
                    new.source.as_str(),
                    "stale_lead" | "stale_deal" | "quotation"
                ) {
                    let same: Option<(Uuid,)> = sqlx::query_as(
                        "SELECT id FROM crm_reminders WHERE source = $1 AND entity_type = $2 \
                         AND entity_id = $3 AND status = 'open' LIMIT 1",
                    )
                    .bind(&new.source)
                    .bind(&new.entity_type)
                    .bind(entity_id)
                    .fetch_optional(&mut *db)
                    .await?;
                    if same.is_some() {
                        return Ok(None);
                    }
                }
            }
        }
    }
    let due_at = match new.due_at {
        Some(d) => d,
        None => {
            let cfg = settings(db).await?;
            let tz = clock::local_tz(db).await?;
            default_due(&cfg, tz, None)?
        }
    };
    let title: String = new.title.chars().take(255).collect();
    let row: CrmReminder = sqlx::query_as(
        "INSERT INTO crm_reminders (id, owner_user_id, created_by_id, source, entity_type, entity_id, chat_id, \
         title, note, due_at, original_due_at, status, snooze_count, rule_key) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10, 'open', 0, $11) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(new.owner_user_id)
    .bind(new.created_by_id.unwrap_or(new.owner_user_id))
    .bind(&new.source)
    .bind(&new.entity_type)
    .bind(new.entity_id)
    .bind(&new.chat_id)
    .bind(&title)
    .bind(&new.note)
    .bind(due_at)
    .bind(&new.rule_key)
    .fetch_one(&mut *db)
    .await?;
    Ok(Some(row))
}

/// `_owner_of_activity`: who last wrote to this customer from our side.
async fn _owner_of_activity(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<Option<Uuid>> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT created_by_id FROM activities WHERE entity_type = $1 AND entity_id = $2 \
         AND direction = 'outbound' AND created_by_id IS NOT NULL \
         ORDER BY completed_at DESC NULLS LAST, created_at DESC LIMIT 1",
    )
    .bind(entity_type)
    .bind(entity_id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.and_then(|r| r.0))
}

/// `owner_for(db, entity_type, entity_id)`: assigned owner, else creator, else whoever last wrote to the
/// customer. None: nobody is reminded.
pub async fn owner_for(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<Option<Uuid>> {
    if entity_type == "lead" || entity_type == "deal" {
        let table = if entity_type == "lead" {
            "leads"
        } else {
            "deals"
        };
        let row: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(&format!(
            "SELECT assigned_to_id, created_by_id FROM {table} WHERE id = $1"
        ))
        .bind(entity_id)
        .fetch_optional(&mut *db)
        .await?;
        return Ok(row.and_then(|(a, c)| a.or(c)));
    }
    if entity_type == "quotation" {
        let row: Option<(Option<Uuid>,)> =
            sqlx::query_as("SELECT created_by_id FROM quotations WHERE id = $1")
                .bind(entity_id)
                .fetch_optional(&mut *db)
                .await?;
        return Ok(row.and_then(|r| r.0));
    }
    // contact / chat / thread: the owner of a linked deal, else lead, else the last sender
    for table in ["deals", "leads"] {
        let row: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(&format!(
            "SELECT assigned_to_id, created_by_id FROM {table} WHERE contact_id = $1 AND is_deleted = false \
             ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(entity_id)
        .fetch_optional(&mut *db)
        .await?;
        if let Some((a, c)) = row {
            if let Some(owner) = a.or(c) {
                return Ok(Some(owner));
            }
        }
    }
    _owner_of_activity(db, "contact", entity_id).await
}

// ─── Rules ───────────────────────────────────────────────────────────────────

/// `Conversation`.
#[derive(Debug, Clone)]
pub struct Conversation {
    pub entity_type: String,
    pub entity_id: Uuid,
    pub last_outbound: DateTime<Utc>,
}

/// `_silent_conversations(db, days)`: records whose newest message is ours and older than `days`.
/// (Python joins the newest moment back to ANY activity of the record at that moment, whatever its type.)
async fn _silent_conversations(db: &mut PgConnection, days: i64) -> Result<Vec<Conversation>> {
    let cutoff = _now() - TimeDelta::days(days);
    let rows: Vec<(String, Uuid, DateTime<Utc>)> = sqlx::query_as(
        "SELECT activities.entity_type, activities.entity_id, anon_1.last_at \
         FROM activities JOIN (SELECT activities.entity_type AS entity_type, activities.entity_id AS entity_id, \
            max(coalesce(activities.completed_at, activities.scheduled_at, activities.created_at)) AS last_at \
            FROM activities WHERE activities.activity_type IN ('whatsapp', 'email') \
            AND activities.entity_type IN ('contact', 'lead', 'deal') \
            GROUP BY activities.entity_type, activities.entity_id) AS anon_1 \
         ON anon_1.entity_type = activities.entity_type AND anon_1.entity_id = activities.entity_id \
            AND anon_1.last_at = coalesce(activities.completed_at, activities.scheduled_at, activities.created_at) \
         WHERE activities.direction = 'outbound' AND anon_1.last_at <= $1",
    )
    .bind(cutoff)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(entity_type, entity_id, last_outbound)| Conversation {
            entity_type,
            entity_id,
            last_outbound,
        })
        .collect())
}

/// `rule_no_reply(db, cfg)`.
pub async fn rule_no_reply(db: &mut PgConnection, cfg: &FollowupSettings) -> Result<i64> {
    let mut made = 0;
    let cap = cfg.max_per_rule_pass;
    let tz = clock::local_tz(db).await?;
    for conv in _silent_conversations(db, cfg.no_reply_days).await? {
        if made >= cap {
            break;
        }
        let Some(owner) = owner_for(db, &conv.entity_type, conv.entity_id).await? else {
            continue;
        };
        let name = _label(db, &conv.entity_type, conv.entity_id).await?;
        let days = days_between(Utc::now(), conv.last_outbound);
        let _ = tz; // local(tz) - local(tz, x): the zone cancels out of the difference
        let mut new = NewReminder::new(owner, &conv.entity_type, &format!("No reply from {name}"));
        new.entity_id = Some(conv.entity_id);
        new.source = "no_reply".into();
        new.note = Some(format!(
            "We wrote {days} day(s) ago and there has been no answer since."
        ));
        new.due_at = Some(_now());
        new.rule_key = Some(format!(
            "no_reply:{}:{}:{}",
            conv.entity_type,
            conv.entity_id,
            conv.last_outbound.date_naive()
        ));
        if create(db, new).await?.is_some() {
            made += 1;
        }
    }
    Ok(made)
}

/// `rule_stale_lead(db, cfg)`: an open lead nobody has touched: no logged activity, no WhatsApp message
/// and no email on it.
pub async fn rule_stale_lead(db: &mut PgConnection, cfg: &FollowupSettings) -> Result<i64> {
    let cutoff = _now() - TimeDelta::days(cfg.lead_idle_days);
    let statuses: Vec<String> = ACTIVE_LEAD_STATUSES.iter().map(|s| s.to_string()).collect();
    let rows: Vec<(Uuid, Option<String>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT leads.id, leads.title, anon_1.last_at FROM leads LEFT OUTER JOIN \
           (SELECT anon_2.lead_id AS entity_id, max(anon_2.at) AS last_at FROM \
              (SELECT activities.entity_id AS lead_id, \
                      coalesce(activities.completed_at, activities.scheduled_at, activities.created_at) AS at \
                 FROM activities WHERE activities.entity_type = 'lead' \
               UNION ALL SELECT wa_messages.lead_id AS lead_id, wa_messages.wa_ts AS at \
                 FROM wa_messages WHERE wa_messages.lead_id IS NOT NULL \
               UNION ALL SELECT crm_email_threads.linked_lead_id AS lead_id, crm_email_threads.last_message_at AS at \
                 FROM crm_email_threads WHERE crm_email_threads.linked_lead_id IS NOT NULL) AS anon_2 \
            GROUP BY anon_2.lead_id) AS anon_1 ON anon_1.entity_id = leads.id \
         WHERE leads.is_deleted = false AND leads.status = ANY($1) \
           AND (anon_1.last_at IS NULL OR anon_1.last_at <= $2) AND leads.created_at <= $2",
    )
    .bind(&statuses)
    .bind(cutoff)
    .fetch_all(&mut *db)
    .await?;
    let mut made = 0;
    let cap = cfg.max_per_rule_pass;
    for (lead_id, title, last_at) in rows {
        if made >= cap {
            break;
        }
        let Some(owner) = owner_for(db, "lead", lead_id).await? else {
            continue;
        };
        // A lead never touched keeps one key (not today's date): one reminder, not a new copy every day.
        let key = match last_at {
            Some(t) => format!("stale_lead:{lead_id}:{}", t.date_naive()),
            None => format!("stale_lead:{lead_id}"),
        };
        let title = match title.as_deref() {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => "untitled".to_string(),
        };
        let mut new = NewReminder::new(owner, "lead", &format!("Lead has gone quiet: {title}"));
        new.entity_id = Some(lead_id);
        new.source = "stale_lead".into();
        new.note = Some(match last_at {
            Some(t) => format!("No activity since {}.", t.date_naive()),
            None => "Nothing has happened on it yet.".to_string(),
        });
        new.due_at = Some(_now());
        new.rule_key = Some(key);
        if create(db, new).await?.is_some() {
            made += 1;
        }
    }
    Ok(made)
}

/// `rule_stale_deal(db, cfg)`: an open deal that has sat in its stage too long (since it entered the
/// stage, from the last deal_stage_events row into it; else since it was created).
pub async fn rule_stale_deal(db: &mut PgConnection, cfg: &FollowupSettings) -> Result<i64> {
    let cutoff = _now() - TimeDelta::days(cfg.deal_stage_idle_days);
    let stages: Vec<String> = ACTIVE_STAGES.iter().map(|s| s.to_string()).collect();
    let rows: Vec<(Uuid, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT deals.id, deals.title, deals.stage, coalesce(anon_1.at, deals.created_at) AS coalesce_1 \
         FROM deals LEFT OUTER JOIN (SELECT deal_stage_events.deal_id AS deal_id, max(deal_stage_events.at) AS at \
            FROM deal_stage_events JOIN deals ON deals.id = deal_stage_events.deal_id \
            WHERE deal_stage_events.to_stage = deals.stage GROUP BY deal_stage_events.deal_id) AS anon_1 \
         ON anon_1.deal_id = deals.id \
         WHERE deals.is_deleted = false AND deals.stage = ANY($1) AND coalesce(anon_1.at, deals.created_at) <= $2",
    )
    .bind(&stages)
    .bind(cutoff)
    .fetch_all(&mut *db)
    .await?;
    let mut made = 0;
    let cap = cfg.max_per_rule_pass;
    for (deal_id, title, stage, entered_at) in rows {
        if made >= cap {
            break;
        }
        let Some(owner) = owner_for(db, "deal", deal_id).await? else {
            continue;
        };
        let mut new = NewReminder::new(
            owner,
            "deal",
            &format!("Deal stuck in {}: {title}", stage.replace('_', " ")),
        );
        new.entity_id = Some(deal_id);
        new.source = "stale_deal".into();
        new.note = Some(format!("In this stage since {}.", entered_at.date_naive()));
        new.due_at = Some(_now());
        new.rule_key = Some(format!(
            "stale_deal:{deal_id}:{stage}:{}",
            entered_at.date_naive()
        ));
        if create(db, new).await?.is_some() {
            made += 1;
        }
    }
    Ok(made)
}

#[derive(sqlx::FromRow)]
struct DueActivity {
    id: Uuid,
    entity_type: String,
    entity_id: Uuid,
    activity_type: String,
    subject: Option<String>,
    next_action: Option<String>,
    next_action_due: Option<NaiveDate>,
    created_by_id: Option<Uuid>,
    assigned_to_id: Option<Uuid>,
}

/// `rule_next_action(db, cfg)`: an activity's next action fell due and it isn't done.
pub async fn rule_next_action(db: &mut PgConnection, cfg: &FollowupSettings) -> Result<i64> {
    let today = clock::today_local(db).await?;
    let rows: Vec<DueActivity> = sqlx::query_as(
        "SELECT id, entity_type, entity_id, activity_type, subject, next_action, next_action_due, \
                created_by_id, assigned_to_id \
         FROM activities WHERE next_action_due IS NOT NULL AND next_action_due <= $1 \
           AND is_completed IS NOT true AND entity_type IN ('lead', 'deal', 'contact') \
         ORDER BY next_action_due",
    )
    .bind(today)
    .fetch_all(&mut *db)
    .await?;
    let mut made = 0;
    let cap = cfg.max_per_rule_pass;
    let mut alive_stages: Vec<String> = ACTIVE_STAGES.iter().map(|s| s.to_string()).collect();
    alive_stages.push("nurture".to_string());
    for act in rows {
        if made >= cap {
            break;
        }
        let alive = match act.entity_type.as_str() {
            "lead" => sqlx::query_as::<_, (Uuid,)>(
                "SELECT id FROM leads WHERE id = $1 AND is_deleted = false \
                 AND status NOT IN ('disqualified', 'promoted')",
            )
            .bind(act.entity_id)
            .fetch_optional(&mut *db)
            .await?
            .is_some(),
            "deal" => sqlx::query_as::<_, (Uuid,)>(
                "SELECT id FROM deals WHERE id = $1 AND is_deleted = false AND stage = ANY($2)",
            )
            .bind(act.entity_id)
            .bind(&alive_stages)
            .fetch_optional(&mut *db)
            .await?
            .is_some(),
            _ => true,
        };
        if !alive {
            continue;
        }
        let owner = match act.assigned_to_id.or(act.created_by_id) {
            Some(o) => Some(o),
            None => owner_for(db, &act.entity_type, act.entity_id).await?,
        };
        let Some(owner) = owner else { continue };
        let what = [act.next_action.as_deref(), act.subject.as_deref()]
            .into_iter()
            .flatten()
            .find(|s| !s.is_empty())
            .unwrap_or(&act.activity_type)
            .to_string();
        let due = act
            .next_action_due
            .map(|d| d.to_string())
            .unwrap_or_else(|| "None".into());
        let label = _label(db, &act.entity_type, act.entity_id).await?;
        let mut new =
            NewReminder::new(owner, &act.entity_type, &format!("Next action due: {what}"));
        new.entity_id = Some(act.entity_id);
        new.source = "next_action".into();
        new.note = Some(format!("Planned for {due} on {label}."));
        new.due_at = Some(_now());
        new.rule_key = Some(format!("next_action:{}:{due}", act.id));
        if create(db, new).await?.is_some() {
            made += 1;
        }
    }
    Ok(made)
}

/// `rule_quotation(db, cfg)`: a sent quotation with no answer (still inside its validity).
pub async fn rule_quotation(db: &mut PgConnection, cfg: &FollowupSettings) -> Result<i64> {
    let tz = clock::local_tz(db).await?;
    let cutoff = _now() - TimeDelta::days(cfg.quotation_followup_days);
    let rows: Vec<(Uuid, Option<String>, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, full_number, number, updated_at FROM quotations \
         WHERE is_deleted = false AND status = 'sent' AND updated_at <= $1 AND valid_until >= $2",
    )
    .bind(cutoff)
    .bind(local(tz, None).date_naive())
    .fetch_all(&mut *db)
    .await?;
    let mut made = 0;
    let cap = cfg.max_per_rule_pass;
    for (quote_id, full_number, number, updated_at) in rows {
        if made >= cap {
            break;
        }
        let Some(owner) = owner_for(db, "quotation", quote_id).await? else {
            continue;
        };
        let shown = match full_number {
            Some(f) if !f.is_empty() => f,
            _ => number,
        };
        let mut new = NewReminder::new(
            owner,
            "quotation",
            &format!("No answer on quotation {shown}"),
        );
        new.entity_id = Some(quote_id);
        new.source = "quotation".into();
        new.note = Some(format!(
            "Sent {} day(s) ago and still not accepted.",
            days_between(Utc::now(), updated_at)
        ));
        new.due_at = Some(_now());
        new.rule_key = Some(format!("quotation:{quote_id}"));
        if create(db, new).await?.is_some() {
            made += 1;
        }
    }
    Ok(made)
}

/// `RULES` keys, in order.
pub const RULES: [&str; 5] = [
    "rule_no_reply",
    "rule_stale_lead",
    "rule_stale_deal",
    "rule_quotation",
    "rule_next_action",
];

async fn run_rule(db: &mut PgConnection, key: &str, cfg: &FollowupSettings) -> Result<i64> {
    match key {
        "rule_no_reply" => rule_no_reply(db, cfg).await,
        "rule_stale_lead" => rule_stale_lead(db, cfg).await,
        "rule_stale_deal" => rule_stale_deal(db, cfg).await,
        "rule_quotation" => rule_quotation(db, cfg).await,
        _ => rule_next_action(db, cfg).await,
    }
}

/// `generate(db)`: one pass of every enabled rule ({rule name without "rule_": made}, in RULES order).
/// A rule that fails is logged and rolled back (with everything not yet committed, as in Python).
pub async fn generate(db: &mut PgConnection) -> Result<Vec<(String, i64)>> {
    let cfg = settings(db).await?;
    let mut made = Vec::new();
    for key in RULES {
        if !cfg.rule_enabled(key) {
            continue;
        }
        match run_rule(db, key, &cfg).await {
            Ok(n) => made.push((key.trim_start_matches("rule_").to_string(), n)),
            Err(e) => {
                tracing::warn!(rule = key, error = %e, "followup_rule_failed");
                py_rollback(db).await?;
            }
        }
    }
    py_commit(db).await?;
    Ok(made)
}

/// `_label(db, entity_type, entity_id)` (db.get: soft-deleted records still have their name).
/// A record's name for a card or a note (a contact, lead, deal or organization).
pub async fn label_of(db: &mut PgConnection, entity_type: &str, entity_id: Uuid) -> Result<String> {
    _label(db, entity_type, entity_id).await
}

async fn _label(db: &mut PgConnection, entity_type: &str, entity_id: Uuid) -> Result<String> {
    if entity_type == "contact" {
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT first_name, last_name FROM contacts WHERE id = $1")
                .bind(entity_id)
                .fetch_optional(&mut *db)
                .await?;
        return Ok(match row {
            Some((first, last)) => {
                pystr::strip(&format!("{first} {}", last.unwrap_or_default())).to_string()
            }
            None => "this contact".to_string(),
        });
    }
    if entity_type == "lead" {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT title FROM leads WHERE id = $1")
                .bind(entity_id)
                .fetch_optional(&mut *db)
                .await?;
        return Ok(match row {
            Some((Some(t),)) if !t.is_empty() => t,
            _ => "this lead".to_string(),
        });
    }
    let row: Option<(String,)> = sqlx::query_as("SELECT title FROM deals WHERE id = $1")
        .bind(entity_id)
        .fetch_optional(&mut *db)
        .await?;
    Ok(row.map(|r| r.0).unwrap_or_else(|| "this deal".to_string()))
}

/// `close_for(db, entity_type, entity_id, status=...)` (Python's default status is "cancelled"): close the
/// open reminders on a record that has ended. The caller commits.
pub async fn close_for(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
    status: &str,
) -> Result<i64> {
    let res = sqlx::query(
        "UPDATE crm_reminders SET status = $1, completed_at = $2, updated_at = now() \
         WHERE entity_type = $3 AND entity_id = $4 AND status = 'open'",
    )
    .bind(status)
    .bind(_now())
    .bind(entity_type)
    .bind(entity_id)
    .execute(&mut *db)
    .await?;
    Ok(res.rows_affected() as i64)
}

// ─── Firing ──────────────────────────────────────────────────────────────────

/// `fire_due` result: {"due", "escalated", "quiet"}.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct FireResult {
    pub due: i64,
    pub escalated: i64,
    pub quiet: i64,
}

/// `fire_due(db, now)`: deliver due reminders (bell + one WhatsApp digest per owner), and email the ones
/// still open `escalate_after_hours` later.
pub async fn fire_due(db: &mut PgConnection, now: Option<DateTime<Utc>>) -> Result<FireResult> {
    use crate::approvals::notify;
    let now = now.unwrap_or_else(_now);
    let cfg = settings(db).await?;
    let tz = clock::local_tz(db).await?;
    if !in_quiet_hours(&cfg, tz, Some(now)) {
        return Ok(FireResult {
            due: 0,
            escalated: 0,
            quiet: 1,
        });
    }

    let due: Vec<CrmReminder> = sqlx::query_as(
        "SELECT * FROM crm_reminders WHERE status = 'open' AND due_at <= $1 AND notified_at IS NULL \
         ORDER BY due_at LIMIT 200",
    )
    .bind(now)
    .fetch_all(&mut *db)
    .await?;
    // The stale query runs before the session flushes (autoflush is off): it never sees these.
    let stale: Vec<CrmReminder> = sqlx::query_as(
        "SELECT * FROM crm_reminders WHERE status = 'open' AND escalated_at IS NULL \
         AND notified_at IS NOT NULL AND notified_at <= $1 LIMIT 200",
    )
    .bind(now - TimeDelta::hours(cfg.escalate_after_hours))
    .fetch_all(&mut *db)
    .await?;

    let mut by_owner: Vec<(Uuid, Vec<CrmReminder>)> = Vec::new();
    for reminder in &due {
        let title = format!("Follow-up: {}", reminder.title);
        let body = match reminder.note.as_deref() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => "This follow-up is due now.".to_string(),
        };
        let link = link_for(
            &reminder.entity_type,
            reminder.entity_id,
            reminder.chat_id.as_deref(),
        );
        let rid = reminder.id.hyphenated().to_string();
        notify::notify(
            db,
            [reminder.owner_user_id],
            notify::Notice::new(&title, &body, &link, "crm_reminder", &rid)
                .email(false)
                .whatsapp(false),
        )
        .await?;
        match by_owner
            .iter_mut()
            .find(|(o, _)| *o == reminder.owner_user_id)
        {
            Some((_, items)) => items.push(reminder.clone()),
            None => by_owner.push((reminder.owner_user_id, vec![reminder.clone()])),
        }
    }
    for (owner, items) in &by_owner {
        let msg = digest_message(items, cfg.digest_max_items);
        notify::push([*owner], &msg.title, &msg.body, &msg.link, false, true);
    }
    if !due.is_empty() {
        let ids: Vec<Uuid> = due.iter().map(|r| r.id).collect();
        sqlx::query(
            "UPDATE crm_reminders SET notified_at = $1, updated_at = now() WHERE id = ANY($2)",
        )
        .bind(now)
        .bind(&ids)
        .execute(&mut *db)
        .await?;
    }

    for reminder in &stale {
        let since = local(tz, Some(reminder.due_at)).format("%d %b %H:%M");
        let title = format!("Still open: {}", reminder.title);
        let body = format!(
            "{}\n\nThis follow-up has been due since {since}.",
            reminder.note.clone().unwrap_or_default()
        );
        let link = link_for(
            &reminder.entity_type,
            reminder.entity_id,
            reminder.chat_id.as_deref(),
        );
        let rid = reminder.id.hyphenated().to_string();
        notify::notify(
            db,
            [reminder.owner_user_id],
            notify::Notice::new(&title, &body, &link, "crm_reminder", &rid)
                .email(true)
                .whatsapp(false)
                .priority("high"),
        )
        .await?;
    }
    if !stale.is_empty() {
        let ids: Vec<Uuid> = stale.iter().map(|r| r.id).collect();
        sqlx::query(
            "UPDATE crm_reminders SET escalated_at = $1, updated_at = now() WHERE id = ANY($2)",
        )
        .bind(now)
        .bind(&ids)
        .execute(&mut *db)
        .await?;
    }

    py_commit(db).await?;
    Ok(FireResult {
        due: due.len() as i64,
        escalated: stale.len() as i64,
        quiet: 0,
    })
}

/// `digest_message(items, cap)`: {"title", "body", "link"}.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DigestMessage {
    pub title: String,
    pub body: String,
    pub link: String,
}

/// `digest_message(items, cap)`: one WhatsApp for the follow-ups that fell due together: numbered, each
/// with its link. `items` must not be empty (Python indexes items[0]).
pub fn digest_message(items: &[CrmReminder], cap: i64) -> DigestMessage {
    let app_url = granary_core::config::settings().APP_URL.clone();
    let base = app_url.trim_end_matches('/');
    let n = items.len() as i64;
    // items[:cap] with Python's slice rules for a negative cap.
    let take = if cap >= 0 {
        cap.min(n)
    } else {
        (n + cap).max(0)
    } as usize;
    let mut lines = Vec::new();
    for (i, r) in items[..take].iter().enumerate() {
        let note = match &r.note {
            Some(note) if !note.is_empty() && items.len() <= 3 => format!(" ({note})"),
            _ => String::new(),
        };
        lines.push(format!("{}. {}{note}", i + 1, r.title));
        lines.push(format!(
            "   {base}{}",
            link_for(&r.entity_type, r.entity_id, r.chat_id.as_deref())
        ));
    }
    if n > cap {
        lines.push(format!("+{} more", n - cap));
    }
    let title = if items.len() == 1 {
        format!("Follow-up due: {}", items[0].title)
    } else {
        format!("Follow-ups due ({})", items.len())
    };
    DigestMessage {
        title,
        body: lines.join("\n"),
        link: "/crm/followups".to_string(),
    }
}

// ─── Weekly digest to the Owner ────────────────────────────────────────────────

/// `weekly_digest(db, now)`: everything overdue across the team, in one message to the Owner.
pub async fn weekly_digest(db: &mut PgConnection, now: Option<DateTime<Utc>>) -> Result<i64> {
    use crate::approvals::{notify, routing};
    let now = now.unwrap_or_else(_now);
    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT crm_reminders.owner_user_id, users.name, count(crm_reminders.id) AS count_1 \
         FROM crm_reminders JOIN users ON users.id = crm_reminders.owner_user_id \
         WHERE crm_reminders.status = 'open' AND crm_reminders.due_at <= $1 \
         GROUP BY crm_reminders.owner_user_id, users.name ORDER BY count(crm_reminders.id) DESC",
    )
    .bind(now)
    .fetch_all(&mut *db)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|(_, name, count)| {
            let name = if name.is_empty() {
                "Unassigned"
            } else {
                name.as_str()
            };
            format!("\u{2022} {name}: {count} overdue")
        })
        .collect();
    let total: i64 = rows.iter().map(|r| r.2).sum();
    let owners = routing::owner_user_ids(db).await?;
    if owners.is_empty() {
        return Ok(0);
    }
    let tz = clock::local_tz(db).await?;
    let title = format!("{total} CRM follow-up(s) overdue");
    let body = format!(
        "Follow-ups past their date, by owner:\n{}",
        lines.join("\n")
    );
    let day = local(tz, Some(now)).date_naive().to_string();
    notify::notify(
        db,
        &owners,
        notify::Notice::new(
            &title,
            &body,
            "/crm/followups?scope=team",
            "crm_reminder_digest",
            &day,
        )
        .email(true)
        .whatsapp(false),
    )
    .await?;
    py_commit(db).await?;
    Ok(total)
}

#[allow(dead_code)]
fn _weekday_unused(d: NaiveDate) -> u32 {
    d.weekday().num_days_from_monday()
}
