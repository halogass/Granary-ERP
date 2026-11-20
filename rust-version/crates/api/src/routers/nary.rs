//! backend/app/routers/nary.py, mounted at `/api/v1/nary` (backend/app/main.py).
//!
//! Nary's ERP surface: status and pause, the approval queue, runs, memory, autonomy, SOP roles, staff
//! WhatsApp chats, the intake pages, the WhatsApp history import and the Owner topics. Permissions:
//! nary:view to look, nary:approve to decide cards, nary:manage for everything that changes how Nary
//! behaves. A user always manages their own staff chats.
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users. Permissions are
//! checked in the body (`_require`), after the body was validated, as in Python.

use std::collections::HashMap;

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::settings_store::{require_int, require_json};
use granary_core::validation::{DateTimeIn, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::crm::Contact;
use granary_db::rows::nary::{
    AgentPendingAction, AgentRun, AgentStep, DomainEvent, NaryAutonomy, NaryExpectedArrival,
    NaryIdentityLink, NaryMemory, NaryOwnerTopic, NaryTriageVerdict, StaffWhatsAppLink, WaChat,
    WaImportChat,
};
use granary_services::ai::base::AiError;
use granary_services::approvals::notify::dispatch_outbox;
use granary_services::nary::{
    actions as action_registry, autonomy, channels, chat_links, executor, governance, history,
    identity, identity_match, memory, owner_channel, persona, triage, wa_ingest,
};

use super::at;
use crate::extract::{parse_uuid, PyForm};
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/nary";

/// `_require(user, code)`: 403 "Permission required: {code}" unless the user holds it.
pub fn _require(user: &AuthUser, code: &str) -> Result<(), ApiError> {
    if !_holds(user, code) {
        return Err(ApiError::http(403, format!("Permission required: {code}")));
    }
    Ok(())
}

/// `_holds(user, code)`: superadmins hold everything.
pub fn _holds(user: &AuthUser, code: &str) -> bool {
    user.is_superadmin || user.perms.contains(code)
}

/// `_iso(v)`: `v.isoformat() if v else None` for a timestamptz.
pub fn _iso(v: Option<&DateTime<Utc>>) -> Value {
    resp::iso_opt(v)
}

/// The routes of `router` in nary.py, in the Python file's order.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/events/given-up"), get(given_up_events))
        .route(
            &at(PREFIX, "/events/dismiss-given-up"),
            post(dismiss_given_up),
        )
        .route(&at(PREFIX, "/status"), get(status))
        .route(&at(PREFIX, "/pause"), post(pause))
        .route(&at(PREFIX, "/shadow"), post(shadow))
        .route(&at(PREFIX, "/briefs/{slot}/send"), post(send_brief_now))
        .route(&at(PREFIX, "/pending"), get(list_cards))
        .route(&at(PREFIX, "/pending/{card_id}/decide"), post(decide_card))
        // fix-longapply: one card, polled while it is still applying
        .route(&at(PREFIX, "/pending/{card_id}"), get(get_card))
        .route(&at(PREFIX, "/send"), post(send_now))
        .route(&at(PREFIX, "/actions/run"), post(run_action))
        .route(&at(PREFIX, "/pending/batch"), post(batch_apply))
        .route(&at(PREFIX, "/runs"), get(list_runs))
        .route(&at(PREFIX, "/runs/{run_id}"), get(get_run))
        .route(&at(PREFIX, "/memory"), get(list_memory).post(add_memory))
        .route(&at(PREFIX, "/memory/style/relearn"), post(relearn_style))
        .route(
            &at(PREFIX, "/memory/{entry_id}"),
            patch(edit_memory).delete(delete_memory),
        )
        .route(&at(PREFIX, "/autonomy"), get(get_autonomy))
        .route(
            &at(PREFIX, "/autonomy/{action}/promote"),
            post(promote_action),
        )
        .route(&at(PREFIX, "/autonomy/{action}"), put(set_action_level))
        .route(
            &at(PREFIX, "/autonomy-never-promote"),
            put(set_never_promote),
        )
        .route(&at(PREFIX, "/roles"), get(list_roles))
        .route(&at(PREFIX, "/roles/{sop_role}"), put(set_role))
        .route(
            &at(PREFIX, "/staff-chats"),
            get(list_staff_chats).post(add_staff_chat),
        )
        // GET /staff-chats/search is registered before DELETE /staff-chats/{link_id} in Python; a GET on
        // /staff-chats/<id> is a 405 on both stacks.
        .route(&at(PREFIX, "/staff-chats/search"), get(search_staff_chats))
        .route(
            &at(PREFIX, "/staff-chats/{link_id}/primary"),
            post(make_staff_chat_primary),
        )
        .route(
            &at(PREFIX, "/staff-chats/{link_id}"),
            delete(remove_staff_chat),
        )
        .route(&at(PREFIX, "/intake/verdicts"), get(list_verdicts))
        .route(
            &at(PREFIX, "/intake/verdicts/{verdict_id}/correct"),
            post(correct_verdict),
        )
        .route(&at(PREFIX, "/intake/held"), get(list_held))
        .route(
            &at(PREFIX, "/intake/held/{chat_id}"),
            post(decide_held_chat),
        )
        .route(&at(PREFIX, "/intake/arrivals"), get(list_arrivals))
        .route(&at(PREFIX, "/intake/links"), get(list_links))
        .route(&at(PREFIX, "/intake/links/{link_id}/undo"), post(undo_link))
        .route(
            &at(PREFIX, "/exclusions"),
            get(list_exclusions).post(add_exclusion),
        )
        .route(
            &at(PREFIX, "/exclusions/{chat_id}/include"),
            post(include_again),
        )
        .route(&at(PREFIX, "/channels/health"), get(channels_health))
        .route(&at(PREFIX, "/debrief"), post(transcribe_debrief))
        .route(&at(PREFIX, "/history"), get(history_overview))
        .route(&at(PREFIX, "/history/refresh"), post(history_refresh))
        .route(&at(PREFIX, "/history/decisions"), post(history_decide))
        .route(&at(PREFIX, "/history/run"), post(history_run))
        .route(
            &at(PREFIX, "/history/chats/{chat_id}/import-now"),
            post(history_import_now),
        )
        .route(
            &at(PREFIX, "/chats/{chat_id}/link"),
            post(link_chat_to_contact),
        )
        .route(&at(PREFIX, "/history/create-records"), post(history_create))
        .route(&at(PREFIX, "/owner/topics"), get(owner_topics))
        .route(
            &at(PREFIX, "/owner/topics/{topic_id}/answer"),
            post(answer_owner_topic),
        )
        .route(
            &at(PREFIX, "/owner/topics/{topic_id}/close"),
            post(close_owner_topic),
        )
        // 3 Nov 2026 (Stream A): the work queue (Owner and system admins)
        .route(&at(PREFIX, "/work-queue"), get(work_queue))
}

/// `GET /work-queue?kinds=a,b&urgency=urgent&limit=50` (3 Nov 2026): what is waiting and the next tool for each
/// item (nary::work_queue), for the Owner (Nary's approver), a superadmin or a system admin.
pub async fn work_queue(
    CurrentUser(user): CurrentUser,
    db: Db,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    use granary_services::nary::work_queue as wq;
    let mut conn = db.conn().await?;
    if !wq::may_read(&mut conn, &user).await? {
        return Err(ApiError::http(
            403,
            "The work queue is for the Owner and system admins",
        ));
    }
    let args = json!({
        "kinds": q.get("kinds").cloned().unwrap_or_default(),
        "urgency": q.get("urgency").cloned().unwrap_or_default(),
        "limit": q.get("limit").cloned().unwrap_or_default(),
    });
    let filter = wq::Filter::from_args(&args).map_err(|e| ApiError::http(422, e))?;
    let out = wq::list(&mut conn, &filter).await?;
    Ok(resp::json(out))
}

// ─── helpers ──────────────────────────────────────────────────────────────────

fn s<T: ToString>(v: Option<T>) -> Value {
    match v {
        Some(x) => Value::String(x.to_string()),
        None => Value::Null,
    }
}

