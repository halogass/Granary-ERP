//! Port of backend/app/services/nary/executor.py.
//!
//! The executor: how a proposed action becomes a shadow draft, an approval card, or a change.
//!
//! ```text
//!     propose()  Nary (or an MCP client) wants to do something. The autonomy level decides:
//!                0 shadow  → stored as a draft, nothing happens
//!                1 ask     → an approval card for the Owner (the approver role)
//!                2 act and tell → applied now, the Owner is told
//!                3 silent  → applied now
//!                A customer-facing action is always a shadow draft in Phase 1, and in shadow mode.
//!     decide()   The Owner applies (as is, or edited) or rejects a card. Feeds autonomy. What a person applies is
//!                their own instruction (WhatsApp quiet hours and the takeover rule don't stop it).
//!     Quiet hours: one of Nary's own WhatsApp messages outside working hours is not failed but "scheduled";
//!                release_scheduled() sends it at the start of the next working period, and "1" sends it now.
//!     compare()  Shadow mode: how far a draft was from what a person actually sent.
//! ```
//!
//! Port notes
//! - Python commits the caller's session in the middle of these functions; here that is `nary::commit`
//!   (`COMMIT; BEGIN` when the caller passes a transaction).
//! - `_apply` runs in a session of its own (a fresh pool connection in a transaction, the 14 Oct MissingGreenlet
//!   fix); the card row is re-read from the database afterwards, which is what `populate_existing` does.
//!   Notifications an action queues on that own session go to a nested notify scope; once the action is
//!   committed they move to the caller's scope (`notify.move_outbox`), whose request or job delivers them after
//!   its commit. A failed action's are discarded with its rollback.
//! - wa_send's QuietHours / PacingLimit reach the executor as the HTTPException they are (an `ApiError::Http`,
//!   since actions return `ApiResult`): QuietHours is recognised by its 409 detail "Outside working hours: it
//!   goes out {label}." (its `until` is `wa_send::quiet_until` again), PacingLimit by its 429 and one of its
//!   two texts.
use chrono::{DateTime, TimeDelta, Utc};
use granary_compat::pyjson::PyDateTime;
use granary_compat::text::{difflib, pystr};
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::require_int;
use granary_db::rows::nary::AgentPendingAction;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::{self as registry, py_truthy, Action};
use super::governance::{self, http_detail, http_status, is_http_exception, py_str};
use super::wa_send::{self, SendError};
use super::{autonomy, commands, owner_channel, py_slice, roles};
use crate::approvals::notify::{self, Notice};

pub const WHERE: &str = "Nary & AI → Autonomy & approvals";

/// `NaryPaused()`: HTTPException 409.
#[allow(non_snake_case)]
pub fn NaryPaused() -> ApiError {
    ApiError::http(409, "Nary is paused. Resume it on the Nary page.")
}

pub fn is_nary_paused(e: &ApiError) -> bool {
    http_status(e) == Some(409)
        && http_detail(e).as_deref() == Some("Nary is paused. Resume it on the Nary page.")
}

tokio::task_local! {
    /// 3 Nov 2026 (the MCP client as Nary, Owner decisions 3 Nov): work a person asked for in the chat or from an AI client
    /// (MCP) ("chat" / "mcp") that reuses Nary's own code (a contract draft that raises the down-payment card, a
    /// meeting file's debrief that raises the stage/value and summary email cards). While Nary is paused those
    /// cards are still raised, because the person asked; Nary's own work (no scope) stays paused.
    static ON_REQUEST: &'static str;
}

/// Runs `fut` as work a person asked for (`origin` "chat" or "mcp"): see [`ON_REQUEST`].
pub async fn on_request<F: std::future::Future>(origin: &'static str, fut: F) -> F::Output {
    ON_REQUEST.scope(origin, fut).await
}

/// The origin of the request this work runs for, if a person asked for it (see [`on_request`]).
pub fn requested_by() -> Option<&'static str> {
    ON_REQUEST.try_with(|o| *o).ok()
}

/// 3 Nov 2026: the deal close-out actions. A card of these from the chat or the MCP client (origin chat / mcp)
/// is applied as work the person asked for ([`on_request`]): what it raises next (the down-payment proforma
/// after the contract draft) is raised even while Nary is paused.
pub const ON_REQUEST_ACTIONS: &[&str] = &[
    "contract.create_draft",
    "proforma.raise",
    "deal.won",
    "handoff.kickoff",
];

/// The request origin a card is applied under: a close-out card made in the chat or from the MCP client.
fn request_origin(card: &AgentPendingAction) -> Option<&'static str> {
    if !ON_REQUEST_ACTIONS.contains(&card.action.as_str()) {
        return None;
    }
    match card.origin.as_str() {
        "chat" => Some("chat"),
        "mcp" => Some("mcp"),
        _ => None,
    }
}

/// The optional keyword arguments of `propose`.
#[derive(Clone, Debug)]
pub struct ProposeOpts<'a> {
    /// default "nary"
    pub origin: &'a str,
    /// `run=...` (only its id is used)
    pub run_id: Option<Uuid>,
    pub rationale: Option<&'a str>,
    pub sop_ref: Option<&'a str>,
    pub sources: Option<Value>,
    pub price: Option<Value>,
    /// R&D 26 Oct (double sends): what raised the card ("cadence:{step}", "reply:{job}"...). One card per key:
    /// a second propose with the same key returns the first card and sends nothing.
    pub trigger_key: Option<String>,
}

impl Default for ProposeOpts<'_> {
    fn default() -> Self {
        ProposeOpts {
            origin: "nary",
            run_id: None,
            rationale: None,
            sop_ref: None,
            sources: None,
            price: None,
            trigger_key: None,
        }
    }
}

const COLS: &str = "action, origin, status, level, customer_facing, payload, edited_payload, rationale, sop_ref, \
    sources, diff, price, entity_type, entity_id, run_id, actor_id, assignee_id, decided_by_id, decided_at, \
    reject_reason, edit_ratio, result, error, actual_text, diff_ratio, compared_at, id, created_at, updated_at";

/// `db.get(AgentPendingAction, id)`.
pub async fn get_card(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<AgentPendingAction>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM agent_pending_actions WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

async fn must_card(db: &mut PgConnection, id: Uuid) -> ApiResult<AgentPendingAction> {
    get_card(db, id).await?.ok_or_else(|| {
        ApiError::internal(governance::PyException::new(
            "AttributeError",
            "'NoneType' object has no attribute 'status'",
        ))
    })
}

fn diff_label_truthy(diff: &Value) -> Option<String> {
    diff.get("label").filter(|v| py_truthy(v)).map(py_str)
}

/// The INSERT of a new card. `Option` columns left `None` are SQL NULL (an ORM attribute never set);
/// a JSON column Python sets to None explicitly is passed as `Some(Value::Null)` (stored as JSON null).
#[derive(Default)]
struct NewCard {
    action: String,
    origin: String,
    level: i32,
    customer_facing: bool,
    payload: Value,
    rationale: Option<String>,
    sop_ref: Option<String>,
    sources: Option<Value>,
    diff: Option<Value>,
    price: Option<Value>,
    entity_type: Option<String>,
    entity_id: Option<Uuid>,
    run_id: Option<Uuid>,
    actor_id: Option<Uuid>,
    assignee_id: Option<Uuid>,
    decided_by_id: Option<Uuid>,
    decided_at: Option<DateTime<Utc>>,
    status: String,
    trigger_key: Option<String>,
}

async fn insert_card(db: &mut PgConnection, c: NewCard) -> ApiResult<AgentPendingAction> {
    insert_card_once(db, c).await?.ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!("a card with this trigger already exists"))
    })
}

