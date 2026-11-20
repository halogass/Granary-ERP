//! sales.py "Generate Invoice from Quotation (full or milestone)".

use std::collections::HashSet;

use axum::extract::Path;
use axum::response::Response;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use granary_compat::pyfmt::{format_number, PyNum};
use granary_core::auth::CurrentUser;
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::money::{pct_of, quantize_money, D};
use granary_core::settings_store::currency_places;
use granary_core::validation::v::{self, DecC};
use granary_core::validation::PyJson;
use granary_core::{ApiError, ApiResult};
use granary_db::rows::finance::{Invoice, PaymentSchedule};
use granary_db::rows::sales::{Quotation, QuotationItem};
use granary_services::crm::deal_hooks;
use granary_services::invoicing::{self, ItemSpec};

use super::quotations::{emit_quotation, load_quotation};
use super::{commit, fromisoformat_or_422, need};
use crate::resp;

granary_core::py_literal! {
    /// `Literal["commercial", "proforma"]`.
    pub enum InvoiceType { Commercial = "commercial", Proforma = "proforma" }
}

granary_core::py_literal! {
    /// `Literal["full", "milestone"]`.
    pub enum Mode { Full = "full", Milestone = "milestone" }
}

granary_core::py_model! {
    /// `class GenerateInvoiceBody(BaseModel)`. Tax comes from each quotation line (its Tax row or custom
    /// rate); there is no invoice-wide override.
    pub struct GenerateInvoiceBody {
        pub invoice_type: InvoiceType = InvoiceType::Commercial,
        pub due_date: Option<String> = None,
        pub mode: Mode = Mode::Full,
        /// for mode=milestone
        pub schedule_id: Option<String> = None,
        /// milestone without a schedule
        pub percentage: Option<Decimal> = None => v::nullable(v::decimal_c(
            DecC::new().gt("0").le("100").max_digits(5).decimal_places(2))),
    }
}

/// A quotation can be billed once the customer has it: sent, accepted, or signed (PO recorded).
const INVOICEABLE_QUOTATION_STATUSES: [&str; 3] = ["sent", "accepted", "signed"];

/// `_billable_lines(quotation, q_items, places)`: the lines that get billed (optional ones are offered,
/// not sold) and the header discount on them: a percentage applies to the billed lines, a fixed amount
/// is taken whole.
fn billable_lines<'a>(
    q: &Quotation,
    items: &'a [QuotationItem],
    places: u32,
) -> (Vec<&'a QuotationItem>, Decimal) {
    let billed: Vec<&QuotationItem> = items
        .iter()
        .filter(|qi| !qi.is_optional.unwrap_or(false))
        .collect();
    let lines_net: Decimal = billed
        .iter()
        .map(|qi| quantize_money(qi.amount, places))
        .sum();
    let header = match q.discount_type.as_deref() {
        Some("percentage") => pct_of(lines_net, q.discount_value, places),
        Some("fixed") => {
            let v = match q.discount_value {
                Some(d) if !d.is_zero() => Some(d),
                _ => q.discount_amount,
            };
            quantize_money(v, places).min(lines_net)
        }
        _ => Decimal::ZERO,
    };
    (billed, header)
}

/// `BILLING_WORDS`.
fn billing_word(every: i32) -> String {
    match every {
        1 => "monthly".into(),
        3 => "quarterly".into(),
        6 => "every six months".into(),
        12 => "annually".into(),
        n => format!("every {n} months"),
    }
}

/// `_term_text(qi)`: "12 months, billed annually" for a recurring line.
fn term_text(qi: &QuotationItem) -> Option<String> {
    let term = qi.term_months.filter(|t| *t != 0)?;
    if qi.sale_model.as_deref() != Some("recurring") {
        return None;
    }
    let billed = qi
        .billing_interval_months
        .filter(|e| *e != 0)
        .map(billing_word);
    Some(format!(
        "{term} months{}",
        billed.map(|b| format!(", billed {b}")).unwrap_or_default()
    ))
}

