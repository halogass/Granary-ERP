//! Port of backend/app/services/research/runs.py.
//!
//! Research runs: start one, carry it out (on a worker), read the latest, and act on the people
//! a company run found.
//!
//! A run is its own row so the record page can show "Researching..." while the worker is busy and
//! what came back afterwards, and so two clicks on the button don't start two runs.
//!
//! Python mutates ORM objects and lets the session flush them; here every change is an UPDATE
//! (with `updated_at = now()`, SQLAlchemy's `onupdate`). `db.commit()` / `db.rollback()` in the
//! middle of `execute` follow the google_calendar.rs pattern: inside a caller's transaction they
//! end it and open a new one; on an autocommit connection `execute` opens its own transaction
//! around the research step so that a failure can still be rolled back as in Python.

use chrono::{DateTime, Duration, Utc};
use granary_compat::pyjson::PyDateTime;
use granary_db::rows::crm::{Contact, Organization};
use granary_db::rows::crm_research::{CRMResearchPerson, CRMResearchRun};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::core_::ResearchFail;
use super::{company as company_research, contact as contact_research};

/// `PATHS`.
pub fn path_for(object_type: &str) -> &'static str {
    match object_type {
        "organization" => "/crm/organizations",
        _ => "/crm/contacts",
    }
}

/// A queued/running run younger than this is "still going": a second click returns it.
pub fn in_flight() -> Duration {
    Duration::minutes(15)
}

/// `ResearchError`: the request is wrong (unknown type, missing record, already added...), a 4xx.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct ResearchError(pub String);

