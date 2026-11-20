//! finance.py: invoice endpoints, line item CRUD and proforma conversion.

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::money::{quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_core::validation::{v, PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::finance::{Invoice, InvoiceItem, InvoiceRefund, Payment};
use granary_services::invoicing::{self, ItemSpec, SpecOverrides};
use granary_services::numbering::next_document_number;

use super::{
    f, f_truthy, fmt_comma, fromiso, has, py_str, py_truthy, require_perm, strip, strip_or_none,
    Dict, Sent,
};
use crate::extract::parse_uuid;
use crate::resp;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// `full_number or number`.
fn full_or_number(full: &Option<String>, number: &str) -> String {
    match full.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => number.to_string(),
    }
}

/// `_inv_summary(i)`.
pub fn inv_summary(i: &Invoice) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!(i.id.to_string()));
    m.insert("number".into(), json!(i.number));
    m.insert(
        "full_number".into(),
        json!(full_or_number(&i.full_number, &i.number)),
    );
    m.insert("invoice_type".into(), json!(i.invoice_type));
    m.insert("status".into(), json!(i.status));
    m.insert("currency".into(), json!(i.currency));
    m.insert(
        "exchange_rate".into(),
        match i.exchange_rate {
            Some(r) if !r.is_zero() => f(&r),
            _ => resp::py_float(1.0),
        },
    );
    m.insert("subtotal".into(), f_truthy(i.subtotal));
    m.insert("tax_total".into(), f_truthy(i.tax_total));
    m.insert("total".into(), f_truthy(i.total));
    m.insert(
        "amount_paid".into(),
        match i.amount_paid {
            Some(a) if !a.is_zero() => f(&a),
            _ => json!(0),
        },
    );
    m.insert("amount_due".into(), f(&D(i.amount_due)));
    m.insert("refund_due".into(), f(&i.refund_due));
    m.insert("issue_date".into(), json!(resp::date_iso(&i.issue_date)));
    m.insert("due_date".into(), resp::date_iso_opt(i.due_date.as_ref()));
    m.insert(
        "customer_org_id".into(),
        json!(i.customer_org_id.map(|u| u.to_string())),
    );
    m.insert("customer_name".into(), json!(i.customer_name));
    m.insert("ref_pi_number".into(), json!(i.ref_pi_number));
    m
}

/// 21 Nov 2026: the invoice's customer name for lists and reports: the bill-to snapshot, else its organization,
/// else its contact (an invoice made on "New invoice" before then has no snapshot).
pub const CUSTOMER_NAME_SQL: &str = "COALESCE(NULLIF(btrim(invoices.customer_name), ''), \
     (SELECT o.name FROM organizations o WHERE o.id = invoices.customer_org_id), \
     (SELECT NULLIF(btrim(c.first_name || ' ' || COALESCE(c.last_name, '')), '') FROM contacts c \
      WHERE c.id = invoices.customer_contact_id))";

/// [`CUSTOMER_NAME_SQL`] for these invoices.
pub async fn customer_names(
    db: &mut PgConnection,
    ids: &[Uuid],
) -> ApiResult<std::collections::HashMap<Uuid, Option<String>>> {
    if ids.is_empty() {
        return Ok(Default::default());
    }
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(&format!(
        "SELECT invoices.id, {CUSTOMER_NAME_SQL} FROM invoices WHERE invoices.id = ANY($1)"
    ))
    .bind(ids)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The bill-to name a new or edited invoice keeps: the one given, else its organization's, else its contact's.
async fn billed_name(
    db: &mut PgConnection,
    given: Option<&str>,
    org_id: Option<Uuid>,
    contact_id: Option<Uuid>,
) -> ApiResult<Option<String>> {
    if let Some(g) = given.filter(|g| !g.trim().is_empty()) {
        return Ok(Some(g.to_string()));
    }
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT o.name FROM organizations o WHERE o.id = $1), \
         (SELECT NULLIF(btrim(c.first_name || ' ' || COALESCE(c.last_name, '')), '') FROM contacts c WHERE c.id = $2))",
    )
    .bind(org_id)
    .bind(contact_id)
    .fetch_one(&mut *db)
    .await?)
}