/// `_periods(qi)`.
fn periods(qi: &QuotationItem) -> Decimal {
    match (qi.term_months, qi.rate_period_months) {
        (Some(t), Some(r)) if qi.sale_model.as_deref() == Some("recurring") && t != 0 && r != 0 => {
            Decimal::from(t) / Decimal::from(r)
        }
        _ => Decimal::ONE,
    }
}

/// `_line_tax(qi)`: (tax_id, tax_pct).
fn line_tax(qi: &QuotationItem) -> (Option<String>, Option<Decimal>) {
    match qi.tax_id {
        Some(t) => (Some(t.to_string()), None),
        None => (None, Some(D(qi.tax_pct))),
    }
}

fn spec(name: String, description: Option<String>, unit_price: Decimal, unit: &str) -> ItemSpec {
    ItemSpec {
        name,
        description,
        sku: None,
        unit: Some(unit.into()),
        product_id: None,
        sort_order: None,
        quantity: Decimal::ONE,
        unit_price,
        discount_pct: None,
        tax_id: None,
        tax_pct: None,
        tax_code: None,
        tax_name: None,
        term_months: None,
        price_rule: None,
        key: None,
        parent_key: None,
    }
}

/// `_invoice_line_spec(qi, billed_ids)`: an invoice line from a quotation line. A recurring line bills
/// its whole term: the unit price becomes unit price x periods, and the description says what the term is.
fn invoice_line_spec(qi: &QuotationItem, billed_ids: &HashSet<Uuid>) -> ItemSpec {
    let term = term_text(qi);
    let parts: Vec<String> = [qi.description.clone(), term]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    let description = Some(parts.join("; ")).filter(|s| !s.is_empty());
    let (tax_id, tax_pct) = line_tax(qi);
    ItemSpec {
        name: qi.name.clone(),
        description,
        sku: qi.sku.clone(),
        unit: qi.unit.clone(),
        product_id: qi.product_id,
        sort_order: None,
        quantity: qi.quantity,
        unit_price: quantize_money(
            qi.unit_price * periods(qi),
            granary_services::pricing::PRICE_PLACES,
        ),
        discount_pct: Some(D(qi.discount_percentage)),
        tax_id,
        tax_pct,
        tax_code: None,
        tax_name: None,
        term_months: qi.term_months,
        price_rule: qi.price_rule.clone(),
        key: Some(qi.id.to_string()),
        parent_key: qi
            .parent_item_id
            .filter(|p| billed_ids.contains(p))
            .map(|p| p.to_string()),
    }
}

/// `_split(total, weights, places)`: `total` in proportion to `weights`, rounded, the largest part taking
/// the remainder.
fn split(total: Decimal, weights: &[Decimal], places: u32) -> Vec<Decimal> {
    let whole: Decimal = weights.iter().copied().sum();
    if weights.is_empty() {
        return Vec::new();
    }
    if whole <= Decimal::ZERO {
        let mut out = vec![total];
        out.extend(std::iter::repeat_n(Decimal::ZERO, weights.len() - 1));
        return out;
    }
    let mut parts: Vec<Decimal> = weights
        .iter()
        .map(|w| quantize_money(total * *w / whole, places))
        .collect();
    // max(range(n), key=...): the first largest weight
    let mut best = 0;
    for (i, w) in weights.iter().enumerate() {
        if *w > weights[best] {
            best = i;
        }
    }
    let sum: Decimal = parts.iter().copied().sum();
    parts[best] += total - sum;
    parts
}

/// One `_tax_groups` entry.
struct TaxGroup {
    tax_id: Option<String>,
    tax_pct: Option<Decimal>,
    gross: Decimal,
    net: Decimal,
    label: String,
}

