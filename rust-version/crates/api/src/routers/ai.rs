//! backend/app/routers/ai.py, mounted at `/api/v1/ai` (backend/app/main.py).
//!
//! AI assistant API: streaming chat (SSE) + per-user conversation history, reply suggestions and document
//! analysis.
//!
//! Not behind INTERNAL_ONLY: routes guard themselves. The suggest, analyze and chat routes take internal
//! staff only (`_require_internal`); the conversation routes only ever show the caller's own chats.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{patch, post};
use axum::Router;
use futures::StreamExt;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{v, PyJson};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::ai::{AIConversation, AIMessage};
use granary_services::ai::assistant::{question_first, run_assistant_live, AssistantOptions}; // fix-stream
use granary_services::nary::governance;

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/ai";

/// The routes of `router` in ai.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/suggest/reply"), post(suggest_reply_endpoint))
        .route(
            &at(PREFIX, "/document/analyze"),
            post(analyze_document_endpoint),
        )
        .route(&at(PREFIX, "/assistant/chat"), post(assistant_chat))
        // fix-stream: diagnostic for the chat's long-lived stream through nginx and Cloudflare.
        .route(
            &at(PREFIX, "/stream-check"),
            axum::routing::get(stream_check),
        )
        .route(
            &at(PREFIX, "/conversations"),
            axum::routing::get(list_conversations),
        )
        .route(
            &at(PREFIX, "/conversations/{conversation_id}"),
            axum::routing::get(get_conversation)
                .patch(rename_conversation)
                .delete(delete_conversation),
        )
        .route(
            &at(
                PREFIX,
                "/conversations/{conversation_id}/proposals/{proposal_id}",
            ),
            patch(set_proposal_status),
        )
}

granary_core::py_model! {
    /// `class ChatBody(BaseModel)`.
    pub struct ChatBody {
        pub message: String,
        pub conversation_id: Option<String> = None,
        pub context: Option<Map<String, Value>> = None,
    }
}

/// `_require_internal(user)`.
fn _require_internal(user: &AuthUser) -> ApiResult<()> {
    if !(user.is_superadmin || user.is_system_admin || user.portal_type == "internal") {
        return Err(ApiError::http(403, "No access"));
    }
    Ok(())
}

/// `if out.get("error"): raise HTTPException(400, detail=out["error"])`.
fn error_out(out: &Value) -> ApiResult<()> {
    match out.get("error") {
        Some(e) if granary_services::nary::actions::py_truthy(e) => {
            Err(ApiError::http_json(400, e.clone()))
        }
        _ => Ok(()),
    }
}

granary_core::py_model! {
    /// `class SuggestReplyBody(BaseModel)`.
    pub struct SuggestReplyBody {
        /// whatsapp | email
        pub channel: String,
        pub chat_id: Option<String> = None,
        pub thread_id: Option<String> = None,
        /// What the user wants the reply to say, and optionally their own draft to start from.
        pub instruction: Option<String> = None => v::nullable(v::str_c(v::StrC::new().max_length(8000))),
    }
}

/// `POST /suggest/reply`: draft a suggested reply for a WhatsApp/email thread (never sends).
pub async fn suggest_reply_endpoint(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<SuggestReplyBody>,
) -> ApiResult<Response> {
    _require_internal(&user)?;
    let mut conn = db.conn().await?;
    let out = granary_services::ai::suggestions::suggest_reply(
        &mut conn,
        &user,
        &body.channel,
        body.chat_id.as_deref(),
        body.thread_id.as_deref(),
        body.instruction.as_deref(),
    )
    .await?;
    error_out(&out)?;
    Ok(resp::json(out))
}

granary_core::py_model! {
    /// `class AnalyzeDocBody(BaseModel)`.
    pub struct AnalyzeDocBody {
        /// attachment | email
        pub source: String,
        pub attachment_id: Option<String> = None,
        pub message_id: Option<String> = None,
        pub att_id: Option<String> = None,
        pub question: Option<String> = None,
    }
}

