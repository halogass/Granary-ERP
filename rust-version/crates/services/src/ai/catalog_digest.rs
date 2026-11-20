//! Port of backend/app/services/ai/catalog_digest.py: the whole product catalog in one compact list,
//! for the AI chat's prompt.
//!
//! Two levels: "list" (how each product is sold and its list price) for everyone who may see the
//! catalog, and "full" for cost viewers: also volume tiers, the white-label rule, cost and margin at
//! list. The text is cached in the process, keyed by the catalog's size and last change (products,
//! prices, tiers), the company currency and the latest FX rate, so a price edit shows up on the next
//! message.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use chrono::{DateTime, NaiveDate, Utc};
use granary_compat::pyfmt::{format_number, PyNum};
use granary_compat::pyjson::{str_value, PyValue};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use granary_db::rows::sales::{CatalogVolumeTier, ProductCatalog};
use once_cell::sync::Lazy;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;

pub const WHERE: &str = "Nary & AI → AI chat";

static CACHE: Lazy<Mutex<HashMap<Vec<Option<String>>, String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// `float(Decimal)`: correctly rounded, as Python does it from the decimal digits.
fn pyfloat(d: Decimal) -> f64 {
    d.to_string().parse::<f64>().unwrap_or(0.0)
}

fn fmt(v: f64, spec: &str) -> String {
    format_number(&PyNum::Float(v), spec).expect("supported format spec")
}

/// `str(e.detail)` of an HTTPException; None for anything else (AppException, SettingError...).
pub(crate) fn http_detail(e: &ApiError) -> Option<String> {
    match e {
        ApiError::Http { detail, .. } => Some(match detail {
            Value::String(s) => s.clone(),
            other => str_value(&PyValue::from(other)),
        }),
        _ => None,
    }
}

async fn key(db: &mut PgConnection, currency: &str) -> ApiResult<Vec<Option<String>>> {
    let mut parts: Vec<Option<String>> = Vec::new();
    for table in ["product_catalog", "catalog_prices", "catalog_volume_tiers"] {
        let (count, last): (i64, Option<DateTime<Utc>>) = sqlx::query_as(&format!(
            "SELECT count({table}.id), max({table}.updated_at) FROM {table}"
        ))
        .fetch_one(&mut *db)
        .await?;
        parts.push(Some(count.to_string()));
        parts.push(last.map(|l| l.to_rfc3339()));
    }
    let fx: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT max(exchange_rates.updated_at) FROM exchange_rates")
            .fetch_one(&mut *db)
            .await?;
    parts.push(Some(currency.to_string()));
    parts.push(fx.map(|l| l.to_rfc3339()));
    // Python: date.today(), the server's local date (not the company's).
    parts.push(Some(chrono::Local::now().date_naive().to_string()));
    Ok(parts)
}

