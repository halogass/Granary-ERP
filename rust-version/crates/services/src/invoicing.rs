//! Port of backend/app/services/invoicing.py: invoice lines and totals, shared by finance (invoice CRUD)
//! and sales (quotation -> invoice).
//!
//! Every path that writes invoice items goes through `write_items()`, so the header's subtotal, tax and
//! total always equal the sum of the stored, rounded lines. A line's tax comes from the Tax table and is
//! snapshotted (code + rate) when the line is saved; `resolve_line_taxes()` does that for user input,
//! and paths that copy an existing invoice (revise, credit note, convert) pass the snapshot through.
//!
//! Python mutates the ORM `invoice` and lets the session flush it. Here the `&mut Invoice` is updated in
//! memory and the changed columns are written at once (with `updated_at = now()`, the ORM's onupdate);
//! the invoice row must already exist (Python flushes it before `write_items` for the FK).

use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::errors::{ApiError, ApiResult};
use granary_core::money::{compute_lines, LineInput, Totals, D};
use granary_core::settings_store::currency_places;
use granary_db::rows::finance::{Invoice, InvoiceItem};

/// A rate typed on the line rather than picked from the Tax table.
pub const CUSTOM_TAX_CODE: &str = "CUSTOM";

/// One line spec (Python: a dict). `None` = key absent; for `sort_order` the outer Option is the key
/// and the inner one its value (`item_spec` copies a NULL sort_order, which then takes the column
/// default 0). `quantity` and `unit_price` are required by `write_items` (Python raises KeyError).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemSpec {
    pub name: String,
    pub description: Option<String>,
    pub sku: Option<String>,
    pub unit: Option<String>,
    pub product_id: Option<Uuid>,
    pub sort_order: Option<Option<i32>>,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    pub discount_pct: Option<Decimal>,
    /// The raw tax id as given (a string from the API, a UUID string after resolution).
    pub tax_id: Option<String>,
    pub tax_pct: Option<Decimal>,
    pub tax_code: Option<String>,
    pub tax_name: Option<String>,
    pub term_months: Option<i32>,
    pub price_rule: Option<String>,
    /// `key` / `parent_key`: an add-on line names its parent's key (`str()` of whatever was given).
    pub key: Option<String>,
    pub parent_key: Option<String>,
}

fn truthy(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|v| !v.is_empty())
}

/// `resolve_line_taxes(db, specs)`: fill tax_pct / tax_code / tax_name on each spec from its tax_id
/// (in place). The tax must be active and usable on sales. A spec without tax_id but with a tax_pct
/// other than 0 keeps that rate as a custom tax (code CUSTOM) if it is within 0..100. Otherwise no tax.
pub async fn resolve_line_taxes(db: &mut PgConnection, specs: &mut [ItemSpec]) -> ApiResult<()> {
    // (id, code, name, rate, is_active, applies_to_sales)
    type TaxRow = (Uuid, String, String, Decimal, Option<bool>, Option<bool>);
    let mut ids: Vec<String> = Vec::new();
    for s in specs.iter() {
        if let Some(t) = truthy(&s.tax_id) {
            if !ids.iter().any(|i| i == t) {
                ids.push(t.to_string());
            }
        }
    }
    let mut taxes: Vec<TaxRow> = Vec::new();
    if !ids.is_empty() {
        let mut uuids = Vec::with_capacity(ids.len());
        for i in &ids {
            uuids.push(
                granary_compat::text::utils::parse_uuid(i, "tax_id")
                    .map_err(|e| ApiError::http(400, e))?,
            );
        }
        taxes = sqlx::query_as(
            "SELECT id, code, name, rate, is_active, applies_to_sales FROM taxes \
             WHERE id = ANY($1) AND is_deleted = false",
        )
        .bind(&uuids)
        .fetch_all(&mut *db)
        .await?;
    }
    for s in specs.iter_mut() {
        let Some(tid) = truthy(&s.tax_id).map(|t| t.to_string()) else {
            match s.tax_pct {
                Some(custom) if !custom.is_zero() => {
                    if custom < Decimal::ZERO || custom > Decimal::ONE_HUNDRED {
                        return Err(ApiError::http(
                            400,
                            "A custom tax rate must be between 0 and 100%",
                        ));
                    }
                    s.tax_id = None;
                    s.tax_pct = Some(custom);
                    s.tax_code = Some(CUSTOM_TAX_CODE.to_string());
                    s.tax_name = Some("Custom tax".to_string());
                }
                _ => {
                    s.tax_id = None;
                    s.tax_pct = Some(Decimal::ZERO);
                    s.tax_code = None;
                    s.tax_name = None;
                }
            }
            continue;
        };
        // taxes keyed by str(t.id): the spec's tax_id must be the canonical lower-case form to match.
        let Some(tax) = taxes.iter().find(|t| t.0.to_string() == tid) else {
            return Err(ApiError::http(400, format!("Tax {tid} not found")));
        };
        if !tax.4.unwrap_or(false) || !tax.5.unwrap_or(false) {
            return Err(ApiError::http(
                400,
                format!("Tax {} is not active for sales invoices", tax.1),
            ));
        }
        s.tax_id = Some(tax.0.to_string());
        s.tax_pct = Some(tax.3);
        s.tax_code = Some(tax.1.clone());
        s.tax_name = Some(tax.2.clone());
    }
    Ok(())
}