/// `_tax_groups(billed_items, header_discount, places)`: billed lines grouped by tax: gross (line amounts)
/// and net (after the header discount share).
fn tax_groups(billed: &[&QuotationItem], header_discount: Decimal, places: u32) -> Vec<TaxGroup> {
    let mut groups: Vec<TaxGroup> = Vec::new();
    for qi in billed {
        let (tax_id, tax_pct) = line_tax(qi);
        let i = match groups
            .iter()
            .position(|g| g.tax_id == tax_id && g.tax_pct == tax_pct)
        {
            Some(i) => i,
            None => {
                let label = match qi.tax_code.as_deref().filter(|s| !s.is_empty()) {
                    Some(c) => c.to_string(),
                    None if D(qi.tax_pct).is_zero() => "No tax".into(),
                    None => "Tax".into(),
                };
                groups.push(TaxGroup {
                    tax_id,
                    tax_pct,
                    gross: Decimal::ZERO,
                    net: Decimal::ZERO,
                    label,
                });
                groups.len() - 1
            }
        };
        groups[i].gross += quantize_money(qi.amount, places);
    }
    let gross: Vec<Decimal> = groups.iter().map(|g| g.gross).collect();
    for (g, share) in groups
        .iter_mut()
        .zip(split(header_discount, &gross, places))
    {
        g.net = g.gross - share;
    }
    groups
}

fn fmt(spec: &str, n: PyNum) -> ApiResult<String> {
    format_number(&n, spec).map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))
}