/// What `runs` functions fail with: a `ResearchError` or a database error.
#[derive(Debug, thiserror::Error)]
pub enum RunsError {
    #[error("{0}")]
    Research(#[from] ResearchError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// The record `load_record` returns (Python: the ORM object of `MODELS[object_type]`).
#[derive(Debug, Clone)]
pub enum Record {
    Organization(Organization),
    Contact(Contact),
}

impl Record {
    pub fn id(&self) -> Uuid {
        match self {
            Record::Organization(o) => o.id,
            Record::Contact(c) => c.id,
        }
    }
}

fn _now() -> DateTime<Utc> {
    Utc::now()
}

/// `_label(object_type, rec)`.
pub fn _label(rec: &Record) -> String {
    match rec {
        Record::Organization(o) => o.name.clone(),
        Record::Contact(c) => [Some(c.first_name.as_str()), c.last_name.as_deref()]
            .into_iter()
            .flatten()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// `load_record(db, object_type, record_id)`.
pub async fn load_record(
    db: &mut PgConnection,
    object_type: &str,
    record_id: Uuid,
) -> Result<Record, RunsError> {
    match object_type {
        "organization" => {
            let rec: Option<Organization> =
                sqlx::query_as("SELECT * FROM organizations WHERE id = $1 AND is_deleted = false")
                    .bind(record_id)
                    .fetch_optional(&mut *db)
                    .await?;
            rec.map(Record::Organization)
        }
        "contact" => {
            let rec: Option<Contact> =
                sqlx::query_as("SELECT * FROM contacts WHERE id = $1 AND is_deleted = false")
                    .bind(record_id)
                    .fetch_optional(&mut *db)
                    .await?;
            rec.map(Record::Contact)
        }
        _ => {
            return Err(ResearchError("Research covers companies and contacts only.".into()).into())
        }
    }
    .ok_or_else(|| ResearchError(format!("That {object_type} no longer exists.")).into())
}

/// `latest_run(db, object_type, record_id)`.
pub async fn latest_run(
    db: &mut PgConnection,
    object_type: &str,
    record_id: Uuid,
) -> Result<Option<CRMResearchRun>, sqlx::Error> {
    sqlx::query_as(
        "SELECT * FROM crm_research_runs WHERE object_type = $1 AND record_id = $2 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(object_type)
    .bind(record_id)
    .fetch_optional(&mut *db)
    .await
}

/// `get_run(db, run_id)` (Python `db.get(CRMResearchRun, run_id)`).
pub async fn get_run(
    db: &mut PgConnection,
    run_id: Uuid,
) -> Result<Option<CRMResearchRun>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM crm_research_runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(&mut *db)
        .await
}

/// `start(db, object_type, record, user, depth="deep")`: a new queued run, or the one already in
/// flight. Returns (run, created).
pub async fn start(
    db: &mut PgConnection,
    object_type: &str,
    record: &Record,
    user_id: Option<Uuid>,
    depth: &str,
) -> Result<(CRMResearchRun, bool), sqlx::Error> {
    if let Some(current) = latest_run(db, object_type, record.id()).await? {
        if matches!(current.status.as_str(), "queued" | "running")
            && _now() - current.created_at < in_flight()
        {
            return Ok((current, false));
        }
    }
    let run: CRMResearchRun = sqlx::query_as(
        "INSERT INTO crm_research_runs (id, object_type, record_id, depth, status, requested_by_id) \
         VALUES ($1, $2, $3, $4, 'queued', $5) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(object_type)
    .bind(record.id())
    .bind(depth)
    .bind(user_id)
    .fetch_one(&mut *db)
    .await?;
    Ok((run, true))
}

// ─── transactions in the middle of a service function ─────────────────────────

/// Whether the connection is inside a transaction block (SAVEPOINT only succeeds there).
async fn in_transaction(db: &mut PgConnection) -> Result<bool, sqlx::Error> {
    if sqlx::query("SAVEPOINT granary_commit_probe")
        .execute(&mut *db)
        .await
        .is_ok()
    {
        sqlx::query("RELEASE SAVEPOINT granary_commit_probe")
            .execute(&mut *db)
            .await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// `await db.commit()` on the caller's connection.
async fn commit(db: &mut PgConnection) -> Result<(), sqlx::Error> {
    if in_transaction(db).await? {
        sqlx::query("COMMIT").execute(&mut *db).await?;
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    Ok(())
}

/// `execute(db, run_id, user=None, *, notify=True)`: carry out a queued run. Never fails for a
/// research failure: the run records it. `notify=false` for bulk runs, which send one summary
/// instead of one message per record.
pub async fn execute(
    db: &mut PgConnection,
    run_id: Uuid,
    user_id: Option<Uuid>,
    notify: bool,
) -> Result<CRMResearchRun, RunsError> {
    if get_run(db, run_id).await?.is_none() {
        return Err(ResearchError("That research run no longer exists.".into()).into());
    }
    let outer_tx = in_transaction(db).await?;
    sqlx::query(
        "UPDATE crm_research_runs SET status = 'running', started_at = $2, error = NULL, updated_at = now() \
         WHERE id = $1",
    )
    .bind(run_id)
    .bind(_now())
    .execute(&mut *db)
    .await?;
    commit(db).await?;
    if !outer_tx {
        // Python's session keeps everything below pending until the commit, so a failure can be
        // rolled back: on an autocommit connection that needs an explicit transaction.
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }

    let mut outcome = json!({});
    let res = carry_out(db, run_id, user_id).await;
    match res {
        Ok(o) => {
            outcome = o;
            sqlx::query(
                "UPDATE crm_research_runs SET status = 'done', updated_at = now() WHERE id = $1",
            )
            .bind(run_id)
            .execute(&mut *db)
            .await?;
        }
        Err(fail) => {
            let error = match &fail {
                ResearchFail::Unavailable(e) => prefix(&e.0, 1000).to_string(),
                ResearchFail::Request(e) => prefix(e, 1000).to_string(),
                ResearchFail::Other(e) => {
                    // a crash must still end the run, not leave it "running"
                    tracing::error!(run = %run_id, error = %e, "research_run_crashed");
                    format!("Research failed unexpectedly: {}", prefix(e, 300))
                }
            };
            // db.rollback(); the run's own changes then go into a new transaction (ours or the caller's).
            sqlx::query("ROLLBACK").execute(&mut *db).await?;
            sqlx::query("BEGIN").execute(&mut *db).await?;
            sqlx::query("UPDATE crm_research_runs SET status = 'failed', error = $2, updated_at = now() WHERE id = $1")
                .bind(run_id)
                .bind(error)
                .execute(&mut *db)
                .await?;
        }
    }
    sqlx::query("UPDATE crm_research_runs SET finished_at = $2, updated_at = now() WHERE id = $1")
        .bind(run_id)
        .bind(_now())
        .execute(&mut *db)
        .await?;
    if outer_tx {
        commit(db).await?;
    } else {
        sqlx::query("COMMIT").execute(&mut *db).await?;
    }

    let run = get_run(db, run_id)
        .await?
        .ok_or_else(|| ResearchError("That research run no longer exists.".into()))?;
    if notify && run.requested_by_id.is_some() {
        _notify(db, &run, &outcome).await?;
    }
    Ok(run)
}

/// The `try:` body of `execute`: load the record and research it.
async fn carry_out(
    db: &mut PgConnection,
    run_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<Value, ResearchFail> {
    let run = get_run(db, run_id)
        .await?
        .ok_or_else(|| ResearchFail::Other("run vanished".into()))?;
    let rec = match load_record(db, &run.object_type, run.record_id).await {
        Ok(r) => r,
        Err(RunsError::Research(e)) => return Err(ResearchFail::Request(e.0)),
        Err(RunsError::Db(e)) => return Err(e.into()),
    };
    match &rec {
        Record::Organization(org) => company_research::deep(db, org, &run, user_id).await,
        Record::Contact(c) => contact_research::research(db, c, &run, user_id).await,
    }
}

fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Python `str.capitalize()`: first character upper-cased, the rest lower-cased.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => {
            let first: String = f.to_uppercase().collect();
            // Python uses the title case of the first character; for the ASCII letters these
            // texts start with ("filled", "no") it is the same as upper case.
            first + &granary_compat::text::pystr::lower(chars.as_str())
        }
        None => String::new(),
    }
}

/// `_notify(db, run, outcome)`.
async fn _notify(
    db: &mut PgConnection,
    run: &CRMResearchRun,
    outcome: &Value,
) -> Result<(), RunsError> {
    let label = match load_record(db, &run.object_type, run.record_id).await {
        Ok(rec) => _label(&rec),
        Err(RunsError::Research(_)) => run.object_type.clone(),
        Err(e) => return Err(e),
    };
    let (title, body) = if run.status == "failed" {
        (
            format!("Research on {label} could not run"),
            run.error.clone(),
        )
    } else {
        let filled = match &run.written {
            Some(Value::Object(m)) => m.keys().cloned().collect::<Vec<_>>().join(", "),
            _ => String::new(),
        };
        let people = outcome.get("people_found").cloned().unwrap_or(Value::Null);
        let mut bits = vec![if filled.is_empty() {
            "no empty fields filled".to_string()
        } else {
            format!("filled {filled}")
        }];
        if super::fetch::py_truthy(&people) {
            bits.push(format!(
                "{} people found: review them on the company",
                granary_compat::pyjson::str_value(&(&people).into())
            ));
        }
        (
            format!("Research on {label} done"),
            Some(capitalize(&bits.join("; ")) + "."),
        )
    };
    sqlx::query(
        "INSERT INTO notifications (id, user_id, type, title, body, link, entity_type, entity_id, is_read, priority) \
         VALUES ($1, $2, 'crm_research', $3, $4, $5, $6, $7, false, 'normal')",
    )
    .bind(Uuid::new_v4())
    .bind(run.requested_by_id)
    .bind(prefix(&title, 255))
    .bind(body)
    .bind(format!("{}/{}", path_for(&run.object_type), run.record_id))
    .bind(&run.object_type)
    .bind(run.record_id.to_string())
    .execute(&mut *db)
    .await?;
    commit(db).await?;
    Ok(())
}

// ─── Reading ──────────────────────────────────────────────────────────────────

fn iso(dt: &DateTime<Utc>) -> String {
    PyDateTime::from_chrono_utc(dt).isoformat()
}

/// `serialize_run(run)`.
pub fn serialize_run(run: Option<&CRMResearchRun>) -> Value {
    let Some(run) = run else { return Value::Null };
    let or = |v: &Option<Value>, empty: Value| match v {
        Some(x) if super::fetch::py_truthy(x) => x.clone(),
        _ => empty,
    };
    json!({
        "id": run.id.to_string(), "object_type": run.object_type, "record_id": run.record_id.to_string(),
        "depth": run.depth, "status": run.status, "summary": run.summary,
        "sources": or(&run.sources, json!([])), "news": or(&run.news, json!([])),
        "channels": or(&run.channels, json!({})),
        "written": or(&run.written, json!({})), "suggested": or(&run.suggested, json!({})), "error": run.error,
        "created_at": iso(&run.created_at),
        "finished_at": run.finished_at.as_ref().map(iso),
    })
}

/// `serialize_person(p)`.
pub fn serialize_person(p: &CRMResearchPerson) -> Value {
    json!({
        "id": p.id.to_string(), "name": p.name, "title": p.title, "source_url": p.source_url,
        "evidence": p.evidence, "status": p.status,
        "contact_id": p.contact_id.map(|u| u.to_string()),
        "matched_contact_id": p.matched_contact_id.map(|u| u.to_string()),
    })
}

/// `people(db, organization_id, status="pending")`.
pub async fn people(
    db: &mut PgConnection,
    organization_id: Uuid,
    status: &str,
) -> Result<Vec<CRMResearchPerson>, sqlx::Error> {
    if status != "all" {
        sqlx::query_as(
            "SELECT * FROM crm_research_people WHERE organization_id = $1 AND status = $2 ORDER BY created_at",
        )
        .bind(organization_id)
        .bind(status)
        .fetch_all(&mut *db)
        .await
    } else {
        sqlx::query_as(
            "SELECT * FROM crm_research_people WHERE organization_id = $1 ORDER BY created_at",
        )
        .bind(organization_id)
        .fetch_all(&mut *db)
        .await
    }
}

// ─── Acting on people ─────────────────────────────────────────────────────────

async fn get_person(
    db: &mut PgConnection,
    person_id: Uuid,
) -> Result<Option<CRMResearchPerson>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM crm_research_people WHERE id = $1")
        .bind(person_id)
        .fetch_optional(&mut *db)
        .await
}

/// `_pending(db, person_id)`.
async fn _pending(db: &mut PgConnection, person_id: Uuid) -> Result<CRMResearchPerson, RunsError> {
    let person = get_person(db, person_id)
        .await?
        .ok_or_else(|| ResearchError("That suggestion no longer exists.".into()))?;
    if person.status != "pending" {
        return Err(ResearchError(format!("That person was already {}.", person.status)).into());
    }
    Ok(person)
}

/// `add_person(db, person_id, user, *, name=None, title=None)`: the only way research creates a
/// contact: someone confirms this person.
pub async fn add_person(
    db: &mut PgConnection,
    person_id: Uuid,
    user_id: Uuid,
    name: Option<&str>,
    title: Option<&str>,
) -> Result<Contact, RunsError> {
    let person = _pending(db, person_id).await?;
    let org: Option<Organization> = sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
        .bind(person.organization_id)
        .fetch_optional(&mut *db)
        .await?;
    let org = match org {
        Some(o) if !o.is_deleted => o,
        _ => return Err(ResearchError("The company no longer exists.".into()).into()),
    };
    // `name or person.name`: an empty name falls back to the found one.
    let raw = name.filter(|n| !n.is_empty()).unwrap_or(&person.name);
    let full = granary_compat::text::pystr::squash(raw);
    if full.is_empty() {
        return Err(ResearchError("A contact needs a name.".into()).into());
    }
    let (first, last) = match full.split_once(' ') {
        Some((f, l)) => (f.to_string(), l.to_string()),
        None => (full.clone(), String::new()),
    };
    let mut note = "Found by web research".to_string();
    if let Some(src) = person.source_url.as_deref().filter(|s| !s.is_empty()) {
        note += &format!(": {src}");
    }
    // `(title if title is not None else person.title) or ""`
    let position = match title {
        Some(t) => t.to_string(),
        None => person.title.clone().unwrap_or_default(),
    };
    let position = prefix(granary_compat::text::pystr::strip(&position), 150).to_string();
    let last = prefix(&last, 100).to_string();
    let contact: Contact = sqlx::query_as(
        "INSERT INTO contacts (id, organization_id, first_name, last_name, position, notes, assigned_to_id, \
         created_by_id, is_primary, tags, custom, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false, '{}'::varchar[], '{}'::jsonb, false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(org.id)
    .bind(prefix(&first, 100))
    .bind(if last.is_empty() { None } else { Some(last) })
    .bind(if position.is_empty() { None } else { Some(position) })
    .bind(note)
    .bind(org.assigned_to_id)
    .bind(user_id)
    .fetch_one(&mut *db)
    .await?;
    sqlx::query(
        "UPDATE crm_research_people SET status = 'added', contact_id = $2, resolved_by_id = $3, \
         resolved_at = $4, updated_at = now() WHERE id = $1",
    )
    .bind(person.id)
    .bind(contact.id)
    .bind(user_id)
    .bind(_now())
    .execute(&mut *db)
    .await?;
    commit(db).await?;
    Ok(contact)
}

/// `dismiss_person(db, person_id, user)`.
pub async fn dismiss_person(
    db: &mut PgConnection,
    person_id: Uuid,
    user_id: Uuid,
) -> Result<CRMResearchPerson, RunsError> {
    let person = _pending(db, person_id).await?;
    let person: CRMResearchPerson = sqlx::query_as(
        "UPDATE crm_research_people SET status = 'dismissed', resolved_by_id = $2, resolved_at = $3, \
         updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(person.id)
    .bind(user_id)
    .bind(_now())
    .fetch_one(&mut *db)
    .await?;
    commit(db).await?;
    Ok(person)
}