/// Python's `str[:n]` (characters, not bytes).
fn cut(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// `(d or {}).get("label")` with Python truthiness of the outer value.
fn label_of(v: Option<&Value>) -> Value {
    match v {
        Some(Value::Object(m)) => m.get("label").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// `a or b`.
fn or(a: Value, b: Value) -> Value {
    if action_registry::py_truthy(&a) {
        a
    } else {
        b
    }
}

/// `{**d, key: value}`: a key already there keeps its position.
fn with(mut v: Value, key: &str, value: Value) -> Value {
    if let Value::Object(m) = &mut v {
        m.insert(key.to_string(), value);
    }
    v
}

/// `_max_attempts(db)`.
async fn _max_attempts(db: &mut PgConnection) -> ApiResult<i64> {
    Ok(require_int(db, "nary.events.max_attempts", "Nary & AI → Engine").await?)
}

async fn count(db: &mut PgConnection, sql: &str, binds: &[Value]) -> ApiResult<i64> {
    let mut q = sqlx::query_scalar::<_, i64>(sql);
    for b in binds {
        q = match b {
            Value::String(t) => q.bind(t.clone()),
            Value::Number(n) => q.bind(n.as_i64().unwrap_or_default()),
            _ => q.bind(Option::<String>::None),
        };
    }
    Ok(q.fetch_one(&mut *db).await?)
}

/// `isinstance(e, HTTPException)` raised by a call that yields an [`AiError`]: its detail.
fn ai_http_detail(e: &AiError) -> Option<Value> {
    match e {
        AiError::Ai { detail, .. } => Some(Value::String(detail.clone())),
        AiError::Api(ApiError::Http { detail, .. }) => Some(detail.clone()),
        _ => None,
    }
}

fn http_detail_value(e: &ApiError) -> Option<Value> {
    match e {
        ApiError::Http { detail, .. } => Some(detail.clone()),
        _ => None,
    }
}

// ─── Status ───────────────────────────────────────────────────────────────────

/// `GET /events/given-up`: events Nary stopped retrying, with their last error.
pub async fn given_up_events(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let max = _max_attempts(&mut conn).await?;
    let rows: Vec<DomainEvent> = sqlx::query_as(
        "SELECT * FROM domain_events WHERE dispatched_at IS NULL AND attempts >= $1 \
         ORDER BY occurred_at LIMIT 50",
    )
    .bind(max as i32)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|e| {
            json!({"id": e.id.to_string(), "type": e.type_, "occurred_at": resp::iso(&e.occurred_at),
                   "attempts": e.attempts, "error": cut(e.last_error.as_deref().unwrap_or(""), 300)})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /events/dismiss-given-up`.
pub async fn dismiss_given_up(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let max = _max_attempts(&mut conn).await?;
    // A Core UPDATE: the updated_at onupdate applies too.
    let res = sqlx::query(
        "UPDATE domain_events SET dispatched_at = $1, updated_at = now() \
         WHERE dispatched_at IS NULL AND attempts >= $2",
    )
    .bind(Utc::now())
    .bind(max as i32)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"dismissed": res.rows_affected()})))
}

/// `GET /status`.
pub async fn status(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let since = Utc::now() - chrono::TimeDelta::days(1);
    let cnt = "SELECT count(agent_pending_actions.id) FROM agent_pending_actions WHERE status = $1";
    let pending_cards = count(&mut conn, cnt, &[json!("pending")]).await?;
    let shadow_drafts = count(&mut conn, cnt, &[json!("shadow")]).await?;
    let scheduled = count(&mut conn, cnt, &[json!("scheduled")]).await?;
    let runs_24h: i64 =
        sqlx::query_scalar("SELECT count(agent_runs.id) FROM agent_runs WHERE started_at >= $1")
            .bind(since)
            .fetch_one(&mut *conn)
            .await?;
    let failed_24h: i64 = sqlx::query_scalar(
        "SELECT count(agent_runs.id) FROM agent_runs WHERE started_at >= $1 AND status = 'failed'",
    )
    .bind(since)
    .fetch_one(&mut *conn)
    .await?;
    let max = _max_attempts(&mut conn).await?;
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(domain_events.id) FROM domain_events WHERE dispatched_at IS NULL AND attempts < $1",
    )
    .bind(max as i32)
    .fetch_one(&mut *conn)
    .await?;
    let max = _max_attempts(&mut conn).await?;
    let given_up: i64 = sqlx::query_scalar(
        "SELECT count(domain_events.id) FROM domain_events WHERE dispatched_at IS NULL AND attempts >= $1",
    )
    .bind(max as i32)
    .fetch_one(&mut *conn)
    .await?;
    use granary_services::ai::registry::{budget_state, get_ai_config};
    let budget = async {
        let cfg = get_ai_config(&mut conn).await?;
        budget_state(&mut conn, &cfg).await
    }
    .await;
    let budget = match budget {
        Ok(st) => json!({"cap": resp::dec_float(&st.cap), "spent": resp::dec_float(&st.spent),
                         "pct": resp::dec_float(&st.pct()), "level": st.level()}),
        Err(e) => match ai_http_detail(&e) {
            Some(d) => json!({"error": d}),
            None => return Err(e.into()),
        },
    };
    use granary_services::nary::rhythm::{brief_slots, is_working_time};
    let wt = async {
        let working = is_working_time(&mut conn, None).await?;
        let slots = brief_slots(&mut conn).await?;
        Ok::<_, ApiError>((working, slots))
    }
    .await;
    let (working, slots) = match wt {
        Ok((w, sl)) => {
            let mut m = Map::new();
            for (k, v) in sl {
                m.insert(k, json!(v.format("%H:%M").to_string()));
            }
            (json!(w), Value::Object(m))
        }
        Err(e) => match http_detail_value(&e) {
            Some(d) => (Value::Null, json!({"error": d})),
            None => return Err(e),
        },
    };
    let paused = governance::is_paused(&mut conn).await?;
    let shadow_mode = governance::shadow_mode(&mut conn).await?;
    Ok(resp::json(json!({
        "paused": paused, "shadow_mode": shadow_mode,
        "customer_sending_enabled": action_registry::CUSTOMER_SENDING_ENABLED, "working_time": working,
        "brief_times": slots, "budget": budget,
        "pending_cards": pending_cards, "shadow_drafts": shadow_drafts, "scheduled_messages": scheduled,
        "runs_24h": runs_24h, "failed_runs_24h": failed_24h, "events_queued": queued,
        "events_given_up": given_up,
    })))
}

granary_core::py_model! {
    /// `class Flag(BaseModel)`.
    pub struct Flag {
        pub on: bool,
    }
}

/// `POST /pause`.
pub async fn pause(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<Flag>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    governance::set_flag(&mut conn, "nary.paused", body.on, Some(&user)).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"paused": body.on})))
}

/// `POST /shadow`.
pub async fn shadow(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<Flag>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    governance::set_flag(&mut conn, "nary.shadow_mode", body.on, Some(&user)).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"shadow_mode": body.on})))
}

/// `POST /briefs/{slot}/send`: send a brief now.
pub async fn send_brief_now(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(slot): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    if !matches!(slot.as_str(), "morning" | "midday" | "wrapup") {
        return Err(ApiError::http(
            400,
            "slot must be morning, midday or wrapup",
        ));
    }
    let mut conn = db.conn().await?;
    let out = granary_services::nary::briefs::send_brief(&mut conn, &slot).await?;
    Ok(resp::json(out))
}

// ─── Approval queue ───────────────────────────────────────────────────────────

/// `_card(r)`.
pub fn _card(r: &AgentPendingAction) -> Value {
    let label = action_registry::REGISTRY
        .read()
        .unwrap()
        .iter()
        .find(|a| a.name == r.action)
        .map(|a| a.label.clone())
        .unwrap_or_else(|| r.action.clone());
    json!({
        "id": r.id.to_string(), "action": r.action, "label": label, "origin": r.origin,
        "status": r.status, "level": r.level, "customer_facing": r.customer_facing,
        "payload": r.payload, "edited_payload": r.edited_payload, "rationale": r.rationale, "sop_ref": r.sop_ref,
        "sources": r.sources.clone().filter(action_registry::py_truthy).unwrap_or_else(|| json!([])),
        "diff": r.diff, "price": r.price,
        "entity_type": r.entity_type, "entity_id": s(r.entity_id),
        "run_id": s(r.run_id), "created_at": resp::iso(&r.created_at),
        "decided_at": _iso(r.decided_at.as_ref()), "decided_by_id": s(r.decided_by_id),
        "reject_reason": r.reject_reason, "edit_ratio": resp::dec_float_opt(r.edit_ratio.as_ref()),
        "result": r.result, "error": r.error,
        "actual_text": r.actual_text, "diff_ratio": resp::dec_float_opt(r.diff_ratio.as_ref()),
    })
}

granary_core::py_model! {
    /// Query of `list_cards`.
    pub struct ListCardsQ {
        pub status: String = "pending".to_string(),
        pub entity_type: Option<String> = None,
        pub entity_id: Option<String> = None,
        pub limit: i64 = 50,
    }
}

/// `GET /pending`.
pub async fn list_cards(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListCardsQ>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut sql = String::from("SELECT * FROM agent_pending_actions WHERE true");
    let mut n = 0;
    if q.status != "all" {
        n += 1;
        sql.push_str(&format!(" AND status = ${n}"));
    }
    let et = q.entity_type.clone().filter(|v| !v.is_empty());
    if et.is_some() {
        n += 1;
        sql.push_str(&format!(" AND entity_type = ${n}"));
    }
    let eid = match q.entity_id.as_deref().filter(|v| !v.is_empty()) {
        Some(v) => Some(parse_uuid(v, "entity_id")?),
        None => None,
    };
    if eid.is_some() {
        n += 1;
        sql.push_str(&format!(" AND entity_id = ${n}"));
    }
    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT ${}", n + 1));
    let mut query = sqlx::query_as::<_, AgentPendingAction>(&sql);
    if q.status != "all" {
        query = query.bind(q.status.clone());
    }
    if let Some(v) = et {
        query = query.bind(v);
    }
    if let Some(v) = eid {
        query = query.bind(v);
    }
    let mut conn = db.conn().await?;
    let rows = query.bind(q.limit.min(200)).fetch_all(&mut *conn).await?;
    Ok(resp::json(Value::Array(rows.iter().map(_card).collect())))
}

granary_core::py_model! {
    /// `class Decision(BaseModel)`.
    pub struct Decision {
        /// apply | reject
        pub decision: String,
        pub edited_payload: Option<Map<String, Value>> = None,
        pub reason: Option<String> = None,
    }
}

/// `POST /pending/{card_id}/decide`.
pub async fn decide_card(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(card_id): Path<String>,
    PyJson(body): PyJson<Decision>,
) -> ApiResult<Response> {
    _require(&user, "nary:approve")?;
    let mut conn = db.conn().await?;
    let cid = parse_uuid(&card_id, "card_id");
    let res = match cid {
        Ok(id) => {
            executor::decide(
                &mut conn,
                id,
                &user,
                &body.decision,
                body.edited_payload.clone().map(Value::Object),
                body.reason.as_deref(),
            )
            .await
        }
        Err(e) => Err(e),
    };
    match res {
        Ok(row) => {
            drop(conn);
            db.commit().await?;
            dispatch_outbox(); // what applying the card notified (WhatsApp/email), after its commit
            if row.status == "applying" {
                return Ok(still_working(&row));
            }
            Ok(resp::json(_card(&row)))
        }
        Err(e) => {
            let st = governance::http_status(&e);
            if matches!(st, Some(403) | Some(400)) && body.decision == "apply" {
                let id = parse_uuid(&card_id, "card_id")?;
                let row: Option<AgentPendingAction> =
                    sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&mut *conn)
                        .await?;
                if let Some(row) = row.filter(|r| r.status == "failed") {
                    let detail = http_detail_value(&e).unwrap_or(Value::Null);
                    return Ok(resp::json(with(_card(&row), "detail", detail)));
                }
            }
            Err(e)
        }
    }
}

/// fix-longapply: 202 for a card still being applied after `nary.apply_wait_seconds` (it finishes on its
/// own; poll GET /pending/{card_id}).
pub fn still_working(row: &AgentPendingAction) -> Response {
    let mut out = _card(row);
    out["card_id"] = json!(row.id.to_string());
    out["still_working"] = json!(true);
    out["label"] = json!(granary_services::nary::long_work::STILL_WORKING_MCP);
    resp::json_status(202, out)
}

/// fix-longapply: `GET /pending/{card_id}`: one card (nary:view, or the person who asked for it).
pub async fn get_card(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(card_id): Path<String>,
) -> ApiResult<Response> {
    let id = parse_uuid(&card_id, "card_id")?;
    let mut conn = db.conn().await?;
    let row: Option<AgentPendingAction> =
        sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(row) = row else {
        return Err(ApiError::http(404, "Card not found"));
    };
    if row.actor_id != Some(user.id) {
        _require(&user, "nary:view")?;
    }
    Ok(resp::json(_card(&row)))
}

granary_core::py_model! {
    /// `class SendNow(BaseModel)`.
    pub struct SendNow {
        /// email | whatsapp
        pub channel: String,
        pub text: String,
        pub chat_id: Option<String> = None,
        pub thread_id: Option<String> = None,
        pub contact_id: Option<String> = None,
        pub to: Option<String> = None,
        pub subject: Option<String> = None,
        /// email: [{type: quotation|invoice|sales_document, id}]
        pub attach: Option<Vec<Map<String, Value>>> = None,
        /// batch 2 T3 (A5): send it at this moment (company time or ISO with an offset) instead of now
        pub send_at: Option<String> = None,
        /// R&D 26 Oct: email copies, blind copies, and the CRM inbox it goes from
        pub cc: Option<Vec<String>> = None,
        pub bcc: Option<Vec<String>> = None,
        pub from_inbox: Option<String> = None,
    }
}

/// `{k: v for k, v in body.model_dump().items() if v not in (None, "")}`.
fn dump_send(body: &SendNow) -> Value {
    let mut m = Map::new();
    let fields: [(&str, Option<Value>); 11] = [
        ("channel", Some(json!(body.channel))),
        ("text", Some(json!(body.text))),
        ("chat_id", body.chat_id.clone().map(Value::String)),
        ("thread_id", body.thread_id.clone().map(Value::String)),
        ("contact_id", body.contact_id.clone().map(Value::String)),
        ("to", body.to.clone().map(Value::String)),
        ("subject", body.subject.clone().map(Value::String)),
        (
            "attach",
            body.attach
                .clone()
                .map(|a| Value::Array(a.into_iter().map(Value::Object).collect())),
        ),
        (
            "cc",
            body.cc.clone().filter(|c| !c.is_empty()).map(|c| json!(c)),
        ),
        (
            "bcc",
            body.bcc.clone().filter(|c| !c.is_empty()).map(|c| json!(c)),
        ),
        ("from_inbox", body.from_inbox.clone().map(Value::String)),
    ];
    for (k, v) in fields {
        if let Some(v) = v {
            if v != json!("") {
                m.insert(k.to_string(), v);
            }
        }
    }
    Value::Object(m)
}

/// `POST /send`: send a message as Nary on the Owner's instruction.
pub async fn send_now(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<SendNow>,
) -> ApiResult<Response> {
    _require(&user, "nary:approve")?;
    if !matches!(body.channel.as_str(), "email" | "whatsapp") {
        return Err(ApiError::http(400, "channel must be email or whatsapp"));
    }
    let payload = dump_send(&body);
    let mut conn = db.conn().await?;
    // batch 2 T3 (A5): a timed message is kept until its moment (executor::release_timed sends it)
    if let Some(raw) = body.send_at.as_deref().filter(|r| !r.trim().is_empty()) {
        let at = match granary_services::nary::scheduled_sends::check(
            &mut conn,
            raw,
            chrono::Utc::now(),
        )
        .await?
        {
            Ok(at) => at,
            Err(why) => return Err(ApiError::http(400, why)),
        };
        let row = executor::schedule_on_instruction(
            &mut conn,
            &user,
            &format!("{}.reply.send", body.channel),
            payload,
            &at,
        )
        .await?;
        drop(conn);
        db.commit().await?;
        let label = or(label_of(row.result.as_ref()), label_of(row.diff.as_ref()));
        return Ok(resp::json(with(_card(&row), "label", label)));
    }
    let row = executor::send_on_instruction(
        &mut conn,
        &user,
        &format!("{}.reply.send", body.channel),
        payload,
        None,
    )
    .await?;
    drop(conn);
    // Batch 2: commit, then hand over what applying it notified (WhatsApp/email).
    db.commit().await?;
    dispatch_outbox();
    let label = or(label_of(row.result.as_ref()), label_of(row.diff.as_ref()));
    Ok(resp::json(with(_card(&row), "label", label)))
}

granary_core::py_model! {
    /// `class RunAction(BaseModel)`.
    pub struct RunAction {
        pub action: String,
        pub payload: Map<String, Value>,
        /// optional, to the customer as Nary afterwards: {channel, text, chat_id | to, ...}
        pub message: Option<Map<String, Value>> = None,
    }
}

/// `POST /actions/run`: do what a chat card asked, as this person, on their confirm.
pub async fn run_action(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<RunAction>,
) -> ApiResult<Response> {
    // `if body.message:` an empty dict is falsy.
    let message = body.message.clone().filter(|m| !m.is_empty());
    if message.is_some() {
        _require(&user, "nary:approve")?;
    }
    let mut conn = db.conn().await?;
    let row = executor::run_on_instruction(
        &mut conn,
        &user,
        &body.action,
        Value::Object(body.payload.clone()),
        None,
    )
    .await?;
    // fix-longapply: still running after the wait: 202, the card finishes on its own (a message waits for it)
    if row.status == "applying" {
        drop(conn);
        db.commit().await?;
        dispatch_outbox();
        let mut resp_row = _card(&row);
        resp_row["card_id"] = json!(row.id.to_string());
        resp_row["still_working"] = json!(true);
        resp_row["label"] = json!(granary_services::nary::long_work::STILL_WORKING_MCP);
        resp_row["sent"] = Value::Null;
        resp_row["sent_error"] = if message.is_some() {
            json!("Not sent yet: the action is still running. Send the message once it is done.")
        } else {
            Value::Null
        };
        return Ok(resp::json_status(202, resp_row));
    }
    let mut label = or(label_of(row.result.as_ref()), label_of(row.diff.as_ref()));
    let mut out = with(_card(&row), "label", label.clone());
    let (mut sent, mut sent_error) = (Value::Null, Value::Null);
    if let Some(msg) = message {
        let fields: Map<String, Value> = msg
            .into_iter()
            .filter(|(_, v)| !v.is_null() && *v != json!(""))
            .collect();
        let channel = match fields.get("channel") {
            Some(v) => granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(v)),
            None => "None".to_string(),
        };
        let r = executor::send_on_instruction(
            &mut conn,
            &user,
            &format!("{channel}.reply.send"),
            Value::Object(fields),
            None,
        )
        .await;
        // `out["label"] += ...`: a missing label is a TypeError (500) in Python.
        let Value::String(l) = &label else {
            return Err(ApiError::internal(anyhow::anyhow!(
                "unsupported operand type(s) for +=: 'NoneType' and 'str'"
            )));
        };
        match r {
            Ok(done) => {
                sent = or(label_of(done.result.as_ref()), json!("sent"));
                label = json!(format!("{l}; the message was sent"));
            }
            Err(e) if governance::is_http_exception(&e) => {
                let detail = governance::http_detail(&e).unwrap_or_default();
                let why = detail
                    .strip_prefix("Not sent: ")
                    .unwrap_or(&detail)
                    .to_string();
                label = json!(format!("{l}; the message was NOT sent: {why}"));
                sent_error = json!(why);
            }
            Err(e) => return Err(e),
        }
    }
    out = with(out, "label", label);
    out = with(out, "sent", sent);
    out = with(out, "sent_error", sent_error);
    drop(conn);
    // Batch 2: commit, then hand over what applying it notified (WhatsApp/email).
    db.commit().await?;
    dispatch_outbox();
    Ok(resp::json(out))
}

granary_core::py_model! {
    /// `class Batch(BaseModel)`.
    pub struct Batch {
        pub ids: Vec<String>,
    }
}

/// `POST /pending/batch`.
pub async fn batch_apply(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<Batch>,
) -> ApiResult<Response> {
    _require(&user, "nary:approve")?;
    let ids = body
        .ids
        .iter()
        .map(|i| parse_uuid(i, "id"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut conn = db.conn().await?;
    let out = executor::decide_many(&mut conn, &ids, &user).await?;
    drop(conn);
    db.commit().await?;
    dispatch_outbox();
    Ok(resp::json(out))
}

// ─── Runs ─────────────────────────────────────────────────────────────────────

fn _run(r: &AgentRun) -> Value {
    json!({"id": r.id.to_string(), "trigger_type": r.trigger_type, "trigger_ref": r.trigger_ref, "job": r.job,
           "context": r.context, "sop_ref": r.sop_ref, "model": r.model, "status": r.status,
           "outcome": r.outcome, "error": r.error, "prompt_tokens": r.prompt_tokens,
           "completion_tokens": r.completion_tokens, "cache_read_tokens": r.cache_read_tokens,
           "est_cost": resp::dec_float(&r.est_cost), "started_at": resp::iso(&r.started_at),
           "finished_at": _iso(r.finished_at.as_ref())})
}

granary_core::py_model! {
    /// Query of `list_runs`.
    pub struct ListRunsQ {
        pub limit: i64 = 50,
        pub status: Option<String> = None,
    }
}

/// `GET /runs`.
pub async fn list_runs(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListRunsQ>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let rows: Vec<AgentRun> = match q.status.filter(|v| !v.is_empty()) {
        Some(st) => {
            sqlx::query_as(
                "SELECT * FROM agent_runs WHERE status = $1 ORDER BY started_at DESC LIMIT $2",
            )
            .bind(st)
            .bind(q.limit.min(200))
            .fetch_all(&mut *conn)
            .await?
        }
        None => {
            sqlx::query_as("SELECT * FROM agent_runs ORDER BY started_at DESC LIMIT $1")
                .bind(q.limit.min(200))
                .fetch_all(&mut *conn)
                .await?
        }
    };
    Ok(resp::json(Value::Array(rows.iter().map(_run).collect())))
}

/// `GET /runs/{run_id}`.
pub async fn get_run(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(run_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let id = parse_uuid(&run_id, "run_id")?;
    let mut conn = db.conn().await?;
    let run: Option<AgentRun> = sqlx::query_as("SELECT * FROM agent_runs WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(run) = run else {
        return Err(ApiError::http(404, "Run not found"));
    };
    let steps: Vec<AgentStep> =
        sqlx::query_as("SELECT * FROM agent_steps WHERE run_id = $1 ORDER BY seq")
            .bind(run.id)
            .fetch_all(&mut *conn)
            .await?;
    let cards: Vec<AgentPendingAction> =
        sqlx::query_as("SELECT * FROM agent_pending_actions WHERE run_id = $1")
            .bind(run.id)
            .fetch_all(&mut *conn)
            .await?;
    let mut out = _run(&run);
    out = with(
        out,
        "steps",
        Value::Array(
            steps
                .iter()
                .map(|s| {
                    json!({"seq": s.seq, "kind": s.kind, "name": s.name, "input": s.input, "output": s.output,
                           "prompt_tokens": s.prompt_tokens, "completion_tokens": s.completion_tokens})
                })
                .collect(),
        ),
    );
    out = with(
        out,
        "actions",
        Value::Array(cards.iter().map(_card).collect()),
    );
    Ok(resp::json(out))
}

// ─── Memory ───────────────────────────────────────────────────────────────────

fn _mem(m: &NaryMemory) -> Value {
    json!({"id": m.id.to_string(), "layer": m.layer, "subject_type": m.subject_type, "subject_id": m.subject_id,
           "title": m.title, "content": m.content, "source_type": m.source_type, "source_ref": m.source_ref,
           "trusted": m.trusted, "status": m.status, "authored_by_nary": m.authored_by_nary,
           "expires_at": _iso(m.expires_at.as_ref()), "created_at": resp::iso(&m.created_at)})
}

/// `_mem(m)` right after Python set `m.expires_at` from the request: the attribute is still the
/// request's datetime (its own offset, or naive), not the value read back.
fn _mem_with_input(m: &NaryMemory, expires_at: Option<&DateTimeIn>) -> Value {
    let out = _mem(m);
    match expires_at {
        Some(d) => with(out, "expires_at", json!(d.isoformat())),
        None => out,
    }
}

granary_core::py_model! {
    /// Query of `list_memory`.
    pub struct ListMemoryQ {
        pub layer: Option<String> = None,
        pub subject_type: Option<String> = None,
        pub subject_id: Option<String> = None,
        pub include_proposed: bool = true,
    }
}

/// `GET /memory`.
pub async fn list_memory(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListMemoryQ>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let layers: Vec<String> = match q.layer.filter(|l| !l.is_empty()) {
        Some(l) => vec![l],
        None => memory::LAYERS.iter().map(|s| s.to_string()).collect(),
    };
    let mut conn = db.conn().await?;
    let rows = memory::recall(
        &mut conn,
        memory::Recall {
            layers,
            subject_type: q.subject_type,
            subject_id: q.subject_id,
            trusted_only: false,
            include_proposed: q.include_proposed,
            limit: 200,
        },
    )
    .await?;
    Ok(resp::json(Value::Array(rows.iter().map(_mem).collect())))
}

/// `POST /memory/style/relearn`: re-learn the Owner's style now (proposed for approval).
pub async fn relearn_style(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let refresh = !persona::style_guide_text(&mut conn).await?.is_empty();
    let made = history::build_style_guide(&mut conn, refresh).await?;
    drop(conn);
    db.commit().await?;
    if !made {
        return Err(ApiError::http(
            400,
            "Nothing to learn from yet: no WhatsApp replies or sent emails from you were found.",
        ));
    }
    Ok(resp::json(json!({"status": "proposed"})))
}

granary_core::py_model! {
    /// `class MemoryIn(BaseModel)`.
    pub struct MemoryIn {
        pub layer: String,
        pub content: String,
        pub title: Option<String> = None,
        pub subject_type: Option<String> = None,
        pub subject_id: Option<String> = None,
        pub expires_at: Option<DateTimeIn> = None,
    }
}

/// `POST /memory` (201).
pub async fn add_memory(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<MemoryIn>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let m = memory::remember(
        &mut conn,
        memory::Remember {
            layer: body.layer.clone(),
            content: body.content.clone(),
            source_type: "owner".into(),
            subject_type: body.subject_type.clone(),
            subject_id: body.subject_id.clone(),
            title: body.title.clone(),
            source_ref: None,
            author: Some(user.id),
            by_nary: false,
            expires_at: body.expires_at.as_ref().map(DateTimeIn::to_utc),
        },
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        _mem_with_input(&m, body.expires_at.as_ref()),
    ))
}

granary_core::py_model! {
    /// `class MemoryPatch(BaseModel)`.
    pub struct MemoryPatch {
        pub content: Option<String> = None,
        pub title: Option<String> = None,
        pub expires_at: Option<DateTimeIn> = None,
        /// approve | reject (a proposed company fact)
        pub decision: Option<String> = None,
    }
}

/// `PATCH /memory/{entry_id}`.
pub async fn edit_memory(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(entry_id): Path<String>,
    PyJson(body): PyJson<MemoryPatch>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let eid = parse_uuid(&entry_id, "entry_id")?;
    let mut conn = db.conn().await?;
    let mut m = memory::edit(
        &mut conn,
        eid,
        user.id,
        body.content.as_deref(),
        body.title.as_deref(),
        body.expires_at.as_ref().map(DateTimeIn::to_utc),
    )
    .await?;
    if let Some(d) = body.decision.as_deref().filter(|d| !d.is_empty()) {
        m = memory::decide(&mut conn, eid, user.id, d).await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(_mem_with_input(&m, body.expires_at.as_ref())))
}

/// `DELETE /memory/{entry_id}` (204).
pub async fn delete_memory(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(entry_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let eid = parse_uuid(&entry_id, "entry_id")?;
    let mut conn = db.conn().await?;
    memory::forget(&mut conn, eid).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Autonomy ─────────────────────────────────────────────────────────────────

/// `GET /autonomy`.
pub async fn get_autonomy(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let levels: HashMap<String, i64> = autonomy::levels(&mut conn).await?.into_iter().collect();
    let stats: Vec<NaryAutonomy> = sqlx::query_as("SELECT * FROM nary_autonomy")
        .fetch_all(&mut *conn)
        .await?;
    let stats: HashMap<String, NaryAutonomy> =
        stats.into_iter().map(|r| (r.action.clone(), r)).collect();
    let never = autonomy::never_promote(&mut conn).await?;
    let mut actions: Vec<(String, String, bool)> = action_registry::REGISTRY
        .read()
        .unwrap()
        .iter()
        .map(|a| (a.name.clone(), a.label.clone(), a.customer_facing))
        .collect();
    actions.sort_by(|a, b| a.0.cmp(&b.0));
    let mut rows = vec![];
    for (name, label, customer_facing) in actions {
        let st = stats.get(&name);
        let level = autonomy::level_for(&mut conn, &name).await?;
        rows.push(json!({
            "action": name, "label": label, "customer_facing": customer_facing,
            "level": level, "configured_level": levels.get(&name),
            "never_promote": autonomy::_matches(&name, &never),
            "streak": st.map(|s| s.streak).unwrap_or(0),
            "candidate_since": st.map(|s| _iso(s.candidate_since.as_ref())).unwrap_or(Value::Null),
            "approvals": st.map(|s| s.approvals_total).unwrap_or(0),
            "edits": st.map(|s| s.edits_total).unwrap_or(0),
            "rejects": st.map(|s| s.rejects_total).unwrap_or(0),
            "last_change": st.and_then(|s| s.last_change.clone()),
        }));
    }
    let unlocked = require_json(
        &mut conn,
        "nary.autonomy.never_promote_unlocked",
        "Nary & AI → Autonomy & approvals",
    )
    .await?
        == Value::Bool(true);
    let promotion_count = require_int(
        &mut conn,
        "nary.autonomy.promotion_count",
        "Nary & AI → Autonomy & approvals",
    )
    .await?;
    let mut lv = Map::new();
    for (k, v) in autonomy::LEVELS {
        lv.insert(k.to_string(), json!(v));
    }
    Ok(resp::json(json!({
        "actions": rows, "never_promote": never, "never_promote_unlocked": unlocked,
        "promotion_count": promotion_count, "levels": lv,
    })))
}

/// `POST /autonomy/{action}/promote`.
pub async fn promote_action(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(action): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let level = autonomy::promote(&mut conn, &action, Some(&user)).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"action": action, "level": level})))
}

granary_core::py_model! {
    /// `class LevelIn(BaseModel)`.
    pub struct LevelIn {
        pub level: i64,
    }
}

/// `PUT /autonomy/{action}`.
pub async fn set_action_level(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(action): Path<String>,
    PyJson(body): PyJson<LevelIn>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    action_registry::get(&action)?;
    let mut conn = db.conn().await?;
    let level = autonomy::set_level(&mut conn, &action, body.level, Some(&user)).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"action": action, "level": level})))
}

granary_core::py_model! {
    /// `class NeverPromoteIn(BaseModel)`.
    pub struct NeverPromoteIn {
        pub items: Vec<String>,
    }
}

/// `PUT /autonomy-never-promote`.
pub async fn set_never_promote(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<NeverPromoteIn>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let items = autonomy::set_never_promote(&mut conn, &body.items, &user).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"never_promote": items})))
}

// ─── SOP roles ────────────────────────────────────────────────────────────────

/// `GET /roles`.
pub async fn list_roles(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let rows: Vec<(String, bool, Option<Uuid>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT r.sop_role, r.is_nary, r.user_id, users.name, r.note FROM nary_role_assignments r \
         LEFT OUTER JOIN users ON users.id = r.user_id ORDER BY r.sop_role",
    )
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(sop_role, is_nary, user_id, name, note)| {
            json!({"sop_role": sop_role, "is_nary": is_nary, "user_id": s(user_id),
                   "user_name": name, "note": note})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class RoleIn(BaseModel)`.
    pub struct RoleIn {
        pub is_nary: bool = false,
        pub user_id: Option<String> = None,
    }
}

/// Stage 8: the SOP role whose holder `roles::approver_ids` reads (it decides Nary's cards).
fn is_approver_role(sop_role: &str) -> bool {
    sop_role
        .trim()
        .eq_ignore_ascii_case(granary_services::nary::roles::APPROVER_ROLE)
}

/// `PUT /roles/{sop_role}`.
pub async fn set_role(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(sop_role): Path<String>,
    PyJson(body): PyJson<RoleIn>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let found: Option<(Uuid, String, bool, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, sop_role, is_nary, user_id FROM nary_role_assignments WHERE sop_role = $1",
    )
    .bind(&sop_role)
    .fetch_optional(&mut *conn)
    .await?;
    let has_user = body.user_id.as_deref().is_some_and(|u| !u.is_empty());
    if !body.is_nary && !has_user {
        return Err(ApiError::http(
            400,
            "Give a person, or assign the role to Nary",
        ));
    }
    let user_id = if body.is_nary {
        None
    } else {
        Some(parse_uuid(
            body.user_id.as_deref().unwrap_or(""),
            "user_id",
        )?)
    };
    // Stage 8: the approver role decides Nary's cards and Nary acts with its holder's permissions: only the
    // superadmin may hand it out (a nary:manage holder could otherwise make themselves the approver)
    if is_approver_role(&sop_role) && !user.is_superadmin {
        return Err(ApiError::http(
            403,
            "Only the superadmin can assign the Owner role.",
        ));
    }
    // Stage 8: a person who holds a role is an active staff account (never a portal user or a closed account)
    if let Some(uid) = user_id {
        let assignee: Option<(bool, String)> =
            sqlx::query_as("SELECT is_active, portal_type FROM users WHERE id = $1")
                .bind(uid)
                .fetch_optional(&mut *conn)
                .await?;
        let ok = assignee
            .is_some_and(|(active, portal)| active && (portal.is_empty() || portal == "internal"));
        if !ok {
            return Err(ApiError::http(
                400,
                "Give an active staff account (not a portal user or a deactivated account).",
            ));
        }
    }
    let before = found
        .as_ref()
        .map(|(_, _, was_nary, was_user)| json!({"is_nary": was_nary, "user_id": s(*was_user)}));
    let changed = !matches!(&found, Some((_, _, n, u)) if (*n, *u) == (body.is_nary, user_id));
    let role = match found {
        Some((_, role, was_nary, was_user)) if (was_nary, was_user) == (body.is_nary, user_id) => {
            role
        }
        Some((id, role, _, _)) => {
            sqlx::query(
                "UPDATE nary_role_assignments SET is_nary = $2, user_id = $3, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(body.is_nary)
            .bind(user_id)
            .execute(&mut *conn)
            .await?;
            role
        }
        None => {
            let role = cut(&sop_role, 40);
            sqlx::query(
                "INSERT INTO nary_role_assignments (sop_role, is_nary, user_id, id) VALUES ($1, $2, $3, $4)",
            )
            .bind(&role)
            .bind(body.is_nary)
            .bind(user_id)
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
            role
        }
    };
    // Stage 8: who changed which role, from what to what
    if changed {
        sqlx::query(
            "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
             old_values, new_values, origin) VALUES ($1, $2, $3, 'nary_role_assign', 'nary_role', $4, $4, \
             $5::json, $6::json, 'ui')",
        )
        .bind(Uuid::new_v4())
        .bind(user.id)
        .bind(&user.name)
        .bind(&role)
        .bind(before.map(|v| v.to_string()))
        .bind(json!({"is_nary": body.is_nary, "user_id": s(user_id)}).to_string())
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"sop_role": role, "is_nary": body.is_nary, "user_id": s(user_id)}),
    ))
}

// ─── Staff WhatsApp chats (profile) ───────────────────────────────────────────

fn _link(link: &StaffWhatsAppLink) -> Value {
    json!({"id": link.id.to_string(), "user_id": link.user_id.to_string(), "chat_id": link.chat_id,
           "phone": link.phone, "label": link.label, "is_primary": link.is_primary,
           "is_group": identity::is_group(link), "created_at": resp::iso(&link.created_at)})
}

fn _target_user(user: &AuthUser, user_id: Option<&str>) -> ApiResult<Uuid> {
    match user_id {
        None | Some("") => Ok(user.id),
        Some(u) if u == user.id.to_string() => Ok(user.id),
        Some(u) => {
            _require(user, "nary:manage")?;
            parse_uuid(u, "user_id")
        }
    }
}

granary_core::py_model! {
    /// Query of `list_staff_chats`.
    pub struct StaffChatsQ {
        pub user_id: Option<String> = None,
    }
}

/// `GET /staff-chats`.
pub async fn list_staff_chats(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<StaffChatsQ>,
) -> ApiResult<Response> {
    let target = _target_user(&user, q.user_id.as_deref())?;
    let mut conn = db.conn().await?;
    let rows: Vec<StaffWhatsAppLink> = sqlx::query_as(
        "SELECT * FROM staff_whatsapp_links WHERE user_id = $1 ORDER BY is_primary DESC, created_at",
    )
    .bind(target)
    .fetch_all(&mut *conn)
    .await?;
    let suggestions = identity::suggestions(&mut conn, target).await?;
    Ok(resp::json(json!({
        "links": rows.iter().map(_link).collect::<Vec<_>>(),
        "suggestions": suggestions,
    })))
}

granary_core::py_model! {
    /// `class StaffChatIn(BaseModel)`.
    pub struct StaffChatIn {
        /// a number or a WhatsApp chat id
        pub chat: String,
        pub label: Option<String> = None,
        /// someone else (nary:manage)
        pub user_id: Option<String> = None,
    }
}

/// `POST /staff-chats` (201).
pub async fn add_staff_chat(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<StaffChatIn>,
) -> ApiResult<Response> {
    let target = _target_user(&user, body.user_id.as_deref())?;
    let mut conn = db.conn().await?;
    let link = identity::link_chat(
        &mut conn,
        target,
        Some(&body.chat),
        body.label.as_deref(),
        Some(user.id),
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, _link(&link)))
}

