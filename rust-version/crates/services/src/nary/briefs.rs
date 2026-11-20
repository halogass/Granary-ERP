//! Port of backend/app/services/nary/briefs.py.
//!
//! The morning brief and the wrap-up (design §9): what is waiting for the Owner, numbered.
//!
//! The facts are collected by code (cards, new leads, due follow-ups, deals at risk, errors, budget);
//! Opus (job nary.brief) only writes them up. When Nary is paused, over budget or the model fails, the
//! plain numbered list goes out instead, so a brief always arrives. Delivered to the bell, the Owner's
//! WhatsApp (their linked staff chat, else the Employee number) and email.
//!
//! The collected facts are a JSON object (`serde_json::Value`, insertion-ordered like the Python dict).

use chrono::{DateTime, NaiveDateTime, NaiveTime, TimeDelta, Utc};
use granary_compat::pyjson::{
    date_isoformat, dumps, str_value, DumpOpts, PyDateTime, PyTz, PyValue,
};
use granary_compat::text::pystr;
use granary_core::clock::{local_tz, now_local};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{owner_channel, persona, rhythm};

pub const WHERE: &str = "Nary & AI → Nary's day";
/// items per section in a brief; the ERP page has the full list
pub const LIST_LIMIT: i64 = 10;

/// `TITLES.get(slot)`.
pub fn title_of(slot: &str) -> Option<&'static str> {
    match slot {
        "morning" => Some("Morning brief"),
        "midday" => Some("Midday queue"),
        "wrapup" => Some("Wrap-up"),
        _ => None,
    }
}

/// `since` as Python holds it: the datetime (for isoformat) and its instant (for the queries; a naive
/// value is taken as UTC, as asyncpg does on a UTC server).
#[derive(Debug, Clone, Copy)]
struct Since {
    py: PyDateTime,
    at: DateTime<Utc>,
}

async fn _since(db: &mut PgConnection) -> ApiResult<Since> {
    let raw: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM system_settings WHERE key = 'nary.last_brief_at'")
            .fetch_optional(&mut *db)
            .await?;
    if let Some((Some(v),)) = raw {
        if !v.is_empty() {
            if let Some((wall, off)) = crate::crm::writes::py_datetime_fromisoformat(&v) {
                let py = match off {
                    Some((0, 0)) => PyDateTime::utc(wall),
                    Some((s, _)) => PyDateTime {
                        local: wall,
                        tz: Some(PyTz::Fixed(s as i32)),
                    },
                    None => PyDateTime::naive(wall),
                };
                let at = match off {
                    Some((s, us)) => wall - TimeDelta::seconds(s) - TimeDelta::microseconds(us),
                    None => wall,
                };
                return Ok(Since {
                    py,
                    at: at.and_utc(),
                });
            }
        }
    }
    let at = Utc::now() - TimeDelta::days(1);
    Ok(Since {
        py: PyDateTime::from_chrono_utc(&at),
        at,
    })
}

fn iso_utc(d: &DateTime<Utc>) -> String {
    PyDateTime::from_chrono_utc(d).isoformat()
}

