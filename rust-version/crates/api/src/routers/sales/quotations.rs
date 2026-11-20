//! sales.py "Quotations" and "Quotation Line Item CRUD" sections (and the quotation routes after the
//! invoice section: status, send, record-po, delete).

use crate::extract::pg_uuid;
use std::collections::{HashMap, HashSet};

use axum::extract::Path;
use axum::response::Response;
use chrono::{Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::{currency_places, require_int};
use granary_core::validation::v::{self, DecC, NumC};
use granary_core::validation::{Cx, PyJson, PyObj, PyQuery, Validate};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::crm::{Contact, Organization};
use granary_db::rows::sales::{ProductCatalog, Quotation, QuotationItem};
use granary_services::approvals::engine;
use granary_services::crm::{deal_hooks, followups};
use granary_services::pricing::{self, LineIn, PricedLine, PricedResult};

use super::{
    commit, db_str, fromisoformat_or_422, has, need, perm_403, py_str, truthy, WithSet, MONEY_MAX,
    QTY_MAX,
};
use crate::resp::{self, date_iso, date_iso_opt, dec_float, dec_float_opt, iso};

fn opt_dec(c: DecC) -> impl Fn(&PyObj, &mut Cx) -> Option<Option<Decimal>> {
    v::nullable(v::decimal_c(c))
}

/// An optional text no longer than its `varchar(n)` column.
fn opt_str_max(n: usize) -> impl Fn(&PyObj, &mut Cx) -> Option<Option<String>> {
    v::nullable(v::str_c(v::StrC::new().max_length(n)))
}

granary_core::py_model! {
    /// `class QuotationItemInput(BaseModel)`.
    pub struct QuotationItemInput {
        /// client-side id; add-ons name their parent's key
        pub key: Option<String> = None,
        pub product_id: Option<String> = None,
        /// line name; defaults to the catalog name
        pub name: Option<String> = None => opt_str_max(255),
        pub description: Option<String> = None,
        pub quantity: Decimal = Decimal::ONE => v::decimal_c(DecC::new().gt("0").le(QTY_MAX)),
        /// omitted on a catalog line: its price
        pub unit_price: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le(MONEY_MAX)),
        pub unit: Option<String> = None => opt_str_max(30),
        /// omitted: the volume tier (or 0)
        pub discount_percentage: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le("100")),
        pub discount_type: Option<String> = None,
        pub is_optional: bool = false,
        pub sort_order: Option<i64> = None,
        /// the line this add-on attaches to (an existing item id on add-item)
        pub parent_key: Option<String> = None,
        pub term_months: Option<i64> = None => v::nullable(v::int_c(NumC::new().gt("0").le("1200"))),
        pub tax_id: Option<String> = None,
        /// custom rate when tax_id is empty
        pub tax_pct: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le("100")),
    }
}

granary_core::py_model! {
    /// The fields of `class QuotationCreate(BaseModel)` (its model validator is [`QuotationCreate`]).
    pub struct QuotationCreateFields {
        /// the column sizes of `quotations` (an over-long value is a 422, not a database error)
        pub subject: Option<String> = None => opt_str_max(500),
        pub customer_contact_id: Option<String> = None,
        pub customer_org_id: Option<String> = None,
        pub deal_id: Option<String> = None,
        pub currency: String = "USD" => v::str_c(v::StrC::new().max_length(3)),
        /// Rate to the functional currency; omitted: the recorded rate for the issue date (400 if none)
        pub exchange_rate: Option<Decimal> = None
            => opt_dec(DecC::new().gt("0").max_digits(24).decimal_places(12)),
        /// Defaults to today
        pub issue_date: Option<String> = None,
        /// Defaults to issue_date + validity_days
        pub valid_until: Option<String> = None,
        pub validity_days: i64 = 14,
        pub payment_terms: Option<Map<String, Value>> = None,
        /// CIP | DAP | EXW | CIF | DDP
        pub shipping_terms: Option<String> = None => opt_str_max(50),
        /// standard | government | white_label | oem
        pub procurement_type: String = "standard" => v::str_c(v::StrC::new().max_length(20)),
        pub lead_time_days_min: Option<i64> = None,
        pub lead_time_days_max: Option<i64> = None,
        /// percentage | fixed
        pub discount_type: Option<String> = None => opt_str_max(20),
        pub discount_value: Decimal = Decimal::ZERO => v::decimal_c(DecC::new().ge("0").le(MONEY_MAX)),
        pub notes: Option<String> = None,
        pub internal_notes: Option<String> = None,
        pub items: Vec<QuotationItemInput> = Vec::new(),
        /// 1 Nov 2026: why the header discount was given (printed under the Discount line, or in the notes)
        pub discount_note: Option<String> = None,
        /// 1 Nov 2026: pdf (a Discount line) | notes_only (totals stay full, the notes state the discount)
        pub discount_display: Option<String> = None => opt_str_max(20),
    }
}

/// `class QuotationCreate(BaseModel)` with its `@model_validator(mode="after")` and `model_fields_set`.
#[derive(Debug, Clone)]
pub struct QuotationCreate {
    pub f: QuotationCreateFields,
    pub set: Vec<String>,
}

impl Validate for QuotationCreate {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let f = QuotationCreateFields::validate(v, cx)?;
        if f.discount_type.as_deref() == Some("percentage")
            && f.discount_value > Decimal::ONE_HUNDRED
        {
            cx.err("Value error, A percentage discount can't exceed 100");
            return None;
        }
        let set = match v {
            PyObj::Dict(d) => d.keys().cloned().collect(),
            _ => Vec::new(),
        };
        Some(QuotationCreate { f, set })
    }
}

granary_core::py_model! {
    /// `class QuotationItemUpdate(BaseModel)`: PATCH body for one quotation line.
    pub struct QuotationItemUpdate {
        pub name: Option<String> = None => opt_str_max(255),
        pub description: Option<String> = None,
        pub quantity: Option<Decimal> = None => opt_dec(DecC::new().gt("0").le(QTY_MAX)),
        pub unit_price: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le(MONEY_MAX)),
        pub unit: Option<String> = None => opt_str_max(30),
        pub discount_percentage: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le("100")),
        pub discount_type: Option<String> = None,
        pub is_optional: Option<bool> = None,
        pub term_months: Option<i64> = None => v::nullable(v::int_c(NumC::new().gt("0").le("1200"))),
        pub tax_id: Option<String> = None,
        pub tax_pct: Option<Decimal> = None => opt_dec(DecC::new().ge("0").le("100")),
    }
}

granary_core::py_model! {
    /// `class ReviseBody(BaseModel)`.
    pub struct ReviseBody {
        pub reason: String,
    }
}

granary_core::py_model! {
    /// `class SendQuotationBody(BaseModel)`.
    pub struct SendQuotationBody {
        pub send_email: bool = false,
        /// Defaults to contact email if omitted
        pub recipient_email: Option<String> = None,
    }
}

