//! Port of backend/app/services/approvals/engine.py.
//!
//! Approval engine: submit -> route -> approve/reject, plus the gate other endpoints call.
//!
//! Invariants
//! - The chain is resolved at submit and frozen on the request (`steps`); policy edits never
//!   change a request in flight.
//! - Only one pending request per document. Rejection ends the request; the document goes back
//!   to an editable state and resubmitting starts the chain again from step 1.
//! - An approval is bound to the document content it was given for: `ensure_approved` refuses when
//!   the document's facts fingerprint (or revision) no longer matches what was approved.
//! - Self-approval is refused unless the actor is a superadmin (the Owner) or the step is an SOP-sanctioned
//!   self-approval step.
//!
//! Port notes
//! - The SQLAlchemy object is an `ApprovalRequest` row struct here; every change Python made on the object
//!   (flushed later) is written with an UPDATE at the point Python would have flushed it at the latest, so
//!   the rows in the database are the same when the caller commits. Inserted rows use the server defaults
//!   (`created_at = now()`, the transaction time) exactly like Python's flush.
//! - `db.commit()` inside `_auto_cancel` / `ensure_or_submit` is `COMMIT; BEGIN` on the caller's
//!   connection when it is inside a transaction (nothing in autocommit): the caller's transaction wrapper
//!   then commits or rolls back the fresh transaction, like SQLAlchemy's autobegin.
//! - The email/WhatsApp outbox is `notify`'s task-local scope (see notify.rs).

use chrono::{DateTime, Duration, Utc};
use granary_compat::pyjson::{dumps, DumpOpts, PyDateTime, PyValue};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_db::rows::approval::{
    ApprovalAction, ApprovalPolicy, ApprovalPolicyStep, ApprovalRequest,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use super::adapters::{get_adapter, DocumentAdapter, Entity};
use super::conditions::{evaluate, failing_preconditions, py, CondError};
use super::notify::{self, notify, Notice};
use super::routing::{resolve_candidates, RoutingError};

pub const FINAL_STATUSES: &[&str] = &["approved", "rejected", "cancelled", "superseded"];

/// `REMINDER_INTERVAL = timedelta(hours=24)`.
pub fn reminder_interval() -> Duration {
    Duration::hours(24)
}

/// `datetime.now(timezone.utc)` (microsecond precision, like Python).
fn now() -> DateTime<Utc> {
    let n = Utc::now();
    DateTime::from_timestamp_micros(n.timestamp_micros()).unwrap_or(n)
}

fn iso(dt: &DateTime<Utc>) -> String {
    PyDateTime::from_chrono_utc(dt).isoformat()
}

fn cond_err(e: CondError) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{}: {}", e.kind, e.msg))
}

/// `facts_fingerprint(facts, flags)`: sha256 of `json.dumps({"facts": facts, "flags": sorted(flags or [])},
/// sort_keys=True, default=str)`.
pub fn facts_fingerprint(facts: &Value, flags: &[String]) -> String {
    let mut sorted: Vec<&String> = flags.iter().collect();
    sorted.sort();
    let payload = PyValue::Dict(vec![
        ("facts".to_string(), PyValue::from(facts)),
        (
            "flags".to_string(),
            PyValue::List(
                sorted
                    .into_iter()
                    .map(|f| PyValue::Str(f.clone()))
                    .collect(),
            ),
        ),
    ]);
    let text =
        dumps(&payload, &DumpOpts::sorted_default_str()).expect("JSON-shaped facts always dump");
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The document's facts as the request approved them. An approval made from 17 Nov 2026 (`facts_bound`) covers
/// them all. An earlier one covers only the facts its snapshot has: a fact added to a document type later (a
/// quotation's notes) never makes it stale by itself; a change to any fact it did cover still does.
pub fn as_approved(facts: Map<String, Value>, snapshot: &Value) -> Value {
    if snapshot.get("facts_bound") == Some(&Value::Bool(true)) {
        return Value::Object(facts);
    }
    match snapshot.get("document").and_then(Value::as_object) {
        Some(approved) => Value::Object(
            facts
                .into_iter()
                .filter(|(k, _)| approved.contains_key(k))
                .collect(),
        ),
        None => Value::Object(facts),
    }
}

/// The request's `flags` column as the list of strings it holds.
fn flags_of(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items.iter().map(py::str_v).collect(),
        _ => vec![],
    }
}

/// An active policy with its steps (ordered by sequence).
#[derive(Debug, Clone)]
pub struct Policy {
    pub row: ApprovalPolicy,
    pub steps: Vec<ApprovalPolicyStep>,
}

fn step_dict(step: &ApprovalPolicyStep) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("seq".into(), json!(step.sequence));
    m.insert("name".into(), json!(step.name));
    m.insert("approver_mode".into(), json!(step.approver_mode));
    m.insert("role_slug".into(), json!(step.role_slug));
    m.insert("department".into(), json!(step.department));
    m.insert("snapshot_user_key".into(), json!(step.snapshot_user_key));
    m.insert("user_id".into(), json!(step.user_id.map(|u| u.to_string())));
    m.insert("sla_hours".into(), json!(step.sla_hours));
    m.insert("is_notify_only".into(), json!(step.is_notify_only));
    m.insert(
        "allow_self_approval".into(),
        json!(step.allow_self_approval),
    );
    m
}

fn steps_of(request: &ApprovalRequest) -> &[Value] {
    match &request.steps {
        Value::Array(a) => a,
        _ => &[],
    }
}

fn is_notify_only(s: &Value) -> bool {
    py::truthy(s.get("is_notify_only").unwrap_or(&Value::Null))
}

/// `gating_steps(request)`: the steps that gate (not notify-only).
pub fn gating_steps(request: &ApprovalRequest) -> Vec<&Value> {
    steps_of(request)
        .iter()
        .filter(|s| !is_notify_only(s))
        .collect()
}

fn seq_of(s: &Value) -> &Value {
    s.get("seq").unwrap_or(&Value::Null)
}

fn current_step_index(request: &ApprovalRequest) -> Option<usize> {
    let seq = request.current_step_seq?;
    let want = json!(seq);
    steps_of(request)
        .iter()
        .position(|s| !is_notify_only(s) && py::eq(seq_of(s), &want))
}

/// `current_step(request)`: the active gating step, if any.
pub fn current_step(request: &ApprovalRequest) -> Option<&Value> {
    current_step_index(request).map(|i| &steps_of(request)[i])
}

fn step_str(s: &Value, k: &str) -> String {
    py::str_v(s.get(k).unwrap_or(&Value::Null))
}

fn candidate_ids(s: &Value) -> Vec<String> {
    match s.get("candidate_user_ids") {
        Some(Value::Array(a)) => a.iter().map(py::str_v).collect(),
        _ => vec![],
    }
}

