//! Nary fix batch 4, agent D2: the rate card and cost sheets (internal, sales:view_costs) and the product
//! files Nary can send to customers.
//!
//! - `get_rate_card`: each active role and seniority with its man-day cost, the gross monthly salary it came
//!   from (when its source names one) or the monthly equivalent, and the proposal.cost.* settings. The same
//!   rows as GET /sales/rate-card (active only) priced the way nary::rates does.
//! - `get_cost_sheet`: a proposal's newest internal cost sheet (labour per workstream and role, non-labour
//!   items, contingency, the summary at low / likely / high), as GET /sales/documents/{id}/cost-sheet.
//! - `find_product_files`: datasheets, brochures and spec sheets by product, family or name: product files
//!   on catalog items (Sales → Catalog), Knowledge Base article files and customer-safe AI knowledge
//!   documents with an original file. The ids go into propose_customer_message `attach` as {type: "file"}.

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use granary_core::settings_store::{require_decimal, require_int};
use once_cell::sync::Lazy;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::{PgConnection, Postgres, QueryBuilder};
use uuid::Uuid;

use super::handlers::{iso_utc, py_str, truthy};
use crate::nary::outbound_files::human_size;

const COST_WHERE: &str = "Sales & CRM → Cost breakdown & rate card";

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn text(args: &Value, k: &str) -> String {
    match args.get(k) {
        Some(v) if truthy(Some(v)) => py_str(v).trim().to_string(),
        _ => String::new(),
    }
}

fn dec(d: Decimal) -> Value {
    json!(d.normalize().to_string())
}

/// "gross 12000000/month" in a rate's source (the rate research writes it so).
static GROSS: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"gross\s+([0-9][0-9.,]*)\s*/\s*month").unwrap());

/// The gross monthly salary a rate's source names, if it names one.
pub fn monthly_from_source(source: &str) -> Option<Decimal> {
    let caps = GROSS.captures(source)?;
    let raw = caps.get(1)?.as_str().replace(',', "");
    raw.parse::<Decimal>().ok()
}

/// The monthly salary a man-day cost stands for: cost × days_per_month ÷ load_factor, to the nearest 1,000 currency units.
pub fn monthly_equivalent(cost: Decimal, load: Decimal, days: i64) -> Option<Decimal> {
    if load.is_zero() {
        return None;
    }
    let m = cost * Decimal::from(days) / load / Decimal::from(1000);
    Some(m.round() * Decimal::from(1000))
}

/// `_h_get_rate_card`.
pub async fn _h_get_rate_card(
    db: &mut PgConnection,
    _user: &AuthUser,
    _args: Value,
) -> ApiResult<Value> {
    let load = require_decimal(db, "proposal.cost.load_factor", COST_WHERE).await?;
    let days = require_int(db, "proposal.cost.days_per_month", COST_WHERE).await?;
    let contingency = require_decimal(db, "proposal.cost.contingency_pct", COST_WHERE).await?;
    let min_monthly = require_decimal(db, "proposal.cost.min_monthly", COST_WHERE).await?;
    let threshold = require_decimal(db, "proposal.cost.update_threshold_pct", COST_WHERE).await?;
    let rows: Vec<granary_db::rows::documents::NaryRateCard> = sqlx::query_as(
        "SELECT * FROM nary_rate_card WHERE status = 'active' ORDER BY role, seniority, valid_from",
    )
    .fetch_all(&mut *db)
    .await?;
    // the latest valid_from per role and seniority (nary::costing::rates)
    let mut active: Vec<granary_db::rows::documents::NaryRateCard> = Vec::new();
    for r in rows {
        match active
            .iter_mut()
            .find(|x| x.role == r.role && x.seniority == r.seniority)
        {
            Some(x) if x.valid_from <= r.valid_from => *x = r,
            Some(_) => {}
            None => active.push(r),
        }
    }
    let roles: Vec<Value> = active
        .iter()
        .map(|r| {
            let source = r.source.clone().unwrap_or_default();
            let stated = monthly_from_source(&source);
            json!({
                "role": r.role,
                "seniority": r.seniority,
                "man_day_cost_base": dec(r.cost_base),
                "monthly_salary_base": stated.map(dec),
                "monthly_equivalent_base": monthly_equivalent(r.cost_base, load, days).map(dec),
                "source": if source.is_empty() { Value::Null } else { json!(source) },
                "valid_from": r.valid_from.format("%Y-%m-%d").to_string(),
                "proposed_by_nary": r.proposed_by_nary,
            })
        })
        .collect();
    Ok(json!({
        "internal": "Internal costs: never quote them to a customer.",
        "currency": "USD",
        "roles": roles,
        "settings": {
            "proposal.cost.load_factor": dec(load),
            "proposal.cost.days_per_month": days,
            "proposal.cost.contingency_pct": dec(contingency),
            "proposal.cost.min_monthly": dec(min_monthly),
            "proposal.cost.update_threshold_pct": dec(threshold),
        },
        "how": "man-day cost = max(monthly salary, min_monthly) x load_factor / days_per_month, to the nearest 1,000 currency units; \
                monthly_salary_base is the salary the rate's source names, monthly_equivalent_base the salary the cost stands for. \
                Settings → Sales & CRM → Cost breakdown & rate card.",
    }))
}

