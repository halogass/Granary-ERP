//! The pricing rules of a quotation.
//!
//! ```text
//! BLOCK  a line sold below sales.pricing_rules.min_cost_multiple (2.0) x its catalog cost price, per unit (per
//!        rate period for a recurring item); custom and service lines against the labour of the deal's cost sheet
//! WARN   cost unknown (no cost price, or no exchange rate to the quotation's currency): never a block
//! ```
//! Costs are in the functional currency (like cost_price) and converted at the ERP rate (never 1.0: no rate =
//! cost unknown). The check runs on the card and the editor (warnings) and at approval submit, where a blocking
//! issue refuses the draft (sales.pricing_rules.block_submit) unless the Owner gave a pricing override with a
//! reason for these exact lines (quotations.pricing_override_*, audited). The pure core is `evaluate`.

use std::collections::{HashMap, HashSet};

use chrono::{NaiveDate, Utc};
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, require_decimal, require_json, SettingError};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::pricing::PricedLine;
use crate::quotation_notes::glob;

pub const WHERE: &str = "Sales & CRM \u{2192} Pricing rules";
pub const MIN_COST_KEY: &str = "sales.pricing_rules.min_cost_multiple";
pub const BLOCK_KEY: &str = "sales.pricing_rules.block_submit";
pub const OWNER_SKUS_KEY: &str = "sales.pricing_rules.owner_approval_skus";

pub const OVERRIDE_REFUSED: &str = "Only the Owner can override the pricing rules.";

/// The settings the rules read.
#[derive(Debug, Clone, PartialEq)]
pub struct Rules {
    pub min_cost_multiple: Decimal,
    pub owner_approval_skus: Vec<String>,
}

fn patterns(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A setting's problem as Settings shows it (None = fine). Used by Settings before saving.
pub fn validate_setting(key: &str, value: &str) -> Option<Result<(), String>> {
    match key {
        MIN_COST_KEY => Some(match value.trim().parse::<Decimal>() {
            Ok(d) if d > Decimal::ZERO => Ok(()),
            _ => Err(format!("{key} must be a number above 0")),
        }),
        OWNER_SKUS_KEY => Some(match serde_json::from_str::<Value>(value) {
            Ok(Value::Array(a)) if a.iter().all(Value::is_string) => Ok(()),
            _ => Err(format!(
                "{key} must be a JSON list of SKU patterns, e.g. [\"SVC-RETAINER-*\"]"
            )),
        }),
        _ => None,
    }
}

/// The rules from Settings (a missing setting is a clear error naming the key).
pub async fn load(db: &mut PgConnection) -> ApiResult<Rules> {
    Ok(Rules {
        min_cost_multiple: require_decimal(db, MIN_COST_KEY, WHERE).await?,
        owner_approval_skus: patterns(&require_json(db, OWNER_SKUS_KEY, WHERE).await?),
    })
}

/// Whether a blocking issue refuses the approval submit.
pub async fn blocks_submit(db: &mut PgConnection) -> ApiResult<bool> {
    let raw = get_optional(&mut *db, BLOCK_KEY)
        .await?
        .ok_or_else(|| SettingError::new(BLOCK_KEY, "is not set", WHERE))?;
    Ok(matches!(
        raw.trim().to_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    ))
}

fn matches_any(pats: &[String], sku: &str) -> bool {
    pats.iter().any(|p| glob(p, sku))
}

/// What the rules read of a catalog item.
#[derive(Debug, Clone, Default, PartialEq, sqlx::FromRow)]
pub struct Item {
    pub id: Uuid,
    pub sku: String,
    pub name: String,
    pub product_type: String,
    pub sale_model: String,
    pub pricing_mode: String,
    pub price_status: Option<String>,
    /// per unit (per rate period for a recurring item), functional currency
    pub cost_price: Option<Decimal>,
    pub is_active: Option<bool>,
}

const ITEM_COLS: &str =
    "id, sku, name, product_type, sale_model, pricing_mode, price_status, cost_price, is_active";

/// A quotation line as the rules read it.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleLine {
    pub item: Option<Item>,
    pub name: String,
    pub quantity: Decimal,
    /// the line's own net total, after its line and header discount (quotation currency)
    pub net_amount: Decimal,
    /// net price per unit per rate period (quotation currency)
    pub net_unit: Decimal,
    pub is_optional: bool,
}

