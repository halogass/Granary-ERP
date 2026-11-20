//! Port of backend/app/services/crm/timeline.py: unified record timeline, every touchpoint against one CRM
//! record, newest first.
//!
//! Merges the sources that already exist and already carry the linkage we need:
//!   - Activity            polymorphic via entity_type/entity_id, plus contact/lead/deal FKs
//!   - CrmEmailThread      linked_org_id / linked_contact_id / linked_lead_id / linked_deal_id
//!   - Quotation           customer_org_id / customer_contact_id / deal_id
//!   - Invoice             customer_org_id / customer_contact_id / deal_id
//!
//! Read-only. Callers must have already authorized access to the record itself.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyfmt::{format_number, PyNum};
use granary_compat::pyjson::{date_isoformat, PyDateTime};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};

pub const OBJECT_TYPES: &[&str] = &["organization", "contact", "lead", "deal"];

/// `_iso(dt)` for a timestamptz (asyncpg hands back UTC-aware datetimes).
fn iso_dt(dt: &DateTime<Utc>) -> String {
    PyDateTime::from_chrono_utc(dt).isoformat()
}

/// The ids each source can be queried by (`_resolve_scope`); sets in Python, de-duplicated lists here.
#[derive(Debug, Default, Clone)]
pub struct Scope {
    pub org_ids: Vec<Uuid>,
    pub contact_ids: Vec<Uuid>,
    pub lead_ids: Vec<Uuid>,
    pub deal_ids: Vec<Uuid>,
}

fn add(v: &mut Vec<Uuid>, id: Uuid) {
    if !v.contains(&id) {
        v.push(id);
    }
}

/// `_resolve_scope(db, object_type, record_id)`: fan a record out to the ids each source can be queried by.
/// An organization pulls in its contacts, leads and deals so its timeline shows the whole relationship.
pub async fn resolve_scope(
    db: &mut PgConnection,
    object_type: &str,
    record_id: Uuid,
) -> Result<Scope, sqlx::Error> {
    let mut scope = Scope::default();
    match object_type {
        "organization" => {
            add(&mut scope.org_ids, record_id);
            for (table, target) in [
                ("contacts", &mut scope.contact_ids),
                ("leads", &mut scope.lead_ids),
                ("deals", &mut scope.deal_ids),
            ] {
                let ids: Vec<Uuid> = sqlx::query_scalar(&format!(
                    "SELECT {table}.id FROM {table} WHERE {table}.organization_id = $1 AND {table}.is_deleted = false"
                ))
                .bind(record_id)
                .fetch_all(&mut *db)
                .await?;
                for id in ids {
                    add(target, id);
                }
            }
        }
        "contact" => add(&mut scope.contact_ids, record_id),
        "lead" => {
            add(&mut scope.lead_ids, record_id);
            let c: Option<Option<Uuid>> =
                sqlx::query_scalar("SELECT contact_id FROM leads WHERE id = $1")
                    .bind(record_id)
                    .fetch_optional(&mut *db)
                    .await?;
            if let Some(Some(c)) = c {
                add(&mut scope.contact_ids, c);
            }
        }
        "deal" => {
            add(&mut scope.deal_ids, record_id);
            let c: Option<Option<Uuid>> =
                sqlx::query_scalar("SELECT contact_id FROM deals WHERE id = $1")
                    .bind(record_id)
                    .fetch_optional(&mut *db)
                    .await?;
            if let Some(Some(c)) = c {
                add(&mut scope.contact_ids, c);
            }
        }
        _ => {}
    }
    Ok(scope)
}

/// `(text or "")[:n]` in code points.
fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[derive(sqlx::FromRow)]
struct ActRow {
    id: Uuid,
    activity_type: String,
    subject: Option<String>,
    content: Option<String>,
    direction: Option<String>,
    outcome: Option<String>,
    completed_at: Option<DateTime<Utc>>,
    scheduled_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

async fn activities(
    db: &mut PgConnection,
    scope: &Scope,
    record_id: Uuid,
    limit: i64,
) -> Result<Vec<Value>, sqlx::Error> {
    let mut clauses = vec!["entity_id = $1".to_string()];
    if !scope.contact_ids.is_empty() {
        clauses.push("contact_id = ANY($2)".into());
    }
    if !scope.lead_ids.is_empty() {
        clauses.push("lead_id = ANY($3)".into());
    }
    if !scope.deal_ids.is_empty() {
        clauses.push("deal_id = ANY($4)".into());
    }
    let sql = format!(
        "SELECT id, activity_type, subject, content, direction, outcome, completed_at, scheduled_at, created_at \
         FROM activities WHERE {} ORDER BY created_at DESC LIMIT $5",
        clauses.join(" OR ")
    );
    let rows: Vec<ActRow> = sqlx::query_as(&sql)
        .bind(record_id)
        .bind(&scope.contact_ids)
        .bind(&scope.lead_ids)
        .bind(&scope.deal_ids)
        .bind(limit)
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|a| {
            let title = match a.subject.as_deref() {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => pystr::title(&a.activity_type.replace('_', " ")),
            };
            let preview = take_chars(a.content.as_deref().unwrap_or(""), 280);
            let at = a.completed_at.or(a.scheduled_at).unwrap_or(a.created_at);
            json!({
                "kind": "activity",
                "sub_kind": a.activity_type,
                "id": a.id.to_string(),
                "title": title,
                "preview": if preview.is_empty() { Value::Null } else { Value::String(preview) },
                "direction": a.direction,
                "outcome": a.outcome,
                "at": iso_dt(&at),
                "href": Value::Null,
            })
        })
        .collect())
}

