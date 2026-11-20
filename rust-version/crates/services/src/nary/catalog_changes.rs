//! 23 Oct 2026, the Owner's request: Nary in the ERP chat and the MCP client change the product catalog (prices,
//! product details, tiers and cost price, new SKUs, retiring), always as a card the Owner confirms.
//!
//! ```text
//!     catalog.change   one product, one or more changes in order (payload `changes`):
//!                      create_product (fields, then the prices and tiers that follow), update_product (the
//!                      fields given), add_price (an open row of that currency is ended the day before the new
//!                      one starts), update_price, end_price, set_tiers (the full list, private), set_cost_price,
//!                      retire, unretire (is_active and is_orderable).
//! ```
//!
//! Only the Owner (the holder of the SOP role Owner; a superadmin for tests), at the tool and again at apply. Starts
//! at Ask and is on nary.autonomy.never_promote and nary.owner.confirm_twice_actions (migration 0127); over MCP it
//! never applies in one call (mcp_access::CATALOG_ACTIONS). The card stores a fingerprint of the product as it
//! was shown; a product changed since is refused. The writes are the editor's own
//! (crate::catalog::writes), so the same checks and texts apply. After a price, tier, new or retired product
//! the internal price list PDF gets a new version (a failure is reported, never fatal); open quotations keep
//! their price.

use chrono::{Duration, NaiveDate};
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::validation::{validate_value, LineError, PyObj, Validate};
use granary_db::rows::sales::{CatalogPrice, ProductCatalog, Quotation};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::{apply_fn, Action, Field, Schema, Ty};
use crate::catalog::writes::{
    self, PriceRowBody, PriceRowPatch, ProductCreate, ProductFields, TiersBody,
};

pub const CHANGE: &str = "catalog.change";

/// The ops a change can carry.
pub const OPS: [&str; 9] = [
    "create_product",
    "update_product",
    "add_price",
    "update_price",
    "end_price",
    "set_tiers",
    "set_cost_price",
    "retire",
    "unretire",
];

pub const ONLY_OWNER: &str =
    "Only the Owner can change the catalog through Nary or the MCP client (Sales -> Catalog is the editor)";
pub const STALE: &str = "The catalog changed since this card was made; ask again";
pub const PRICE_LIST: &str = "The internal price list PDF is refreshed after this change";
pub const QUOTES_KEEP: &str =
    "Open quotations keep their price; Nary can update a draft with propose_quotation_update";

fn schema() -> Schema {
    Schema::strict(
        "CatalogChange",
        vec![
            Field::opt("product_id", Ty::Str),
            Field::req("changes", Ty::list(Ty::Dict)),
            Field::req("before", Ty::Str),
        ],
    )
}

/// The actions of this module (registered by actions::module_actions).
pub fn actions() -> Vec<Action> {
    vec![Action::new(
        CHANGE,
        "Change the product catalog (the Owner only)",
        schema(),
        apply_fn(|db, a, p| Box::pin(apply(db, a, p))),
    )
    .describe(apply_fn(|db, a, p| Box::pin(describe(db, a, p))))]
}

/// The Owner (the SOP role's holder), or a superadmin.
pub async fn is_owner(db: &mut PgConnection, user: &AuthUser) -> ApiResult<bool> {
    if user.is_superadmin {
        return Ok(true);
    }
    Ok(super::roles::holder(db, "Owner").await? == Some(user.id))
}

// ─── the ops ──────────────────────────────────────────────────────────────────

/// One parsed change.
#[derive(Clone, Debug)]
pub enum Op {
    Create(Box<ProductCreate>),
    Update(Box<ProductFields>, Vec<String>),
    AddPrice(PriceRowBody),
    UpdatePrice(String, PriceRowPatch, Vec<String>),
    EndPrice(String, NaiveDate),
    SetTiers(TiersBody),
    SetCost(Option<Decimal>),
    Retire,
    Unretire,
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::Create(_) => "create_product",
            Op::Update(..) => "update_product",
            Op::AddPrice(_) => "add_price",
            Op::UpdatePrice(..) => "update_price",
            Op::EndPrice(..) => "end_price",
            Op::SetTiers(_) => "set_tiers",
            Op::SetCost(_) => "set_cost_price",
            Op::Retire => "retire",
            Op::Unretire => "unretire",
        }
    }
    /// A change the price list shows (prices, tiers, a new or retired product).
    fn refreshes_price_list(&self) -> bool {
        matches!(
            self,
            Op::Create(_)
                | Op::AddPrice(_)
                | Op::UpdatePrice(..)
                | Op::EndPrice(..)
                | Op::SetTiers(_)
                | Op::Retire
                | Op::Unretire
        )
    }
    /// A change that matters to an open quotation of the product.
    fn touches_quotes(&self) -> bool {
        matches!(
            self,
            Op::AddPrice(_) | Op::UpdatePrice(..) | Op::EndPrice(..) | Op::SetTiers(_) | Op::Retire
        )
    }
}

