//! Port of backend/app/services/nary/costing.py.
//!
//! The internal cost breakdown of a proposal (Nary Phase 4 item C). Never in a customer document or a
//! customer context: the sheet is its own table, the render never reads it, and its keys are on the
//! context wall's forbidden list.
//!
//! ```text
//!     estimate()   Sonnet (nary.cost_estimate) breaks the scope into workstreams × roles × man-days (low,
//!                  likely, high) and non-labour items. Code prices them: labour from the rate card (cost per
//!                  man-day, functional currency), hardware from the catalog's cost price (else "to confirm"), managed
//!                  months from the catalog, travel as a manual line; then contingency, and the price at the
//!                  target margin and at the floor for the right category.
//!     The Owner sets the final prices; they become the manual lines of the quotation.
//! ```
//! All money is Decimal; costs are in the functional currency (the rate card's currency).

use chrono::NaiveDate;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_decimal;
use granary_db::rows::documents::{NaryCostSheet, NaryRateCard, SalesDocument};
use granary_db::rows::nary::AgentRun;
use granary_db::rows::sales::ProductCatalog;
use once_cell::sync::Lazy;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::margins::{self, cent, d_value, div_q, s, Band};

pub const WHERE: &str = "Sales & CRM → Cost breakdown & rate card";
pub const POINTS: [&str; 3] = ["low", "likely", "high"];

/// The model's output schema (key order as in Python, for a byte-identical request).
pub static SCHEMA: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(r#"{"type": "object", "properties": {"workstreams": {"type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}, "lines": {"type": "array", "items": {"type": "object", "properties": {"role": {"type": "string"}, "seniority": {"type": "string", "enum": ["junior", "mid", "senior"]}, "low": {"type": "number"}, "likely": {"type": "number"}, "high": {"type": "number"}}, "required": ["role", "seniority", "low", "likely", "high"], "additionalProperties": false}}}, "required": ["name", "lines"], "additionalProperties": false}}, "non_labour": {"type": "array", "items": {"type": "object", "properties": {"item": {"type": "string"}, "kind": {"type": "string", "enum": ["hardware", "fabrication", "managed_months", "data", "travel", "other"]}, "quantity": {"type": "number"}, "sku": {"type": ["string", "null"]}}, "required": ["item", "kind", "quantity", "sku"], "additionalProperties": false}}, "category": {"type": "string"}, "assumptions": {"type": "array", "items": {"type": "string"}}}, "required": ["workstreams", "non_labour", "category", "assumptions"], "additionalProperties": false}"#).unwrap()
});

pub const ESTIMATE_RULES: &str =
    "Estimate the delivery effort of this proposal for the company's internal cost \
sheet. Break it into workstreams; each line is a role and seniority from the \
rate card list, with man-days as low, likely and high. List non-labour items \
(hardware with its catalog SKU when one fits, fabrication, managed server \
months, data plans, travel). category: the margin category key that fits the \
services best. Be realistic for a small local team; no padding, the \
contingency is added separately.";

/// The `dict[(role, seniority), NaryRateCard]` `rates()` returns, in first-insertion order.
#[derive(Debug, Clone, Default)]
pub struct RateTable(pub Vec<NaryRateCard>);

