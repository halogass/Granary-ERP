//! Helpers shared by the fx / gl_posting / invoicing tests (agent S2): row structs with every column
//! filled, ledger setup (an account per role), settings writes.
#![allow(dead_code)]

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_db::rows::assets::AssetDepreciationLine;
use granary_db::rows::finance::{Expense, Invoice, Payment};

fn merge(mut base: Value, over: Value) -> Value {
    if let (Some(b), Some(o)) = (base.as_object_mut(), over.as_object()) {
        for (k, v) in o {
            b.insert(k.clone(), v.clone());
        }
    }
    base
}

const TS: &str = "2031-01-15T00:00:00Z";

/// An Invoice row with Python's defaults; `over` replaces fields (JSON as the row serializes).
pub fn invoice(over: Value) -> Invoice {
    let base = json!({
        "number": "INV-T", "invoice_type": "commercial", "revision": 0, "full_number": null,
        "revision_group": null, "previous_version_id": null, "revision_reason": null,
        "credit_of_invoice_id": null, "deal_id": null, "project_id": null, "quotation_id": null,
        "ref_pi_number": null, "customer_org_id": null, "customer_contact_id": null, "customer_name": null,
        "customer_address": null, "customer_tax_id": null, "customer_tel": null, "customer_email": null,
        "customer_contact_person": null, "customer_contact_email": null, "status": "draft",
        "currency": "JPY", "exchange_rate": null, "subtotal": "0", "tax_lines": null, "tax_total": "0",
        "total": "0", "amount_paid": "0", "amount_due": "0", "refund_due": "0",
        "issue_date": "2031-01-15", "due_date": null, "paid_at": null, "payment_method": null,
        "payment_reference": null, "payment_terms_text": null, "incoterms": null,
        "incoterms_destination": null, "carrier": null, "tracking_number": null, "hs_code": null,
        "country_of_origin": null, "shipped_from": null, "pdf_url": null, "authorized_by_id": null,
        "authorized_by_name": null, "authorized_by_title": null, "notes": null, "internal_notes": null,
        "created_by_id": null, "id": Uuid::new_v4().to_string(),
        "created_at": TS, "updated_at": TS, "is_deleted": false, "deleted_at": null
    });
    serde_json::from_value(merge(base, over)).unwrap()
}

pub fn payment(over: Value) -> Payment {
    let base = json!({
        "invoice_id": Uuid::new_v4().to_string(), "amount": "0", "currency": null, "exchange_rate": null,
        "payment_method": null, "payment_reference": null, "bank_name": null, "payment_date": "2031-01-20",
        "received_at": null, "status": "confirmed", "confirmed_by_id": null, "proof_url": null,
        "bank_account_id": null, "notes": null, "id": Uuid::new_v4().to_string(), "created_at": TS,
        "updated_at": TS, "is_deleted": false, "deleted_at": null
    });
    serde_json::from_value(merge(base, over)).unwrap()
}

pub fn expense(over: Value) -> Expense {
    let base = json!({
        "category": "travel", "description": "x", "amount": "0", "currency": "JPY", "exchange_rate": null,
        "expense_date": "2031-01-10", "project_id": null, "vendor_id": null, "work_order_id": null,
        "status": "approved", "receipt_url": null, "invoice_number": null, "tax_deductible": null,
        "withholding_applicable": null, "withholding_rate": null, "withholding_amount": null, "submitted_by_id": null,
        "employee_id": null, "reimbursed_at": null, "bank_account_id": null, "approved_by_id": null,
        "approved_at": null, "rejection_reason": null, "notes": null, "id": Uuid::new_v4().to_string(),
        "created_at": TS, "updated_at": TS, "is_deleted": false, "deleted_at": null
    });
    serde_json::from_value(merge(base, over)).unwrap()
}

pub fn depreciation_line(over: Value) -> AssetDepreciationLine {
    let base = json!({
        "asset_id": Uuid::new_v4().to_string(), "period_date": "2031-01-31", "period_label": null,
        "depreciation_amount": "0", "accumulated_depreciation": "0", "book_value": "0",
        "journal_entry_id": null, "status": null, "id": Uuid::new_v4().to_string(), "created_at": TS,
        "updated_at": TS
    });
    serde_json::from_value(merge(base, over)).unwrap()
}

pub async fn set_setting(db: &mut PgConnection, key: &str, value: &str) {
    let n = sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(key)
        .bind(value)
        .execute(&mut *db)
        .await
        .unwrap()
        .rows_affected();
    if n == 0 {
        sqlx::query(
            "INSERT INTO system_settings (id, key, value, value_type, category, is_public, is_encrypted) \
             VALUES ($1, $2, $3, 'string', 'finance', false, false)",
        )
        .bind(Uuid::new_v4())
        .bind(key)
        .bind(value)
        .execute(&mut *db)
        .await
        .unwrap();
    }
}