fn line_errors(errors: &[LineError]) -> String {
    errors
        .iter()
        .map(|e| {
            let f = e.field();
            if f.is_empty() {
                e.msg.clone()
            } else {
                format!("{f}: {}", e.msg)
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn model<T: Validate>(v: &Value, what: &str) -> Result<T, String> {
    validate_value::<T>(&PyObj::from_json(v), None)
        .map_err(|e| format!("{what}: {}", line_errors(&e)))
}

fn keys(v: &Value) -> Vec<String> {
    v.as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn without(v: &Value, drop: &[&str]) -> Value {
    let mut m = v.as_object().cloned().unwrap_or_default();
    for k in drop {
        m.remove(*k);
    }
    Value::Object(m)
}

fn date_of(v: Option<&Value>, what: &str) -> Result<Option<NaiveDate>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
            .map(Some)
            .map_err(|_| format!("{what} must be a date like 2026-11-01")),
        Some(_) => Err(format!("{what} must be a date like 2026-11-01")),
    }
}

/// The changes of a payload as ops, checked with the editor's request models. `today` fills an add_price
/// without valid_from and an end_price without valid_to.
pub fn parse_ops(changes: &[Value], today: NaiveDate) -> Result<Vec<Op>, String> {
    if changes.is_empty() {
        return Err("changes is empty: give at least one change".into());
    }
    let mut out = vec![];
    for (i, c) in changes.iter().enumerate() {
        let n = i + 1;
        let op = c.get("op").and_then(Value::as_str).unwrap_or("").trim();
        let what = format!("change {n} ({op})");
        let parsed = match op {
            "create_product" => {
                if i != 0 {
                    return Err(format!(
                        "{what}: a new product is the first change; prices and tiers follow it"
                    ));
                }
                let fields = c.get("fields").cloned().unwrap_or_else(|| json!({}));
                Op::Create(Box::new(model::<ProductCreate>(&fields, &what)?))
            }
            "update_product" => {
                let fields = c.get("fields").cloned().unwrap_or_else(|| json!({}));
                let set = keys(&fields);
                if set.is_empty() {
                    return Err(format!("{what}: give the fields to change"));
                }
                if let Some(bad) = set
                    .iter()
                    .find(|k| !writes::field_names().contains(&k.as_str()))
                {
                    return Err(format!(
                        "{what}: {bad} is not a product field (the fields: {})",
                        writes::field_names().join(", ")
                    ));
                }
                Op::Update(Box::new(model::<ProductFields>(&fields, &what)?), set)
            }
            "add_price" => {
                let mut body = without(c, &["op"]);
                if date_of(body.get("valid_from"), "valid_from")?.is_none() {
                    body["valid_from"] = json!(today.to_string());
                }
                Op::AddPrice(model::<PriceRowBody>(&body, &what)?)
            }
            "update_price" => {
                let id = c
                    .get("price_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if id.is_empty() {
                    return Err(format!("{what}: price_id is required"));
                }
                let body = without(c, &["op", "price_id"]);
                let set: Vec<String> = keys(&body)
                    .into_iter()
                    .filter(|k| {
                        ["currency", "amount", "valid_from", "valid_to"].contains(&k.as_str())
                    })
                    .collect();
                if set.is_empty() {
                    return Err(format!(
                        "{what}: give what changes (currency, amount, valid_from, valid_to)"
                    ));
                }
                Op::UpdatePrice(id.to_string(), model::<PriceRowPatch>(&body, &what)?, set)
            }
            "end_price" => {
                let id = c
                    .get("price_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if id.is_empty() {
                    return Err(format!("{what}: price_id is required"));
                }
                let to = date_of(c.get("valid_to"), &format!("{what}: valid_to"))?.unwrap_or(today);
                Op::EndPrice(id.to_string(), to)
            }
            "set_tiers" => {
                let body = json!({"tiers": c.get("tiers").cloned().unwrap_or_else(|| json!([]))});
                Op::SetTiers(model::<TiersBody>(&body, &what)?)
            }
            "set_cost_price" => {
                let Some(v) = c.get("cost_price") else {
                    return Err(format!("{what}: cost_price is required (null clears it)"));
                };
                let f = model::<ProductFields>(&json!({ "cost_price": v }), &what)?;
                Op::SetCost(f.cost_price)
            }
            "retire" => Op::Retire,
            "unretire" => Op::Unretire,
            other => {
                return Err(format!(
                    "change {n}: op {other:?} is not one of {}",
                    OPS.join(", ")
                ))
            }
        };
        out.push(parsed);
    }
    if out.iter().skip(1).any(|o| matches!(o, Op::Create(_))) {
        return Err("Only one new product per card".into());
    }
    Ok(out)
}

/// Runs the ops on `product` (None: the first op creates it). Nothing commits. The product's id.
pub async fn run_ops(
    db: &mut PgConnection,
    actor_id: Uuid,
    product: Option<ProductCatalog>,
    ops: &[Op],
) -> ApiResult<Uuid> {
    let mut product = product;
    for op in ops {
        if let Op::Create(body) = op {
            if product.is_some() {
                return Err(ApiError::http(409, "create_product is for a new SKU only"));
            }
            let id = writes::create_product(db, actor_id, body).await?;
            product = Some(writes::load_product(db, &id.to_string()).await?);
            continue;
        }
        let Some(p) = product.clone() else {
            return Err(ApiError::http(
                422,
                "Name the product (id or SKU), or start with create_product",
            ));
        };
        match op {
            Op::Create(_) => unreachable!(),
            Op::Update(fields, set) => writes::update_product(db, &p, fields, set).await?,
            Op::SetCost(cost) => {
                let f = ProductFields {
                    cost_price: *cost,
                    ..empty_fields()
                };
                writes::update_product(db, &p, &f, &["cost_price".to_string()]).await?
            }
            Op::Retire | Op::Unretire => {
                let on = matches!(op, Op::Unretire);
                let f = ProductFields {
                    is_active: Some(on),
                    is_orderable: Some(on),
                    ..empty_fields()
                };
                let set = ["is_active".to_string(), "is_orderable".to_string()];
                writes::update_product(db, &p, &f, &set).await?
            }
            Op::AddPrice(body) => {
                // an open row of this currency ends the day before the new one starts
                let code = body.currency.trim().to_uppercase();
                let open: Vec<CatalogPrice> = sqlx::query_as(
                    "SELECT * FROM catalog_prices WHERE product_id = $1 AND currency = $2 \
                     AND valid_from < $3 AND (valid_to IS NULL OR valid_to >= $3)",
                )
                .bind(p.id)
                .bind(&code)
                .bind(body.valid_from)
                .fetch_all(&mut *db)
                .await?;
                for row in open {
                    let patch = PriceRowPatch {
                        currency: None,
                        amount: None,
                        valid_from: None,
                        valid_to: Some(body.valid_from - Duration::days(1)),
                    };
                    writes::update_price_row(db, &row, &patch, &["valid_to".to_string()]).await?;
                }
                writes::add_price_row(db, p.id, body).await?;
            }
            Op::UpdatePrice(id, patch, set) => {
                let row = own_price(db, &p, id).await?;
                writes::update_price_row(db, &row, patch, set).await?;
            }
            Op::EndPrice(id, to) => {
                let row = own_price(db, &p, id).await?;
                let patch = PriceRowPatch {
                    currency: None,
                    amount: None,
                    valid_from: None,
                    valid_to: Some(*to),
                };
                writes::update_price_row(db, &row, &patch, &["valid_to".to_string()]).await?;
            }
            Op::SetTiers(body) => writes::replace_tiers(db, p.id, &body.tiers).await?,
        }
        product = Some(writes::load_product(db, &p.id.to_string()).await?);
    }
    product
        .map(|p| p.id)
        .ok_or_else(|| ApiError::http(422, "Nothing to change"))
}

fn empty_fields() -> ProductFields {
    ProductFields {
        name: None,
        short_name: None,
        description: None,
        category: None,
        family: None,
        product_type: None,
        unit: None,
        sale_model: None,
        price_metric: None,
        rate_period_months: None,
        billing_interval_months: None,
        min_quantity: None,
        min_term_months: None,
        unit_equivalents: None,
        modules: None,
        included_support_months: None,
        pricing_mode: None,
        percent_rate: None,
        addon_for: None,
        whitelabel_free_min_qty: None,
        whitelabel_rule: None,
        is_whitelabel: None,
        terms_note: None,
        cost_price: None,
        is_active: None,
        is_orderable: None,
        image_url: None,
        product_line_id: None,
        segment: None,
        price_status: None,
    }
}

/// A price row of this product (another product's row is refused).
async fn own_price(db: &mut PgConnection, p: &ProductCatalog, id: &str) -> ApiResult<CatalogPrice> {
    let row = writes::load_price_row(db, id).await?;
    if row.product_id != p.id {
        return Err(ApiError::http(
            422,
            format!("Price {id} belongs to another product than {}", p.sku),
        ));
    }
    Ok(row)
}

// ─── snapshot, fingerprint, preview ──────────────────────────────────────────

/// The product as the card shows it: its row (no timestamps), price rows and tiers.
pub async fn snapshot(db: &mut PgConnection, product_id: Uuid) -> ApiResult<Value> {
    let row: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(p) - 'created_at' - 'updated_at' - 'deleted_at' FROM product_catalog p \
         WHERE p.id = $1 AND p.is_deleted = false",
    )
    .bind(product_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(row) = row else {
        return Ok(Value::Null);
    };
    let prices: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(jsonb_build_object('id', id::text, 'currency', currency, \
         'amount', amount::text, 'valid_from', valid_from::text, 'valid_to', valid_to::text) \
         ORDER BY currency, valid_from), '[]'::jsonb) FROM catalog_prices WHERE product_id = $1",
    )
    .bind(product_id)
    .fetch_one(&mut *db)
    .await?;
    let tiers: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(jsonb_build_object('min_qty', min_qty::text, 'max_qty', max_qty::text, \
         'discount_pct', discount_pct::text) ORDER BY min_qty), '[]'::jsonb) \
         FROM catalog_volume_tiers WHERE product_id = $1",
    )
    .bind(product_id)
    .fetch_one(&mut *db)
    .await?;
    Ok(json!({"product": row, "prices": prices, "tiers": tiers}))
}