/// What the rules need beyond the lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ctx {
    pub currency: String,
    /// functional currency -> quotation currency (None: no rate on record, every cost is unknown)
    pub fx: Option<Decimal>,
    /// the labour of the deal's latest cost sheet (likely), in the quotation currency
    pub labour: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub code: &'static str,
    pub blocking: bool,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Check {
    pub issues: Vec<Issue>,
}

/// 8 Nov 2026: what a person without sales:view_costs sees instead of an issue's message (the messages name the
/// cost floor, a multiple of the cost, and the cost sheet's labour).
pub const HIDDEN_BLOCKING: &str =
    "A line is priced below the pricing rules' floor. The Owner or Finance can show the figures.";
pub const HIDDEN_WARNING: &str =
    "A pricing rules warning. The Owner or Finance can show the details.";
const APPROVAL_LINE: &str = "Pricing check: the approval refuses these prices unless the Owner gives a pricing override with a reason (set_pricing_override).";

/// A card warning line as someone without sales:view_costs may see it: a "Pricing check" issue loses its figures,
/// every other line is unchanged.
pub fn hide_card_line(line: &str) -> String {
    if line == APPROVAL_LINE {
        line.to_string()
    } else if line.starts_with("Pricing check (blocks approval): ") {
        format!("Pricing check (blocks approval): {HIDDEN_BLOCKING}")
    } else if line.starts_with("Pricing check: ") {
        format!("Pricing check: {HIDDEN_WARNING}")
    } else {
        line.to_string()
    }
}

impl Issue {
    /// The message as someone may see it: without sales:view_costs a cost-based issue says only that it is flagged.
    pub fn message_for(&self, see_costs: bool) -> &str {
        if see_costs {
            &self.message
        } else if self.blocking {
            HIDDEN_BLOCKING
        } else {
            HIDDEN_WARNING
        }
    }
}

impl Check {
    pub fn blocking(&self) -> Vec<&Issue> {
        self.issues.iter().filter(|i| i.blocking).collect()
    }
    pub fn warnings(&self) -> Vec<&Issue> {
        self.issues.iter().filter(|i| !i.blocking).collect()
    }
    /// One line per issue, for a card ("Pricing check: ...").
    pub fn card_lines(&self) -> Vec<String> {
        self.card_lines_for(true)
    }

    /// The card lines for someone who may (or may not) see costs: without sales:view_costs no figures.
    pub fn card_lines_for(&self, see_costs: bool) -> Vec<String> {
        let mut out: Vec<String> = self
            .issues
            .iter()
            .map(|i| {
                let msg = i.message_for(see_costs);
                format!(
                    "Pricing check{}: {}",
                    if i.blocking { " (blocks approval)" } else { "" },
                    msg
                )
            })
            .collect();
        if !self.blocking().is_empty() {
            out.push(APPROVAL_LINE.into());
        }
        out
    }
    pub fn json(&self) -> Value {
        json!({
            "blocking": self.blocking().iter().map(|i| json!({"code": i.code, "message": i.message})).collect::<Vec<_>>(),
            "warnings": self.warnings().iter().map(|i| json!({"code": i.code, "message": i.message})).collect::<Vec<_>>(),
        })
    }
}

/// `1234567.5` -> "1,234,567.50".
pub fn money(d: Decimal) -> String {
    let r = d.round_dp(2);
    let neg = r.is_sign_negative() && !r.is_zero();
    let s = format!("{:.2}", r.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, "00"));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{}{grouped}.{frac}", if neg { "-" } else { "" })
}

fn fmt_mult(d: Decimal) -> String {
    d.normalize().to_string()
}

fn label(l: &RuleLine) -> String {
    match &l.item {
        Some(i) => format!("{} ({})", i.sku, l.name),
        None => l.name.clone(),
    }
}