/// `_h_get_cost_sheet`.
pub async fn _h_get_cost_sheet(
    db: &mut PgConnection,
    _user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let raw = text(&args, "document_id");
    if raw.is_empty() {
        return err(
            "document_id is required (the proposal's id, from list_quotations or the deal)",
        );
    }
    let doc: Option<(Uuid, String, String, Option<String>, String, Option<Uuid>)> =
        match Uuid::parse_str(&raw) {
            Ok(id) => {
                sqlx::query_as(
                    "SELECT id, title, number, full_number, status, deal_id FROM sales_documents \
                     WHERE id = $1 AND is_deleted = false",
                )
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
            }
            Err(_) => {
                sqlx::query_as(
                    "SELECT id, title, number, full_number, status, deal_id FROM sales_documents \
                     WHERE (full_number = $1 OR number = $1) AND is_deleted = false ORDER BY created_at DESC LIMIT 1",
                )
                .bind(&raw)
                .fetch_optional(&mut *db)
                .await?
            }
        };
    let Some((id, title, number, full_number, status, deal_id)) = doc else {
        return err(format!("No proposal with id or number {raw}"));
    };
    let sheet = crate::nary::costing::latest(db, id).await?;
    let head = json!({
        "document_id": id.to_string(),
        "title": title,
        "number": full_number.filter(|f| !f.is_empty()).unwrap_or(number),
        "document_status": status,
        "deal_id": deal_id.map(|d| d.to_string()),
        "internal": "Internal costs: never quote them to a customer.",
    });
    let Some(sheet) = sheet else {
        let mut out = head;
        out["sheet"] = Value::Null;
        out["note"] = json!("This proposal has no cost sheet yet (Re-estimate on the proposal's Cost sheet tab makes one).");
        return Ok(out);
    };
    let c = &sheet.content;
    let mut out = head;
    out["sheet_status"] = json!(sheet.status);
    out["created_at"] = json!(iso_utc(&sheet.created_at));
    out["currency"] = json!(sheet.currency);
    out["labour"] = c.get("labour").cloned().unwrap_or(Value::Null);
    out["non_labour"] = c.get("non_labour").cloned().unwrap_or(Value::Null);
    out["contingency_pct"] = c.get("contingency_pct").cloned().unwrap_or(Value::Null);
    out["summary"] = c.get("summary").cloned().unwrap_or(Value::Null);
    out["category"] = c.get("category").cloned().unwrap_or(Value::Null);
    out["assumptions"] = c.get("assumptions").cloned().unwrap_or(json!([]));
    Ok(out)
}

/// The words of a search, lower-cased, without the very short ones.
fn words(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .map(|w| w.to_lowercase())
        .collect()
}