/// `POST /quotations/{quotation_id}/generate-invoice` (201): bill the whole quotation (mode=full) or one
/// milestone (mode=milestone). The invoice never bills more than the quotation is worth: optional lines
/// are left out, the header discount carries over, and earlier invoices of the same type count.
pub async fn generate_invoice_from_quotation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(quotation_id): Path<String>,
    PyJson(body): PyJson<GenerateInvoiceBody>,
) -> ApiResult<Response> {
    need(&user, "finance:create_invoice")?;
    let mut conn = db.conn().await?;
    // Row lock: two clicks on "Generate" must not both pass the already-billed check.
    let q = load_quotation(&mut conn, &quotation_id, true).await?;
    if !q
        .status
        .as_deref()
        .is_some_and(|s| INVOICEABLE_QUOTATION_STATUSES.contains(&s))
    {
        return Err(ApiError::http(
            400,
            format!(
                "A {} quotation can't be invoiced: send it to the customer first",
                q.status.clone().unwrap_or_else(|| "None".into())
            ),
        ));
    }
    let q_items: Vec<QuotationItem> = sqlx::query_as(
        "SELECT * FROM quotation_items WHERE quotation_items.quotation_id = $1 ORDER BY quotation_items.sort_order",
    )
    .bind(q.id)
    .fetch_all(&mut *conn)
    .await?;
    let currency = q
        .currency
        .clone()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "USD".into());
    let inv_type = body.invoice_type.as_str();
    let places = currency_places(&mut conn, Some(&currency)).await?;
    let (billed, header_discount) = billable_lines(&q, &q_items, places);
    let billable_total: Decimal = billed
        .iter()
        .map(|qi| quantize_money(qi.amount, places))
        .sum::<Decimal>()
        - header_discount;
    let (already_billed,): (Decimal,) = sqlx::query_as(
        "SELECT COALESCE(SUM(invoices.subtotal), 0) FROM invoices WHERE invoices.quotation_id = $1 \
         AND invoices.invoice_type = $2 AND invoices.is_deleted = false \
         AND invoices.status NOT IN ('cancelled', 'superseded')",
    )
    .bind(q.id)
    .bind(inv_type)
    .fetch_one(&mut *conn)
    .await?;

    let mut schedule: Option<PaymentSchedule> = None;
    let mut pct: Option<Decimal> = None;
    let mut m_name = String::new();
    let mut amount = Decimal::ZERO;
    if body.mode == Mode::Full {
        if already_billed > Decimal::ZERO {
            return Err(ApiError::http(
                409,
                format!(
                    "This quotation already has a {inv_type} invoice. \
                     Cancel it first, or bill the remainder as a milestone."
                ),
            ));
        }
    } else {
        pct = body.percentage;
        m_name = format!("Milestone - {}", q.number);
        let mut amount_set = false;
        if let Some(sid) = body.schedule_id.as_deref().filter(|s| !s.is_empty()) {
            let id = crate::extract::parse_uuid(sid, "schedule_id")?;
            let s: Option<PaymentSchedule> = sqlx::query_as(
                "SELECT * FROM payment_schedules WHERE payment_schedules.id = $1 \
                 AND payment_schedules.is_deleted = false",
            )
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
            let Some(s) = s else {
                return Err(ApiError::not_found("PaymentSchedule", Some(sid)));
            };
            if s.quotation_id != Some(q.id) {
                return Err(ApiError::http(
                    400,
                    "That payment milestone belongs to another quotation",
                ));
            }
            if s.status.as_deref() == Some("invoiced")
                || s.invoice_id.is_some()
                || s.proforma_invoice_id.is_some()
            {
                return Err(ApiError::http(
                    409,
                    format!("Milestone '{}' is already invoiced", s.name),
                ));
            }
            m_name = s.name.clone();
            if let Some(a) = s.amount.filter(|a| !a.is_zero()) {
                amount = quantize_money(a, places);
                amount_set = true;
                pct = None;
            } else if let Some(p) = s.percentage.filter(|p| !p.is_zero()) {
                pct = Some(p);
            } else {
                return Err(ApiError::http(
                    400,
                    format!("Milestone '{}' has no amount or percentage", s.name),
                ));
            }
            schedule = Some(s);
        } else if pct.is_none() {
            return Err(ApiError::http(
                422,
                "A milestone needs a schedule_id or a percentage",
            ));
        }
        if let Some(p) = pct {
            amount = pct_of(billable_total, p, places);
            amount_set = true;
        }
        debug_assert!(amount_set);
        if amount <= Decimal::ZERO {
            return Err(ApiError::http(400, "The milestone amount must be positive"));
        }
        if already_billed + amount > billable_total {
            return Err(ApiError::http(
                409,
                format!(
                    "This milestone would bill {} of a {} quotation",
                    fmt(",", PyNum::from(already_billed + amount))?,
                    fmt(",", PyNum::from(billable_total))?
                ),
            ));
        }
    }

    let due_date = match body.due_date.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(fromisoformat_or_422(s)?),
        None => None,
    };
    let base = if inv_type == "proforma" {
        "proforma"
    } else {
        "invoice"
    };
    let seq =
        granary_services::numbering::currency_sequence(&mut conn, base, Some(&currency)).await?;
    let number = granary_services::numbering::next_document_number(&mut conn, &seq).await?;
    let today = today_local(&mut conn).await?;
    let functional = granary_services::fx::functional_currency(&mut conn).await?;
    // 400 if none on record
    let exchange_rate =
        granary_services::fx::require_rate(&mut conn, &currency, &functional, Some(today)).await?;
    let customer_name: Option<String> = match q.customer_org_id {
        Some(org) => sqlx::query_as::<_, (String,)>(
            "SELECT organizations.name FROM organizations WHERE organizations.id = $1",
        )
        .bind(org)
        .fetch_optional(&mut *conn)
        .await?
        .map(|r| r.0),
        None => None,
    };

    // The billed amount per tax, after the header discount: the header discount and a milestone are
    // split over these groups so each part carries its own tax.
    let groups = tax_groups(&billed, header_discount, places);
    let mut specs: Vec<ItemSpec> = Vec::new();
    if body.mode == Mode::Milestone {
        let label = match pct {
            Some(p) => format!("{m_name} ({}% of {})", pct_text(p), q.number),
            None => format!("{m_name} - {}", q.number),
        };
        let description = format!(
            "Payment for {}",
            q.subject
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| q.number.clone())
        );
        let nets: Vec<Decimal> = groups.iter().map(|g| g.net).collect();
        for (g, share) in groups.iter().zip(split(amount, &nets, places)) {
            let name = if groups.len() > 1 {
                format!("{label} - {}", g.label)
            } else {
                label.clone()
            };
            let mut s = spec(name, Some(description.clone()), share, "Fixed");
            s.tax_id = g.tax_id.clone();
            s.tax_pct = g.tax_pct;
            specs.push(s);
        }
        if specs.is_empty() {
            specs.push(spec(label, Some(description), amount, "Fixed"));
        }
    } else {
        let billed_ids: HashSet<Uuid> = billed.iter().map(|qi| qi.id).collect();
        specs = billed
            .iter()
            .map(|qi| invoice_line_spec(qi, &billed_ids))
            .collect();
        if header_discount > Decimal::ZERO {
            // Negative line(s), one per tax, so the discount also lowers each tax base.
            let label = if q.discount_type.as_deref() == Some("percentage") {
                let v = q.discount_value.ok_or_else(|| {
                    ApiError::internal(anyhow::anyhow!(
                        "TypeError: float() argument must be a string or a real number, not 'NoneType'"
                    ))
                })?;
                format!("Discount {}%", pct_text(v))
            } else {
                "Discount".to_string()
            };
            let gross: Vec<Decimal> = groups.iter().map(|g| g.gross).collect();
            for (g, share) in groups.iter().zip(split(header_discount, &gross, places)) {
                if !share.is_zero() {
                    let name = if groups.len() > 1 {
                        format!("{label} - {}", g.label)
                    } else {
                        label.clone()
                    };
                    let mut s = spec(
                        name,
                        Some(format!("Quotation {}", q.number)),
                        -share,
                        "Discount",
                    );
                    s.tax_id = g.tax_id.clone();
                    s.tax_pct = g.tax_pct;
                    specs.push(s);
                }
            }
        }
    }
    for (idx, s) in specs.iter_mut().enumerate() {
        s.sort_order = Some(Some(idx as i32));
    }
    invoicing::resolve_line_taxes(&mut conn, &mut specs).await?;

    let mut invoice: Invoice = sqlx::query_as(
        "INSERT INTO invoices (id, number, full_number, revision, revision_group, invoice_type, customer_org_id, \
         customer_contact_id, customer_name, quotation_id, deal_id, currency, exchange_rate, issue_date, due_date, \
         status, subtotal, tax_total, total, amount_paid, amount_due, created_by_id, tax_lines, refund_due, \
         is_deleted) VALUES ($1, $2, $3, 1, $2, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'draft', 0, 0, 0, 0, 0, \
         $14, '[]'::jsonb, 0, false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(&number)
    .bind(format!("{number}-1"))
    .bind(inv_type)
    .bind(q.customer_org_id)
    .bind(q.customer_contact_id)
    .bind(&customer_name)
    .bind(q.id)
    .bind(q.deal_id)
    .bind(&currency)
    .bind(exchange_rate)
    .bind(today)
    .bind(due_date)
    .bind(user.id)
    .fetch_one(&mut *conn)
    .await?;
    invoicing::write_items(&mut conn, &mut invoice, &specs, None).await?;

    if let Some(s) = &schedule {
        let col = if inv_type == "proforma" {
            "proforma_invoice_id"
        } else {
            "invoice_id"
        };
        sqlx::query(&format!(
            "UPDATE payment_schedules SET {col} = $2, status = 'invoiced', updated_at = now() WHERE id = $1"
        ))
        .bind(s.id)
        .bind(invoice.id)
        .execute(&mut *conn)
        .await?;
    }
    let inv_number = invoice
        .full_number
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| invoice.number.clone());
    deal_hooks::log_quotation_event(&mut conn, &q, "invoiced", Some(user.id), Some(&inv_number))
        .await?;
    let mut extra = Map::new();
    extra.insert("invoice_id".into(), Value::String(invoice.id.to_string()));
    extra.insert("invoice_type".into(), json!(invoice.invoice_type.clone()));
    emit_quotation(&mut conn, "quotation.invoiced", &q, &user, extra).await?;
    commit(&mut conn).await?;
    Ok(resp::json_status(
        201,
        json!({"id": invoice.id.to_string(), "number": invoice.number, "invoice_type": invoice.invoice_type}),
    ))
}

/// A percentage in a line's text, exact and without trailing zeros ("12.5", "33.333", "10"). 13 Nov 2026: it went
/// through f64 and "g" before, which kept only 6 significant digits.
fn pct_text(p: Decimal) -> String {
    p.normalize().to_string()
}
