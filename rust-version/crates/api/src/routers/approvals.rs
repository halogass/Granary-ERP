//! backend/app/routers/approvals.py, mounted at `/api/v1/approvals` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! The inbox, submit/approve/reject/withdraw, the approval trail for a document and policy administration.
//! Who may act on a step is decided by the engine from the request's frozen routing; `approvals:view`
//! only controls visibility.
//!
//! Route order: Python registers the static routes before `/{request_id}`, which is what axum does. Two
//! Starlette fall-throughs are reproduced: `GET /submit` reaches `get_request("submit")` (only POST is
//! declared at `/submit`), and `POST /policies/{approve|reject|cancel}` reaches the request actions
//! with request_id "policies" (only PATCH is declared at `/policies/{policy_id}`).

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::{v, Cx, PyJson, PyObj, PyQuery, Validate};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::approval::{
    ApprovalPolicy, ApprovalPolicyStep, ApprovalRequest, DiscountAllowance,
};
use granary_services::approvals::adapters::{get_adapter, ADAPTERS};
use granary_services::approvals::conditions::{
    precondition_meta, rule_meta, validate_condition, validate_preconditions, CondError,
};
use granary_services::approvals::engine;
use granary_services::approvals::notify::{discard_outbox, dispatch_outbox};

use super::at;

use crate::extract::ClientIp;
use crate::resp::{self, iso, iso_opt};

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/approvals";

/// The routes of `router` in approvals.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/count"), get(count_awaiting))
        .route(&at(PREFIX, "/rules"), get(list_rules))
        .route(
            &at(PREFIX, "/submit"),
            post(submit_for_approval).get(get_request_submit),
        )
        .route(
            &at(PREFIX, "/entity/{entity_type}/{entity_id}"),
            get(approval_for_document),
        )
        .route(&at(PREFIX, "/policies"), get(list_policies))
        .route(
            &at(PREFIX, "/policies/{request_id}"),
            patch(update_policy).post(policies_fallthrough),
        )
        .route(
            &at(PREFIX, "/discount-allowances"),
            get(list_allowances).put(replace_allowances),
        )
        .route(&at(PREFIX, ""), get(list_requests))
        .route(&at(PREFIX, "/{request_id}"), get(get_request))
        .route(&at(PREFIX, "/{request_id}/approve"), post(approve_request))
        .route(&at(PREFIX, "/{request_id}/reject"), post(reject_request))
        .route(&at(PREFIX, "/{request_id}/cancel"), post(cancel_request))
}

const APPROVER_MODES: [&str; 5] = ["owner", "manager", "role", "snapshot_user", "user"]; // sorted
const SIGNATURE_FLOWS: [&str; 4] = [
    "counterparty",
    "internal",
    "internal_then_counterparty",
    "none",
];
const VIEW_SCOPES: [&str; 2] = ["metadata", "metadata_document"];
const ALLOWANCE_KINDS: [&str; 4] = ["bundle", "loyalty", "multi_year_saas", "volume"];

/// `str(sorted(SET))`: Python's list repr.
fn py_list(items: &[&str]) -> String {
    let parts: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", parts.join(", "))
}

fn require_view(user: &AuthUser) -> ApiResult<()> {
    if !(user.is_superadmin || user.perms.contains("approvals:view")) {
        return Err(ApiError::http(403, "Permission required: approvals:view"));
    }
    Ok(())
}

fn can_manage(user: &AuthUser) -> bool {
    user.is_superadmin || user.perms.contains("approvals:manage_policies")
}

fn require_manage(user: &AuthUser) -> ApiResult<()> {
    if !can_manage(user) {
        return Err(ApiError::http(
            403,
            "Permission required: approvals:manage_policies",
        ));
    }
    Ok(())
}

fn str_opt(id: Option<Uuid>) -> String {
    id.map(|u| u.to_string()).unwrap_or_else(|| "None".into())
}

fn list_contains(v: Option<&Value>, needle: &str) -> bool {
    match v {
        Some(Value::Array(a)) => a.iter().any(|x| x.as_str() == Some(needle)),
        Some(Value::String(s)) => s.contains(needle),
        _ => false,
    }
}

/// `_can_see(user, request)`.
fn can_see(user: &AuthUser, request: &ApprovalRequest) -> bool {
    let uid = user.id.to_string();
    if can_manage(user) || str_opt(request.submitted_by_id) == uid {
        return true;
    }
    match &request.steps {
        Value::Array(steps) => steps
            .iter()
            .any(|s| list_contains(s.get("candidate_user_ids"), &uid)),
        _ => false,
    }
}

