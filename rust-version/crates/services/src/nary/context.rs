//! Port of backend/app/services/nary/context.py: context walls (design §12): which tools Nary gets depends
//! on who it is talking to.
//!
//! - internal (the Owner, staff): the assistant's permission-gated tools, as the person.
//! - customer:<contact id>: only that customer's own leads, deals and quotations, the public catalog
//!   (list prices only), knowledge documents an admin marked customer-safe, and approved, trusted
//!   company facts. Costs, volume tiers, margins, other customers and internal memory are not
//!   reachable: the tools that could return them are never built, and every customer tool's output
//!   is scrubbed of those keys as a second line.
//!
//! The wall is in tool construction, not in the prompt: a customer can't talk the model into a tool
//! it was never given.

use serde_json::{Map, Value};
use uuid::Uuid;

pub const INTERNAL: &str = "internal";
pub const CUSTOMER_TOOL_NAMES: [&str; 4] = [
    "customer_records",
    "catalog_public",
    "customer_safe_knowledge",
    "company_facts",
];
/// Never in a customer tool's output, at any depth.
pub const FORBIDDEN_KEYS: [&str; 22] = [
    "cost_price",
    "cost",
    "margin",
    "tier_pct",
    "tier_available_pct",
    "tiers",
    "volume_tiers",
    "notes",
    "internal_notes",
    "price_rule",
    "unit_equivalents",
    "assigned_to_id",
    "created_by_id",
    // Phase 4: the internal cost sheet and rate card
    "cost_sheet",
    "cost_base",
    "day_rate",
    "price_at_target",
    "price_at_floor",
    "target_pct",
    "floor_pct",
    "unit_cost_base",
    "contingency",
];

/// `customer_context(contact_id)`.
pub fn customer_context(contact_id: impl std::fmt::Display) -> String {
    format!("customer:{contact_id}")
}

/// `parse(context)`: ("internal", None) or ("customer", contact id). Err is the ValueError text (an
/// unknown context, or `uuid.UUID(ref)` failing: "badly formed hexadecimal UUID string").
pub fn parse(context: &str) -> Result<(&'static str, Option<Uuid>), String> {
    if context == INTERNAL {
        return Ok((INTERNAL, None));
    }
    let (kind, rf) = match context.split_once(':') {
        Some((k, r)) => (k, r),
        None => (context, ""),
    };
    if kind != "customer" || rf.is_empty() {
        return Err(format!("Unknown context '{context}'"));
    }
    py_uuid(rf)
        .map(|u| ("customer", Some(u)))
        .ok_or_else(|| "badly formed hexadecimal UUID string".to_string())
}

/// `uuid.UUID(hex)`: braces, "urn:uuid:" and hyphens are dropped, then 32 hex digits.
fn py_uuid(s: &str) -> Option<Uuid> {
    let mut h = s.replace("urn:", "").replace("uuid:", "");
    h = h.trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if h.len() != 32 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Uuid::parse_str(&h).ok()
}

/// `scrub(value)`: drop the forbidden keys at any depth.
pub fn scrub(value: &Value) -> Value {
    match value {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| !FORBIDDEN_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), scrub(v)))
                .collect::<Map<String, Value>>(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(scrub).collect()),
        other => other.clone(),
    }
}

// ─── The customer tools ──────────────────────────────────────────────────────

use granary_compat::pyjson::{date_isoformat, str_decimal, PyDecimal};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use serde_json::json;
use sqlx::PgConnection;

use crate::ai::tools::catalog::{handler, HandlerFuture};
use crate::ai::tools::Tool;

/// `scope(db)`: the contact and its organisation.
async fn scope(db: &mut PgConnection, contact_id: Uuid) -> ApiResult<(Uuid, Option<Uuid>)> {
    let org: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT organization_id FROM contacts WHERE id = $1")
            .bind(contact_id)
            .fetch_optional(&mut *db)
            .await?;
    Ok((contact_id, org.and_then(|o| o.0)))
}