/// The card with this trigger key, if one was made.
async fn card_by_trigger(
    db: &mut PgConnection,
    key: &str,
) -> ApiResult<Option<AgentPendingAction>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM agent_pending_actions WHERE trigger_key = $1"
    ))
    .bind(key)
    .fetch_optional(&mut *db)
    .await?)
}

/// [`insert_card`]; `None` when a card with the same trigger key exists already (made by another run).
async fn insert_card_once(
    db: &mut PgConnection,
    c: NewCard,
) -> ApiResult<Option<AgentPendingAction>> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO agent_pending_actions (id, action, origin, level, customer_facing, payload, rationale, sop_ref, \
         sources, diff, price, entity_type, entity_id, run_id, actor_id, assignee_id, decided_by_id, decided_at, \
         status, trigger_key) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20) \
         ON CONFLICT (trigger_key) WHERE trigger_key IS NOT NULL DO NOTHING \
         RETURNING {COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(&c.action)
    .bind(&c.origin)
    .bind(c.level)
    .bind(c.customer_facing)
    .bind(&c.payload)
    .bind(&c.rationale)
    .bind(&c.sop_ref)
    .bind(&c.sources)
    .bind(&c.diff)
    .bind(&c.price)
    .bind(&c.entity_type)
    .bind(c.entity_id)
    .bind(c.run_id)
    .bind(c.actor_id)
    .bind(c.assignee_id)
    .bind(c.decided_by_id)
    .bind(c.decided_at)
    .bind(&c.status)
    .bind(&c.trigger_key)
    .fetch_optional(&mut *db)
    .await?)
}

async fn set_status(db: &mut PgConnection, id: Uuid, status: &str) -> ApiResult<()> {
    sqlx::query("UPDATE agent_pending_actions SET status = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// R&D 26 Oct (double sends): takes a scheduled card for this run only. Two runs releasing at once (a slow
/// tick, a redelivered job) can't both send it: the one whose UPDATE moves it to applying goes, the other
/// finds it gone. True when this run has it.
async fn claim_scheduled(db: &mut PgConnection, id: Uuid) -> ApiResult<bool> {
    let got: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE agent_pending_actions SET status = 'applying', updated_at = now() \
         WHERE id = $1 AND status = 'scheduled' RETURNING id",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(got.is_some())
}

/// Route a proposed action by its autonomy level. `actor` is the user the action acts as (its
/// permissions apply). Commits.
pub async fn propose<'a>(
    db: &mut PgConnection,
    action_name: &str,
    payload: Value,
    actor: impl Into<Option<&'a AuthUser>>,
    opts: ProposeOpts<'_>,
) -> ApiResult<AgentPendingAction> {
    let actor: Option<&AuthUser> = actor.into();
    let action = registry::get(action_name)?;
    // 3 Nov 2026: work a person asked for (chat / the MCP client) raises Nary's follow-up cards while it is paused
    let asked = opts.origin == "nary" && requested_by().is_some();
    if opts.origin == "nary" && !asked && governance::is_paused(db).await? {
        return Err(NaryPaused());
    }
    if let Some(key) = opts.trigger_key.as_deref() {
        if let Some(made) = card_by_trigger(db, key).await? {
            tracing::info!(card_id = %made.id, trigger_key = key, action = action_name, "nary_card_trigger_repeat");
            return Ok(made);
        }
    }
    let mut clean = action.validate(&payload)?;
    // R&D 26 Oct: Nary's own email cards say who else gets them (reply to all) and the inbox they go from
    if action.customer_facing && opts.origin == "nary" {
        super::channel_actions::email_card_defaults(db, action_name, &mut clean).await?;
    }
    let mut level = autonomy::level_for(db, action_name).await?;
    // Nary's own customer messages pass the writing check first (B3): one rewrite on a hit, else the
    // problems go on the card and it waits for the Owner.
    let mut rationale_owned: Option<String> = opts.rationale.map(str::to_string);
    if action.customer_facing && opts.origin == "nary" {
        // boxed: the check's model call would otherwise sit inside every propose future
        let review = Box::pin(super::writing::review_card(db, action_name, &mut clean)).await?;
        if let Some(note) = review.note {
            rationale_owned = Some(match rationale_owned.filter(|r| !r.is_empty()) {
                Some(r) => format!("{r}\n{note}"),
                None => note,
            });
        }
        if review.hold && level > 1 {
            level = 1;
        }
    }
    let opts = ProposeOpts {
        rationale: rationale_owned.as_deref(),
        ..opts
    };
    let forced_shadow = action.customer_facing
        && (!registry::CUSTOMER_SENDING_ENABLED
            || (governance::shadow_mode(db).await?
                && !governance::shadow_exempt(db)
                    .await?
                    .iter()
                    .any(|a| a == action_name)));
    if forced_shadow {
        level = 0;
    }
    // 3 Nov 2026: a customer message raised by work a person asked for always waits for the Owner's yes
    if asked && action.customer_facing && level > 1 {
        level = 1;
    }
    // Fix batch 1 (B2): one customer message per chat (reply_guard). A reply replaces the older messages
    // still waiting for the chat; a proactive one is cancelled when something else is already waiting.
    let guard_cancel = if level >= 1 {
        super::reply_guard::on_propose(db, action_name, &clean, opts.run_id).await?
    } else {
        None
    };
    let (entity_type, entity_id) = (action.entity)(&clean)?;
    let diff = match &action.describe {
        Some(d) => d(db, actor, &clean).await?,
        None => Value::Null,
    };
    let approvers = roles::approver_ids(db).await?;
    let trigger_key = opts.trigger_key.clone();
    let row = insert_card_once(
        db,
        NewCard {
            action: action_name.to_string(),
            origin: opts.origin.to_string(),
            level: level as i32,
            customer_facing: action.customer_facing,
            payload: clean.clone(),
            rationale: opts.rationale.map(str::to_string),
            sop_ref: opts.sop_ref.map(str::to_string),
            sources: Some(opts.sources.clone().unwrap_or(Value::Null)),
            diff: Some(diff.clone()),
            price: Some(opts.price.clone().unwrap_or(Value::Null)),
            entity_type,
            entity_id,
            run_id: opts.run_id,
            actor_id: actor.map(|a| a.id),
            assignee_id: approvers.first().copied(),
            status: match level {
                0 => "shadow",
                1 => "pending",
                _ => "applying",
            }
            .to_string(),
            trigger_key: trigger_key.clone(),
            ..Default::default()
        },
    )
    .await?;
    let row = match (row, trigger_key.as_deref()) {
        (Some(row), _) => row,
        // another run made the card for this trigger between the check above and now: that one stands
        (None, Some(key)) => {
            super::rollback(db).await?;
            return card_by_trigger(db, key).await?.ok_or_else(|| {
                ApiError::internal(anyhow::anyhow!("the card of trigger {key} is gone"))
            });
        }
        (None, None) => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "the card insert returned nothing"
            )))
        }
    };
    if let Some(reason) = guard_cancel {
        super::reply_guard::cancel_card(db, row.id, &reason).await?;
        super::commit(db).await?;
        return must_card(db, row.id).await;
    }
    if level == 1 {
        let body_full = opts
            .rationale
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .or_else(|| diff_label_truthy(&diff))
            .unwrap_or_default();
        let body = py_slice(&body_full, 500).to_string();
        let title = format!("Nary asks: {}", action.label);
        let link = format!("/nary/approvals?card={}", row.id);
        let card_id = row.id.to_string();
        if owner_channel::enabled(db).await? {
            // The Owner's WhatsApp conversation with Nary carries it (timing, bundling, reply in words).
            notify::notify(
                db,
                &approvers,
                Notice::new(&title, &body, &link, "nary_action", &card_id)
                    .type_("nary_card")
                    .email(false)
                    .whatsapp(false),
            )
            .await?;
            // C5-C8: the whole card (full message text, prices, quotation lines, merge differences).
            let null = Value::Null;
            let text = Box::pin(super::cards::render(
                db,
                super::cards::CardView {
                    action: action_name,
                    payload: &clean,
                    diff: &diff,
                    price: opts.price.as_ref().unwrap_or(&null),
                    rationale: opts.rationale,
                },
            ))
            .await?;
            let never = autonomy::never_promote(db)
                .await?
                .iter()
                .any(|a| a == action_name);
            let mut t = owner_channel::Topic::new("approval", &title, &text);
            t.ref_type = Some("nary_card".into());
            t.ref_id = Some(row.id);
            t.numbered = Some(json!({"kind": "card", "card_id": card_id}));
            t.facts = Some(
                json!({"action": action_name, "card_id": card_id, "payload": clean,
                                  "price": opts.price.clone().unwrap_or(Value::Null), "never_promote": never}),
            );
            owner_channel::raise_topic(db, t).await?;
        } else {
            let on_whatsapp = commands::card_notice_on_whatsapp(db).await?;
            let body2 = if on_whatsapp {
                let null = Value::Null;
                let full = Box::pin(super::cards::render(
                    db,
                    super::cards::CardView {
                        action: action_name,
                        payload: &clean,
                        diff: &diff,
                        price: opts.price.as_ref().unwrap_or(&null),
                        rationale: opts.rationale,
                    },
                ))
                .await?;
                format!("{full}\n\n{}", commands::CARD_FOOTER)
            } else {
                body.clone()
            };
            notify::notify(
                db,
                &approvers,
                Notice::new(&title, &body2, &link, "nary_action", &card_id)
                    .type_("nary_card")
                    .email(false)
                    .whatsapp(on_whatsapp),
            )
            .await?;
            if on_whatsapp {
                commands::set_prompt(db, &json!({"kind": "card", "card_id": card_id})).await?;
            }
        }
        super::commit(db).await?;
        return Ok(row);
    }
    if level == 0 {
        super::commit(db).await?;
        return Ok(row);
    }
    // Levels 2 and 3: apply now, acting as `actor`.
    let row_id = row.id;
    super::commit(db).await?;
    _apply(
        db,
        row_id,
        actor,
        &clean,
        level == 2 && opts.origin == "nary",
        true,
    )
    .await?;
    must_card(db, row_id).await
}

