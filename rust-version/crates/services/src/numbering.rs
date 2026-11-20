//! Port of backend/app/services/numbering.py: the document sequence generator. All document numbers are
//! system-issued: the period's `document_sequences` row is locked (SELECT ... FOR UPDATE), incremented and
//! formatted as `{prefix}-{YYYY}-{MM}-{zfill(seq, padding)}`, e.g. `QUO-2025-05-00001`.
//!
//! Differences from Python worth knowing:
//! - Python changes the ORM object and the UPDATE is flushed later (autoflush / commit); here the UPDATE runs
//!   at once, inside the caller's transaction. Same rows, same lock, same `updated_at = now()`.
//! - Python reads the locked row with `populate_existing=True` (fresh across commits); there is no identity
//!   map here, so the locked row is always read fresh too.

use chrono::{DateTime, Datelike};
use chrono_tz::Tz;
use sqlx::PgConnection;
use uuid::Uuid;

use std::collections::HashMap;
use std::sync::RwLock;

use once_cell::sync::Lazy;

use granary_core::errors::ApiResult;

/// `DOCUMENT_SEQUENCES`: the configured number prefix of each sequence type, as (sequence_type, prefix,
/// description). app.seed creates a row per type from this list; `next_document_number` falls back to it
/// when a type has no row yet, so numbers never come out as the raw type key.
pub const DOCUMENT_SEQUENCES: &[(&str, &str, &str)] = &[
    // Quotations, proforma and commercial invoices: in the company's own currency (home) or another
    // (foreign), each with its own counter
    ("quotation_home", "QUO", "Sales Quotation"),
    (
        "quotation_foreign",
        "QUO/X",
        "Sales Quotation (foreign currency)",
    ),
    ("proforma_home", "PI", "Proforma Invoice"),
    (
        "proforma_foreign",
        "PI/X",
        "Proforma Invoice (foreign currency)",
    ),
    ("invoice_home", "INV", "Commercial Invoice"),
    (
        "invoice_foreign",
        "INV/X",
        "Commercial Invoice (foreign currency)",
    ),
    // Internal entity numbers
    ("project", "PRJ", "Project Number"),
    ("employee", "EMP", "Employee Number"),
    ("partner_agreement", "PA", "Partner Agreement"),
    ("purchase_order", "PO", "Purchase Order"),
    ("work_order", "WO", "Work Order"),
    ("production", "PRD", "Production Order"),
    ("expense", "EXP", "Expense Report"),
    ("contract", "CTR", "Contract"),
    ("proposal", "PRP", "Solution Proposal"),
    ("rma", "RMA", "Return Merchandise Authorization"),
    ("journal_entry", "JE", "Journal Entry"),
    ("asset", "AST", "Fixed Asset"),
    ("rfq", "RFQ", "Request for Quotation"),
    ("goods_receipt", "GR", "Goods Receipt"),
    ("maintenance_request", "MR", "Maintenance Request"),
    ("maintenance_work_order", "MWO", "Maintenance Work Order"),
    ("helpdesk_ticket", "HD", "Helpdesk Ticket"),
    ("offer_letter", "OL", "Offer Letter"),
    ("memo", "MEMO", "Internal Memo"),
];

/// `_DEFAULT_PREFIXES`: sequence_type -> prefix, from DOCUMENT_SEQUENCES plus the credit notes (seeded by
/// migration 051, not by app.seed). A module-level dict in Python (tests `monkeypatch.setitem` it), so a
/// lock-guarded map here.
pub static _DEFAULT_PREFIXES: Lazy<RwLock<HashMap<String, String>>> = Lazy::new(|| {
    let mut m: HashMap<String, String> = DOCUMENT_SEQUENCES
        .iter()
        .map(|(t, p, _)| (t.to_string(), p.to_string()))
        .collect();
    m.insert("credit_note_home".into(), "CN".into());
    m.insert("credit_note_foreign".into(), "CN/X".into());
    RwLock::new(m)
});

/// The sequence of a currency-split document (`quotation`, `proforma`, `invoice`, `credit_note`):
/// `<base>_home` in the company's own currency (`default_currency`), else `<base>_foreign`.
pub async fn currency_sequence(
    db: &mut PgConnection,
    base: &str,
    currency: Option<&str>,
) -> ApiResult<String> {
    let home = granary_core::settings_store::get_optional(db, "default_currency")
        .await?
        .unwrap_or_default();
    let cur = currency.unwrap_or("").trim();
    let side = if home.trim().is_empty() || cur.is_empty() || cur.eq_ignore_ascii_case(home.trim())
    {
        "home"
    } else {
        "foreign"
    };
    Ok(format!("{base}_{side}"))
}

/// `_DEFAULT_PREFIXES.get(sequence_type)`.
pub fn default_prefix(sequence_type: &str) -> Option<String> {
    _DEFAULT_PREFIXES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(sequence_type)
        .cloned()
}

/// The locked row of one period.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DocumentSequence {
    pub id: Uuid,
    pub sequence_type: String,
    pub prefix: String,
    pub year: i32,
    pub month: i32,
    pub current_sequence: i32,
    pub sequence_padding: Option<i32>,
    pub last_generated: Option<String>,
}