/// `_full(p, price_text, amount)`: the cost viewer's extra detail for one product.
pub fn full(
    p: &ProductCatalog,
    tiers: &[CatalogVolumeTier],
    _price_text: &str,
    amount: Option<Decimal>,
) -> String {
    let mut bits: Vec<String> = Vec::new();
    let mut tiers: Vec<&CatalogVolumeTier> = tiers.iter().collect();
    // sorted(key=float(min_qty or 0)): a stable sort, ties keep the load order.
    tiers.sort_by(|a, b| {
        pyfloat(a.min_qty)
            .partial_cmp(&pyfloat(b.min_qty))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if !tiers.is_empty() {
        let txt: Vec<String> = tiers
            .iter()
            .map(|t| {
                let hi = match t.max_qty {
                    Some(m) if !m.is_zero() => format!("-{}", fmt(pyfloat(m), "g")),
                    _ => "+".to_string(),
                };
                format!(
                    "{}{}: -{}%",
                    fmt(pyfloat(t.min_qty), "g"),
                    hi,
                    fmt(pyfloat(t.discount_pct), "g")
                )
            })
            .collect();
        bits.push(format!("tiers {}", txt.join(", ")));
    }
    if let Some(n) = p.whitelabel_free_min_qty.filter(|n| *n != 0) {
        bits.push(format!(
            "white-label free from {} units",
            fmt(n as f64, "g")
        ));
    }
    let rule = p.whitelabel_rule.as_ref().filter(|r| py_truthy(r));
    let mode = rule.and_then(|r| r.get("mode"));
    let value = rule.and_then(|r| r.get("value"));
    if mode == Some(&Value::String("percent".into())) {
        bits.push(format!(
            "white-label +{}% of the parent line",
            fmt(json_float_or_zero(value), "g")
        ));
    } else if mode == Some(&Value::String("fixed".into())) {
        let v = value
            .map(|v| str_value(&PyValue::from(v)))
            .unwrap_or_else(|| "None".to_string());
        bits.push(format!("white-label fixed {v}"));
    }
    if let Some(t) = p.terms_note.as_deref().filter(|t| !t.is_empty()) {
        let cut: String = t.chars().take(160).collect();
        bits.push(format!("terms: {cut}"));
    }
    if let Some(c) = p.cost_price.filter(|c| !c.is_zero()) {
        let cost = pyfloat(c);
        bits.push(format!("cost {}", fmt(cost, ",.0f")));
        if let Some(a) = amount.filter(|a| !a.is_zero()) {
            let a = pyfloat(a);
            bits.push(format!(
                "margin at list {}%",
                fmt(100.0 * (a - cost) / a, ".0f")
            ));
        }
    }
    bits.join("; ")
}

/// Python truthiness of a JSON value (`rule = p.whitelabel_rule or {}`).
fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `float(rule.get('value') or 0)`.
fn json_float_or_zero(v: Option<&Value>) -> f64 {
    match v {
        Some(v) if py_truthy(v) => match v {
            Value::Number(n) => n.to_string().parse().unwrap_or(0.0),
            Value::String(s) => granary_compat::text::pystr::strip(s).parse().unwrap_or(0.0),
            Value::Bool(true) => 1.0,
            _ => 0.0,
        },
        _ => 0.0,
    }
}

/// `_money(amount, currency)`.
pub fn money(amount: Decimal, currency: &str) -> String {
    let value = pyfloat(amount);
    if value >= 100.0 {
        format!("{currency} {}", fmt(value, ",.0f"))
    } else {
        format!("{currency} {}", fmt(value, ",.2f"))
    }
}

/// `_sold(p)`: how the product is sold.
pub fn sold(p: &ProductCatalog) -> String {
    let mut bits: Vec<String> = vec![
        p.sale_model.clone(),
        if p.price_metric.is_empty() {
            String::new()
        } else {
            format!("per {}", p.price_metric)
        },
    ];
    if let Some(n) = p.rate_period_months.filter(|n| *n != 0) {
        bits.push(format!("rate per {n} mo"));
    }
    if let Some(n) = p.billing_interval_months.filter(|n| *n != 0) {
        bits.push(format!("billed every {n} mo"));
    }
    if let Some(q) = p.min_quantity.filter(|q| !q.is_zero()) {
        if pyfloat(q) > 1.0 {
            bits.push(format!("min {}", fmt(pyfloat(q), "g")));
        }
    }
    if let Some(n) = p.min_term_months.filter(|n| *n != 0) {
        bits.push(format!("min term {n} mo"));
    }
    if let Some(n) = p.included_support_months.filter(|n| *n != 0) {
        bits.push(format!("{n} mo support included"));
    }
    if let Some(a) = p.addon_for.as_ref().filter(|a| !a.is_empty()) {
        bits.push(format!("add-on for {}", a.join(", ")));
    }
    bits.into_iter()
        .filter(|b| !b.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `_price(db, p, currency, today)`: (the price as shown, the list amount or None).
async fn price(
    db: &mut PgConnection,
    p: &ProductCatalog,
    currency: &str,
    today: NaiveDate,
) -> ApiResult<(String, Option<Decimal>)> {
    match p.pricing_mode.as_str() {
        "quote_only" => return Ok(("quote only".into(), None)),
        "manual" => return Ok(("priced by hand".into(), None)),
        "percent_of_parent" => {
            let rate = pyfloat(p.percent_rate.unwrap_or_default());
            return Ok((format!("{}% of the parent line", fmt(rate, "g")), None));
        }
        _ => {}
    }
    if p.is_whitelabel {
        return Ok(("white-label, priced from its parent line".into(), None));
    }
    match crate::pricing::price_for(db, p, currency, today).await {
        Ok(quote) => Ok((money(quote.amount, currency), Some(quote.amount))),
        Err(e) => match http_detail(&e) {
            Some(detail) => Ok((format!("no price ({detail})"), None)),
            None => Err(e),
        },
    }
}

/// `digest(db, detail)`: every active product, one line each; "" when there is none. detail="full"
/// adds tiers, white-label rule, cost and margin (cost viewers only).
pub async fn digest(db: &mut PgConnection, detail: &str) -> ApiResult<String> {
    let currency = crate::fx::functional_currency(&mut *db)
        .await?
        .to_uppercase();
    let mut k = vec![Some(detail.to_string())];
    k.extend(key(&mut *db, &currency).await?);
    if let Some(hit) = CACHE.lock().unwrap().get(&k) {
        return Ok(hit.clone());
    }
    let cap = require_int(&mut *db, "ai.assistant.catalog_digest_max_items", WHERE).await?;
    let rows: Vec<ProductCatalog> = sqlx::query_as(
        "SELECT * FROM product_catalog WHERE product_catalog.is_deleted = false AND product_catalog.is_active = true \
         ORDER BY product_catalog.family, product_catalog.sku",
    )
    .fetch_all(&mut *db)
    .await?;
    let text = if rows.is_empty() {
        String::new()
    } else if rows.len() as i64 > cap {
        let mut families: BTreeMap<String, i64> = BTreeMap::new();
        for p in &rows {
            let f = p
                .family
                .clone()
                .filter(|f| !f.is_empty())
                .unwrap_or_else(|| "other".into());
            *families.entry(f).or_insert(0) += 1;
        }
        format!(
            "PRODUCT CATALOG: {} active products, too many to list (over \
             ai.assistant.catalog_digest_max_items). Families: {}. Use search_catalog with a family or keyword.",
            rows.len(),
            families.iter().map(|(k, v)| format!("{k} ({v})")).collect::<Vec<_>>().join(", ")
        )
    } else {
        let today = granary_core::clock::today_local(&mut *db).await?;
        // selectinload(ProductCatalog.volume_tiers): one query for all products.
        let ids: Vec<uuid::Uuid> = rows.iter().map(|p| p.id).collect();
        let all_tiers: Vec<CatalogVolumeTier> = if detail == "full" {
            sqlx::query_as(
                "SELECT * FROM catalog_volume_tiers WHERE product_id = ANY($1) ORDER BY min_qty",
            )
            .bind(&ids)
            .fetch_all(&mut *db)
            .await?
        } else {
            Vec::new()
        };
        let mut lines = Vec::new();
        for p in &rows {
            let mut flags = Vec::new();
            if p.is_whitelabel {
                flags.push("white-label");
            }
            // Python: `not p.is_orderable`, so a NULL counts as not orderable.
            if !p.is_orderable.unwrap_or(false) {
                flags.push("not orderable");
            }
            let (price_text, amount) = price(&mut *db, p, &currency, today).await?;
            let extra = if detail == "full" {
                let tiers: Vec<CatalogVolumeTier> = all_tiers
                    .iter()
                    .filter(|t| t.product_id == p.id)
                    .cloned()
                    .collect();
                full(p, &tiers, &price_text, amount)
            } else {
                String::new()
            };
            let fam = p.family.clone().unwrap_or_default();
            let sold = sold(p);
            let flags = flags.join(", ");
            let pieces = [
                p.sku.as_str(),
                p.name.as_str(),
                fam.as_str(),
                sold.as_str(),
                price_text.as_str(),
                flags.as_str(),
                extra.as_str(),
            ];
            lines.push(
                pieces
                    .iter()
                    .filter(|x| !x.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
        let private = if detail == "full" {
            " With INTERNAL detail (tiers, white-label rules, cost, margin): for you and the person you \
             are helping only, never in anything sent to a customer."
        } else {
            ""
        };
        format!(
            "PRODUCT CATALOG: all {} active products (SKU · name · family · how it's sold · list \
             price in {currency}). This is the whole catalog: if something isn't here, the company doesn't sell it \
             as a catalog item. Use search_catalog for one item's full detail or another currency; the draft \
             tools price quotation lines themselves.{private}\n{}",
            rows.len(),
            lines.join("\n")
        )
    };
    let mut cache = CACHE.lock().unwrap();
    // only the current catalog is worth keeping (one per detail level)
    cache.retain(|k, _| k[0].as_deref() != Some(detail));
    cache.insert(k, text.clone());
    Ok(text)
}