/// `POST /document/analyze`: analyze a stored PDF/image attachment.
pub async fn analyze_document_endpoint(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<AnalyzeDocBody>,
) -> ApiResult<Response> {
    _require_internal(&user)?;
    let mut conn = db.conn().await?;
    let out = granary_services::ai::documents::analyze_document(
        &mut conn,
        &user,
        &body.source,
        body.attachment_id.as_deref(),
        body.message_id.as_deref(),
        body.att_id.as_deref(),
        body.question.as_deref(),
    )
    .await?;
    error_out(&out)?;
    Ok(resp::json(out))
}

/// `uuid.UUID(str(conversation_id))`: None where Python raises ValueError.
fn py_uuid(raw: &str) -> Option<Uuid> {
    granary_compat::text::utils::py_uuid(raw)
}

/// `_get_owned_conversation(db, user, conversation_id)` once the id parsed.
async fn owned(conn: &mut PgConnection, user: &AuthUser, cid: Uuid) -> ApiResult<AIConversation> {
    let conv: Option<AIConversation> =
        sqlx::query_as("SELECT * FROM ai_conversations WHERE id = $1")
            .bind(cid)
            .fetch_optional(&mut *conn)
            .await?;
    match conv {
        Some(c) if c.user_id == user.id => Ok(c),
        _ => Err(ApiError::http(404, "Conversation not found")),
    }
}

/// `_get_owned_conversation` where the caller does not catch the ValueError of a malformed id: the
/// central 422 "Invalid id format" (`uuid.UUID(...)` on a router line).
async fn owned_or_500(
    conn: &mut PgConnection,
    user: &AuthUser,
    raw: &str,
) -> ApiResult<AIConversation> {
    let cid = crate::extract::router_uuid(raw)?;
    owned(conn, user, cid).await
}

/// `json.dumps(payload)` (default separators, ensure_ascii).
fn dumps(v: &Value) -> String {
    use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
    dumps(&PyValue::from(v), &DumpOpts::default()).unwrap_or_else(|_| v.to_string())
}

/// `_produce(queue)`: the assistant's events on a dedicated session (the request's one ends when the
/// handler returns).
/// How long a chat's turn lock lives at most (a crashed turn frees its chat after this); a turn that ends
/// releases it at once. Turns of 10+ minutes happen (web research), so it is generous.
const TURN_LOCK_SECS: i64 = 1800;

/// The lock that keeps one Nary turn per chat: a second message sent while a turn runs would start a turn
/// that cannot see the first one's results (21 Oct: two overlapping turns, one ended with no text).
struct TurnLock {
    conn: redis::aio::MultiplexedConnection,
    key: String,
    token: String,
}

impl TurnLock {
    /// `Ok(None)` when Redis can't be reached (the chat still works, unguarded), `Err` 409 when a turn of
    /// this chat is running.
    async fn take(conv_id: Uuid) -> ApiResult<Option<TurnLock>> {
        let mut conn = match granary_services::redis_lease::client().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "assistant_turn_lock_unavailable");
                return Ok(None);
            }
        };
        let key = format!("ai:turn:{conv_id}");
        let token = granary_services::redis_lease::new_token();
        match granary_services::redis_lease::acquire(&mut conn, &key, &token, TURN_LOCK_SECS).await {
            Ok(true) => Ok(Some(TurnLock { conn, key, token })),
            Ok(false) => Err(ApiError::http(
                409,
                "Nary is still working on your last message in this chat. Wait for the answer, or start a new chat.",
            )),
            Err(e) => {
                tracing::warn!(error = %e, "assistant_turn_lock_unavailable");
                Ok(None)
            }
        }
    }

    async fn release(mut self) {
        let _ =
            granary_services::redis_lease::release(&mut self.conn, &self.key, &self.token).await;
    }
}