granary_core::py_model! {
    /// The query parameters of `GET /quotations`.
    pub struct QuotationListQuery {
        pub status: Option<String> = None,
        pub contact_id: Option<String> = None,
        pub organization_id: Option<String> = None,
        /// 10 Nov 2026: text search (number, subject, customer organization); `q` is read too
        pub search: Option<String> = None,
        pub q: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

const QUOTATION_DISCOUNT_TYPES: [&str; 6] = [
    "none",
    "volume",
    "bundle",
    "loyalty",
    "multi_year_saas",
    "owner_exception",
];
const QUOTATION_HEADER_DISCOUNT_TYPES: [&str; 3] = ["none", "percentage", "fixed"];
const QUOTATION_PROCUREMENT_TYPES: [&str; 4] = ["standard", "government", "white_label", "oem"];

/// `_validate_item_discount_type(item)`.
pub(crate) fn validate_item_discount_type(discount_type: Option<&str>) -> ApiResult<()> {
    let t = discount_type.filter(|s| !s.is_empty()).unwrap_or("none");
    if !QUOTATION_DISCOUNT_TYPES.contains(&t) {
        return Err(ApiError::http(
            400,
            "discount_type must be one of ['bundle', 'owner_exception', 'loyalty', 'multi_year_saas', 'none', 'volume']",
        ));
    }
    Ok(())
}

/// `_require_discount_permission(current_user)`: creating a quotation and discounting one are
/// different rights.
fn require_discount_permission(user: &AuthUser) -> ApiResult<()> {
    if user.is_superadmin || user.perms.contains("sales:apply_discount") {
        Ok(())
    } else {
        Err(perm_403("sales:apply_discount"))
    }
}

/// `_validate_quotation_body(body, current_user)`: header checks.
fn validate_quotation_body(body: &QuotationCreateFields, user: &AuthUser) -> ApiResult<()> {
    let p = Some(body.procurement_type.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("standard");
    if !QUOTATION_PROCUREMENT_TYPES.contains(&p) {
        return Err(ApiError::http(
            400,
            "procurement_type must be one of ['government', 'oem', 'standard', 'white_label']",
        ));
    }
    let dt = body
        .discount_type
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("none");
    if !QUOTATION_HEADER_DISCOUNT_TYPES.contains(&dt) {
        return Err(ApiError::http(
            400,
            "discount_type must be one of ['fixed', 'none', 'percentage']",
        ));
    }
    for item in &body.items {
        validate_item_discount_type(item.discount_type.as_deref())?;
    }
    if let Some(d) = body.discount_display.as_deref().filter(|d| !d.is_empty()) {
        if !QUOTATION_DISCOUNT_DISPLAYS.contains(&d) {
            return Err(ApiError::http(
                400,
                "discount_display must be one of ['notes_only', 'pdf']",
            ));
        }
    }
    // the discount permission is checked by check_header_discount (the Owner may too, 1 Nov 2026)
    let _ = user;
    Ok(())
}

const QUOTATION_DISCOUNT_DISPLAYS: [&str; 2] = ["pdf", "notes_only"];

/// What a quotation saves of its header discount: (discount_type, discount_value, discount_note,
/// discount_display).
type HeaderDiscount = (Option<String>, Decimal, Option<String>, String);

/// 1 Nov 2026: the header discount as it is saved. With notes_only nothing is taken off (the note states the
/// discount). On an edit, a note or display the body does not send keeps the stored one.
fn header_discount(body: &QuotationCreate, stored: Option<&Quotation>) -> HeaderDiscount {
    let b = &body.f;
    let sent = |k: &str| stored.is_none() || body.set.iter().any(|s| s == k);
    let note = if sent("discount_note") {
        b.discount_note.clone()
    } else {
        stored.and_then(|q| q.discount_note.clone())
    }
    .map(|n| n.trim().to_string())
    .filter(|n| !n.is_empty());
    let display = if sent("discount_display") {
        b.discount_display.clone()
    } else {
        stored.and_then(|q| q.discount_display.clone())
    }
    .filter(|d| d == "notes_only")
    .unwrap_or_else(|| "pdf".into());
    if display == "notes_only" {
        return (Some("none".into()), Decimal::ZERO, note, display);
    }
    (b.discount_type.clone(), b.discount_value, note, display)
}

/// 1 Nov 2026: a discount (or a new discount note) needs the pricing permission or the Owner.
async fn check_header_discount(
    db: &mut PgConnection,
    user: &AuthUser,
    saved: &HeaderDiscount,
    stored: Option<&Quotation>,
) -> ApiResult<()> {
    let dt = saved
        .0
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("none");
    let old_note = stored
        .and_then(|q| q.discount_note.clone())
        .filter(|n| !n.trim().is_empty());
    let needs =
        (saved.1 > Decimal::ZERO && dt != "none") || (saved.2.is_some() && saved.2 != old_note);
    if needs && !granary_services::nary::quotation_updates::can_discount(db, user).await? {
        return Err(perm_403("sales:apply_discount"));
    }
    Ok(())
}

fn nonempty(s: &Option<String>) -> Option<String> {
    s.clone().filter(|x| !x.is_empty())
}

/// `_line_in(item, idx)`.
fn line_in(item: &QuotationItemInput, idx: usize) -> LineIn {
    LineIn {
        key: nonempty(&item.key).unwrap_or_else(|| format!("line-{idx}")),
        product_id: nonempty(&item.product_id),
        name: item.name.clone(),
        description: item.description.clone(),
        unit: item.unit.clone(),
        quantity: item.quantity,
        unit_price: item.unit_price,
        discount_pct: item.discount_percentage,
        discount_type: item.discount_type.clone(),
        is_optional: item.is_optional,
        parent_key: nonempty(&item.parent_key),
        term_months: item.term_months,
        tax_id: nonempty(&item.tax_id),
        tax_pct: item.tax_pct,
        sort_order: item.sort_order,
    }
}

/// The fields of a saved line a PATCH changes (`body.model_dump(exclude_unset=True)`).
#[derive(Clone, Debug, Default)]
pub(crate) struct ItemChanges {
    name: Option<Option<String>>,
    description: Option<Option<String>>,
    quantity: Option<Option<Decimal>>,
    unit_price: Option<Option<Decimal>>,
    unit: Option<Option<String>>,
    discount_percentage: Option<Option<Decimal>>,
    discount_type: Option<Option<String>>,
    is_optional: Option<Option<bool>>,
    term_months: Option<Option<i64>>,
    tax_id: Option<Option<String>>,
    tax_pct: Option<Option<Decimal>>,
}

/// `_stored_line_in(item, override)`: a saved line as pricing input. Catalog-priced lines follow the
/// catalog again; typed prices and every granted discount stay.
fn stored_line_in(item: &QuotationItem, o: Option<&ItemChanges>) -> LineIn {
    let empty = ItemChanges::default();
    let o = o.unwrap_or(&empty);
    let rule = item.price_rule.as_deref();
    let follows_catalog =
        item.product_id.is_some() && rule.is_some_and(|r| pricing::LIST_RULES.contains(&r));
    let price = match &o.unit_price {
        Some(p) => *p,
        None if follows_catalog => None,
        None => Some(item.unit_price),
    };
    let disc = match &o.discount_percentage {
        Some(d) => *d,
        None if follows_catalog && D(item.discount_percentage).is_zero() => None,
        // A discount granted within the tier is the Owner's choice now: it stays like any other.
        None => item.discount_percentage,
    };
    let (tax_id, tax_pct): (Option<String>, Option<Decimal>) =
        if o.tax_id.is_some() || o.tax_pct.is_some() {
            (o.tax_id.clone().flatten(), o.tax_pct.flatten())
        } else if item.tax_id.is_none() {
            // a saved line keeps "no tax" / its custom rate
            (None, Some(D(item.tax_pct)))
        } else {
            (item.tax_id.map(|u| u.to_string()), Some(item.tax_pct))
        };
    LineIn {
        key: item.id.to_string(),
        product_id: item.product_id.map(|u| u.to_string()),
        name: match &o.name {
            Some(n) => n.clone(),
            None => Some(item.name.clone()),
        },
        description: o
            .description
            .clone()
            .unwrap_or_else(|| item.description.clone()),
        unit: o.unit.clone().unwrap_or_else(|| item.unit.clone()),
        quantity: o.quantity.flatten().unwrap_or(item.quantity),
        unit_price: price,
        discount_pct: disc,
        discount_type: o
            .discount_type
            .clone()
            .unwrap_or_else(|| item.discount_type.clone()),
        is_optional: match o.is_optional {
            Some(b) => b.unwrap_or(false),
            None => item.is_optional.unwrap_or(false),
        },
        parent_key: item.parent_item_id.map(|u| u.to_string()),
        term_months: o
            .term_months
            .unwrap_or_else(|| item.term_months.map(i64::from)),
        tax_id: tax_id.filter(|s| !s.is_empty()),
        tax_pct,
        sort_order: item.sort_order.map(i64::from),
    }
}

/// What `_price` reads of a quotation-like object.
pub(crate) struct PriceCtx<'a> {
    pub currency: Option<&'a str>,
    pub issue_date: Option<NaiveDate>,
    pub customer_org_id: Option<String>,
    pub discount_type: Option<&'a str>,
    pub discount_value: Option<Decimal>,
}

impl<'a> PriceCtx<'a> {
    fn of_row(q: &'a Quotation) -> PriceCtx<'a> {
        PriceCtx {
            currency: q.currency.as_deref(),
            issue_date: Some(q.issue_date),
            customer_org_id: q.customer_org_id.map(|u| u.to_string()),
            discount_type: q.discount_type.as_deref(),
            discount_value: q.discount_value,
        }
    }
}

/// `_price(db, quotation_like, lines, current_user, check_keys=None)`.
async fn price(
    db: &mut PgConnection,
    ctx: &PriceCtx<'_>,
    lines: &[LineIn],
    user: &AuthUser,
    check_keys: Option<&HashSet<String>>,
) -> ApiResult<PricedResult> {
    let Some(currency) = ctx.currency else {
        // pricing does `currency.upper()`: AttributeError on None
        return Err(ApiError::internal(anyhow::anyhow!(
            "AttributeError: 'NoneType' object has no attribute 'upper'"
        )));
    };
    let on_date = match ctx.issue_date {
        Some(d) => d,
        None => today_local(db).await?,
    };
    let org = ctx.customer_org_id.clone().filter(|s| !s.is_empty());
    pricing::price_lines(
        db,
        currency,
        on_date,
        org.as_deref(),
        lines,
        Some(user),
        true,
        true,
        check_keys,
        ctx.discount_type,
        ctx.discount_value,
    )
    .await
}

/// `_depth_order(lines)`: parents before add-ons (stable).
fn depth_order(lines: &[PricedLine]) -> Vec<&PricedLine> {
    let by_key: HashMap<&str, &PricedLine> = lines.iter().map(|p| (p.key.as_str(), p)).collect();
    fn depth(p: &PricedLine, n: usize, total: usize, by_key: &HashMap<&str, &PricedLine>) -> usize {
        let parent = p
            .parent_key
            .as_deref()
            .filter(|k| !k.is_empty())
            .and_then(|k| by_key.get(k));
        match parent {
            None => 0,
            Some(_) if n > total => 0,
            Some(parent) => 1 + depth(parent, n + 1, total, by_key),
        }
    }
    let mut out: Vec<&PricedLine> = lines.iter().collect();
    out.sort_by_key(|p| depth(p, 0, lines.len(), &by_key));
    out
}

const ITEM_SNAPSHOT_COLS: &str = "product_id, sku, name, description, unit, quantity, unit_price, \
     list_unit_price, discount_percentage, discount_type, amount, is_optional, sale_model, price_metric, \
     rate_period_months, billing_interval_months, term_months, price_rule, tax_id, tax_code, tax_pct, tax_amount";

/// INSERT one QuotationItem with a priced line's snapshot (`_snapshot`). Unset (None) values take the
/// column's Python default (unit "Unit", currency "USD").
async fn insert_item(
    db: &mut PgConnection,
    id: Uuid,
    quotation_id: Uuid,
    currency: Option<&str>,
    sort_order: i64,
    parent_item_id: Option<Uuid>,
    p: &PricedLine,
) -> ApiResult<()> {
    let sql = format!(
        "INSERT INTO quotation_items (id, quotation_id, currency, sort_order, parent_item_id, {ITEM_SNAPSHOT_COLS}) \
         VALUES ($1, $2, COALESCE($3, 'USD'), $4::int8, $5, $6::uuid, $7, $8, $9, COALESCE($10, 'Unit'), $11, $12, \
         $13, $14, $15, $16, $17, $18, $19, $20::int8, $21::int8, $22::int8, $23, $24::uuid, $25, $26, $27)"
    );
    sqlx::query(&sql)
        .bind(id)
        .bind(quotation_id)
        .bind(currency)
        .bind(sort_order)
        .bind(parent_item_id)
        .bind(&p.product_id)
        .bind(&p.sku)
        .bind(&p.name)
        .bind(&p.description)
        .bind(&p.unit)
        .bind(p.quantity)
        .bind(p.unit_price)
        .bind(p.list_unit_price)
        .bind(p.discount_pct)
        .bind(&p.discount_type)
        .bind(p.amount)
        .bind(p.is_optional)
        .bind(&p.sale_model)
        .bind(&p.price_metric)
        .bind(p.rate_period_months)
        .bind(p.billing_interval_months)
        .bind(p.term_months)
        .bind(&p.price_rule)
        .bind(&p.tax_id)
        .bind(&p.tax_code)
        .bind(p.tax_pct)
        .bind(p.tax_amount)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// `_snapshot(item, p)` on a saved line: an UPDATE (updated_at only moves when a value changed, as
/// SQLAlchemy only writes changed rows). An attribute set to None becomes NULL on update.
async fn update_item_snapshot(db: &mut PgConnection, id: Uuid, p: &PricedLine) -> ApiResult<()> {
    let cols: Vec<&str> = ITEM_SNAPSHOT_COLS.split(", ").collect();
    let ph = |i: usize, c: &str| -> String {
        match c {
            "product_id" | "tax_id" => format!("${i}::uuid"),
            "rate_period_months" | "billing_interval_months" | "term_months" => {
                format!("${i}::int8::int4")
            }
            _ => format!("${i}"),
        }
    };
    let sets: Vec<String> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{c} = {}", ph(i + 2, c)))
        .collect();
    let olds: Vec<String> = cols
        .iter()
        .map(|c| format!("quotation_items.{c}"))
        .collect();
    let news: Vec<String> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| match *c {
            "sku" | "name" | "unit" | "discount_type" | "sale_model" | "price_metric"
            | "price_rule" | "tax_code" => format!("${}::varchar", i + 2),
            _ => ph(i + 2, c),
        })
        .collect();
    let sql = format!(
        "UPDATE quotation_items SET {}, updated_at = CASE WHEN ({}) IS DISTINCT FROM ({}) THEN now() \
         ELSE quotation_items.updated_at END WHERE quotation_items.id = $1",
        sets.join(", "),
        olds.join(", "),
        news.join(", ")
    );
    sqlx::query(&sql)
        .bind(id)
        .bind(&p.product_id)
        .bind(&p.sku)
        .bind(&p.name)
        .bind(&p.description)
        .bind(&p.unit)
        .bind(p.quantity)
        .bind(p.unit_price)
        .bind(p.list_unit_price)
        .bind(p.discount_pct)
        .bind(&p.discount_type)
        .bind(p.amount)
        .bind(p.is_optional)
        .bind(&p.sale_model)
        .bind(&p.price_metric)
        .bind(p.rate_period_months)
        .bind(p.billing_interval_months)
        .bind(p.term_months)
        .bind(&p.price_rule)
        .bind(&p.tax_id)
        .bind(&p.tax_code)
        .bind(p.tax_pct)
        .bind(p.tax_amount)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// `_write_new_items(db, quotation, res)`: one QuotationItem per priced line (parents first).
async fn write_new_items(
    db: &mut PgConnection,
    quotation_id: Uuid,
    currency: Option<&str>,
    res: &PricedResult,
) -> ApiResult<()> {
    let mut ids: HashMap<&str, Uuid> = HashMap::new();
    let mut order: HashMap<&str, usize> = HashMap::new();
    for (idx, p) in res.lines.iter().enumerate() {
        ids.insert(p.key.as_str(), Uuid::new_v4());
        order.insert(p.key.as_str(), idx);
    }
    for p in depth_order(&res.lines) {
        let parent = match p.parent_key.as_deref().filter(|k| !k.is_empty()) {
            Some(k) => Some(
                *ids.get(k)
                    .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{k}'")))?,
            ),
            None => None,
        };
        let sort = p.sort_order.unwrap_or(order[p.key.as_str()] as i64);
        insert_item(
            db,
            ids[p.key.as_str()],
            quotation_id,
            currency,
            sort,
            parent,
            p,
        )
        .await?;
    }
    Ok(())
}

/// `_apply_quotation_totals(quotation, res, places)`.
#[derive(Clone, Debug)]
pub(crate) struct Totals {
    subtotal: Decimal,
    discount_amount: Decimal,
    tax_amount: Decimal,
    tax_lines: Value,
    total: Decimal,
    rounded_total: Decimal,
    total_quantity: i64,
}

fn totals(res: &PricedResult, places: u32) -> Totals {
    let qty: Decimal = res.lines.iter().map(|p| p.quantity).sum();
    Totals {
        subtotal: res.subtotal,
        discount_amount: res.discount_amount,
        tax_amount: res.tax_amount,
        tax_lines: Value::Array(res.tax_lines.clone()),
        total: res.total,
        // the amount payable in the currency's minor unit
        rounded_total: quantize_money(res.total, places),
        total_quantity: qty.trunc().try_into().unwrap_or(0),
    }
}

/// Writes the totals onto a saved quotation (only a changed value bumps updated_at).
async fn save_totals(db: &mut PgConnection, id: Uuid, t: &Totals) -> ApiResult<()> {
    sqlx::query(
        "UPDATE quotations SET subtotal = $2, discount_amount = $3, tax_amount = $4, tax_lines = $5, total = $6, \
         rounded_total = $7, total_quantity = $8::int8, updated_at = CASE WHEN (subtotal, discount_amount, \
         tax_amount, tax_lines, total, rounded_total, total_quantity) IS DISTINCT FROM ($2, $3, $4, $5::jsonb, $6, \
         $7, $8::int8::int4) THEN now() ELSE updated_at END WHERE id = $1",
    )
    .bind(id)
    .bind(t.subtotal)
    .bind(t.discount_amount)
    .bind(t.tax_amount)
    .bind(&t.tax_lines)
    .bind(t.total)
    .bind(t.rounded_total)
    .bind(t.total_quantity)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `_quotation_exchange_rate(db, currency, given, issue_date)`: the rate entered on the form, else the
/// rate on record for the issue date (a 400 when there is none), and 1 for the functional currency.
async fn quotation_exchange_rate(
    db: &mut PgConnection,
    currency: &str,
    given: Option<Decimal>,
    issue_date: NaiveDate,
) -> ApiResult<Decimal> {
    let functional = granary_services::fx::functional_currency(db).await?;
    if currency.to_uppercase() == functional {
        return Ok(Decimal::ONE);
    }
    if let Some(g) = given {
        if g > Decimal::ZERO {
            return Ok(g);
        }
    }
    granary_services::fx::require_rate(db, currency, &functional, Some(issue_date)).await
}

/// `_deal_id(db, raw)`: None, or an existing deal (400 otherwise).
async fn deal_id(db: &mut PgConnection, raw: Option<&str>) -> ApiResult<Option<Uuid>> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let id = crate::extract::parse_uuid(raw, "deal_id")?;
    let found: Option<(Uuid,)> = sqlx::query_as(
        "SELECT deals.id FROM deals WHERE deals.id = $1 AND deals.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    if found.is_none() {
        return Err(ApiError::http(400, "deal_id: no such deal"));
    }
    Ok(Some(id))
}

/// A live (not deleted) quotation by the path string; `for_update` locks the row.
pub(crate) async fn load_quotation(
    db: &mut PgConnection,
    quotation_id: &str,
    for_update: bool,
) -> ApiResult<Quotation> {
    let id = pg_uuid(quotation_id)?;
    let sql = format!(
        "SELECT * FROM quotations WHERE quotations.id = $1 AND quotations.is_deleted = false{}",
        if for_update { " FOR UPDATE" } else { "" }
    );
    let q: Option<Quotation> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    q.ok_or_else(|| ApiError::not_found("Quotation", Some(quotation_id)))
}

/// `_emit_quotation(db, type_, quotation, user, **extra)` (the services port).
pub(crate) async fn emit_quotation(
    db: &mut PgConnection,
    type_: &str,
    q: &Quotation,
    user: &AuthUser,
    extra: Map<String, Value>,
) -> ApiResult<()> {
    granary_services::verification::delivery::_emit_quotation(db, type_, q, user, extra).await
}

/// `GET /quotation-defaults`: validity and payment-term days, both from Settings.
pub async fn quotation_defaults(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    if !(has(&user, "sales:create_quotation") || has(&user, "sales:edit_quotation")) {
        return Err(perm_403("sales:create_quotation"));
    }
    let mut conn = db.conn().await?;
    let validity = require_int(
        &mut conn,
        "quotation_validity_days",
        "Sales & CRM \u{2192} Quotations",
    )
    .await?;
    let terms = require_int(
        &mut conn,
        "payment_terms_default",
        "Company & branding \u{2192} Document numbers",
    )
    .await?;
    Ok(resp::json(
        json!({"validity_days": validity, "payment_terms_days": terms}),
    ))
}

/// 2 Nov 2026: `GET /tax-notes`: the "Prices exclude tax" sentence and the phrases the notes/tax gate reads
/// (the quotation and invoice editors switch and warn with them).
pub async fn tax_notes(CurrentUser(_user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let r = granary_services::quotation_tax::rules(&mut conn).await?;
    Ok(resp::json(json!({
        "note": r.note, "exclusive_phrases": r.exclusive, "inclusive_phrases": r.inclusive,
    })))
}

/// `GET /quotations`.
pub async fn list_quotations(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(p): PyQuery<QuotationListQuery>,
) -> ApiResult<Response> {
    need(&user, "sales:view_quotations")?;
    let mut conn = db.conn().await?;
    let mut sql = String::from("SELECT * FROM quotations WHERE quotations.is_deleted = false");
    let mut uuids: Vec<Uuid> = Vec::new();
    let mut status: Option<&str> = None;
    if let Some(s) = p.status.as_deref().filter(|s| !s.is_empty()) {
        status = Some(s);
        sql.push_str(" AND quotations.status = $1");
    } else {
        // 1 Nov 2026: a cancelled quotation is kept for history, listed only when asked for
        sql.push_str(" AND quotations.status IS DISTINCT FROM 'cancelled'");
    }
    let base = if status.is_some() { 1 } else { 0 };
    let contact = p.contact_id.as_deref().filter(|s| !s.is_empty());
    let org = p.organization_id.as_deref().filter(|s| !s.is_empty());
    // Scope to a contact and/or their organization (the inbox/WhatsApp CRM panel).
    match (contact, org) {
        (Some(c), Some(o)) => {
            uuids.push(pg_uuid(c)?);
            uuids.push(pg_uuid(o)?);
            sql.push_str(&format!(
                " AND (quotations.customer_contact_id = ${} OR quotations.customer_org_id = ${})",
                base + 1,
                base + 2
            ));
        }
        (Some(c), None) => {
            uuids.push(pg_uuid(c)?);
            sql.push_str(&format!(
                " AND quotations.customer_contact_id = ${}",
                base + 1
            ));
        }
        (None, Some(o)) => {
            uuids.push(pg_uuid(o)?);
            sql.push_str(&format!(" AND quotations.customer_org_id = ${}", base + 1));
        }
        (None, None) => {}
    }
    let mut n = base + uuids.len();
    // 10 Nov 2026: the list page sent `search`, which nothing read, so its search box did nothing.
    let text = p
        .search
        .as_deref()
        .or(p.q.as_deref())
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let pattern = text.map(|t| format!("%{}%", granary_compat::text::utils::escape_like(t)));
    if pattern.is_some() {
        n += 1;
        sql.push_str(&format!(
            " AND (quotations.full_number ILIKE ${n} ESCAPE '\\' OR quotations.subject ILIKE ${n} ESCAPE '\\' \
             OR quotations.customer_org_id IN (SELECT id FROM organizations WHERE name ILIKE ${n} ESCAPE '\\'))"
        ));
    }
    sql.push_str(&format!(
        " ORDER BY quotations.created_at DESC LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    ));
    let mut q = sqlx::query_as::<_, Quotation>(&sql);
    if let Some(s) = status {
        q = q.bind(s);
    }
    for u in &uuids {
        q = q.bind(u);
    }
    if let Some(pat) = &pattern {
        q = q.bind(pat);
    }
    let rows = q.bind(p.limit).bind(p.offset).fetch_all(&mut *conn).await?;
    // 17 Nov 2026: "approved" for a draft whose approval still matches it (the badge; `status` stays stored)
    let mut shown: Vec<String> = Vec::with_capacity(rows.len());
    for q in &rows {
        let st = q.status.clone().unwrap_or_default();
        shown.push(
            granary_services::approvals::engine::quotation_display_status(&mut conn, q.id, &st)
                .await?,
        );
    }
    // 21 Nov 2026: the list's Client column reads customer_org / customer_contact (it showed "-" on every row)
    let org_ids: Vec<Uuid> = rows.iter().filter_map(|q| q.customer_org_id).collect();
    let contact_ids: Vec<Uuid> = rows.iter().filter_map(|q| q.customer_contact_id).collect();
    let orgs: std::collections::HashMap<Uuid, String> = if org_ids.is_empty() {
        Default::default()
    } else {
        sqlx::query_as::<_, (Uuid, String)>("SELECT id, name FROM organizations WHERE id = ANY($1)")
            .bind(&org_ids)
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect()
    };
    let contacts: std::collections::HashMap<Uuid, String> = if contact_ids.is_empty() {
        Default::default()
    } else {
        sqlx::query_as::<_, (Uuid, String, Option<String>)>(
            "SELECT id, first_name, last_name FROM contacts WHERE id = ANY($1)",
        )
        .bind(&contact_ids)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|(id, f, l)| {
            (
                id,
                pystr::strip(&format!("{f} {}", l.unwrap_or_default())).to_string(),
            )
        })
        .collect()
    };
    let out: Vec<Value> = rows
        .iter()
        .zip(shown)
        .map(|(q, display)| {
            let org = q
                .customer_org_id
                .and_then(|id| {
                    orgs.get(&id)
                        .map(|n| json!({"id": id.to_string(), "name": n}))
                })
                .unwrap_or(Value::Null);
            let contact = q
                .customer_contact_id
                .and_then(|id| {
                    contacts
                        .get(&id)
                        .map(|n| json!({"id": id.to_string(), "full_name": n}))
                })
                .unwrap_or(Value::Null);
            json!({
                "id": q.id.to_string(), "number": q.number, "full_number": q.full_number,
                "subject": q.subject,
                "status": q.status, "display_status": display, "currency": q.currency,
                "subtotal": super::float_if(q.subtotal),
                "total": super::float_if(q.total),
                "issue_date": date_iso(&q.issue_date),
                "valid_until": date_iso(&q.valid_until),
                "customer_org_id": q.customer_org_id.map(|u| u.to_string()),
                "customer_contact_id": q.customer_contact_id.map(|u| u.to_string()),
                "customer_org": org,
                "customer_contact": contact,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// A JSONB value as SQLAlchemy writes it: None is the JSON `null`.
fn json_or_null(v: Option<Value>) -> Value {
    v.unwrap_or(Value::Null)
}

/// `valid_until`: the given date, else issue_date + validity_days (an overflow is Python's OverflowError).
fn valid_until(body: &QuotationCreateFields, issue: NaiveDate, days: i64) -> ApiResult<NaiveDate> {
    match body.valid_until.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => fromisoformat_or_422(s),
        None => Duration::try_days(days)
            .and_then(|d| issue.checked_add_signed(d))
            .ok_or_else(|| {
                ApiError::internal(anyhow::anyhow!("OverflowError: date value out of range"))
            }),
    }
}

/// `create_quotation(body, current_user, db)`: the route and what nary/actions.py calls.
pub(crate) async fn create_quotation_core(
    db: &mut PgConnection,
    body: &QuotationCreate,
    user: &AuthUser,
) -> ApiResult<Value> {
    need(user, "sales:create_quotation")?;
    let b = &body.f;
    validate_quotation_body(b, user)?;
    let disc = header_discount(body, None);
    check_header_discount(db, user, &disc, None).await?;
    let today = today_local(db).await?;
    let issue = match b.issue_date.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => fromisoformat_or_422(s)?,
        None => today,
    };
    let validity_days = if b.validity_days != 0 {
        b.validity_days
    } else {
        14
    };
    let valid_until = valid_until(b, issue, validity_days)?;
    let lines: Vec<LineIn> = b
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| line_in(it, i))
        .collect();
    let ctx = PriceCtx {
        currency: Some(&b.currency),
        issue_date: Some(issue),
        customer_org_id: b.customer_org_id.clone(),
        discount_type: disc.0.as_deref(),
        discount_value: Some(disc.1),
    };
    let priced = price(db, &ctx, &lines, user, None).await?;
    let exchange_rate = quotation_exchange_rate(db, &b.currency, b.exchange_rate, issue).await?;
    let seq =
        granary_services::numbering::currency_sequence(db, "quotation", Some(&b.currency)).await?;
    let number = granary_services::numbering::next_document_number(db, &seq).await?;
    let full_number = format!("{number}-1"); // Revision 1
    let deal = deal_id(db, b.deal_id.as_deref()).await?;
    let payment_terms = b.payment_terms.clone().map(Value::Object);
    let terms_days =
        granary_services::approvals::adapters::derive_terms_days(payment_terms.as_ref());
    let places = currency_places(db, Some(&b.currency)).await?;
    let t = totals(&priced, places);
    let id = Uuid::new_v4();
    let procurement = Some(b.procurement_type.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "standard".into());
    sqlx::query(
        "INSERT INTO quotations (id, number, full_number, revision, revision_group, status, subject, deal_id, \
         customer_org_id, customer_contact_id, currency, exchange_rate, issue_date, valid_until, validity_days, \
         payment_terms, payment_terms_days, procurement_type, shipping_terms, lead_time_days_min, \
         lead_time_days_max, discount_type, discount_value, notes, internal_notes, created_by_id, subtotal, \
         discount_amount, tax_amount, tax_lines, total, rounded_total, total_quantity, \
         customer_po_accepted_verbally, is_deleted) VALUES ($1, $2, $3, 1, $2, 'draft', $4, $5, $6::uuid, \
         $7::uuid, $8, $9, $10, $11, $12::int8, $13, $14::int8, $15, $16, $17::int8, $18::int8, $19, $20, $21, \
         $22, $23, $24, $25, $26, $27, $28, $29, $30::int8, false, false)",
    )
    .bind(id)
    .bind(&number)
    .bind(&full_number)
    .bind(&b.subject)
    .bind(deal)
    .bind(&b.customer_org_id)
    .bind(&b.customer_contact_id)
    .bind(&b.currency)
    .bind(exchange_rate)
    .bind(issue)
    .bind(valid_until)
    .bind(validity_days)
    .bind(json_or_null(payment_terms))
    .bind(terms_days)
    .bind(&procurement)
    .bind(&b.shipping_terms)
    .bind(b.lead_time_days_min)
    .bind(b.lead_time_days_max)
    .bind(&disc.0)
    .bind(disc.1)
    .bind(&b.notes)
    .bind(&b.internal_notes)
    .bind(user.id)
    .bind(t.subtotal)
    .bind(t.discount_amount)
    .bind(t.tax_amount)
    .bind(&t.tax_lines)
    .bind(t.total)
    .bind(t.rounded_total)
    .bind(t.total_quantity)
    .execute(&mut *db)
    .await?;
    write_new_items(db, id, Some(&b.currency), &priced).await?;
    sqlx::query("UPDATE quotations SET discount_note = $2, discount_display = $3 WHERE id = $1")
        .bind(id)
        .bind(&disc.2)
        .bind(&disc.3)
        .execute(&mut *db)
        .await?;
    commit(db).await?;
    Ok(
        json!({"id": id.to_string(), "number": number, "full_number": full_number, "status": "draft"}),
    )
}

/// `create_quotation(QuotationCreate(**p), actor, db)` as nary/actions.py calls it: a Pydantic error is
/// raised as is (a ValidationError, not a 422).
pub(crate) async fn create_quotation_hook(
    db: &mut PgConnection,
    body: &Value,
    actor: &AuthUser,
) -> ApiResult<Value> {
    let parsed =
        granary_core::validation::validate_value::<QuotationCreate>(&PyObj::from_json(body), None)
            .map_err(|errors| {
                let lines: Vec<String> = errors
                    .iter()
                    .map(|e| format!("{}\n  {}", e.field(), e.msg))
                    .collect();
                ApiError::internal(anyhow::anyhow!(
                    "{} validation error{} for QuotationCreate\n{}",
                    errors.len(),
                    if errors.len() == 1 { "" } else { "s" },
                    lines.join("\n")
                ))
            })?;
    create_quotation_core(db, &parsed, actor).await
}

/// `POST /quotations` (201).
pub async fn create_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<QuotationCreate>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let out = create_quotation_core(&mut conn, &body, &user).await?;
    Ok(resp::json_status(201, out))
}

/// `PATCH /quotations/{quotation_id}`: edit a DRAFT quotation: replace header fields + line items,
/// recompute totals. Sent/accepted quotations are immutable (issue a revision instead).
pub async fn update_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(body): PyJson<QuotationCreate>,
) -> ApiResult<Response> {
    need(&user, "sales:create_quotation")?;
    let mut conn = db.conn().await?;
    let out = update_quotation_core(&mut conn, &quotation_id, &body, &user).await?;
    Ok(resp::json(out))
}

/// batch 4 D3: the body of `PATCH /quotations/{quotation_id}`, also what Nary's `quotation.update` action
/// calls (through `later::nary_actions::update_quotation`).
pub(crate) async fn update_quotation_core(
    conn: &mut PgConnection,
    quotation_id: &str,
    body: &QuotationCreate,
    user: &AuthUser,
) -> ApiResult<Value> {
    need(user, "sales:create_quotation")?;
    let q = load_quotation(&mut *conn, quotation_id, false).await?;
    if q.status.as_deref() != Some("draft") {
        return Err(ApiError::http(400, "Only draft quotations can be edited."));
    }
    // 1 Nov 2026: not while the approver has it (the same message as Nary's tools)
    if let Some(r) = engine::latest_request(&mut *conn, "quotation", q.id).await? {
        if r.status == "pending" {
            let n = q.full_number.clone().unwrap_or_else(|| q.number.clone());
            return Err(ApiError::http(
                400,
                granary_services::nary::quotation_updates::pending_message(&n),
            ));
        }
    }
    let b = &body.f;
    validate_quotation_body(b, user)?;
    let disc = header_discount(body, Some(&q));
    check_header_discount(&mut *conn, user, &disc, Some(&q)).await?;
    let issue = match b.issue_date.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => fromisoformat_or_422(s)?,
        None => q.issue_date,
    };
    let validity_days = if b.validity_days != 0 {
        b.validity_days
    } else {
        q.validity_days
            .filter(|d| *d != 0)
            .map(i64::from)
            .unwrap_or(14)
    };
    let valid_until = valid_until(b, issue, validity_days)?;
    let lines: Vec<LineIn> = b
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| line_in(it, i))
        .collect();
    let ctx = PriceCtx {
        currency: Some(&b.currency),
        issue_date: Some(issue),
        customer_org_id: b.customer_org_id.clone(),
        discount_type: disc.0.as_deref(),
        discount_value: Some(disc.1),
    };
    let priced = price(&mut *conn, &ctx, &lines, user, None).await?;
    // a form that doesn't send deal_id keeps the link
    let deal = if body.set.iter().any(|k| k == "deal_id") {
        deal_id(&mut *conn, b.deal_id.as_deref()).await?
    } else {
        q.deal_id
    };
    let exchange_rate =
        quotation_exchange_rate(&mut *conn, &b.currency, b.exchange_rate, issue).await?;
    let payment_terms = b.payment_terms.clone().map(Value::Object);
    let terms_days =
        granary_services::approvals::adapters::derive_terms_days(payment_terms.as_ref());
    let procurement = Some(b.procurement_type.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "standard".into());
    sqlx::query(
        "UPDATE quotations SET subject = $2, deal_id = $3, customer_org_id = $4::uuid, customer_contact_id = $5::uuid, \
         exchange_rate = $6, currency = $7, issue_date = $8, valid_until = $9, validity_days = $10::int8, \
         payment_terms = $11, payment_terms_days = $12::int8, procurement_type = $13, shipping_terms = $14, \
         lead_time_days_min = $15::int8, lead_time_days_max = $16::int8, discount_type = $17, discount_value = $18, \
         notes = $19, internal_notes = $20, discount_note = $21, discount_display = $22, updated_at = now() \
         WHERE id = $1",
    )
    .bind(q.id)
    .bind(&b.subject)
    .bind(deal)
    .bind(&b.customer_org_id)
    .bind(&b.customer_contact_id)
    .bind(exchange_rate)
    .bind(&b.currency)
    .bind(issue)
    .bind(valid_until)
    .bind(validity_days)
    .bind(json_or_null(payment_terms))
    .bind(terms_days)
    .bind(&procurement)
    .bind(&b.shipping_terms)
    .bind(b.lead_time_days_min)
    .bind(b.lead_time_days_max)
    .bind(&disc.0)
    .bind(disc.1)
    .bind(&b.notes)
    .bind(&b.internal_notes)
    .bind(&disc.2)
    .bind(&disc.3)
    .execute(&mut *conn)
    .await?;
    // Replace items
    sqlx::query("DELETE FROM quotation_items WHERE quotation_items.quotation_id = $1")
        .bind(q.id)
        .execute(&mut *conn)
        .await?;
    write_new_items(&mut *conn, q.id, Some(&b.currency), &priced).await?;
    let places = currency_places(&mut *conn, Some(&b.currency)).await?;
    save_totals(&mut *conn, q.id, &totals(&priced, places)).await?;
    commit(&mut *conn).await?;
    Ok(json!({
        "id": q.id.to_string(), "number": q.number, "full_number": q.full_number, "status": q.status,
    }))
}

/// batch 4 D3: `update_quotation` as Nary's `quotation.update` action calls it, with the full body as JSON
/// (validated as QuotationCreate; a validation error is raised as is, like `create_quotation_hook`).
pub(crate) async fn update_quotation_hook(
    db: &mut PgConnection,
    quotation_id: &str,
    body: &Value,
    actor: &AuthUser,
) -> ApiResult<Value> {
    let parsed =
        granary_core::validation::validate_value::<QuotationCreate>(&PyObj::from_json(body), None)
            .map_err(|errors| {
                let lines: Vec<String> = errors
                    .iter()
                    .map(|e| format!("{}\n  {}", e.field(), e.msg))
                    .collect();
                ApiError::internal(anyhow::anyhow!(
                    "{} validation error{} for QuotationCreate\n{}",
                    errors.len(),
                    if errors.len() == 1 { "" } else { "s" },
                    lines.join("\n")
                ))
            })?;
    update_quotation_core(db, quotation_id, &parsed, actor).await
}

/// `_item_json(i)`.
fn item_json(i: &QuotationItem, product: Option<&ProductCatalog>) -> Value {
    json!({
        "id": i.id.to_string(), "sort_order": i.sort_order,
        "parent_item_id": i.parent_item_id.map(|u| u.to_string()),
        "name": i.name, "description": i.description, "sku": i.sku,
        "quantity": dec_float(&i.quantity), "qty": dec_float(&i.quantity),
        "unit_price": dec_float(&i.unit_price), "list_unit_price": dec_float_opt(i.list_unit_price.as_ref()),
        "unit": i.unit,
        "discount_percentage": super::float_or_0(i.discount_percentage),
        "discount_type": i.discount_type.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "none".into()),
        "discount_pct": super::float_or_0(i.discount_percentage),
        "amount": dec_float(&i.amount),
        "subtotal": super::float_or_0(Some(i.amount)),
        "is_optional": i.is_optional,
        "sale_model": i.sale_model, "price_metric": i.price_metric,
        "rate_period_months": i.rate_period_months, "billing_interval_months": i.billing_interval_months,
        "term_months": i.term_months, "price_rule": i.price_rule,
        "tax_id": i.tax_id.map(|u| u.to_string()), "tax_code": i.tax_code,
        "tax_pct": dec_float(&i.tax_pct), "tax_amount": dec_float(&i.tax_amount),
        "product_id": i.product_id.map(|u| u.to_string()),
        "product": product.map(|p| json!({"id": p.id.to_string(), "name": p.name, "sku": p.sku,
                                           "terms_note": p.terms_note,
                                           // 5 Nov 2026: a proposed catalog price is shown as such
                                           "price_status": p.price_status,
                                           "segment": p.segment})),
    })
}

/// `GET /quotations/{quotation_id}`.
pub async fn get_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "sales:view_quotations")?;
    let mut conn = db.conn().await?;
    let q = load_quotation(&mut conn, &quotation_id, false).await?;
    let display_status = granary_services::approvals::engine::quotation_display_status(
        &mut conn,
        q.id,
        q.status.as_deref().unwrap_or(""),
    )
    .await?;
    let items: Vec<QuotationItem> = sqlx::query_as(
        "SELECT * FROM quotation_items WHERE quotation_items.quotation_id = $1 ORDER BY quotation_items.sort_order",
    )
    .bind(q.id)
    .fetch_all(&mut *conn)
    .await?;
    // selectinload(QuotationItem.product)
    let mut pids: Vec<Uuid> = Vec::new();
    for i in &items {
        if let Some(p) = i.product_id {
            if !pids.contains(&p) {
                pids.push(p);
            }
        }
    }
    let products: Vec<ProductCatalog> = if pids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as("SELECT * FROM product_catalog WHERE product_catalog.id = ANY($1)")
            .bind(&pids)
            .fetch_all(&mut *conn)
            .await?
    };
    let org: Option<Organization> = match q.customer_org_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM organizations WHERE organizations.id = $1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    let contact: Option<Contact> = match q.customer_contact_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM contacts WHERE contacts.id = $1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    // Revision history: all quotations sharing this revision_group.
    let group = q
        .revision_group
        .clone()
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| q.number.clone());
    let revs: Vec<Quotation> = sqlx::query_as(
        "SELECT * FROM quotations WHERE quotations.revision_group = $1 AND quotations.is_deleted = false \
         ORDER BY quotations.revision",
    )
    .bind(&group)
    .fetch_all(&mut *conn)
    .await?;
    let revisions: Vec<Value> = revs
        .iter()
        .map(|r| {
            json!({
                "id": r.id.to_string(), "full_number": r.full_number, "revision": r.revision,
                "status": r.status, "created_at": iso(&r.created_at),
                "revision_reason": r.revision_reason, "is_current": r.id == q.id,
            })
        })
        .collect();
    // The active (non-superseded) revision, for the "superseded" banner link.
    let active = revs
        .iter()
        .find(|r| r.status.as_deref() != Some("superseded"));
    let functional = granary_services::fx::functional_currency(&mut conn).await?;
    let items_json: Vec<Value> = items
        .iter()
        .map(|i| {
            let p = i
                .product_id
                .and_then(|pid| products.iter().find(|p| p.id == pid));
            item_json(i, p)
        })
        .collect();
    let contact_json = contact.as_ref().map(|c| {
        let full = format!(
            "{} {}",
            c.first_name,
            c.last_name.clone().unwrap_or_default()
        );
        json!({"id": c.id.to_string(), "full_name": pystr::strip(&full), "email": c.email})
    });
    let out = json!({
        "id": q.id.to_string(), "number": q.number, "full_number": q.full_number, "revision": q.revision,
        "revision_group": group,
        "revisions": revisions,
        "active_revision_id": active.map(|a| a.id.to_string()),
        "revision_reason": q.revision_reason,
        "subject": q.subject,
        "status": q.status, "display_status": display_status, "currency": q.currency,
        "exchange_rate": dec_float_opt(q.exchange_rate.as_ref()),
        "functional_currency": functional,
        "subtotal": super::float_if(q.subtotal),
        "discount_type": q.discount_type,
        "procurement_type": q.procurement_type.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "standard".into()),
        "payment_terms_days": q.payment_terms_days,
        "discount_value": super::float_or_0(q.discount_value),
        "discount_amount": super::float_or_0(q.discount_amount),
        "tax_amount": super::float_or_0(q.tax_amount),
        "tax_lines": match &q.tax_lines { Some(v) if super::truthy(v) => v.clone(), _ => json!([]) },
        "total": super::float_if(q.total),
        "rounded_total": super::float_if(q.rounded_total),
        "total_quantity": q.total_quantity,
    });
    let rest = json!({
        "issue_date": date_iso(&q.issue_date),
        "valid_until": date_iso(&q.valid_until),
        "validity_days": q.validity_days,
        "payment_terms": q.payment_terms.clone().unwrap_or(Value::Null),
        "shipping_terms": q.shipping_terms,
        "lead_time_days_min": q.lead_time_days_min,
        "lead_time_days_max": q.lead_time_days_max,
        "customer_po_number": q.customer_po_number,
        "customer_po_date": date_iso_opt(q.customer_po_date.as_ref()),
        "customer_po_accepted_verbally": q.customer_po_accepted_verbally,
        "notes": q.notes, "internal_notes": q.internal_notes,
        "customer_org_id": q.customer_org_id.map(|u| u.to_string()),
        "customer_contact_id": q.customer_contact_id.map(|u| u.to_string()),
        "customer_org": org.as_ref().map(|o| json!({"id": o.id.to_string(), "name": o.name, "email": o.email})),
        "customer_contact": contact_json,
        "deal_id": q.deal_id.map(|u| u.to_string()),
        "items": items_json,
        // 1 Nov 2026
        "discount_note": q.discount_note,
        "discount_display": q.discount_display.clone().unwrap_or_else(|| "pdf".into()),
        "cancelled_at": q.cancelled_at.as_ref().map(iso),
        "cancel_reason": q.cancel_reason,
    });
    let mut out = out;
    if let (Value::Object(o), Value::Object(r)) = (&mut out, rest) {
        o.extend(r);
        // 4 Nov 2026: optional lines are out of the total; their own figures and the total with them
        o.extend(pricing::stored_optional_json(&q, &items));
    }
    // 5 Nov 2026 (E7): the pricing rules (blocking issues, warnings, the Owner's override)
    if let Some(c) = granary_services::pricing_rules::check_stored(&mut conn, q.id).await? {
        out["pricing_check"] =
            c.json_for(user.is_superadmin || user.perms.contains("sales:view_costs"));
    }
    Ok(resp::json(out))
}