#[derive(sqlx::FromRow)]
struct ThreadRow {
    id: Uuid,
    status: Option<String>,
    subject: Option<String>,
    message_count: Option<i32>,
    last_sender_name: Option<String>,
    last_sender_email: Option<String>,
    last_message_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    /// R&D 26 Oct: the newest message's direction (inbound: the customer wrote last)
    last_direction: Option<String>,
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|x| !x.is_empty())
}

async fn email_threads(
    db: &mut PgConnection,
    scope: &Scope,
    limit: i64,
) -> Result<Vec<Value>, sqlx::Error> {
    let mut clauses = Vec::new();
    if !scope.org_ids.is_empty() {
        clauses.push("linked_org_id = ANY($1)");
    }
    if !scope.contact_ids.is_empty() {
        clauses.push("linked_contact_id = ANY($2)");
    }
    if !scope.lead_ids.is_empty() {
        clauses.push("linked_lead_id = ANY($3)");
    }
    if !scope.deal_ids.is_empty() {
        clauses.push("linked_deal_id = ANY($4)");
    }
    if clauses.is_empty() {
        return Ok(vec![]);
    }
    let sql = format!(
        "SELECT t.id, t.status, t.subject, t.message_count, \
         coalesce(n.from_name, t.last_sender_name) AS last_sender_name, \
         coalesce(n.from_email, t.last_sender_email) AS last_sender_email, t.last_message_at, t.created_at, \
         n.direction AS last_direction \
         FROM crm_email_threads t LEFT JOIN LATERAL (SELECT m.from_name, m.from_email, m.direction \
           FROM crm_email_messages m WHERE m.thread_id = t.id ORDER BY m.received_at DESC NULLS LAST LIMIT 1) n ON true \
         WHERE {} ORDER BY t.last_message_at DESC NULLS LAST LIMIT $5",
        clauses.join(" OR ")
    );
    let rows: Vec<ThreadRow> = sqlx::query_as(&sql)
        .bind(&scope.org_ids)
        .bind(&scope.contact_ids)
        .bind(&scope.lead_ids)
        .bind(&scope.deal_ids)
        .bind(limit)
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|t| {
            let who = nonempty(&t.last_sender_name).or(nonempty(&t.last_sender_email)).unwrap_or("-");
            // R&D 26 Oct: the newest message's sender, ours included, and who is waiting on whom
            let ours = t.last_direction.as_deref() == Some("outbound");
            let from = if ours {
                format!("us ({})", nonempty(&t.last_sender_email).unwrap_or(who))
            } else {
                who.to_string()
            };
            let waiting = match t.last_direction.as_deref() {
                Some("outbound") => " · waiting on the customer",
                Some("inbound") => " · waiting on us",
                _ => "",
            };
            let mut item = json!({
                "kind": "email",
                "sub_kind": t.status,
                "id": t.id.to_string(),
                "title": nonempty(&t.subject).unwrap_or("(No Subject)"),
                "preview": format!("{} message(s) · last from {from}{waiting}", t.message_count.unwrap_or(0)),
                "direction": Value::Null,
                "outcome": Value::Null,
                "at": iso_dt(&t.last_message_at.unwrap_or(t.created_at)),
                "href": format!("/crm/email?thread={}", t.id),
            });
            // R&D 26 Oct: who is waiting, when the thread has messages
            if t.last_direction.is_some() {
                item["waiting_on"] = json!(if ours { "customer" } else { "us" });
            }
            item
        })
        .collect())
}

#[derive(sqlx::FromRow)]
struct DocRow {
    id: Uuid,
    status: Option<String>,
    full_number: Option<String>,
    currency: Option<String>,
    total: Option<Decimal>,
    issue_date: Option<NaiveDate>,
    created_at: DateTime<Utc>,
}