async fn produce(
    pool: PgPool,
    user: AuthUser,
    conv_id: Uuid,
    message: String,
    context: Option<Map<String, Value>>,
    tx: tokio::sync::mpsc::UnboundedSender<Option<Value>>,
    lock: Option<TurnLock>,
) {
    let run = async {
        // AsyncSessionLocal(): a transaction the assistant commits itself; what is left is rolled back
        // when the session closes.
        let mut session = pool.begin().await?;
        let mut conv = owned(&mut session, &user, conv_id).await?;
        let opts = AssistantOptions {
            context,
            ..AssistantOptions::default()
        };
        // fix-stream: the chat shows the answer's text as the model writes it.
        let mut events = run_assistant_live(&mut session, &user, &mut conv, &message, opts);
        while let Some(item) = events.next().await {
            match item {
                Ok(ev) => {
                    let _ = tx.send(Some(ev));
                }
                Err(e) => return Err(e),
            }
        }
        drop(events);
        Ok::<(), ApiError>(())
    }
    .await;
    if let Err(e) = run {
        let text: String = governance::py_exception(&e).2.chars().take(200).collect();
        let _ = tx.send(Some(json!({"type": "error", "text": text})));
        // The conversation exists already; hand its id over so the next message continues it.
        let _ = tx.send(Some(
            json!({"type": "done", "conversation_id": conv_id.to_string()}),
        ));
    }
    if let Some(lock) = lock {
        lock.release().await;
    }
    let _ = tx.send(None);
}

/// `POST /assistant/chat`: stream the assistant's reply as Server-Sent Events.
pub async fn assistant_chat(
    CurrentUser(user): CurrentUser,
    State(state): State<AppState>,
    db: Db,
    PyJson(body): PyJson<ChatBody>,
) -> ApiResult<Response> {
    // Internal staff only: a portal user must not reach the unguarded tools or the AI knowledge.
    _require_internal(&user)?;
    let message = granary_compat::text::pystr::strip(&body.message).to_string();
    if message.is_empty() {
        return Err(ApiError::http(400, "message is required"));
    }
    let mut conn = db.conn().await?;
    let conv_id = match body.conversation_id.as_deref().filter(|c| !c.is_empty()) {
        Some(raw) => {
            let Some(cid) = py_uuid(raw) else {
                return Err(ApiError::http(404, "Conversation not found"));
            };
            owned(&mut conn, &user, cid).await?.id
        }
        None => {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO ai_conversations (user_id, id) VALUES ($1, $2)")
                .bind(user.id)
                .bind(id)
                .execute(&mut *conn)
                .await?;
            drop(conn);
            db.commit().await?;
            id
        }
    };
    // One turn per chat at a time: refused before anything of this message is stored.
    let lock = TurnLock::take(conv_id).await?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Option<Value>>();
    let task = crate::lifecycle::spawn_scoped(produce(
        state.db.clone(),
        user.clone(),
        conv_id,
        message,
        body.context.clone(),
        tx,
        lock,
    ));
    // fix-walist: the turn finishes and saves even when the browser disconnects (Cloudflare's 100 s limit,
    // a closed tab): dropping the JoinHandle with the stream detaches the task instead of aborting it (Python
    // cancelled it). The task owns its own DB session and ignores a closed channel.
    let guard = task;
    // Merge the events with a heartbeat every 15 s so a slow turn does not get its idle connection killed
    // by a reverse proxy (`:` comment lines are ignored by the client).
    let stream =
        futures::stream::unfold((rx, guard, false), |(mut rx, guard, finished)| async move {
            if finished {
                return None;
            }
            match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await {
                Err(_) => Some((
                    Ok::<String, Infallible>(": keepalive\n\n".to_string()),
                    (rx, guard, false),
                )),
                Ok(Some(Some(ev))) => {
                    Some((Ok(format!("data: {}\n\n", dumps(&ev))), (rx, guard, false)))
                }
                Ok(Some(None)) | Ok(None) => None,
            }
        });
    let out = resp::streaming(stream, "text/event-stream");
    Ok(resp::with_headers(
        out,
        &[
            ("cache-control", "no-cache".to_string()),
            ("x-accel-buffering", "no".to_string()),
            ("connection", "keep-alive".to_string()),
        ],
    ))
}

