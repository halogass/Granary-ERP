//! Port of backend/app/services/nary/report.py.
//!
//! The weekly report and the monthly loss review (design §12; Nary Phase 3 item E).
//!
//! Weekly, with the morning brief on nary.report.weekday: the SOP KPIs against their targets
//! (nary.report.kpi_targets), the pipeline by stage, deals at risk, approval-queue latency, Nary's
//! quality per action (share approved without a meaningful edit) and its promotion candidates, the
//! filtered count and AI cost against the budget. The numbers are computed here; Opus (nary.report)
//! only writes them up, and the plain table goes out when it can't.
//!
//! Monthly, on the first working day: the losses of the month before with Nary's analyses, and what
//! they teach.
//!
//! KPI windows: ratios over the last nary.report.window_days, activity counts over the last week.
//!
//! The collected data is a JSON object (`serde_json::Value`, insertion-ordered like the Python dict);
//! Python floats are JSON numbers rendered with Python's float repr.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, Utc};
use granary_compat::pyjson::{
    date_isoformat, dumps, str_decimal, str_value, DumpOpts, PyDateTime, PyDecimal, PyValue,
};
use granary_compat::text::pystr;
use granary_core::clock::now_local;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{persona, rhythm};

pub const WHERE: &str = "Nary & AI → Nary and you on WhatsApp";

static NOW_OVERRIDE: std::sync::RwLock<Option<DateTime<chrono_tz::Tz>>> =
    std::sync::RwLock::new(None);

/// Tests: `monkeypatch.setattr(report, "now_local", fixed_now)` (None restores the clock).
pub fn set_now_override(now: Option<DateTime<chrono_tz::Tz>>) {
    *NOW_OVERRIDE.write().unwrap() = now;
}

/// `_pct(n, d)`: `round(100.0 * n / d, 1)` or None when d is 0.
pub fn _pct(n: i64, d: i64) -> Option<f64> {
    if d == 0 {
        None
    } else {
        Some(crate::crm::writes::py_round(100.0 * n as f64 / d as f64, 1))
    }
}