/// `select(Invoice).where(Invoice.id == invoice_id, Invoice.is_deleted == False)` (optionally FOR UPDATE).
pub async fn load_invoice(
    db: &mut PgConnection,
    id: Uuid,
    for_update: bool,
) -> ApiResult<Option<Invoice>> {
    let sql = if for_update {
        "SELECT * FROM invoices WHERE id = $1 AND is_deleted = false FOR UPDATE"
    } else {
        "SELECT * FROM invoices WHERE id = $1 AND is_deleted = false"
    };
    Ok(sqlx::query_as(sql)
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

/// `Invoice.items` (selectinload, ordered by sort_order).
pub async fn invoice_items(db: &mut PgConnection, invoice_id: Uuid) -> ApiResult<Vec<InvoiceItem>> {
    Ok(
        sqlx::query_as("SELECT * FROM invoice_items WHERE invoice_id = $1 ORDER BY sort_order")
            .bind(invoice_id)
            .fetch_all(&mut *db)
            .await?,
    )
}

/// An `Invoice(...)` to insert. `None` columns that have a model default take it (SQLAlchemy skips None).
#[derive(Default, Clone, Debug)]
pub struct NewInvoice {
    pub number: String,
    pub full_number: Option<String>,
    pub revision: Option<i32>,
    pub revision_group: Option<String>,
    pub previous_version_id: Option<Uuid>,
    pub revision_reason: Option<String>,
    pub credit_of_invoice_id: Option<Uuid>,
    pub invoice_type: Option<String>,
    pub customer_org_id: Option<Uuid>,
    pub customer_contact_id: Option<Uuid>,
    pub customer_name: Option<String>,
    pub customer_address: Option<String>,
    pub customer_tax_id: Option<String>,
    pub customer_contact_person: Option<String>,
    pub customer_email: Option<String>,
    pub customer_tel: Option<String>,
    pub quotation_id: Option<Uuid>,
    pub deal_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub ref_pi_number: Option<String>,
    pub currency: Option<String>,
    pub exchange_rate: Option<Decimal>,
    pub issue_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub payment_terms_text: Option<String>,
    pub notes: Option<String>,
    pub incoterms: Option<String>,
    pub carrier: Option<String>,
    pub hs_code: Option<String>,
    pub country_of_origin: Option<String>,
    pub shipped_from: Option<String>,
    pub status: Option<String>,
    pub subtotal: Option<Decimal>,
    pub tax_total: Option<Decimal>,
    pub total: Option<Decimal>,
    pub amount_paid: Option<Decimal>,
    pub amount_due: Option<Decimal>,
    pub created_by_id: Option<Uuid>,
}

/// `db.add(Invoice(...)); await db.flush()`: the row as stored.
pub async fn insert_invoice(db: &mut PgConnection, n: &NewInvoice) -> ApiResult<Invoice> {
    let zero = Decimal::ZERO;
    Ok(sqlx::query_as(
        "INSERT INTO invoices (id, number, full_number, revision, revision_group, previous_version_id, \
         revision_reason, credit_of_invoice_id, invoice_type, customer_org_id, customer_contact_id, \
         customer_name, customer_address, customer_tax_id, customer_contact_person, customer_email, \
         customer_tel, quotation_id, deal_id, project_id, ref_pi_number, currency, exchange_rate, \
         issue_date, due_date, payment_terms_text, notes, incoterms, carrier, hs_code, country_of_origin, \
         shipped_from, status, subtotal, tax_lines, tax_total, total, amount_paid, amount_due, refund_due, \
         created_by_id, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, \
         $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33, \
         $34, '[]'::jsonb, $35, $36, $37, $38, 0, $39, false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&n.number)
    .bind(&n.full_number)
    .bind(n.revision.unwrap_or(1))
    .bind(&n.revision_group)
    .bind(n.previous_version_id)
    .bind(&n.revision_reason)
    .bind(n.credit_of_invoice_id)
    .bind(n.invoice_type.clone().unwrap_or_else(|| "commercial".into()))
    .bind(n.customer_org_id)
    .bind(n.customer_contact_id)
    .bind(&n.customer_name)
    .bind(&n.customer_address)
    .bind(&n.customer_tax_id)
    .bind(&n.customer_contact_person)
    .bind(&n.customer_email)
    .bind(&n.customer_tel)
    .bind(n.quotation_id)
    .bind(n.deal_id)
    .bind(n.project_id)
    .bind(&n.ref_pi_number)
    .bind(n.currency.clone().unwrap_or_else(|| "USD".into()))
    .bind(n.exchange_rate.unwrap_or(Decimal::ONE))
    .bind(n.issue_date)
    .bind(n.due_date)
    .bind(&n.payment_terms_text)
    .bind(&n.notes)
    .bind(&n.incoterms)
    .bind(&n.carrier)
    .bind(&n.hs_code)
    .bind(&n.country_of_origin)
    .bind(&n.shipped_from)
    .bind(n.status.clone().unwrap_or_else(|| "draft".into()))
    .bind(n.subtotal.unwrap_or(zero))
    .bind(n.tax_total.unwrap_or(zero))
    .bind(n.total.unwrap_or(zero))
    .bind(n.amount_paid.unwrap_or(zero))
    .bind(n.amount_due.unwrap_or(zero))
    .bind(n.created_by_id)
    .fetch_one(&mut *db)
    .await?)
}

/// The header of a copy (revise / credit note / convert): the customer and document fields of `inv`.
fn copy_header(inv: &Invoice) -> NewInvoice {
    NewInvoice {
        customer_org_id: inv.customer_org_id,
        customer_contact_id: inv.customer_contact_id,
        customer_name: inv.customer_name.clone(),
        customer_address: inv.customer_address.clone(),
        customer_tax_id: inv.customer_tax_id.clone(),
        customer_contact_person: inv.customer_contact_person.clone(),
        quotation_id: inv.quotation_id,
        deal_id: inv.deal_id,
        project_id: inv.project_id,
        currency: inv.currency.clone(),
        exchange_rate: inv.exchange_rate,
        payment_terms_text: inv.payment_terms_text.clone(),
        ..Default::default()
    }
}

/// `status` as Python sees it (None -> "None" in f-strings).
fn status_str(s: &Option<String>) -> String {
    s.clone().unwrap_or_else(|| "None".into())
}

// ─── Invoice endpoints ───────────────────────────────────────────────────────

fn qty_c() -> impl Fn(&PyObj, &mut granary_core::validation::Cx) -> Option<Decimal> {
    v::decimal_c(v::DecC::new().gt("0").max_digits(10).decimal_places(2))
}
fn price_c() -> impl Fn(&PyObj, &mut granary_core::validation::Cx) -> Option<Decimal> {
    v::decimal_c(v::DecC::new().ge("0").max_digits(18).decimal_places(2))
}
fn disc_c() -> impl Fn(&PyObj, &mut granary_core::validation::Cx) -> Option<Decimal> {
    v::decimal_c(
        v::DecC::new()
            .ge("0")
            .le("100")
            .max_digits(5)
            .decimal_places(2),
    )
}
fn taxpct_c() -> impl Fn(&PyObj, &mut granary_core::validation::Cx) -> Option<Decimal> {
    v::decimal_c(
        v::DecC::new()
            .ge("0")
            .le("100")
            .max_digits(8)
            .decimal_places(4),
    )
}
fn rate_c() -> impl Fn(&PyObj, &mut granary_core::validation::Cx) -> Option<Decimal> {
    v::decimal_c(v::DecC::new().gt("0").max_digits(18).decimal_places(6))
}

granary_core::py_model! {
    /// `class InvoiceItemInput(BaseModel)`.
    pub struct InvoiceItemInput {
        pub name: String,
        pub description: Option<String> = None,
        pub sku: Option<String> = None,
        pub quantity: Decimal = Decimal::ONE => qty_c(),
        pub unit_price: Decimal => price_c(),
        pub unit: String = "Unit",
        pub product_id: Option<String> = None,
        pub discount_pct: Decimal = Decimal::ZERO => disc_c(),
        pub tax_id: Option<String> = None,
        pub tax_pct: Option<Decimal> = None => v::nullable(taxpct_c()),
        pub key: Option<String> = None,
        pub parent_key: Option<String> = None,
        pub parent_item_id: Option<String> = None,
        pub term_months: Option<i32> = None,
        pub price_rule: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class InvoiceItemPatch(BaseModel)`; `__fields_set__` is kept for `exclude_unset`.
    pub struct InvoiceItemPatch {
        pub name: Option<String> = None,
        pub description: Option<String> = None,
        pub sku: Option<String> = None,
        pub quantity: Option<Decimal> = None => v::nullable(qty_c()),
        pub unit_price: Option<Decimal> = None => v::nullable(price_c()),
        pub unit: Option<String> = None,
        pub discount_pct: Option<Decimal> = None => v::nullable(disc_c()),
        pub tax_id: Option<String> = None,
        pub tax_pct: Option<Decimal> = None => v::nullable(taxpct_c()),
        pub clear_tax: bool = false,
        pub term_months: Option<i32> = None,
    }
}

/// `_item_specs(db, items)`: specs with their taxes resolved (sort_order = position).
async fn item_specs(db: &mut PgConnection, items: &[InvoiceItemInput]) -> ApiResult<Vec<ItemSpec>> {
    let mut specs: Vec<ItemSpec> = items
        .iter()
        .enumerate()
        .map(|(idx, it)| ItemSpec {
            name: it.name.clone(),
            description: it.description.clone(),
            sku: it.sku.clone(),
            unit: Some(it.unit.clone()),
            product_id: None,
            sort_order: Some(Some(idx as i32)),
            quantity: it.quantity,
            unit_price: it.unit_price,
            discount_pct: Some(it.discount_pct),
            tax_id: it.tax_id.clone(),
            tax_pct: it.tax_pct,
            tax_code: None,
            tax_name: None,
            term_months: it.term_months,
            price_rule: it.price_rule.clone(),
            key: it.key.clone(),
            parent_key: it.parent_key.clone(),
        })
        .collect();
    invoicing::resolve_line_taxes(db, &mut specs).await?;
    // product_id=s.get("product_id") or None: a non-UUID string fails when the row is written.
    for (spec, it) in specs.iter_mut().zip(items) {
        spec.product_id = match it.product_id.as_deref() {
            None | Some("") => None,
            Some(p) => Some(pg_uuid(p)?),
        };
    }
    Ok(specs)
}

granary_core::py_model! {
    /// `class InvoiceCreate(BaseModel)`.
    pub struct InvoiceCreate {
        pub invoice_type: String = "proforma",
        pub customer_org_id: Option<String> = None,
        pub customer_contact_id: Option<String> = None,
        pub customer_name: Option<String> = None,
        pub customer_address: Option<String> = None,
        pub customer_tax_id: Option<String> = None,
        pub customer_contact_person: Option<String> = None,
        pub customer_email: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(255))),
        pub customer_tel: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(50))),
        pub quotation_id: Option<String> = None,
        pub deal_id: Option<String> = None,
        pub project_id: Option<String> = None,
        pub ref_pi_number: Option<String> = None,
        pub currency: String = "USD",
        pub exchange_rate: Option<Decimal> = None => v::nullable(rate_c()),
        pub issue_date: Option<String> = None,
        pub due_date: Option<String> = None,
        pub payment_terms_text: Option<String> = None,
        pub notes: Option<String> = None,
        pub incoterms: Option<String> = None,
        pub carrier: Option<String> = None,
        pub hs_code: Option<String> = None,
        pub country_of_origin: Option<String> = None,
        pub shipped_from: Option<String> = None,
        pub items: Vec<InvoiceItemInput> = Vec::new(),
    }
}

granary_core::py_model! {
    pub struct ListInvoicesQuery {
        pub status: Option<String> = None,
        pub invoice_type: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|x| !x.is_empty())
}

/// `GET /invoices`.
pub async fn list_invoices(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListInvoicesQuery>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:view_invoices")?;
    let mut conn = db.conn().await?;
    let mut sql = String::from("SELECT * FROM invoices WHERE is_deleted = false");
    let mut binds: Vec<String> = Vec::new();
    if let Some(s) = nonempty(&q.status) {
        binds.push(s.to_string());
        sql.push_str(&format!(" AND status = ${}", binds.len()));
    }
    if let Some(t) = nonempty(&q.invoice_type) {
        binds.push(t.to_string());
        sql.push_str(&format!(" AND invoice_type = ${}", binds.len()));
    }
    sql.push_str(&format!(
        " ORDER BY created_at DESC LIMIT ${} OFFSET ${}",
        binds.len() + 1,
        binds.len() + 2
    ));
    let mut query = sqlx::query_as::<_, Invoice>(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let rows = query
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    // 21 Nov 2026: an invoice made on "New invoice" has no bill-to snapshot (customer_name null), so the
    // list's Client column showed "-": every row carries its organization / contact, and the name falls back
    // to them.
    let org_ids: Vec<Uuid> = rows.iter().filter_map(|i| i.customer_org_id).collect();
    let contact_ids: Vec<Uuid> = rows.iter().filter_map(|i| i.customer_contact_id).collect();
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
                format!("{f} {}", l.unwrap_or_default()).trim().to_string(),
            )
        })
        .collect()
    };
    let out: Vec<Value> = rows
        .iter()
        .map(|i| {
            let mut m = inv_summary(i);
            let org = i
                .customer_org_id
                .and_then(|id| orgs.get(&id).map(|n| (id, n)));
            let contact = i
                .customer_contact_id
                .and_then(|id| contacts.get(&id).map(|n| (id, n)));
            m.insert(
                "customer_org".into(),
                org.map(|(id, n)| json!({"id": id.to_string(), "name": n}))
                    .unwrap_or(Value::Null),
            );
            m.insert(
                "customer_contact".into(),
                contact
                    .map(|(id, n)| json!({"id": id.to_string(), "full_name": n}))
                    .unwrap_or(Value::Null),
            );
            let snapshot = i.customer_name.as_deref().filter(|n| !n.trim().is_empty());
            if snapshot.is_none() {
                let name = org.map(|(_, n)| n).or(contact.map(|(_, n)| n));
                m.insert("customer_name".into(), json!(name));
            }
            Value::Object(m)
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /invoices/summary`: KPI figures per currency over all invoices.
pub async fn invoice_summary(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_perm(&user, "finance:view_invoices")?;
    let mut conn = db.conn().await?;
    let today = today_local(&mut conn).await?;
    let month_start = today.with_day(1).expect("day 1");
    let next_month = (month_start + chrono::Duration::days(32))
        .with_day(1)
        .expect("day 1");
    let open_rows: Vec<(Option<String>, Decimal, Decimal)> = sqlx::query_as(
        "SELECT invoices.currency, coalesce(sum(invoices.amount_due), 0), \
         coalesce(sum(CASE WHEN (invoices.status = 'overdue' OR invoices.due_date IS NOT NULL \
         AND invoices.due_date < $1) THEN invoices.amount_due ELSE 0 END), 0) FROM invoices \
         WHERE invoices.is_deleted = false AND invoices.invoice_type = 'commercial' \
         AND invoices.status IN ('sent', 'partially_paid', 'overdue') GROUP BY invoices.currency",
    )
    .bind(today)
    .fetch_all(&mut *conn)
    .await?;
    let paid_rows: Vec<(Option<String>, Decimal)> = sqlx::query_as(
        "SELECT payments.currency, coalesce(sum(payments.amount), 0) FROM payments \
         JOIN invoices ON invoices.id = payments.invoice_id WHERE payments.is_deleted = false \
         AND payments.status = 'confirmed' AND payments.payment_date >= $1 AND payments.payment_date < $2 \
         AND invoices.is_deleted = false AND invoices.invoice_type IN ('commercial', 'proforma') \
         GROUP BY payments.currency",
    )
    .bind(month_start)
    .bind(next_month)
    .fetch_all(&mut *conn)
    .await?;
    // code -> [outstanding, overdue, paid_this_month]
    let mut totals: std::collections::BTreeMap<String, [Decimal; 3]> = Default::default();
    let code = |cur: &Option<String>| {
        match cur.as_deref() {
            Some(c) if !c.is_empty() => c,
            _ => "USD",
        }
        .to_uppercase()
    };
    for (cur, outstanding, overdue) in &open_rows {
        let r = totals.entry(code(cur)).or_insert([Decimal::ZERO; 3]);
        r[0] += outstanding;
        r[1] += overdue;
    }
    for (cur, paid) in &paid_rows {
        let r = totals.entry(code(cur)).or_insert([Decimal::ZERO; 3]);
        r[2] += paid;
    }
    let mut currencies = Vec::new();
    for (c, vals) in &totals {
        let places = currency_places(&mut conn, Some(c)).await?;
        currencies.push(json!({
            "currency": c,
            "outstanding": f(&quantize_money(vals[0], places)),
            "overdue": f(&quantize_money(vals[1], places)),
            "paid_this_month": f(&quantize_money(vals[2], places)),
        }));
    }
    let month = resp::date_iso(&month_start)[..7].to_string();
    Ok(resp::json(
        json!({"month": month, "currencies": currencies}),
    ))
}

/// `POST /invoices` (201).
pub async fn create_invoice(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<InvoiceCreate>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:create_invoice")?;
    let mut conn = db.conn().await?;
    let base = if body.invoice_type == "proforma" {
        "proforma"
    } else {
        "invoice"
    };
    let sequence_type =
        granary_services::numbering::currency_sequence(&mut conn, base, Some(&body.currency))
            .await?;
    let number = next_document_number(&mut conn, &sequence_type).await?;
    let today = today_local(&mut conn).await?;
    let issue_date = match nonempty(&body.issue_date) {
        Some(s) => fromiso(s)?,
        None => today,
    };
    let due_date = match nonempty(&body.due_date) {
        Some(s) => Some(fromiso(s)?),
        None => None,
    };
    let exchange_rate = match body.exchange_rate {
        Some(r) => r,
        None => {
            let functional = granary_services::fx::functional_currency(&mut conn).await?;
            if body.currency.to_uppercase() != functional {
                match granary_services::fx::get_rate(
                    &mut conn,
                    &body.currency,
                    &functional,
                    Some(issue_date),
                )
                .await
                {
                    Ok(r) => r,
                    Err(granary_services::fx::RateError::NoRate(_)) => {
                        // A silent 1.0 would value a USD invoice as 1 JPY per dollar in every report.
                        return Err(ApiError::http(
                            400,
                            format!(
                                "No {}/{} exchange rate is on record for {}. Enter the exchange rate on the invoice.",
                                body.currency.to_uppercase(),
                                functional,
                                resp::date_iso(&issue_date)
                            ),
                        ));
                    }
                    Err(granary_services::fx::RateError::Other(e)) => return Err(e),
                }
            } else {
                Decimal::ONE
            }
        }
    };
    let specs = item_specs(&mut conn, &body.items).await?;
    let new = NewInvoice {
        number: number.clone(),
        full_number: Some(format!("{number}-1")),
        revision: Some(1),
        revision_group: Some(number.clone()),
        invoice_type: Some(body.invoice_type.clone()),
        customer_org_id: pg_uuid_opt(body.customer_org_id.as_deref())?,
        customer_contact_id: pg_uuid_opt(body.customer_contact_id.as_deref())?,
        customer_name: billed_name(
            &mut conn,
            body.customer_name.as_deref(),
            pg_uuid_opt(body.customer_org_id.as_deref())?,
            pg_uuid_opt(body.customer_contact_id.as_deref())?,
        )
        .await?,
        customer_address: body.customer_address.clone(),
        customer_tax_id: body.customer_tax_id.clone(),
        customer_contact_person: body.customer_contact_person.clone(),
        customer_email: strip_or_none(body.customer_email.as_deref()),
        customer_tel: strip_or_none(body.customer_tel.as_deref()),
        quotation_id: pg_uuid_opt(body.quotation_id.as_deref())?,
        deal_id: pg_uuid_opt(body.deal_id.as_deref())?,
        project_id: pg_uuid_opt(body.project_id.as_deref())?,
        ref_pi_number: body.ref_pi_number.clone(),
        currency: Some(body.currency.clone()),
        exchange_rate: Some(exchange_rate),
        issue_date: Some(issue_date),
        due_date,
        payment_terms_text: body.payment_terms_text.clone(),
        notes: body.notes.clone(),
        incoterms: body.incoterms.clone(),
        carrier: body.carrier.clone(),
        hs_code: body.hs_code.clone(),
        country_of_origin: body.country_of_origin.clone(),
        shipped_from: body.shipped_from.clone(),
        status: Some("draft".into()),
        subtotal: Some(Decimal::ZERO),
        tax_total: Some(Decimal::ZERO),
        total: Some(Decimal::ZERO),
        amount_paid: Some(Decimal::ZERO),
        amount_due: Some(Decimal::ZERO),
        created_by_id: Some(user.id),
        ..Default::default()
    };
    let mut invoice = insert_invoice(&mut conn, &new).await?;
    invoicing::write_items(&mut conn, &mut invoice, &specs, None).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": invoice.id.to_string(), "number": invoice.number, "invoice_type": invoice.invoice_type}),
    ))
}

/// `GET /invoices/{invoice_id}`.
pub async fn get_invoice(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(invoice_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:view_invoices")?;
    let mut conn = db.conn().await?;
    let Some(inv) = load_invoice(&mut conn, pg_uuid(&invoice_id)?, false).await? else {
        return Err(ApiError::not_found("Invoice", Some(&invoice_id)));
    };
    let items = invoice_items(&mut conn, inv.id).await?;
    let payments: Vec<Payment> = sqlx::query_as(
        "SELECT * FROM payments WHERE invoice_id = $1 AND is_deleted = false ORDER BY payment_date",
    )
    .bind(inv.id)
    .fetch_all(&mut *conn)
    .await?;
    let refunds: Vec<InvoiceRefund> =
        sqlx::query_as("SELECT * FROM invoice_refunds WHERE invoice_id = $1 ORDER BY paid_date")
            .bind(inv.id)
            .fetch_all(&mut *conn)
            .await?;
    // Revision history (proforma): invoices sharing the revision_group.
    let group = match inv.revision_group.as_deref() {
        Some(g) if !g.is_empty() => g.to_string(),
        _ => inv.number.clone(),
    };
    let rev_rows: Vec<Invoice> = sqlx::query_as(
        "SELECT * FROM invoices WHERE revision_group = $1 AND is_deleted = false ORDER BY revision",
    )
    .bind(&group)
    .fetch_all(&mut *conn)
    .await?;
    let revisions: Vec<Value> = if rev_rows.len() > 1 {
        rev_rows
            .iter()
            .map(|r| {
                json!({
                    "id": r.id.to_string(),
                    "full_number": full_or_number(&r.full_number, &r.number),
                    "revision": r.revision,
                    "status": r.status,
                    "created_at": resp::iso(&r.created_at),
                    "revision_reason": r.revision_reason,
                    "is_current": r.id == inv.id,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let active = rev_rows
        .iter()
        .find(|r| r.status.as_deref() != Some("superseded"));
    // Credit-note linkage.
    let mut credited_by = Value::Null;
    if inv.invoice_type.as_deref() == Some("commercial") {
        let cn: Option<Invoice> = sqlx::query_as(
            "SELECT * FROM invoices WHERE credit_of_invoice_id = $1 AND is_deleted = false \
             ORDER BY status = 'cancelled' ASC, created_at DESC LIMIT 1",
        )
        .bind(inv.id)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(cn) = cn {
            credited_by = json!({"id": cn.id.to_string(), "full_number": full_or_number(&cn.full_number, &cn.number),
                                 "status": cn.status});
        }
    }
    let mut converted_to = Value::Null;
    if inv.invoice_type.as_deref() == Some("proforma") && inv.status.as_deref() == Some("converted")
    {
        let ci: Option<Invoice> = sqlx::query_as(
            "SELECT * FROM invoices WHERE invoice_type = 'commercial' AND ref_pi_number = $1 \
             AND is_deleted = false ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&inv.number)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(ci) = ci {
            converted_to = json!({"id": ci.id.to_string(), "full_number": full_or_number(&ci.full_number, &ci.number)});
        }
    }
    let discount_total: Decimal = items.iter().map(|i| i.discount_amount).sum();
    // 21 Nov 2026: the client card reads these (as on a quotation); without them it said "No client set."
    let (customer_org, customer_contact) =
        customer_refs(&mut conn, inv.customer_org_id, inv.customer_contact_id).await?;
    let payments_json: Vec<Value> = payments
        .iter()
        .map(|p| {
            json!({
                "id": p.id.to_string(), "amount": f(&p.amount), "currency": p.currency,
                "payment_date": resp::date_iso(&p.payment_date),
                "payment_method": p.payment_method,
                "payment_reference": p.payment_reference,
                "notes": p.notes,
            })
        })
        .collect::<Vec<_>>();
    let refunds_json: Vec<Value> = refunds.iter().map(|r| json!({
            "id": r.id.to_string(), "amount": f(&r.amount), "currency": r.currency,
            "exchange_rate": f(&if r.exchange_rate.is_zero() { Decimal::ONE } else { r.exchange_rate }),
            "paid_date": resp::date_iso(&r.paid_date),
            "bank_account_id": r.bank_account_id.map(|u| u.to_string()),
            "reference": r.reference,
        })).collect::<Vec<_>>();
    let items_json: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({
                "id": i.id.to_string(), "sort_order": i.sort_order, "name": i.name,
                "description": i.description, "sku": i.sku,
                "quantity": f(&i.quantity), "qty": f(&i.quantity),
                "unit_price": f(&i.unit_price),
                "unit": i.unit, "amount": f(&i.amount),
                "subtotal": f(&i.amount),
                "discount_pct": f(&i.discount_pct),
                "discount_amount": f(&i.discount_amount),
                "tax_id": i.tax_id.map(|u| u.to_string()), "tax_code": i.tax_code,
                "tax_pct": f(&i.tax_pct), "tax_amount": f(&i.tax_amount),
                "product_id": i.product_id.map(|u| u.to_string()),
                "parent_item_id": i.parent_item_id.map(|u| u.to_string()),
                "term_months": i.term_months, "price_rule": i.price_rule,
            })
        })
        .collect::<Vec<_>>();
    let mut out = inv_summary(&inv);
    let extra = json!({
        "revision": inv.revision,
        "revision_group": group,
        "revisions": revisions,
        "active_revision_id": active.map(|a| a.id.to_string()),
        "revision_reason": inv.revision_reason,
        "credit_of_invoice_id": inv.credit_of_invoice_id.map(|u| u.to_string()),
        "credited_by": credited_by,
        "converted_to": converted_to,
        "tax_amount": f(&D(inv.tax_total)),
        "discount_total": f(&discount_total),
        "customer_contact_id": inv.customer_contact_id.map(|u| u.to_string()),
        "customer_org": customer_org,
        "customer_contact": customer_contact,
        "customer_contact_person": inv.customer_contact_person,
        "payments": payments_json,
        "refunds": refunds_json,
        "customer_address": inv.customer_address,
        "customer_tax_id": inv.customer_tax_id,
        "customer_email": inv.customer_email,
        "customer_tel": inv.customer_tel,
        "payment_terms_text": inv.payment_terms_text,
        "notes": inv.notes,
        "tax_lines": inv.tax_lines,
        "incoterms": inv.incoterms,
        "carrier": inv.carrier,
        "hs_code": inv.hs_code,
        "country_of_origin": inv.country_of_origin,
        "shipped_from": inv.shipped_from,
    });
    if let Value::Object(m) = extra {
        out.extend(m);
    }
    out.insert("items".into(), Value::Array(items_json));
    Ok(resp::json(Value::Object(out)))
}

/// The invoice's organization `{id, name, email}` and contact `{id, full_name, email}` (null when unset or gone).
async fn customer_refs(
    db: &mut PgConnection,
    org_id: Option<Uuid>,
    contact_id: Option<Uuid>,
) -> ApiResult<(Value, Value)> {
    let org = match org_id {
        Some(id) => {
            let row: Option<(Uuid, String, Option<String>)> =
                sqlx::query_as("SELECT id, name, email FROM organizations WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut *db)
                    .await?;
            row.map(|(id, name, email)| json!({"id": id.to_string(), "name": name, "email": email}))
                .unwrap_or(Value::Null)
        }
        None => Value::Null,
    };
    let contact = match contact_id {
        Some(id) => {
            let row: Option<(Uuid, String, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT id, first_name, last_name, email FROM contacts WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
            row.map(|(id, first, last, email)| {
                let full = format!("{first} {}", last.unwrap_or_default());
                json!({"id": id.to_string(), "full_name": full.trim(), "email": email})
            })
            .unwrap_or(Value::Null)
        }
        None => Value::Null,
    };
    Ok((org, contact))
}

/// `PATCH /invoices/{invoice_id}`: edit a DRAFT invoice (header + items).
pub async fn update_invoice(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(invoice_id): Path<String>,
    PyJson(body): PyJson<InvoiceCreate>,
) -> ApiResult<Response> {
    if !(user.is_superadmin
        || has(&user, "finance:edit_invoice")
        || has(&user, "finance:create_invoice"))
    {
        return Err(ApiError::http(
            403,
            "Permission required: finance:edit_invoice",
        ));
    }
    let mut conn = db.conn().await?;
    let Some(mut inv) = load_invoice(&mut conn, pg_uuid(&invoice_id)?, false).await? else {
        return Err(ApiError::not_found("Invoice", Some(&invoice_id)));
    };
    if inv.status.as_deref() != Some("draft") {
        return Err(ApiError::http(400, "Only draft invoices can be edited."));
    }
    if inv.invoice_type.as_deref() == Some("credit_note") {
        return Err(ApiError::http(
            400,
            "A credit note mirrors the invoice it reverses and can't be edited. Delete the draft and issue it again.",
        ));
    }
    if body.currency.to_uppercase() != inv.currency.as_deref().unwrap_or("").to_uppercase() {
        return Err(ApiError::http(
            400,
            "An invoice's currency can't be changed after it is created. Delete this draft and create a new invoice.",
        ));
    }
    if Some(body.invoice_type.as_str()) != inv.invoice_type.as_deref() {
        return Err(ApiError::http(
            400,
            "An invoice's type can't be changed after it is created. Delete this draft and create a new invoice.",
        ));
    }
    let new_issue = match nonempty(&body.issue_date) {
        Some(s) => Some(fromiso(s)?),
        None => None,
    };
    let issue_changed = matches!(new_issue, Some(d) if d != inv.issue_date);
    if let Some(d) = new_issue {
        inv.issue_date = d;
    }
    if let Some(s) = nonempty(&body.due_date) {
        inv.due_date = Some(fromiso(s)?);
    }
    let billed = billed_name(
        &mut conn,
        body.customer_name.as_deref(),
        pg_uuid_opt(body.customer_org_id.as_deref())?,
        pg_uuid_opt(body.customer_contact_id.as_deref())?,
    )
    .await?;
    // The next query autoflushes these attributes (a non-UUID id fails here, before the rate lookup).
    sqlx::query(
        "UPDATE invoices SET customer_org_id = $2, customer_contact_id = $3, customer_name = $4, \
         customer_address = $5, customer_tax_id = $6, customer_contact_person = $7, quotation_id = $8, \
         deal_id = $9, project_id = $10, ref_pi_number = $11, payment_terms_text = $12, notes = $13, \
         incoterms = $14, carrier = $15, hs_code = $16, country_of_origin = $17, shipped_from = $18, \
         customer_email = $19, customer_tel = $20, issue_date = $21, due_date = $22, updated_at = now() \
         WHERE id = $1",
    )
    .bind(inv.id)
    .bind(pg_uuid_opt(body.customer_org_id.as_deref())?)
    .bind(pg_uuid_opt(body.customer_contact_id.as_deref())?)
    .bind(&billed)
    .bind(&body.customer_address)
    .bind(&body.customer_tax_id)
    .bind(&body.customer_contact_person)
    .bind(pg_uuid_opt(body.quotation_id.as_deref())?)
    .bind(pg_uuid_opt(body.deal_id.as_deref())?)
    .bind(pg_uuid_opt(body.project_id.as_deref())?)
    .bind(&body.ref_pi_number)
    .bind(&body.payment_terms_text)
    .bind(&body.notes)
    .bind(&body.incoterms)
    .bind(&body.carrier)
    .bind(&body.hs_code)
    .bind(&body.country_of_origin)
    .bind(&body.shipped_from)
    .bind(strip_or_none(body.customer_email.as_deref()))
    .bind(strip_or_none(body.customer_tel.as_deref()))
    .bind(inv.issue_date)
    .bind(inv.due_date)
    .execute(&mut *conn)
    .await?;
    // The rate follows the issue date: an explicit one wins, otherwise re-look it up when the date moved.
    let functional = granary_services::fx::functional_currency(&mut conn).await?;
    let currency = inv.currency.clone().unwrap_or_default();
    let new_rate = if currency.to_uppercase() == functional {
        Some(Decimal::ONE)
    } else if let Some(r) = body.exchange_rate {
        Some(r)
    } else if issue_changed || inv.exchange_rate.map(|r| r.is_zero()).unwrap_or(true) {
        Some(
            granary_services::fx::require_rate(
                &mut conn,
                &currency,
                &functional,
                Some(inv.issue_date),
            )
            .await?,
        )
    } else {
        None
    };
    if let Some(r) = new_rate {
        inv.exchange_rate = Some(r);
        sqlx::query("UPDATE invoices SET exchange_rate = $2, updated_at = now() WHERE id = $1")
            .bind(inv.id)
            .bind(r)
            .execute(&mut *conn)
            .await?;
    }
    let specs = item_specs(&mut conn, &body.items).await?;
    sqlx::query("DELETE FROM invoice_items WHERE invoice_id = $1")
        .bind(inv.id)
        .execute(&mut *conn)
        .await?;
    invoicing::write_items(&mut conn, &mut inv, &specs, None).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": inv.id.to_string(), "number": inv.number, "invoice_type": inv.invoice_type}),
    ))
}

granary_core::py_model! {
    /// `class InvReviseBody(BaseModel)` / `CreditNoteBody`.
    pub struct ReasonBody {
        pub reason: String,
    }
}

/// `POST /invoices/{invoice_id}/revise` (201): supersede a sent proforma with an editable draft copy.
pub async fn revise_invoice(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(invoice_id): Path<String>,
    PyJson(body): PyJson<ReasonBody>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:create_invoice")?;
    if strip(&body.reason).is_empty() {
        return Err(ApiError::http(
            400,
            "A reason for the revision is required.",
        ));
    }
    let mut conn = db.conn().await?;
    let Some(inv) = load_invoice(&mut conn, pg_uuid(&invoice_id)?, false).await? else {
        return Err(ApiError::not_found("Invoice", Some(&invoice_id)));
    };
    let items = invoice_items(&mut conn, inv.id).await?;
    if inv.invoice_type.as_deref() != Some("proforma") {
        return Err(ApiError::http(
            400,
            "Only proforma invoices can be revised. Issue a credit note for a commercial invoice.",
        ));
    }
    if inv.status.as_deref() != Some("sent") {
        return Err(ApiError::http(
            400,
            "Only a sent proforma can be revised. Edit drafts directly.",
        ));
    }
    let group = match inv.revision_group.as_deref() {
        Some(g) if !g.is_empty() => g.to_string(),
        _ => inv.number.clone(),
    };
    let new_rev = match inv.revision {
        Some(r) if r != 0 => r,
        _ => 1,
    } + 1;
    sqlx::query("UPDATE invoices SET status = 'superseded', updated_at = now() WHERE id = $1")
        .bind(inv.id)
        .execute(&mut *conn)
        .await?;
    let today = today_local(&mut conn).await?;
    let new = NewInvoice {
        number: format!("{group}-R{new_rev}"),
        full_number: Some(format!("{group}-{new_rev}")),
        revision: Some(new_rev),
        revision_group: Some(group.clone()),
        previous_version_id: Some(inv.id),
        revision_reason: Some(strip(&body.reason).to_string()),
        invoice_type: Some("proforma".into()),
        status: Some("draft".into()),
        ref_pi_number: inv.ref_pi_number.clone(),
        issue_date: Some(today),
        due_date: inv.due_date,
        notes: inv.notes.clone(),
        incoterms: inv.incoterms.clone(),
        carrier: inv.carrier.clone(),
        hs_code: inv.hs_code.clone(),
        country_of_origin: inv.country_of_origin.clone(),
        shipped_from: inv.shipped_from.clone(),
        amount_paid: Some(Decimal::ZERO),
        created_by_id: Some(user.id),
        ..copy_header(&inv)
    };
    let mut rev = insert_invoice(&mut conn, &new).await?;
    let specs: Vec<ItemSpec> = items
        .iter()
        .map(|it| invoicing::item_spec(it, SpecOverrides::default()))
        .collect();
    invoicing::write_items(&mut conn, &mut rev, &specs, None).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": rev.id.to_string(), "full_number": rev.full_number, "revision": rev.revision}),
    ))
}

/// `POST /invoices/{invoice_id}/credit-note` (201): draft a full credit note against a commercial invoice.
pub async fn issue_credit_note(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(invoice_id): Path<String>,
    PyJson(body): PyJson<ReasonBody>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:create_invoice")?;
    if strip(&body.reason).is_empty() {
        return Err(ApiError::http(
            400,
            "A reason for the credit note is required.",
        ));
    }
    let mut conn = db.conn().await?;
    let Some(inv) = load_invoice(&mut conn, pg_uuid(&invoice_id)?, true).await? else {
        return Err(ApiError::not_found("Invoice", Some(&invoice_id)));
    };
    let items = invoice_items(&mut conn, inv.id).await?;
    if inv.invoice_type.as_deref() != Some("commercial") {
        return Err(ApiError::http(
            400,
            "Credit notes apply to commercial invoices only.",
        ));
    }
    if inv.status.as_deref() == Some("cancelled") {
        return Err(ApiError::http(400, "This invoice is already cancelled."));
    }
    let live: Option<Invoice> = sqlx::query_as(
        "SELECT * FROM invoices WHERE credit_of_invoice_id = $1 AND is_deleted = false \
         AND status != 'cancelled' LIMIT 1",
    )
    .bind(inv.id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(live) = live {
        return Err(ApiError::http(
            400,
            format!(
                "Credit note {} already exists for this invoice ({}).",
                full_or_number(&live.full_number, &live.number),
                status_str(&live.status)
            ),
        ));
    }
    let seq = granary_services::numbering::currency_sequence(
        &mut conn,
        "credit_note",
        inv.currency.as_deref(),
    )
    .await?;
    let cn_number = next_document_number(&mut conn, &seq).await?;
    let today = today_local(&mut conn).await?;
    let reason = strip(&body.reason).to_string();
    let new = NewInvoice {
        number: cn_number.clone(),
        full_number: Some(format!("{cn_number}-1")),
        revision: Some(1),
        revision_group: Some(cn_number.clone()),
        invoice_type: Some("credit_note".into()),
        credit_of_invoice_id: Some(inv.id),
        ref_pi_number: Some(inv.number.clone()),
        revision_reason: Some(reason.clone()),
        status: Some("draft".into()),
        issue_date: Some(today),
        notes: Some(format!(
            "Credit note for invoice {}. Reason: {reason}",
            inv.number
        )),
        amount_paid: Some(Decimal::ZERO),
        created_by_id: Some(user.id),
        ..copy_header(&inv)
    };
    let mut cn = insert_invoice(&mut conn, &new).await?;
    // Mirror every line (same discount, same tax snapshot) at a negative price.
    let specs: Vec<ItemSpec> = items
        .iter()
        .map(|it| {
            invoicing::item_spec(
                it,
                SpecOverrides {
                    unit_price: Some(-it.unit_price),
                    ..Default::default()
                },
            )
        })
        .collect();
    invoicing::write_items(&mut conn, &mut cn, &specs, Some(Decimal::ZERO)).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": cn.id.to_string(), "number": cn.number, "full_number": cn.full_number,
               "invoice_type": "credit_note", "status": cn.status}),
    ))
}

granary_core::py_model! {
    /// `class RefundBody(BaseModel)`.
    pub struct RefundBody {
        pub amount: Decimal => v::decimal_c(v::DecC::new().gt("0").max_digits(18).decimal_places(2)),
        pub paid_date: Option<String> = None,
        pub bank_account_id: Option<String> = None,
        pub reference: Option<String> = None
            => v::nullable(v::str_c(v::StrC::new().max_length(255))),
    }
}

/// `POST /invoices/{invoice_id}/refund` (201): pay back what a credit note left owing.
pub async fn record_refund(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(invoice_id): Path<String>,
    PyJson(body): PyJson<RefundBody>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:confirm_payment")?;
    let mut conn = db.conn().await?;
    let id = parse_uuid(&invoice_id, "invoice_id")?;
    let Some(mut inv) = load_invoice(&mut conn, id, true).await? else {
        return Err(ApiError::not_found("Invoice", Some(&invoice_id)));
    };
    let places = currency_places(&mut conn, inv.currency.as_deref()).await?;
    let amount = quantize_money(body.amount, places);
    let due = inv.refund_due;
    if due <= Decimal::ZERO {
        return Err(ApiError::http(
            400,
            "Nothing is due to be refunded on this invoice",
        ));
    }
    if amount <= Decimal::ZERO || amount > due {
        return Err(ApiError::http(
            400,
            format!(
                "The refund must be above 0 and at most the {} {} due",
                fmt_comma(&due),
                inv.currency.as_deref().unwrap_or("None")
            ),
        ));
    }
    let paid_on = match nonempty(&body.paid_date) {
        Some(s) => fromiso(s)?,
        None => today_local(&mut conn).await?,
    };
    let functional = granary_services::fx::functional_currency(&mut conn).await?;
    let currency = inv.currency.clone().unwrap_or_default();
    let rate = granary_services::fx::require_rate(&mut conn, &currency, &functional, Some(paid_on))
        .await?;
    let bank_id = match nonempty(&body.bank_account_id) {
        Some(b) => Some(parse_uuid(b, "bank_account_id")?),
        None => None,
    };
    if let Some(b) = bank_id {
        let found: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM bank_accounts WHERE id = $1 AND is_deleted = false")
                .bind(b)
                .fetch_optional(&mut *conn)
                .await?;
        if found.is_none() {
            return Err(ApiError::http(400, "Bank account not found"));
        }
    }
    let refund_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO invoice_refunds (id, invoice_id, amount, currency, exchange_rate, paid_date, \
         bank_account_id, reference, created_by_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(refund_id)
    .bind(inv.id)
    .bind(amount)
    .bind(&inv.currency)
    .bind(rate)
    .bind(paid_on)
    .bind(bank_id)
    .bind(strip_or_none(body.reference.as_deref()))
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    inv.refund_due = quantize_money(due - amount, places);
    sqlx::query("UPDATE invoices SET refund_due = $2, updated_at = now() WHERE id = $1")
        .bind(inv.id)
        .bind(inv.refund_due)
        .execute(&mut *conn)
        .await?;
    let je = granary_services::gl_posting::post_refund(
        &mut conn,
        &inv,
        amount,
        paid_on,
        Some(rate),
        bank_id,
        Some(user.id),
        refund_id,
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": refund_id.to_string(), "refund_due": f(&inv.refund_due),
               "journal_entry_id": je.map(|j| j.id.to_string())}),
    ))
}

