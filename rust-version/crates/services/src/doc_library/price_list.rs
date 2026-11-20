//! doclib A2: the internal price list. `generate` renders the list prices of the catalog (through the
//! pricing service, never a number typed here) with the PDF service template `price_list`, one section per
//! currency that has its own price list, grouped by product family, and adds it as a new live version of the
//! Internal "Price List" document (kind price_list, category pricing, audiences {internal}), created on
//! first use. Nary never gets this document (search and deliver refuse kind price_list).

use std::collections::BTreeMap;

use chrono::NaiveDate;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{currency_places, get_optional};
use granary_db::rows::sales::ProductCatalog;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::versions::{self, InsertVersion};
use super::{access, load_document, Document, Version, OBJECT_PREFIX, WHERE};
use crate::pricing;
use crate::storage::{ensure_bucket, minio};

/// The PDF service template.
pub const TEMPLATE: &str = "price_list";
pub const KIND: &str = "price_list";

/// `1234567.5` with `places` decimals -> "1,234,567.50".
pub fn group_amount(amount: Decimal, places: u32) -> String {
    let s = amount.round_dp(places).abs().to_string();
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), f.to_string()),
        None => (s.clone(), String::new()),
    };
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let mut frac = frac;
    while (frac.len() as u32) < places {
        frac.push('0');
    }
    let sign = if amount.is_sign_negative() && !amount.is_zero() {
        "-"
    } else {
        ""
    };
    if places == 0 {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{frac}")
    }
}

fn humanise(s: &str) -> String {
    let t = s.replace('_', " ");
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// "per user, per 12 months" style terms of a catalog line.
fn terms(p: &ProductCatalog) -> String {
    let mut parts: Vec<String> = Vec::new();
    match p.price_metric.as_str() {
        "unit" | "" => {
            if let Some(u) = p.unit.as_deref().filter(|u| !u.trim().is_empty()) {
                parts.push(format!("per {}", u.trim()));
            }
        }
        m => parts.push(format!("per {}", m.replace('_', " "))),
    }
    if p.sale_model != "one_time" {
        match p.rate_period_months {
            Some(1) => parts.push("per month".into()),
            Some(12) => parts.push("per year".into()),
            Some(n) if n > 0 => parts.push(format!("per {n} months")),
            _ => parts.push(humanise(&p.sale_model).to_lowercase()),
        }
    }
    if let Some(m) = p.min_term_months.filter(|m| *m > 0) {
        parts.push(format!("minimum {m} months"));
    }
    if let Some(q) = p.min_quantity.filter(|q| *q > Decimal::ONE) {
        parts.push(format!("minimum {} units", q.normalize()));
    }
    parts.join(", ")
}

/// The template data: company header, generated date, one section per currency with families and lines.
pub async fn build_data(db: &mut PgConnection, on_date: NaiveDate) -> ApiResult<Value> {
    let title = get_optional(db, "documents.price_list.document_title")
        .await?
        .ok_or_else(|| {
            ApiError::http(
                400,
                format!(
                    "Setting 'documents.price_list.document_title' is not set. Set it in Settings → {WHERE}."
                ),
            )
        })?;
    let company = crate::verification::render::company_block(db).await?;
    let products: Vec<ProductCatalog> = sqlx::query_as(
        "SELECT * FROM product_catalog WHERE is_deleted = false AND is_active IS NOT FALSE \
         ORDER BY family NULLS LAST, sku",
    )
    .fetch_all(&mut *db)
    .await?;
    // currencies with their own list valid on that day
    let currencies: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT upper(currency) FROM catalog_prices WHERE valid_from <= $1 \
         AND (valid_to IS NULL OR valid_to >= $1) ORDER BY 1",
    )
    .bind(on_date)
    .fetch_all(&mut *db)
    .await?;
    let mut sections = Vec::new();
    for cur in &currencies {
        let places = currency_places(db, Some(cur)).await?;
        let mut families: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut count = 0;
        for p in &products {
            let price_text = match p.pricing_mode.as_str() {
                "list" => {
                    let has_own: Option<i32> = sqlx::query_scalar(
                        "SELECT 1 FROM catalog_prices WHERE product_id = $1 AND upper(currency) = $2 \
                         AND valid_from <= $3 AND (valid_to IS NULL OR valid_to >= $3) LIMIT 1",
                    )
                    .bind(p.id)
                    .bind(cur)
                    .bind(on_date)
                    .fetch_optional(&mut *db)
                    .await?;
                    if has_own.is_none() {
                        continue;
                    }
                    let q = pricing::price_for(db, p, cur, on_date).await?;
                    format!("{} {}", q.currency, group_amount(q.amount, places))
                }
                // the parent SKUs go in the note: a long list in the price column broke the table
                "percent_of_parent" => match p.percent_rate {
                    Some(r) => format!("{}% of the parent", r.normalize()),
                    None => continue,
                },
                _ => "Quoted per project".to_string(),
            };
            count += 1;
            let family = p
                .family
                .clone()
                .filter(|f| !f.trim().is_empty())
                .unwrap_or_else(|| "Other".into());
            families.entry(family).or_default().push(json!({
                "sku": p.sku,
                "name": p.name,
                "description": p.short_name.clone().filter(|s| !s.trim().is_empty() && *s != p.name),
                "terms": terms(p),
                "price": price_text,
                "note": addon_note(p),
            }));
        }
        if count == 0 {
            continue;
        }
        sections.push(json!({
            "currency": cur,
            "families": families
                .into_iter()
                .map(|(family, items)| json!({"family": humanise(&family), "items": items}))
                .collect::<Vec<_>>(),
        }));
    }
    if sections.is_empty() {
        return Err(ApiError::http(
            400,
            "The catalog has no list prices yet; add prices in Sales → Catalog first",
        ));
    }
    Ok(json!({
        "title": title,
        "generated_on": super::fingerprint::footer_date(on_date),
        "effective_date": on_date.format("%Y-%m-%d").to_string(),
        "company": company,
        "internal_mark": "Internal",
        "sections": sections,
    }))
}