/// `_awaiting(user, request)`.
fn awaiting(user: &AuthUser, request: &ApprovalRequest) -> bool {
    let Some(step) = engine::current_step(request) else {
        return false;
    };
    let uid = user.id.to_string();
    let empty = step.as_object().is_some_and(|o| o.is_empty());
    if empty || request.status != "pending" || !list_contains(step.get("candidate_user_ids"), &uid)
    {
        return false;
    }
    let self_ok = user.is_superadmin
        || step
            .get("allow_self_approval")
            .is_some_and(granary_services::whatsapp::py_truthy);
    !(uid == str_opt(request.submitted_by_id) && !self_ok)
}

/// `_commit_and_dispatch(db)`.
async fn commit_and_dispatch(db: &Db) -> ApiResult<()> {
    if let Err(e) = db.commit().await {
        discard_outbox();
        return Err(e);
    }
    dispatch_outbox();
    Ok(())
}

// ─── Static routes first ──────────────────────────────────────────────────────

/// `GET /count`: pending requests awaiting the caller.
pub async fn count_awaiting(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let pending: Vec<ApprovalRequest> = sqlx::query_as(
        "SELECT * FROM approval_requests WHERE approval_requests.status = 'pending'",
    )
    .fetch_all(&mut *conn)
    .await?;
    let n = pending.iter().filter(|r| awaiting(&user, r)).count();
    Ok(resp::json(json!({"awaiting_me": n})))
}

/// `GET /rules`: the condition builder's metadata.
pub async fn list_rules(InternalUser(user): InternalUser) -> ApiResult<Response> {
    require_view(&user)?;
    let mut types = Map::new();
    for a in ADAPTERS.iter() {
        types.insert(
            a.entity_type.to_string(),
            Value::String(a.label.to_string()),
        );
    }
    Ok(resp::json(json!({
        "rules": rule_meta(),
        "preconditions": precondition_meta(),
        "approver_modes": APPROVER_MODES,
        "entity_types": Value::Object(types),
    })))
}

granary_core::py_model! {
    /// `class SubmitBody(BaseModel)`.
    pub struct SubmitBody {
        pub entity_type: String,
        pub entity_id: String,
        pub flags: Vec<String> = Vec::new(),
    }
}

/// `request.headers.get("user-agent")`.
fn user_agent(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("user-agent")
        .map(|v| v.as_bytes().iter().map(|b| *b as char).collect())
}

/// `POST /submit` (201).
pub async fn submit_for_approval(
    InternalUser(user): InternalUser,
    db: Db,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    PyJson(body): PyJson<SubmitBody>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let ua = user_agent(&headers);
    let mut conn = db.conn().await?;
    let result = engine::submit(
        &mut conn,
        &body.entity_type,
        &body.entity_id,
        &user,
        Some(&body.flags),
        ip.as_deref(),
        ua.as_deref(),
    )
    .await?;
    drop(conn);
    commit_and_dispatch(&db).await?;
    let mut conn = db.conn().await?;
    let out = engine::serialize(&mut conn, &result, Some(&user)).await?;
    Ok(resp::json_status(201, out))
}

/// `_flatten(condition)`: the leaf conditions.
fn flatten(condition: &Value) -> ApiResult<Vec<&Map<String, Value>>> {
    if !granary_services::whatsapp::py_truthy(condition) {
        return Ok(vec![]);
    }
    let Value::Object(o) = condition else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "AttributeError: condition is not a dict"
        )));
    };
    if o.contains_key("any") || o.contains_key("all") {
        let items = o
            .get("any")
            .or_else(|| o.get("all"))
            .unwrap_or(&Value::Null);
        let Value::Array(items) = items else {
            return Err(ApiError::internal(anyhow::anyhow!(
                "TypeError: condition group is not a list"
            )));
        };
        let mut out = vec![];
        for c in items {
            out.extend(flatten(c)?);
        }
        return Ok(out);
    }
    if let Some(n) = o.get("not") {
        return flatten(n);
    }
    Ok(vec![o])
}