/// `collect(db)`: everything a brief can mention, as plain data (also what the model is given).
pub async fn collect(db: &mut PgConnection) -> ApiResult<Value> {
    let since = _since(db).await?;
    let now = now_local(db).await?;
    // `now.replace(hour=23, minute=59, second=59)` (microseconds kept, fold kept)
    let tz = now.timezone();
    let pynow = PyDateTime::from_utc_in_zone(now.naive_utc(), tz);
    let eod_wall = pynow
        .local
        .date()
        .and_time(NaiveTime::from_hms_micro_opt(23, 59, 59, pynow.microsecond()).unwrap());
    let end_of_day = PyDateTime {
        local: eod_wall,
        tz: pynow.tz,
    }
    .to_utc_naive()
    .and_utc();

    // C9: every waiting card is read, the ones where a customer asks for a quotation or says it is urgent
    // go first, then the rest oldest first; the first LIST_LIMIT are listed.
    #[allow(clippy::type_complexity)]
    let all_cards: Vec<(Uuid, String, Option<String>, Option<Value>, Value, Option<String>, Option<Uuid>)> =
        sqlx::query_as(
            "SELECT id, action, rationale, diff, payload, entity_type, entity_id FROM agent_pending_actions \
             WHERE status = 'pending' ORDER BY created_at",
        )
        .fetch_all(&mut *db)
        .await?;
    let mut ranked = Vec::with_capacity(all_cards.len());
    for (id, action, rationale, diff, payload, entity_type, entity_id) in all_cards {
        let mut text = vec![rationale.clone().unwrap_or_default()];
        if let Some(l) = diff.as_ref().and_then(|d| d.get("label")) {
            text.push(py_str(l));
        }
        customer_strings(&payload, &mut text);
        let mut who = card_who(&payload);
        if let (Some("lead"), Some(lead_id)) = (entity_type.as_deref(), entity_id) {
            let lead: Option<(Option<String>, Option<String>, Option<String>)> =
                sqlx::query_as("SELECT title, description, raw_message FROM leads WHERE id = $1")
                    .bind(lead_id)
                    .fetch_optional(&mut *db)
                    .await?;
            if let Some((t, d, r)) = lead {
                if who.is_none() {
                    who = t.clone().filter(|x| !x.is_empty());
                }
                text.extend([t, d, r].into_iter().flatten());
            }
        }
        let want = wants(&text.join("\n"));
        ranked.push((want, id, action, rationale, diff, who));
    }
    // a stable sort: the most pressing first, the rest keep their order (oldest first)
    ranked.sort_by_key(|r| std::cmp::Reverse(r.0.rank()));
    ranked.truncate(LIST_LIMIT as usize);
    #[allow(clippy::type_complexity)]
    let leads_all: Vec<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT title, source, status, description, raw_message FROM leads WHERE created_at >= $1 \
             AND is_deleted = false ORDER BY created_at DESC LIMIT $2",
    )
    .bind(since.at)
    .bind(LIST_LIMIT * 5)
    .fetch_all(&mut *db)
    .await?;
    let mut leads_ranked: Vec<(Want, Option<String>, Option<String>, Option<String>)> = leads_all
        .into_iter()
        .map(|(t, src, st, d, r)| {
            let want = wants(
                &[t.clone(), d, r]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            (want, t, src, st)
        })
        .collect();
    leads_ranked.sort_by_key(|r| std::cmp::Reverse(r.0.rank()));
    leads_ranked.truncate(LIST_LIMIT as usize);
    let (leads_total,): (i64,) =
        sqlx::query_as("SELECT count(id) FROM leads WHERE created_at >= $1 AND is_deleted = false")
            .bind(since.at)
            .fetch_one(&mut *db)
            .await?;
    let (cards_total,): (i64,) =
        sqlx::query_as("SELECT count(id) FROM agent_pending_actions WHERE status = 'pending'")
            .fetch_one(&mut *db)
            .await?;
    let approvers = deps::approver_ids(db).await?;
    // `owner_user_id.in_(approvers or [None])`: `IN (NULL)` matches nothing.
    let start_of_day = PyDateTime::in_zone(pynow.local.date().and_hms_opt(0, 0, 0).unwrap(), tz, 0)
        .to_utc_naive()
        .and_utc();
    let reminders: Vec<(String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT title, due_at FROM crm_reminders WHERE status = 'open' AND due_at <= $1 AND due_at >= $4 \
         AND owner_user_id = ANY($2) ORDER BY due_at LIMIT $3",
    )
    .bind(end_of_day)
    .bind(&approvers)
    .bind(LIST_LIMIT)
    .bind(start_of_day)
    .fetch_all(&mut *db)
    .await?;
    // C11: an overdue follow-up is listed once (with "still open since" and done / snooze / drop); after that
    // it is only counted, until its due time changes (a snooze) and it falls due again.
    let shown = overdue_shown(db).await?;
    let overdue_rows: Vec<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, title, due_at FROM crm_reminders WHERE status = 'open' AND due_at < $1 \
         AND owner_user_id = ANY($2) ORDER BY due_at",
    )
    .bind(start_of_day)
    .bind(&approvers)
    .fetch_all(&mut *db)
    .await?;
    let mut overdue_new = vec![];
    let mut overdue_seen = 0i64;
    for (id, title, due) in overdue_rows {
        let stamp = iso_utc(&due);
        if shown.get(&id.to_string()).and_then(|v| v.as_str()) == Some(stamp.as_str()) {
            overdue_seen += 1;
        } else if (overdue_new.len() as i64) < LIST_LIMIT {
            let since = due.with_timezone(&tz).format("%d %b").to_string();
            overdue_new
                .push(json!({"id": id.to_string(), "title": title, "since": since, "due": stamp}));
        } else {
            overdue_seen += 1;
        }
    }
    let today = pynow.local.date();
    let active: Vec<String> = crate::crm::stages::ACTIVE_STAGES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let overdue: Vec<(String, String, chrono::NaiveDate)> = sqlx::query_as(
        "SELECT title, stage, expected_close_date FROM deals WHERE is_deleted = false AND stage = ANY($1) \
         AND expected_close_date IS NOT NULL AND expected_close_date < $2 LIMIT $3",
    )
    .bind(&active)
    .bind(today)
    .bind(LIST_LIMIT)
    .fetch_all(&mut *db)
    .await?;
    // C10: problems with the same title are one line: how often, and the newest description (the title
    // alone, "WhatsApp or Nary needs attention", says nothing). One notification per approver counts once.
    // R&D 26 Oct: a figure problem of a proposal no longer a draft or in review (rejected, sent...) is left out.
    let errors: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT title, count(DISTINCT COALESCE(entity_id, '') || '@' || date_trunc('minute', created_at)::text), \
         (array_agg(body ORDER BY created_at DESC))[1] FROM notifications n \
         WHERE type = 'nary_error' AND created_at >= $1 \
         AND NOT (split_part(COALESCE(n.entity_id, ''), ':', 1) = 'proposal_figure' \
                  AND NOT EXISTS (SELECT 1 FROM sales_documents d \
                                  WHERE d.id::text = split_part(n.entity_id, ':', 2) \
                                  AND d.status IN ('draft', 'review'))) \
         GROUP BY title ORDER BY max(created_at) DESC LIMIT $2",
    )
    .bind(since.at)
    .bind(LIST_LIMIT)
    .fetch_all(&mut *db)
    .await?;
    let likely: Vec<(String, Value)> = sqlx::query_as(
        "SELECT subject_ref, signals FROM nary_identity_links WHERE tier = 'likely' AND status = 'linked' \
         AND created_at >= $1 ORDER BY created_at DESC LIMIT $2",
    )
    .bind(since.at)
    .bind(LIST_LIMIT)
    .fetch_all(&mut *db)
    .await?;
    let held: Vec<(Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT display_name, phone, chat_id FROM wa_chats WHERE status = 'held' ORDER BY updated_at DESC LIMIT $1",
    )
    .bind(LIST_LIMIT)
    .fetch_all(&mut *db)
    .await?;
    let missed: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT name, company FROM nary_expected_arrivals WHERE status = 'missed' AND notified_at >= $1 LIMIT $2",
    )
    .bind(since.at)
    .bind(LIST_LIMIT)
    .fetch_all(&mut *db)
    .await?;
    let filtered: Vec<String> = FILTERED.iter().map(|s| s.to_string()).collect();
    let (filtered_n,): (i64,) = sqlx::query_as(
        "SELECT count(id) FROM nary_triage_verdicts WHERE verdict = ANY($1) AND created_at >= $2",
    )
    .bind(&filtered)
    .bind(since.at)
    .fetch_one(&mut *db)
    .await?;
    let replies_fyi = super::reply_alerts::for_brief(db).await?;
    let budget = deps::budget(db).await?;
    let paused = deps::is_paused(db).await?;

    let mut card_rows = vec![];
    for (want, id, action, rationale, diff, who) in ranked {
        let label = deps::action_label(&action).unwrap_or_else(|| action.clone());
        // `(c.rationale or (c.diff or {}).get("label") or "")[:160]`
        let what = match rationale.filter(|r| !r.is_empty()) {
            Some(r) => Value::String(r),
            None => match diff
                .as_ref()
                .and_then(|d| d.as_object())
                .and_then(|o| o.get("label"))
            {
                Some(v) if crate::whatsapp::py_truthy(v) => v.clone(),
                _ => Value::String(String::new()),
            },
        };
        let what = match what {
            Value::String(s) => Value::String(s.chars().take(160).collect()),
            Value::Array(a) => Value::Array(a.into_iter().take(160).collect()),
            // Python slices only str/list; anything else raises TypeError.
            other => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "unhashable/unsliceable diff label: {}",
                    str_value(&PyValue::from(&other))
                )))
            }
        };
        match want.phrase(who.as_deref()) {
            // C9: worded as what the customer wants, the card's own label after it
            Some(headline) => {
                let what = rstrip_colon_space(&format!("{label}: {}", py_str(&what))).to_string();
                card_rows.push(
                    json!({"id": id.to_string(), "action": headline, "what": what, "urgent": true}),
                );
            }
            None => card_rows.push(json!({"id": id.to_string(), "action": label, "what": what})),
        }
    }
    let likely_rows: Vec<Value> = likely
        .into_iter()
        .map(|(subject_ref, signals)| {
            let sigs: Vec<Map<String, Value>> = match &signals {
                Value::Array(a) => a.iter().filter_map(|x| x.as_object().cloned()).collect(),
                _ => vec![],
            };
            let is_target =
                |x: &Map<String, Value>| x.get("signal").and_then(|v| v.as_str()) == Some("target");
            let who = sigs
                .iter()
                .find(|x| is_target(x))
                .map(|x| x.get("detail").cloned().unwrap_or(Value::Null));
            let why = sigs
                .iter()
                .filter(|x| !is_target(x))
                .map(|x| {
                    format!(
                        "{}: {}",
                        py_str(x.get("signal").unwrap_or(&Value::Null)),
                        py_str(x.get("detail").unwrap_or(&Value::Null))
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            json!({"chat": subject_ref, "who": who.unwrap_or(Value::Null), "why": why})
        })
        .collect();
    let mut d = Map::new();
    d.insert("since".into(), json!(since.py.isoformat()));
    d.insert("paused".into(), json!(paused));
    d.insert("cards".into(), Value::Array(card_rows));
    d.insert(
        "new_leads".into(),
        Value::Array(
            leads_ranked
                .into_iter()
                .map(|(want, t, s, st)| {
                    let title = t
                        .filter(|x| !x.is_empty())
                        .unwrap_or_else(|| "(untitled)".into());
                    let title = match want.phrase(None) {
                        Some(w) => format!("{title}: {}", pystr::lower(&w)),
                        None => title,
                    };
                    json!({"title": title, "source": s, "status": st})
                })
                .collect(),
        ),
    );
    d.insert("new_leads_total".into(), json!(leads_total));
    d.insert("cards_total".into(), json!(cards_total));
    d.insert(
        "due_followups".into(),
        Value::Array(
            reminders
                .into_iter()
                .map(|(t, due)| json!({"title": t, "due": iso_utc(&due)}))
                .collect(),
        ),
    );
    d.insert(
        "overdue_deals".into(),
        Value::Array(
            overdue
                .into_iter()
                .map(|(t, s, e)| json!({"title": t, "stage": s, "expected_close": date_isoformat(e)}))
                .collect(),
        ),
    );
    d.insert("overdue_followups".into(), Value::Array(overdue_new));
    d.insert("overdue_seen_total".into(), json!(overdue_seen));
    d.insert(
        "errors".into(),
        Value::Array(
            errors
                .into_iter()
                .map(|(t, n, body)| {
                    let detail = body
                        .map(|b| pystr::strip(&b).to_string())
                        .filter(|b| !b.is_empty() && *b != t);
                    json!({"title": t, "count": n, "detail": detail})
                })
                .collect(),
        ),
    );
    d.insert("likely_links".into(), Value::Array(likely_rows));
    d.insert(
        "held_chats".into(),
        Value::Array(
            held.into_iter()
                .map(|(n, p, c)| {
                    json!({"who": n.filter(|x| !x.is_empty()).or(p.filter(|x| !x.is_empty())).unwrap_or(c)})
                })
                .collect(),
        ),
    );
    d.insert(
        "missed_arrivals".into(),
        Value::Array(
            missed
                .into_iter()
                .map(|(n, c)| json!({"name": n, "company": c}))
                .collect(),
        ),
    );
    d.insert(
        "replies".into(),
        Value::Array(
            replies_fyi
                .iter()
                .map(|r| json!({"id": r.id.to_string(), "who": r.who, "summary": r.summary}))
                .collect(),
        ),
    );
    d.insert("filtered_total".into(), json!(filtered_n));
    d.insert("budget".into(), budget.unwrap_or(Value::Null));
    Ok(Value::Object(d))
}

/// triage.FILTERED
const FILTERED: [&str; 5] = ["vendor", "job_seeker", "spam", "scam", "wrong_number"];

/// `str(x)` of a JSON value, as an f-string shows it.
fn py_str(v: &Value) -> String {
    str_value(&PyValue::from(v))
}

fn truthy(v: Option<&Value>) -> bool {
    v.map(crate::whatsapp::py_truthy).unwrap_or(false)
}

/// `item[key]` (a missing key reads as None; collect() always sets them).
fn f<'a>(item: &'a Value, key: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    item.get(key).unwrap_or(&NULL)
}

/// `data.get(key, [])` as a list (a non-list value is iterated as Python would only for lists; others are
/// treated as no items when falsy).
fn items<'a>(data: &'a Value, key: &str) -> Vec<&'a Value> {
    match data.get(key) {
        Some(Value::Array(a)) => a.iter().collect(),
        _ => vec![],
    }
}