/// `revise_quotation(quotation_id, ReviseBody(reason=...), current_user, db)`: a new revision of a
/// sent/expired/rejected quotation: the original is superseded and an editable DRAFT copy opens with the
/// next revision number. Commits. Also what documents/core.py calls.
pub(crate) async fn revise_quotation_core(
    db: &mut PgConnection,
    quotation_id: &str,
    reason: &str,
    user: &AuthUser,
) -> ApiResult<Value> {
    need(user, "sales:create_quotation")?;
    let reason = pystr::strip(reason);
    if reason.is_empty() {
        return Err(ApiError::http(
            400,
            "A reason for the revision is required.",
        ));
    }
    let q = load_quotation(db, quotation_id, false).await?;
    // selectinload(Quotation.items): order_by sort_order
    let items: Vec<QuotationItem> = sqlx::query_as(
        "SELECT * FROM quotation_items WHERE quotation_items.quotation_id = $1 ORDER BY quotation_items.sort_order",
    )
    .bind(q.id)
    .fetch_all(&mut *db)
    .await?;
    if !matches!(q.status.as_deref(), Some("sent" | "expired" | "rejected")) {
        return Err(ApiError::http(
            400,
            "Only sent, expired or rejected quotations can be revised. Edit drafts directly.",
        ));
    }
    // Block when downstream documents already reference this quotation.
    let has_invoice: Option<(Uuid,)> = sqlx::query_as(
        "SELECT invoices.id FROM invoices WHERE invoices.quotation_id = $1 AND invoices.is_deleted = false LIMIT 1",
    )
    .bind(q.id)
    .fetch_optional(&mut *db)
    .await?;
    let has_schedule: Option<(Uuid,)> = sqlx::query_as(
        "SELECT payment_schedules.id FROM payment_schedules WHERE payment_schedules.quotation_id = $1 \
         AND payment_schedules.is_deleted = false LIMIT 1",
    )
    .bind(q.id)
    .fetch_optional(&mut *db)
    .await?;
    if has_invoice.is_some() || has_schedule.is_some() {
        return Err(ApiError::http(
            400,
            "This quotation has generated invoices or a payment schedule. Void those first.",
        ));
    }
    let group = q
        .revision_group
        .clone()
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| q.number.clone());
    let new_rev = q.revision.filter(|r| *r != 0).unwrap_or(1) as i64 + 1;
    sqlx::query("UPDATE quotations SET status = 'superseded', updated_at = now() WHERE id = $1")
        .bind(q.id)
        .execute(&mut *db)
        .await?;
    engine::supersede_for_entity(
        db,
        "quotation",
        q.id,
        &format!("Superseded by revision {new_rev}"),
    )
    .await?;
    // 4 Nov 2026: optional lines are out of the totals. A revision of a quotation with optional lines works
    // its totals out again from the copied lines (their prices unchanged), so an older one stops counting them.
    let places = currency_places(db, q.currency.as_deref()).await?;
    let recomputed = items
        .iter()
        .any(|i| i.is_optional.unwrap_or(false))
        .then(|| pricing::stored_lines_totals(&q, &items, places));
    let (subtotal, discount_amount, tax_amount, tax_lines, total, rounded_total) = match &recomputed
    {
        Some((t, _)) => (
            Some(t.subtotal),
            Some(t.discount_amount),
            Some(t.tax_amount),
            Some(Value::Array(t.tax_lines.clone())),
            Some(t.total),
            Some(quantize_money(t.total, places)),
        ),
        None => (
            q.subtotal,
            q.discount_amount,
            q.tax_amount,
            q.tax_lines.clone(),
            q.total,
            q.rounded_total,
        ),
    };
    let item_tax: HashMap<Uuid, Decimal> = recomputed
        .map(|(_, taxes)| items.iter().map(|i| i.id).zip(taxes).collect())
        .unwrap_or_default();
    let rev_id = Uuid::new_v4();
    let full_number = format!("{group}-{new_rev}");
    sqlx::query(
        "INSERT INTO quotations (id, number, full_number, revision, revision_group, previous_version_id, \
         revision_reason, status, subject, deal_id, customer_org_id, customer_contact_id, currency, exchange_rate, \
         subtotal, discount_type, discount_value, discount_amount, tax_amount, tax_lines, total, rounded_total, \
         total_quantity, issue_date, valid_until, validity_days, payment_terms, shipping_terms, payment_terms_days, \
         procurement_type, lead_time_days_min, lead_time_days_max, notes, internal_notes, created_by_id, \
         customer_po_accepted_verbally, is_deleted) VALUES ($1, $2, $3, $4::int8, $5, $6, $7, 'draft', $8, $9, $10, \
         $11, COALESCE($12, 'USD'), $13, COALESCE($14, 0), $15, COALESCE($16, 0), COALESCE($17, 0), \
         COALESCE($18, 0), $19, COALESCE($20, 0), COALESCE($21, 0), COALESCE($22, 0), $23, $24, COALESCE($25, 14), \
         $26, $27, $28, COALESCE($29, 'standard'), $30, $31, $32, $33, $34, false, false)",
    )
    .bind(rev_id)
    .bind(format!("{group}-R{new_rev}"))
    .bind(&full_number)
    .bind(new_rev)
    .bind(&group)
    .bind(q.id)
    .bind(reason)
    .bind(&q.subject)
    .bind(q.deal_id)
    .bind(q.customer_org_id)
    .bind(q.customer_contact_id)
    .bind(&q.currency)
    .bind(q.exchange_rate)
    .bind(subtotal)
    .bind(&q.discount_type)
    .bind(q.discount_value)
    .bind(discount_amount)
    .bind(tax_amount)
    .bind(json_or_null(tax_lines))
    .bind(total)
    .bind(rounded_total)
    .bind(q.total_quantity)
    .bind(q.issue_date)
    .bind(q.valid_until)
    .bind(q.validity_days)
    .bind(json_or_null(q.payment_terms.clone()))
    .bind(&q.shipping_terms)
    .bind(q.payment_terms_days)
    .bind(&q.procurement_type)
    .bind(q.lead_time_days_min)
    .bind(q.lead_time_days_max)
    .bind(&q.notes)
    .bind(&q.internal_notes)
    .bind(user.id)
    .execute(&mut *db)
    .await?;
    // 1 Nov 2026: the discount's note and display go with it
    sqlx::query("UPDATE quotations SET discount_note = $2, discount_display = $3 WHERE id = $1")
        .bind(rev_id)
        .bind(&q.discount_note)
        .bind(&q.discount_display)
        .execute(&mut *db)
        .await?;
    // 10 Nov 2026: a notes family chosen by hand goes with it
    sqlx::query(
        "UPDATE quotations SET notes_family = (SELECT notes_family FROM quotations WHERE id = $2) WHERE id = $1",
    )
    .bind(rev_id)
    .bind(q.id)
    .execute(&mut *db)
    .await?;
    // The revision starts as an exact copy of the lines (prices included); editing it re-prices them.
    let new_ids: HashMap<Uuid, Uuid> = items.iter().map(|it| (it.id, Uuid::new_v4())).collect();
    let by_id: HashMap<Uuid, &QuotationItem> = items.iter().map(|it| (it.id, it)).collect();
    fn depth(it: &QuotationItem, n: usize, by_id: &HashMap<Uuid, &QuotationItem>) -> usize {
        let parent = it.parent_item_id.and_then(|p| by_id.get(&p));
        match parent {
            None => 0,
            Some(_) if n > by_id.len() => 0,
            Some(p) => 1 + depth(p, n + 1, by_id),
        }
    }
    let mut ordered: Vec<&QuotationItem> = items.iter().collect();
    ordered.sort_by_key(|it| depth(it, 0, &by_id));
    for it in ordered {
        let parent = it.parent_item_id.and_then(|p| new_ids.get(&p).copied());
        sqlx::query(
            "INSERT INTO quotation_items (id, quotation_id, sort_order, product_id, parent_item_id, name, description, \
             sku, quantity, unit_price, unit, currency, discount_percentage, discount_type, amount, is_optional, \
             image_url, sale_model, price_metric, rate_period_months, billing_interval_months, term_months, \
             list_unit_price, price_rule, tax_id, tax_code, tax_pct, tax_amount) VALUES ($1, $2, COALESCE($3, 0), \
             $4, $5, $6, $7, $8, $9, $10, COALESCE($11, 'Unit'), COALESCE($12, 'USD'), COALESCE($13, 0), \
             COALESCE($14, 'none'), $15, COALESCE($16, false), $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, \
             $27, $28)",
        )
        .bind(new_ids[&it.id])
        .bind(rev_id)
        .bind(it.sort_order)
        .bind(it.product_id)
        .bind(parent)
        .bind(&it.name)
        .bind(&it.description)
        .bind(&it.sku)
        .bind(it.quantity)
        .bind(it.unit_price)
        .bind(&it.unit)
        .bind(&it.currency)
        .bind(it.discount_percentage)
        .bind(&it.discount_type)
        .bind(it.amount)
        .bind(it.is_optional)
        .bind(&it.image_url)
        .bind(&it.sale_model)
        .bind(&it.price_metric)
        .bind(it.rate_period_months)
        .bind(it.billing_interval_months)
        .bind(it.term_months)
        .bind(it.list_unit_price)
        .bind(&it.price_rule)
        .bind(it.tax_id)
        .bind(&it.tax_code)
        .bind(it.tax_pct)
        .bind(item_tax.get(&it.id).copied().unwrap_or(it.tax_amount))
        .execute(&mut *db)
        .await?;
    }
    commit(db).await?;
    Ok(
        json!({"id": rev_id.to_string(), "full_number": full_number, "revision": new_rev, "status": "draft"}),
    )
}