/// `exc.detail if isinstance(exc, HTTPException) else governance.plain_error(exc)`.
fn reason_of(exc: &ApiError) -> String {
    if is_http_exception(exc) {
        http_detail(exc).unwrap_or_default()
    } else {
        governance::plain_error(exc)
    }
}

/// The Owner told Nary to send this final text (the chat's "send it", confirmed on its card). It is the
/// Owner's own decision, so it is applied at once as the Owner, past shadow mode, and kept on the Approvals
/// page (origin "chat"). Only reply sends; the send rules of the channel still apply. The caller checks
/// nary:approve (routers/nary.py /send).
pub async fn send_on_instruction(
    db: &mut PgConnection,
    user: &AuthUser,
    action_name: &str,
    payload: Value,
    rationale: Option<&str>,
) -> ApiResult<AgentPendingAction> {
    if !matches!(action_name, "email.reply.send" | "whatsapp.reply.send") {
        return Err(ApiError::http(
            400,
            "Only email and WhatsApp messages can be sent this way",
        ));
    }
    let action = registry::get(action_name)?;
    let mut p = match payload {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    p.insert("by_owner".into(), Value::Bool(true));
    let clean = action.validate(&Value::Object(p))?;
    action.check_permission(Some(user))?;
    let (entity_type, entity_id) = (action.entity)(&clean)?;
    let text = clean["text"].as_str().unwrap_or_default().to_string();
    let diff = match &action.describe {
        Some(d) => d(db, Some(user), &clean).await?,
        None => json!({"label": py_slice(&text, 120), "after": {"text": text}}),
    };
    let now = Utc::now();
    let row = insert_card(
        db,
        NewCard {
            action: action_name.into(),
            origin: "chat".into(),
            level: 1,
            customer_facing: action.customer_facing,
            payload: clean.clone(),
            rationale: Some(
                rationale
                    .filter(|r| !r.is_empty())
                    .unwrap_or("Sent on the Owner's instruction in the chat.")
                    .to_string(),
            ),
            diff: Some(diff),
            entity_type,
            entity_id,
            actor_id: Some(user.id),
            assignee_id: Some(user.id),
            decided_by_id: Some(user.id),
            decided_at: Some(now),
            status: "applying".into(),
            ..Default::default()
        },
    )
    .await?;
    let row_id = row.id;
    super::commit(db).await?;
    if let Err(exc) = _apply(db, row_id, Some(user), &clean, false, false).await {
        // The Owner asked for this send: every failure is told with its reason (a refusal included), and the
        // card shows that reason, never a bare server error.
        let reason = reason_of(&exc);
        let truthy_str = |k: &str| clean.get(k).filter(|v| py_truthy(v)).map(py_str);
        let mut who = truthy_str("chat_id")
            .or_else(|| truthy_str("to"))
            .or_else(|| truthy_str("contact_id"))
            .unwrap_or_default();
        if let Some(chat_id) = truthy_str("chat_id") {
            let chat: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT chat_id, display_name, phone FROM wa_chats WHERE chat_id = $1",
            )
            .bind(&chat_id)
            .fetch_optional(&mut *db)
            .await?;
            if let Some((cid, name, phone)) = chat {
                let shown = name
                    .filter(|s| !s.is_empty())
                    .or(phone.filter(|s| !s.is_empty()));
                if let Some(shown) = shown {
                    who = format!("{shown} ({cid})");
                }
            }
        }
        let channel = if action_name.starts_with("whatsapp") {
            "WhatsApp"
        } else {
            "email"
        };
        let title = format!(
            "Nary couldn't send your {channel}{}",
            if who.is_empty() {
                String::new()
            } else {
                format!(" to {who}")
            }
        );
        governance::error_card(
            db,
            &format!("send:{action_name}:{}", _fingerprint(&[&who, &reason])),
            &title,
            &format!("{reason}\nWhat was to go out: {}", py_slice(&text, 300)),
            &format!("/nary/approvals?card={row_id}"),
        )
        .await?;
        if is_http_exception(&exc) {
            return Err(exc);
        }
        return Err(ApiError::http(502, format!("Not sent: {reason}")));
    }
    must_card(db, row_id).await
}

// batch 2 T3: a customer message the Owner wants sent at a set time (A5).