/// `s.rstrip(": ")`
fn rstrip_colon_space(s: &str) -> &str {
    s.trim_end_matches([':', ' '])
}

type Fmt = fn(&Value) -> String;

pub const OVERDUE_TITLE: &str = "Overdue follow-ups (reply *done N*, *snooze N* or *drop N*)";

fn sections() -> [(&'static str, &'static str, Fmt); 12] {
    [
        ("Waiting for you", "cards", |c| {
            rstrip_colon_space(&format!(
                "{}: {}",
                py_str(f(c, "action")),
                py_str(f(c, "what"))
            ))
            .to_string()
        }),
        ("New leads", "new_leads", |l| {
            format!("{} ({})", py_str(f(l, "title")), py_str(f(l, "source")))
        }),
        ("Follow-ups due today", "due_followups", |r| {
            py_str(f(r, "title"))
        }),
        (OVERDUE_TITLE, "overdue_followups", |r| {
            format!(
                "{} (still open since {})",
                py_str(f(r, "title")),
                py_str(f(r, "since"))
            )
        }),
        (
            "Deals past their expected close date",
            "overdue_deals",
            |d| {
                format!(
                    "{} ({}, expected {})",
                    py_str(f(d, "title")),
                    py_str(f(d, "stage")),
                    py_str(f(d, "expected_close"))
                )
            },
        ),
        ("Problems", "errors", |e| {
            // C10: one line per title, how often, and what it is about
            let mut line = py_str(f(e, "title"));
            if let Some(n) = e.get("count").and_then(Value::as_i64).filter(|n| *n > 1) {
                line.push_str(&format!(" ({n} times)"));
            }
            if let Some(d) = e.get("detail").filter(|v| crate::whatsapp::py_truthy(v)) {
                line.push_str(&format!(": {}", py_str(d)));
            }
            line
        }),
        (
            "Chats Nary linked to a record (undo on the Nary Intake page if wrong)",
            "likely_links",
            |l| {
                format!(
                    "{} → {} ({})",
                    py_str(f(l, "chat")),
                    py_str(f(l, "who")),
                    py_str(f(l, "why"))
                )
            },
        ),
        (
            "Chats waiting for you to say business or personal",
            "held_chats",
            |c| py_str(f(c, "who")),
        ),
        (
            "Web forms whose WhatsApp chat never came",
            "missed_arrivals",
            |a| {
                let name = f(a, "name");
                let name = if crate::whatsapp::py_truthy(name) {
                    py_str(name)
                } else {
                    "?".into()
                };
                let company = a.get("company");
                if truthy(company) {
                    format!("{name} ({})", py_str(company.unwrap()))
                } else {
                    name
                }
            },
        ),
        ("Replies, nothing needed from us", "replies", |r| {
            format!("{}: {}", py_str(f(r, "who")), py_str(f(r, "summary")))
        }),
        ("Done today", "done_today", |d| py_str(f(d, "what"))),
        ("Tomorrow", "tomorrow", |d| py_str(f(d, "what"))),
    ]
}