/// `_invoice_transition_allowed(inv, new_status)`.
fn invoice_transition_allowed(inv: &Invoice, new_status: Option<&str>) -> bool {
    let current = inv.status.as_deref();
    match new_status {
        Some("sent") => current == Some("draft"),
        Some("paid") => matches!(current, Some("sent" | "partially_paid" | "overdue")),
        Some("cancelled") => {
            if current == Some("draft") {
                return true;
            }
            // A proforma is a payment request with no ledger effect: it can be withdrawn while unpaid.
            inv.invoice_type.as_deref() == Some("proforma")
                && matches!(current, Some("sent" | "overdue"))
                && D(inv.amount_paid).is_zero()
        }
        _ => false,
    }
}

/// `PATCH /invoices/{invoice_id}/status`.
pub async fn update_invoice_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(invoice_id): Path<String>,
    PyJson(body): PyJson<Dict>,
) -> ApiResult<Response> {
    let is_super = user.is_superadmin;
    if !(is_super || has(&user, "finance:edit_invoice") || has(&user, "finance:create_invoice")) {
        return Err(ApiError::http(
            403,
            "Permission required: finance:edit_invoice",
        ));
    }
    let mut conn = db.conn().await?;
    let Some(mut inv) = load_invoice(&mut conn, pg_uuid(&invoice_id)?, false).await? else {
        return Err(ApiError::not_found("Invoice", Some(&invoice_id)));
    };
    let raw = body.get("status").cloned().unwrap_or(PyObj::None);
    let new_status: Option<&str> = match &raw {
        PyObj::Str(s) => Some(s.as_str()),
        _ => None,
    };
    if !invoice_transition_allowed(&inv, new_status) {
        let shown = if py_truthy(&raw) {
            py_str(&raw)
        } else {
            "nothing".to_string()
        };
        return Err(ApiError::http(
            400,
            format!(
                "An invoice cannot move from {} to {shown}",
                status_str(&inv.status)
            ),
        ));
    }
    let new_status = new_status.expect("allowed").to_string();
    if new_status == "sent" && !has(&user, "finance:send_invoice") && !is_super {
        return Err(ApiError::http(
            403,
            "Permission required: finance:send_invoice",
        ));
    }
    if new_status == "paid" && !has(&user, "finance:mark_paid") && !is_super {
        return Err(ApiError::http(
            403,
            "Permission required: finance:mark_paid",
        ));
    }
    if new_status == "sent" {
        // Large invoices, non-standard terms and credit notes need the Owner first
        granary_services::approvals::engine::ensure_or_submit(
            &mut conn, "invoice", inv.id, &user, "sent", None,
        )
        .await?;
    }
    inv.status = Some(new_status.clone());
    sqlx::query("UPDATE invoices SET status = $2, updated_at = now() WHERE id = $1")
        .bind(inv.id)
        .bind(&inv.status)
        .execute(&mut *conn)
        .await?;
    if new_status == "sent" {
        // a commercial invoice is posted; a credit note cancels its original
        invoicing::on_invoice_issued(&mut conn, &mut inv, Some(user.id)).await?;
    }
    drop(conn);
    db.commit().await?;
    granary_services::approvals::notify::dispatch_outbox();
    Ok(resp::json(
        json!({"id": inv.id.to_string(), "status": inv.status}),
    ))
}