/// `_line_inputs(specs)`.
pub fn _line_inputs(specs: &[ItemSpec]) -> Vec<LineInput> {
    specs
        .iter()
        .map(|s| LineInput {
            quantity: s.quantity,
            unit_price: s.unit_price,
            discount_pct: D(s.discount_pct),
            tax_pct: D(s.tax_pct),
            tax_code: s.tax_code.clone(),
            tax_name: s.tax_name.clone(),
        })
        .collect()
}

/// `totals.tax_lines` as the JSONB list Python stores (`[{code, name, rate, base, amount}]`, floats).
pub fn tax_lines_json(totals: &Totals) -> Value {
    serde_json::to_value(&totals.tax_lines).expect("tax lines serialize")
}

/// Python `==` between two decoded JSON values: numbers compare by value (1 == 1.0).
fn py_json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            match (
                x.to_string().parse::<f64>().ok(),
                y.to_string().parse::<f64>().ok(),
            ) {
                (Some(p), Some(q)) => p == q,
                _ => x == y,
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_json_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).map(|w| py_json_eq(v, w)).unwrap_or(false))
        }
        _ => a == b,
    }
}

/// `apply_totals(invoice, totals, amount_due=None)`: in memory only (the pure part). Returns whether
/// any column changed (Python's ORM writes a row only when a value differs).
pub fn apply_totals(invoice: &mut Invoice, totals: &Totals, amount_due: Option<Decimal>) -> bool {
    let before = (
        invoice.subtotal,
        invoice.tax_total,
        invoice.tax_lines.clone(),
        invoice.total,
        invoice.amount_due,
    );
    invoice.subtotal = Some(totals.subtotal);
    invoice.tax_total = Some(totals.tax_total);
    invoice.tax_lines = Some(tax_lines_json(totals));
    invoice.total = Some(totals.total);
    invoice.amount_due = Some(match amount_due {
        Some(a) => a,
        None => {
            let rest = totals.total - D(invoice.amount_paid);
            // max(Decimal("0"), x)
            if rest > Decimal::ZERO {
                rest
            } else {
                Decimal::ZERO
            }
        }
    });
    let json_same = match (&before.2, &invoice.tax_lines) {
        (Some(a), Some(b)) => py_json_eq(a, b),
        (None, None) => true,
        _ => false,
    };
    !(before.0 == invoice.subtotal
        && before.1 == invoice.tax_total
        && json_same
        && before.3 == invoice.total
        && before.4 == invoice.amount_due)
}