const COLS: &str =
    "id, sequence_type, prefix, year, month, current_sequence, sequence_padding, last_generated";

/// `_lock_sequence(db, sequence_type, year, month)`.
pub async fn _lock_sequence(
    db: &mut PgConnection,
    sequence_type: &str,
    year: i32,
    month: i32,
) -> Result<Option<DocumentSequence>, sqlx::Error> {
    sqlx::query_as::<_, DocumentSequence>(&format!(
        "SELECT {COLS} FROM document_sequences \
         WHERE sequence_type = $1 AND year = $2 AND month = $3 FOR UPDATE"
    ))
    .bind(sequence_type)
    .bind(year)
    .bind(month)
    .fetch_optional(&mut *db)
    .await
}

/// Python's `str.zfill(width)` on the decimal text of an integer (a leading sign stays in front).
pub fn zfill(s: &str, width: i64) -> String {
    let len = s.chars().count() as i64;
    if width <= len {
        return s.to_string();
    }
    let fill = "0".repeat((width - len) as usize);
    match s.chars().next() {
        Some(c @ ('+' | '-')) => format!("{c}{fill}{}", &s[1..]),
        _ => format!("{fill}{s}"),
    }
}

/// The number text: `f"{prefix}-{year}-{month:02d}-{str(seq).zfill(padding)}"`.
pub fn format_number(prefix: &str, year: i32, month: i32, sequence: i64, padding: i64) -> String {
    // `{month:02d}` pads with zeros after the sign, like zfill.
    let month = zfill(&month.to_string(), 2);
    format!(
        "{prefix}-{year}-{month}-{}",
        zfill(&sequence.to_string(), padding)
    )
}

/// `next_document_number(db, sequence_type)`: the year and month are the company's local ones (setting
/// `timezone`).
pub async fn next_document_number(db: &mut PgConnection, sequence_type: &str) -> ApiResult<String> {
    let now = granary_core::clock::now_local(&mut *db).await?;
    next_document_number_at(db, sequence_type, now).await
}

/// `next_document_number` with the local "now" given (tests replace `clock.now_in` in Python).
pub async fn next_document_number_at(
    db: &mut PgConnection,
    sequence_type: &str,
    now: DateTime<Tz>,
) -> ApiResult<String> {
    let (year, month) = (now.year(), now.month() as i32);

    let mut seq = _lock_sequence(db, sequence_type, year, month).await?;

    if seq.is_none() {
        // Carry the prefix/padding over from the most recent period of this type (one row per period).
        let reference: Option<(String, Option<i32>)> = sqlx::query_as(
            "SELECT prefix, sequence_padding FROM document_sequences WHERE sequence_type = $1 \
             ORDER BY year DESC, month DESC LIMIT 1",
        )
        .bind(sequence_type)
        .fetch_optional(&mut *db)
        .await?;
        let (prefix, padding) = match reference {
            Some((p, pad)) => (p, pad),
            None => (
                default_prefix(sequence_type).unwrap_or_else(|| sequence_type.to_uppercase()),
                Some(5),
            ),
        };
        // ON CONFLICT DO NOTHING: a concurrent request may have created the row for this period already
        // (uq_doc_seq_type_year_month). SQLAlchemy's Core insert fills the Python-side defaults: id (uuid4)
        // and suffix_separator ("-"); created_at / updated_at are server defaults.
        sqlx::query(
            "INSERT INTO document_sequences \
             (id, sequence_type, prefix, suffix_separator, year, month, current_sequence, sequence_padding) \
             VALUES ($1, $2, $3, '-', $4, $5, 0, $6) \
             ON CONFLICT (sequence_type, year, month) DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(sequence_type)
        .bind(&prefix)
        .bind(year)
        .bind(month)
        .bind(padding)
        .execute(&mut *db)
        .await?;
        seq = _lock_sequence(db, sequence_type, year, month).await?;
    }

    // Python: `seq.current_sequence += 1` on the row (AttributeError on None cannot happen after the insert).
    let mut seq = seq
        .ok_or_else(|| anyhow::anyhow!("'NoneType' object has no attribute 'current_sequence'"))?;
    if seq.prefix == sequence_type.to_uppercase() {
        // A row created before its type had a configured prefix carries the raw type key; correct it so the
        // remaining numbers of this period use the configured one.
        if let Some(p) = default_prefix(sequence_type) {
            seq.prefix = p;
        }
    }
    let next = seq.current_sequence as i64 + 1;
    // `str(n).zfill(None)` raises TypeError in Python when the padding column is NULL.
    let padding = seq
        .sequence_padding
        .ok_or_else(|| anyhow::anyhow!("'NoneType' object cannot be interpreted as an integer"))?;
    let full = format_number(&seq.prefix, year, month, next, padding as i64);
    sqlx::query(
        "UPDATE document_sequences SET current_sequence = $2, last_generated = $3, prefix = $4, \
         updated_at = now() WHERE id = $1",
    )
    .bind(seq.id)
    .bind(next)
    .bind(&full)
    .bind(&seq.prefix)
    .execute(&mut *db)
    .await?;
    Ok(full)
}