/// `GET /entity/{entity_type}/{entity_id}`: the approval panel of a document.
pub async fn approval_for_document(
    InternalUser(user): InternalUser,
    db: Db,
    Path((entity_type, entity_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let adapter = get_adapter(&entity_type)?;
    let mut conn = db.conn().await?;
    let Some(entity) = adapter.load(&mut conn, &entity_id).await? else {
        return Err(ApiError::http(404, format!("{} not found", adapter.label)));
    };
    let policy = engine::get_policy(&mut conn, &entity_type).await?;
    let history: Vec<ApprovalRequest> = sqlx::query_as(
        "SELECT * FROM approval_requests WHERE approval_requests.entity_type = $1 \
         AND approval_requests.entity_id = $2 ORDER BY approval_requests.submitted_at DESC",
    )
    .bind(&entity_type)
    .bind(entity.id())
    .fetch_all(&mut *conn)
    .await?;
    let history: Vec<ApprovalRequest> = history.into_iter().filter(|r| can_see(&user, r)).collect();
    let latest = history.iter().find(|r| r.status != "superseded");

    let mut gate = "not_required".to_string();
    if policy.is_some() {
        match engine::ensure_approved(&mut conn, &entity_type, &entity, "used").await {
            Ok(_) => gate = "approved".into(),
            Err(ApiError::Http { detail, .. }) => {
                gate = match &detail {
                    Value::Object(d) => match d.get("approval_status") {
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => py_str(other),
                        None => "not_submitted".into(),
                    },
                    _ => "blocked".into(),
                }
            }
            Err(e) => return Err(e),
        }
    }

    let policy_json = match &policy {
        Some(p) => {
            let mut flags: Vec<String> = vec![];
            for s in &p.steps {
                for c in flatten(s.condition.as_ref().unwrap_or(&Value::Null))? {
                    if c.get("rule").and_then(|r| r.as_str()) == Some("flag_set") {
                        let v = c.get("value").cloned().unwrap_or(Value::Null);
                        flags.push(py_str(&v));
                    }
                }
            }
            if let Value::Array(pre) = &p.row.preconditions {
                for pc in pre {
                    if pc.get("rule").and_then(|r| r.as_str()) == Some("flag_missing_above_base") {
                        let flag =
                            pc.get("value").and_then(|v| v.get("flag")).ok_or_else(|| {
                                ApiError::internal(anyhow::anyhow!("KeyError: 'flag'"))
                            })?;
                        flags.push(py_str(flag));
                    }
                }
            }
            flags.sort();
            flags.dedup();
            json!({"id": p.row.id.to_string(), "name": p.row.name, "sop_reference": p.row.sop_reference,
                   "signature_flow": p.row.signature_flow, "qr_enabled": p.row.qr_enabled, "flags": flags})
        }
        None => Value::Null,
    };
    let request = match latest {
        Some(r) => engine::serialize(&mut conn, r, Some(&user)).await?,
        None => Value::Null,
    };
    let hist: Vec<Value> = history
        .iter()
        .map(|r| {
            json!({"id": r.id.to_string(), "status": r.status, "document_revision": r.document_revision,
                   "submitted_at": iso(&r.submitted_at), "completed_at": iso_opt(r.completed_at.as_ref())})
        })
        .collect();
    Ok(resp::json(json!({
        "policy": policy_json,
        "gate": gate,
        "request": request,
        "history": hist,
    })))
}

// ─── Policies ─────────────────────────────────────────────────────────────────

fn policy_dict(p: &ApprovalPolicy, steps: &[ApprovalPolicyStep]) -> Value {
    let label = ADAPTERS
        .iter()
        .find(|a| a.entity_type == p.entity_type)
        .map(|a| a.label.to_string())
        .unwrap_or_else(|| p.entity_type.clone());
    let preconditions = if granary_services::whatsapp::py_truthy(&p.preconditions) {
        p.preconditions.clone()
    } else {
        json!([])
    };
    json!({
        "id": p.id.to_string(), "entity_type": p.entity_type, "entity_label": label,
        "name": p.name, "description": p.description, "sop_reference": p.sop_reference,
        "is_active": p.is_active, "version": p.version, "preconditions": preconditions,
        "signature_flow": p.signature_flow, "signer_role": p.signer_role,
        "sign_deadline_days": p.sign_deadline_days, "max_extension_days": p.max_extension_days,
        "qr_enabled": p.qr_enabled, "view_scope": p.view_scope, "allow_download": p.allow_download,
        "requires_login": p.requires_login, "token_ttl_days": p.token_ttl_days,
        "sla_hours": p.sla_hours,
        "whatsapp_escalate_above_base": resp::dec_float_opt(p.whatsapp_escalate_above_base.as_ref()),
        "updated_at": iso(&p.updated_at),
        "steps": steps.iter().map(|s| json!({
            "id": s.id.to_string(), "sequence": s.sequence, "name": s.name, "approver_mode": s.approver_mode,
            "role_slug": s.role_slug, "department": s.department, "snapshot_user_key": s.snapshot_user_key,
            "user_id": s.user_id.map(|u| u.to_string()), "condition": s.condition,
            "sla_hours": s.sla_hours, "is_notify_only": s.is_notify_only,
            "allow_self_approval": s.allow_self_approval,
        })).collect::<Vec<_>>(),
    })
}

async fn steps_for(conn: &mut PgConnection, ids: &[Uuid]) -> ApiResult<Vec<ApprovalPolicyStep>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    Ok(sqlx::query_as(
        "SELECT * FROM approval_policy_steps WHERE approval_policy_steps.policy_id = ANY($1) \
         ORDER BY approval_policy_steps.sequence",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?)
}

/// `GET /policies`: every policy with its steps, by name.
pub async fn list_policies(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let policies: Vec<ApprovalPolicy> =
        sqlx::query_as("SELECT * FROM approval_policies ORDER BY approval_policies.name")
            .fetch_all(&mut *conn)
            .await?;
    let ids: Vec<Uuid> = policies.iter().map(|p| p.id).collect();
    let steps = steps_for(&mut conn, &ids).await?;
    let out: Vec<Value> = policies
        .iter()
        .map(|p| {
            let mine: Vec<ApprovalPolicyStep> = steps
                .iter()
                .filter(|s| s.policy_id == p.id)
                .cloned()
                .collect();
            policy_dict(p, &mine)
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class StepBody(BaseModel)`.
    pub struct StepBody {
        pub sequence: i64 => v::int_c(v::NumC::new().ge("1")),
        pub name: String => v::str_c(v::StrC::new().min_length(1).max_length(255)),
        pub approver_mode: String,
        pub role_slug: Option<String> = None,
        pub department: Option<String> = None,
        pub snapshot_user_key: Option<String> = None,
        pub user_id: Option<String> = None,
        pub condition: Option<Map<String, Value>> = None,
        pub sla_hours: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("1"))),
        pub is_notify_only: bool = false,
        pub allow_self_approval: bool = false,
    }
}

granary_core::py_model! {
    /// `class PolicyUpdate(BaseModel)`.
    pub struct PolicyUpdate {
        pub name: Option<String> = None,
        pub description: Option<String> = None,
        pub is_active: Option<bool> = None,
        pub preconditions: Option<Vec<Map<String, Value>>> = None,
        pub signature_flow: Option<String> = None,
        pub signer_role: Option<String> = None,
        pub sign_deadline_days: Option<i64> = None,
        pub max_extension_days: Option<i64> = None,
        pub qr_enabled: Option<bool> = None,
        pub view_scope: Option<String> = None,
        pub allow_download: Option<bool> = None,
        pub requires_login: Option<bool> = None,
        pub token_ttl_days: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("1"))),
        pub sla_hours: Option<i64> = None => v::nullable(v::int_c(v::NumC::new().ge("1"))),
        pub whatsapp_escalate_above_base: Option<Decimal> = None,
        pub steps: Option<Vec<StepBody>> = None,
    }
}

/// `PolicyUpdate` plus the keys the client sent (`model_dump(exclude_unset=True)`).
pub struct PolicyUpdateIn {
    pub body: PolicyUpdate,
    pub set: Vec<String>,
}

impl Validate for PolicyUpdateIn {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self> {
        let body = PolicyUpdate::validate(v, cx)?;
        let set = match v {
            PyObj::Dict(d) => d.keys().cloned().collect(),
            _ => vec![],
        };
        Some(PolicyUpdateIn { body, set })
    }
}

fn cond_err(e: CondError) -> ApiError {
    if e.kind == "ValueError" {
        ApiError::http(400, e.msg)
    } else {
        ApiError::internal(anyhow::anyhow!("{}: {}", e.kind, e.msg))
    }
}

fn i32_of(v: Option<i64>) -> ApiResult<Option<i32>> {
    v.map(|x| {
        i32::try_from(x)
            .map_err(|_| ApiError::internal(anyhow::anyhow!("DataError: value out of int32 range")))
    })
    .transpose()
}

/// `PATCH /policies/{policy_id}`: validated edit; steps are replaced; the version is bumped.
pub async fn update_policy(
    InternalUser(user): InternalUser,
    db: Db,
    Path(policy_id): Path<String>,
    PyJson(input): PyJson<PolicyUpdateIn>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    let id = pg_uuid(&policy_id)?;
    let mut conn = db.conn().await?;
    let policy: Option<ApprovalPolicy> =
        sqlx::query_as("SELECT * FROM approval_policies WHERE approval_policies.id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(policy) = policy else {
        return Err(ApiError::http(404, "Policy not found"));
    };
    let body = &input.body;
    let set = |k: &str| input.set.iter().any(|s| s == k);

    // Validation (ValueError -> 400), in Python's order.
    if set("preconditions") {
        let pre = match &body.preconditions {
            Some(list) => Value::Array(list.iter().cloned().map(Value::Object).collect()),
            None => Value::Null,
        };
        validate_preconditions(&pre).map_err(cond_err)?;
    }
    if let Some(sf) = &body.signature_flow {
        if !SIGNATURE_FLOWS.contains(&sf.as_str()) {
            return Err(ApiError::http(
                400,
                format!(
                    "signature_flow must be one of {}",
                    py_list(&SIGNATURE_FLOWS)
                ),
            ));
        }
    }
    if let Some(vs) = &body.view_scope {
        if !VIEW_SCOPES.contains(&vs.as_str()) {
            return Err(ApiError::http(
                400,
                format!("view_scope must be one of {}", py_list(&VIEW_SCOPES)),
            ));
        }
    }
    let steps = if set("steps") {
        body.steps.as_ref()
    } else {
        None
    };
    if let Some(steps) = steps {
        let mut seen = std::collections::HashSet::new();
        if !steps.iter().all(|s| seen.insert(s.sequence)) {
            return Err(ApiError::http(400, "Step sequences must be unique"));
        }
        let truthy = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.is_empty());
        for s in steps {
            if !APPROVER_MODES.contains(&s.approver_mode.as_str()) {
                return Err(ApiError::http(
                    400,
                    format!("approver_mode must be one of {}", py_list(&APPROVER_MODES)),
                ));
            }
            if s.approver_mode == "role" && !truthy(&s.role_slug) {
                return Err(ApiError::http(
                    400,
                    format!("Step '{}' needs a role", s.name),
                ));
            }
            if s.approver_mode == "snapshot_user" && !truthy(&s.snapshot_user_key) {
                return Err(ApiError::http(
                    400,
                    format!("Step '{}' needs a snapshot user key", s.name),
                ));
            }
            if s.approver_mode == "user" && !truthy(&s.user_id) {
                return Err(ApiError::http(
                    400,
                    format!("Step '{}' needs a user", s.name),
                ));
            }
            let cond = s
                .condition
                .clone()
                .map(Value::Object)
                .unwrap_or(Value::Null);
            validate_condition(&cond).map_err(cond_err)?;
        }
    }

    // setattr(policy, key, value) for every key sent (steps aside), in the body's order.
    let mut sets: Vec<String> = vec![];
    let mut n = 1;
    let mut binds: Vec<Bind> = vec![];
    for key in &input.set {
        let b = match key.as_str() {
            "name" => Bind::Text(body.name.clone()),
            "description" => Bind::Text(body.description.clone()),
            "is_active" => Bind::Bool(body.is_active),
            "preconditions" => Bind::Json(
                body.preconditions
                    .as_ref()
                    .map(|l| Value::Array(l.iter().cloned().map(Value::Object).collect())),
            ),
            "signature_flow" => Bind::Text(body.signature_flow.clone()),
            "signer_role" => Bind::Text(body.signer_role.clone()),
            "sign_deadline_days" => Bind::Int(i32_of(body.sign_deadline_days)?),
            "max_extension_days" => Bind::Int(i32_of(body.max_extension_days)?),
            "qr_enabled" => Bind::Bool(body.qr_enabled),
            "view_scope" => Bind::Text(body.view_scope.clone()),
            "allow_download" => Bind::Bool(body.allow_download),
            "requires_login" => Bind::Bool(body.requires_login),
            "token_ttl_days" => Bind::Int(i32_of(body.token_ttl_days)?),
            "sla_hours" => Bind::Int(i32_of(body.sla_hours)?),
            "whatsapp_escalate_above_base" => Bind::Dec(body.whatsapp_escalate_above_base),
            _ => continue,
        };
        if sets.iter().any(|s| s.starts_with(&format!("{key} ="))) {
            continue;
        }
        n += 1;
        sets.push(format!("{key} = ${n}"));
        binds.push(b);
    }

    if let Some(steps) = steps {
        sqlx::query("DELETE FROM approval_policy_steps WHERE approval_policy_steps.policy_id = $1")
            .bind(policy.id)
            .execute(&mut *conn)
            .await?;
        let mut sorted: Vec<&StepBody> = steps.iter().collect();
        sorted.sort_by_key(|s| s.sequence);
        for s in sorted {
            let user_id = match s.user_id.as_deref() {
                Some(u) => Some(pg_uuid(u)?),
                None => None,
            };
            sqlx::query(
                "INSERT INTO approval_policy_steps (id, policy_id, sequence, name, approver_mode, role_slug, \
                 department, snapshot_user_key, user_id, condition, sla_hours, is_notify_only, allow_self_approval) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            )
            .bind(Uuid::new_v4())
            .bind(policy.id)
            .bind(i32_of(Some(s.sequence))?)
            .bind(&s.name)
            .bind(&s.approver_mode)
            .bind(&s.role_slug)
            .bind(&s.department)
            .bind(&s.snapshot_user_key)
            .bind(user_id)
            .bind(s.condition.clone().map(|c| sqlx::types::Json(Value::Object(c))))
            .bind(i32_of(s.sla_hours)?)
            .bind(s.is_notify_only)
            .bind(s.allow_self_approval)
            .execute(&mut *conn)
            .await?;
        }
    }
    n += 1;
    sets.push(format!("version = ${n}"));
    n += 1;
    sets.push(format!("updated_by_id = ${n}"));
    let sql = format!(
        "UPDATE approval_policies SET {}, updated_at = now() WHERE id = $1",
        sets.join(", ")
    );
    let mut q = sqlx::query(&sql).bind(policy.id);
    for b in binds {
        q = match b {
            Bind::Text(v) => q.bind(v),
            Bind::Bool(v) => q.bind(v),
            Bind::Int(v) => q.bind(v),
            Bind::Dec(v) => q.bind(v),
            Bind::Json(v) => q.bind(v.map(sqlx::types::Json)),
        };
    }
    q.bind(policy.version + 1)
        .bind(user.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let fresh: ApprovalPolicy = sqlx::query_as("SELECT * FROM approval_policies WHERE id = $1")
        .bind(policy.id)
        .fetch_one(&mut *conn)
        .await?;
    let steps = steps_for(&mut conn, &[fresh.id]).await?;
    Ok(resp::json(policy_dict(&fresh, &steps)))
}

enum Bind {
    Text(Option<String>),
    Bool(Option<bool>),
    Int(Option<i32>),
    Dec(Option<Decimal>),
    Json(Option<Value>),
}

/// `POST /policies/{x}`: Starlette finds no POST at `/policies/{policy_id}` and falls through to
/// `/{request_id}/{action}` with request_id "policies".
pub async fn policies_fallthrough(
    InternalUser(user): InternalUser,
    db: Db,
    ClientIp(ip): ClientIp,
    Path(action): Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    let input = granary_core::validation::body_from_bytes(&headers, &body)?;
    match action.as_str() {
        "approve" | "reject" => {
            let b: DecisionBody = validate_body(input)?;
            let decision = if action == "approve" {
                "approved"
            } else {
                "rejected"
            };
            decide(user, db, ip, &headers, "policies".into(), b, decision).await
        }
        "cancel" => {
            let b: CancelBody = validate_body(input)?;
            cancel(user, db, "policies".into(), b).await
        }
        _ => {
            let mut err = ApiError::http_default(405);
            if let ApiError::Http { headers, .. } = &mut err {
                headers.push((
                    axum::http::header::ALLOW,
                    axum::http::HeaderValue::from_static("PATCH"),
                ));
            }
            Err(err)
        }
    }
}

fn validate_body<T: Validate>(input: granary_core::validation::BodyInput) -> ApiResult<T> {
    use granary_core::validation::{BodyInput, Loc};
    let mut cx = Cx::new();
    let out = cx.at(Loc::Key("body".into()), |cx| match &input {
        BodyInput::Missing => {
            cx.err(granary_core::validation::MSG_MISSING);
            None
        }
        BodyInput::Value(v) => T::validate(v, cx),
    });
    match out {
        Some(v) if cx.errors.is_empty() => Ok(v),
        _ => Err(ApiError::RequestValidation(cx.errors)),
    }
}

fn allowance_dict(a: &DiscountAllowance) -> Value {
    json!({"id": a.id.to_string(), "kind": a.kind, "label": a.label,
           "min_qty": resp::dec_float_opt(a.min_qty.as_ref()),
           "max_qty": resp::dec_float_opt(a.max_qty.as_ref()),
           "max_discount_pct": resp::dec_float(&a.max_discount_pct), "notify_owner": a.notify_owner,
           "is_active": a.is_active, "sop_reference": a.sop_reference})
}

/// `GET /discount-allowances`.
pub async fn list_allowances(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let rows: Vec<DiscountAllowance> = sqlx::query_as(
        "SELECT * FROM discount_allowances ORDER BY discount_allowances.kind, discount_allowances.min_qty",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(allowance_dict).collect(),
    )))
}

granary_core::py_model! {
    /// `class AllowanceBody(BaseModel)`.
    pub struct AllowanceBody {
        pub kind: String,
        pub label: String => v::str_c(v::StrC::new().min_length(1).max_length(255)),
        pub min_qty: Option<Decimal> = None => v::nullable(v::decimal_c(v::DecC::new().ge("0"))),
        pub max_qty: Option<Decimal> = None => v::nullable(v::decimal_c(v::DecC::new().ge("0"))),
        pub max_discount_pct: Decimal => v::decimal_c(v::DecC::new().gt("0").le("100")),
        pub notify_owner: bool = false,
        pub is_active: bool = true,
    }
}

/// `PUT /discount-allowances`: replaces the whole table.
pub async fn replace_allowances(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<Vec<AllowanceBody>>,
) -> ApiResult<Response> {
    require_manage(&user)?;
    for a in &body {
        if !ALLOWANCE_KINDS.contains(&a.kind.as_str()) {
            return Err(ApiError::http(
                400,
                format!("kind must be one of {}", py_list(&ALLOWANCE_KINDS)),
            ));
        }
        if let (Some(min), Some(max)) = (a.min_qty, a.max_qty) {
            if max < min {
                return Err(ApiError::http(
                    400,
                    format!("'{}': max quantity is below min quantity", a.label),
                ));
            }
        }
    }
    let mut conn = db.conn().await?;
    sqlx::query("DELETE FROM discount_allowances")
        .execute(&mut *conn)
        .await?;
    for a in &body {
        sqlx::query(
            "INSERT INTO discount_allowances (id, kind, label, min_qty, max_qty, max_discount_pct, notify_owner, \
             is_active, sop_reference) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL)",
        )
        .bind(Uuid::new_v4())
        .bind(&a.kind)
        .bind(&a.label)
        .bind(a.min_qty)
        .bind(a.max_qty)
        .bind(a.max_discount_pct)
        .bind(a.notify_owner)
        .bind(a.is_active)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    let mut conn = db.conn().await?;
    let rows: Vec<DiscountAllowance> =
        sqlx::query_as("SELECT * FROM discount_allowances ORDER BY discount_allowances.kind")
            .fetch_all(&mut *conn)
            .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(allowance_dict).collect(),
    )))
}

// ─── Inbox and request actions ────────────────────────────────────────────────

granary_core::py_model! {
    pub struct ListQ {
        pub scope: String = "awaiting_me".to_string(),
        pub status: Option<String> = None,
        pub entity_type: Option<String> = None,
        pub limit: i64 = 100,
    }
}

/// Python `rows[:limit]`.
fn py_head<T>(mut rows: Vec<T>, limit: i64) -> Vec<T> {
    let len = rows.len() as i64;
    let end = if limit >= 0 {
        limit.min(len)
    } else {
        (len + limit).max(0)
    };
    rows.truncate(end as usize);
    rows
}

/// `GET ""`: the inbox (scope: awaiting_me | submitted_by_me | involved | all).
pub async fn list_requests(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<ListQ>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut sql = String::from("SELECT * FROM approval_requests");
    let mut wheres: Vec<String> = vec![];
    let mut n = 0;
    let entity_type = q.entity_type.clone().filter(|e| !e.is_empty());
    if entity_type.is_some() {
        n += 1;
        wheres.push(format!("approval_requests.entity_type = ${n}"));
    }
    let mut by_submitter = false;
    match q.scope.as_str() {
        "awaiting_me" => wheres.push("approval_requests.status = 'pending'".into()),
        "submitted_by_me" => {
            n += 1;
            wheres.push(format!("approval_requests.submitted_by_id = ${n}"));
            by_submitter = true;
        }
        "all" => require_manage(&user)?,
        "involved" => {}
        _ => {
            return Err(ApiError::http(
                400,
                "scope must be awaiting_me, submitted_by_me, involved or all",
            ))
        }
    }
    let status = q
        .status
        .clone()
        .filter(|s| !s.is_empty() && q.scope != "awaiting_me");
    if status.is_some() {
        n += 1;
        wheres.push(format!("approval_requests.status = ${n}"));
    }
    if !wheres.is_empty() {
        sql += &format!(" WHERE {}", wheres.join(" AND "));
    }
    let factor = if matches!(q.scope.as_str(), "awaiting_me" | "involved") {
        5
    } else {
        1
    };
    let fetch = q.limit.clamp(i64::MIN, 500).max(1) * factor;
    sql += &format!(
        " ORDER BY approval_requests.submitted_at DESC LIMIT ${}",
        n + 1
    );
    let mut query = sqlx::query_as::<_, ApprovalRequest>(&sql);
    if let Some(e) = &entity_type {
        query = query.bind(e);
    }
    if by_submitter {
        query = query.bind(user.id);
    }
    if let Some(s) = &status {
        query = query.bind(s);
    }
    let mut conn = db.conn().await?;
    let rows = query.bind(fetch).fetch_all(&mut *conn).await?;
    let rows: Vec<ApprovalRequest> = match q.scope.as_str() {
        "awaiting_me" => rows.into_iter().filter(|r| awaiting(&user, r)).collect(),
        "involved" => rows.into_iter().filter(|r| can_see(&user, r)).collect(),
        _ => rows,
    };
    let mut out = vec![];
    for r in py_head(rows, q.limit) {
        out.push(engine::serialize(&mut conn, &r, Some(&user)).await?);
    }
    Ok(resp::json(Value::Array(out)))
}

/// `_load_visible(db, request_id, user)`.
async fn load_visible(
    conn: &mut PgConnection,
    request_id: &str,
    user: &AuthUser,
) -> ApiResult<ApprovalRequest> {
    // A malformed id raises inside the try: "not found".
    let row: Option<ApprovalRequest> = match pg_uuid(request_id) {
        Ok(id) => sqlx::query_as("SELECT * FROM approval_requests WHERE approval_requests.id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .ok()
            .flatten(),
        Err(_) => None,
    };
    let Some(row) = row else {
        return Err(ApiError::http(404, "Approval request not found"));
    };
    if !can_see(user, &row) {
        return Err(ApiError::http(403, "You are not involved in this approval"));
    }
    Ok(row)
}

/// `GET /{request_id}`.
pub async fn get_request(
    InternalUser(user): InternalUser,
    db: Db,
    Path(request_id): Path<String>,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    let row = load_visible(&mut conn, &request_id, &user).await?;
    Ok(resp::json(
        engine::serialize(&mut conn, &row, Some(&user)).await?,
    ))
}

/// `GET /submit`: Starlette's fall-through to `get_request("submit")`.
pub async fn get_request_submit(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    get_request(InternalUser(user), db, Path("submit".to_string())).await
}

granary_core::py_model! {
    /// `class DecisionBody(BaseModel)`.
    pub struct DecisionBody {
        pub comment: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(4000))),
    }
}

granary_core::py_model! {
    /// `class CancelBody(BaseModel)`.
    pub struct CancelBody {
        pub reason: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(1000))),
    }
}

async fn decide(
    user: AuthUser,
    db: Db,
    ip: Option<String>,
    headers: &axum::http::HeaderMap,
    request_id: String,
    body: DecisionBody,
    decision: &str,
) -> ApiResult<Response> {
    require_view(&user)?;
    let ua = user_agent(headers);
    let mut conn = db.conn().await?;
    load_visible(&mut conn, &request_id, &user).await?;
    let row = engine::act(
        &mut conn,
        &request_id,
        &user,
        decision,
        body.comment.as_deref(),
        ip.as_deref(),
        ua.as_deref(),
    )
    .await?;
    drop(conn);
    commit_and_dispatch(&db).await?;
    let mut conn = db.conn().await?;
    Ok(resp::json(
        engine::serialize(&mut conn, &row, Some(&user)).await?,
    ))
}

/// `POST /{request_id}/approve`.
pub async fn approve_request(
    InternalUser(user): InternalUser,
    db: Db,
    ClientIp(ip): ClientIp,
    Path(request_id): Path<String>,
    headers: axum::http::HeaderMap,
    PyJson(body): PyJson<DecisionBody>,
) -> ApiResult<Response> {
    decide(user, db, ip, &headers, request_id, body, "approved").await
}

/// `POST /{request_id}/reject`.
pub async fn reject_request(
    InternalUser(user): InternalUser,
    db: Db,
    ClientIp(ip): ClientIp,
    Path(request_id): Path<String>,
    headers: axum::http::HeaderMap,
    PyJson(body): PyJson<DecisionBody>,
) -> ApiResult<Response> {
    decide(user, db, ip, &headers, request_id, body, "rejected").await
}

async fn cancel(
    user: AuthUser,
    db: Db,
    request_id: String,
    body: CancelBody,
) -> ApiResult<Response> {
    require_view(&user)?;
    let mut conn = db.conn().await?;
    load_visible(&mut conn, &request_id, &user).await?;
    let row = engine::cancel(&mut conn, &request_id, &user, body.reason.as_deref()).await?;
    drop(conn);
    commit_and_dispatch(&db).await?;
    let mut conn = db.conn().await?;
    Ok(resp::json(
        engine::serialize(&mut conn, &row, Some(&user)).await?,
    ))
}

/// `POST /{request_id}/cancel`: the submitter withdraws (or a policy manager cancels) a pending request.
pub async fn cancel_request(
    InternalUser(user): InternalUser,
    db: Db,
    Path(request_id): Path<String>,
    PyJson(body): PyJson<CancelBody>,
) -> ApiResult<Response> {
    cancel(user, db, request_id, body).await
}

/// `str(value)` of a JSON value.
fn py_str(v: &Value) -> String {
    granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(v))
}