/// Python int-like comparison `total > len(items)` for the JSON total (ints only; collect() counts).
fn as_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.to_string().parse::<f64>().ok(),
        Value::Bool(b) => Some(*b as i64 as f64),
        _ => None,
    }
}

/// `render_plain(data, slot, cap=None)`: the brief without a model: numbered and short. With `cap`, each
/// section shows its first `cap` items and says how many more there are (render_rest lists them; the
/// numbers stay the same).
pub fn render_plain(data: &Value, _slot: &str, cap: Option<i64>) -> String {
    let mut lines: Vec<String> = vec![];
    let mut n = 0i64;
    if truthy(data.get("paused")) {
        lines.push("Nary is paused: nothing was done since the last brief.".into());
    }
    for (title, key, fmt) in sections() {
        let list = items(data, key);
        if list.is_empty() {
            continue;
        }
        let total = match title {
            "Waiting for you" => data.get("cards_total"),
            "New leads" => data.get("new_leads_total"),
            _ => None,
        };
        let more = match total {
            Some(t)
                if crate::whatsapp::py_truthy(t)
                    && as_num(t).map(|x| x > list.len() as f64).unwrap_or(false) =>
            {
                format!(" ({} in all, newest first)", py_str(t))
            }
            _ => String::new(),
        };
        lines.push(format!("\n{title}{more}:"));
        for (i, item) in list.iter().enumerate() {
            n += 1;
            if cap.is_none() || (i as i64) < cap.unwrap() {
                lines.push(format!("{n}. {}", fmt(item)));
            }
        }
        if let Some(c) = cap {
            if list.len() as i64 > c {
                lines.push(format!("(+{} more, reply *more*)", list.len() as i64 - c));
            }
        }
    }
    if truthy(data.get("overdue_seen_total")) {
        lines.push(format!(
            "\nOverdue follow-ups listed before and still open: {} (CRM → Follow-ups).",
            py_str(&data["overdue_seen_total"])
        ));
    }
    if truthy(data.get("filtered_total")) {
        lines.push(format!(
            "\nFiltered (spam, vendors, job seekers...): {}, listed on the Nary Intake page.",
            py_str(&data["filtered_total"])
        ));
    }
    if truthy(data.get("budget")) {
        let b = &data["budget"];
        lines.push(format!(
            "\nAI spend this month: ${} of ${}.",
            py_str(f(b, "spent")),
            py_str(f(b, "cap"))
        ));
    }
    if n == 0 {
        lines.push("Nothing is waiting for you.".into());
    }
    pystr::strip(&lines.join("\n")).to_string()
}

