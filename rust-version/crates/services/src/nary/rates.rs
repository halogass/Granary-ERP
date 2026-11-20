//! Port of backend/app/services/nary/rates.py (agent N6).
//!
//! Rate card research (Nary Phase 4 item C): Nary proposes rate updates, the Owner approves each.
//!
//! The rate card holds the COST of a man-day at the lowest local-market rate. Research uses the
//! self-hosted search (SearXNG) for current salary references; Sonnet (nary.rate_research) reads the
//! results and gives, per role and seniority, the lowest credible gross monthly salary with its source.
//! Code turns that into a man-day cost (monthly × proposal.cost.load_factor ÷ proposal.cost.days_per_month,
//! never below the loaded regional minimum wage in proposal.cost.min_monthly) and proposes a
//! rates.propose_update card where it differs from the active rate by more than
//! proposal.cost.update_threshold_pct. Nothing changes without the Owner.

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_decimal, require_int};
use once_cell::sync::Lazy;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_db::rows::documents::NaryRateCard;

use super::margins::{d_value, div_q, s};

pub const WHERE: &str = "Sales & CRM → Cost breakdown & rate card";

pub static SCHEMA: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(r#"{"type": "object", "properties": {"rates": {"type": "array", "items": {"type": "object", "properties": {"role": {"type": "string"}, "seniority": {"type": "string", "enum": ["junior", "mid", "senior"]}, "monthly_base": {"type": ["number", "null"]}, "source_url": {"type": ["string", "null"]}, "note": {"type": "string"}}, "required": ["role", "seniority", "monthly_base", "source_url", "note"], "additionalProperties": false}}}, "required": ["rates"], "additionalProperties": false}"#).unwrap()
});

pub const RESEARCH_RULES: &str =
    "From the search results, give the LOWEST credible gross monthly salary in \
the company's local market for each role and seniority asked, with the \
URL it comes from. Null when the results don't support a number. Never invent one.";

/// The pure half of `man_day`: `(max(monthly, floor) * load / days / 1000).quantize(Decimal(1)) * 1000`.
/// To the nearest 1,000 currency units (the old `quantize(Decimal("1000"))` has exponent 0: it only rounded
/// to whole units), kept at exponent 0 so str() stays "1235000", not "1.235E+6". Half-even, the context's rounding.
pub fn man_day_from(
    monthly: Decimal,
    load: Decimal,
    days: i64,
    floor: Decimal,
) -> ApiResult<Decimal> {
    // max(D(monthly), floor): the first argument when equal (same value either way).
    let base = if floor > monthly { floor } else { monthly };
    let per_day = super::margins::div(base * load, Decimal::from(days))?;
    Ok(div_q(per_day, Decimal::from(1000), 0)? * Decimal::from(1000))
}

/// `man_day(db, monthly)`.
pub async fn man_day(db: &mut PgConnection, monthly: Decimal) -> ApiResult<Decimal> {
    let load = require_decimal(db, "proposal.cost.load_factor", WHERE).await?;
    let days = require_int(db, "proposal.cost.days_per_month", WHERE).await?;
    let floor = require_decimal(db, "proposal.cost.min_monthly", WHERE).await?;
    man_day_from(monthly, load, days, floor)
}