granary_core::py_model! {
    /// Query of `search_staff_chats`.
    pub struct SearchChatsQ {
        pub q: String = String::new(),
        pub user_id: Option<String> = None,
    }
}

/// `GET /staff-chats/search`: find a chat to link.
pub async fn search_staff_chats(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<SearchChatsQ>,
) -> ApiResult<Response> {
    _target_user(&user, q.user_id.as_deref())?;
    let full = _holds(&user, "crm:view_whatsapp") || _holds(&user, "nary:manage");
    let mut conn = db.conn().await?;
    let results = identity::search_chats(&mut conn, Some(&q.q), full, 20).await?;
    Ok(resp::json(json!({"full": full, "results": results})))
}

async fn load_link(conn: &mut PgConnection, link_id: &str) -> ApiResult<StaffWhatsAppLink> {
    let id = parse_uuid(link_id, "link_id")?;
    let link: Option<StaffWhatsAppLink> =
        sqlx::query_as("SELECT * FROM staff_whatsapp_links WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    link.ok_or_else(|| ApiError::http(404, "Link not found"))
}

/// `POST /staff-chats/{link_id}/primary`.
pub async fn make_staff_chat_primary(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(link_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let mut link = load_link(&mut conn, &link_id).await?;
    _target_user(&user, Some(&link.user_id.to_string()))?;
    identity::set_primary(&mut conn, &mut link).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(_link(&link)))
}

/// `DELETE /staff-chats/{link_id}` (204).
pub async fn remove_staff_chat(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(link_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let link = load_link(&mut conn, &link_id).await?;
    _target_user(&user, Some(&link.user_id.to_string()))?;
    identity::unlink(&mut conn, &link).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Channels and intake (Phase 2) ────────────────────────────────────────────

fn _verdict(v: &NaryTriageVerdict) -> Value {
    json!({"id": v.id.to_string(), "subject_type": v.subject_type, "subject_ref": v.subject_ref,
           "lead_id": s(v.lead_id), "verdict": v.verdict, "language": v.language,
           "reason": v.reason, "excerpt": v.excerpt, "model": v.model, "cascaded": v.cascaded,
           "cascade_reason": v.cascade_reason, "corrected_to": v.corrected_to,
           "corrected_at": _iso(v.corrected_at.as_ref()), "created_at": resp::iso(&v.created_at)})
}

granary_core::py_model! {
    /// Query of `list_verdicts`.
    pub struct VerdictsQ {
        pub filtered: bool = true,
        pub limit: i64 = 100,
    }
}

/// `GET /intake/verdicts`: the Filtered list, or every verdict with filtered=false.
pub async fn list_verdicts(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<VerdictsQ>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let rows: Vec<NaryTriageVerdict> = if q.filtered {
        let mut kinds: Vec<String> = triage::FILTERED.iter().map(|s| s.to_string()).collect();
        kinds.push("personal".into());
        sqlx::query_as(
            "SELECT * FROM nary_triage_verdicts WHERE verdict = ANY($1) ORDER BY created_at DESC LIMIT $2",
        )
        .bind(kinds)
        .bind(q.limit.min(500))
        .fetch_all(&mut *conn)
        .await?
    } else {
        sqlx::query_as("SELECT * FROM nary_triage_verdicts ORDER BY created_at DESC LIMIT $1")
            .bind(q.limit.min(500))
            .fetch_all(&mut *conn)
            .await?
    };
    Ok(resp::json(Value::Array(
        rows.iter().map(_verdict).collect(),
    )))
}

granary_core::py_model! {
    /// `class Correction(BaseModel)`.
    pub struct Correction {
        pub verdict: String,
        pub note: Option<String> = None,
    }
}

/// `POST /intake/verdicts/{verdict_id}/correct`: the Owner's correction.
pub async fn correct_verdict(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(verdict_id): Path<String>,
    PyJson(body): PyJson<Correction>,
) -> ApiResult<Response> {
    _require(&user, "nary:approve")?;
    let vid = parse_uuid(&verdict_id, "verdict_id")?;
    let mut conn = db.conn().await?;
    let row = triage::correct(&mut conn, vid, &body.verdict, &user, body.note.as_deref()).await?;
    if row.subject_type == "wa_chat" {
        let chat = chat_by_id(&mut conn, &row.subject_ref).await?;
        if let Some(chat) =
            chat.filter(|c| c.status == "active" && c.triage.as_deref() != Some(&body.verdict))
        {
            sqlx::query("UPDATE wa_chats SET triage = $2, updated_at = now() WHERE id = $1")
                .bind(chat.id)
                .bind(&body.verdict)
                .execute(&mut *conn)
                .await?;
        }
    } else if row.subject_type == "email_thread" {
        granary_services::nary::email_intake::requeue(&mut conn, &row.subject_ref, &body.verdict)
            .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(_verdict(&row)))
}

/// `select(WaChat).where(WaChat.chat_id == chat_id)).scalar_one_or_none()` (chat_id is unique).
async fn chat_by_id(conn: &mut PgConnection, chat_id: &str) -> ApiResult<Option<WaChat>> {
    Ok(sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
        .bind(chat_id)
        .fetch_optional(&mut *conn)
        .await?)
}

fn _chat(c: &WaChat) -> Value {
    json!({"chat_id": c.chat_id, "phone": c.phone, "name": c.display_name, "status": c.status,
           "saved_on_phone": c.is_my_contact, "triage": c.triage, "language": c.language,
           "contact_id": s(c.contact_id), "lead_id": s(c.lead_id),
           "deal_id": s(c.deal_id), "human_until": _iso(c.human_until.as_ref()),
           "acked_at": _iso(c.acked_at.as_ref()), "last_inbound_at": _iso(c.last_inbound_at.as_ref()),
           "business_profile": c.business_profile, "exclude_reason": c.exclude_reason,
           "excluded_at": _iso(c.excluded_at.as_ref())})
}

/// `GET /intake/held`: chats waiting for the Owner's "business or personal?".
pub async fn list_held(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let rows: Vec<WaChat> =
        sqlx::query_as("SELECT * FROM wa_chats WHERE status = 'held' ORDER BY updated_at DESC")
            .fetch_all(&mut *conn)
            .await?;
    Ok(resp::json(Value::Array(rows.iter().map(_chat).collect())))
}

granary_core::py_model! {
    /// `class HeldDecision(BaseModel)`.
    pub struct HeldDecision {
        pub business: bool,
    }
}

/// `POST /intake/held/{chat_id}`.
pub async fn decide_held_chat(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(chat_id): Path<String>,
    PyJson(body): PyJson<HeldDecision>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let chat = channels::decide_held(&mut conn, &chat_id, body.business, Some(&user)).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(_chat(&chat)))
}

granary_core::py_model! {
    /// Query of `list_arrivals`.
    pub struct ArrivalsQ {
        pub status: Option<String> = None,
        pub limit: i64 = 100,
    }
}

/// `GET /intake/arrivals`.
pub async fn list_arrivals(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ArrivalsQ>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let rows: Vec<NaryExpectedArrival> = match q.status.filter(|v| !v.is_empty()) {
        Some(st) => {
            sqlx::query_as(
                "SELECT * FROM nary_expected_arrivals WHERE status = $1 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(st)
            .bind(q.limit.min(500))
            .fetch_all(&mut *conn)
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT * FROM nary_expected_arrivals ORDER BY created_at DESC LIMIT $1",
            )
            .bind(q.limit.min(500))
            .fetch_all(&mut *conn)
            .await?
        }
    };
    let out: Vec<Value> = rows
        .iter()
        .map(|a| {
            json!({"id": a.id.to_string(), "lead_id": s(a.lead_id), "name": a.name, "company": a.company,
                   "phone": a.phone, "email": a.email, "status": a.status, "due_at": resp::iso(&a.due_at),
                   "chat_id": a.chat_id, "arrived_at": _iso(a.arrived_at.as_ref()),
                   "created_at": resp::iso(&a.created_at)})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

fn _identity(link: &NaryIdentityLink) -> Value {
    let signals: Vec<Value> = match &link.signals {
        Value::Array(a) => a.clone(),
        _ => vec![],
    };
    let is_target = |s: &Value| s.get("signal") == Some(&json!("target"));
    let target = signals
        .iter()
        .find(|s| is_target(s))
        .and_then(|s| s.get("detail").cloned())
        .unwrap_or(Value::Null);
    let rest: Vec<Value> = signals.iter().filter(|s| !is_target(s)).cloned().collect();
    json!({"id": link.id.to_string(), "chat_id": link.subject_ref, "tier": link.tier, "score": link.score,
           "status": link.status, "cascaded": link.cascaded, "review": link.review, "target": target,
           "signals": rest, "contact_id": s(link.contact_id), "lead_id": s(link.lead_id),
           "created_at": resp::iso(&link.created_at)})
}

granary_core::py_model! {
    /// Query of `list_links`.
    pub struct LinksQ {
        pub limit: i64 = 100,
    }
}

/// `GET /intake/links`.
pub async fn list_links(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<LinksQ>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let rows: Vec<NaryIdentityLink> = sqlx::query_as(
        "SELECT * FROM nary_identity_links WHERE status IN ('linked', 'pending', 'undone') \
         ORDER BY created_at DESC LIMIT $1",
    )
    .bind(q.limit.min(500))
    .fetch_all(&mut *conn)
    .await?;
    Ok(resp::json(Value::Array(
        rows.iter().map(_identity).collect(),
    )))
}

/// `POST /intake/links/{link_id}/undo`.
pub async fn undo_link(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(link_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:approve")?;
    let id = parse_uuid(&link_id, "link_id")?;
    let mut conn = db.conn().await?;
    let link = identity_match::undo(&mut conn, id, user.id).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(_identity(&link)))
}

/// `GET /exclusions`: personal chats.
pub async fn list_exclusions(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let rows: Vec<WaChat> = sqlx::query_as(
        "SELECT * FROM wa_chats WHERE status = 'excluded' ORDER BY excluded_at DESC",
    )
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|c| {
            json!({"chat_id": c.chat_id, "phone": c.phone, "reason": c.exclude_reason,
                   "excluded_at": _iso(c.excluded_at.as_ref())})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class ExclusionIn(BaseModel)`.
    pub struct ExclusionIn {
        /// a chat id or a phone number
        pub chat: String,
    }
}

/// `POST /exclusions` (201): mark a chat personal by hand.
pub async fn add_exclusion(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ExclusionIn>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let chat_id = identity::chat_id_for(Some(&body.chat))?;
    let mut conn = db.conn().await?;
    let chat = wa_ingest::exclude_chat(&mut conn, &chat_id, Some(&user), "manual").await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"chat_id": chat.chat_id, "phone": chat.phone, "reason": chat.exclude_reason}),
    ))
}

/// `POST /exclusions/{chat_id}/include`: undo "personal".
pub async fn include_again(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(chat_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let chat = chat_by_id(&mut conn, &chat_id).await?;
    let Some(chat) = chat.filter(|c| c.status == "excluded") else {
        return Err(ApiError::http(404, "That chat is not marked personal"));
    };
    // The Owner called it business: never held again.
    sqlx::query(
        "UPDATE wa_chats SET status = 'active', exclude_reason = NULL, excluded_at = NULL, \
         excluded_by_id = NULL, released_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(chat.id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    let row: Option<WaImportChat> =
        sqlx::query_as("SELECT * FROM wa_import_chats WHERE chat_id = $1")
            .bind(&chat_id)
            .fetch_optional(&mut *conn)
            .await?;
    let mut reimport = false;
    if let Some(row) = row.filter(|r| r.decision == "exclude") {
        sqlx::query(
            "UPDATE wa_import_chats SET decision = 'include', decided_by_id = $2, decided_at = $3, \
             import_status = 'waiting', read_status = 'waiting', error = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(row.id)
        .bind(user.id)
        .bind(Utc::now())
        .execute(&mut *conn)
        .await?;
        reimport = true;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"chat_id": chat.chat_id, "status": "active", "history_reimport": reimport}),
    ))
}

/// `GET /channels/health`.
pub async fn channels_health(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let owed: i64 = sqlx::query_scalar(
        "SELECT count(wa_chats.id) FROM wa_chats WHERE ack_pending_since IS NOT NULL",
    )
    .fetch_one(&mut *conn)
    .await?;
    let held: i64 =
        sqlx::query_scalar("SELECT count(wa_chats.id) FROM wa_chats WHERE status = 'held'")
            .fetch_one(&mut *conn)
            .await?;
    let ack_enabled = granary_services::nary::ack::enabled(&mut conn).await?;
    drop(conn);
    let stt = granary_services::stt_client::health().await;
    Ok(resp::json(
        json!({"ack_enabled": ack_enabled, "acks_owed": owed, "held_chats": held,
                         "speech_to_text": stt}),
    ))
}

/// `POST /debrief`: the Owner's voice-note debrief, transcribed on the server.
pub async fn transcribe_debrief(
    CurrentUser(user): CurrentUser,
    db: Db,
    form: PyForm,
) -> ApiResult<Response> {
    let mut cx = granary_core::validation::Cx::new();
    let audio = form.req_file(&mut cx, "audio");
    let language = form.opt_text("language");
    form.check(cx)?;
    let audio = audio.expect("checked");
    _require(&user, "nary:approve")?;
    let mut conn = db.conn().await?;
    let max = require_int(
        &mut conn,
        "nary.stt.max_seconds",
        "Nary & AI → Nary on WhatsApp",
    )
    .await?;
    drop(conn);
    let filename = if audio.filename.is_empty() {
        "debrief.ogg".to_string()
    } else {
        audio.filename.clone()
    };
    let mime = audio
        .content_type
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "audio/ogg".into());
    let out = granary_services::stt_client::transcribe(
        &audio.data,
        granary_services::stt_client::TranscribeArgs {
            filename: &filename,
            mime: &mime,
            language: language.as_deref(),
            max_seconds: Some(max),
        },
    )
    .await?;
    Ok(resp::json(out))
}

// ─── WhatsApp history import (Phase 2) ────────────────────────────────────────

fn _import_row(r: &WaImportChat, staff: Option<&HashMap<String, String>>) -> Value {
    let outputs = match &r.outputs {
        Some(Value::Object(m)) if !m.is_empty() => m.clone(),
        _ => Map::new(),
    };
    let reading = match outputs.get("reading") {
        Some(Value::Object(m)) if !m.is_empty() => m.clone(),
        _ => Map::new(),
    };
    let staff_name = staff.and_then(|m| m.get(&r.chat_id)).cloned();
    let g = |m: &Map<String, Value>, k: &str| m.get(k).cloned().unwrap_or(Value::Null);
    json!({"chat_id": r.chat_id, "name": r.name, "message_count": r.message_count,
           "staff": staff_name,
           "last_message_at": _iso(r.last_message_at.as_ref()), "saved_on_phone": g(&outputs, "saved_on_phone"),
           "decision": r.decision, "import_status": r.import_status, "imported_count": r.imported_count,
           "read_status": r.read_status, "classification": r.classification, "summary": r.summary,
           "missed": g(&outputs, "missed"), "created": g(&outputs, "created"),
           "person": g(&reading, "person"), "opportunity": g(&reading, "opportunity"), "error": r.error})
}

/// `_staff_names(db)`: chat id -> the staff member it is linked to.
async fn _staff_names(conn: &mut PgConnection) -> ApiResult<HashMap<String, String>> {
    let links: Vec<(String, String)> = sqlx::query_as(
        "SELECT staff_whatsapp_links.chat_id, users.name FROM staff_whatsapp_links \
         JOIN users ON users.id = staff_whatsapp_links.user_id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let chats: Vec<(String, String)> = sqlx::query_as(
        "SELECT wa_chats.chat_id, users.name FROM wa_chats JOIN users ON users.id = wa_chats.staff_user_id",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(chats.into_iter().chain(links).collect())
}

/// `GET /history`.
pub async fn history_overview(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let rows: Vec<WaImportChat> =
        sqlx::query_as("SELECT * FROM wa_import_chats ORDER BY last_message_at DESC NULLS LAST")
            .fetch_all(&mut *conn)
            .await?;
    let staff = _staff_names(&mut conn).await?;
    let state = history::state(&mut conn).await?;
    Ok(resp::json(json!({
        "state": state,
        "chats": rows.iter().map(|r| _import_row(r, Some(&staff))).collect::<Vec<_>>(),
    })))
}

/// `POST /history/refresh`: load the review list from the phone.
pub async fn history_refresh(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let added = history::refresh_review(&mut conn).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"added": added})))
}

granary_core::py_model! {
    /// `class ImportDecisions(BaseModel)`.
    pub struct ImportDecisions {
        /// chat id -> include | exclude
        pub decisions: IndexMap<String, String>,
    }
}

/// `POST /history/decisions`.
pub async fn history_decide(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ImportDecisions>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let decisions: Vec<(String, String)> = body.decisions.into_iter().collect();
    let mut conn = db.conn().await?;
    let out = history::decide(&mut conn, &decisions, &user).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(out))
}

granary_core::py_model! {
    /// `class RunFlag(BaseModel)`.
    pub struct RunFlag {
        pub run: bool,
    }
}

/// `POST /history/run`: start (or pause) the import.
pub async fn history_run(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<RunFlag>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let value = if body.run {
        history::start(&mut conn, &user).await?
    } else {
        let value = with(history::state(&mut conn).await?, "status", json!("paused"));
        history::set_state(&mut conn, &value).await?;
        value
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json(value))
}

/// `POST /history/chats/{chat_id}/import-now`: one chat to the front.
pub async fn history_import_now(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(chat_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let row: Option<WaImportChat> =
        sqlx::query_as("SELECT * FROM wa_import_chats WHERE chat_id = $1")
            .bind(&chat_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(row) = row else {
        return Err(ApiError::http(404, "That chat isn't on the history list"));
    };
    if row.decision != "include" {
        return Err(ApiError::http(
            409,
            "Only chats ticked to import can be imported",
        ));
    }
    if !matches!(row.import_status.as_str(), "waiting" | "running" | "failed") {
        return Err(ApiError::http(
            409,
            format!("This chat is already {}", row.import_status),
        ));
    }
    match history::import_now(&mut conn, &row).await {
        Ok(()) => {}
        Err(history::TakeInFailure::TakeIn(e)) => return Err(ApiError::http(502, e.0)),
        Err(history::TakeInFailure::Api(e)) => return Err(e),
    }
    // `await db.refresh(row)`.
    let row: WaImportChat = sqlx::query_as("SELECT * FROM wa_import_chats WHERE id = $1")
        .bind(row.id)
        .fetch_one(&mut *conn)
        .await?;
    let staff = _staff_names(&mut conn).await?;
    Ok(resp::json(_import_row(&row, Some(&staff))))
}

granary_core::py_model! {
    /// `class ChatLink(BaseModel)`.
    pub struct ChatLink {
        pub contact_id: String,
    }
}

/// `POST /chats/{chat_id}/link`: link one WhatsApp chat to a CRM contact.
pub async fn link_chat_to_contact(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(chat_id): Path<String>,
    PyJson(body): PyJson<ChatLink>,
) -> ApiResult<Response> {
    _require(&user, "crm:edit_contact")?;
    let mut conn = db.conn().await?;
    let Some(mut chat) = chat_by_id(&mut conn, &chat_id).await? else {
        return Err(ApiError::http(404, "No such WhatsApp chat"));
    };
    let cid = parse_uuid(&body.contact_id, "contact_id")?;
    let contact: Option<Contact> = sqlx::query_as("SELECT * FROM contacts WHERE id = $1")
        .bind(cid)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(contact) = contact.filter(|c| !c.is_deleted) else {
        return Err(ApiError::http(404, "Contact not found"));
    };
    let by = if user.name.is_empty() {
        "a person".to_string()
    } else {
        user.name.clone()
    };
    match chat_links::link(&mut conn, &mut chat, &contact, &by).await {
        Ok(()) => {}
        Err(chat_links::LinkError::Refused(r)) => return Err(ApiError::http(409, r.0)),
        Err(chat_links::LinkError::Api(e)) => return Err(e),
    }
    drop(conn);
    db.commit().await?;
    let name = chat_links::_name(&contact);
    Ok(resp::json(json!({
        "chat_id": chat.chat_id, "contact_id": contact.id.to_string(), "label": format!("linked to {name}"),
        "lead_id": s(chat.lead_id), "deal_id": s(chat.deal_id),
    })))
}

granary_core::py_model! {
    /// `class ChatIds(BaseModel)`.
    pub struct ChatIds {
        pub chat_ids: Vec<String>,
    }
}

/// `POST /history/create-records`: apply the reconstructed pipeline for the chats the Owner picked.
pub async fn history_create(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ChatIds>,
) -> ApiResult<Response> {
    _require(&user, "nary:manage")?;
    let mut conn = db.conn().await?;
    let out = history::create_records(&mut conn, &body.chat_ids, &user).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(out))
}

// ─── Nary and the Owner on WhatsApp (migration 094) ──────────────────────────────

granary_core::py_model! {
    /// Query of `owner_topics`.
    pub struct TopicsQ {
        pub status: String = "open".to_string(),
    }
}

/// `GET /owner/topics`: what Nary has raised with the Owner.
pub async fn owner_topics(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<TopicsQ>,
) -> ApiResult<Response> {
    _require(&user, "nary:view")?;
    let mut conn = db.conn().await?;
    let sql = if q.status == "open" {
        "SELECT * FROM nary_owner_topics WHERE status IN ('queued', 'open', 'waiting') \
         ORDER BY created_at DESC LIMIT 100"
    } else {
        "SELECT * FROM nary_owner_topics ORDER BY created_at DESC LIMIT 100"
    };
    let rows: Vec<NaryOwnerTopic> = sqlx::query_as(sql).fetch_all(&mut *conn).await?;
    Ok(resp::json(json!({
        "topics": rows.iter().map(owner_channel::serialize).collect::<Vec<_>>(),
    })))
}

granary_core::py_model! {
    /// `class TopicAnswer(BaseModel)`.
    pub struct TopicAnswer {
        pub answer: String,
        pub tier_pct: Option<Decimal> = None,
    }
}

async fn load_topic(conn: &mut PgConnection, topic_id: &str) -> ApiResult<Option<NaryOwnerTopic>> {
    let id = parse_uuid(topic_id, "topic_id")?;
    Ok(
        sqlx::query_as("SELECT * FROM nary_owner_topics WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// `POST /owner/topics/{topic_id}/answer`.
pub async fn answer_owner_topic(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(topic_id): Path<String>,
    PyJson(body): PyJson<TopicAnswer>,
) -> ApiResult<Response> {
    _require(&user, "nary:approve")?;
    let mut conn = db.conn().await?;
    let topic = load_topic(&mut conn, &topic_id).await?;
    let Some(mut topic) = topic.filter(|t| matches!(t.status.as_str(), "open" | "waiting")) else {
        return Err(ApiError::http(404, "No open topic with that id"));
    };
    // exact from the request (13 Nov 2026: was f64; NaN or Infinity is now a 422, not a 500)
    let tier_pct = body.tier_pct;
    let outcome = owner_channel::answer(
        &mut conn,
        &mut topic,
        granary_compat::text::pystr::strip(&body.answer),
        &user,
        owner_channel::AnswerExtra { tier_pct },
    )
    .await?;
    drop(conn);
    db.commit().await?;
    dispatch_outbox();
    let mut out = owner_channel::serialize(&topic);
    out = with(out, "outcome", json!(outcome));
    out = with(out, "label", json!(outcome));
    Ok(resp::json(out))
}

/// `POST /owner/topics/{topic_id}/close`.
pub async fn close_owner_topic(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(topic_id): Path<String>,
) -> ApiResult<Response> {
    _require(&user, "nary:approve")?;
    let mut conn = db.conn().await?;
    let Some(mut topic) = load_topic(&mut conn, &topic_id).await? else {
        return Err(ApiError::http(404, "Topic not found"));
    };
    owner_channel::close(&mut conn, &mut topic).await?;
    Ok(resp::json(owner_channel::serialize(&topic)))
}