/// Python `line[:1].isdigit()`.
fn starts_digit(line: &str) -> bool {
    line.chars().next().map(pystr::is_digit).unwrap_or(false)
}

/// `render_rest(data, slot, cap)`: the items render_plain(cap=…) left out, with their numbers, by section;
/// "" when nothing was left out.
pub fn render_rest(data: &Value, slot: &str, cap: i64) -> String {
    let full_text = render_plain(data, slot, None);
    let short_text = render_plain(data, slot, Some(cap));
    let short: std::collections::HashSet<&str> =
        pystr::splitlines(&short_text).into_iter().collect();
    let mut out: Vec<String> = vec![];
    let mut header: Option<String> = None;
    for line in pystr::splitlines(&full_text) {
        if line.ends_with(':') && !starts_digit(line) {
            header = Some(pystr::strip(line).to_string());
            continue;
        }
        if starts_digit(line) && !short.contains(line) {
            if let Some(h) = header.take() {
                out.push(format!("\n{h}"));
            }
            out.push(line.to_string());
        }
    }
    pystr::strip(&out.join("\n")).to_string()
}

/// The model's write-up: its text, plus what runs.add_step records (the result and the AI client).
pub struct ByModel {
    pub text: String,
    pub step: deps::ModelStep,
}

const SECTION_KEYS: [&str; 11] = [
    "cards",
    "new_leads",
    "due_followups",
    "overdue_followups",
    "overdue_deals",
    "errors",
    "likely_links",
    "held_chats",
    "missed_arrivals",
    "done_today",
    "tomorrow",
];

/// The system prompt of the brief job (`persona.system_prompt("staff", ..., context=f"Write it in {lang}.")`).
pub fn brief_system(slot: &str, lang: &str) -> String {
    persona::system_prompt(
        "staff",
        &format!(
            "Write the Owner's {} from the facts given. Keep every item in the draft's order with its number: \
the draft is already ranked (what customers want most urgently first), so never reorder or renumber. One line per \
item. Keep only the items the draft shows, and its \"(+N more, reply more)\" lines (translated). No greeting, \
no sign-off.",
            title_of(slot).unwrap_or("brief").to_lowercase()
        ),
        "",
        &format!("Write it in {lang}."),
    )
}

/// The user message of the brief job.
pub fn brief_user(data: &Value, plain: &str) -> ApiResult<String> {
    let facts = dumps(&PyValue::from(data), &DumpOpts::unicode())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
    Ok(format!(
        "FACTS (JSON):\n{facts}\n\nDRAFT TO IMPROVE:\n{plain}"
    ))
}

/// `compose(db, data, slot, cap=None)`: (text, the model's result when the model wrote it, else None).
/// With `cap`, the short version.
pub async fn compose(
    db: &mut PgConnection,
    data: &Value,
    slot: &str,
    cap: Option<i64>,
) -> ApiResult<(String, Option<ByModel>)> {
    let plain = render_plain(data, slot, cap);
    if truthy(data.get("paused")) || !SECTION_KEYS.iter().any(|k| truthy(data.get(*k))) {
        return Ok((plain, None));
    }
    let attempt: ApiResult<Option<ByModel>> = async {
        let (user, _) = owner_channel::owner(db).await?;
        let _ = owner_channel::language(db, user.as_ref()).await?;
        let lang = "English";
        let system = brief_system(slot, lang);
        let content = brief_user(data, &plain)?;
        let out = deps::chat(db, "nary.brief", &system, &content).await?;
        if out.refused || pystr::strip(&out.text).is_empty() {
            return Ok(None);
        }
        Ok(Some(ByModel {
            text: pystr::strip(&out.text).to_string(),
            step: out.step,
        }))
    }
    .await;
    match attempt {
        // C12: a write-up whose numbers are out of order (2, 3, 1) would answer "1" with the wrong card
        Ok(Some(m)) if !numbers_in_order(&m.text) => {
            tracing::warn!("nary_brief_model_reordered");
            Ok((plain, None))
        }
        Ok(Some(m)) => Ok((m.text.clone(), Some(m))),
        Ok(None) => Ok((plain, None)),
        Err(ApiError::Http { detail, .. }) => {
            tracing::info!(reason = %py_str(&detail), "nary_brief_plain");
            Ok((plain, None))
        }
        Err(e) => {
            // the brief must arrive even when the model doesn't
            tracing::warn!(error = %e, "nary_brief_model_failed");
            Ok((plain, None))
        }
    }
}

/// C12: the item numbers at the start of the lines ("3. ...", "*3.* ...", "3) ...") rise from line to line.
pub fn numbers_in_order(text: &str) -> bool {
    static NUM: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r"^\s*[*_]?(\d{1,3})[*_]?[.)]").unwrap());
    let mut last = 0i64;
    for line in text.lines() {
        if let Some(c) = NUM.captures(line) {
            let n: i64 = c[1].parse().unwrap_or(0);
            if n <= last {
                return false;
            }
            last = n;
        }
    }
    true
}

/// The numbers render_plain gives the items of one section (hidden ones included), in order.
pub fn numbers_of(data: &Value, section: &str) -> Vec<i64> {
    let mut n = 0i64;
    let mut out = vec![];
    for (_, key, _) in sections() {
        for _ in items(data, key) {
            n += 1;
            if key == section {
                out.push(n);
            }
        }
    }
    out
}

/// What a customer wants, read from a card or a lead (C9).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Want {
    pub quote: bool,
    pub urgent: bool,
}

impl Want {
    /// Higher first: a quotation asked urgently, then urgent, then a quotation, then the rest.
    pub fn rank(&self) -> u8 {
        (self.urgent as u8) * 2 + self.quote as u8
    }

    /// The headline, worded as what the customer wants; None when nothing stands out.
    pub fn phrase(&self, who: Option<&str>) -> Option<String> {
        let who = who.filter(|w| !w.trim().is_empty()).unwrap_or("A customer");
        match (self.quote, self.urgent) {
            (true, true) => Some(format!("{who} wants a quotation, urgently")),
            (true, false) => Some(format!("{who} asks for a quotation")),
            (false, true) => Some(format!("{who} says it is urgent")),
            (false, false) => None,
        }
    }
}