/// `POST /quotations/{quotation_id}/revise` (201).
pub async fn revise_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(body): PyJson<ReviseBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let out = revise_quotation_core(&mut conn, &quotation_id, &body.reason, &user).await?;
    Ok(resp::json_status(201, out))
}

// ─── Quotation Line Item CRUD ─────────────────────────────────────────────────

/// `_recalculate_quotation_totals(db, quotation, current_user, overrides=, new_lines=, check_keys=)`:
/// re-price every stored line (plus `new_lines`) and update the lines and the header in place. Only
/// `check_keys` lines are permission-checked. Returns the priced result (the session keeps
/// `quotation.total` = its total after the commit).
async fn recalculate(
    db: &mut PgConnection,
    q: &Quotation,
    user: &AuthUser,
    overrides: Option<(Uuid, &ItemChanges)>,
    new_lines: Vec<LineIn>,
    check_keys: HashSet<String>,
) -> ApiResult<PricedResult> {
    let items: Vec<QuotationItem> = sqlx::query_as(
        "SELECT * FROM quotation_items WHERE quotation_items.quotation_id = $1 \
         ORDER BY quotation_items.sort_order, quotation_items.created_at",
    )
    .bind(q.id)
    .fetch_all(&mut *db)
    .await?;
    let mut lines: Vec<LineIn> = items
        .iter()
        .map(|i| {
            let o = overrides.and_then(|(id, c)| (id == i.id).then_some(c));
            stored_line_in(i, o)
        })
        .collect();
    lines.extend(new_lines);
    let res = price(db, &PriceCtx::of_row(q), &lines, user, Some(&check_keys)).await?;
    for item in &items {
        let key = item.id.to_string();
        let p = res
            .lines
            .iter()
            .find(|p| p.key == key)
            .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{key}'")))?;
        update_item_snapshot(db, item.id, p).await?;
    }
    let places = currency_places(db, q.currency.as_deref()).await?;
    save_totals(db, q.id, &totals(&res, places)).await?;
    Ok(res)
}