/// `f"{currency or ''} {float(total or 0):,.0f}".strip()`.
fn money_preview(currency: &Option<String>, total: Option<Decimal>) -> String {
    let f = match total {
        Some(t) if !t.is_zero() => t.to_string().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    };
    let amount = format_number(&PyNum::Float(f), ",.0f").unwrap_or_default();
    let s = format!("{} {amount}", currency.as_deref().unwrap_or(""));
    pystr::strip(&s).to_string()
}

async fn documents(
    db: &mut PgConnection,
    scope: &Scope,
    limit: i64,
    table: &str,
    kind: &str,
    default_title: &str,
    href_prefix: &str,
) -> Result<Vec<Value>, sqlx::Error> {
    let mut clauses = Vec::new();
    if !scope.org_ids.is_empty() {
        clauses.push("customer_org_id = ANY($1)");
    }
    if !scope.contact_ids.is_empty() {
        clauses.push("customer_contact_id = ANY($2)");
    }
    if !scope.deal_ids.is_empty() {
        clauses.push("deal_id = ANY($3)");
    }
    if clauses.is_empty() {
        return Ok(vec![]);
    }
    let sql = format!(
        "SELECT id, status, full_number, currency, total, issue_date, created_at FROM {table} \
         WHERE {table}.is_deleted = false AND ({}) ORDER BY created_at DESC LIMIT $4",
        clauses.join(" OR ")
    );
    let rows: Vec<DocRow> = sqlx::query_as(&sql)
        .bind(&scope.org_ids)
        .bind(&scope.contact_ids)
        .bind(&scope.deal_ids)
        .bind(limit)
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|q| {
            let at = match q.issue_date {
                Some(d) => date_isoformat(d),
                None => iso_dt(&q.created_at),
            };
            json!({
                "kind": kind,
                "sub_kind": q.status,
                "id": q.id.to_string(),
                "title": nonempty(&q.full_number).unwrap_or(default_title),
                "preview": money_preview(&q.currency, q.total),
                "direction": Value::Null,
                "outcome": q.status,
                "at": at,
                "href": format!("{href_prefix}{}", q.id),
            })
        })
        .collect())
}

/// `build_timeline(db, user, object_type, record_id, *, limit=60)`: merged, newest-first timeline for one
/// record. Each source is capped at `limit` before merging so one chatty channel can't crowd the others out.
/// An unsupported object_type is Python's ValueError (a 500; the router checks it first).
pub async fn build_timeline(
    db: &mut PgConnection,
    _user: &AuthUser,
    object_type: &str,
    record_id: Uuid,
    limit: i64,
) -> ApiResult<Vec<Value>> {
    if !OBJECT_TYPES.contains(&object_type) {
        return Err(ApiError::internal(anyhow::anyhow!(
            "unsupported object_type {object_type}"
        )));
    }
    let scope = resolve_scope(db, object_type, record_id).await?;
    let mut entries = entries_for_scope(db, &scope, record_id, limit).await?;

    // `entries.sort(key=_sort_key, reverse=True)`: stable, equal keys keep their order; no timestamp sorts
    // last. Python compares str by code point, which for these ASCII stamps is byte order.
    let key = |e: &Value| {
        e.get("at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    entries.sort_by_key(|e| std::cmp::Reverse(key(e)));
    // `entries[:limit]` (a negative limit counts from the end, as a Python slice does).
    let n = entries.len() as i64;
    let end = if limit < 0 {
        (n + limit).max(0)
    } else {
        limit.min(n)
    };
    entries.truncate(end as usize);
    Ok(entries)
}

/// The timeline sources of a scope (activities, email threads, quotations, invoices), each capped at `limit`,
/// not yet sorted (batch 2 T3: shared with Nary's get_activity_history, which adds a deal's source lead).
pub async fn entries_for_scope(
    db: &mut PgConnection,
    scope: &Scope,
    record_id: Uuid,
    limit: i64,
) -> ApiResult<Vec<Value>> {
    let mut entries: Vec<Value> = Vec::new();
    entries.extend(activities(db, scope, record_id, limit).await?);
    entries.extend(email_threads(db, scope, limit).await?);
    entries.extend(
        documents(
            db,
            scope,
            limit,
            "quotations",
            "quotation",
            "Quotation",
            "/sales/quotations/",
        )
        .await?,
    );
    entries.extend(
        documents(
            db,
            scope,
            limit,
            "invoices",
            "invoice",
            "Invoice",
            "/finance/invoices/",
        )
        .await?,
    );
    Ok(entries)
}

/// `related_counts(db, object_type, record_id)`: small related-record tallies for the record page sidebar.
pub async fn related_counts(
    db: &mut PgConnection,
    object_type: &str,
    record_id: Uuid,
) -> ApiResult<Value> {
    let scope = resolve_scope(db, object_type, record_id).await?;
    Ok(json!({
        "contacts": scope.contact_ids.len(),
        "leads": scope.lead_ids.len(),
        "deals": scope.deal_ids.len(),
    }))
}