impl RateTable {
    pub fn get(&self, role: &str, seniority: &str) -> Option<&NaryRateCard> {
        // The model sometimes copies the seniority into the role ("Project Manager (mid)") or changes the
        // case: compare the bare role name, trimmed and case-insensitive.
        let want_role = bare_role(role);
        let want_sen = seniority.trim().to_lowercase();
        self.0.iter().find(|r| {
            bare_role(&r.role) == want_role && r.seniority.trim().to_lowercase() == want_sen
        })
    }
    fn put(&mut self, r: NaryRateCard) {
        match self
            .0
            .iter_mut()
            .find(|x| x.role == r.role && x.seniority == r.seniority)
        {
            Some(x) => *x = r,
            None => self.0.push(r),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn http(status: u16, detail: &str) -> ApiError {
    ApiError::http(status, detail.to_string())
}

fn internal(msg: &str) -> ApiError {
    ApiError::internal(anyhow::anyhow!(msg.to_string()))
}

/// A role name without a trailing "(junior|mid|senior)", trimmed and lower-cased.
fn bare_role(role: &str) -> String {
    let mut r = role.trim().to_lowercase();
    for sen in ["junior", "mid", "senior"] {
        if let Some(rest) = r.strip_suffix(&format!("({sen})")) {
            r = rest.trim_end().to_string();
            break;
        }
    }
    r
}

/// `rates(db, on=None)`: the active rate per (role, seniority): the latest valid_from on or before `on`.
pub async fn rates(db: &mut PgConnection, on: Option<NaiveDate>) -> ApiResult<RateTable> {
    let rows: Vec<NaryRateCard> = match on {
        Some(d) => sqlx::query_as(
            "SELECT * FROM nary_rate_card WHERE status = 'active' AND valid_from <= $1 ORDER BY valid_from",
        )
        .bind(d)
        .fetch_all(&mut *db)
        .await?,
        None => sqlx::query_as("SELECT * FROM nary_rate_card WHERE status = 'active' ORDER BY valid_from")
            .fetch_all(&mut *db)
            .await?,
    };
    let mut out = RateTable::default();
    for r in rows {
        out.put(r);
    }
    if out.is_empty() {
        return Err(http(
            400,
            "The rate card is empty (Settings → Sales & CRM → Cost breakdown & rate card)",
        ));
    }
    Ok(out)
}

/// `str(x)` for a value interpolated in an f-string (strings bare, the rest as Python prints them).
fn py_str(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        other => granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(other)),
    }
}

/// `d[key]` on a dict: KeyError (500) when it is missing or `d` isn't a dict.
fn key<'a>(d: &'a Value, k: &str) -> ApiResult<&'a Value> {
    d.as_object()
        .and_then(|o| o.get(k))
        .ok_or_else(|| internal(&format!("KeyError: '{k}'")))
}

/// `d.get(k) or []` as a list (a non-list truthy value is iterated by Python; only lists are expected).
fn list_or_empty<'a>(d: &'a Value, k: &str) -> ApiResult<Vec<&'a Value>> {
    match d.get(k) {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => Ok(a.iter().collect()),
        Some(Value::Bool(false)) => Ok(vec![]),
        Some(Value::String(t)) if t.is_empty() => Ok(vec![]),
        Some(Value::Object(o)) if o.is_empty() => Ok(vec![]),
        Some(_) => Err(internal(&format!("TypeError: '{k}' is not a list"))),
    }
}

/// `s[:n]` on a Python str (code points).
pub fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `labour(workstreams, table)`: cost of every line at low / likely / high; a role missing from the rate
/// card is flagged.
pub fn labour(workstreams: &[Value], table: &RateTable) -> ApiResult<Value> {
    let mut out_ws = Vec::new();
    let mut missing: Vec<Value> = Vec::new();
    let mut totals = [Decimal::ZERO; 3];
    for ws in workstreams {
        let mut lines = Vec::new();
        let mut ws_tot = [Decimal::ZERO; 3];
        let ws_lines = if ws.is_object() {
            list_or_empty(ws, "lines")?
        } else {
            return Err(internal("AttributeError: get"));
        };
        for ln in ws_lines {
            let role = key(ln, "role")?;
            let seniority = key(ln, "seniority")?;
            let rate = match (role.as_str(), seniority.as_str()) {
                (Some(r), Some(sn)) => table.get(r, sn),
                _ => None,
            };
            let mut line: Map<String, Value> = ln.as_object().cloned().unwrap_or_default();
            let Some(rate) = rate else {
                missing.push(Value::String(format!(
                    "{} ({})",
                    py_str(role),
                    py_str(seniority)
                )));
                line.insert("day_rate".into(), Value::Null);
                for p in POINTS {
                    line.insert(format!("cost_{p}"), Value::Null);
                }
                lines.push(Value::Object(line));
                continue;
            };
            let mut cost = [Decimal::ZERO; 3];
            for (i, p) in POINTS.iter().enumerate() {
                cost[i] = cent(d_value(key(ln, p)?)? * rate.cost_base);
            }
            for i in 0..3 {
                ws_tot[i] += cost[i];
            }
            line.insert("day_rate".into(), Value::String(s(rate.cost_base)));
            for (i, p) in POINTS.iter().enumerate() {
                line.insert(format!("cost_{p}"), Value::String(s(cost[i])));
            }
            lines.push(Value::Object(line));
        }
        for i in 0..3 {
            totals[i] += ws_tot[i];
        }
        let mut w = Map::new();
        w.insert("name".into(), key(ws, "name")?.clone());
        w.insert("lines".into(), Value::Array(lines));
        for (i, p) in POINTS.iter().enumerate() {
            w.insert(format!("total_{p}"), Value::String(s(ws_tot[i])));
        }
        out_ws.push(Value::Object(w));
    }
    let mut t = Map::new();
    for (i, p) in POINTS.iter().enumerate() {
        t.insert(p.to_string(), Value::String(s(totals[i])));
    }
    Ok(json!({"workstreams": out_ws, "totals": t, "missing_rates": missing}))
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(t)) => !t.is_empty(),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Bool(true)) => true,
    }
}