fn reasons_of(s: &Value) -> Vec<String> {
    match s.get("reasons") {
        Some(Value::Array(a)) => a.iter().map(py::str_v).collect(),
        _ => vec![],
    }
}

/// `get_policy(db, entity_type)`: the active policy for a document type.
pub async fn get_policy(
    db: &mut PgConnection,
    entity_type: &str,
) -> Result<Option<Policy>, ApiError> {
    let row: Option<ApprovalPolicy> = sqlx::query_as(
        "SELECT * FROM approval_policies WHERE entity_type = $1 AND is_active = true",
    )
    .bind(entity_type)
    .fetch_optional(&mut *db)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let steps: Vec<ApprovalPolicyStep> = sqlx::query_as(
        "SELECT * FROM approval_policy_steps WHERE policy_id = $1 ORDER BY sequence",
    )
    .bind(row.id)
    .fetch_all(&mut *db)
    .await?;
    Ok(Some(Policy { row, steps }))
}

/// `latest_request(db, entity_type, entity_id)`: the newest request that is not superseded.
pub async fn latest_request(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<Option<ApprovalRequest>, ApiError> {
    Ok(sqlx::query_as(
        "SELECT * FROM approval_requests WHERE entity_type = $1 AND entity_id = $2 AND status != 'superseded' \
         ORDER BY submitted_at DESC LIMIT 1",
    )
    .bind(entity_type)
    .bind(entity_id)
    .fetch_optional(&mut *db)
    .await?)
}

async fn load_for_update(
    db: &mut PgConnection,
    request_id: &str,
) -> Result<Option<ApprovalRequest>, ApiError> {
    let id = Uuid::parse_str(request_id).map_err(|e| {
        ApiError::internal(anyhow::anyhow!(
            "invalid input for query argument $1: {request_id:?} ({e})"
        ))
    })?;
    Ok(
        sqlx::query_as("SELECT * FROM approval_requests WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// Writes the request's mutable columns back (Python's flush of the dirty object; `onupdate` sets
/// updated_at to now()).
async fn save(db: &mut PgConnection, r: &mut ApprovalRequest) -> Result<(), ApiError> {
    let updated: DateTime<Utc> = sqlx::query_scalar(
        "UPDATE approval_requests SET status = $2, completed_at = $3, current_step_seq = $4, steps = $5, \
         snapshot = $6, flags = $7, last_reminded_at = $8, updated_at = now() WHERE id = $1 RETURNING updated_at",
    )
    .bind(r.id)
    .bind(&r.status)
    .bind(r.completed_at)
    .bind(r.current_step_seq)
    .bind(&r.steps)
    .bind(&r.snapshot)
    .bind(&r.flags)
    .bind(r.last_reminded_at)
    .fetch_one(&mut *db)
    .await?;
    r.updated_at = updated;
    Ok(())
}

/// `_action(db, request, action, *, actor, step_seq, comment, ip, user_agent)`.
#[allow(clippy::too_many_arguments)]
async fn action(
    db: &mut PgConnection,
    request: &ApprovalRequest,
    action: &str,
    actor: Option<&AuthUser>,
    step_seq: Option<i32>,
    comment: Option<&str>,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO approval_actions (id, request_id, step_seq, actor_id, action, comment, ip_address, user_agent) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(request.id)
    .bind(step_seq)
    .bind(actor.map(|a| a.id))
    .bind(action)
    .bind(comment)
    .bind(ip)
    .bind(user_agent)
    .execute(&mut *db)
    .await?;
    Ok(())
}

fn int_of(v: &Value) -> Option<i32> {
    match v {
        Value::Number(n) => match py::num(n) {
            py::Num::Int(i) => i32::try_from(i).ok(),
            py::Num::Float(_) => None,
        },
        _ => None,
    }
}

/// `_activate_step(request, step, policy_sla)` on `request.steps[index]`.
fn activate_step(request: &mut ApprovalRequest, index: usize, policy_sla: Option<i32>) {
    let Value::Array(steps) = &mut request.steps else {
        return;
    };
    let step = &mut steps[index];
    request.current_step_seq = int_of(seq_of(step));
    // hours = step.get("sla_hours") or policy_sla
    let sla = step.get("sla_hours").cloned().unwrap_or(Value::Null);
    let hours: Option<i64> = if py::truthy(&sla) {
        py::int(&sla).ok().map(|h| h as i64)
    } else {
        policy_sla.map(|h| h as i64)
    };
    let obj = step.as_object_mut().expect("a step is a dict");
    obj.insert("activated_at".into(), json!(iso(&now())));
    let due = match hours {
        Some(h) if h != 0 => json!(iso(&(now() + Duration::hours(h)))),
        _ => Value::Null,
    };
    obj.insert("due_at".into(), due);
}

/// `_should_whatsapp(policy_threshold, snapshot)`.
fn should_whatsapp(
    policy_threshold: Option<rust_decimal::Decimal>,
    snapshot: &Value,
) -> Result<bool, ApiError> {
    let Some(threshold) = policy_threshold else {
        return Ok(false);
    };
    let amount = snapshot.get("amount_base").unwrap_or(&Value::Null);
    if amount.is_null() {
        return Ok(true);
    }
    // float(Decimal) is the correctly rounded double of the decimal text.
    let t: f64 = threshold.to_string().parse().unwrap_or(f64::NAN);
    Ok(py::float(amount).map_err(cond_err)? > t)
}

fn request_link(request: &ApprovalRequest) -> String {
    format!("/approvals?request={}", request.id)
}

fn label(request: &ApprovalRequest) -> String {
    py::str_v(&json!(request.document_label))
}

/// `_notify_step(db, request, step, adapter, whatsapp)`.
async fn notify_step(
    db: &mut PgConnection,
    request: &ApprovalRequest,
    step: &Value,
    whatsapp: bool,
) -> Result<(), ApiError> {
    let reasons = reasons_of(step).join("; ");
    let reasons = if reasons.is_empty() {
        "Required by policy".to_string()
    } else {
        reasons
    };
    let title = format!("Approval needed: {}", label(request));
    let body = format!("{}: {}", step_str(step, "name"), reasons);
    let link = request_link(request);
    let entity_id = request.entity_id.to_string();
    notify(
        db,
        candidate_ids(step),
        Notice::new(&title, &body, &link, &request.entity_type, &entity_id)
            .priority("high")
            .email(true)
            .whatsapp(whatsapp),
    )
    .await
}

/// `submit(db, *, entity_type, entity_id, user, flags=None, ip=None, user_agent=None)`.
pub async fn submit(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: &str,
    user: &AuthUser,
    flags: Option<&[String]>,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> Result<ApprovalRequest, ApiError> {
    let adapter = get_adapter(entity_type)?;
    let Some(policy) = get_policy(db, entity_type).await? else {
        return Err(ApiError::http(
            400,
            format!(
                "No active approval policy for {}s",
                adapter.label.to_lowercase()
            ),
        ));
    };
    let Some(mut entity) = adapter.load(db, entity_id).await? else {
        return Err(ApiError::http(404, format!("{} not found", adapter.label)));
    };
    adapter.check_submittable(&entity)?;
    // 1 Nov 2026 (Owner decision): a Solution Proposal without its required figures (timeline, architecture and
    // the ones its requirement sheet calls for), or with one whose image is missing or whose check failed,
    // can't be submitted; the refusal lists them.
    if entity_type == "sales_document" {
        let missing = crate::nary::proposals::figures_refusal(db, entity.id()).await?;
        if !missing.is_empty() {
            return Err(ApiError::http_json(
                400,
                json!({"code": "required_figures", "messages": missing}),
            ));
        }
    }

    // 2 Nov 2026 (Owner decision): a quotation whose notes say prices exclude tax while a line carries tax (or
    // say prices include tax while no line does) can't be submitted; the refusal says why.
    if entity_type == "quotation" {
        if let Some(why) = crate::quotation_tax::quotation_refusal(db, entity.id()).await? {
            return Err(ApiError::http(400, why));
        }
        // 4 Nov 2026 (Owner decision): a draft whose notes don't follow the house template (missing required
        // clauses, not numbered, an accuracy figure, a delay penalty, a discount without the permission) can't be
        // submitted; the refusal lists what is wrong (sales.quotation.notes.block_submit)
        if let Some(why) = crate::quotation_notes::quotation_refusal(db, entity.id(), user).await? {
            return Err(ApiError::http(400, why));
        }
        // A draft with a line below the minimum multiple of its cost can't be submitted unless the Owner gave a
        // pricing override with a reason for these lines (sales.pricing_rules.block_submit)
        if let Some(why) = crate::pricing_rules::quotation_refusal_for(
            db,
            entity.id(),
            crate::ai::tools::catalog::can_see_costs(user),
        )
        .await?
        {
            return Err(ApiError::http(400, why));
        }
    }

    // Serialize submissions per document: two concurrent submits must not both see "no pending
    // request" (the partial unique index uq_approval_requests_pending_entity backs this up).
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("approval:{entity_type}:{}", entity.id()))
        .execute(&mut *db)
        .await?;
    let mut previous = latest_request(db, entity_type, entity.id()).await?;
    if previous.as_ref().is_some_and(|p| p.status == "pending") {
        return Err(ApiError::http(
            409,
            format!("{} already has a pending approval request", adapter.label),
        ));
    }

    // flags = sorted(set(flags or []))
    let mut flags: Vec<String> = flags.unwrap_or(&[]).to_vec();
    flags.sort();
    flags.dedup();
    let mut snapshot = adapter.snapshot(db, &entity).await?;
    snapshot.insert("flags".into(), json!(flags));
    // 17 Nov 2026: this approval covers every fact of the document as the ERP knows them now (a quotation's
    // notes included); approvals without the mark keep only the facts they had (as_approved)
    snapshot.insert("facts_bound".into(), json!(true));
    let fingerprint = facts_fingerprint(snapshot.get("document").unwrap_or(&Value::Null), &flags);
    snapshot.insert("fingerprint".into(), json!(fingerprint));
    let snapshot = Value::Object(snapshot);

    if let Some(p) = &previous {
        if p.status == "approved"
            && p.document_revision == adapter.revision(&entity)
            && p.snapshot.get("fingerprint") == Some(&json!(fingerprint))
        {
            return Err(ApiError::http(
                409,
                format!("{} is already approved in its current form", adapter.label),
            ));
        }
    }

    let blocked = failing_preconditions(&policy.row.preconditions, &snapshot).map_err(cond_err)?;
    if !blocked.is_empty() {
        return Err(ApiError::http_json(
            400,
            json!({"code": "precondition_failed", "messages": blocked}),
        ));
    }

    let mut resolved: Vec<Map<String, Value>> = vec![];
    for step in &policy.steps {
        let (applies, reasons) =
            evaluate(step.condition.as_ref().unwrap_or(&Value::Null), &snapshot)
                .map_err(cond_err)?;
        if !applies {
            continue;
        }
        let mut frozen = step_dict(step);
        let frozen_v = Value::Object(frozen.clone());
        match resolve_candidates(
            db,
            &frozen_v,
            &snapshot,
            &user.id.to_string(),
            user.is_superadmin,
        )
        .await
        {
            Ok((ids, note)) => {
                frozen.insert("candidate_user_ids".into(), json!(ids));
                frozen.insert("routing_note".into(), json!(note));
            }
            Err(RoutingError::Value(msg)) => return Err(ApiError::http(400, msg)),
            Err(RoutingError::Other(e)) => return Err(e),
        }
        frozen.insert("reasons".into(), json!(reasons));
        resolved.push(frozen);
    }

    // A "Owner informed" step is redundant when the Owner already has to approve.
    let audience = |s: &Map<String, Value>| {
        let mut ids = candidate_ids(&Value::Object(s.clone()));
        ids.sort();
        ids
    };
    let gating_audiences: Vec<Vec<String>> = resolved
        .iter()
        .filter(|s| !py::truthy(&s["is_notify_only"]))
        .map(audience)
        .collect();
    let resolved: Vec<Value> = resolved
        .into_iter()
        .filter(|s| !(py::truthy(&s["is_notify_only"]) && gating_audiences.contains(&audience(s))))
        .map(Value::Object)
        .collect();

    if let Some(p) = previous.as_mut() {
        if p.status == "approved" {
            p.status = "superseded".into();
            save(db, p).await?;
            action(
                db,
                p,
                "superseded",
                None,
                None,
                Some("Replaced by a new submission"),
                None,
                None,
            )
            .await?;
        }
    }

    let now = now();
    let label_full = adapter.document_label(&entity);
    let label_cut: String = label_full.chars().take(255).collect();
    let mut request: ApprovalRequest = sqlx::query_as(
        "INSERT INTO approval_requests (id, entity_type, entity_id, policy_id, policy_version, status, \
         document_revision, document_label, submitted_by_id, submitted_at, snapshot, flags, steps) \
         VALUES ($1, $2, $3, $4, $5, 'pending', $6, $7, $8, $9, $10, $11, $12) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(entity_type)
    .bind(entity.id())
    .bind(policy.row.id)
    .bind(policy.row.version)
    .bind(adapter.revision(&entity))
    .bind(&label_cut)
    .bind(user.id)
    .bind(now)
    .bind(&snapshot)
    .bind(json!(flags))
    .bind(Value::Array(resolved.clone()))
    .fetch_one(&mut *db)
    .await?;
    let joined = flags.join("; ");
    action(
        db,
        &request,
        "submitted",
        Some(user),
        None,
        if flags.is_empty() {
            None
        } else {
            Some(joined.as_str())
        },
        ip,
        user_agent,
    )
    .await?;

    let entity_id_s = entity.id().to_string();
    for step in resolved.iter().filter(|s| is_notify_only(s)) {
        let reasons = reasons_of(step).join("; ");
        action(
            db,
            &request,
            "notified",
            None,
            int_of(seq_of(step)),
            Some(&reasons),
            None,
            None,
        )
        .await?;
        let title = format!("FYI: {}", label(&request));
        let body = format!("{}: {}", step_str(step, "name"), reasons);
        let link = request_link(&request);
        notify(
            db,
            candidate_ids(step),
            Notice::new(&title, &body, &link, entity_type, &entity_id_s)
                .priority("low")
                .email(true),
        )
        .await?;
    }

    let first_gating = steps_of(&request).iter().position(|s| !is_notify_only(s));
    match first_gating {
        None => {
            request.status = "approved".into();
            request.completed_at = Some(now);
            request.current_step_seq = None;
            save(db, &mut request).await?;
            emit_decided(db, &request, "approved", None, true).await?;
            let authority = policy
                .row
                .sop_reference
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or(policy.row.name.clone());
            let comment =
                format!("No approval step applies: within standing authority ({authority})");
            action(
                db,
                &request,
                "auto_approved",
                None,
                None,
                Some(&comment),
                None,
                None,
            )
            .await?;
            adapter
                .on_decision(db, &mut entity, "approved", None, None)
                .await?;
        }
        Some(index) => {
            activate_step(&mut request, index, policy.row.sla_hours);
            save(db, &mut request).await?;
            let whatsapp = should_whatsapp(policy.row.whatsapp_escalate_above_base, &snapshot)?;
            let step = steps_of(&request)[index].clone();
            notify_step(db, &request, &step, whatsapp).await?;
            adapter.on_submitted(db, &mut entity, user).await?;
        }
    }
    Ok(request)
}

/// `await db.commit()` on the caller's connection, then autobegin (see the module notes). The shared
/// pattern of google_calendar.rs: SAVEPOINT only succeeds inside a transaction block, so the probe tells
/// whether there is one to commit (in autocommit every statement is already committed).
async fn session_commit(db: &mut PgConnection) -> Result<(), ApiError> {
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

/// `_auto_cancel(db, request, adapter, entity, comment, detail)`: cancel a request the engine can no longer
/// decide, persist that, and refuse the action. The cancellation (and the submitter's "Resubmit" notice)
/// must survive the 409, so it is committed here and the email/WhatsApp outbox is dispatched.
async fn auto_cancel(
    db: &mut PgConnection,
    request: &mut ApprovalRequest,
    adapter: &DocumentAdapter,
    entity: Option<&mut Entity>,
    comment: &str,
    detail: &str,
) -> ApiError {
    let result: Result<(), ApiError> = async {
        request.status = "cancelled".into();
        request.completed_at = Some(now());
        request.current_step_seq = None;
        save(db, request).await?;
        action(
            db,
            request,
            "cancelled",
            None,
            None,
            Some(comment),
            None,
            None,
        )
        .await?;
        if let Some(e) = entity {
            adapter.on_cancelled(db, e).await?;
        }
        Ok(())
    }
    .await;
    if let Err(e) = result {
        // Python: the flush error surfaces from db.commit(); the outbox is discarded.
        notify::discard_outbox();
        return e;
    }
    if let Err(e) = session_commit(db).await {
        notify::discard_outbox();
        return e;
    }
    notify::dispatch_outbox();
    ApiError::http(409, detail)
}

/// `_emit_decided(db, request, decision, user, *, auto=False)`.
async fn emit_decided(
    db: &mut PgConnection,
    request: &ApprovalRequest,
    decision: &str,
    user: Option<&AuthUser>,
    auto: bool,
) -> Result<(), ApiError> {
    crate::nary::events::emit(
        db,
        "approval.decided",
        Some(&request.entity_type),
        Some(request.entity_id),
        Some(
            json!({"request_id": request.id.to_string(), "decision": decision, "auto": auto,
                    "label": request.document_label}),
        ),
        user.map(|u| u.id),
    )
    .await?;
    Ok(())
}

/// `str.strip()`.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `act(db, *, request_id, user, decision, comment=None, ip=None, user_agent=None)`.
pub async fn act(
    db: &mut PgConnection,
    request_id: &str,
    user: &AuthUser,
    decision: &str,
    comment: Option<&str>,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> Result<ApprovalRequest, ApiError> {
    if decision != "approved" && decision != "rejected" {
        return Err(ApiError::http(
            400,
            "decision must be 'approved' or 'rejected'",
        ));
    }
    let Some(mut request) = load_for_update(db, request_id).await? else {
        return Err(ApiError::http(404, "Approval request not found"));
    };
    if request.status != "pending" {
        return Err(ApiError::http(
            409,
            format!("This request is already {}", request.status),
        ));
    }
    let Some(index) = current_step_index(&request) else {
        return Err(ApiError::http(409, "This request has no active step"));
    };
    let step = steps_of(&request)[index].clone();

    let uid = user.id.to_string();
    if !candidate_ids(&step).contains(&uid) && !user.is_superadmin {
        return Err(ApiError::http(403, "You are not an approver for this step"));
    }
    let submitter = request
        .submitted_by_id
        .map(|u| u.to_string())
        .unwrap_or_else(|| "None".into());
    if uid == submitter
        && !(user.is_superadmin
            || py::truthy(step.get("allow_self_approval").unwrap_or(&Value::Null)))
    {
        return Err(ApiError::http(
            403,
            "You cannot approve your own submission",
        ));
    }
    let comment: Option<String> = comment
        .map(py_strip)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    if decision == "rejected" && comment.is_none() {
        return Err(ApiError::http(400, "A comment is required when rejecting"));
    }

    let adapter = get_adapter(&request.entity_type)?;
    let Some(mut entity) = adapter.load(db, &request.entity_id.to_string()).await? else {
        return Err(auto_cancel(
            db,
            &mut request,
            adapter,
            None,
            "Document no longer exists",
            "The document no longer exists; the request was cancelled",
        )
        .await);
    };

    let facts = as_approved(
        adapter.document_facts(db, &entity).await?,
        &request.snapshot,
    );
    if Some(&json!(facts_fingerprint(&facts, &flags_of(&request.flags))))
        != request.snapshot.get("fingerprint")
        || adapter.revision(&entity) != request.document_revision
    {
        let title = format!("Resubmit: {}", label(&request));
        let link = request_link(&request);
        let entity_id = request.entity_id.to_string();
        notify(
            db,
            request.submitted_by_id,
            Notice::new(
                &title,
                "The document was edited while awaiting approval, so the request was cancelled.",
                &link,
                &request.entity_type,
                &entity_id,
            )
            .priority("normal")
            .email(true),
        )
        .await?;
        return Err(auto_cancel(
            db,
            &mut request,
            adapter,
            Some(&mut entity),
            "Document changed after submission: resubmit for approval",
            "The document changed after it was submitted: the request was cancelled and must be resubmitted",
        )
        .await);
    }

    let now = now();
    {
        let Value::Array(steps) = &mut request.steps else {
            unreachable!("indexed above")
        };
        let obj = steps[index].as_object_mut().expect("a step is a dict");
        obj.insert("decided_at".into(), json!(iso(&now)));
        obj.insert("decided_by_id".into(), json!(uid));
        obj.insert("decision".into(), json!(decision));
    }
    let step = steps_of(&request)[index].clone();
    let step_seq = int_of(seq_of(&step));
    save(db, &mut request).await?;
    action(
        db,
        &request,
        decision,
        Some(user),
        step_seq,
        comment.as_deref(),
        ip,
        user_agent,
    )
    .await?;

    let policy = get_policy(db, &request.entity_type).await?;
    let link = request_link(&request);
    let entity_id = request.entity_id.to_string();
    if decision == "rejected" {
        request.status = "rejected".into();
        request.completed_at = Some(now);
        request.current_step_seq = None;
        save(db, &mut request).await?;
        emit_decided(db, &request, "rejected", Some(user), false).await?;
        adapter
            .on_decision(db, &mut entity, "rejected", Some(user), comment.as_deref())
            .await?;
        let title = format!("Rejected: {}", label(&request));
        let body = format!(
            "{}: rejected by {}: {}",
            step_str(&step, "name"),
            user.name,
            comment.as_deref().unwrap_or("None")
        );
        notify(
            db,
            request.submitted_by_id,
            Notice::new(&title, &body, &link, &request.entity_type, &entity_id)
                .priority("normal")
                .email(true),
        )
        .await?;
        return Ok(request);
    }

    // remaining = [s for s in gating_steps(request) if s["seq"] > step["seq"]]
    let mut remaining: Option<usize> = None;
    for (i, s) in steps_of(&request).iter().enumerate() {
        if is_notify_only(s) {
            continue;
        }
        let later = match (py::float(seq_of(s)), py::float(seq_of(&step))) {
            (Ok(a), Ok(b)) => a > b,
            _ => false,
        };
        if later {
            remaining = Some(i);
            break;
        }
    }
    if let Some(next) = remaining {
        activate_step(
            &mut request,
            next,
            policy.as_ref().and_then(|p| p.row.sla_hours),
        );
        save(db, &mut request).await?;
        let whatsapp = should_whatsapp(
            policy
                .as_ref()
                .and_then(|p| p.row.whatsapp_escalate_above_base),
            &request.snapshot,
        )?;
        let next_step = steps_of(&request)[next].clone();
        notify_step(db, &request, &next_step, whatsapp).await?;
    } else {
        request.status = "approved".into();
        request.completed_at = Some(now);
        request.current_step_seq = None;
        save(db, &mut request).await?;
        emit_decided(db, &request, "approved", Some(user), false).await?;
        adapter
            .on_decision(db, &mut entity, "approved", Some(user), comment.as_deref())
            .await?;
        let title = format!("Approved: {}", label(&request));
        let body = format!(
            "Approved by {}{}",
            user.name,
            comment
                .as_deref()
                .map(|c| format!(": {c}"))
                .unwrap_or_default()
        );
        notify(
            db,
            request.submitted_by_id,
            Notice::new(&title, &body, &link, &request.entity_type, &entity_id)
                .priority("normal")
                .email(true),
        )
        .await?;
    }
    Ok(request)
}

/// `cancel(db, *, request_id, user, reason=None)`: the submitter (or a superadmin) withdraws a pending request.
pub async fn cancel(
    db: &mut PgConnection,
    request_id: &str,
    user: &AuthUser,
    reason: Option<&str>,
) -> Result<ApprovalRequest, ApiError> {
    let Some(mut request) = load_for_update(db, request_id).await? else {
        return Err(ApiError::http(404, "Approval request not found"));
    };
    if request.status != "pending" {
        return Err(ApiError::http(
            409,
            format!("This request is already {}", request.status),
        ));
    }
    // 1 Nov 2026 (Owner decision): the Owner (Nary's approver) may withdraw too
    if Some(user.id) != request.submitted_by_id
        && !user.is_superadmin
        && !crate::nary::roles::approver_ids(db)
            .await?
            .contains(&user.id)
    {
        return Err(ApiError::http(
            403,
            "Only the submitter can withdraw this request",
        ));
    }
    request.status = "cancelled".into();
    request.completed_at = Some(now());
    request.current_step_seq = None;
    save(db, &mut request).await?;
    let reason = reason
        .filter(|r| !r.is_empty())
        .unwrap_or("Withdrawn by submitter");
    action(
        db,
        &request,
        "cancelled",
        Some(user),
        None,
        Some(reason),
        None,
        None,
    )
    .await?;
    let adapter = get_adapter(&request.entity_type)?;
    if let Some(mut entity) = adapter.load(db, &request.entity_id.to_string()).await? {
        adapter.on_cancelled(db, &mut entity).await?;
    }
    Ok(request)
}

/// 1 Nov 2026 (direct proposal authoring): a document edited while its approval is pending. The request would
/// be cancelled at the decision anyway (its fingerprint changed); it is cancelled now, so nobody approves a text
/// that is gone, and the document goes back as on a withdrawal (a proposal in review becomes a draft). Returns
/// whether a pending request was cancelled. Does not commit.
pub async fn cancel_for_edit(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
    reason: &str,
) -> Result<bool, ApiError> {
    let Some(mut request) = latest_request(db, entity_type, entity_id).await? else {
        return Ok(false);
    };
    if request.status != "pending" {
        return Ok(false);
    }
    request.status = "cancelled".into();
    request.completed_at = Some(now());
    request.current_step_seq = None;
    save(db, &mut request).await?;
    action(
        db,
        &request,
        "cancelled",
        None,
        None,
        Some(reason),
        None,
        None,
    )
    .await?;
    let adapter = get_adapter(entity_type)?;
    if let Some(mut entity) = adapter.load(db, &entity_id.to_string()).await? {
        adapter.on_cancelled(db, &mut entity).await?;
    }
    Ok(true)
}

/// `supersede_for_entity(db, entity_type, entity_id, reason)`: called when a document is revised or
/// cancelled: old approvals stop counting (and its verification codes are retired).
pub async fn supersede_for_entity(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
    reason: &str,
) -> Result<(), ApiError> {
    let rows: Vec<ApprovalRequest> = sqlx::query_as(
        "SELECT * FROM approval_requests WHERE entity_type = $1 AND entity_id = $2 \
         AND status IN ('pending', 'approved')",
    )
    .bind(entity_type)
    .bind(entity_id)
    .fetch_all(&mut *db)
    .await?;
    for mut request in rows {
        request.status = "superseded".into();
        request.current_step_seq = None;
        request.completed_at = Some(request.completed_at.unwrap_or_else(now));
        save(db, &mut request).await?;
        action(
            db,
            &request,
            "superseded",
            None,
            None,
            Some(reason),
            None,
            None,
        )
        .await?;
    }
    supersede_verifications(db, entity_type, entity_id).await
}

async fn supersede_verifications(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<(), ApiError> {
    crate::verification::service::supersede_for_entity(db, entity_type, entity_id).await
}

/// What a gate is given: the loaded document (Python passes the ORM object) or just its id (loaded through
/// the adapter; a document that cannot be loaded is then a 404 "<Label> not found", where Python would
/// have used the caller's stale object).
#[derive(Clone, Copy)]
pub enum EntityArg<'a> {
    Entity(&'a Entity),
    Id(Uuid),
}

impl<'a> From<&'a Entity> for EntityArg<'a> {
    fn from(e: &'a Entity) -> Self {
        EntityArg::Entity(e)
    }
}

impl From<Uuid> for EntityArg<'_> {
    fn from(id: Uuid) -> Self {
        EntityArg::Id(id)
    }
}

impl EntityArg<'_> {
    fn id(&self) -> Uuid {
        match self {
            EntityArg::Entity(e) => e.id(),
            EntityArg::Id(id) => *id,
        }
    }
}

/// `ensure_approved(db, entity_type, entity, action)`: gate for sending / signing / publishing. No active
/// policy -> no gate. Otherwise the latest request must be approved AND still describe the document as it
/// is now.
pub async fn ensure_approved<'a>(
    db: &mut PgConnection,
    entity_type: &str,
    entity: impl Into<EntityArg<'a>>,
    action: &str,
) -> Result<Option<ApprovalRequest>, ApiError> {
    let entity = entity.into();
    if get_policy(db, entity_type).await?.is_none() {
        return Ok(None);
    }
    let adapter = get_adapter(entity_type)?;
    // Callers pass whatever they loaded; reload with the adapter's eager options.
    let loaded = match (adapter.load(db, &entity.id().to_string()).await?, entity) {
        (Some(e), _) => e,
        (None, EntityArg::Entity(e)) => e.clone(),
        (None, EntityArg::Id(_)) => {
            return Err(ApiError::http(404, format!("{} not found", adapter.label)))
        }
    };
    let latest = latest_request(db, entity_type, loaded.id()).await?;
    let lower = adapter.label.to_lowercase();
    let request = match latest {
        Some(r) if r.status == "approved" => r,
        latest => {
            let state = latest
                .as_ref()
                .map(|r| r.status.clone())
                .unwrap_or_else(|| "not_submitted".into());
            let mut message = format!("This {lower} must be approved before it can be {action}");
            if state == "pending" {
                message.push_str(" (approval is pending)");
            }
            return Err(ApiError::http_json(
                409,
                json!({
                    "code": "approval_required", "approval_status": state,
                    "request_id": latest.as_ref().map(|r| r.id.to_string()),
                    "message": message,
                }),
            ));
        }
    };
    let facts = as_approved(
        adapter.document_facts(db, &loaded).await?,
        &request.snapshot,
    );
    if Some(&json!(facts_fingerprint(&facts, &flags_of(&request.flags))))
        != request.snapshot.get("fingerprint")
        || adapter.revision(&loaded) != request.document_revision
    {
        return Err(ApiError::http_json(
            409,
            json!({
                "code": "approval_stale", "approval_status": "stale", "request_id": request.id.to_string(),
                "message": format!("This {lower} changed after it was approved: resubmit it for approval"),
            }),
        ));
    }
    Ok(Some(request))
}

/// The approval gate as one word (17 Nov 2026, shared by the approval panel's idea of it, the quotation badge and
/// Nary's tools): `not_required` (no policy), `approved`, or the refusal's `approval_status` (`not_submitted`,
/// `pending`, `stale`, `rejected`, ...).
pub async fn gate_state<'a>(
    db: &mut PgConnection,
    entity_type: &str,
    entity: impl Into<EntityArg<'a>>,
) -> Result<String, ApiError> {
    match ensure_approved(db, entity_type, entity, "used").await {
        Ok(Some(_)) => Ok("approved".into()),
        Ok(None) => Ok("not_required".into()),
        Err(ApiError::Http { detail, .. }) => Ok(match &detail {
            Value::Object(d) => match d.get("approval_status") {
                Some(Value::String(s)) => s.clone(),
                _ => "not_submitted".into(),
            },
            _ => "blocked".into(),
        }),
        Err(e) => Err(e),
    }
}

/// A quotation's status as people read it (17 Nov 2026, Owner): a draft whose approval still matches it is
/// `approved` (approved, not sent yet); every other quotation keeps its stored status. The stored status stays
/// `draft` (it is still edited and sent as a draft; an edit makes the approval stale and it reads `draft` again).
pub async fn quotation_display_status(
    db: &mut PgConnection,
    quotation_id: Uuid,
    stored: &str,
) -> Result<String, ApiError> {
    if stored != "draft" {
        return Ok(stored.to_string());
    }
    // cheap first: only a draft whose latest request is approved can read `approved`
    let latest: Option<String> = sqlx::query_scalar(
        "SELECT status FROM approval_requests WHERE entity_type = 'quotation' AND entity_id = $1 \
         AND status <> 'superseded' ORDER BY submitted_at DESC LIMIT 1",
    )
    .bind(quotation_id)
    .fetch_optional(&mut *db)
    .await?;
    if latest.as_deref() != Some("approved") {
        return Ok("draft".into());
    }
    Ok(
        if gate_state(db, "quotation", quotation_id).await? == "approved" {
            "approved".into()
        } else {
            "draft".into()
        },
    )
}

/// `is_approved(db, entity_type, entity)`: the gate as a boolean (any HTTPException is "no").
pub async fn is_approved<'a>(
    db: &mut PgConnection,
    entity_type: &str,
    entity: impl Into<EntityArg<'a>>,
) -> Result<bool, ApiError> {
    match ensure_approved(db, entity_type, entity, "used").await {
        Ok(_) => Ok(true),
        Err(ApiError::Http { .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `states_for(db, entity_type, entity_ids)`: latest approval state per document, for list pages
/// (`{entity_id: {"request_id", "status", "current_step"}}`, dict order kept).
pub async fn states_for(
    db: &mut PgConnection,
    entity_type: &str,
    entity_ids: &[Uuid],
) -> Result<Map<String, Value>, ApiError> {
    if entity_ids.is_empty() {
        return Ok(Map::new());
    }
    let rows: Vec<ApprovalRequest> = sqlx::query_as(
        "SELECT * FROM approval_requests WHERE entity_type = $1 AND entity_id = ANY($2) \
         AND status != 'superseded' ORDER BY submitted_at",
    )
    .bind(entity_type)
    .bind(entity_ids)
    .fetch_all(&mut *db)
    .await?;
    let mut out = Map::new();
    for request in &rows {
        // ascending, so the latest wins
        let step = current_step(request);
        out.insert(
            request.entity_id.to_string(),
            json!({
                "request_id": request.id.to_string(), "status": request.status,
                "current_step": step.map(|s| s.get("name").cloned().unwrap_or(Value::Null)),
            }),
        );
    }
    Ok(out)
}

fn opt_get(s: &Value, k: &str) -> Value {
    s.get(k).cloned().unwrap_or(Value::Null)
}

/// `serialize(db, request, viewer=None)`: the request as the API returns it.
pub async fn serialize(
    db: &mut PgConnection,
    request: &ApprovalRequest,
    viewer: Option<&AuthUser>,
) -> Result<Value, ApiError> {
    let adapter = get_adapter(&request.entity_type)?;
    let actions: Vec<ApprovalAction> =
        sqlx::query_as("SELECT * FROM approval_actions WHERE request_id = $1 ORDER BY created_at")
            .bind(request.id)
            .fetch_all(&mut *db)
            .await?;
    let mut user_ids: Vec<String> = vec![];
    if let Some(s) = request.submitted_by_id {
        user_ids.push(s.to_string());
    }
    for step in steps_of(request) {
        user_ids.extend(candidate_ids(step));
    }
    user_ids.extend(
        actions
            .iter()
            .filter_map(|a| a.actor_id)
            .map(|u| u.to_string()),
    );
    let mut names: std::collections::HashMap<String, String> = Default::default();
    if !user_ids.is_empty() {
        let mut ids: Vec<Uuid> = vec![];
        for u in &user_ids {
            ids.push(
                Uuid::parse_str(u).map_err(|e| {
                    ApiError::internal(anyhow::anyhow!("invalid user id {u:?}: {e}"))
                })?,
            );
        }
        let rows: Vec<(Uuid, String)> =
            sqlx::query_as("SELECT id, name FROM users WHERE id = ANY($1)")
                .bind(&ids)
                .fetch_all(&mut *db)
                .await?;
        names = rows
            .into_iter()
            .map(|(id, n)| (id.to_string(), n))
            .collect();
    }
    let name_of = |id: &str| -> Value { names.get(id).map(|n| json!(n)).unwrap_or(Value::Null) };

    let submitted_by = request.submitted_by_id.map(|u| u.to_string());
    let step_now = current_step(request);
    let mut can_act = false;
    if let (Some(viewer), Some(step_now)) = (viewer, step_now) {
        if request.status == "pending" {
            let vid = viewer.id.to_string();
            let eligible = candidate_ids(step_now).contains(&vid) || viewer.is_superadmin;
            let self_blocked = Some(&vid) == submitted_by.as_ref()
                && !(viewer.is_superadmin || py::truthy(&opt_get(step_now, "allow_self_approval")));
            can_act = eligible && !self_blocked;
        }
    }
    // str(None) == "None" never equals a user id, so a missing submitter blocks nobody.

    let snap = &request.snapshot;
    let current = request.current_step_seq.map(|s| json!(s));
    let steps: Vec<Value> = steps_of(request)
        .iter()
        .map(|s| {
            let notify_only = py::truthy(&opt_get(s, "is_notify_only"));
            let decision = opt_get(s, "decision");
            let state = if notify_only {
                json!("notified")
            } else if py::truthy(&decision) {
                decision.clone()
            } else if current.as_ref().is_some_and(|c| py::eq(seq_of(s), c)) {
                json!("current")
            } else if FINAL_STATUSES.contains(&request.status.as_str()) {
                json!("skipped")
            } else {
                json!("waiting")
            };
            let reasons = opt_get(s, "reasons");
            let decided_by = opt_get(s, "decided_by_id");
            json!({
                "seq": seq_of(s), "name": opt_get(s, "name"),
                "is_notify_only": s.get("is_notify_only").cloned().unwrap_or(json!(false)),
                "reasons": if py::truthy(&reasons) { reasons } else { json!([]) },
                "routing_note": opt_get(s, "routing_note"),
                "candidates": candidate_ids(s).iter().map(|c| json!({"id": c, "name": name_of(c)})).collect::<Vec<_>>(),
                "due_at": opt_get(s, "due_at"), "decided_at": opt_get(s, "decided_at"),
                "decided_by": if py::truthy(&decided_by) {
                    json!({"id": decided_by, "name": name_of(&py::str_v(&decided_by))})
                } else { Value::Null },
                "decision": decision,
                "state": state,
            })
        })
        .collect();
    let actions_out: Vec<Value> = actions
        .iter()
        .map(|a| {
            json!({
                "action": a.action, "step_seq": a.step_seq, "comment": a.comment,
                "actor": a.actor_id.map(|id| json!({"id": id.to_string(), "name": name_of(&id.to_string())})),
                "at": iso(&a.created_at),
            })
        })
        .collect();
    let flags = if py::truthy(&request.flags) {
        request.flags.clone()
    } else {
        json!([])
    };
    Ok(json!({
        "id": request.id.to_string(),
        "entity_type": request.entity_type,
        "entity_label": adapter.label,
        "entity_id": request.entity_id.to_string(),
        "document_label": request.document_label,
        "document_revision": request.document_revision,
        "link": adapter.link_for_id(&request.entity_id.to_string()),
        "status": request.status,
        "submitted_by": {"id": submitted_by, "name": submitted_by.as_deref().map(name_of).unwrap_or(Value::Null)},
        "submitted_at": iso(&request.submitted_at),
        "completed_at": request.completed_at.as_ref().map(iso),
        "current_step_seq": request.current_step_seq,
        "flags": flags,
        "amount": opt_get(snap, "amount"), "currency": opt_get(snap, "currency"),
        "amount_base": opt_get(snap, "amount_base"),
        "steps": steps,
        "actions": actions_out,
        "can_act": can_act,
        "can_cancel": viewer.is_some_and(|v| request.status == "pending"
            && (Some(v.id) == request.submitted_by_id || v.is_superadmin)),
    }))
}

// ─── Helpers for document endpoints ───────────────────────────────────────────

/// `submit_if_policy(db, *, entity_type, entity_id, user, flags=None)`: submit only when the document type
/// has an active policy (no policy -> no approval).
pub async fn submit_if_policy(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: &str,
    user: &AuthUser,
    flags: Option<&[String]>,
) -> Result<Option<ApprovalRequest>, ApiError> {
    if get_policy(db, entity_type).await?.is_none() {
        return Ok(None);
    }
    Ok(Some(
        submit(db, entity_type, entity_id, user, flags, None, None).await?,
    ))
}

/// `act_on_pending(db, entity_type, entity_id, user, decision, comment=None)`: for the document's own
/// approve/reject endpoints: decide its pending request's current step.
pub async fn act_on_pending(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
    user: &AuthUser,
    decision: &str,
    comment: Option<&str>,
) -> Result<ApprovalRequest, ApiError> {
    let request = latest_request(db, entity_type, entity_id).await?;
    match request {
        Some(r) if r.status == "pending" => {
            act(db, &r.id.to_string(), user, decision, comment, None, None).await
        }
        other => {
            let label = get_adapter(entity_type)?.label.to_lowercase();
            Err(ApiError::http_json(
                409,
                json!({
                    "code": "nothing_pending",
                    "approval_status": other.map(|r| r.status).unwrap_or_else(|| "not_submitted".into()),
                    "message": format!("This {label} has no pending approval request: submit it for approval first"),
                }),
            ))
        }
    }
}

/// `ensure_or_submit(db, entity_type, entity, user, action, *, flags=None)`: gate for an action that should
/// TRIGGER approval when needed (sending a quotation, publishing...).
///
/// Approved and unchanged -> proceed. Never submitted / rejected / withdrawn / stale -> submit now:
/// a document within standing authority is auto-approved and proceeds, and an Owner's own submission
/// is approved on the spot (the Owner may self-approve). Otherwise the submission is committed and a
/// 409 `approval_pending` is raised: the action waits for the approver.
pub async fn ensure_or_submit<'a>(
    db: &mut PgConnection,
    entity_type: &str,
    entity: impl Into<EntityArg<'a>>,
    user: &AuthUser,
    action: &str,
    flags: Option<&[String]>,
) -> Result<Option<ApprovalRequest>, ApiError> {
    let entity = entity.into();
    // 2 Nov 2026 (Owner decision): a quotation whose notes and line taxes disagree is never sent (also one
    // approved before the gate existed)
    if entity_type == "quotation" && action.starts_with("sent") {
        if let Some(why) = crate::quotation_tax::quotation_refusal(db, entity.id()).await? {
            return Err(ApiError::http(400, why));
        }
    }
    if get_policy(db, entity_type).await?.is_none() {
        return Ok(None);
    }
    match ensure_approved(db, entity_type, entity, action).await {
        Ok(r) => return Ok(r),
        Err(e) => {
            let state = match &e {
                ApiError::Http {
                    detail: Value::Object(d),
                    ..
                } => d.get("approval_status").and_then(|s| s.as_str()),
                _ => None,
            };
            if !matches!(
                state,
                Some("not_submitted" | "rejected" | "cancelled" | "stale")
            ) {
                return Err(e);
            }
        }
    }

    let mut request = submit(
        db,
        entity_type,
        &entity.id().to_string(),
        user,
        flags,
        None,
        None,
    )
    .await?;
    let uid = user.id.to_string();
    let self_approve = request.status == "pending"
        && user.is_superadmin
        && current_step(&request).is_some_and(|s| candidate_ids(s).contains(&uid));
    if self_approve {
        request = act(
            db,
            &request.id.to_string(),
            user,
            "approved",
            Some("Approved on submission"),
            None,
            None,
        )
        .await?;
    }
    if request.status == "approved" {
        return Ok(Some(request));
    }

    session_commit(db).await?;
    notify::dispatch_outbox();
    let step_name = current_step(&request).map(|s| step_str(s, "name"));
    Err(ApiError::http_json(
        409,
        json!({
            "code": "approval_pending", "approval_status": "pending", "request_id": request.id.to_string(),
            "message": format!("Submitted for approval: {} is required before it can be {action}",
                               step_name.as_deref().unwrap_or("approval")),
        }),
    ))
}

// ─── SLA sweep ────────────────────────────────────────────────────────────────

/// `datetime.fromisoformat(s)` for the aware timestamps the engine writes.
fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%:z"))
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%:z"))
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// `remind_overdue(db, now=None)`: re-notify the approvers of every step past its SLA (by email and
/// WhatsApp) and tell the submitter it is waiting. At most one reminder per request per REMINDER_INTERVAL.
pub async fn remind_overdue(
    db: &mut PgConnection,
    now_arg: Option<DateTime<Utc>>,
) -> Result<i64, ApiError> {
    let now = now_arg.unwrap_or_else(now);
    let rows: Vec<ApprovalRequest> = sqlx::query_as(
        "SELECT * FROM approval_requests WHERE status = 'pending' FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(&mut *db)
    .await?;
    let mut reminded = 0;
    for mut request in rows {
        let Some(step) = current_step(&request).cloned() else {
            continue;
        };
        let due_v = opt_get(&step, "due_at");
        if !py::truthy(&due_v) {
            continue;
        }
        let due_s = py::str_v(&due_v);
        let due = parse_iso(&due_s).ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!(
                "ValueError: Invalid isoformat string: {due_s:?}"
            ))
        })?;
        if due > now
            || request
                .last_reminded_at
                .is_some_and(|last| now - last < reminder_interval())
        {
            continue;
        }
        let micros = (now - due).num_microseconds().unwrap_or(i64::MAX);
        let hours_over = ((micros as f64 / 1e6) / 3600.0).floor().max(1.0) as i64;
        let link = request_link(&request);
        let entity_id = request.entity_id.to_string();
        let name = step_str(&step, "name");
        let title = format!("Overdue approval: {}", label(&request));
        let body =
            format!("{name} is {hours_over}h past its SLA and still waiting for your decision");
        notify(
            db,
            candidate_ids(&step),
            Notice::new(&title, &body, &link, &request.entity_type, &entity_id)
                .priority("high")
                .email(true)
                .whatsapp(true),
        )
        .await?;
        if let Some(submitter) = request.submitted_by_id {
            if !candidate_ids(&step).contains(&submitter.to_string()) {
                let title = format!("Approval overdue: {}", label(&request));
                let body = format!(
                    "Still waiting on {name} ({hours_over}h past its SLA); the approvers were reminded"
                );
                notify(
                    db,
                    [submitter],
                    Notice::new(&title, &body, &link, &request.entity_type, &entity_id)
                        .priority("low")
                        .email(false),
                )
                .await?;
            }
        }
        request.last_reminded_at = Some(now);
        save(db, &mut request).await?;
        reminded += 1;
    }
    Ok(reminded)
}