/// A short fingerprint of a snapshot ("new" for a product not made yet).
pub fn fingerprint(snap: &Value) -> String {
    if snap.is_null() {
        return "new".into();
    }
    let digest = Sha256::digest(snap.to_string().as_bytes());
    hex::encode(&digest[..12])
}

/// What a change would do, worked out on the real tables and rolled back.
#[derive(Debug, Clone)]
pub struct Preview {
    pub before: Value,
    pub after: Value,
    pub product_id: Option<Uuid>,
}

/// Runs the ops inside a savepoint (or a transaction of its own) and rolls them back: the before and after
/// snapshots, or the editor's error.
pub async fn preview(
    db: &mut PgConnection,
    actor_id: Uuid,
    product: Option<ProductCatalog>,
    ops: &[Op],
) -> ApiResult<Preview> {
    let nested = sqlx::query("SAVEPOINT catalog_preview")
        .execute(&mut *db)
        .await
        .is_ok();
    if !nested {
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    let pid = product.as_ref().map(|p| p.id);
    let res: ApiResult<Preview> = async {
        let before = match pid {
            Some(id) => snapshot(db, id).await?,
            None => Value::Null,
        };
        let id = run_ops(db, actor_id, product, ops).await?;
        let after = snapshot(db, id).await?;
        Ok(Preview {
            before,
            after,
            product_id: pid,
        })
    }
    .await;
    if nested {
        sqlx::query("ROLLBACK TO SAVEPOINT catalog_preview")
            .execute(&mut *db)
            .await?;
        sqlx::query("RELEASE SAVEPOINT catalog_preview")
            .execute(&mut *db)
            .await?;
    } else {
        sqlx::query("ROLLBACK").execute(&mut *db).await?;
    }
    res
}

// ─── what the card says ───────────────────────────────────────────────────────

fn money(cur: &str, amount: &Value) -> String {
    format!("{cur} {}", super::cards::money(amount))
}

/// A field value as the card shows it.
fn show(key: &str, v: &Value) -> String {
    match v {
        Value::Null => "(empty)".into(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.into(),
        Value::String(s) if s.is_empty() => "(empty)".into(),
        Value::String(s) => s.clone(),
        Value::Number(_) if key == "cost_price" => super::cards::money(v),
        Value::Number(n) => n
            .to_string()
            .parse::<Decimal>()
            .map(|d| d.normalize().to_string())
            .unwrap_or_else(|_| n.to_string()),
        Value::Array(a) if a.is_empty() => "(none)".into(),
        Value::Array(a) => a.iter().map(|x| show("", x)).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

fn label(key: &str) -> String {
    key.replace('_', " ")
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn price_text(r: &Value) -> String {
    let to = s(r, "valid_to");
    format!(
        "{} from {}{}",
        money(&s(r, "currency"), &r["amount"]),
        s(r, "valid_from"),
        if to.is_empty() {
            " (open-ended)".to_string()
        } else {
            format!(" to {to}")
        }
    )
}

/// "5.00" -> "5".
fn num(raw: &str) -> String {
    raw.parse::<Decimal>()
        .map(|d| d.normalize().to_string())
        .unwrap_or_else(|_| raw.to_string())
}

fn tiers_text(t: &Value) -> String {
    let rows = t.as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return "none".into();
    }
    rows.iter()
        .map(|r| {
            let min = num(&s(r, "min_qty"));
            let max = s(r, "max_qty");
            let range = if max.is_empty() {
                format!("{min}+")
            } else {
                format!("{min}-{}", num(&max))
            };
            format!("{range}: {}%", num(&s(r, "discount_pct")))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// "NAME (SKU)".
pub fn product_label(snap: &Value) -> String {
    format!(
        "{} ({})",
        s(&snap["product"], "name"),
        s(&snap["product"], "sku")
    )
}

/// The before / after lines of a preview.
pub fn change_lines(p: &Preview) -> Vec<String> {
    let mut lines = vec![];
    let after = &p.after;
    let before = &p.before;
    if before.is_null() {
        lines.push(format!("New product: {}", product_label(after)));
        if let Some(m) = after["product"].as_object() {
            for k in writes::field_names() {
                let v = m.get(*k).cloned().unwrap_or(Value::Null);
                let skip = matches!(&v, Value::Null)
                    || matches!(&v, Value::String(x) if x.is_empty())
                    || matches!(&v, Value::Array(a) if a.is_empty());
                if !skip && *k != "name" {
                    lines.push(format!("{}: {}", label(k), show(k, &v)));
                }
            }
        }
    } else {
        lines.push(format!("Product: {}", product_label(before)));
        let (b, a) = (&before["product"], &after["product"]);
        let retired = |x: &Value| {
            !(x["is_active"].as_bool().unwrap_or(true)
                && x["is_orderable"].as_bool().unwrap_or(true))
        };
        if retired(b) != retired(a) {
            lines.push(format!(
                "Status: {} -> {}",
                if retired(b) { "retired" } else { "active" },
                if retired(a) { "retired" } else { "active" }
            ));
        }
        if let Some(m) = a.as_object() {
            for (k, v) in m {
                if k == "id" || k == "is_active" || k == "is_orderable" {
                    continue;
                }
                let old = b.get(k).cloned().unwrap_or(Value::Null);
                if &old != v {
                    lines.push(format!("{}: {} -> {}", label(k), show(k, &old), show(k, v)));
                }
            }
        }
    }
    // price rows by id
    let rows = |x: &Value| x["prices"].as_array().cloned().unwrap_or_default();
    let (old_rows, new_rows) = (rows(before), rows(after));
    for r in &old_rows {
        match new_rows.iter().find(|n| n["id"] == r["id"]) {
            None => lines.push(format!("Price removed: {}", price_text(r))),
            Some(n) if n != r => {
                lines.push(format!("Price: {} -> {}", price_text(r), price_text(n)))
            }
            _ => {}
        }
    }
    for n in &new_rows {
        if !old_rows.iter().any(|r| r["id"] == n["id"]) {
            lines.push(format!("New price: {}", price_text(n)));
        }
    }
    if before["tiers"] != after["tiers"]
        && !(before.is_null() && tiers_text(&after["tiers"]) == "none")
    {
        let old = if before.is_null() {
            "none".to_string()
        } else {
            tiers_text(&before["tiers"])
        };
        lines.push(format!(
            "Volume tiers (private): {old} -> {}",
            tiers_text(&after["tiers"])
        ));
    }
    lines
}

/// The open quotations (draft or sent) with a line for this product: number, customer, status and the
/// price on the quote, by number.
pub async fn open_quotations(db: &mut PgConnection, product_id: Uuid) -> ApiResult<Vec<String>> {
    let rows: Vec<(Uuid, Decimal, Option<String>)> = sqlx::query_as(
        "SELECT q.id, qi.unit_price, qi.currency FROM quotation_items qi JOIN quotations q ON q.id = qi.quotation_id \
         WHERE qi.product_id = $1 AND q.is_deleted = false AND q.status IN ('draft', 'sent') \
         ORDER BY q.number, q.revision, qi.sort_order",
    )
    .bind(product_id)
    .fetch_all(&mut *db)
    .await?;
    let mut out = vec![];
    for (qid, price, cur) in rows {
        let q: Quotation = sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
            .bind(qid)
            .fetch_one(&mut *db)
            .await?;
        let who = super::proposal_quotations::customer_of(db, &q)
            .await?
            .unwrap_or_else(|| "no customer".into());
        let cur = cur.or(q.currency.clone()).unwrap_or_default();
        out.push(format!(
            "{} ({who}, {}): {} on the quote",
            super::proposal_quotations::quote_number(&q),
            q.status.as_deref().unwrap_or("draft"),
            money(&cur, &json!(price.normalize().to_string()))
        ));
    }
    Ok(out)
}

/// Every line of the card: the changes, the open quotations and the price list.
pub async fn card_lines(db: &mut PgConnection, p: &Preview, ops: &[Op]) -> ApiResult<Vec<String>> {
    let mut lines = change_lines(p);
    if let Some(pid) = p.product_id {
        if ops.iter().any(Op::touches_quotes) {
            let quotes = open_quotations(db, pid).await?;
            if quotes.is_empty() {
                lines.push("Open quotations with this product: none".into());
            } else {
                lines.push(format!(
                    "Open quotations with this product: {}",
                    quotes.join("; ")
                ));
                lines.push(QUOTES_KEEP.into());
            }
        }
    }
    if ops.iter().any(Op::refreshes_price_list) {
        lines.push(PRICE_LIST.into());
    }
    Ok(lines)
}

/// The card's one-line summary.
pub fn summary(p: &Preview, ops: &[Op]) -> String {
    let what: Vec<&str> = ops.iter().map(Op::name).collect();
    if p.before.is_null() {
        format!("New catalog product {}", product_label(&p.after))
    } else {
        format!(
            "Catalog change for {}: {}",
            product_label(&p.before),
            what.join(", ").replace('_', " ")
        )
    }
}

// ─── the action ───────────────────────────────────────────────────────────────

fn payload_ops(db_today: NaiveDate, p: &Value) -> ApiResult<Vec<Op>> {
    let changes = p
        .get("changes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    parse_ops(&changes, db_today).map_err(|e| ApiError::http(422, e))
}

async fn payload_product(db: &mut PgConnection, p: &Value) -> ApiResult<Option<ProductCatalog>> {
    match p.get("product_id").and_then(Value::as_str).map(str::trim) {
        Some(id) if !id.is_empty() => Ok(Some(writes::load_product(db, id).await?)),
        _ => Ok(None),
    }
}

async fn describe(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    let today = granary_core::clock::today_local(db).await?;
    let actor_id = actor.map(|a| a.id).unwrap_or_else(Uuid::nil);
    let worked = async {
        let ops = payload_ops(today, p)?;
        let product = payload_product(db, p).await?;
        let pv = preview(db, actor_id, product, &ops).await?;
        let lines = card_lines(db, &pv, &ops).await?;
        Ok::<_, ApiError>((summary(&pv, &ops), lines, pv))
    }
    .await;
    Ok(match worked {
        Ok((label, lines, pv)) => json!({"label": label, "lines": lines,
                                         "before": pv.before, "after": pv.after}),
        Err(e) => json!({"label": "Catalog change",
                         "lines": [format!("Can't be applied as it stands: {}", detail(&e))]}),
    })
}

/// An error's text without the status.
pub fn detail(e: &ApiError) -> String {
    match e {
        ApiError::Http {
            detail: Value::String(s),
            ..
        } => s.clone(),
        ApiError::App { detail, .. } => detail.clone(),
        other => crate::ai::tools::catalog::exc_str(other),
    }
}

/// A new internal price list version (as the editor's "Regenerate"), in a savepoint: Ok(the version label)
/// or the reason it failed (the catalog change stays).
async fn refresh_price_list(db: &mut PgConnection, actor: &AuthUser) -> Result<String, String> {
    let nested = sqlx::query("SAVEPOINT catalog_price_list")
        .execute(&mut *db)
        .await
        .is_ok();
    let res = crate::doc_library::price_list::generate(db, actor).await;
    match res {
        Ok((_doc, v)) => {
            if nested {
                let _ = sqlx::query("RELEASE SAVEPOINT catalog_price_list")
                    .execute(&mut *db)
                    .await;
            }
            super::commit(db).await.map_err(|e| detail(&e))?;
            Ok(v.label.clone())
        }
        Err(e) => {
            if nested {
                let _ = sqlx::query("ROLLBACK TO SAVEPOINT catalog_price_list")
                    .execute(&mut *db)
                    .await;
                let _ = sqlx::query("RELEASE SAVEPOINT catalog_price_list")
                    .execute(&mut *db)
                    .await;
            }
            Err(detail(&e))
        }
    }
}

/// Applies catalog.change. Commits.
async fn apply(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    let actor = super::actions::need_actor(actor)?;
    if !is_owner(db, actor).await? {
        return Err(ApiError::http(403, ONLY_OWNER));
    }
    let today = granary_core::clock::today_local(db).await?;
    let ops = payload_ops(today, p)?;
    let product = payload_product(db, p).await?;
    let before = match &product {
        Some(pr) => snapshot(db, pr.id).await?,
        None => Value::Null,
    };
    if fingerprint(&before) != p.get("before").and_then(Value::as_str).unwrap_or("") {
        return Err(ApiError::http(409, STALE));
    }
    let quotes_pid = product.as_ref().map(|x| x.id);
    let pid = run_ops(db, actor.id, product, &ops).await?;
    let after = snapshot(db, pid).await?;
    super::commit(db).await?;
    let name = product_label(&after);
    let mut out = Map::new();
    out.insert("product_id".into(), json!(pid.to_string()));
    out.insert("sku".into(), after["product"]["sku"].clone());
    let mut said = vec![if before.is_null() {
        format!("New catalog product {name} saved")
    } else {
        format!("Catalog updated: {name}")
    }];
    if let Some(qp) = quotes_pid {
        if ops.iter().any(Op::touches_quotes) {
            let quotes = open_quotations(db, qp).await?;
            if !quotes.is_empty() {
                said.push(format!(
                    "{} open quotation(s) keep their price: {}",
                    quotes.len(),
                    quotes.join("; ")
                ));
            }
            out.insert("open_quotations".into(), json!(quotes));
        }
    }
    if ops.iter().any(Op::refreshes_price_list) {
        match refresh_price_list(db, actor).await {
            Ok(v) => {
                said.push(format!("Internal price list refreshed ({v})"));
                out.insert("price_list".into(), json!(v));
            }
            Err(why) => {
                said.push(format!("The internal price list was not refreshed: {why}"));
                out.insert("price_list_error".into(), json!(why));
            }
        }
    }
    out.insert("label".into(), json!(said.join(". ")));
    Ok(Value::Object(out))
}