/// `DELETE /invoices/{invoice_id}` (204): soft-delete a draft.
pub async fn delete_invoice(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(invoice_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:create_invoice")?;
    let mut conn = db.conn().await?;
    let Some(inv) = load_invoice(&mut conn, pg_uuid(&invoice_id)?, false).await? else {
        return Err(ApiError::not_found("Invoice", Some(&invoice_id)));
    };
    if inv.status.as_deref() != Some("draft") {
        return Err(ApiError::http(400, "Only draft invoices can be deleted"));
    }
    sqlx::query(
        "UPDATE invoices SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(inv.id)
    .bind(super::now_utc())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Invoice Line Item CRUD ──────────────────────────────────────────────────

/// The checks shared by the line-item endpoints: the draft invoice (not a credit note).
async fn draft_invoice_for_items(
    conn: &mut PgConnection,
    invoice_id: &str,
    not_draft: &str,
) -> ApiResult<Invoice> {
    let Some(inv) = load_invoice(conn, pg_uuid(invoice_id)?, false).await? else {
        return Err(ApiError::not_found("Invoice", Some(invoice_id)));
    };
    if inv.status.as_deref() != Some("draft") {
        return Err(ApiError::http(400, not_draft.to_string()));
    }
    if inv.invoice_type.as_deref() == Some("credit_note") {
        return Err(ApiError::http(
            400,
            "A credit note's lines mirror the invoice it reverses and can't be changed",
        ));
    }
    Ok(inv)
}

/// `POST /invoices/{invoice_id}/items` (201).
pub async fn add_invoice_item(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(invoice_id): Path<String>,
    PyJson(body): PyJson<InvoiceItemInput>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:create_invoice")?;
    let mut conn = db.conn().await?;
    let mut inv = draft_invoice_for_items(
        &mut conn,
        &invoice_id,
        "Items can only be added to draft invoices",
    )
    .await?;
    let spec = item_specs(&mut conn, std::slice::from_ref(&body))
        .await?
        .remove(0);
    let mut parent_id = None;
    if let Some(p) = nonempty(&body.parent_item_id) {
        let pid = parse_uuid(p, "parent_item_id")?;
        let found: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM invoice_items WHERE id = $1 AND invoice_id = $2")
                .bind(pid)
                .bind(inv.id)
                .fetch_optional(&mut *conn)
                .await?;
        if found.is_none() {
            return Err(ApiError::http(
                400,
                "The parent line is not on this invoice",
            ));
        }
        parent_id = Some(pid);
    }
    let max: Option<i32> = sqlx::query_scalar(
        "SELECT coalesce(max(sort_order), -1) FROM invoice_items WHERE invoice_id = $1",
    )
    .bind(inv.id)
    .fetch_one(&mut *conn)
    .await?;
    let next_order = max.unwrap_or(-1) + 1;
    let tax_id: Option<Uuid> = match &spec.tax_id {
        Some(t) => Some(pg_uuid(t)?),
        None => None,
    };
    let item_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO invoice_items (id, invoice_id, sort_order, name, description, sku, quantity, unit_price, \
         unit, currency, product_id, discount_pct, discount_amount, tax_id, tax_code, tax_pct, tax_amount, \
         amount, parent_item_id, term_months, price_rule) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, \
         $11, $12, 0, $13, $14, $15, 0, 0, $16, $17, $18)",
    )
    .bind(item_id)
    .bind(inv.id)
    .bind(next_order)
    .bind(&spec.name)
    .bind(&spec.description)
    .bind(&spec.sku)
    .bind(spec.quantity)
    .bind(spec.unit_price)
    .bind(spec.unit.clone().unwrap_or_else(|| "Unit".into()))
    .bind(inv.currency.clone().unwrap_or_else(|| "USD".into()))
    .bind(spec.product_id)
    .bind(D(spec.discount_pct))
    .bind(tax_id)
    .bind(&spec.tax_code)
    .bind(D(spec.tax_pct))
    .bind(parent_id)
    .bind(spec.term_months)
    .bind(&spec.price_rule)
    .execute(&mut *conn)
    .await?;
    invoicing::recalculate(&mut conn, &mut inv).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": item_id.to_string(), "invoice_total": f(&D(inv.total))}),
    ))
}