fn num(x: Option<f64>) -> Value {
    match x {
        Some(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        None => Value::Null,
    }
}

fn py_str(v: &Value) -> String {
    str_value(&PyValue::from(v))
}

/// `kpis(db, now)`.
pub async fn kpis(db: &mut PgConnection, now: DateTime<Utc>) -> ApiResult<Value> {
    let window = TimeDelta::days(require_int(db, "nary.report.window_days", WHERE).await?);
    let sla = TimeDelta::minutes(require_int(db, "nary.report.response_sla_minutes", WHERE).await?);
    let since = now - window;
    let week = now - TimeDelta::days(7);
    // Response time: first customer message of a conversation in the last week answered within the SLA.
    let firsts: Vec<(String, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT chat_id, min(wa_ts) FROM wa_messages WHERE from_me = false AND wa_ts >= $1 GROUP BY chat_id",
    )
    .bind(week)
    .fetch_all(&mut *db)
    .await?;
    let mut answered = 0i64;
    for (chat_id, first) in &firsts {
        let first = first.expect("min(wa_ts) over rows with wa_ts >= week");
        let (reply,): (Option<DateTime<Utc>>,) = sqlx::query_as(
            "SELECT min(wa_ts) FROM wa_messages WHERE chat_id = $1 AND from_me = true AND wa_ts >= $2",
        )
        .bind(chat_id)
        .bind(first)
        .fetch_one(&mut *db)
        .await?;
        if matches!(reply, Some(r) if r - first <= sla) {
            answered += 1;
        }
    }
    let leads: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, converted_to_deal_id FROM leads WHERE created_at >= $1 AND is_deleted = false",
    )
    .bind(since)
    .fetch_all(&mut *db)
    .await?;
    let lead_ids: Vec<Uuid> = leads.iter().map(|l| l.0).collect();
    // `lead_id.in_([...] or [None])`: an empty list matches nothing.
    let sql_rows: Vec<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT lead_id FROM nary_scores WHERE lead_id = ANY($1) AND class IN ('hot', 'warm')",
    )
    .bind(&lead_ids)
    .fetch_all(&mut *db)
    .await?;
    let sql_ids: HashSet<Option<Uuid>> = sql_rows.into_iter().map(|r| r.0).collect();
    let converted_sql = leads
        .iter()
        .filter(|l| sql_ids.contains(&Some(l.0)) && l.1.is_some())
        .count() as i64;
    let closed: Vec<(Uuid, String, Option<Value>, Option<String>, Option<NaiveDate>)> = sqlx::query_as(
        "SELECT id, stage, loss_reasons, loss_reason, actual_close_date FROM deals \
         WHERE stage IN ('won', 'handover', 'onboarding', 'lost') AND updated_at >= $1 AND is_deleted = false",
    )
    .bind(since)
    .fetch_all(&mut *db)
    .await?;
    let won: Vec<_> = closed.iter().filter(|d| d.1 != "lost").collect();
    let lost: Vec<_> = closed.iter().filter(|d| d.1 == "lost").collect();
    let closed_ids: Vec<Uuid> = closed.iter().map(|d| d.0).collect();
    let quoted: Vec<(Option<Uuid>, Option<NaiveDate>)> = sqlx::query_as(
        "SELECT deal_id, min(issue_date) FROM quotations WHERE deal_id = ANY($1) \
         AND status IN ('sent', 'accepted', 'signed', 'expired', 'rejected', 'superseded') GROUP BY deal_id",
    )
    .bind(&closed_ids)
    .fetch_all(&mut *db)
    .await?;
    let first_quote: HashMap<Uuid, NaiveDate> = quoted
        .into_iter()
        .filter_map(|(d, day)| Some((d?, day?)))
        .collect();
    let decision_days: Vec<i64> = closed
        .iter()
        .filter_map(|d| {
            let q = first_quote.get(&d.0)?;
            let close = d.4?;
            Some((close - *q).num_days())
        })
        .collect();
    let truthy_reasons = |d: &&(
        Uuid,
        String,
        Option<Value>,
        Option<String>,
        Option<NaiveDate>,
    )| {
        d.2.as_ref()
            .map(crate::whatsapp::py_truthy)
            .unwrap_or(false)
            || d.3.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    };
    let mut out = Map::new();
    out.insert(
        "response_sla_pct".into(),
        num(_pct(answered, firsts.len() as i64)),
    );
    out.insert(
        "lead_to_sql_pct".into(),
        num(_pct(sql_ids.len() as i64, leads.len() as i64)),
    );
    out.insert(
        "sql_to_deal_pct".into(),
        num(_pct(converted_sql, sql_ids.len() as i64)),
    );
    out.insert(
        "quote_to_win_pct".into(),
        num(_pct(
            won.iter()
                .filter(|d| first_quote.contains_key(&d.0))
                .count() as i64,
            first_quote.len() as i64,
        )),
    );
    out.insert(
        "win_rate_pct".into(),
        num(_pct(won.len() as i64, closed.len() as i64)),
    );
    out.insert(
        "quote_to_decision_days".into(),
        num(if decision_days.is_empty() {
            None
        } else {
            Some(crate::crm::writes::py_round(
                decision_days.iter().sum::<i64>() as f64 / decision_days.len() as f64,
                1,
            ))
        }),
    );
    out.insert(
        "loss_reason_pct".into(),
        num(_pct(
            lost.iter().filter(|d| truthy_reasons(d)).count() as i64,
            lost.len() as i64,
        )),
    );
    Ok(Value::Object(out))
}

/// A JSON number as a Python float/int for comparisons.
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.to_string().parse::<f64>().ok(),
        Value::Bool(b) => Some(*b as i64 as f64),
        _ => None,
    }
}

/// `str(Decimal)` of a numeric's text as Postgres prints it (asyncpg's Decimal keeps the same digits).
fn decimal_text(s: &str) -> String {
    match PyDecimal::parse(s) {
        Ok(d) => str_decimal(&d),
        Err(_) => s.to_string(),
    }
}