/// The Owner told Nary to send this final text at `send_at` (propose_customer_message with send_at, confirmed):
/// kept as a "scheduled" card that [`release_timed`] sends when the moment comes. Same checks as
/// [`send_on_instruction`]; the send rules of the channel apply when it goes out. Commits.
pub async fn schedule_on_instruction(
    db: &mut PgConnection,
    user: &AuthUser,
    action_name: &str,
    payload: Value,
    send_at: &super::scheduled_sends::SendAt,
) -> ApiResult<AgentPendingAction> {
    if !matches!(action_name, "email.reply.send" | "whatsapp.reply.send") {
        return Err(ApiError::http(
            400,
            "Only email and WhatsApp messages can be sent this way",
        ));
    }
    let action = registry::get(action_name)?;
    let mut p = match payload {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    p.insert("by_owner".into(), Value::Bool(true));
    let clean = action.validate(&Value::Object(p))?;
    action.check_permission(Some(user))?;
    let (entity_type, entity_id) = (action.entity)(&clean)?;
    let text = clean["text"].as_str().unwrap_or_default().to_string();
    let diff = match &action.describe {
        Some(d) => d(db, Some(user), &clean).await?,
        None => json!({"label": py_slice(&text, 120), "after": {"text": text}}),
    };
    let row = insert_card(
        db,
        NewCard {
            action: action_name.into(),
            origin: "chat".into(),
            level: 1,
            customer_facing: action.customer_facing,
            payload: clean.clone(),
            rationale: Some(format!(
                "Scheduled on the Owner's instruction in the chat, to go out {}.",
                send_at.label
            )),
            diff: Some(diff.clone()),
            entity_type,
            entity_id,
            actor_id: Some(user.id),
            assignee_id: Some(user.id),
            decided_by_id: Some(user.id),
            decided_at: Some(Utc::now()),
            status: "scheduled".into(),
            ..Default::default()
        },
    )
    .await?;
    let channel = if action_name.starts_with("whatsapp") {
        "WhatsApp"
    } else {
        "email"
    };
    let what = diff_label_truthy(&diff).unwrap_or_else(|| py_slice(&text, 80).to_string());
    let result = json!({"send_at": true,
                        "scheduled_for": PyDateTime::from_chrono_utc(&send_at.at).isoformat(),
                        "when": send_at.label,
                        "label": format!("Scheduled for {}: {channel} ({what})", send_at.label)});
    sqlx::query("UPDATE agent_pending_actions SET result = $2, updated_at = now() WHERE id = $1")
        .bind(row.id)
        .bind(&result)
        .execute(&mut *db)
        .await?;
    super::commit(db).await?;
    tracing::info!(card_id = %row.id, action = %action_name, until = %send_at.label, "nary_timed_send_scheduled");
    must_card(db, row.id).await
}

/// Send the timed messages whose moment has come (the Owner's instruction: no quiet hours, no per-chat limit;
/// the pre-send check and the time greeting of the moment apply). A failure is told to the Owner with its
/// reason. Commits.
pub async fn release_timed(db: &mut PgConnection) -> ApiResult<Released> {
    let mut out = Released::default();
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM agent_pending_actions WHERE status = 'scheduled' AND (result ->> 'send_at') = 'true' \
         AND (result ->> 'scheduled_for') IS NOT NULL \
         ORDER BY (result ->> 'scheduled_for'), created_at",
    )
    .fetch_all(&mut *db)
    .await?;
    let now = Utc::now();
    for row_id in ids {
        let Some(row) = get_card(db, row_id).await? else {
            continue;
        };
        let due = row
            .result
            .as_ref()
            .and_then(|r| r.get("scheduled_for"))
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc));
        if row.status != "scheduled" || due.is_none_or(|d| d > now) {
            continue;
        }
        let payload = row
            .edited_payload
            .clone()
            .filter(py_truthy)
            .unwrap_or_else(|| row.payload.clone());
        let mut actor = match row.actor_id {
            Some(id) => roles::load_actor(db, id).await?,
            None => None,
        };
        if actor.is_none() {
            if let Some(first) = roles::approver_ids(db).await?.first() {
                actor = roles::load_actor(db, *first).await?;
            }
        }
        let Some(actor) = actor else { continue };
        if !claim_scheduled(db, row_id).await? {
            continue;
        }
        _close_topics(db, row_id).await?;
        super::commit(db).await?;
        if let Err(exc) =
            wa_send::as_owner(_apply(db, row_id, Some(&actor), &payload, false, false)).await
        {
            let reason = reason_of(&exc);
            let truthy_str = |k: &str| payload.get(k).filter(|v| py_truthy(v)).map(py_str);
            let who = truthy_str("chat_id")
                .or_else(|| truthy_str("to"))
                .unwrap_or_default();
            let channel = if row.action.starts_with("whatsapp") {
                "WhatsApp"
            } else {
                "email"
            };
            governance::error_card(
                db,
                &format!("timed:{}:{}", row.action, _fingerprint(&[&who, &reason])),
                &format!("Nary couldn't send your scheduled {channel} to {who}"),
                &format!(
                    "{reason}\nWhat was to go out: {}",
                    py_slice(payload["text"].as_str().unwrap_or_default(), 300)
                ),
                &format!("/nary/approvals?card={row_id}"),
            )
            .await?;
            out.failed += 1;
            continue;
        }
        match must_card(db, row_id).await?.status.as_str() {
            "applied" => out.sent += 1,
            "cancelled" | "expired" => out.expired += 1,
            _ => {}
        }
    }
    Ok(out)
}

/// Actions a person can have Nary do from the chat (a card confirmed there, or at once at Act and tell).
pub const CHAT_ACTIONS: &[&str] = &[
    "meeting.add_guests",
    "meeting.cancel",
    "contact.merge",
    // batch 2 T1: Solution Proposals from the chat
    "proposal.from_chat",
    "proposal.update_requirements",
    // batch 2 T3: cancel a timed customer message
    "message.cancel_scheduled",
    // batch 2 T2: CRM hygiene (merge organizations, delete a contact)
    "organization.merge",
    "contact.delete",
    // R&D 26 Oct 2026: a deal entered twice; a typed-in activity deleted; an email thread linked
    "deal.merge",
    "activity.void",
    "email.link",
    // batch 4 D3: change a draft quotation
    "quotation.update",
    // doclib A3: send a library document to a customer
    "document.send",
    // visuals V3: proposal figures and "Ask Nary to change"
    "proposal.visual",
    "proposal.edit",
    // fix-longapply: revise a proposal, reject a draft
    "proposal.revise",
    "proposal.reject",
    // 22 Oct 2026: link a quotation to a proposal
    "proposal.quotation",
    // 23 Oct 2026: a proposal written from the given sections
    "proposal.from_sections",
    // 23 Oct 2026: catalog changes (the Owner only)
    "catalog.change",
    // 1 Nov 2026: reprice a draft quotation to today, withdraw one from approval, cancel one
    "quotation.reprice",
    "quotation.withdraw",
    "quotation.cancel",
    // 2 Nov 2026: a new revision of a sent or expired quotation
    "quotation.revision",
    // 5 Nov 2026 (Stream C): link a quotation to a deal without a revision
    "quotation.link_deal",
    // 3 Nov 2026 (Stream B1): Nary's WhatsApp and lead work by hand while Nary is paused
    "lead.merge",
    "lead.from_whatsapp",
    // 5 Nov 2026: propose_lead_promote
    "lead.promote",
    "chat.held_decide",
    // 3 Nov 2026 (Stream B2): deal close-out by hand while Nary is paused
    "contract.create_draft",
    "proforma.raise",
    "deal.won",
    "handoff.kickoff",
    // 5 Nov 2026 (Stream A): a product file added by the Owner's card (propose_product_file)
    "catalog.product_file",
];