/// `PATCH /invoices/{invoice_id}/items/{item_id}`.
pub async fn update_invoice_item(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((invoice_id, item_id)): Path<(String, String)>,
    PyJson(Sent { model: body, keys }): PyJson<Sent<InvoiceItemPatch>>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:create_invoice")?;
    let mut conn = db.conn().await?;
    let mut inv = draft_invoice_for_items(
        &mut conn,
        &invoice_id,
        "Items can only be edited on draft invoices",
    )
    .await?;
    let item: Option<InvoiceItem> =
        sqlx::query_as("SELECT * FROM invoice_items WHERE id = $1 AND invoice_id = $2")
            .bind(pg_uuid(&item_id)?)
            .bind(inv.id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(mut item) = item else {
        return Err(ApiError::not_found("InvoiceItem", Some(&item_id)));
    };
    // changes = model_dump(exclude_unset=True, exclude={tax_id, tax_pct, clear_tax}); None values skipped.
    let sent = |k: &str| keys.iter().any(|x| x == k);
    if sent("name") {
        if let Some(v) = &body.name {
            item.name = v.clone();
        }
    }
    if sent("description") && body.description.is_some() {
        item.description = body.description.clone();
    }
    if sent("sku") && body.sku.is_some() {
        item.sku = body.sku.clone();
    }
    if sent("quantity") {
        if let Some(v) = body.quantity {
            item.quantity = v;
        }
    }
    if sent("unit_price") {
        if let Some(v) = body.unit_price {
            item.unit_price = v;
        }
    }
    if sent("unit") && body.unit.is_some() {
        item.unit = body.unit.clone();
    }
    if sent("discount_pct") {
        if let Some(v) = body.discount_pct {
            item.discount_pct = v;
        }
    }
    if sent("term_months") && body.term_months.is_some() {
        item.term_months = body.term_months;
    }
    if body.clear_tax {
        item.tax_id = None;
        item.tax_code = None;
        item.tax_pct = Decimal::ZERO;
    } else if nonempty(&body.tax_id).is_some() || body.tax_pct.is_some() {
        // A Tax row, or a custom rate typed on the line (code CUSTOM)
        let mut spec = vec![ItemSpec {
            tax_id: body.tax_id.clone(),
            tax_pct: body.tax_pct,
            ..Default::default()
        }];
        invoicing::resolve_line_taxes(&mut conn, &mut spec).await?;
        let s = spec.remove(0);
        item.tax_id = match &s.tax_id {
            Some(t) => Some(pg_uuid(t)?),
            None => None,
        };
        item.tax_code = s.tax_code;
        item.tax_pct = D(s.tax_pct);
    }
    sqlx::query(
        "UPDATE invoice_items SET name = $2, description = $3, sku = $4, quantity = $5, unit_price = $6, \
         unit = $7, discount_pct = $8, term_months = $9, tax_id = $10, tax_code = $11, tax_pct = $12, \
         updated_at = now() WHERE id = $1",
    )
    .bind(item.id)
    .bind(&item.name)
    .bind(&item.description)
    .bind(&item.sku)
    .bind(item.quantity)
    .bind(item.unit_price)
    .bind(&item.unit)
    .bind(item.discount_pct)
    .bind(item.term_months)
    .bind(item.tax_id)
    .bind(&item.tax_code)
    .bind(item.tax_pct)
    .execute(&mut *conn)
    .await?;
    invoicing::recalculate(&mut conn, &mut inv).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": item.id.to_string(), "invoice_total": f(&D(inv.total))}),
    ))
}