/// `_draft_quotation_for_items(db, quotation_id, current_user, verb)`.
async fn draft_quotation_for_items(
    db: &mut PgConnection,
    quotation_id: &str,
    user: &AuthUser,
    verb: &str,
) -> ApiResult<Quotation> {
    need(user, "sales:create_quotation")?;
    let q = load_quotation(db, quotation_id, true).await?;
    if q.status.as_deref() != Some("draft") {
        return Err(ApiError::http(
            400,
            format!("Items can only be {verb} draft quotations"),
        ));
    }
    Ok(q)
}

/// `POST /quotations/{quotation_id}/items` (201).
pub async fn add_quotation_item(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(body): PyJson<QuotationItemInput>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let q = draft_quotation_for_items(&mut conn, &quotation_id, &user, "added to").await?;
    validate_item_discount_type(body.discount_type.as_deref())?;
    let (max_sort,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(MAX(quotation_items.sort_order), -1)::int8 FROM quotation_items \
         WHERE quotation_items.quotation_id = $1",
    )
    .bind(q.id)
    .fetch_one(&mut *conn)
    .await?;
    let next_sort = max_sort + 1;
    let new_id = Uuid::new_v4();
    let mut line = line_in(&body, next_sort.max(0) as usize);
    line.key = new_id.to_string();
    line.sort_order = Some(body.sort_order.unwrap_or(next_sort));
    if let Some(pk) = body.parent_key.as_deref().filter(|s| !s.is_empty()) {
        let pid = crate::extract::parse_uuid(pk, "parent_key")?;
        let parent: Option<(Uuid,)> = sqlx::query_as(
            "SELECT quotation_items.id FROM quotation_items WHERE quotation_items.quotation_id = $1 \
             AND quotation_items.id = $2",
        )
        .bind(q.id)
        .bind(pid)
        .fetch_optional(&mut *conn)
        .await?;
        if parent.is_none() {
            return Err(ApiError::http(
                400,
                "The parent line is not on this quotation",
            ));
        }
    }
    let key = line.key.clone();
    let sort = line.sort_order.unwrap_or(next_sort);
    let check: HashSet<String> = [key.clone()].into_iter().collect();
    let res = recalculate(&mut conn, &q, &user, None, vec![line], check).await?;
    let p = res
        .lines
        .iter()
        .find(|x| x.key == key)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("StopIteration")))?;
    let parent = match p.parent_key.as_deref().filter(|s| !s.is_empty()) {
        Some(k) => {
            Some(Uuid::parse_str(k).map_err(|e| ApiError::internal(anyhow::anyhow!("{e}")))?)
        }
        None => None,
    };
    insert_item(
        &mut conn,
        new_id,
        q.id,
        q.currency.as_deref(),
        sort,
        parent,
        p,
    )
    .await?;
    commit(&mut conn).await?;
    let total = dec_float(&res.total);
    Ok(resp::json_status(
        201,
        json!({"id": new_id.to_string(), "quotation_total": total}),
    ))
}