/// Writes the header columns `apply_totals` sets.
async fn save_totals(db: &mut PgConnection, invoice: &Invoice) -> ApiResult<()> {
    sqlx::query(
        "UPDATE invoices SET subtotal = $2, tax_total = $3, tax_lines = $4, total = $5, amount_due = $6, \
         updated_at = now() WHERE id = $1",
    )
    .bind(invoice.id)
    .bind(invoice.subtotal)
    .bind(invoice.tax_total)
    .bind(&invoice.tax_lines)
    .bind(invoice.total)
    .bind(invoice.amount_due)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `apply_totals` followed by the flush of the invoice row (only when something changed).
pub async fn apply_and_save_totals(
    db: &mut PgConnection,
    invoice: &mut Invoice,
    totals: &Totals,
    amount_due: Option<Decimal>,
) -> ApiResult<()> {
    if apply_totals(invoice, totals, amount_due) {
        save_totals(db, invoice).await?;
    }
    Ok(())
}

/// `write_items(db, invoice, specs, amount_due=None)`: add one InvoiceItem per spec (taxes already
/// resolved or snapshotted) and set the totals. Parents are inserted before their add-ons.
pub async fn write_items(
    db: &mut PgConnection,
    invoice: &mut Invoice,
    specs: &[ItemSpec],
    amount_due: Option<Decimal>,
) -> ApiResult<Totals> {
    let places = currency_places(db, invoice.currency.as_deref()).await?;
    let totals = compute_lines(&_line_inputs(specs), places);
    // {str(s["key"]): uuid4() ...}: a repeated key keeps one id (the later one).
    let mut ids: Vec<(String, Uuid)> = Vec::new();
    for s in specs {
        if let Some(k) = &s.key {
            let id = Uuid::new_v4();
            match ids.iter_mut().find(|(kk, _)| kk == k) {
                Some(e) => e.1 = id,
                None => ids.push((k.clone(), id)),
            }
        }
    }
    let id_of = |k: &str| ids.iter().find(|(kk, _)| kk == k).map(|(_, v)| *v);
    for s in specs {
        if let Some(pk) = &s.parent_key {
            if id_of(pk).is_none() {
                return Err(ApiError::http(
                    400,
                    format!(
                        "Line '{}' is an add-on to a line that isn't on the invoice",
                        s.name
                    ),
                ));
            }
        }
    }
    // by_key: the last spec with a key wins (dict comprehension).
    let by_key = |k: &str| specs.iter().rev().find(|s| s.key.as_deref() == Some(k));
    fn depth<'a>(
        spec: &'a ItemSpec,
        seen: usize,
        n: usize,
        by_key: &dyn Fn(&str) -> Option<&'a ItemSpec>,
    ) -> usize {
        let parent = spec.parent_key.as_deref().and_then(by_key);
        match parent {
            None => 0,
            Some(_) if seen > n => 0,
            Some(p) => 1 + depth(p, seen + 1, n, by_key),
        }
    }
    let mut rows: Vec<(usize, &ItemSpec, &granary_core::money::LineResult)> = specs
        .iter()
        .zip(totals.lines.iter())
        .enumerate()
        .map(|(i, (s, r))| (i, s, r))
        .collect();
    // stable sort, like list.sort
    rows.sort_by_key(|(_, s, _)| depth(s, 0, specs.len(), &by_key));
    for (idx, s, r) in rows {
        let id = match &s.key {
            Some(k) => id_of(k).expect("key id"),
            None => Uuid::new_v4(),
        };
        let parent_item_id = s.parent_key.as_deref().and_then(id_of);
        let sort_order: i32 = match s.sort_order {
            None => idx as i32,
            Some(None) => 0, // explicit None takes the column default
            Some(Some(v)) => v,
        };
        let tax_id: Option<Uuid> = match &s.tax_id {
            None => None,
            Some(t) => Some(
                Uuid::parse_str(t)
                    .map_err(|_| ApiError::internal(anyhow::anyhow!("invalid tax_id {t}")))?,
            ),
        };
        let unit = truthy(&s.unit).unwrap_or("Unit").to_string();
        sqlx::query(
            "INSERT INTO invoice_items (id, parent_item_id, term_months, price_rule, invoice_id, sort_order, \
             name, description, sku, unit, product_id, currency, quantity, unit_price, discount_pct, \
             discount_amount, amount, tax_id, tax_code, tax_pct, tax_amount) VALUES \
             ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21)",
        )
        .bind(id)
        .bind(parent_item_id)
        .bind(s.term_months)
        .bind(&s.price_rule)
        .bind(invoice.id)
        .bind(sort_order)
        .bind(&s.name)
        .bind(&s.description)
        .bind(&s.sku)
        .bind(&unit)
        .bind(s.product_id)
        // currency=invoice.currency; None takes the column default "USD"
        .bind(invoice.currency.clone().unwrap_or_else(|| "USD".to_string()))
        .bind(s.quantity)
        .bind(s.unit_price)
        .bind(D(s.discount_pct))
        .bind(r.discount_amount)
        .bind(r.amount)
        .bind(tax_id)
        .bind(&s.tax_code)
        .bind(D(s.tax_pct))
        .bind(r.tax_amount)
        .execute(&mut *db)
        .await?;
    }
    apply_and_save_totals(db, invoice, &totals, amount_due).await?;
    Ok(totals)
}