/// The rules over a quotation's lines (pure).
pub fn evaluate(rules: &Rules, lines: &[RuleLine], ctx: &Ctx) -> Check {
    let mut c = Check::default();
    let cur = ctx.currency.as_str();

    let mut unknown: Vec<String> = Vec::new();
    // custom lines and service items without a cost price: their cost is the labour of the cost sheet
    let mut services: Vec<&RuleLine> = Vec::new();
    for l in lines.iter().filter(|l| !l.is_optional) {
        let Some(item) = &l.item else {
            services.push(l);
            continue;
        };
        match item.cost_price {
            None => {
                if item.pricing_mode == "percent_of_parent" {
                    continue; // Care and add-ons follow their parent line
                }
                if item.product_type == "service" {
                    services.push(l);
                    continue;
                }
                unknown.push(item.sku.clone());
            }
            Some(cost) => {
                let Some(fx) = ctx.fx else {
                    unknown.push(item.sku.clone());
                    continue;
                };
                let mult = rules.min_cost_multiple;
                let floor = (mult * cost * fx).round_dp(2);
                if l.net_unit < floor {
                    c.issues.push(Issue {
                        code: "below_min_cost_multiple",
                        blocking: true,
                        message: format!(
                            "{}: {cur} {} per unit is below {}x its cost; at least {cur} {} per unit.",
                            label(l),
                            money(l.net_unit),
                            fmt_mult(mult),
                            money(floor)
                        ),
                    });
                }
            }
        }
    }

    // custom and service lines against the labour of the deal's cost sheet
    let service_sell: Decimal = services.iter().map(|l| l.net_amount).sum();
    match ctx.labour.filter(|x| *x > Decimal::ZERO) {
        Some(labour) => {
            let floor = (rules.min_cost_multiple * labour).round_dp(2);
            if services.is_empty() {
                c.issues.push(Issue {
                    code: "labour_not_priced",
                    blocking: false,
                    message: format!(
                        "The deal's cost sheet has labour of {cur} {} but no service or custom line carries it.",
                        money(labour)
                    ),
                });
            } else if service_sell < floor {
                c.issues.push(Issue {
                    code: "below_min_cost_multiple",
                    blocking: true,
                    message: format!(
                        "The services and custom lines total {cur} {}, below {}x the labour of the deal's cost sheet ({cur} {}); at least {cur} {}.",
                        money(service_sell),
                        fmt_mult(rules.min_cost_multiple),
                        money(labour),
                        money(floor)
                    ),
                });
            }
        }
        None => {
            for l in services.iter().filter(|l| l.net_amount > Decimal::ZERO) {
                unknown.push(match &l.item {
                    Some(i) => i.sku.clone(),
                    None => l.name.clone(),
                });
            }
        }
    }

    unknown.dedup();
    let mut seen_u = HashSet::new();
    unknown.retain(|u| seen_u.insert(u.clone()));
    if !unknown.is_empty() {
        c.issues.push(Issue {
            code: "cost_unknown",
            blocking: false,
            message: format!(
                "Cost unknown for {}: the minimum cost multiple could not be checked (no cost price in the catalog, no cost sheet, or no exchange rate to {cur}).",
                unknown.join(", ")
            ),
        });
    }
    c
}

// ─── loading ─────────────────────────────────────────────────────────────────

/// Catalog items by id.
pub async fn items(db: &mut PgConnection, ids: &[Uuid]) -> ApiResult<HashMap<Uuid, Item>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<Item> = sqlx::query_as(&format!(
        "SELECT {ITEM_COLS} FROM product_catalog WHERE id = ANY($1)"
    ))
    .bind(ids)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows.into_iter().map(|i| (i.id, i)).collect())
}