/// `collect(db, now=None)`.
pub async fn collect(db: &mut PgConnection, now: Option<DateTime<Utc>>) -> ApiResult<Value> {
    let now = now.unwrap_or_else(Utc::now);
    let week = now - TimeDelta::days(7);
    let targets = require_json(db, "nary.report.kpi_targets", WHERE).await?;
    let values = kpis(db, now).await?;
    let mut kpi_rows = vec![];
    // `(targets or {}).items()`
    if crate::whatsapp::py_truthy(&targets) {
        let Some(t) = targets.as_object() else {
            return Err(ApiError::internal(anyhow::anyhow!(
                "'list' object has no attribute 'items'"
            )));
        };
        for (key, target) in t {
            let value = values.get(key).cloned().unwrap_or(Value::Null);
            let lower_is_better = key.ends_with("_days");
            let ok = if value.is_null() {
                Value::Null
            } else {
                let (Some(v), Some(tg)) = (as_f64(&value), as_f64(target)) else {
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "'<=' not supported between instances of 'float' and '{}'",
                        match target {
                            Value::String(_) => "str",
                            Value::Null => "NoneType",
                            Value::Array(_) => "list",
                            _ => "dict",
                        }
                    )));
                };
                json!(if lower_is_better { v <= tg } else { v >= tg })
            };
            kpi_rows.push(json!({"kpi": key, "value": value, "target": target, "ok": ok}));
        }
    }
    let active: Vec<String> = crate::crm::stages::ACTIVE_STAGES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let stages: Vec<(String, Option<String>, i64, String)> = sqlx::query_as(
        "SELECT stage, currency, count(id), coalesce(sum(value), 0)::text FROM deals \
         WHERE stage = ANY($1) AND is_deleted = false GROUP BY stage, currency",
    )
    .bind(&active)
    .fetch_all(&mut *db)
    .await?;
    let at_risk: Vec<(String, String)> = sqlx::query_as(
        "SELECT title, stage FROM deals WHERE is_deleted = false AND stage = ANY($1) \
         AND (custom ->> 'at_risk') = 'true' LIMIT 20",
    )
    .bind(&active)
    .fetch_all(&mut *db)
    .await?;
    let decided: Vec<(String, String, Option<rust_decimal::Decimal>, Option<DateTime<Utc>>, DateTime<Utc>)> =
        sqlx::query_as(
            "SELECT action, status, edit_ratio, decided_at, created_at FROM agent_pending_actions WHERE decided_at >= $1",
        )
        .bind(week)
        .fetch_all(&mut *db)
        .await?;
    let latency: Vec<f64> = decided
        .iter()
        .filter_map(|c| {
            let d = c.3?;
            let us = (d - c.4).num_microseconds()?;
            Some(us as f64 / 1e6 / 3600.0)
        })
        .collect();
    // insertion-ordered like the Python dict
    let mut quality: Vec<(String, i64, i64, i64)> = vec![];
    for c in &decided {
        let idx = match quality.iter().position(|q| q.0 == c.0) {
            Some(i) => i,
            None => {
                quality.push((c.0.clone(), 0, 0, 0));
                quality.len() - 1
            }
        };
        let q = &mut quality[idx];
        q.1 += 1;
        if c.1 == "rejected" {
            q.3 += 1;
        } else if c.2.map(|r| r.is_zero()).unwrap_or(true) {
            q.2 += 1;
        }
    }
    let mut quality_obj = Map::new();
    for (action, decided_n, unedited, rejected) in &quality {
        quality_obj.insert(
            action.clone(),
            json!({"decided": decided_n, "unedited": unedited, "rejected": rejected,
                   "unedited_pct": num(_pct(*unedited, *decided_n))}),
        );
    }
    let (filtered,): (i64,) = sqlx::query_as(
        "SELECT count(id) FROM nary_triage_verdicts WHERE created_at >= $1 \
         AND verdict IN ('vendor', 'job_seeker', 'spam', 'scam', 'wrong_number')",
    )
    .bind(week)
    .fetch_one(&mut *db)
    .await?;
    let budget = super::briefs::deps::budget(db).await?;
    let candidates = deps::candidates(db).await?;
    let mut d = Map::new();
    d.insert(
        "week_ending".into(),
        json!(date_isoformat(now.date_naive())),
    );
    d.insert("kpis".into(), Value::Array(kpi_rows));
    d.insert(
        "pipeline".into(),
        Value::Array(
            stages
                .into_iter()
                .map(|(s, c, n, v)| json!({"stage": s, "currency": c, "deals": n, "value": decimal_text(&v)}))
                .collect(),
        ),
    );
    d.insert(
        "at_risk".into(),
        Value::Array(
            at_risk
                .into_iter()
                .map(|(t, s)| json!({"title": t, "stage": s}))
                .collect(),
        ),
    );
    d.insert(
        "approval_latency_hours".into(),
        num(if latency.is_empty() {
            None
        } else {
            Some(crate::crm::writes::py_round(
                latency.iter().sum::<f64>() / latency.len() as f64,
                1,
            ))
        }),
    );
    d.insert("quality".into(), Value::Object(quality_obj));
    d.insert("promotion_candidates".into(), candidates);
    d.insert("filtered".into(), json!(filtered));
    d.insert("budget".into(), budget.unwrap_or(Value::Null));
    Ok(Value::Object(d))
}

