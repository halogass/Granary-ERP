//! backend/app/routers/crm_research.py, mounted at `/api/v1/crm/research` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//!   POST /{object_type}/{id}             start deep research (organization | contact), queued
//!   GET  /{object_type}/{id}             the latest run, and for a company the people it found
//!   POST /people/{person_id}/add         turn a found person into a contact
//!   POST /people/{person_id}/dismiss     not relevant: hide it; a re-run won't bring it back

use std::time::Duration;

use axum::extract::Path;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::v;
use granary_core::validation::PyJson;
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::crm::Organization;
use granary_db::rows::crm_research::CRMResearchPerson;
use granary_services::crm::bulk_ai::_may_view;
use granary_services::research::runs::{self, Record, RunsError};

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/crm/research";

/// The routes of `router` in crm_research.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/{object_type}/{record_id}"),
            post(start_research).get(research_state),
        )
        .route(&at(PREFIX, "/people/{person_id}/add"), post(add_person))
        .route(
            &at(PREFIX, "/people/{person_id}/dismiss"),
            post(dismiss_person),
        )
}

/// `_require(user, *codes)`.
fn require(user: &AuthUser, codes: &[&str]) -> ApiResult<()> {
    if user.is_superadmin || codes.iter().any(|c| user.perms.contains(*c)) {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        format!("Permission required: {}", codes.join(" or ")),
    ))
}

fn assigned(rec: &Record) -> Option<Uuid> {
    match rec {
        Record::Organization(o) => o.assigned_to_id,
        Record::Contact(c) => c.assigned_to_id,
    }
}

/// `_record(db, user, object_type, record_id)`.
async fn record(
    conn: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    record_id: &str,
) -> ApiResult<Record> {
    let id = crate::extract::parse_uuid(record_id, "record_id")?;
    let rec = match runs::load_record(conn, object_type, id).await {
        Ok(rec) => rec,
        Err(RunsError::Research(e)) => {
            let status = if e.0.contains("exist") { 404 } else { 400 };
            return Err(ApiError::http(status, e.0));
        }
        Err(RunsError::Db(e)) => return Err(e.into()),
    };
    if !_may_view(user, assigned(&rec)) {
        return Err(ApiError::http(
            403,
            "This record is assigned to someone else",
        ));
    }
    Ok(rec)
}

/// `queue(run_id, user_id)`: hand the run to a worker; false when no worker answers (the caller runs it
/// inline). Never raises.
pub async fn queue(run_id: Uuid, user_id: Uuid) -> bool {
    if !granary_services::tasks::ping(Duration::from_millis(1500)).await {
        return false;
    }
    granary_services::tasks::delay(
        "research_task",
        vec![json!(run_id.to_string()), json!(user_id.to_string())],
        Map::new(),
    )
    .await
    .is_ok()
}

fn runs_err(e: RunsError) -> ApiError {
    match e {
        RunsError::Research(r) => ApiError::internal(anyhow::anyhow!(r.0)),
        RunsError::Db(d) => d.into(),
    }
}

/// `POST /{object_type}/{record_id}`: start deep research.
pub async fn start_research(
    InternalUser(user): InternalUser,
    db: Db,
    Path((object_type, record_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require(&user, &["crm:create_contact", "crm:edit_contact"])?;
    let mut conn = db.conn().await?;
    let rec = record(&mut conn, &user, &object_type, &record_id).await?;
    let (mut run, created) =
        runs::start(&mut conn, &object_type, &rec, Some(user.id), "deep").await?;
    drop(conn);
    db.commit().await?;
    if created && !queue(run.id, user.id).await {
        let mut conn = db.conn().await?;
        run = runs::execute(&mut conn, run.id, Some(user.id), true)
            .await
            .map_err(runs_err)?;
    }
    let queued = matches!(run.status.as_str(), "queued" | "running");
    Ok(resp::json(
        json!({"run": runs::serialize_run(Some(&run)), "queued": queued}),
    ))
}

/// `GET /{object_type}/{record_id}`: the latest run, and for a company the people it found.
pub async fn research_state(
    InternalUser(user): InternalUser,
    db: Db,
    Path((object_type, record_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require(&user, &["crm:view_all", "crm:view_own"])?;
    let mut conn = db.conn().await?;
    let rec = record(&mut conn, &user, &object_type, &record_id).await?;
    let latest = runs::latest_run(&mut conn, &object_type, rec.id()).await?;
    let mut people: Vec<Value> = vec![];
    if object_type == "organization" {
        people = runs::people(&mut conn, rec.id(), "pending")
            .await?
            .iter()
            .map(runs::serialize_person)
            .collect();
    }
    Ok(resp::json(
        json!({"run": runs::serialize_run(latest.as_ref()), "people": people}),
    ))
}

granary_core::py_model! {
    /// `class AddPersonBody(BaseModel)`.
    pub struct AddPersonBody {
        pub name: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(200))),
        pub title: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(150))),
    }
}

async fn get_person(conn: &mut PgConnection, id: Uuid) -> ApiResult<Option<CRMResearchPerson>> {
    Ok(
        sqlx::query_as("SELECT * FROM crm_research_people WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// `_person_org(db, user, person_id)`.
async fn person_org(
    conn: &mut PgConnection,
    user: &AuthUser,
    person_id: &str,
) -> ApiResult<CRMResearchPerson> {
    let id = crate::extract::parse_uuid(person_id, "person_id")?;
    let Some(person) = get_person(conn, id).await? else {
        return Err(ApiError::http(404, "That suggestion no longer exists."));
    };
    let org: Option<Organization> = sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
        .bind(person.organization_id)
        .fetch_optional(&mut *conn)
        .await?;
    match org {
        Some(o) if _may_view(user, o.assigned_to_id) => Ok(person),
        _ => Err(ApiError::http(
            403,
            "This company is assigned to someone else",
        )),
    }
}

fn conflict_on_research(e: RunsError) -> ApiError {
    match e {
        RunsError::Research(r) => ApiError::http(409, r.0),
        RunsError::Db(d) => d.into(),
    }
}

/// `POST /people/{person_id}/add` (201): turn a found person into a contact.
pub async fn add_person(
    InternalUser(user): InternalUser,
    db: Db,
    Path(person_id): Path<String>,
    PyJson(body): PyJson<AddPersonBody>,
) -> ApiResult<Response> {
    require(&user, &["crm:create_contact"])?;
    let mut conn = db.conn().await?;
    let person = person_org(&mut conn, &user, &person_id).await?;
    let contact = runs::add_person(
        &mut conn,
        person.id,
        user.id,
        body.name.as_deref(),
        body.title.as_deref(),
    )
    .await
    .map_err(conflict_on_research)?;
    let now = get_person(&mut conn, person.id)
        .await?
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("person vanished")))?;
    Ok(resp::json_status(
        201,
        json!({"contact_id": contact.id.to_string(), "person": runs::serialize_person(&now)}),
    ))
}

/// `POST /people/{person_id}/dismiss`.
pub async fn dismiss_person(
    InternalUser(user): InternalUser,
    db: Db,
    Path(person_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, &["crm:create_contact", "crm:edit_contact"])?;
    let mut conn = db.conn().await?;
    let person = person_org(&mut conn, &user, &person_id).await?;
    let out = runs::dismiss_person(&mut conn, person.id, user.id)
        .await
        .map_err(conflict_on_research)?;
    Ok(resp::json(runs::serialize_person(&out)))
}