// fix-stream: GET /stream-check.

granary_core::py_model! {
    /// The query of `GET /stream-check`.
    pub struct StreamCheckQ {
        pub seconds: i64 = 130 => v::int_c(v::NumC::new().ge("1").le("300")),
    }
}

/// What `GET /stream-check` sends for `total` seconds: (second, event), `None` being a `: keepalive`
/// comment. Something goes out every 15 s, as in the chat: a keepalive, or on every 30th second a
/// `tick`; the `done` event ends it.
pub fn stream_check_plan(total: u64) -> Vec<(u64, Option<Value>)> {
    let mut out = Vec::new();
    let mut at = 15;
    while at < total {
        if at % 30 == 0 {
            out.push((at, Some(json!({"type": "tick", "elapsed": at}))));
        } else {
            out.push((at, None));
        }
        at += 15;
    }
    out.push((total, Some(json!({"type": "done", "elapsed": total}))));
    out
}

/// `GET /stream-check?seconds=N` (internal staff, N 1..300, default 130): an SSE stream shaped like the
/// chat's (same headers, a `: keepalive` comment every 15 s), with a `tick` event every 30 s and a `done`
/// event after N seconds. No AI, no database: it tells whether a long stream survives the proxies
/// between the browser and the API.
pub async fn stream_check(
    CurrentUser(user): CurrentUser,
    granary_core::validation::PyQuery(q): granary_core::validation::PyQuery<StreamCheckQ>,
) -> ApiResult<Response> {
    _require_internal(&user)?;
    let start = tokio::time::Instant::now();
    let plan = stream_check_plan(q.seconds as u64);
    let stream = futures::stream::iter(plan).then(move |(at, ev)| async move {
        tokio::time::sleep_until(start + Duration::from_secs(at)).await;
        Ok::<String, Infallible>(match ev {
            Some(ev) => format!("data: {}\n\n", dumps(&ev)),
            None => ": keepalive\n\n".to_string(),
        })
    });
    let out = resp::streaming(stream, "text/event-stream");
    Ok(resp::with_headers(
        out,
        &[
            ("cache-control", "no-cache".to_string()),
            ("x-accel-buffering", "no".to_string()),
            ("connection", "keep-alive".to_string()),
        ],
    ))
}