fn g<'a>(v: &'a Value, key: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    v.get(key).unwrap_or(&NULL)
}

fn list<'a>(v: &'a Value, key: &str) -> Vec<&'a Value> {
    match v.get(key) {
        Some(Value::Array(a)) => a.iter().collect(),
        _ => vec![],
    }
}

/// `render_plain(data)`.
pub fn render_plain(data: &Value) -> String {
    let mut lines = vec![
        format!(
            "Weekly report, week ending {}",
            py_str(g(data, "week_ending"))
        ),
        String::new(),
        "SOP KPIs:".to_string(),
    ];
    for k in list(data, "kpis") {
        let ok = g(k, "ok");
        let mark = if ok.is_null() {
            ""
        } else if crate::whatsapp::py_truthy(ok) {
            " ok"
        } else {
            " BELOW TARGET"
        };
        let value = g(k, "value");
        let value = if value.is_null() {
            "n/a".to_string()
        } else {
            py_str(value)
        };
        lines.push(format!(
            "- {}: {value} (target {}){mark}",
            py_str(g(k, "kpi")).replace('_', " "),
            py_str(g(k, "target"))
        ));
    }
    let pipeline = list(data, "pipeline");
    if !pipeline.is_empty() {
        lines.push(String::new());
        lines.push("Pipeline:".into());
        for p in pipeline {
            lines.push(format!(
                "- {}: {} deal(s), {} {}",
                py_str(g(p, "stage")),
                py_str(g(p, "deals")),
                py_str(g(p, "currency")),
                py_str(g(p, "value"))
            ));
        }
    }
    let at_risk = list(data, "at_risk");
    if !at_risk.is_empty() {
        lines.push(String::new());
        lines.push("At risk:".into());
        for d in at_risk {
            lines.push(format!(
                "- {} ({})",
                py_str(g(d, "title")),
                py_str(g(d, "stage"))
            ));
        }
    }
    if !g(data, "approval_latency_hours").is_null() {
        lines.push(format!(
            "\nCards waited {} h on average for a decision.",
            py_str(g(data, "approval_latency_hours"))
        ));
    }
    if let Some(Value::Object(q)) = data.get("quality") {
        if !q.is_empty() {
            lines.push(String::new());
            lines.push("Nary quality (approved without a meaningful edit):".into());
            let mut keys: Vec<&String> = q.keys().collect();
            keys.sort_by(|a, b| a.chars().cmp(b.chars()));
            for a in keys {
                let x = &q[a];
                lines.push(format!(
                    "- {a}: {}% of {}",
                    py_str(g(x, "unedited_pct")),
                    py_str(g(x, "decided"))
                ));
            }
        }
    }
    let cands = list(data, "promotion_candidates");
    if !cands.is_empty() {
        lines.push(format!(
            "Ready to be promoted: {}",
            cands
                .iter()
                .map(|c| c.get("action").map(py_str).unwrap_or_else(|| "?".into()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.push(format!(
        "\nFiltered conversations this week: {}.",
        py_str(g(data, "filtered"))
    ));
    if let Some(b) = data.get("budget").filter(|b| crate::whatsapp::py_truthy(b)) {
        lines.push(format!(
            "AI spend this month: ${} of ${}.",
            py_str(g(b, "spent")),
            py_str(g(b, "cap"))
        ));
    }
    lines.join("\n")
}

/// The user message of the report jobs.
pub fn write_up_user(data: &Value, plain: &str) -> ApiResult<String> {
    let facts = dumps(&PyValue::from(data), &DumpOpts::unicode_default_str())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
    Ok(format!("FACTS (JSON):\n{facts}\n\nPLAIN VERSION:\n{plain}"))
}

pub const WEEKLY_INSTRUCTION: &str =
    "Write the Owner's weekly sales report from the facts. Lead with what is below target and \
what needs a decision. Keep every number as given. Short sections, no preamble.";
pub const LOSS_INSTRUCTION: &str =
    "Write the monthly loss review: the patterns across the losses, \
what to change, then the list. Short.";

async fn _write_up(
    db: &mut PgConnection,
    job: &str,
    data: &Value,
    plain: &str,
    instruction: &str,
) -> ApiResult<String> {
    let attempt: ApiResult<Option<String>> = async {
        let system = persona::system_prompt("staff", instruction, "", "");
        let user = write_up_user(data, plain)?;
        let out = super::briefs::deps::chat(db, job, &system, &user).await?;
        if !out.refused && !pystr::strip(&out.text).is_empty() {
            return Ok(Some(pystr::strip(&out.text).to_string()));
        }
        Ok(None)
    }
    .await;
    match attempt {
        Ok(Some(t)) => return Ok(t),
        Ok(None) => {}
        Err(ApiError::Http { detail, .. }) => {
            tracing::info!(reason = %py_str(&detail), "nary_report_plain")
        }
        // the report must arrive
        Err(e) => tracing::warn!(error = %e, "nary_report_model_failed"),
    }
    Ok(plain.to_string())
}

/// `send_weekly(db)`.
pub async fn send_weekly(db: &mut PgConnection) -> ApiResult<Value> {
    use crate::approvals::notify::{notify, Notice};
    let data = collect(db, None).await?;
    let plain = render_plain(&data);
    let text = _write_up(db, "nary.report", &data, &plain, WEEKLY_INSTRUCTION).await?;
    let week_ending = py_str(g(&data, "week_ending"));
    let mut run = deps::start_run(db, &week_ending).await?;
    deps::add_note_step(db, &mut run, &text).await?;
    deps::finish_run(db, &mut run).await?;
    let approvers = super::briefs::deps::approver_ids(db).await?;
    let title = format!("Weekly report ({week_ending})");
    notify(
        db,
        approvers,
        Notice::new(&title, &text, "/nary/report", "nary_report", &week_ending)
            .type_("nary_brief")
            .email(true)
            .whatsapp(false),
    )
    .await?;
    rhythm::commit(db).await?;
    Ok(data)
}

/// The loss review's plain text.
pub fn loss_plain(data: &Value) -> String {
    let losses = list(data, "losses");
    let body: Vec<String> = losses
        .iter()
        .map(|x| {
            let reasons: Vec<String> = match g(x, "reasons") {
                Value::Array(a) => a.iter().map(py_str).collect(),
                _ => vec![],
            };
            let joined = reasons.join(", ");
            let lesson = g(x, "lesson");
            format!(
                "- {}: {}. {}",
                py_str(g(x, "deal")),
                if joined.is_empty() {
                    "no reason".to_string()
                } else {
                    joined
                },
                if crate::whatsapp::py_truthy(lesson) {
                    py_str(lesson)
                } else {
                    String::new()
                }
            )
        })
        .collect();
    format!(
        "Loss review {}: {} lost deal(s).\n{}",
        py_str(g(data, "month")),
        losses.len(),
        body.join("\n")
    )
}

/// `(month_start - timedelta(days=1)).replace(day=1)`: wall arithmetic, fold 0.
pub fn prev_month_start(month_start: &PyDateTime) -> PyDateTime {
    let prev_wall = (month_start.local - TimeDelta::days(1))
        .with_day(1)
        .unwrap();
    match month_start.tz {
        Some(granary_compat::pyjson::PyTz::Zone(z))
        | Some(granary_compat::pyjson::PyTz::ZoneFold1(z)) => PyDateTime::in_zone(prev_wall, z, 0),
        tz => PyDateTime {
            local: prev_wall,
            tz,
        },
    }
}

/// `loss_review(db, month_start)`: the losses of the month before `month_start` (company-time midnight of
/// the 1st), with Nary's analyses.
pub async fn loss_review(db: &mut PgConnection, month_start: PyDateTime) -> ApiResult<Value> {
    use crate::approvals::notify::{notify, Notice};
    let prev_start = prev_month_start(&month_start);
    let rows: Vec<(Value, String)> = sqlx::query_as(
        "SELECT nary_insights.content, deals.title FROM nary_insights JOIN deals ON deals.id = nary_insights.deal_id \
         WHERE nary_insights.kind = 'loss' AND nary_insights.created_at >= $1 AND nary_insights.created_at < $2",
    )
    .bind(prev_start.to_utc_naive().and_utc())
    .bind(month_start.to_utc_naive().and_utc())
    .fetch_all(&mut *db)
    .await?;
    let month = prev_start.local.format("%Y-%m").to_string();
    let losses: Vec<Value> = rows
        .iter()
        .map(|(content, title)| {
            json!({"deal": title, "reasons": g(content, "reasons"), "analysis": g(content, "analysis"),
                   "lesson": g(content, "lesson")})
        })
        .collect();
    let data = json!({"month": month, "losses": losses});
    let plain = loss_plain(&data);
    let text = if rows.is_empty() {
        plain.clone()
    } else {
        _write_up(db, "nary.report", &data, &plain, LOSS_INSTRUCTION).await?
    };
    let approvers = super::briefs::deps::approver_ids(db).await?;
    let title = format!("Loss review {month}");
    let entity_id = format!("loss:{month}");
    notify(
        db,
        approvers,
        Notice::new(&title, &text, "/nary/report", "nary_report", &entity_id)
            .type_("nary_brief")
            .email(true)
            .whatsapp(false),
    )
    .await?;
    rhythm::commit(db).await?;
    Ok(data)
}

/// `maybe_send(db)`: called by the rhythm tick after the morning brief: the weekly report on its weekday,
/// the loss review on the first working day of the month (once each).
pub async fn maybe_send(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    let fixed = *NOW_OVERRIDE.read().unwrap();
    let now = match fixed {
        Some(n) => n,
        None => now_local(db).await?,
    };
    let slots = rhythm::brief_slots(db).await?;
    let morning = slots.iter().find(|(s, _)| s == "morning").map(|(_, t)| *t);
    let iso = now.weekday().number_from_monday() as i64;
    let Some(morning) = morning else {
        return Ok(vec![]);
    };
    if now.time() < morning || !rhythm::working_hours(db).await?.days.contains(&iso) {
        return Ok(vec![]);
    }
    let mut sent = vec![];
    let weekday = require_int(db, "nary.report.weekday", WHERE).await?;
    let iw = now.iso_week();
    let week_key = format!("{}-W{}", iw.year(), iw.week());
    if iso == weekday
        && rhythm::_stamp(db, "nary.last_report.weekly")
            .await?
            .as_deref()
            != Some(week_key.as_str())
    {
        rhythm::_mark(db, "nary.last_report.weekly", &week_key).await?;
        rhythm::commit(db).await?;
        send_weekly(db).await?;
        sent.push("weekly".to_string());
    }
    let month_key = now.format("%Y-%m").to_string();
    if rhythm::_stamp(db, "nary.last_report.loss")
        .await?
        .as_deref()
        != Some(month_key.as_str())
    {
        rhythm::_mark(db, "nary.last_report.loss", &month_key).await?;
        rhythm::commit(db).await?;
        // `now.replace(day=1, hour=0, ...)` keeps now's tzinfo and fold.
        let pynow = PyDateTime::from_utc_in_zone(now.naive_utc(), now.timezone());
        let start = PyDateTime {
            local: pynow
                .local
                .date()
                .with_day(1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            tz: pynow.tz,
        };
        loss_review(db, start).await?;
        sent.push("loss_review".to_string());
    }
    Ok(sent)
}

/// Calls into autonomy and runs.
pub mod deps {
    use super::*;
    use granary_db::rows::nary::AgentRun;

    pub async fn candidates(db: &mut PgConnection) -> ApiResult<Value> {
        Ok(Value::Array(super::super::autonomy::candidates(db).await?))
    }
    /// `runs.start_run(db, trigger_type="brief", job="nary.report", trigger_ref=week_ending)`.
    pub async fn start_run(db: &mut PgConnection, week_ending: &str) -> ApiResult<AgentRun> {
        super::super::runs::start_run(
            db,
            "brief",
            "nary.report",
            Some(week_ending),
            "internal",
            None,
        )
        .await
    }
    /// `runs.add_step(db, run, "note", "weekly report", output={"text": text})`.
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
            Some("weekly report"),
            super::super::runs::StepOpts {
                output: Some(&output),
                ..Default::default()
            },
        )
        .await?;
        Ok(())
    }
    /// `runs.finish_run(db, run, "done", outcome="weekly report sent")`.
    pub async fn finish_run(db: &mut PgConnection, run: &mut AgentRun) -> ApiResult<()> {
        super::super::runs::finish_run(db, run, "done", Some("weekly report sent"), None).await
    }
}