/// `PATCH /quotations/{quotation_id}/items/{item_id}`.
pub async fn update_quotation_item(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((quotation_id, item_id)): Path<(String, String)>,
    PyJson(body): PyJson<WithSet<QuotationItemUpdate>>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let q = draft_quotation_for_items(&mut conn, &quotation_id, &user, "edited on").await?;
    let iid = pg_uuid(&item_id)?;
    let item: Option<QuotationItem> = sqlx::query_as(
        "SELECT * FROM quotation_items WHERE quotation_items.id = $1 AND quotation_items.quotation_id = $2",
    )
    .bind(iid)
    .bind(q.id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(item) = item else {
        return Err(ApiError::not_found("QuotationItem", Some(&item_id)));
    };
    let m = &body.model;
    let set = |k: &str| body.is_set(k);
    let changes = ItemChanges {
        name: set("name").then(|| m.name.clone()),
        description: set("description").then(|| m.description.clone()),
        quantity: set("quantity").then_some(m.quantity),
        unit_price: set("unit_price").then_some(m.unit_price),
        unit: set("unit").then(|| m.unit.clone()),
        discount_percentage: set("discount_percentage").then_some(m.discount_percentage),
        discount_type: set("discount_type").then(|| m.discount_type.clone()),
        is_optional: set("is_optional").then_some(m.is_optional),
        term_months: set("term_months").then_some(m.term_months),
        tax_id: set("tax_id").then(|| m.tax_id.clone()),
        tax_pct: set("tax_pct").then_some(m.tax_pct),
    };
    for (k, empty) in [
        ("name", matches!(changes.name, Some(None))),
        ("quantity", matches!(changes.quantity, Some(None))),
        ("unit_price", matches!(changes.unit_price, Some(None))),
        (
            "discount_percentage",
            matches!(changes.discount_percentage, Some(None)),
        ),
        ("is_optional", matches!(changes.is_optional, Some(None))),
    ] {
        if empty {
            return Err(ApiError::http(422, format!("{k} can't be empty")));
        }
    }
    if let Some(dt) = &changes.discount_type {
        let dt = dt.as_deref().filter(|s| !s.is_empty()).unwrap_or("none");
        validate_item_discount_type(Some(dt))?;
        // Documenting a discount (other than the automatic volume tier) is a discounting act
        if !matches!(dt, "none" | "volume") {
            require_discount_permission(&user)?;
        }
    }
    let check: HashSet<String> = [item.id.to_string()].into_iter().collect();
    let res = recalculate(
        &mut conn,
        &q,
        &user,
        Some((item.id, &changes)),
        Vec::new(),
        check,
    )
    .await?;
    commit(&mut conn).await?;
    let total = dec_float(&res.total);
    Ok(resp::json(
        json!({"id": item.id.to_string(), "quotation_total": total}),
    ))
}

/// `DELETE /quotations/{quotation_id}/items/{item_id}` (204).
pub async fn delete_quotation_item(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((quotation_id, item_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let q = draft_quotation_for_items(&mut conn, &quotation_id, &user, "deleted from").await?;
    let iid = pg_uuid(&item_id)?;
    let item: Option<(Uuid,)> = sqlx::query_as(
        "SELECT quotation_items.id FROM quotation_items WHERE quotation_items.id = $1 \
         AND quotation_items.quotation_id = $2",
    )
    .bind(iid)
    .bind(q.id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((item_uuid,)) = item else {
        return Err(ApiError::not_found("QuotationItem", Some(&item_id)));
    };
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(quotation_items.id) FROM quotation_items WHERE quotation_items.quotation_id = $1",
    )
    .bind(q.id)
    .fetch_one(&mut *conn)
    .await?;
    if count <= 1 {
        return Err(ApiError::http(
            400,
            "Cannot delete the last item on a quotation",
        ));
    }
    sqlx::query("DELETE FROM quotation_items WHERE quotation_items.id = $1")
        .bind(item_uuid)
        .execute(&mut *conn)
        .await?;
    recalculate(&mut conn, &q, &user, None, Vec::new(), HashSet::new()).await?;
    commit(&mut conn).await?;
    Ok(resp::no_content())
}

/// `QUOTATION_STATUS_TRANSITIONS`: where a quotation may go from each status by setting it directly.
fn transitions(status: Option<&str>) -> &'static [&'static str] {
    match status {
        Some("draft") => &["sent"],
        Some("sent") => &["accepted", "rejected", "expired"],
        Some("accepted") => &["rejected"],
        _ => &[],
    }
}

/// `str(quotation.status)` in an f-string.
fn status_str(s: &Option<String>) -> String {
    s.clone().unwrap_or_else(|| "None".into())
}

/// `PATCH /quotations/{quotation_id}/status`.
pub async fn update_quotation_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let mut q = load_quotation(&mut conn, &quotation_id, false).await?;
    let new_status = body.get("status").cloned().unwrap_or(Value::Null);
    let as_str = new_status.as_str();
    if as_str == Some("approved") {
        return Err(ApiError::http(
            400,
            "Quotations are approved through the approval flow, not by setting a status",
        ));
    }
    if as_str == Some("signed") {
        return Err(ApiError::http(
            400,
            "A quotation is signed by recording the customer's PO",
        ));
    }
    if matches!(new_status, Value::Array(_) | Value::Object(_)) {
        // `new_status not in {...}`: an unhashable value is a TypeError
        return Err(ApiError::internal(anyhow::anyhow!(
            "TypeError: unhashable type"
        )));
    }
    let Some(new) = as_str.filter(|s| transitions(q.status.as_deref()).contains(s)) else {
        return Err(ApiError::http(
            400,
            format!(
                "A {} quotation can't move to {}",
                status_str(&q.status),
                py_str(&new_status)
            ),
        ));
    };
    let new = new.to_string();
    if new == "sent" {
        need(&user, "sales:send_quotation")?;
        engine::ensure_or_submit(&mut conn, "quotation", q.id, &user, "sent", None).await?;
    } else {
        need(&user, "sales:create_quotation")?;
    }
    sqlx::query("UPDATE quotations SET status = $2, updated_at = now() WHERE id = $1")
        .bind(q.id)
        .bind(&new)
        .execute(&mut *conn)
        .await?;
    q.status = Some(new.clone());
    if new == "sent" {
        deal_hooks::log_quotation_event(&mut conn, &q, "sent", Some(user.id), None).await?;
        emit_quotation(&mut conn, "quotation.sent", &q, &user, Map::new()).await?;
    } else {
        // accepted, rejected or expired: the quotation has ended, nothing is left to chase.
        if new == "expired" {
            deal_hooks::log_quotation_event(&mut conn, &q, "expired", Some(user.id), None).await?;
        }
        emit_quotation(
            &mut conn,
            &format!("quotation.{new}"),
            &q,
            &user,
            Map::new(),
        )
        .await?;
        let status = if new == "accepted" {
            "done"
        } else {
            "cancelled"
        };
        followups::close_for(&mut conn, "quotation", q.id, status).await?;
    }
    commit(&mut conn).await?;
    granary_services::approvals::notify::dispatch_outbox();
    Ok(resp::json(json!({"id": q.id.to_string(), "status": new})))
}

/// `POST /quotations/{quotation_id}/send`: send or resend a quotation. Resend does NOT change the number
/// (Revise amends an issued quotation).
pub async fn send_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(_body): PyJson<SendQuotationBody>,
) -> ApiResult<Response> {
    need(&user, "sales:send_quotation")?;
    let mut conn = db.conn().await?;
    let mut q = load_quotation(&mut conn, &quotation_id, false).await?;
    if matches!(
        q.status.as_deref(),
        Some("accepted" | "cancelled" | "superseded")
    ) {
        return Err(ApiError::http(
            400,
            format!(
                "Cannot send a quotation with status '{}'",
                status_str(&q.status)
            ),
        ));
    }
    engine::ensure_or_submit(&mut conn, "quotation", q.id, &user, "sent", None).await?;
    // full_number is fixed at creation/revision from the revision group; keep it stable on (re)send.
    let full_number = match q.full_number.clone().filter(|s| !s.is_empty()) {
        Some(f) => f,
        None => format!(
            "{}-{}",
            q.revision_group
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| q.number.clone()),
            q.revision
                .map(|r| r.to_string())
                .unwrap_or_else(|| "None".into())
        ),
    };
    sqlx::query(
        "UPDATE quotations SET full_number = $2, status = 'sent', updated_at = now() WHERE id = $1",
    )
    .bind(q.id)
    .bind(&full_number)
    .execute(&mut *conn)
    .await?;
    q.full_number = Some(full_number.clone());
    q.status = Some("sent".into());
    deal_hooks::log_quotation_event(&mut conn, &q, "sent", Some(user.id), None).await?;
    emit_quotation(&mut conn, "quotation.sent", &q, &user, Map::new()).await?;
    commit(&mut conn).await?;
    granary_services::approvals::notify::dispatch_outbox();
    Ok(resp::json(json!({
        "id": q.id.to_string(),
        "full_number": full_number,
        "revision": q.revision,
        "status": "sent",
    })))
}