/// The context: the rate and the cost sheet's labour.
pub async fn context(
    db: &mut PgConnection,
    currency: &str,
    on_date: Option<NaiveDate>,
    deal_id: Option<Uuid>,
) -> ApiResult<Ctx> {
    let functional = crate::fx::functional_currency(db).await?;
    let fx = match crate::fx::get_rate(db, &functional, currency, on_date).await {
        Ok(r) => Some(r),
        Err(crate::fx::RateError::NoRate(_)) => None,
        Err(crate::fx::RateError::Other(e)) => return Err(e),
    };
    let mut labour = None;
    if let Some(deal) = deal_id {
        let content: Option<Value> = sqlx::query_scalar(
            "SELECT content FROM nary_cost_sheets WHERE deal_id = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(deal)
        .fetch_optional(&mut *db)
        .await?;
        if let (Some(content), Some(fx)) = (content, fx) {
            let raw = &content["summary"]["likely"]["labour"];
            let v = match raw {
                Value::String(s) => s.trim().parse::<Decimal>().ok(),
                Value::Number(n) => n.to_string().parse::<Decimal>().ok(),
                _ => None,
            };
            // the cost sheet's own currency (the functional currency when it names none)
            let sheet_cur = content["currency"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| functional.clone());
            if let Some(v) = v {
                let to_functional = if sheet_cur.eq_ignore_ascii_case(&functional) {
                    Some(Decimal::ONE)
                } else {
                    crate::fx::get_rate(db, &sheet_cur, &functional, on_date)
                        .await
                        .ok()
                };
                labour = to_functional.map(|r| (v * r * fx).round_dp(2));
            }
        }
    }
    Ok(Ctx {
        currency: currency.to_uppercase(),
        fx,
        labour,
    })
}

fn parse_uuid(s: Option<&str>) -> Option<Uuid> {
    s.and_then(|s| Uuid::parse_str(s.trim()).ok())
}

/// The check of lines priced by the pricing service (cards, before saving).
pub async fn check_priced(
    db: &mut PgConnection,
    currency: &str,
    on_date: Option<NaiveDate>,
    deal_id: Option<Uuid>,
    priced: &[PricedLine],
) -> ApiResult<Check> {
    let rules = load(db).await?;
    let ids: Vec<Uuid> = priced
        .iter()
        .filter_map(|p| parse_uuid(p.product_id.as_deref()))
        .collect();
    let items = items(db, &ids).await?;
    let lines: Vec<RuleLine> = priced
        .iter()
        .map(|p| {
            let net = p.amount - p.header_discount;
            let per = p.quantity * p.periods;
            RuleLine {
                item: parse_uuid(p.product_id.as_deref()).and_then(|id| items.get(&id).cloned()),
                name: p.name.clone(),
                quantity: p.quantity,
                net_amount: net,
                net_unit: if per.is_zero() {
                    Decimal::ZERO
                } else {
                    (net / per).round_dp(2)
                },
                is_optional: p.is_optional,
            }
        })
        .collect();
    let ctx = context(db, currency, on_date, deal_id).await?;
    Ok(evaluate(&rules, &lines, &ctx))
}

/// A saved quotation's check, with the fingerprint of its lines and the Owner's override.
#[derive(Debug, Clone)]
pub struct Stored {
    pub id: Uuid,
    pub status: String,
    pub check: Check,
    pub fingerprint: String,
    pub override_reason: Option<String>,
    pub override_by: Option<String>,
    pub override_at: Option<chrono::DateTime<Utc>>,
    /// the override was given for these lines (not for an earlier version of them)
    pub override_valid: bool,
}

impl Stored {
    pub fn json(&self) -> Value {
        let mut v = self.check.json();
        v["override"] = match &self.override_reason {
            Some(r) => json!({
                "reason": r, "by": self.override_by,
                "at": self.override_at.map(|t| t.to_rfc3339()),
                "valid": self.override_valid,
            }),
            None => Value::Null,
        };
        v["refused_at_approval"] = json!(
            !self.check.blocking().is_empty()
                && !self.override_valid
                && matches!(self.status.as_str(), "draft" | "rejected" | "")
        );
        v
    }

    /// 8 Nov 2026: what someone may see. The messages and codes name costs and the floor (a multiple of the
    /// cost), so a person without sales:view_costs sees only that a rule is flagged, never a figure; the Owner's
    /// override reason is left out for the same reason.
    pub fn json_for(&self, see_costs: bool) -> Value {
        let mut v = self.json();
        if see_costs {
            return v;
        }
        let hide = |issues: Vec<&Issue>| {
            issues
                .into_iter()
                .map(|i| json!({"code": "pricing_rule", "message": i.message_for(false)}))
                .collect::<Vec<_>>()
        };
        v["blocking"] = json!(hide(self.check.blocking()));
        v["warnings"] = json!(hide(self.check.warnings()));
        if let Some(o) = v["override"].as_object_mut() {
            o.remove("reason");
        }
        v
    }
}

#[derive(sqlx::FromRow)]
struct StoredItem {
    product_id: Option<Uuid>,
    sku: Option<String>,
    name: String,
    quantity: Decimal,
    unit_price: Decimal,
    discount_percentage: Option<Decimal>,
    is_optional: Option<bool>,
    amount: Decimal,
    term_months: Option<i32>,
    rate_period_months: Option<i32>,
    sale_model: Option<String>,
}

#[derive(sqlx::FromRow)]
struct StoredHead {
    id: Uuid,
    status: Option<String>,
    currency: Option<String>,
    issue_date: NaiveDate,
    deal_id: Option<Uuid>,
    subtotal: Option<Decimal>,
    discount_type: Option<String>,
    discount_value: Option<Decimal>,
    discount_amount: Option<Decimal>,
    pricing_override_reason: Option<String>,
    pricing_override_by_id: Option<Uuid>,
    pricing_override_at: Option<chrono::DateTime<Utc>>,
    pricing_override_fingerprint: Option<String>,
}

/// The fingerprint of a quotation's lines and header discount (an override holds only for these).
fn fingerprint(head: &StoredHead, items: &[StoredItem]) -> String {
    let mut rows: Vec<String> = items
        .iter()
        .map(|i| {
            format!(
                "{}|{}|{}|{}|{}|{}",
                i.product_id.map(|p| p.to_string()).unwrap_or_default(),
                i.sku.clone().unwrap_or_default(),
                i.quantity.normalize(),
                i.unit_price.normalize(),
                i.discount_percentage.unwrap_or_default().normalize(),
                i.is_optional.unwrap_or(false)
            )
        })
        .collect();
    rows.sort();
    rows.push(format!(
        "{}|{}|{}",
        head.currency.clone().unwrap_or_default(),
        head.discount_type.clone().unwrap_or_default(),
        head.discount_value.unwrap_or_default().normalize()
    ));
    hex::encode(Sha256::digest(rows.join("\n").as_bytes()))
}

/// Periods a stored line's amount covers (term / rate period for a recurring line with a term, else 1).
fn periods(i: &StoredItem) -> Decimal {
    if i.sale_model.as_deref() == Some("recurring") {
        if let (Some(t), Some(r)) = (i.term_months, i.rate_period_months.filter(|r| *r > 0)) {
            if t > 0 {
                return Decimal::from(t) / Decimal::from(r);
            }
        }
    }
    Decimal::ONE
}

/// The check of a saved quotation (None when it doesn't exist).
pub async fn check_stored(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Stored>> {
    let head: Option<StoredHead> = sqlx::query_as(
        "SELECT id, status, currency, issue_date, deal_id, subtotal, discount_type, discount_value, \
         discount_amount, pricing_override_reason, pricing_override_by_id, pricing_override_at, \
         pricing_override_fingerprint FROM quotations WHERE id = $1 AND is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(head) = head else {
        return Ok(None);
    };
    let rows: Vec<StoredItem> = sqlx::query_as(
        "SELECT product_id, sku, name, quantity, unit_price, discount_percentage, is_optional, amount, \
         term_months, rate_period_months, sale_model FROM quotation_items WHERE quotation_id = $1 \
         ORDER BY sort_order",
    )
    .bind(id)
    .fetch_all(&mut *db)
    .await?;
    let rules = load(db).await?;
    let ids: Vec<Uuid> = rows.iter().filter_map(|r| r.product_id).collect();
    let items = items(db, &ids).await?;
    // the header discount as a share of the billed subtotal
    let subtotal = head.subtotal.unwrap_or_default();
    let header_share = if subtotal > Decimal::ZERO {
        head.discount_amount.unwrap_or_default() / subtotal
    } else {
        Decimal::ZERO
    };
    let lines: Vec<RuleLine> = rows
        .iter()
        .map(|r| {
            let optional = r.is_optional.unwrap_or(false);
            let share = if optional {
                Decimal::ZERO
            } else {
                header_share
            };
            let net = r.amount * (Decimal::ONE - share);
            let per = r.quantity * periods(r);
            RuleLine {
                item: r.product_id.and_then(|p| items.get(&p).cloned()),
                name: r.name.clone(),
                quantity: r.quantity,
                net_amount: net.round_dp(2),
                net_unit: if per.is_zero() {
                    Decimal::ZERO
                } else {
                    (net / per).round_dp(2)
                },
                is_optional: optional,
            }
        })
        .collect();
    let currency = head.currency.clone().unwrap_or_else(|| "USD".into());
    let ctx = context(db, &currency, Some(head.issue_date), head.deal_id).await?;
    let check = evaluate(&rules, &lines, &ctx);
    let fp = fingerprint(&head, &rows);
    let override_by = match head.pricing_override_by_id {
        Some(u) => sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(NULLIF(trim(name), ''), email) FROM users WHERE id = $1",
        )
        .bind(u)
        .fetch_optional(&mut *db)
        .await
        .ok()
        .flatten(),
        None => None,
    };
    let reason = head
        .pricing_override_reason
        .clone()
        .filter(|r| !r.trim().is_empty());
    Ok(Some(Stored {
        id: head.id,
        status: head.status.clone().unwrap_or_default(),
        override_valid: reason.is_some()
            && head.pricing_override_fingerprint.as_deref() == Some(fp.as_str()),
        check,
        fingerprint: fp,
        override_reason: reason,
        override_by,
        override_at: head.pricing_override_at,
    }))
}

/// Why this draft can't be submitted for approval: a blocking pricing rule and no valid Owner override. Sent or
/// approved quotations are never re-checked.
pub async fn quotation_refusal(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<String>> {
    quotation_refusal_for(db, id, true).await
}

/// The refusal as the submitter may see it: without sales:view_costs it names no figure.
pub async fn quotation_refusal_for(
    db: &mut PgConnection,
    id: Uuid,
    see_costs: bool,
) -> ApiResult<Option<String>> {
    if !blocks_submit(db).await? {
        return Ok(None);
    }
    let Some(s) = check_stored(db, id).await? else {
        return Ok(None);
    };
    if !matches!(s.status.as_str(), "draft" | "rejected" | "") || s.override_valid {
        return Ok(None);
    }
    let blocking: Vec<String> = s
        .check
        .blocking()
        .iter()
        .map(|i| i.message_for(see_costs).to_string())
        .collect();
    // without costs every hidden issue reads the same: say it once
    let mut seen = HashSet::new();
    let blocking: Vec<String> = blocking
        .into_iter()
        .filter(|m| see_costs || m != HIDDEN_BLOCKING || seen.insert(m.clone()))
        .collect();
    if blocking.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "The prices break the pricing rules: {} Fix the prices, or ask the Owner for a pricing override with a reason.",
        blocking.join(" ")
    )))
}