/// `DELETE /invoices/{invoice_id}/items/{item_id}` (204).
pub async fn delete_invoice_item(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((invoice_id, item_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:create_invoice")?;
    let mut conn = db.conn().await?;
    let mut inv = draft_invoice_for_items(
        &mut conn,
        &invoice_id,
        "Items can only be deleted from draft invoices",
    )
    .await?;
    let item: Option<InvoiceItem> =
        sqlx::query_as("SELECT * FROM invoice_items WHERE id = $1 AND invoice_id = $2")
            .bind(pg_uuid(&item_id)?)
            .bind(inv.id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(item) = item else {
        return Err(ApiError::not_found("InvoiceItem", Some(&item_id)));
    };
    let count: i64 =
        sqlx::query_scalar("SELECT count(id) FROM invoice_items WHERE invoice_id = $1")
            .bind(inv.id)
            .fetch_one(&mut *conn)
            .await?;
    if count <= 1 {
        return Err(ApiError::http(
            400,
            "Cannot delete the last item on an invoice",
        ));
    }
    sqlx::query("DELETE FROM invoice_items WHERE id = $1")
        .bind(item.id)
        .execute(&mut *conn)
        .await?;
    invoicing::recalculate(&mut conn, &mut inv).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `POST /invoices/{pi_id}/convert-to-commercial` (201): a paid proforma becomes a commercial invoice.
pub async fn convert_proforma_to_commercial(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(pi_id): Path<String>,
    PyJson(_body): PyJson<Dict>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:create_invoice")?;
    let mut conn = db.conn().await?;
    let Some(pi) = load_invoice(&mut conn, pg_uuid(&pi_id)?, false).await? else {
        return Err(ApiError::not_found("ProformaInvoice", Some(&pi_id)));
    };
    if pi.invoice_type.as_deref() != Some("proforma") {
        return Err(ApiError::http(
            400,
            "Source invoice is not a proforma invoice",
        ));
    }
    if matches!(
        pi.status.as_deref(),
        Some("cancelled" | "superseded" | "converted")
    ) {
        return Err(ApiError::http(
            409,
            format!("A {} proforma cannot be converted", status_str(&pi.status)),
        ));
    }
    // One commercial invoice per proforma: converting twice would bill the customer twice.
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT number FROM invoices WHERE invoice_type = 'commercial' AND ref_pi_number = $1 \
         AND is_deleted = false AND (status NOT IN ('cancelled', 'superseded')) LIMIT 1",
    )
    .bind(&pi.number)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(existing) = existing.filter(|e| !e.is_empty()) {
        return Err(ApiError::http(
            409,
            format!(
                "Proforma {} was already converted to invoice {existing}",
                pi.number
            ),
        ));
    }
    let pi_items = invoice_items(&mut conn, pi.id).await?;
    let inv_seq = granary_services::numbering::currency_sequence(
        &mut conn,
        "invoice",
        pi.currency.as_deref(),
    )
    .await?;
    let number = next_document_number(&mut conn, &inv_seq).await?;
    let today = today_local(&mut conn).await?;
    let new = NewInvoice {
        number: number.clone(),
        full_number: Some(format!("{number}-1")),
        revision: Some(1),
        revision_group: Some(number.clone()),
        invoice_type: Some("commercial".into()),
        ref_pi_number: Some(pi.number.clone()),
        issue_date: Some(today),
        due_date: pi.due_date,
        notes: pi.notes.clone(),
        incoterms: pi.incoterms.clone(),
        carrier: pi.carrier.clone(),
        hs_code: pi.hs_code.clone(),
        country_of_origin: pi.country_of_origin.clone(),
        shipped_from: pi.shipped_from.clone(),
        status: Some("draft".into()),
        amount_paid: pi.amount_paid,
        created_by_id: Some(user.id),
        ..copy_header(&pi)
    };
    let mut commercial = insert_invoice(&mut conn, &new).await?;
    let specs: Vec<ItemSpec> = pi_items
        .iter()
        .map(|it| invoicing::item_spec(it, SpecOverrides::default()))
        .collect();
    invoicing::write_items(&mut conn, &mut commercial, &specs, None).await?;
    // The commercial invoice now carries the balance: the proforma stops taking payments and reminders.
    let notes = format!(
        "{}\nConverted to commercial invoice {number}",
        pi.internal_notes.as_deref().unwrap_or("")
    );
    sqlx::query(
        "UPDATE invoices SET status = 'converted', internal_notes = $2, updated_at = now() WHERE id = $1",
    )
    .bind(pi.id)
    .bind(strip(&notes))
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": commercial.id.to_string(), "number": commercial.number, "ref_pi_number": pi.number}),
    ))
}