/// `GET /conversations`: the caller's own chat sessions, newest first.
pub async fn list_conversations(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let rows: Vec<AIConversation> = sqlx::query_as(
        "SELECT * FROM ai_conversations WHERE user_id = $1 ORDER BY updated_at DESC LIMIT 100",
    )
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|c| {
            json!({"id": c.id.to_string(),
                   "title": c.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "New chat".into()),
                   "updated_at": resp::iso(&c.updated_at)})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `(m.tool_calls or {}).get(key) if isinstance(m.tool_calls, dict) else None`.
fn tool_calls_key(m: &AIMessage, key: &str) -> Value {
    match &m.tool_calls {
        Some(Value::Object(d)) => d.get(key).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// `GET /conversations/{conversation_id}`.
pub async fn get_conversation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(conversation_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let conv = owned_or_500(&mut conn, &user, &conversation_id).await?;
    let rows: Vec<AIMessage> = sqlx::query_as(&format!(
        "SELECT * FROM ai_messages WHERE conversation_id = $1 ORDER BY ai_messages.created_at, {}",
        question_first()
    ))
    .bind(conv.id)
    .fetch_all(&mut *conn)
    .await?;
    let messages: Vec<Value> = rows
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| {
            json!({"role": m.role, "content": m.content, "citations": m.citations,
                   "cards": tool_calls_key(m, "cards"), "proposals": tool_calls_key(m, "proposals"),
                   "created_at": resp::iso(&m.created_at)})
        })
        .collect();
    Ok(resp::json(json!({
        "id": conv.id.to_string(),
        "title": conv.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "New chat".into()),
        "messages": messages,
    })))
}

granary_core::py_literal! {
    /// `Literal["applied", "discarded"]`.
    pub enum ProposalStatus { Applied = "applied", Discarded = "discarded" }
}

granary_core::py_model! {
    /// `class ProposalStatusBody(BaseModel)`.
    pub struct ProposalStatusBody {
        pub status: ProposalStatus,
        /// {id, label} of what was created or updated
        pub result: Option<Map<String, Value>> = None,
    }
}

/// `PATCH /conversations/{conversation_id}/proposals/{proposal_id}`: record what the user did with a
/// confirm card.
pub async fn set_proposal_status(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((conversation_id, proposal_id)): Path<(String, String)>,
    PyJson(body): PyJson<ProposalStatusBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let Some(cid) = py_uuid(&conversation_id) else {
        return Err(ApiError::http(404, "Conversation not found"));
    };
    let conv = owned(&mut conn, &user, cid).await?;
    let rows: Vec<AIMessage> = sqlx::query_as(
        "SELECT * FROM ai_messages WHERE conversation_id = $1 AND role = 'assistant'",
    )
    .bind(conv.id)
    .fetch_all(&mut *conn)
    .await?;
    let status = body.status.as_str();
    for msg in rows {
        let Some(Value::Object(mut calls)) = msg.tool_calls.clone() else {
            continue;
        };
        let Some(Value::Array(mut proposals)) = calls.get("proposals").cloned() else {
            continue;
        };
        let mut hit = false;
        for p in proposals.iter_mut() {
            let Value::Object(p) = p else {
                // `p.get(...)` on a non-dict: AttributeError (500).
                return Err(ApiError::internal(anyhow::anyhow!(
                    "'list' object has no attribute 'get'"
                )));
            };
            if p.get("proposal_id") == Some(&json!(proposal_id)) {
                p.insert("status".into(), json!(status));
                if let Some(result) = body.result.as_ref().filter(|r| !r.is_empty()) {
                    let mut kept = Map::new();
                    for k in ["id", "label"] {
                        if let Some(v) = result.get(k) {
                            kept.insert(k.into(), v.clone());
                        }
                    }
                    p.insert("result".into(), Value::Object(kept));
                }
                hit = true;
                break;
            }
        }
        if hit {
            calls.insert("proposals".into(), Value::Array(proposals));
            sqlx::query("UPDATE ai_messages SET tool_calls = $2, updated_at = now() WHERE id = $1")
                .bind(msg.id)
                .bind(Value::Object(calls))
                .execute(&mut *conn)
                .await?;
            drop(conn);
            db.commit().await?;
            return Ok(resp::json(
                json!({"proposal_id": proposal_id, "status": status}),
            ));
        }
    }
    Err(ApiError::http(404, "Proposal not found"))
}

granary_core::py_model! {
    /// `class RenameBody(BaseModel)`.
    pub struct RenameBody {
        pub title: String,
    }
}

/// `PATCH /conversations/{conversation_id}`.
pub async fn rename_conversation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(conversation_id): Path<String>,
    PyJson(body): PyJson<RenameBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let conv = owned_or_500(&mut conn, &user, &conversation_id).await?;
    let title: String = granary_compat::text::pystr::strip(&body.title)
        .chars()
        .take(300)
        .collect();
    let title = if title.is_empty() {
        "New chat".to_string()
    } else {
        title
    };
    // No UPDATE (and no updated_at bump) when the title did not change.
    if conv.title.as_deref() != Some(title.as_str()) {
        sqlx::query("UPDATE ai_conversations SET title = $2, updated_at = now() WHERE id = $1")
            .bind(conv.id)
            .bind(&title)
            .execute(&mut *conn)
            .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"id": conv.id.to_string(), "title": title}),
    ))
}

/// `DELETE /conversations/{conversation_id}`.
pub async fn delete_conversation(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(conversation_id): Path<String>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let conv = owned_or_500(&mut conn, &user, &conversation_id).await?;
    sqlx::query("DELETE FROM ai_conversations WHERE ai_conversations.id = $1")
        .bind(conv.id)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "deleted"})))
}