/// Overrides for `item_spec(item, **overrides)` (only the keys callers override).
#[derive(Clone, Debug, Default)]
pub struct SpecOverrides {
    pub unit_price: Option<Decimal>,
    pub quantity: Option<Decimal>,
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub sort_order: Option<Option<i32>>,
}

/// `item_spec(item, **overrides)`: spec copied from a stored InvoiceItem, keeping its tax snapshot.
/// Quirk kept: `tax_name` is the item's tax_code (InvoiceItem stores no name), so copied invoices'
/// tax_lines name each tax by its code.
pub fn item_spec(item: &InvoiceItem, overrides: SpecOverrides) -> ItemSpec {
    let mut spec = ItemSpec {
        name: item.name.clone(),
        description: item.description.clone(),
        sku: item.sku.clone(),
        unit: item.unit.clone(),
        product_id: item.product_id,
        sort_order: Some(item.sort_order),
        quantity: item.quantity,
        unit_price: item.unit_price,
        discount_pct: Some(item.discount_pct),
        tax_id: item.tax_id.map(|t| t.to_string()),
        tax_pct: Some(item.tax_pct),
        tax_code: item.tax_code.clone(),
        tax_name: item.tax_code.clone(),
        term_months: item.term_months,
        price_rule: item.price_rule.clone(),
        key: Some(item.id.to_string()),
        parent_key: item.parent_item_id.map(|p| p.to_string()),
    };
    if let Some(v) = overrides.unit_price {
        spec.unit_price = v;
    }
    if let Some(v) = overrides.quantity {
        spec.quantity = v;
    }
    if let Some(v) = overrides.name {
        spec.name = v;
    }
    if let Some(v) = overrides.description {
        spec.description = v;
    }
    if let Some(v) = overrides.sort_order {
        spec.sort_order = Some(v);
    }
    spec
}