/// `PATCH /quotations/{quotation_id}/record-po`: the customer's PO reference (a document or verbal).
pub async fn record_customer_po(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    need(&user, "sales:create_quotation")?;
    let mut conn = db.conn().await?;
    let mut q = load_quotation(&mut conn, &quotation_id, false).await?;
    let mut po_number: Option<Value> = None;
    if let Some(v) = body.get("po_number").filter(|v| truthy(v)) {
        po_number = Some(v.clone());
    }
    if let Some(v) = body.get("po_date").filter(|v| truthy(v)) {
        let parsed = match v {
            Value::String(s) => granary_core::settings_store::py_date_fromisoformat(s),
            _ => None,
        };
        match parsed {
            Some(d) => q.customer_po_date = Some(d),
            None => return Err(ApiError::http(422, "po_date must be a date (YYYY-MM-DD)")),
        }
    }
    if let Some(v) = body.get("verbal") {
        q.customer_po_accepted_verbally = Some(truthy(v));
    }
    if let Some(v) = &po_number {
        // a non-str value fails in asyncpg when the row is written
        q.customer_po_number = db_str(v)?;
    }
    // Mark as signed once customer has issued or verbally confirmed PO
    let signing = matches!(q.status.as_deref(), Some("sent" | "accepted"));
    if signing {
        q.status = Some("signed".into());
    }
    sqlx::query(
        "UPDATE quotations SET customer_po_number = $2, customer_po_date = $3, customer_po_accepted_verbally = $4, \
         status = $5, updated_at = CASE WHEN (customer_po_number, customer_po_date, customer_po_accepted_verbally, \
         status) IS DISTINCT FROM ($2::varchar, $3::date, $4::bool, $5::varchar) THEN now() ELSE updated_at END \
         WHERE id = $1",
    )
    .bind(q.id)
    .bind(&q.customer_po_number)
    .bind(q.customer_po_date)
    .bind(q.customer_po_accepted_verbally)
    .bind(&q.status)
    .execute(&mut *conn)
    .await?;
    if signing {
        deal_hooks::log_quotation_event(&mut conn, &q, "signed", Some(user.id), None).await?;
        emit_quotation(&mut conn, "quotation.signed", &q, &user, Map::new()).await?;
        followups::close_for(&mut conn, "quotation", q.id, "done").await?;
    }
    commit(&mut conn).await?;
    Ok(resp::json(json!({
        "id": q.id.to_string(),
        "customer_po_number": q.customer_po_number,
        "customer_po_accepted_verbally": q.customer_po_accepted_verbally,
        "status": q.status,
    })))
}