pub async fn new_account(db: &mut PgConnection, label: &str, category: &str, active: bool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO accounts (id, code, name, category, currency, is_active, normal_balance, is_deleted) \
         VALUES ($1, $2, $3, $4, 'JPY', $5, $6, false)",
    )
    .bind(id)
    .bind(format!("G{}", &id.simple().to_string()[..7]))
    .bind(format!("Test {label}"))
    .bind(category)
    .bind(active)
    .bind(if category == "assets" || category == "expenses" { "debit" } else { "credit" })
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

pub async fn map_role(db: &mut PgConnection, role: &str, account: Option<Uuid>) {
    sqlx::query("DELETE FROM gl_account_roles WHERE role = $1")
        .bind(role)
        .execute(&mut *db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO gl_account_roles (id, role, account_id) VALUES ($1, $2, $3)")
        .bind(Uuid::new_v4())
        .bind(role)
        .bind(account)
        .execute(&mut *db)
        .await
        .unwrap();
}

/// Fresh accounts mapped to every role (plus `extra` pseudo-roles), auto-posting on from 2020.
/// Returns role -> account id.
pub async fn ledger(db: &mut PgConnection, extra: &[&str]) -> HashMap<String, Uuid> {
    sqlx::query("DELETE FROM gl_account_roles")
        .execute(&mut *db)
        .await
        .unwrap();
    let mut roles: Vec<String> = granary_services::gl_posting::ROLE_LABELS
        .iter()
        .map(|(r, _)| r.to_string())
        .collect();
    roles.extend(extra.iter().map(|s| s.to_string()));
    let mut out = HashMap::new();
    for role in roles {
        let id = new_account(db, &role, "assets", true).await;
        map_role(db, &role, Some(id)).await;
        out.insert(role, id);
    }
    set_setting(db, "finance.gl_autopost_from", "2020-01-01").await;
    out
}

pub async fn add_rate(db: &mut PgConnection, from: &str, to: &str, rate: &str, day: &str) {
    sqlx::query(
        "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, effective_date, source, is_deleted) \
         VALUES ($1, $2, $3, $4::numeric, $5::date, 'manual', false)",
    )
    .bind(Uuid::new_v4())
    .bind(from)
    .bind(to)
    .bind(rate)
    .bind(day)
    .execute(&mut *db)
    .await
    .unwrap();
}

/// (account_id, debit, credit, currency, entry rate) of every line of a document event, like the
/// Python tests' `_lines`.
pub async fn lines_of(
    db: &mut PgConnection,
    source: &str,
    source_id: &str,
) -> Vec<(Uuid, Decimal, Decimal, Option<String>, Option<Decimal>)> {
    sqlx::query_as(
        "SELECT l.account_id, l.debit, l.credit, l.currency, e.exchange_rate FROM journal_lines l \
         JOIN journal_entries e ON e.id = l.entry_id \
         WHERE e.source = $1 AND e.source_id = $2 AND e.is_deleted = false ORDER BY e.created_at, l.sort_order",
    )
    .bind(source)
    .bind(source_id)
    .fetch_all(&mut *db)
    .await
    .unwrap()
}

/// `_balance`: net debit on a role's account, converted to JPY with the entry rate where needed.
pub fn balance(
    lines: &[(Uuid, Decimal, Decimal, Option<String>, Option<Decimal>)],
    accounts: &HashMap<String, Uuid>,
    role: &str,
) -> Decimal {
    let mut total = Decimal::ZERO;
    for (acc, dr, cr, cur, rate) in lines {
        if *acc == accounts[role] {
            let mult = if cur.as_deref() == Some("JPY") {
                Decimal::ONE
            } else {
                rate.unwrap()
            };
            total += (dr - cr) * mult;
        }
    }
    total
}

/// Whether the lines balance in the base currency.
pub fn balanced_in_base(
    lines: &[(Uuid, Decimal, Decimal, Option<String>, Option<Decimal>)],
) -> bool {
    let conv = |v: Decimal, cur: &Option<String>, rate: &Option<Decimal>| {
        v * if cur.as_deref() == Some("JPY") {
            Decimal::ONE
        } else {
            rate.unwrap()
        }
    };
    let dr: Decimal = lines.iter().map(|(_, d, _, c, r)| conv(*d, c, r)).sum();
    let cr: Decimal = lines.iter().map(|(_, _, k, c, r)| conv(*k, c, r)).sum();
    dr == cr
}

/// The detail text and status of an HTTPException-style error.
pub fn http_err(e: &granary_core::errors::ApiError) -> (u16, String) {
    let body = e.body();
    (
        e.status().as_u16(),
        body["detail"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| body.to_string()),
    )
}