/// `args.get(key) or ""` as a str.
fn arg_str(args: &Value, key: &str) -> String {
    match args.get(key) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

async fn records(db: &mut PgConnection, contact_id: Uuid) -> ApiResult<Value> {
    let (cid, oid) = scope(db, contact_id).await?;
    // owned(model, cid, oid): contact_id == cid OR organization_id == oid (when the contact has one)
    let leads: Vec<(String, String)> = sqlx::query_as(
        "SELECT title, status FROM leads WHERE (contact_id = $1 OR ($2::uuid IS NOT NULL AND organization_id = $2)) \
         AND is_deleted = false",
    )
    .bind(cid)
    .bind(oid)
    .fetch_all(&mut *db)
    .await?;
    let deals: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT title, stage FROM deals WHERE (contact_id = $1 OR ($2::uuid IS NOT NULL AND organization_id = $2)) \
         AND is_deleted = false",
    )
    .bind(cid)
    .bind(oid)
    .fetch_all(&mut *db)
    .await?;
    #[allow(clippy::type_complexity)]
    let quotes: Vec<(
        Option<String>,
        String,
        String,
        Option<String>,
        Option<rust_decimal::Decimal>,
        Option<chrono::NaiveDate>,
    )> = sqlx::query_as(
        "SELECT full_number, number, status, currency, total, valid_until FROM quotations \
             WHERE (customer_contact_id = $1 OR ($2::uuid IS NOT NULL AND customer_org_id = $2)) \
             AND is_deleted = false AND status IN ('sent', 'accepted', 'signed')",
    )
    .bind(cid)
    .bind(oid)
    .fetch_all(&mut *db)
    .await?;
    let projects: Vec<Value> = deals
        .iter()
        .map(|(title, stage)| {
            let label = match crate::crm::stages::deal_stage(stage.as_deref()) {
                Some(s) => json!(s.label),
                None => json!(stage),
            };
            json!({"title": title, "stage": label})
        })
        .collect();
    Ok(json!({
        "inquiries": leads.iter().map(|(t, s)| json!({"title": t, "status": s})).collect::<Vec<_>>(),
        "projects": projects,
        "quotations": quotes.iter().map(|(full, number, status, currency, total, valid)| json!({
            "number": match full.as_deref() { Some(f) if !f.is_empty() => f.to_string(), _ => number.clone() },
            "status": status,
            "currency": currency,
            "total": total.map(|t| str_decimal(&PyDecimal::from(t))),
            "valid_until": valid.map(date_isoformat),
        })).collect::<Vec<_>>(),
    }))
}

async fn catalog(db: &mut PgConnection, args: Value) -> ApiResult<Value> {
    use granary_db::rows::sales::ProductCatalog;
    let q = pystr::strip(&arg_str(&args, "query")).to_string();
    let currency = match args.get("currency") {
        Some(Value::String(s)) if !s.is_empty() => s.to_uppercase(),
        _ => "USD".to_string(),
    };
    let mut sql = String::from(
        "SELECT * FROM product_catalog WHERE is_active = true AND is_orderable = true AND is_deleted = false",
    );
    if !q.is_empty() {
        sql.push_str(" AND (name ILIKE $1 OR sku ILIKE $1 OR description ILIKE $1)");
    }
    sql.push_str(" LIMIT 10");
    let mut query = sqlx::query_as::<_, ProductCatalog>(&sql);
    if !q.is_empty() {
        query = query.bind(format!("%{q}%"));
    }
    let rows = query.fetch_all(&mut *db).await?;
    let on = granary_core::clock::today_local(db).await?;
    let mut out = Vec::new();
    for p in rows {
        let mode = if p.pricing_mode.is_empty() {
            "list"
        } else {
            p.pricing_mode.as_str()
        };
        let price = if mode == "list" {
            match crate::pricing::price_for(db, &p, &currency, on).await {
                Ok(quote) => json!({"currency": quote.currency,
                                    "list_price": str_decimal(&PyDecimal::from(quote.amount))}),
                Err(_) => Value::Null,
            }
        } else {
            Value::Null
        };
        let desc: String = p
            .description
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(400)
            .collect();
        out.push(
            json!({"sku": p.sku, "name": p.name, "description": desc, "sale_model": p.sale_model,
                        "price_metric": p.price_metric, "list_price": price}),
        );
    }
    Ok(json!({"products": out, "note": "List prices only. Quote nothing else."}))
}