/// `AND <haystack> ILIKE '%word%'` for every word.
fn push_words(qb: &mut QueryBuilder<'_, Postgres>, haystack: &str, ws: &[String]) {
    for w in ws {
        let pat = format!(
            "%{}%",
            w.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        qb.push(format!(" AND {haystack} ILIKE "));
        qb.push_bind(pat);
    }
}

/// `_h_find_product_files`.
pub async fn _h_find_product_files(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let query = text(&args, "query");
    let ws = words(&query);
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 50);
    let mut files: Vec<Value> = Vec::new();

    // doclib A3: 0. the document library first (customer or partner documents with a live version; the
    // copy sent is stamped for the customer). Ids are doc:<document id>.
    files.extend(super::doc_tools::library_files(db, user, &ws, limit as usize).await?);

    // 1. product files on catalog items (Sales → Catalog → an item → Product files); doclib A3: not those
    // the library took over (the same stored object), which are listed above as library documents
    type ProdRow = (
        Uuid,
        String,
        i32,
        String,
        chrono::DateTime<chrono::Utc>,
        String,
        String,
        Option<String>,
    );
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT a.id, a.file_name, a.file_size, a.mime_type, a.created_at, p.sku, p.name, p.family \
         FROM attachments a JOIN product_catalog p ON a.entity_id = p.id::text \
         WHERE a.entity_type = 'catalog_product' AND p.is_deleted = false \
         AND NOT EXISTS (SELECT 1 FROM library_document_versions lv WHERE lv.file_object_key = a.object_key)",
    );
    push_words(
        &mut qb,
        "concat_ws(' ', p.sku, p.name, p.short_name, p.family, p.category, a.file_name)",
        &ws,
    );
    qb.push(" ORDER BY p.family NULLS LAST, p.sku, a.created_at DESC LIMIT ");
    qb.push_bind(limit);
    let rows: Vec<ProdRow> = qb.build_query_as::<ProdRow>().fetch_all(&mut *db).await?;
    for (id, name, size, mime, at, sku, pname, family) in rows {
        files.push(json!({
            "file_id": id.to_string(), "name": name, "size": human_size(i64::from(size)), "type": mime,
            "date": iso_utc(&at), "source": "product file",
            "product": {"sku": sku, "name": pname, "family": family},
        }));
    }

    // 2. Knowledge Base article files (only articles this person may read)
    type KbRow = (
        Uuid,
        String,
        i32,
        String,
        chrono::DateTime<chrono::Utc>,
        String,
        String,
    );
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT a.id, a.file_name, a.file_size, a.mime_type, a.created_at, k.title, a.entity_id \
         FROM attachments a JOIN kb_articles k ON a.entity_id = k.id::text \
         WHERE a.entity_type = 'kb_article' AND k.is_deleted = false",
    );
    push_words(
        &mut qb,
        "concat_ws(' ', k.title, array_to_string(k.tags, ' '), a.file_name)",
        &ws,
    );
    qb.push(" ORDER BY a.created_at DESC LIMIT ");
    qb.push_bind(limit);
    let rows: Vec<KbRow> = qb.build_query_as::<KbRow>().fetch_all(&mut *db).await?;
    for (id, name, size, mime, at, title, article_id) in rows {
        if crate::attachments_access::assert_can_access(db, user, "kb_article", &article_id)
            .await
            .is_err()
        {
            continue;
        }
        files.push(json!({
            "file_id": id.to_string(), "name": name, "size": human_size(i64::from(size)), "type": mime,
            "date": iso_utc(&at), "source": "knowledge base", "article": title,
        }));
    }

    // 3. AI knowledge documents marked safe for customers, with their original file
    type AiRow = (
        Uuid,
        String,
        Option<String>,
        Option<i32>,
        chrono::DateTime<chrono::Utc>,
    );
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT id, title, original_filename, original_size, created_at FROM ai_knowledge_docs \
         WHERE customer_safe = true AND is_active = true AND original_object_key IS NOT NULL \
         AND original_object_key <> '' AND system_key IS NULL",
    );
    push_words(
        &mut qb,
        "concat_ws(' ', title, description, original_filename)",
        &ws,
    );
    qb.push(" ORDER BY created_at DESC LIMIT ");
    qb.push_bind(limit);
    let rows: Vec<AiRow> = qb.build_query_as::<AiRow>().fetch_all(&mut *db).await?;
    for (id, title, fname, size, at) in rows {
        let name = fname
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| title.clone());
        files.push(json!({
            "file_id": id.to_string(), "type": crate::nary::outbound_files::mime_of(&name), "name": name,
            "size": human_size(i64::from(size.unwrap_or(0))), "date": iso_utc(&at),
            "source": "AI knowledge", "title": title,
        }));
    }

    files.truncate(limit as usize);
    let note = if files.is_empty() {
        if query.is_empty() {
            "No product files yet. They are added on Sales → Catalog (an item → Product files) or as files on a \
             Knowledge Base article."
                .to_string()
        } else {
            format!(
                "No file matches '{query}'. Try fewer words (a product family or model), or tell the user the \
                 datasheet is not in the ERP yet (Sales → Catalog → an item → Product files)."
            )
        }
    } else {
        "Send one with propose_customer_message attach [{\"type\": \"file\", \"id\": file_id}]."
            .to_string()
    };
    Ok(json!({"query": query, "count": files.len(), "files": files, "note": note}))
}