/// `research(db, user)`: proposes rates.propose_update cards; {"proposed", "read"}.
pub async fn research(db: &mut PgConnection, user: &AuthUser) -> ApiResult<Value> {
    use crate::research::core_::{ready, results_block, search_all, ResearchFail};
    let _ = user;
    // No ORDER BY, as in Python: the rows come in the table's order.
    let current: Vec<NaryRateCard> =
        sqlx::query_as("SELECT * FROM nary_rate_card WHERE status = 'active'")
            .fetch_all(&mut *db)
            .await?;
    if current.is_empty() {
        return Err(ApiError::http(400, "The rate card is empty"));
    }
    let mut roles: Vec<String> = current.iter().map(|r| r.role.clone()).collect();
    roles.sort();
    roles.dedup();
    let year = chrono::Datelike::year(&chrono::Local::now());
    let mut queries: Vec<String> = roles
        .iter()
        .map(|r| format!("{r} monthly salary {year}"))
        .collect();
    queries.extend(
        roles
            .iter()
            .take(4)
            .map(|r| format!("{r} salary {year} junior senior")),
    );
    let unavailable = |e: String| ApiError::http(503, format!("Research isn't available: {e}"));
    let cfg = match ready(db).await {
        Ok(c) => c,
        Err(ResearchFail::Unavailable(e)) => return Err(unavailable(e.0)),
        Err(e) => return Err(ApiError::internal(anyhow::anyhow!(e.to_string()))),
    };
    let results = search_all(&cfg, &queries, 6)
        .await
        .map_err(|e| unavailable(e.0))?;
    let pairs: Vec<Value> = current
        .iter()
        .map(|r| json!([r.role, r.seniority]))
        .collect();
    let content = format!(
        "ROLES: {}\n\nRESULTS:\n{}",
        granary_compat::pyjson::dumps(
            &granary_compat::pyjson::PyValue::from(&Value::Array(pairs)),
            &Default::default()
        )
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))?,
        results_block(&results, 40)
    );
    let system = super::persona::system_prompt("staff", RESEARCH_RULES, "", "");
    let data = super::llm::ask_json(
        db,
        "nary.rate_research",
        &system,
        &content,
        &SCHEMA,
        None,
        None,
        None,
    )
    .await?;
    let Some(data) = data else {
        return Err(ApiError::http(
            502,
            "Nary couldn't read the salary references",
        ));
    };
    let threshold = require_decimal(db, "proposal.cost.update_threshold_pct", WHERE).await?;
    let rates_read: Vec<Value> = key(&data, "rates")?.as_array().cloned().unwrap_or_default();
    let mut proposed = 0;
    for r in &rates_read {
        let role = key(r, "role")?;
        let seniority = key(r, "seniority")?;
        // by_key = {(r.role, r.seniority): r}: the last row of a pair wins.
        let cur = current.iter().rev().find(|c| {
            Some(c.role.as_str()) == role.as_str()
                && Some(c.seniority.as_str()) == seniority.as_str()
        });
        let monthly = r
            .get("monthly_base")
            .filter(|v| super::actions::py_truthy(v));
        let url = r.get("source_url").filter(|v| super::actions::py_truthy(v));
        let (Some(cur), Some(monthly), Some(url)) = (cur, monthly, url) else {
            continue;
        };
        let cost = man_day(db, d_value(monthly)?).await?;
        let change = super::margins::div((cost - cur.cost_base).abs(), cur.cost_base)?
            * Decimal::ONE_HUNDRED;
        if change < threshold {
            continue;
        }
        let source = format!(
            "{} (gross {}/month; {})",
            py_str(url),
            py_str(monthly),
            py_str(key(r, "note")?)
        );
        let currency = crate::fx::functional_currency(db).await?;
        super::replies::_propose(
            db,
            "rates.propose_update",
            json!({"role": role, "seniority": seniority, "cost_base": s(cost),
                   "source": super::py_slice(&source, 1000),
                   "valid_from": chrono::Local::now().date_naive().format("%Y-%m-%d").to_string()}),
            None,
            None,
            Some(&format!(
                "{} ({}): {} → {} {currency} per man-day",
                py_str(role),
                py_str(seniority),
                s(cur.cost_base),
                s(cost)
            )),
            None,
        )
        .await?;
        proposed += 1;
    }
    Ok(json!({"proposed": proposed, "read": rates_read.len()}))
}

fn py_str(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        other => granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(other)),
    }
}

fn key<'a>(p: &'a Value, k: &str) -> ApiResult<&'a Value> {
    p.get(k)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{k}'")))
}

fn text<'a>(p: &'a Value, k: &str) -> ApiResult<&'a str> {
    key(p, k)?
        .as_str()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("TypeError: '{k}' is not a string")))
}

/// Applies rates.propose_update: the new rate, and the previous one retired. Commits.
/// `actor` is `getattr(actor, "id", None)`'s source.
pub async fn apply_update(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let role = text(p, "role")?;
    let seniority = text(p, "seniority")?;
    sqlx::query(
        "UPDATE nary_rate_card SET status = 'retired', updated_at = now() \
         WHERE role = $1 AND seniority = $2 AND status = 'active'",
    )
    .bind(role)
    .bind(seniority)
    .execute(&mut *db)
    .await?;
    let cost = d_value(key(p, "cost_base")?)?;
    let valid_from = granary_core::settings_store::py_date_fromisoformat(text(p, "valid_from")?)
        .ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!(
                "ValueError: Invalid isoformat string: '{}'",
                text(p, "valid_from").unwrap_or_default()
            ))
        })?;
    sqlx::query(
        "INSERT INTO nary_rate_card (id, role, seniority, cost_base, source, valid_from, status, proposed_by_nary, approved_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, 'active', true, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(role)
    .bind(seniority)
    .bind(cost)
    .bind(text(p, "source")?)
    .bind(valid_from)
    .bind(actor.map(|a| a.id))
    .execute(&mut *db)
    .await?;
    super::commit(db).await?;
    let cost_str = match key(p, "cost_base")? {
        Value::String(t) => t.clone(),
        other => s(d_value(other)?),
    };
    // The rate card is kept in the functional currency: the label names it.
    let currency = crate::fx::functional_currency(db).await?;
    Ok(json!({"label": format!("{role} ({seniority}) now {cost_str} {currency} per man-day")}))
}