/// `non_labour(db, items)`: hardware and managed months from the catalog's cost price (functional currency); the rest is
/// "to confirm".
pub async fn non_labour(db: &mut PgConnection, items: &[Value]) -> ApiResult<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut total = Decimal::ZERO;
    for it in items {
        let o = it
            .as_object()
            .ok_or_else(|| internal("AttributeError: get"))?;
        let mut product: Option<ProductCatalog> = None;
        if truthy(o.get("sku")) {
            // scalar_one_or_none: more than one row is MultipleResultsFound (500).
            let rows: Vec<ProductCatalog> =
                sqlx::query_as("SELECT * FROM product_catalog WHERE sku = $1 LIMIT 2")
                    .bind(o.get("sku").and_then(|v| v.as_str()).unwrap_or_default())
                    .fetch_all(&mut *db)
                    .await?;
            if rows.len() > 1 {
                return Err(internal("MultipleResultsFound"));
            }
            product = rows.into_iter().next();
        }
        let unit = product.as_ref().and_then(|p| p.cost_price);
        let cost = match unit {
            Some(u) => Some(cent(u * d_value(key(it, "quantity")?)?)),
            None => None,
        };
        if let Some(c) = cost {
            total += c;
        }
        let mut row = o.clone();
        row.insert(
            "unit_cost_base".into(),
            unit.map(|u| Value::String(s(u))).unwrap_or(Value::Null),
        );
        row.insert(
            "cost_base".into(),
            cost.map(|c| Value::String(s(c))).unwrap_or(Value::Null),
        );
        row.insert(
            "source".into(),
            Value::String(match (&product, unit) {
                (Some(p), Some(_)) => format!("catalog cost {}", p.sku),
                _ => "to confirm".into(),
            }),
        );
        row.insert(
            "family".into(),
            product
                .as_ref()
                .and_then(|p| p.family.clone())
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        out.push(Value::Object(row));
    }
    let mut to_confirm = Vec::new();
    for i in &out {
        if i["cost_base"].is_null() {
            to_confirm.push(key(i, "item")?.clone());
        }
    }
    Ok(json!({"items": out, "total": s(total), "to_confirm": to_confirm}))
}

/// `price_lines(cost, band)`: price at the target margin and at the floor.
pub fn price_lines(cost: Decimal, band: Option<&Band>) -> ApiResult<Value> {
    let Some(band) = band else {
        return Ok(json!({"band": null, "price_at_target": null, "price_at_floor": null}));
    };
    Ok(
        json!({"band": band.label, "target_pct": s(band.target), "floor_pct": s(band.floor),
              "price_at_target": s(margins::price_at(cost, band.target)?),
              "price_at_floor": s(margins::price_at(cost, band.floor)?)}),
    )
}

/// `compute(db, estimate)`: the whole sheet from the model's estimate: labour, non-labour, contingency,
/// totals, prices.
pub async fn compute(db: &mut PgConnection, estimate: &Value) -> ApiResult<Value> {
    let contingency = require_decimal(db, "proposal.cost.contingency_pct", WHERE).await?;
    let ws: Vec<Value> = list_or_empty(estimate, "workstreams")?
        .into_iter()
        .cloned()
        .collect();
    let table = rates(db, None).await?;
    let lab = labour(&ws, &table)?;
    let nl_items: Vec<Value> = list_or_empty(estimate, "non_labour")?
        .into_iter()
        .cloned()
        .collect();
    let nl = non_labour(db, &nl_items).await?;
    let bands = margins::table(db).await?;
    // `bands.get(estimate.get("category")) or bands.get("project_services")`
    let services_band = estimate
        .get("category")
        .and_then(|c| c.as_str())
        .and_then(|c| bands.get(c))
        .or_else(|| bands.get("project_services"))
        .cloned();
    let nl_total = d_value(&nl["total"])?;
    let mut summary = Map::new();
    for p in POINTS {
        let base = d_value(&lab["totals"][p])? + nl_total;
        let cont = div_q(base * contingency, Decimal::ONE_HUNDRED, 2)?;
        let total = base + cont;
        let mut row = Map::new();
        row.insert("labour".into(), lab["totals"][p].clone());
        row.insert("non_labour".into(), nl["total"].clone());
        row.insert("contingency".into(), Value::String(s(cont)));
        row.insert("cost".into(), Value::String(s(total)));
        if let Value::Object(pl) = price_lines(total, services_band.as_ref())? {
            row.extend(pl);
        }
        summary.insert(p.to_string(), Value::Object(row));
    }
    let assumptions = if truthy(estimate.get("assumptions")) {
        estimate["assumptions"].clone()
    } else {
        json!([])
    };
    Ok(
        json!({"labour": lab, "non_labour": nl, "contingency_pct": s(contingency), "summary": summary,
              "category": services_band.as_ref().map(|b| b.key.clone()), "assumptions": assumptions,
              "currency": "USD"}),
    )
}