/// A person told Nary to do this in the chat and confirmed it there (or its autonomy level acts at once).
/// Applied as that person (the action's permission applies) and kept on the Approvals page (origin "chat").
/// A failure is raised with its reason and told to the Owner.
pub async fn run_on_instruction(
    db: &mut PgConnection,
    user: &AuthUser,
    action_name: &str,
    payload: Value,
    rationale: Option<&str>,
) -> ApiResult<AgentPendingAction> {
    if !CHAT_ACTIONS.contains(&action_name) {
        return Err(ApiError::http(
            400,
            format!("{action_name} can't be run from the chat"),
        ));
    }
    let action = registry::get(action_name)?;
    let clean = action.validate(&payload)?;
    action.check_permission(Some(user))?;
    let (entity_type, entity_id) = (action.entity)(&clean)?;
    let diff = match &action.describe {
        Some(d) => d(db, Some(user), &clean).await?,
        None => json!({"label": action.label}),
    };
    let wait = super::long_work::wait_limit(db).await?; // fix-longapply
    let now = Utc::now();
    let rationale = rationale
        .filter(|r| !r.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Done on {}'s instruction in the chat.", user.name));
    let row = insert_card(
        db,
        NewCard {
            action: action_name.into(),
            origin: "chat".into(),
            level: 1,
            customer_facing: action.customer_facing,
            payload: clean.clone(),
            rationale: Some(rationale),
            diff: Some(diff.clone()),
            entity_type,
            entity_id,
            actor_id: Some(user.id),
            assignee_id: Some(user.id),
            decided_by_id: Some(user.id),
            decided_at: Some(now),
            status: "applying".into(),
            ..Default::default()
        },
    )
    .await?;
    let row_id = row.id;
    super::commit(db).await?;
    // fix-longapply: the apply (and telling the Owner about a failure) runs on a task of its own; the request
    // waits for it only `nary.apply_wait_seconds`, then answers with the card still applying.
    let user_owned = user.clone();
    let what = diff_label_truthy(&diff).unwrap_or_else(|| action.label.clone());
    let (label, name) = (action.label.clone(), action_name.to_string());
    let work: super::long_work::BoxFut<ApiResult<()>> = Box::pin(async move {
        match _apply_own(row_id, Some(&user_owned), &clean, false, false).await {
            Ok(()) => Ok(()),
            Err(exc) => Err(run_failed(row_id, &name, &label, &what, exc).await),
        }
    });
    if let super::long_work::Waited::Done(Err(e)) =
        super::long_work::wait_or_detach(wait, work).await
    {
        return Err(e);
    }
    must_card(db, row_id).await
}

/// A chat instruction failed: the Owner is told, and the error the caller sees.
async fn run_failed(
    row_id: Uuid,
    action_name: &str,
    label: &str,
    what: &str,
    exc: ApiError,
) -> ApiError {
    let reason = reason_of(&exc);
    let told = async {
        let pool = super::session_pool()?;
        let mut own = pool.acquire().await?;
        governance::error_card(
            &mut own,
            &format!("chat:{action_name}:{}", _fingerprint(&[&reason])),
            &format!("Nary couldn't {}", pystr::lower(label)),
            &format!("{what}: {reason}"),
            &format!("/nary/approvals?card={row_id}"),
        )
        .await
    }
    .await;
    if let Err(e) = told {
        return e;
    }
    if is_http_exception(&exc) {
        return exc;
    }
    ApiError::http(502, format!("Not done: {reason}"))
}

/// `QuietHours.until` / `.local_label`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quiet {
    pub until: DateTime<Utc>,
    pub local_label: String,
}

const QUIET_PREFIX: &str = "Outside working hours: it goes out ";
const PACING_TEXTS: [&str; 2] = [
    "Nary's hourly WhatsApp limit is reached (nary.wa.max_per_hour).",
    "Nary's daily WhatsApp limit is reached (nary.wa.max_per_day).",
];

/// `isinstance(exc, wa_send.QuietHours)` on an action's error: its label, when it is one.
pub fn quiet_label(exc: &ApiError) -> Option<String> {
    if http_status(exc) != Some(409) {
        return None;
    }
    let d = http_detail(exc)?;
    d.strip_prefix(QUIET_PREFIX)?
        .strip_suffix('.')
        .map(str::to_string)
}

/// `isinstance(exc, wa_send.PacingLimit)` on an action's error.
pub fn is_pacing_limit(exc: &ApiError) -> bool {
    http_status(exc) == Some(429)
        && http_detail(exc).is_some_and(|d| PACING_TEXTS.contains(&d.as_str()))
}

/// Nary's own WhatsApp message outside working hours: kept for the next working period, and the Owner is told
/// (1 sends it now, or drop it). Commits.
pub async fn _schedule(
    db: &mut PgConnection,
    row_id: Uuid,
    action: &Action,
    exc: &Quiet,
) -> ApiResult<()> {
    let row = must_card(db, row_id).await?;
    let result = json!({"scheduled_for": PyDateTime::from_chrono_utc(&exc.until).isoformat(), "when": exc.local_label});
    sqlx::query(
        "UPDATE agent_pending_actions SET status = 'scheduled', error = NULL, result = $2, updated_at = now() WHERE id = $1",
    )
    .bind(row_id)
    .bind(&result)
    .execute(&mut *db)
    .await?;
    // C6: the whole message, not its cut label.
    let label = Box::pin(super::cards::render_card(db, &row)).await?;
    let text = format!(
        "{label}\n\nIt goes out {} (outside working hours). Reply 1 to send it now, or tell me to drop it.",
        exc.local_label
    );
    let tag = format!("Waiting for the morning: {}", action.label);
    if owner_channel::enabled(db).await? {
        let mut t = owner_channel::Topic::new("approval", &tag, &text);
        t.ref_type = Some("nary_card".into());
        t.ref_id = Some(row_id);
        t.numbered = Some(json!({"kind": "card", "card_id": row_id.to_string()}));
        t.facts = Some(
            json!({"action": row.action, "card_id": row_id.to_string(), "scheduled_for": exc.local_label}),
        );
        owner_channel::raise_topic(db, t).await?;
    } else {
        let approvers = roles::approver_ids(db).await?;
        let link = format!("/nary/approvals?card={row_id}");
        let id = row_id.to_string();
        notify::notify(
            db,
            &approvers,
            Notice::new(&tag, py_slice(&text, 2000), &link, "nary_action", &id)
                .type_("nary_card")
                .priority("low")
                .email(false),
        )
        .await?;
    }
    super::commit(db).await?;
    tracing::info!(card_id = %row_id, action = %row.action,
                   until = %PyDateTime::from_chrono_utc(&exc.until).isoformat(), "nary_send_scheduled");
    Ok(())
}

/// A WhatsApp message sent from inside another action (a booking's confirmation) met quiet hours: it becomes
/// a scheduled whatsapp.reply.send card of its own. Commits. `exc` is the QuietHours the send raised.
pub async fn schedule_message(
    db: &mut PgConnection,
    payload: Value,
    exc: &SendError,
    rationale: &str,
) -> ApiResult<AgentPendingAction> {
    let action = registry::get("whatsapp.reply.send")?;
    let mut p = match payload {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    p.insert("channel".into(), json!("whatsapp"));
    let clean = action.validate(&Value::Object(p))?;
    let approvers = roles::approver_ids(db).await?;
    let diff = super::channel_actions::_describe_text(db, None, &clean).await?;
    let row = insert_card(
        db,
        NewCard {
            action: action.name.clone(),
            origin: "nary".into(),
            level: 2,
            customer_facing: true,
            payload: clean,
            rationale: Some(rationale.to_string()),
            diff: Some(diff),
            actor_id: approvers.first().copied(),
            assignee_id: approvers.first().copied(),
            status: "scheduled".into(),
            ..Default::default()
        },
    )
    .await?;
    let quiet = match exc {
        SendError::QuietHours { until, local_label } => Quiet {
            until: *until,
            local_label: local_label.clone(),
        },
        _ => {
            return Err(ApiError::internal(governance::PyException::new(
                "AttributeError",
                "object has no attribute 'until'",
            )))
        }
    };
    _schedule(db, row.id, &action, &quiet).await?;
    must_card(db, row.id).await
}

/// The result of [`release_scheduled`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Released {
    pub sent: i64,
    pub expired: i64,
    pub failed: i64,
}