/// `recalculate(db, invoice)`: recompute every stored line and the header from the lines' saved inputs.
pub async fn recalculate(db: &mut PgConnection, invoice: &mut Invoice) -> ApiResult<Totals> {
    let items: Vec<InvoiceItem> =
        sqlx::query_as("SELECT * FROM invoice_items WHERE invoice_id = $1 ORDER BY sort_order")
            .bind(invoice.id)
            .fetch_all(&mut *db)
            .await?;
    let specs: Vec<ItemSpec> = items
        .iter()
        .map(|i| item_spec(i, SpecOverrides::default()))
        .collect();
    let places = currency_places(db, invoice.currency.as_deref()).await?;
    let totals = compute_lines(&_line_inputs(&specs), places);
    for (item, r) in items.iter().zip(totals.lines.iter()) {
        // The ORM writes only values that differ (Decimal equality ignores the scale).
        if item.discount_amount != r.discount_amount
            || item.amount != r.amount
            || item.tax_amount != r.tax_amount
        {
            sqlx::query(
                "UPDATE invoice_items SET discount_amount = $2, amount = $3, tax_amount = $4, \
                 updated_at = now() WHERE id = $1",
            )
            .bind(item.id)
            .bind(r.discount_amount)
            .bind(r.amount)
            .bind(r.tax_amount)
            .execute(&mut *db)
            .await?;
        }
    }
    apply_and_save_totals(db, invoice, &totals, None).await?;
    Ok(totals)
}

/// `on_invoice_issued(db, inv, user)`: a draft invoice was just marked sent. A commercial invoice
/// reaches the ledger; a credit note takes effect only now: the original is cancelled, what the
/// customer had paid on it becomes its refund_due, and the reversal is posted. Safe to run twice.
/// `inv` is updated in memory and in the database (issue_date of a credit note).
pub async fn on_invoice_issued(
    db: &mut PgConnection,
    inv: &mut Invoice,
    user: Option<Uuid>,
) -> ApiResult<()> {
    use crate::gl_posting;
    if inv.invoice_type.as_deref() != Some("credit_note") {
        gl_posting::post_invoice_issued(db, inv, user).await?;
        return Ok(());
    }
    if gl_posting::existing_entry(db, "credit_note", &inv.id.to_string())
        .await?
        .is_some()
    {
        return Ok(());
    }
    let original: Option<Invoice> = match inv.credit_of_invoice_id {
        Some(cid) => {
            sqlx::query_as("SELECT * FROM invoices WHERE id = $1 AND is_deleted = false FOR UPDATE")
                .bind(cid)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let Some(mut original) = original else {
        return Err(ApiError::http(
            400,
            "The invoice this credit note reverses no longer exists.",
        ));
    };
    let number = truthy(&inv.full_number).unwrap_or(&inv.number).to_string();
    let marker = format!("Credited by {number}");
    if original.status.as_deref() == Some("cancelled") {
        if !original
            .internal_notes
            .as_deref()
            .unwrap_or("")
            .contains(&marker)
        {
            return Err(ApiError::http(
                400,
                format!(
                    "Invoice {} is already cancelled; this credit note can't be sent.",
                    truthy(&original.full_number).unwrap_or(&original.number)
                ),
            ));
        }
    } else {
        // Whatever the customer already paid is now owed back to them, up to what the note credits.
        if D(original.amount_paid) > Decimal::ZERO {
            let paid = D(original.amount_paid);
            let credited = D(inv.total).abs();
            original.refund_due = if credited < paid { credited } else { paid };
        }
        original.status = Some("cancelled".to_string());
        let notes = format!(
            "{}\n{marker} ({})",
            original.internal_notes.as_deref().unwrap_or(""),
            truthy(&inv.revision_reason).unwrap_or("credit note")
        );
        original.internal_notes = Some(granary_compat::text::pystr::strip(&notes).to_string());
        sqlx::query(
            "UPDATE invoices SET refund_due = $2, status = $3, internal_notes = $4, updated_at = now() \
             WHERE id = $1",
        )
        .bind(original.id)
        .bind(original.refund_due)
        .bind(&original.status)
        .bind(&original.internal_notes)
        .execute(&mut *db)
        .await?;
        let today = granary_core::clock::today_local(db).await?;
        if inv.issue_date != today {
            inv.issue_date = today;
            sqlx::query("UPDATE invoices SET issue_date = $2, updated_at = now() WHERE id = $1")
                .bind(inv.id)
                .bind(today)
                .execute(&mut *db)
                .await?;
        }
    }
    gl_posting::post_credit_note(db, inv, &original, user).await?;
    Ok(())
}