/// `estimate(db, doc, scope, *, run=None)`: ask Sonnet for the breakdown, price it in code, store the sheet
/// (internal). The sheet is flushed (inserted), not committed.
pub async fn estimate(
    db: &mut PgConnection,
    doc: &SalesDocument,
    scope: &str,
    run: Option<&mut AgentRun>,
) -> ApiResult<NaryCostSheet> {
    let table = rates(db, None).await?;
    // "Role: junior, mid" per role, so the model keeps the role name and the seniority apart (a list of
    // "Role (mid)" labels was copied whole into the role field and matched no rate).
    let mut by_role: Vec<(String, Vec<String>)> = Vec::new();
    for r in &table.0 {
        match by_role.iter_mut().find(|(name, _)| *name == r.role) {
            Some((_, sens)) if !sens.contains(&r.seniority) => sens.push(r.seniority.clone()),
            Some(_) => {}
            None => by_role.push((r.role.clone(), vec![r.seniority.clone()])),
        }
    }
    by_role.sort();
    let roles: Vec<String> = by_role
        .iter()
        .map(|(name, sens)| format!("{name}: {}", sens.join(", ")))
        .collect();
    let bands = margins::table(db).await?;
    let content = format!(
        "RATE CARD ROLES (role name: seniorities; put the name alone in role):\n{}\nMARGIN CATEGORIES: {}\n\nSCOPE:\n{}",
        roles.join("\n"),
        bands.keys().join(", "),
        head(scope, 30000)
    );
    let system = super::persona::system_prompt("staff", ESTIMATE_RULES, "", "");
    let run_id = run.as_ref().map(|r| r.id);
    let data = super::llm::ask_json(
        db,
        "nary.cost_estimate",
        &system,
        &content,
        &SCHEMA,
        run,
        None,
        None,
    )
    .await?;
    let Some(data) = data else {
        return Err(http(502, "Nary couldn't estimate the cost. Try again."));
    };
    let content = compute(db, &data).await?;
    let model = data
        .get("_model")
        .and_then(|m| m.as_str())
        .map(str::to_string);
    let currency = crate::fx::functional_currency(db).await?;
    let sheet: NaryCostSheet = sqlx::query_as(
        "INSERT INTO nary_cost_sheets (id, document_id, deal_id, content, currency, status, model, run_id) \
         VALUES ($1, $2, $3, $4, $7, 'draft', $5, $6) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(doc.id)
    .bind(doc.deal_id)
    .bind(&content)
    .bind(model)
    .bind(run_id)
    .bind(&currency)
    .fetch_one(&mut *db)
    .await?;
    Ok(sheet)
}

/// `latest(db, doc_id)`: the newest cost sheet of a document.
pub async fn latest(db: &mut PgConnection, doc_id: Uuid) -> ApiResult<Option<NaryCostSheet>> {
    Ok(sqlx::query_as(
        "SELECT * FROM nary_cost_sheets WHERE document_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(doc_id)
    .fetch_optional(&mut *db)
    .await?)
}

/// `floor_flags(sheet, final_prices)`: the Owner's final prices against the floor: a warning per price under it.
pub fn floor_flags(sheet: &Value, final_prices: &[(String, String)]) -> ApiResult<Vec<String>> {
    let mut out = Vec::new();
    let likely = sheet.get("summary").and_then(|x| x.get("likely"));
    let floor = match likely {
        Some(l) if truthy(Some(l)) => l.get("price_at_floor").filter(|f| !f.is_null()),
        _ => None,
    };
    let mut total = Decimal::ZERO;
    for (_, v) in final_prices {
        total += d_value(&Value::String(v.clone()))?;
    }
    if let Some(floor) = floor {
        let f = d_value(floor)?;
        if total < f {
            out.push(format!(
                "The final price ({}) is under the floor ({}) for the likely cost.",
                s(total),
                py_str(floor)
            ));
        }
    }
    Ok(out)
}