impl Released {
    pub fn to_json(&self) -> Value {
        json!({"sent": self.sent, "expired": self.expired, "failed": self.failed})
    }
}

/// In working hours: send the messages that waited for the morning, oldest first. A reminder for a meeting
/// that has started is dropped; a pacing limit stops the pass (the rest wait for the next tick).
pub async fn release_scheduled(db: &mut PgConnection) -> ApiResult<Released> {
    let mut out = Released::default();
    if governance::is_paused(db).await? {
        return Ok(out);
    }
    let ids: Vec<(Uuid,)> = sqlx::query_as(
        // batch 2 T3: a message timed by the Owner waits for its own moment (release_timed)
        "SELECT id FROM agent_pending_actions WHERE status = 'scheduled' \
         AND (result ->> 'send_at') IS DISTINCT FROM 'true' ORDER BY created_at",
    )
    .fetch_all(&mut *db)
    .await?;
    // Fix batch 1 (B1): grouped by chat. Of the messages that waited for one chat, only the newest goes; the
    // older ones are cancelled (one message per chat), and the pre-send check runs on the one that goes.
    let ids =
        super::reply_guard::group_released(db, ids.into_iter().map(|r| r.0).collect()).await?;
    super::commit(db).await?;
    let now = Utc::now();
    for row_id in ids {
        let Some(row) = get_card(db, row_id).await? else {
            continue;
        };
        if row.status != "scheduled" {
            continue;
        }
        let payload = row
            .edited_payload
            .clone()
            .filter(py_truthy)
            .unwrap_or_else(|| row.payload.clone());
        if row.action == "meeting.reminder.send" {
            let meeting_id = registry::to_uuid(&payload["meeting_id"])?;
            let meeting: Option<(String, Option<DateTime<Utc>>)> =
                sqlx::query_as("SELECT status, starts_at FROM nary_meetings WHERE id = $1")
                    .bind(meeting_id)
                    .fetch_optional(&mut *db)
                    .await?;
            let gone = match &meeting {
                None => true,
                Some((status, starts_at)) => {
                    status != "booked" || starts_at.is_some_and(|s| s <= now)
                }
            };
            if gone {
                set_status(db, row_id, "expired").await?;
                _close_topics(db, row_id).await?;
                super::commit(db).await?;
                out.expired += 1;
                continue;
            }
        }
        let mut actor = match row.actor_id {
            Some(id) => roles::load_actor(db, id).await?,
            None => None,
        };
        if actor.is_none() {
            let approvers = roles::approver_ids(db).await?;
            if let Some(first) = approvers.first() {
                actor = roles::load_actor(db, *first).await?;
            }
        }
        let Some(actor) = actor else { continue };
        let row = must_card(db, row_id).await?;
        if !claim_scheduled(db, row_id).await? {
            continue;
        }
        _close_topics(db, row_id).await?;
        let level = row.level;
        super::commit(db).await?;
        match _apply(db, row_id, Some(&actor), &payload, level == 2, true).await {
            Ok(()) => {}
            Err(e) if is_http_exception(&e) => {
                if http_status(&e) == Some(429) {
                    // Nary's hourly or daily limit: the rest wait
                    sqlx::query(
                        "UPDATE agent_pending_actions SET status = 'scheduled', error = NULL, updated_at = now() WHERE id = $1",
                    )
                    .bind(row_id)
                    .execute(&mut *db)
                    .await?;
                    super::commit(db).await?;
                    break;
                }
                out.failed += 1;
                continue;
            }
            Err(_) => {
                // already marked failed and alerted by _apply
                out.failed += 1;
                continue;
            }
        }
        let status = must_card(db, row_id).await?.status;
        if status == "applied" {
            out.sent += 1;
        } else if status == "scheduled" {
            // held back by Nary's WhatsApp limit: the rest wait too
            break;
        }
    }
    Ok(out)
}

/// A short key for one problem, so the same failure is told once (dedupe) and a different one is not muted.
pub fn _fingerprint(parts: &[&str]) -> String {
    let joined: Vec<&str> = parts.iter().map(|p| py_slice(p, 200)).collect();
    let digest = Sha1::digest(joined.join("|").as_bytes());
    hex::encode(digest)[..12].to_string()
}

/// Apply in a session of its own. A failure rolls that session back, and a rollback expires every record the
/// session holds: done in the caller's session it wiped the caller's records (14 Oct: a reply refused by the
/// hourly limit made the inbound WhatsApp handler fail with MissingGreenlet on its own chat row, five retries,
/// each one calling the model). The caller re-reads the card for the outcome. What the action notified by
/// email/WhatsApp moves to the caller's outbox once committed: the request or job that called delivers it after
/// its commit, as with any other outbox (it was lost with this session before).
pub async fn _apply(
    _db: &mut PgConnection,
    row_id: Uuid,
    actor: Option<&AuthUser>,
    payload: &Value,
    tell: bool,
    alert: bool,
) -> ApiResult<()> {
    _apply_own(row_id, actor, payload, tell, alert).await
}

/// fix-longapply: [`_apply`] without the caller's connection (it never used it), so a detached task can run
/// it. The card's Redis lease is held while it works (the stuck-card watchdog, [`fail_stuck`]).
pub async fn _apply_own(
    row_id: Uuid,
    actor: Option<&AuthUser>,
    payload: &Value,
    tell: bool,
    alert: bool,
) -> ApiResult<()> {
    // boxed: the apply's future stays off the caller's stack
    let work: futures::future::BoxFuture<'_, ApiResult<()>> =
        Box::pin(_apply_leased(row_id, actor, payload, tell, alert));
    super::long_work::with_card_lease_once(row_id, work).await
}

async fn _apply_leased(
    row_id: Uuid,
    actor: Option<&AuthUser>,
    payload: &Value,
    tell: bool,
    alert: bool,
) -> ApiResult<()> {
    let pool = super::session_pool()?;
    // The own session's notification outbox: a nested scope, moved to the caller's scope on success.
    let (out, moved) = notify::scope(async {
        let out = async {
            let mut own = pool.begin().await?;
            // what the card sends is recorded against it (the pre-send check reads it)
            let out = wa_send::sending_card(
                row_id,
                _apply_in(&mut own, row_id, actor, payload, tell, alert),
            )
            .await;
            drop(own); // closes (rolls back anything uncommitted), as `async with` does
            out
        }
        .await;
        if out.is_err() {
            notify::discard_outbox();
            (out, vec![])
        } else {
            (out, notify::take_outbox())
        }
    })
    .await;
    notify::move_outbox(moved);
    out
}