/// Whether a quotation must go to the Owner whatever its prices: a line whose catalog price is still proposed, or
/// a SKU in sales.pricing_rules.owner_approval_skus (e.g. retainers).
pub async fn needs_owner(db: &mut PgConnection, quotation_id: Uuid) -> ApiResult<bool> {
    let rows: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT COALESCE(p.sku, i.sku), p.price_status FROM quotation_items i \
         LEFT JOIN product_catalog p ON p.id = i.product_id WHERE i.quotation_id = $1",
    )
    .bind(quotation_id)
    .fetch_all(&mut *db)
    .await?;
    if rows.is_empty() {
        return Ok(false);
    }
    let pats = patterns(&require_json(db, OWNER_SKUS_KEY, WHERE).await?);
    Ok(rows.iter().any(|(sku, status)| {
        status.as_deref() == Some("proposed")
            || sku.as_deref().is_some_and(|s| matches_any(&pats, s))
    }))
}

/// The Owner sets (or clears, with an empty reason) the pricing override of a draft; an audit row records it.
pub async fn set_override(
    db: &mut PgConnection,
    user: &AuthUser,
    id: Uuid,
    reason: Option<&str>,
    origin: &str,
) -> ApiResult<Value> {
    if !crate::nary::quotation_updates::is_owner(db, user).await? {
        return Err(ApiError::http(403, OVERRIDE_REFUSED));
    }
    let Some(s) = check_stored(db, id).await? else {
        return Err(ApiError::not_found("Quotation", Some(&id.to_string())));
    };
    if !matches!(s.status.as_str(), "draft" | "rejected" | "") {
        return Err(ApiError::http(
            400,
            "Only a draft (or a rejected quotation) takes a pricing override.",
        ));
    }
    let reason = reason.map(str::trim).filter(|r| !r.is_empty());
    let (action, values) = match reason {
        Some(r) => {
            if r.chars().count() < 10 {
                return Err(ApiError::http(
                    400,
                    "Give the reason for the override in a sentence (at least 10 characters).",
                ));
            }
            sqlx::query(
                "UPDATE quotations SET pricing_override_reason = $2, pricing_override_by_id = $3, \
                 pricing_override_at = now(), pricing_override_fingerprint = $4, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(r)
            .bind(user.id)
            .bind(&s.fingerprint)
            .execute(&mut *db)
            .await?;
            (
                "pricing_override",
                json!({"reason": r, "blocking": s.check.blocking().iter().map(|i| i.message.clone()).collect::<Vec<_>>()}),
            )
        }
        None => {
            sqlx::query(
                "UPDATE quotations SET pricing_override_reason = NULL, pricing_override_by_id = NULL, \
                 pricing_override_at = NULL, pricing_override_fingerprint = NULL, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *db)
            .await?;
            ("pricing_override_cleared", json!({"reason": null}))
        }
    };
    let label: Option<String> =
        sqlx::query_scalar("SELECT COALESCE(full_number, number) FROM quotations WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let name: Option<String> = sqlx::query_scalar(
        "SELECT COALESCE(NULLIF(trim(name), ''), email) FROM users WHERE id = $1",
    )
    .bind(user.id)
    .fetch_optional(&mut *db)
    .await?;
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
         new_values, notes, origin) VALUES ($1, $2, $3, $4, 'quotation', $5, $6, $7::json, $8, $9)",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(name)
    .bind(action)
    .bind(id.to_string())
    .bind(label)
    .bind(values.to_string())
    .bind(reason)
    .bind(origin)
    .execute(&mut *db)
    .await?;
    let s = check_stored(db, id).await?.map(|s| s.json());
    Ok(json!({"id": id.to_string(), "pricing_check": s}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules {
            min_cost_multiple: Decimal::new(20, 1),
            owner_approval_skus: vec!["SVC-RETAINER-*".into()],
        }
    }

    fn item(sku: &str, ptype: &str, sale: &str, cost: Option<i64>) -> Item {
        Item {
            id: Uuid::new_v4(),
            sku: sku.into(),
            name: sku.into(),
            product_type: ptype.into(),
            sale_model: sale.into(),
            pricing_mode: "list".into(),
            price_status: Some("locked".into()),
            cost_price: cost.map(Decimal::from),
            is_active: Some(true),
        }
    }

    fn line(i: Item, unit: i64) -> RuleLine {
        RuleLine {
            name: i.name.clone(),
            item: Some(i),
            quantity: Decimal::ONE,
            net_amount: Decimal::from(unit),
            net_unit: Decimal::from(unit),
            is_optional: false,
        }
    }

    fn ctx() -> Ctx {
        Ctx {
            currency: "USD".into(),
            fx: Some(Decimal::ONE),
            labour: None,
        }
    }

    fn codes(c: &Check) -> Vec<(&'static str, bool)> {
        c.issues.iter().map(|i| (i.code, i.blocking)).collect()
    }

    #[test]
    fn below_the_minimum_multiple_of_cost_blocks() {
        let p = item("WIDGET-1", "physical", "one_time", Some(5000));
        // 2x cost = 10000
        let c = evaluate(&rules(), &[line(p.clone(), 9999)], &ctx());
        assert_eq!(codes(&c), vec![("below_min_cost_multiple", true)]);
        assert!(c.issues[0].message.contains("10,000.00"), "{c:?}");
        assert!(evaluate(&rules(), &[line(p, 10_000)], &ctx())
            .issues
            .is_empty());
    }

    #[test]
    fn the_multiple_comes_from_the_setting() {
        let p = item("WIDGET-1", "physical", "one_time", Some(1000));
        let mut r = rules();
        r.min_cost_multiple = Decimal::new(13, 1); // 1.3x
        assert_eq!(
            codes(&evaluate(&r, &[line(p.clone(), 1299)], &ctx())),
            vec![("below_min_cost_multiple", true)]
        );
        assert!(evaluate(&r, &[line(p, 1300)], &ctx()).issues.is_empty());
    }

    #[test]
    fn fx_converts_the_functional_cost() {
        let p = item("WIDGET-1", "physical", "one_time", Some(16_000_000));
        let mut x = ctx();
        x.fx = Some(Decimal::new(625, 7)); // functional 16,000,000 = USD 1,000
        assert_eq!(
            codes(&evaluate(&rules(), &[line(p.clone(), 1999)], &x)),
            vec![("below_min_cost_multiple", true)]
        );
        assert!(evaluate(&rules(), &[line(p.clone(), 2000)], &x)
            .issues
            .is_empty());
        x.fx = None;
        assert_eq!(
            codes(&evaluate(&rules(), &[line(p, 1)], &x)),
            vec![("cost_unknown", false)],
            "no rate: cost unknown, never a block"
        );
    }

    /// Without sales:view_costs the card lines name no figure; with it they are unchanged.
    #[test]
    fn card_lines_hide_figures_from_non_cost_viewers() {
        let p = item("WIDGET-1", "physical", "one_time", Some(8000));
        let sw = item("APP-PERP", "software", "one_time", None);
        let c = evaluate(&rules(), &[line(p, 15_999), line(sw, 1)], &ctx());
        let full = c.card_lines_for(true);
        assert_eq!(full, c.card_lines());
        assert!(full.iter().any(|l| l.contains("16,000.00")), "{full:?}");
        let hidden = c.card_lines_for(false);
        assert_eq!(hidden.len(), full.len());
        for l in &hidden {
            assert!(
                !l.contains("16,000") && !l.to_lowercase().contains("cost"),
                "{l}"
            );
        }
        assert_eq!(
            hidden[0],
            format!("Pricing check (blocks approval): {HIDDEN_BLOCKING}")
        );
        assert_eq!(hidden[1], format!("Pricing check: {HIDDEN_WARNING}"));
        assert_eq!(hidden.last().unwrap(), APPROVAL_LINE);
        let redacted: Vec<String> = full.iter().map(|l| hide_card_line(l)).collect();
        assert_eq!(redacted, hidden);
        assert_eq!(
            hide_card_line("Customer has no email"),
            "Customer has no email"
        );
    }

    #[test]
    fn missing_cost_warns_never_blocks() {
        let s = item("APP-PERP", "software", "one_time", None);
        let c = evaluate(&rules(), &[line(s, 1)], &ctx());
        assert_eq!(codes(&c), vec![("cost_unknown", false)]);
        assert!(c.issues[0].message.contains("APP-PERP"));
        let mut care = item("CARE-PLAN", "service", "recurring", None);
        care.pricing_mode = "percent_of_parent".into();
        assert!(evaluate(&rules(), &[line(care, 10)], &ctx())
            .issues
            .is_empty());
    }

    #[test]
    fn services_against_the_cost_sheet_labour() {
        let svc = item("SVC-PROJECT", "service", "one_time", None);
        let mut x = ctx();
        x.labour = Some(Decimal::from(10_000));
        assert_eq!(
            codes(&evaluate(&rules(), &[line(svc.clone(), 19_999)], &x)),
            vec![("below_min_cost_multiple", true)]
        );
        assert!(evaluate(&rules(), &[line(svc.clone(), 20_000)], &x)
            .issues
            .is_empty());
        assert_eq!(
            codes(&evaluate(&rules(), &[line(svc, 20_000)], &ctx())),
            vec![("cost_unknown", false)],
            "no cost sheet: cost unknown"
        );
    }

    #[test]
    fn optional_lines_are_offered_not_sold() {
        let p = item("WIDGET-1", "physical", "one_time", Some(5000));
        let mut l = line(p, 1);
        l.is_optional = true;
        assert!(evaluate(&rules(), &[l], &ctx()).issues.is_empty());
    }

    #[test]
    fn settings_are_checked() {
        assert!(validate_setting(MIN_COST_KEY, "2.0").unwrap().is_ok());
        assert!(validate_setting(MIN_COST_KEY, "0").unwrap().is_err());
        assert!(validate_setting(OWNER_SKUS_KEY, "[\"SVC-*\"]")
            .unwrap()
            .is_ok());
        assert!(validate_setting(OWNER_SKUS_KEY, "{}").unwrap().is_err());
        assert!(validate_setting("other.key", "x").is_none());
        assert_eq!(money(Decimal::new(123456789, 2)), "1,234,567.89");
        assert_eq!(money(Decimal::from(95)), "95.00");
    }
}