/// The price list document, created on first use (owner and creator: `user`).
pub async fn ensure_document(db: &mut PgConnection, user: &AuthUser) -> ApiResult<Document> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM library_documents WHERE kind = 'price_list' AND is_deleted = false \
         ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(&mut *db)
    .await?;
    if let Some(id) = existing {
        if let Some(d) = load_document(db, id).await? {
            return Ok(d);
        }
    }
    let title = get_optional(db, "documents.price_list.document_title")
        .await?
        .unwrap_or_else(|| "Price List".into());
    let category = versions::category_id(db, "pricing").await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO library_documents (id, title, description, category_id, audiences, owner_id, language, \
         kind, created_by) VALUES ($1, $2, $3, $4, ARRAY['internal']::text[], $5, 'en', 'price_list', $5)",
    )
    .bind(id)
    .bind(title.chars().take(300).collect::<String>())
    .bind("The catalog's list prices, generated from Sales → Catalog. Internal only: never sent to customers.")
    .bind(category)
    .bind(user.id)
    .execute(&mut *db)
    .await?;
    load_document(db, id)
        .await?
        .ok_or_else(|| ApiError::http(500, "Document vanished"))
}

/// `POST /knowledge/documents/price-list/generate`: render the price list and add it as the new live
/// version. documents:create.
pub async fn generate(db: &mut PgConnection, user: &AuthUser) -> ApiResult<(Document, Version)> {
    if !access::can_create(user) {
        return Err(ApiError::http(403, "Permission required: documents:create"));
    }
    let today = granary_core::clock::today_local(db).await?;
    let data = build_data(db, today).await?;
    let pdf = crate::pdf::generate_pdf(TEMPLATE, &data, None, None)
        .await
        .map_err(|e| {
            ApiError::http(
                502,
                format!("The PDF service could not render the price list: {e}"),
            )
        })?;
    if !pdf.starts_with(b"%PDF") {
        return Err(ApiError::http(
            502,
            "The PDF service did not return a PDF for the price list",
        ));
    }
    let doc = ensure_document(db, user).await?;
    let sha = super::fingerprint::sha256_hex(&pdf);
    let key = format!("{OBJECT_PREFIX}{sha}");
    let store = minio();
    let bucket = granary_core::config::settings()
        .MINIO_BUCKET_DOCUMENTS
        .clone();
    ensure_bucket(&*store, &bucket)
        .await
        .map_err(ApiError::internal)?;
    store
        .put_object(&bucket, &key, &pdf, "application/pdf")
        .await
        .map_err(ApiError::internal)?;
    let pages = {
        let p = pdf.clone();
        tokio::task::spawn_blocking(move || {
            crate::pdftools::Document::open(&p)
                .map(|d| d.page_count() as i32)
                .ok()
        })
        .await
        .ok()
        .flatten()
    };
    let date = super::fingerprint::footer_date(today);
    let file_name = format!(
        "{} {}.pdf",
        doc.title.replace(['/', '\\'], "-"),
        today.format("%Y-%m-%d")
    );
    // the price list is internal: insert_version makes it live at once
    let v = versions::insert_version(
        db,
        user,
        &doc,
        InsertVersion {
            label: None,
            change_note: format!("Generated from the catalog on {date}"),
            key,
            name: file_name,
            mime: "application/pdf".into(),
            size: pdf.len() as i64,
            sha,
            pages,
            source: None,
            units: None,
            duration: None,
            bucket: super::BUCKET_DOCUMENTS.to_string(),
            card_approver: None,
        },
    )
    .await?;
    let doc = load_document(db, doc.id)
        .await?
        .ok_or_else(|| ApiError::http(500, "Document vanished"))?;
    Ok((doc, v))
}

/// The catalog terms note, plus the parent SKUs of a percent-of-parent add-on.
fn addon_note(p: &ProductCatalog) -> Option<String> {
    let parents = p
        .addon_for
        .as_ref()
        .filter(|a| !a.is_empty() && p.pricing_mode == "percent_of_parent")
        .map(|a| format!("Add-on for {}", a.join(", ")));
    match (
        p.terms_note.clone().filter(|n| !n.trim().is_empty()),
        parents,
    ) {
        (Some(n), Some(a)) => Some(format!("{n}\n{a}")),
        (n, a) => n.or(a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn grouping() {
        assert_eq!(
            group_amount(Decimal::from_str("1234567.5").unwrap(), 2),
            "1,234,567.50"
        );
        assert_eq!(group_amount(Decimal::from_str("950").unwrap(), 0), "950");
        assert_eq!(group_amount(Decimal::from_str("1000").unwrap(), 0), "1,000");
    }
}
