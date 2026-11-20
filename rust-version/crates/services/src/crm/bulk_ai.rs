//! Port of backend/app/services/crm/bulk_ai.py.
//!
//! Run AI over a selection of records: one AI attribute, or a web-research pass over companies.
//!
//! The work itself is slow (one model call per record), so the endpoint only validates and queues:
//! this module is what the Celery task runs, and it is where the rules live that keep a bulk run
//! safe: the caller's own/all visibility is re-checked per record, and the pass stops the moment
//! the monthly budget is spent rather than producing one identical failure per row.
//!
//! When it finishes, the person who asked gets an in-app notification: they are rarely still
//! watching the list by then.
//!
//! Test seam: Python tests replace `bulk_ai.enrich_organization`; here `seam::with_enrich(f, fut)` runs
//! `fut` with `f` in its place (task-local).

use std::sync::Arc;

use granary_compat::text::utils::parse_uuid;
use granary_core::auth::AuthUser;
use granary_db::rows::crm::{Contact, Organization};
use granary_db::rows::crm_attributes::CRMAttributeDef;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::ai::attributes::{run_attribute, RunError};
use crate::ai::enrichment::{enrich_organization, EnrichError, EnrichmentUnavailable};

pub const MODELS: &[&str] = &["organization", "contact", "lead", "deal"];
pub const MAX_BULK: usize = 50;

pub fn list_path(object_type: &str) -> Option<&'static str> {
    match object_type {
        "organization" => Some("/crm/organizations"),
        "contact" => Some("/crm/contacts"),
        "lead" => Some("/crm/leads"),
        "deal" => Some("/crm/deals"),
        _ => None,
    }
}

fn table(object_type: &str) -> &'static str {
    match object_type {
        "organization" => "organizations",
        "contact" => "contacts",
        "lead" => "leads",
        _ => "deals",
    }
}

/// The request itself is wrong: surfaced as a 4xx before anything is queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkError(pub String);

impl std::fmt::Display for BulkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BulkError {}

/// What `run_bulk` / `load_attribute` raise: the BulkError, or a database failure.
#[derive(Debug)]
pub enum BulkFail {
    Bulk(BulkError),
    Db(sqlx::Error),
}

impl std::fmt::Display for BulkFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BulkFail::Bulk(e) => write!(f, "{e}"),
            BulkFail::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BulkFail {}

impl From<sqlx::Error> for BulkFail {
    fn from(e: sqlx::Error) -> Self {
        BulkFail::Db(e)
    }
}

fn bulk(s: impl Into<String>) -> BulkFail {
    BulkFail::Bulk(BulkError(s.into()))
}

pub mod seam {
    use super::*;

    /// A stand-in for `enrich_organization(db, org, user=..., fields=None)`.
    pub type EnrichFn =
        Arc<dyn Fn(&mut Organization, Option<Uuid>) -> Result<Value, EnrichError> + Send + Sync>;

    tokio::task_local! {
        pub(super) static ENRICH: EnrichFn;
    }

    /// Run `fut` with `f` replacing `enrich_organization` for bulk runs (task-local).
    pub async fn with_enrich<F: std::future::Future>(f: EnrichFn, fut: F) -> F::Output {
        ENRICH.scope(f, fut).await
    }
}

async fn enrich(
    db: &mut PgConnection,
    org: &mut Organization,
    user_id: Option<Uuid>,
) -> Result<Value, EnrichError> {
    if let Ok(f) = seam::ENRICH.try_with(|f| f.clone()) {
        return f(org, user_id);
    }
    enrich_organization(db, org, user_id, None).await
}

/// Owner scoping, mirroring `_assert_can_view` in routers/crm_records.py.
pub fn _may_view(user: &AuthUser, assigned_to_id: Option<Uuid>) -> bool {
    if user.is_superadmin || user.perms.contains("crm:view_all") {
        return true;
    }
    if !user.perms.contains("crm:view_own") {
        return false;
    }
    match assigned_to_id {
        None => true,
        Some(owner) => owner == user.id,
    }
}

pub async fn load_attribute(
    db: &mut PgConnection,
    object_type: &str,
    attribute_key: Option<&str>,
) -> Result<CRMAttributeDef, BulkFail> {
    // `CRMAttributeDef.key == None` is `key IS NULL`: no row.
    let defn: Option<CRMAttributeDef> = match attribute_key {
        None => None,
        Some(k) => {
            sqlx::query_as(
                "SELECT * FROM crm_attribute_defs WHERE object_type = $1 AND key = $2 AND is_active = true \
                 LIMIT 1",
            )
            .bind(object_type)
            .bind(k)
            .fetch_optional(&mut *db)
            .await?
        }
    };
    let Some(defn) = defn else {
        return Err(bulk(format!(
            "No active AI attribute '{}' on {object_type}.",
            attribute_key.unwrap_or("None")
        )));
    };
    if !defn.is_ai {
        return Err(bulk(format!("'{}' is not an AI attribute.", defn.label)));
    }
    Ok(defn)
}