async fn knowledge_search(db: &mut PgConnection, args: Value) -> ApiResult<Value> {
    let hits = crate::ai::knowledge::search(db, &arg_str(&args, "query"), 4, None, true).await?;
    Ok(json!({"results": hits.iter().map(|h| json!({
        "title": h.title, "content": h.content.chars().take(1200).collect::<String>()})).collect::<Vec<_>>()}))
}

async fn facts(db: &mut PgConnection) -> ApiResult<Value> {
    let rows = crate::nary::memory::recall(
        db,
        crate::nary::memory::Recall {
            layers: vec!["company".into()],
            trusted_only: true,
            limit: 30,
            ..Default::default()
        },
    )
    .await?;
    Ok(
        json!({"facts": rows.iter().map(|r| json!({"title": r.title, "content": r.content})).collect::<Vec<_>>()}),
    )
}

fn scrubbed<'a>(
    fut: impl std::future::Future<Output = ApiResult<Value>> + Send + 'a,
) -> HandlerFuture<'a> {
    Box::pin(async move { Ok(scrub(&fut.await?)) })
}

fn obj(props: Value, req: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": req, "additionalProperties": false})
}

/// `_customer_tools(contact_id)`.
pub fn _customer_tools(contact_id: Uuid) -> Vec<Tool> {
    vec![
        Tool::new(
            "customer_records",
            "This customer's own inquiries, projects and sent quotations.",
            obj(json!({}), &[]),
            handler(move |db, _user, _args| scrubbed(records(db, contact_id))),
            &[],
        ),
        Tool::new(
            "catalog_public",
            "Search the company's products: name, description and list price.",
            obj(
                json!({"query": {"type": "string"}, "currency": {"type": "string", "description": "a currency code, e.g. USD"}}),
                &["query", "currency"],
            ),
            handler(|db, _user, args| scrubbed(catalog(db, args))),
            &[],
        ),
        Tool::new(
            "customer_safe_knowledge",
            "Search product and company information that is safe to share.",
            obj(json!({"query": {"type": "string"}}), &["query"]),
            handler(|db, _user, args| scrubbed(knowledge_search(db, args))),
            &[],
        ),
        Tool::new(
            "company_facts",
            "Approved facts about the company (address, hours, policies).",
            obj(json!({}), &[]),
            handler(|db, _user, _args| scrubbed(facts(db))),
            &[],
        ),
    ]
}

/// `tools_for(context, user)`: the tool list for a conversation context. Internal contexts get the
/// person's own tools (Python fails on a None user there: here the user is required for internal).
pub fn tools_for(context: &str, user: Option<&AuthUser>) -> ApiResult<Vec<Tool>> {
    let (kind, contact_id) = parse(context).map_err(value_error)?;
    if kind == INTERNAL {
        let user = user.ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!(
                "'NoneType' object has no attribute 'is_superadmin'"
            ))
        })?;
        return Ok(crate::ai::tools::tools_for_user(user));
    }
    Ok(_customer_tools(
        contact_id.expect("a customer context has its contact"),
    ))
}

fn value_error(m: String) -> ApiError {
    ApiError::internal(crate::nary::governance::PyException::new("ValueError", m))
}

/// The `user` the customer tools are called with: Python passes None and they never read it.
fn nobody() -> AuthUser {
    AuthUser {
        id: Uuid::nil(),
        email: String::new(),
        name: String::new(),
        phone: None,
        avatar_url: None,
        department: None,
        position: None,
        is_active: false,
        is_superadmin: false,
        is_system_admin: false,
        portal_type: "internal".into(),
        partner_account_id: None,
        vendor_id: None,
        last_login_at: None,
        must_change_password: None,
        created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        updated_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        perms: Default::default(),
    }
}

/// `run_customer_tool(db, context, name, args)`: run one tool inside a customer context; a name outside
/// the wall is refused.
pub async fn run_customer_tool(
    db: &mut PgConnection,
    context: &str,
    name: &str,
    args: Option<Value>,
) -> ApiResult<Value> {
    let tools = tools_for(context, None)?;
    let Some(tool) = tools.into_iter().find(|t| t.name == name) else {
        return Ok(json!({"error": format!("'{name}' is not available in this conversation.")}));
    };
    let args = match args {
        Some(a) if crate::crm::attributes::py::truthy(&a) => a,
        _ => json!({}),
    };
    let user = nobody();
    (tool.handler)(db, &user, args).await
}