async fn _apply_in(
    db: &mut PgConnection,
    row_id: Uuid,
    actor: Option<&AuthUser>,
    payload: &Value,
    tell: bool,
    alert: bool,
) -> ApiResult<()> {
    let action_row = must_card(db, row_id).await?;
    let action = registry::get(&action_row.action)?;
    // R&D 26 Oct (double sends): only a card this run moved to applying is applied; one already applied,
    // cancelled or sent back (a second run, a redelivered job) is left alone.
    if action_row.status != "applying" {
        tracing::warn!(card_id = %row_id, status = %action_row.status, action = %action_row.action,
                       "nary_apply_skipped_not_applying");
        return Ok(());
    }
    // Fix batch 1 (B1/B2): the pre-send check, right before anything goes to a customer (approved, automatic
    // or released after quiet hours): already answered -> cancelled with the reason; over the per-chat limit
    // or in a paused chat -> held for the Owner.
    // Security 27 Oct (#14): a per-customer lock (until this apply commits or rolls back) around the checks
    // below and the send with its log row: two sends to one customer at the same moment go one after the
    // other, and the second one's checks see the first.
    if action.customer_facing {
        super::outbound_log::lock_customer(db, payload).await?;
    }
    let regreeted;
    let payload = if action.customer_facing {
        match super::reply_guard::before_send(db, &action_row, payload).await? {
            super::reply_guard::Gate::Go(None) => payload,
            super::reply_guard::Gate::Go(Some(p)) => {
                regreeted = p;
                &regreeted
            }
            super::reply_guard::Gate::Cancel(reason) => {
                super::reply_guard::cancel_card(db, row_id, &reason).await?;
                super::commit(db).await?;
                return Ok(());
            }
            super::reply_guard::Gate::Hold(reason) => {
                super::reply_guard::hold_card(db, row_id, &reason).await?;
                super::commit(db).await?;
                return Ok(());
            }
        }
    } else {
        payload
    };
    // R&D 26 Oct (double sends): an automatic message waits for the Owner when another automatic one reached
    // the customer within the gap (or already answered their last message), or when it carries Bcc.
    if action.customer_facing {
        if let Some(reason) = super::outbound_log::autosend_gate(db, &action_row, payload).await? {
            tracing::info!(card_id = %row_id, action = %action_row.action, reason = %reason, "nary_autosend_held");
            super::reply_guard::hold_card(db, row_id, &reason).await?;
            super::commit(db).await?;
            return Ok(());
        }
    }
    let attempt: ApiResult<Value> = async {
        action.check_permission(actor)?;
        if action.customer_facing && !registry::CUSTOMER_SENDING_ENABLED {
            return Err(registry::CustomerSendingDisabled(&action.name));
        }
        // 3 Nov 2026: a close-out card from the chat or the MCP client runs as the person's request
        match request_origin(&action_row) {
            Some(origin) => on_request(origin, (action.apply)(db, actor, payload)).await,
            None => (action.apply)(db, actor, payload).await,
        }
    }
    .await;
    let result = match attempt {
        Ok(r) => r,
        Err(exc) => {
            super::rollback(db).await?;
            notify::discard_outbox(); // what the failed action queued was rolled back with it
            let action_row = must_card(db, row_id).await?;
            if let Some(label) = quiet_label(&exc) {
                if !wa_send::owner_decision() {
                    let (until, _) = wa_send::quiet_until(db).await?;
                    _schedule(
                        db,
                        row_id,
                        &action,
                        &Quiet {
                            until,
                            local_label: label,
                        },
                    )
                    .await?;
                    return Ok(());
                }
            }
            if is_pacing_limit(&exc) && !wa_send::owner_decision() {
                // Nary's own message over its hourly or daily limit: it waits for the next tick, not lost.
                sqlx::query(
                    "UPDATE agent_pending_actions SET status = 'scheduled', error = NULL, result = $2, \
                     updated_at = now() WHERE id = $1",
                )
                .bind(row_id)
                .bind(json!({"when": "when Nary's WhatsApp limit allows", "pacing": true}))
                .execute(&mut *db)
                .await?;
                super::commit(db).await?;
                tracing::info!(card_id = %row_id, action = %action_row.action, "nary_send_paced");
                return Ok(());
            }
            let error = py_slice(&reason_of(&exc), 2000).to_string();
            sqlx::query("UPDATE agent_pending_actions SET status = 'failed', error = $2, updated_at = now() WHERE id = $1")
                .bind(row_id)
                .bind(&error)
                .execute(&mut *db)
                .await?;
            super::commit(db).await?;
            if alert && (!is_http_exception(&exc) || http_status(&exc).unwrap_or(0) >= 500) {
                governance::error_card(
                    db,
                    &format!(
                        "action:{}:{}",
                        action.name,
                        _fingerprint(&[py_slice(&error, 80)])
                    ),
                    &format!("Nary couldn't {}", pystr::lower(&action.label)),
                    &error,
                    &format!("/nary/approvals?card={row_id}"),
                )
                .await?;
            }
            return Err(exc);
        }
    };
    let action_row = must_card(db, row_id).await?;
    let result = super::runs::jsonable(&result)?;
    sqlx::query("UPDATE agent_pending_actions SET status = 'applied', result = $2, updated_at = now() WHERE id = $1")
        .bind(row_id)
        .bind(&result)
        .execute(&mut *db)
        .await?;
    if tell {
        let approvers = roles::approver_ids(db).await?;
        let body = action_row
            .diff
            .as_ref()
            .and_then(diff_label_truthy)
            .unwrap_or_default();
        let title = format!("Nary did: {}", action.label);
        let link = format!("/nary/approvals?card={row_id}");
        let id = row_id.to_string();
        notify::notify(
            db,
            &approvers,
            Notice::new(&title, py_slice(&body, 500), &link, "nary_action", &id)
                .type_("nary_done")
                .priority("low")
                .email(false),
        )
        .await?;
    }
    super::commit(db).await?;
    Ok(())
}

/// Apply (optionally edited) or reject a pending card. The decider's permissions apply.
pub async fn decide(
    db: &mut PgConnection,
    card_id: Uuid,
    user: &AuthUser,
    decision: &str,
    edited_payload: Option<Value>,
    reason: Option<&str>,
) -> ApiResult<AgentPendingAction> {
    if !matches!(decision, "apply" | "reject") {
        return Err(ApiError::http(400, "decision must be apply or reject"));
    }
    let row: Option<AgentPendingAction> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM agent_pending_actions WHERE id = $1 FOR UPDATE"
    ))
    .bind(card_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::http(404, "Card not found"));
    };
    // fix-longapply: a failed proposal card (drafting stopped half-way) can be applied again; the retry
    // reuses what the first try made (proposals::apply_from_chat).
    let retry = row.status == "failed" && decision == "apply" && retryable(&row.action);
    if !retry && !matches!(row.status.as_str(), "pending" | "scheduled") {
        return Err(ApiError::http(
            409,
            format!("This card is already {}.", row.status),
        ));
    }
    // Nary's own message waiting for the morning: not a draft to learn from
    let scheduled = row.status == "scheduled";
    let action = registry::get(&row.action)?;
    let now = Utc::now();
    sqlx::query("UPDATE agent_pending_actions SET decided_by_id = $2, decided_at = $3, updated_at = now() WHERE id = $1")
        .bind(row.id)
        .bind(user.id)
        .bind(now)
        .execute(&mut *db)
        .await?;
    _close_topics(db, row.id).await?;
    if decision == "reject" {
        let reason = pystr::strip(reason.unwrap_or_default()).to_string();
        if reason.is_empty() {
            return Err(ApiError::http(
                400,
                "Say why it is rejected (Nary learns from it).",
            ));
        }
        sqlx::query(
            "UPDATE agent_pending_actions SET status = 'rejected', reject_reason = $2, updated_at = now() WHERE id = $1",
        )
        .bind(row.id)
        .bind(&reason)
        .execute(&mut *db)
        .await?;
        if !scheduled {
            autonomy::record_decision(db, &row.action, "rejected", Decimal::ZERO).await?;
        }
        // R&D 26 Oct: "not one deal": the hold on the newer deal is lifted and the pair is never asked again
        if row.action == crate::crm::deal_duplicates::ACTION {
            if let (Ok(keep), Ok(gone)) = (
                registry::to_uuid(&row.payload["keep_id"]),
                registry::to_uuid(&row.payload["merge_id"]),
            ) {
                crate::crm::deal_duplicates::dismiss(db, keep, gone).await?;
            }
        }
        super::commit(db).await?;
        return must_card(db, row.id).await;
    }
    let mut payload = row.payload.clone();
    let mut ratio = Decimal::ZERO;
    if let Some(edited) = edited_payload {
        payload = action.validate(&edited)?;
        ratio = autonomy::edit_ratio(&row.payload, Some(&payload));
        sqlx::query(
            "UPDATE agent_pending_actions SET edited_payload = $2, edit_ratio = $3, updated_at = now() WHERE id = $1",
        )
        .bind(row.id)
        .bind(&payload)
        .bind(ratio)
        .execute(&mut *db)
        .await?;
    }
    if !scheduled && !retry {
        autonomy::record_decision(db, &row.action, "approved", ratio).await?;
    }
    // fix-longapply: the apply runs on a task of its own; the request waits for it only this long
    let wait = super::long_work::wait_limit(db).await?;
    if retry {
        sqlx::query("UPDATE agent_pending_actions SET error = NULL WHERE id = $1")
            .bind(row.id)
            .execute(&mut *db)
            .await?;
    }
    set_status(db, row.id, "applying").await?;
    let row_id = row.id;
    super::commit(db).await?;
    let user_owned = user.clone();
    // boxed: only a pointer lives in this future (debug builds overflow the stack otherwise)
    let work: super::long_work::BoxFut<ApiResult<()>> =
        Box::pin(async move { _apply_own(row_id, Some(&user_owned), &payload, false, true).await });
    // a person applied it: their instruction, at any hour
    match wa_send::as_owner(super::long_work::wait_or_detach(wait, work)).await {
        super::long_work::Waited::Done(r) => r?,
        super::long_work::Waited::Working => {
            tracing::info!(card_id = %row_id, "nary_apply_still_working");
        }
    }
    must_card(db, row_id).await
}