/// The record `_record` loads.
enum Rec {
    Organization(Organization),
    Contact(Contact),
    /// lead / deal: only the owner is read here.
    Other(Option<Uuid>),
}

impl Rec {
    fn assigned_to_id(&self) -> Option<Uuid> {
        match self {
            Rec::Organization(o) => o.assigned_to_id,
            Rec::Contact(c) => c.assigned_to_id,
            Rec::Other(a) => *a,
        }
    }
}

async fn _record(
    db: &mut PgConnection,
    object_type: &str,
    record_id: Uuid,
) -> Result<Option<Rec>, sqlx::Error> {
    let t = table(object_type);
    let sql = format!("SELECT * FROM {t} WHERE id = $1 AND is_deleted = false");
    Ok(match object_type {
        "organization" => sqlx::query_as::<_, Organization>(&sql)
            .bind(record_id)
            .fetch_optional(&mut *db)
            .await?
            .map(Rec::Organization),
        "contact" => sqlx::query_as::<_, Contact>(&sql)
            .bind(record_id)
            .fetch_optional(&mut *db)
            .await?
            .map(Rec::Contact),
        _ => sqlx::query_scalar::<_, Option<Uuid>>(&format!(
            "SELECT assigned_to_id FROM {t} WHERE id = $1 AND is_deleted = false"
        ))
        .bind(record_id)
        .fetch_optional(&mut *db)
        .await?
        .map(Rec::Other),
    })
}