static QUOTE_WORDS: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
    regex::Regex::new(
        r"(?i)\b(quote|quotes|quotation|quotations|rfq|request for quotation|price quote|price list)\b",
    )
    .unwrap()
});
static URGENT_WORDS: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
    regex::Regex::new(
        r"(?i)\b(urgent|urgently|asap|as soon as possible|right away|immediately|emergency)\b",
    )
    .unwrap()
});

/// `wants(text)`: does the text ask for a quotation, or say it is urgent?
pub fn wants(text: &str) -> Want {
    Want {
        quote: QUOTE_WORDS.is_match(text),
        urgent: URGENT_WORDS.is_match(text),
    }
}

/// The customer's words in a card's payload: every text value except the message Nary drafted (`text`),
/// which may itself mention a quotation.
fn customer_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| customer_strings(x, out)),
        Value::Object(m) => {
            for (k, x) in m {
                if k != "text" {
                    customer_strings(x, out);
                }
            }
        }
        _ => {}
    }
}

/// Who a card is about, from its payload (a lead's title, a person's or company's name).
fn card_who(payload: &Value) -> Option<String> {
    let fields = payload.get("fields").unwrap_or(payload);
    for k in ["title", "name", "company", "company_name"] {
        if let Some(v) = fields
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return Some(v.trim().to_string());
        }
    }
    let first = fields
        .get("first_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let last = fields
        .get("last_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let name = format!("{first} {last}").trim().to_string();
    (!name.is_empty()).then_some(name)
}

pub const OVERDUE_SHOWN_KEY: &str = "nary.briefs.overdue_shown";

/// The overdue follow-ups a brief already listed: {reminder id: the due time it was listed with}.
async fn overdue_shown(db: &mut PgConnection) -> ApiResult<Map<String, Value>> {
    let raw = granary_core::settings_store::get_optional(db, OVERDUE_SHOWN_KEY).await?;
    Ok(raw
        .and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default())
}

/// Remember the overdue follow-ups this brief listed (ids no longer open are forgotten).
async fn record_overdue_shown(db: &mut PgConnection, data: &Value) -> ApiResult<()> {
    let listed = items(data, "overdue_followups");
    let mut shown = overdue_shown(db).await?;
    if listed.is_empty() && shown.is_empty() {
        return Ok(());
    }
    for r in listed {
        if let (Some(id), Some(due)) = (r.get("id").and_then(Value::as_str), r.get("due")) {
            shown.insert(id.to_string(), due.clone());
        }
    }
    let ids: Vec<Uuid> = shown
        .keys()
        .filter_map(|k| Uuid::parse_str(k).ok())
        .collect();
    let open: Vec<(Uuid,)> =
        sqlx::query_as("SELECT id FROM crm_reminders WHERE id = ANY($1) AND status = 'open'")
            .bind(&ids)
            .fetch_all(&mut *db)
            .await?;
    let open: std::collections::HashSet<String> =
        open.into_iter().map(|r| r.0.to_string()).collect();
    shown.retain(|k, _| open.contains(k));
    let value = Value::Object(shown).to_string();
    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM system_settings WHERE key = $1")
        .bind(OVERDUE_SHOWN_KEY)
        .fetch_optional(&mut *db)
        .await?;
    match exists {
        Some((id,)) => {
            sqlx::query("UPDATE system_settings SET value = $2, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(&value)
                .execute(&mut *db)
                .await?;
        }
        None => {
            rhythm::insert_state_setting(
                db,
                OVERDUE_SHOWN_KEY,
                &value,
                "Nary: overdue follow-ups the briefs already listed",
            )
            .await?
        }
    }
    Ok(())
}

/// `wrapup_data(db)`: the end-of-day wrap-up's own part: what Nary did today, and tomorrow's plan.
pub async fn wrapup_data(db: &mut PgConnection) -> ApiResult<Value> {
    let tz = local_tz(db).await?;
    let now = PyDateTime::from_utc_in_zone(Utc::now().naive_utc(), tz);
    // `datetime.now(tz).replace(hour=0, ...)` keeps the fold; `+ timedelta` resets it.
    let midnight: NaiveDateTime = now.local.date().and_hms_opt(0, 0, 0).unwrap();
    let start = PyDateTime {
        local: midnight,
        tz: now.tz,
    }
    .to_utc_naive()
    .and_utc();
    let tomorrow = PyDateTime::in_zone(midnight + TimeDelta::days(1), tz, 0)
        .to_utc_naive()
        .and_utc();
    let after = PyDateTime::in_zone(midnight + TimeDelta::days(2), tz, 0)
        .to_utc_naive()
        .and_utc();
    let mut done = vec![];
    let applied: Vec<(String, i64)> = sqlx::query_as(
        "SELECT action, count(id) FROM agent_pending_actions WHERE status = 'applied' AND updated_at >= $1 GROUP BY action",
    )
    .bind(start)
    .fetch_all(&mut *db)
    .await?;
    for (action, n) in applied {
        let label = deps::action_label(&action).unwrap_or_else(|| action.clone());
        done.push(json!({"what": format!("{label}: {n}")}));
    }
    let (sent,): (i64,) = sqlx::query_as(
        "SELECT count(id) FROM wa_messages WHERE sent_by_nary = true AND created_at >= $1",
    )
    .bind(start)
    .fetch_one(&mut *db)
    .await?;
    if sent != 0 {
        done.push(json!({"what": format!("WhatsApp messages Nary sent: {sent}")}));
    }
    let mut plan = vec![];
    let meetings: Vec<(Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
        "SELECT starts_at, title FROM nary_meetings WHERE status = 'booked' AND starts_at >= $1 AND starts_at < $2 \
         ORDER BY starts_at",
    )
    .bind(tomorrow)
    .bind(after)
    .fetch_all(&mut *db)
    .await?;
    for (starts, title) in meetings {
        let hhmm = starts
            .map(|s| s.with_timezone(&tz).format("%H:%M").to_string())
            .unwrap_or_default();
        plan.push(json!({"what": format!("Meeting {hhmm}: {}", title.filter(|t| !t.is_empty()).unwrap_or_else(|| "customer call".into()))}));
    }
    let (steps,): (i64,) = sqlx::query_as(
        "SELECT count(id) FROM nary_cadences WHERE status = 'scheduled' AND due_at >= $1 AND due_at < $2",
    )
    .bind(tomorrow)
    .bind(after)
    .fetch_one(&mut *db)
    .await?;
    if steps != 0 {
        plan.push(json!({"what": format!("Quotation follow-ups due: {steps}")}));
    }
    let approvers = deps::approver_ids(db).await?;
    let reminders: Vec<(String,)> = sqlx::query_as(
        "SELECT title FROM crm_reminders WHERE status = 'open' AND owner_user_id = ANY($1) AND due_at >= $2 \
         AND due_at < $3 ORDER BY due_at LIMIT 5",
    )
    .bind(&approvers)
    .bind(tomorrow)
    .bind(after)
    .fetch_all(&mut *db)
    .await?;
    for (title,) in reminders {
        plan.push(json!({"what": format!("Reminder: {title}")}));
    }
    Ok(json!({"done_today": done, "tomorrow": plan}))
}

/// `send_brief(db, slot)`: returns {"slot", "text", "by_model"}.
pub async fn send_brief(db: &mut PgConnection, slot: &str) -> ApiResult<Value> {
    use crate::approvals::notify::{deliver_outbox_now, notify, Notice};
    let mut data = collect(db).await?;
    if slot == "wrapup" {
        let w = wrapup_data(db).await?;
        if let (Some(d), Value::Object(w)) = (data.as_object_mut(), w) {
            for (k, v) in w {
                d.insert(k, v);
            }
        }
    }
    let mut run = deps::start_run(db, slot).await?;
    let cap = require_int(db, "nary.briefs.items_per_section", WHERE).await?;
    let (text, by_model) = compose(db, &data, slot, Some(cap)).await?;
    let rest = render_rest(&data, slot, cap); // "more" on WhatsApp; the email carries it in full
    match &by_model {
        Some(m) => deps::add_llm_step(db, &mut run, slot, &text, &m.step).await?,
        None => deps::add_note_step(db, &mut run, &text).await?,
    }
    let paused = truthy(data.get("paused"));
    let title = format!(
        "{}{}",
        title_of(slot).unwrap_or("Brief"),
        if paused { " (Nary paused)" } else { "" }
    );
    // "Waiting for you" is numbered first: the Owner can answer "1 2 3" on WhatsApp to apply those cards.
    // The brief always replaces the last prompt, so a number never refers to something older.
    let mut prompt_items = Map::new();
    for (i, c) in self::items(&data, "cards").iter().enumerate() {
        prompt_items.insert((i + 1).to_string(), f(c, "id").clone());
    }
    let mut prompt = json!({"kind": "list", "items": Value::Object(prompt_items)});
    // C11: "done 7", "snooze 7", "drop 7" act on the overdue follow-ups by the numbers shown
    let fu_numbers = numbers_of(&data, "overdue_followups");
    if !fu_numbers.is_empty() {
        let mut fu = Map::new();
        for (n, r) in fu_numbers.iter().zip(items(&data, "overdue_followups")) {
            fu.insert(n.to_string(), f(r, "id").clone());
        }
        prompt["followups"] = Value::Object(fu);
    }
    let on_channel = owner_channel::enabled(db).await?;
    let body = if rest.is_empty() {
        text.clone()
    } else {
        format!("{text}\n\nMore:\n{rest}")
    };
    let entity_id = format!("{slot}:{}", date_isoformat(Utc::now().date_naive()));
    let approvers = deps::approver_ids(db).await?;
    notify(
        db,
        approvers,
        Notice::new(&title, &body, "/nary/approvals", "nary_brief", &entity_id)
            .type_("nary_brief")
            .email(true)
            .whatsapp(!on_channel),
    )
    .await?;
    if on_channel {
        // In the Owner's WhatsApp conversation with Nary, so any line can be asked about. Scheduled: sent now.
        let mut facts = Map::new();
        facts.insert("slot".into(), json!(slot));
        facts.insert(
            "cards".into(),
            data.get("cards").cloned().unwrap_or(Value::Null),
        );
        if !rest.is_empty() {
            facts.insert("more".into(), json!(rest));
        }
        owner_channel::raise_topic(
            db,
            owner_channel::Topic {
                anytime: true,
                numbered: Some(prompt),
                facts: Some(Value::Object(facts)),
                ..owner_channel::Topic::new("brief", &title, &text)
            },
        )
        .await?;
    } else {
        deps::set_prompt(db, prompt).await?;
    }
    let stamp = PyDateTime::from_chrono_utc(&Utc::now()).isoformat();
    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM system_settings WHERE key = 'nary.last_brief_at'")
            .fetch_optional(&mut *db)
            .await?;
    match exists {
        Some((id,)) => {
            sqlx::query("UPDATE system_settings SET value = $2, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(&stamp)
                .execute(&mut *db)
                .await?;
        }
        None => {
            rhythm::insert_state_setting(
                db,
                "nary.last_brief_at",
                &stamp,
                "Nary: when the last brief went out",
            )
            .await?
        }
    }
    record_overdue_shown(db, &data).await?;
    let replies = items(&data, "replies");
    if !replies.is_empty() {
        // each reply is in one brief only
        let mut ids = vec![];
        for r in replies {
            let s = f(r, "id").as_str().unwrap_or("");
            ids.push(Uuid::parse_str(s).map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))?);
        }
        sqlx::query(
            "UPDATE nary_reply_alerts SET briefed_at = $2, updated_at = now() WHERE id = ANY($1)",
        )
        .bind(&ids)
        .bind(Utc::now())
        .execute(&mut *db)
        .await?;
    }
    let outcome = format!(
        "{title} sent ({}).",
        if by_model.is_some() {
            "written by the model"
        } else {
            "plain list"
        }
    );
    deps::finish_run(db, &mut run, &outcome).await?;
    rhythm::commit(db).await?;
    deliver_outbox_now().await;
    if on_channel {
        owner_channel::flush(db).await?;
    }
    Ok(json!({"slot": slot, "text": text, "by_model": by_model.is_some()}))
}

/// Calls into roles, governance, actions, runs, commands and ai.registry, adapted to the shapes used here.
pub mod deps {
    use super::*;
    use crate::ai::base::{AiError, ChatRequest, ChatResult};
    use crate::ai::registry::{budget_state, get_ai_config, use_ai, UseAiOptions};
    use granary_db::rows::nary::AgentRun;

    /// What runs.add_step records for a model-written brief (result, cost, model).
    pub struct ModelStep {
        pub result: ChatResult,
        pub cost: Option<rust_decimal::Decimal>,
        pub model: String,
    }

    pub struct ChatOut {
        pub text: String,
        pub refused: bool,
        pub step: ModelStep,
    }

    /// An AiError as the `except HTTPException` / `except Exception` split sees it.
    pub fn ai_to_api(e: AiError) -> ApiError {
        match e {
            AiError::Ai { status, detail, .. } => ApiError::http(status, detail),
            AiError::Api(a) => a,
            other => ApiError::internal(anyhow::anyhow!("{other}")),
        }
    }

    pub async fn approver_ids(db: &mut PgConnection) -> ApiResult<Vec<Uuid>> {
        super::super::roles::approver_ids(db).await
    }
    pub async fn is_paused(db: &mut PgConnection) -> ApiResult<bool> {
        super::super::governance::is_paused(db).await
    }
    /// `REGISTRY[action].label` when registered.
    pub fn action_label(action: &str) -> Option<String> {
        super::super::actions::get(action)
            .ok()
            .map(|a| a.label.clone())
    }
    /// `{"spent": f"{spent:.2f}", "cap": f"{cap:.2f}", "level"}` from registry.budget_state, None on an
    /// HTTPException.
    pub async fn budget(db: &mut PgConnection) -> ApiResult<Option<Value>> {
        use granary_compat::pyfmt::{format_number, PyNum};
        let state = match get_ai_config(db).await {
            Ok(cfg) => budget_state(db, &cfg).await,
            Err(e) => Err(e),
        };
        match state {
            Ok(st) => {
                let f2 = |d: rust_decimal::Decimal| {
                    format_number(&PyNum::from(d), ".2f")
                        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))
                };
                Ok(Some(
                    json!({"spent": f2(st.spent)?, "cap": f2(st.cap)?, "level": st.level()}),
                ))
            }
            Err(e) if e.is_http_exception() => Ok(None),
            Err(e) => Err(ai_to_api(e)),
        }
    }
    /// `ai = await use_ai(db, job); await ai.chat(system=..., messages=[{"role": "user", "content": ...}])`.
    pub async fn chat(
        db: &mut PgConnection,
        job: &str,
        system: &str,
        user: &str,
    ) -> ApiResult<ChatOut> {
        let ai = use_ai(db, job, UseAiOptions::default())
            .await
            .map_err(ai_to_api)?;
        let result = ai
            .chat(db, ChatRequest::user(system, user))
            .await
            .map_err(ai_to_api)?;
        Ok(ChatOut {
            text: result.text.clone(),
            refused: result.refused(),
            step: ModelStep {
                cost: ai.last_cost(),
                model: ai.model().to_string(),
                result,
            },
        })
    }
    /// `runs.start_run(db, trigger_type="brief", trigger_ref=slot, job="nary.brief")`.
    pub async fn start_run(db: &mut PgConnection, slot: &str) -> ApiResult<AgentRun> {
        super::super::runs::start_run(db, "brief", "nary.brief", Some(slot), "internal", None).await
    }
    /// `runs.add_step(db, run, "llm", "nary.brief", input={"slot": slot}, output={"text": text}, result=...,
    /// cost=ai.last_cost, model=ai.model)`.
    pub async fn add_llm_step(
        db: &mut PgConnection,
        run: &mut AgentRun,
        slot: &str,
        text: &str,
        step: &ModelStep,
    ) -> ApiResult<()> {
        let input = json!({"slot": slot});
        let output = json!({"text": text});
        super::super::runs::add_step(
            db,
            run,
            "llm",
            Some("nary.brief"),
            super::super::runs::StepOpts {
                input: Some(&input),
                output: Some(&output),
                result: Some(&step.result),
                cost: step.cost,
                model: Some(&step.model),
            },
        )
        .await?;
        Ok(())
    }
    /// `runs.add_step(db, run, "note", "plain brief", output={"text": text})`.
    pub async fn add_note_step(
        db: &mut PgConnection,
        run: &mut AgentRun,
        text: &str,
    ) -> ApiResult<()> {
        let output = json!({"text": text});
        super::super::runs::add_step(
            db,
            run,
            "note",
            Some("plain brief"),
            super::super::runs::StepOpts {
                output: Some(&output),
                ..Default::default()
            },
        )
        .await?;
        Ok(())
    }
    /// `runs.finish_run(db, run, "done", outcome=outcome)`.
    pub async fn finish_run(
        db: &mut PgConnection,
        run: &mut AgentRun,
        outcome: &str,
    ) -> ApiResult<()> {
        super::super::runs::finish_run(db, run, "done", Some(outcome), None).await
    }
    pub async fn set_prompt(db: &mut PgConnection, prompt: Value) -> ApiResult<()> {
        super::super::commands::set_prompt(db, &prompt).await
    }
}