/// `DELETE /quotations/{quotation_id}` (204): a soft delete of a draft, rejected or expired quotation.
pub async fn delete_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
) -> ApiResult<Response> {
    // Deleting a draft is an edit-level action; view-only users must not be able to remove one.
    need(&user, "sales:edit_quotation")?;
    let mut conn = db.conn().await?;
    let q = load_quotation(&mut conn, &quotation_id, false).await?;
    if !matches!(q.status.as_deref(), Some("draft" | "rejected" | "expired")) {
        return Err(ApiError::http(
            400,
            "Only draft, rejected, or expired quotations can be deleted",
        ));
    }
    sqlx::query(
        "UPDATE quotations SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(q.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    commit(&mut conn).await?;
    Ok(resp::no_content())
}

// ─── 1 Nov 2026: withdraw from approval, cancel, reprice to today ──────────────
// The same code as Nary's quotation.withdraw / quotation.cancel / quotation.reprice (the MCP client and the chat).

fn reason_of(body: &Map<String, Value>) -> Option<String> {
    body.get("reason")
        .and_then(Value::as_str)
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
}

/// `POST /quotations/{quotation_id}/withdraw`: the person who submitted it, or the Owner, takes it back from
/// approval; it is an editable draft again.
pub async fn withdraw_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    need(&user, "sales:create_quotation")?;
    let mut conn = db.conn().await?;
    let q = load_quotation(&mut conn, &quotation_id, false).await?;
    let out = granary_services::nary::quotation_lifecycle::withdraw(
        &mut conn,
        &user,
        &q,
        reason_of(&body).as_deref(),
    )
    .await?;
    commit(&mut conn).await?;
    Ok(resp::json(out))
}

/// `POST /quotations/{quotation_id}/cancel` {reason}: a draft (also one waiting for approval) is cancelled and
/// kept for history.
pub async fn cancel_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    need(&user, "sales:create_quotation")?;
    let mut conn = db.conn().await?;
    let q = load_quotation(&mut conn, &quotation_id, false).await?;
    let out = granary_services::nary::quotation_lifecycle::cancel(
        &mut conn,
        &user,
        &q,
        &reason_of(&body).unwrap_or_default(),
    )
    .await?;
    commit(&mut conn).await?;
    granary_services::approvals::notify::dispatch_outbox();
    Ok(resp::json(out))
}

/// `POST /quotations/{quotation_id}/reprice`: a draft dated today, catalog lines at today's price, typed lines
/// kept; then back into the approval flow. Returns `changes` (each line before and after).
pub async fn reprice_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
) -> ApiResult<Response> {
    need(&user, "sales:create_quotation")?;
    let mut conn = db.conn().await?;
    let q = load_quotation(&mut conn, &quotation_id, false).await?;
    let out = granary_services::nary::quotation_lifecycle::reprice(
        &mut conn,
        &user,
        &json!({"quotation_id": q.id.to_string()}),
    )
    .await?;
    commit(&mut conn).await?;
    granary_services::approvals::notify::dispatch_outbox();
    Ok(resp::json(out))
}

/// `POST /quotations/{quotation_id}/link-deal` {deal_id, set_deal_value = true}: 5 Nov 2026 (Stream C): a quotation of
/// any status but cancelled or superseded (and every revision of it) is linked to a deal without a revision; by
/// default the deal's value becomes its total. The PDF, lines, terms, approval and status never change (Nary's
/// quotation.link_deal runs the same code). Returns the quotation.
pub async fn link_deal(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    need(&user, "sales:create_quotation")?;
    let deal_id = match body.get("deal_id").and_then(Value::as_str).map(str::trim) {
        Some(d) if !d.is_empty() => pg_uuid(d)?,
        _ => return Err(ApiError::http(422, "deal_id is required")),
    };
    let set_value = match body.get("set_deal_value") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ApiError::http(422, "set_deal_value must be true or false")),
    };
    {
        let mut conn = db.conn().await?;
        let q = load_quotation(&mut conn, &quotation_id, false).await?;
        granary_services::nary::quotation_links::link_deal(
            &mut conn, &user, q.id, deal_id, set_value, "ui",
        )
        .await?;
        commit(&mut conn).await?;
    }
    get_quotation(CurrentUser(user), db, Path(quotation_id)).await
}

/// `POST /quotations/{quotation_id}/pricing-override` {reason}: 5 Nov 2026 (E7, Owner decision): the Owner lets a
/// draft through the pricing rules with a reason (an empty reason clears it); it holds for these lines only and an
/// audit row records it. Anyone else gets a 403.
pub async fn pricing_override(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    need(&user, "sales:view_quotations")?;
    let mut conn = db.conn().await?;
    let q = load_quotation(&mut conn, &quotation_id, false).await?;
    let out = granary_services::pricing_rules::set_override(
        &mut conn,
        &user,
        q.id,
        reason_of(&body).as_deref(),
        "ui",
    )
    .await?;
    commit(&mut conn).await?;
    Ok(resp::json(out))
}