/// `await db.commit()` on the caller's connection (SAVEPOINT probe, as google_calendar.rs).
async fn commit(db: &mut PgConnection) -> Result<(), sqlx::Error> {
    if sqlx::query("SAVEPOINT granary_commit_probe")
        .execute(&mut *db)
        .await
        .is_ok()
    {
        sqlx::query("RELEASE SAVEPOINT granary_commit_probe")
            .execute(&mut *db)
            .await?;
        sqlx::query("COMMIT").execute(&mut *db).await?;
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    Ok(())
}

/// One record's failure, as the except clauses of `run_bulk` see it.
enum Failure {
    /// `AIAttributeError` / `EnrichmentUnavailable`: may stop the run.
    Known(String),
    /// Any other exception: `str(e)[:200]`.
    Other(String),
}

fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

/// Run one AI attribute, or the web-research pass, over a selection. Never fails for a
/// single bad record; the summary says what was filled and what was not.
pub async fn run_bulk(
    db: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    record_ids: &[String],
    attribute_key: Option<&str>,
    action: &str,
) -> Result<Value, BulkFail> {
    if !MODELS.contains(&object_type) {
        return Err(bulk(format!(
            "object_type must be one of {}",
            MODELS.join(", ")
        )));
    }
    if record_ids.is_empty() {
        return Err(bulk("Select at least one record."));
    }
    if record_ids.len() > MAX_BULK {
        return Err(bulk(format!(
            "Too many records in one run (max {MAX_BULK})."
        )));
    }
    if action == "enrich" && object_type != "organization" {
        return Err(bulk("Web research only applies to companies."));
    }
    if action == "research" && !matches!(object_type, "organization" | "contact") {
        return Err(bulk("Deep research covers companies and contacts."));
    }

    let defn = if action == "attribute" {
        Some(load_attribute(db, object_type, attribute_key).await?)
    } else {
        None
    };

    let mut results: Vec<Value> = vec![];
    let mut failed: Vec<Value> = vec![];
    let mut stopped: Option<String> = None;
    for rid in record_ids {
        let outcome: Result<Option<Value>, Failure> = async {
            let record_id = parse_uuid(rid, "record_id")
                .map_err(|d| Failure::Other(short(&format!("400: {d}"))))?;
            let rec = _record(db, object_type, record_id)
                .await
                .map_err(|e| Failure::Other(short(&e.to_string())))?;
            let Some(rec) = rec else {
                failed.push(json!({"record_id": rid, "error": "No longer exists"}));
                return Ok(None);
            };
            if !_may_view(user, rec.assigned_to_id()) {
                failed.push(json!({"record_id": rid, "error": "Assigned to someone else"}));
                return Ok(None);
            }
            if let Some(defn) = &defn {
                return match run_attribute(db, user, defn, record_id).await {
                    Ok(v) => Ok(Some(v)),
                    Err(RunError::Attr(e)) => Err(Failure::Known(e.0)),
                    Err(e) => Err(Failure::Other(short(&e.py_str()))),
                };
            }
            if action == "research" {
                let record = match rec {
                    Rec::Organization(o) => crate::research::runs::Record::Organization(o),
                    Rec::Contact(c) => crate::research::runs::Record::Contact(c),
                    Rec::Other(_) => unreachable!("research covers companies and contacts"),
                };
                let (run, _) =
                    crate::research::runs::start(db, object_type, &record, Some(user.id), "deep")
                        .await
                        .map_err(|e| Failure::Other(short(&e.to_string())))?;
                commit(db)
                    .await
                    .map_err(|e| Failure::Other(short(&e.to_string())))?;
                let run = crate::research::runs::execute(db, run.id, Some(user.id), false)
                    .await
                    .map_err(|e| Failure::Other(short(&e.to_string())))?;
                if run.status == "failed" {
                    let msg = run
                        .error
                        .filter(|e| !e.is_empty())
                        .unwrap_or_else(|| "Research failed.".into());
                    return Err(Failure::Known(EnrichmentUnavailable(msg).0));
                }
                let written = match run.written {
                    Some(w) if crate::ai::drafting::truthy(Some(&w)) => w,
                    _ => json!({}),
                };
                return Ok(Some(json!({"record_id": rid, "written": written})));
            }
            let Rec::Organization(mut org) = rec else {
                unreachable!("enrich applies to companies only")
            };
            let out = match enrich(db, &mut org, Some(user.id)).await {
                Ok(v) => v,
                Err(EnrichError::Unavailable(e)) => return Err(Failure::Known(e.0)),
                Err(e) => return Err(Failure::Other(short(&e.to_string()))),
            };
            let mut row = Map::new();
            row.insert("record_id".into(), json!(rid));
            if let Value::Object(o) = out {
                row.extend(o);
            }
            commit(db)
                .await
                .map_err(|e| Failure::Other(short(&e.to_string())))?;
            Ok(Some(Value::Object(row)))
        }
        .await;
        match outcome {
            Ok(Some(v)) => results.push(v),
            Ok(None) => {}
            Err(Failure::Known(message)) => {
                failed.push(json!({"record_id": rid, "error": message}));
                // Budget exhaustion (or a missing key) applies to every remaining record: stop
                // rather than generating one identical failure per row.
                let low = granary_compat::text::pystr::lower(&message);
                if ["budget", "api key", "disabled", "search"]
                    .iter()
                    .any(|w| low.contains(w))
                {
                    stopped = Some(message);
                    break;
                }
            }
            Err(Failure::Other(message)) => {
                failed.push(json!({"record_id": rid, "error": message}));
            }
        }
    }

    let label = match &defn {
        Some(d) => format!("Filled {}", d.label),
        None if action == "research" => "Researched".into(),
        None => "Enriched from the web".into(),
    };
    Ok(
        json!({"action": action, "attribute_key": attribute_key, "label": label,
        "succeeded": results.len(), "failed": failed, "stopped_early": stopped,
        "results": results, "requested": record_ids.len()}),
    )
}

/// Tell the person who started the run how it went.
pub async fn notify_finished(
    db: &mut PgConnection,
    user_id: Uuid,
    object_type: &str,
    summary: &Value,
) -> Result<(), sqlx::Error> {
    let s =
        |v: &Value| granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(v));
    let total = s(&summary["requested"]);
    let succeeded = s(&summary["succeeded"]);
    let mut body = format!("{succeeded} of {total} {object_type}s done.");
    let stopped = &summary["stopped_early"];
    let failed_n = summary["failed"].as_array().map(|a| a.len()).unwrap_or(0);
    if crate::ai::drafting::truthy(Some(stopped)) {
        body += &format!(" Stopped early: {}", s(stopped));
    } else if failed_n > 0 {
        body += &format!(" {failed_n} could not be processed.");
    }
    let label = s(&summary["label"]);
    sqlx::query(
        "INSERT INTO notifications (id, user_id, type, title, body, link, is_read, priority) \
         VALUES ($1, $2, 'crm_bulk_ai', $3, $4, $5, false, 'normal')",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(format!("{label} for {succeeded} of {total}"))
    .bind(body)
    .bind(list_path(object_type).unwrap_or("/crm"))
    .execute(&mut *db)
    .await?;
    commit(db).await
}