/// fix-longapply: the actions whose failed card can be applied again (the proposal work, which can stop
/// half-way when its connection is cut).
pub fn retryable(action: &str) -> bool {
    action.starts_with("proposal.")
}

/// fix-longapply: the stuck-card watchdog (worker beat, every 5 minutes). A card still `applying` after
/// `nary.applying_stale_minutes` whose apply is not running anywhere (no live lease) becomes `failed` with
/// the reason, and the Owner gets an error card. Redis unreachable: nothing is touched (a running apply can't
/// be told from a dead one). Returns how many cards failed. Commits.
pub async fn fail_stuck(db: &mut PgConnection) -> ApiResult<i64> {
    let minutes = require_int(db, "nary.applying_stale_minutes", WHERE).await?;
    let cutoff = Utc::now() - TimeDelta::minutes(minutes);
    let rows: Vec<(Uuid, String, Option<Value>)> = sqlx::query_as(
        "SELECT id, action, diff FROM agent_pending_actions WHERE status = 'applying' AND updated_at < $1 \
         ORDER BY updated_at LIMIT 200",
    )
    .bind(cutoff)
    .fetch_all(&mut *db)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let Some(live) = super::long_work::running(&ids).await else {
        tracing::warn!(cards = ids.len(), "nary_stuck_cards_unchecked");
        return Ok(0);
    };
    let mut n = 0;
    for (id, action, diff) in rows {
        if live.contains(&id) {
            continue;
        }
        let done = sqlx::query(
            "UPDATE agent_pending_actions SET status = 'failed', error = $2, updated_at = now() \
             WHERE id = $1 AND status = 'applying'",
        )
        .bind(id)
        .bind(super::long_work::STUCK_REASON)
        .execute(&mut *db)
        .await?
        .rows_affected();
        super::commit(db).await?;
        if done == 0 {
            continue;
        }
        n += 1;
        tracing::warn!(card_id = %id, action = %action, "nary_card_stuck_failed");
        let label = registry::get(&action)
            .map(|a| a.label.clone())
            .unwrap_or_else(|_| action.clone());
        let what = diff
            .as_ref()
            .and_then(diff_label_truthy)
            .unwrap_or_else(|| label.clone());
        governance::error_card(
            db,
            &format!("stuck:{id}"),
            &format!("Nary's work stopped: {}", pystr::lower(&label)),
            &format!("{what}: {}", super::long_work::STUCK_REASON),
            &format!("/nary/approvals?card={id}"),
        )
        .await?;
    }
    Ok(n)
}

/// A decided card's WhatsApp topic is done (and an unsent one is never sent).
pub async fn _close_topics(db: &mut PgConnection, card_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "UPDATE nary_owner_topics SET status = 'closed', updated_at = now() WHERE nary_owner_topics.ref_type = 'nary_card' \
         AND nary_owner_topics.ref_id = $1 AND nary_owner_topics.status IN ('queued', 'open', 'waiting')",
    )
    .bind(card_id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// Batch apply ("1 2 3"): each card on its own, so one failure doesn't stop the rest.
pub async fn decide_many(
    db: &mut PgConnection,
    card_ids: &[Uuid],
    user: &AuthUser,
) -> ApiResult<Value> {
    let (mut applied, mut failed, mut working) = (vec![], vec![], vec![]);
    for cid in card_ids {
        match decide(db, *cid, user, "apply", None, None).await {
            // fix-longapply: past the wait the card is still applying in the background: not "applied" yet
            Ok(row) if row.status == "applying" => working.push(json!(cid.to_string())),
            Ok(_) => applied.push(json!(cid.to_string())),
            Err(e) if is_http_exception(&e) => {
                let detail = match &e {
                    ApiError::Http { detail, .. } => detail.clone(),
                    _ => Value::Null,
                };
                failed.push(json!({"id": cid.to_string(), "error": detail}));
            }
            Err(e) => return Err(e),
        }
    }
    let mut out = json!({"applied": applied, "failed": failed});
    if !working.is_empty() {
        out["working"] = json!(working);
    }
    Ok(out)
}

/// Cards nobody decided within nary.pending.expire_days expire (a stale draft is worse than none).
pub async fn expire_stale(db: &mut PgConnection) -> ApiResult<i64> {
    let days = require_int(db, "nary.pending.expire_days", WHERE).await?;
    let cutoff = Utc::now() - TimeDelta::days(days);
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM agent_pending_actions WHERE status = 'pending' AND created_at < $1",
    )
    .bind(cutoff)
    .fetch_all(&mut *db)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    if !ids.is_empty() {
        sqlx::query("UPDATE agent_pending_actions SET status = 'expired', updated_at = now() WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *db)
            .await?;
    }
    super::commit(db).await?;
    Ok(ids.len() as i64)
}

/// Shadow mode: record what a person actually sent and how far it was from Nary's draft (%).
pub async fn compare(
    db: &mut PgConnection,
    card_id: Uuid,
    actual_text: Option<&str>,
) -> ApiResult<Decimal> {
    let row = get_card(db, card_id).await?;
    let Some(row) = row.filter(|r| r.status == "shadow") else {
        return Err(ApiError::http(404, "Shadow draft not found"));
    };
    let draft = match row.payload.get("text") {
        Some(v) if py_truthy(v) => py_str(v),
        _ => String::new(),
    };
    let actual = actual_text.unwrap_or_default();
    let ratio = difflib::edit_pct(&draft, actual);
    sqlx::query(
        "UPDATE agent_pending_actions SET actual_text = $2, diff_ratio = $3, compared_at = $4, updated_at = now() WHERE id = $1",
    )
    .bind(card_id)
    .bind(actual_text)
    .bind(ratio)
    .bind(Utc::now())
    .execute(&mut *db)
    .await?;
    super::commit(db).await?;
    Ok(ratio)
}
