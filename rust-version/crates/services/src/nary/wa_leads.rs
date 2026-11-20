//! 3 Nov 2026 (Owner decisions 3 Nov, Stream B1): the MCP client and the internal chat do internal Nary's
//! WhatsApp and lead work by hand while `nary.paused` stays on. Internal Nary never starts this itself: every
//! card here is raised by a person (origin chat, or an mcp.proposal card), and the code paths reused from
//! Nary skip the pause only for that origin.
//!
//! ```text
//!     propose_whatsapp_lead        card lead.from_whatsapp: a lead from a WhatsApp chat, as channels.rs does
//!                                  for a new prospect: identity first (the chat's contact, the number's
//!                                  contact, a certain identity match); a new number gets a contact and a
//!                                  lead (channels::create_intake_by) and the promotion card; a returning
//!                                  contact with no open lead or deal gets a lead (email_intake::open_lead_by)
//!                                  and the chat joins it. Refused when the chat (or its person) already has an
//!                                  open lead or deal (named). Optional name, organization, notes.
//!     propose_held_chat_decision   card chat.held_decide: the Owner's business-or-personal answer for a held
//!                                  chat (channels::decide_held). Personal deletes what was stored: on the
//!                                  Owner's full-power connection it waits for nary_apply.
//!     propose_lead_merge           the existing lead.merge action as a card (GATED at full power).
//!     score_lead                   BANT score of a lead (nary::scoring::assess); save=true stores it as Nary
//!                                  does (nary::scoring::store), at act and full power only (ACT_ARGS).
//! ```

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::crm::{Contact, Lead};
use granary_db::rows::nary::WaChat;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::{apply_fn, need_actor, Action, Field, Schema, Ty};
use super::identity_match::{self, Candidate};
use super::{channels, email_intake, phones, scoring, wa_ingest};

pub const FROM_WHATSAPP: &str = "lead.from_whatsapp";
pub const HELD_DECIDE: &str = "chat.held_decide";
pub const LEAD_MERGE: &str = "lead.merge";
/// 5 Nov 2026: propose_lead_promote applies the existing lead.promote action (nary/actions.rs).
pub const PROMOTE: &str = "lead.promote";
pub const PROMOTE_PERMISSION: &str = "crm:create_deal";
/// The origin of what a person makes here (never "nary": nary.paused does not stop it).
pub const ORIGIN: &str = "chat";
/// The permission to make or merge leads.
pub const LEAD_PERMISSION: &str = "crm:edit_lead";
/// The permission to decide a held chat (POST /nary/intake/held/{chat_id}).
pub const HELD_PERMISSION: &str = "nary:manage";

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn text(args: &Value, k: &str) -> Option<String> {
    match args.get(k) {
        Some(Value::String(s)) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn api_msg(e: &ApiError) -> String {
    super::governance::http_detail(e).unwrap_or_else(|| crate::ai::tools::catalog::exc_str(e))
}

fn can(user: &AuthUser, perm: &str) -> bool {
    user.is_superadmin || user.perms.contains(perm)
}

fn full_name(c: &Contact) -> String {
    let n = [Some(c.first_name.clone()), c.last_name.clone()]
        .into_iter()
        .flatten()
        .filter(|p| !p.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if n.is_empty() {
        "the contact".into()
    } else {
        n
    }
}

fn chat_label(chat: &WaChat) -> String {
    match (
        chat.display_name
            .as_deref()
            .filter(|s| !s.trim().is_empty()),
        chat.phone.as_deref().filter(|s| !s.is_empty()),
    ) {
        (Some(n), Some(p)) => format!("{n} ({p})"),
        (Some(n), None) => n.to_string(),
        (None, Some(p)) => p.to_string(),
        (None, None) => chat.chat_id.clone(),
    }
}

/// The chat by its id, else by a phone number (the newest chat of that number).
pub async fn find_chat(db: &mut PgConnection, args: &Value) -> ApiResult<Option<WaChat>> {
    if let Some(id) = text(args, "chat_id") {
        if let Some(c) = wa_ingest::chat_by_id(db, &id).await? {
            return Ok(Some(c));
        }
    }
    let raw = text(args, "phone").or_else(|| text(args, "chat_id"));
    let Some(raw) = raw else {
        return Ok(None);
    };
    let digits = raw.split('@').next().unwrap_or("").to_string();
    let country = phones::default_country(db).await?;
    let Some(e164) = phones::to_e164(Some(&digits), &country) else {
        return Ok(None);
    };
    Ok(sqlx::query_as(
        "SELECT * FROM wa_chats WHERE phone = $1 ORDER BY last_inbound_at DESC NULLS LAST, created_at DESC LIMIT 1",
    )
    .bind(e164)
    .fetch_optional(&mut *db)
    .await?)
}

/// (id, title, status) of the lead when it is open (new, triaging, qualifying, nurture).
async fn open_lead(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<(Uuid, String, String)>> {
    let mut statuses: Vec<String> = crate::crm::stages::ACTIVE_LEAD_STATUSES
        .iter()
        .map(|s| s.to_string())
        .collect();
    statuses.push("nurture".into());
    let row: Option<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, title, status FROM leads WHERE id = $1 AND is_deleted = false AND status = ANY($2)",
    )
    .bind(id)
    .bind(&statuses)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|(i, t, s)| {
        (
            i,
            t.unwrap_or_else(|| "untitled".into()),
            s.unwrap_or_default(),
        )
    }))
}

/// (id, title, stage) of the deal when it is open (not won, handed over, lost...).
async fn open_deal(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<(Uuid, String, String)>> {
    let closed: Vec<String> = crate::crm::stages::CLOSED_STAGES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let row: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, title, stage FROM deals WHERE id = $1 AND is_deleted = false AND NOT (stage = ANY($2))",
    )
    .bind(id)
    .bind(&closed)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|(i, t, s)| (i, t, s.unwrap_or_default())))
}

async fn contact(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Contact>> {
    Ok(
        sqlx::query_as("SELECT * FROM contacts WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

// ─── propose_whatsapp_lead ────────────────────────────────────────────────────

/// Who the chat is, worked out the same way for the card and the apply.
enum Who {
    /// a number nobody knows: a contact and a lead are made from the chat
    New,
    /// a known person with no open lead or deal (a returning contact): a lead for them; `link` when the chat is
    /// not linked to them yet (the number's contact, or a certain identity match)
    Known {
        contact: Box<Contact>,
        link: Option<Candidate>,
    },
}

struct LeadPlan {
    chat: WaChat,
    who: Who,
    details: Vec<String>,
}

/// Why a lead can't be made from this chat, or how it would be.
async fn lead_plan(
    db: &mut PgConnection,
    chat: WaChat,
    args: &Value,
) -> ApiResult<Result<LeadPlan, String>> {
    let label = chat_label(&chat);
    if chat.staff_user_id.is_some() {
        return Ok(Err(format!(
            "{label} is a staff chat: it never becomes a lead."
        )));
    }
    match chat.status.as_str() {
        "excluded" => {
            return Ok(Err(format!(
                "{label} is a personal chat (excluded): it never becomes a lead."
            )))
        }
        "held" => {
            return Ok(Err(format!(
                "{label} is held (business or personal not decided yet): decide it first with \
                 propose_held_chat_decision (business), then make the lead."
            )))
        }
        _ => {}
    }
    if let Some(lid) = chat.lead_id {
        if let Some((id, title, status)) = open_lead(db, lid).await? {
            return Ok(Err(format!(
                "This chat already belongs to the open lead '{title}' ({id}, {status}): no second lead."
            )));
        }
    }
    if let Some(did) = chat.deal_id {
        if let Some((id, title, stage)) = open_deal(db, did).await? {
            return Ok(Err(format!(
                "This chat already belongs to the open deal '{title}' ({id}, {stage}): no lead is needed."
            )));
        }
    }
    let mut details = vec![format!("WhatsApp chat: {label}")];
    // identity: the chat's contact, the number's contact, a certain identity match
    let mut known: Option<(Contact, Option<Candidate>, String)> = None;
    if let Some(cid) = chat.contact_id {
        match contact(db, cid).await? {
            Some(c) => known = Some((c, None, "the chat's contact".into())),
            None => {
                return Ok(Err(
                    "The chat's contact was deleted: link the chat to a contact first (propose_chat_link)."
                        .into(),
                ))
            }
        }
    }
    if known.is_none() {
        if let Some(c) = channels::_known_contact(db, &chat).await? {
            let first = if c.first_name.is_empty() {
                "contact".to_string()
            } else {
                c.first_name.clone()
            };
            let mut cand = Candidate::new("contact", c.id, &first);
            cand.score = 100;
            cand.signals = vec![
                json!({"signal": "phone", "score": 100, "detail": "the number belongs to this contact"}),
            ];
            known = Some((c, Some(cand), "the number belongs to them".into()));
        }
    }
    if known.is_none() {
        let cfg = identity_match::settings_bundle(db).await?;
        let convo = channels::conversation_text(db, &chat, None, 12).await?;
        let cands =
            identity_match::candidates_for_chat(db, &chat, Some(&convo), &cfg, None).await?;
        if let Some(best) = cands.first() {
            match identity_match::tier_for(best.score, &cfg.thresholds) {
                Some("certain") if best.kind == "contact" => {
                    if let Some(c) = contact(db, best.target_id).await? {
                        let why = format!("identity match: certain, {}", best.label);
                        known = Some((c, Some(best.clone()), why));
                    }
                }
                Some("certain") => {
                    // a website [whatsapp] form: the chat belongs to that form's lead
                    let lead: Option<Option<Uuid>> = sqlx::query_scalar(
                        "SELECT lead_id FROM nary_expected_arrivals WHERE id = $1",
                    )
                    .bind(best.target_id)
                    .fetch_optional(&mut *db)
                    .await?;
                    if let Some(lid) = lead.flatten() {
                        if let Some((id, title, status)) = open_lead(db, lid).await? {
                            return Ok(Err(format!(
                                "This chat matches the website form of the open lead '{title}' ({id}, {status}): it \
                                 belongs to that lead, no second lead."
                            )));
                        }
                    }
                }
                Some(tier) => details.push(format!(
                    "Note: it may be {} (identity match {tier}, score {}); check before confirming.",
                    best.label, best.score
                )),
                None => {}
            }
        }
    }
    let name = text(args, "name");
    let who = match known {
        Some((c, link, why)) => {
            let (lead, deal) = identity_match::open_opportunity(db, &c).await?;
            let who = full_name(&c);
            if let Some((id, title, status)) = match lead {
                Some(l) => open_lead(db, l).await?,
                None => None,
            } {
                return Ok(Err(format!(
                    "The chat is {who} ({why}), who already has the open lead '{title}' ({id}, {status}): no second \
                     lead. Link the chat to them with propose_chat_link if it isn't."
                )));
            }
            if let Some((id, title, stage)) = match deal {
                Some(d) => open_deal(db, d).await?,
                None => None,
            } {
                return Ok(Err(format!(
                    "The chat is {who} ({why}), who already has the open deal '{title}' ({id}, {stage}): no lead is \
                     needed. Link the chat to them with propose_chat_link if it isn't."
                )));
            }
            details.push(format!(
                "Contact: {who} (known: {why}; a returning contact with no open lead or deal)"
            ));
            if name.is_some() {
                details.push("The name given is not used: the contact already has one.".into());
            }
            details.push(format!(
                "Lead: 'WhatsApp: {who}' (qualifying), the chat joins it"
            ));
            Who::Known {
                contact: Box::new(c),
                link,
            }
        }
        None => {
            let who = name
                .clone()
                .or_else(|| chat.display_name.clone().filter(|n| !n.trim().is_empty()))
                .or_else(|| chat.phone.clone())
                .unwrap_or_else(|| "WhatsApp".into());
            details.push(format!("Contact: new, '{who}', with the chat's number"));
            details.push(format!(
                "Lead: 'WhatsApp: {who}' (qualifying), the chat joins it"
            ));
            details.push("After: a card to promote it to a deal (lead.promote)".into());
            Who::New
        }
    };
    if let Some(org) = text(args, "organization") {
        details.push(format!(
            "Organization: {org} (the one with that name, else a new prospect organization)"
        ));
    }
    if let Some(notes) = text(args, "notes") {
        details.push(format!("Notes on the lead: {notes}"));
    }
    details.push("Nothing is sent to the customer.".into());
    Ok(Ok(LeadPlan { chat, who, details }))
}

fn lead_label(chat: &WaChat) -> String {
    format!("Make a lead from the WhatsApp chat {}", chat_label(chat))
}

async fn chat_of(db: &mut PgConnection, p: &Value) -> ApiResult<WaChat> {
    let id = text(p, "chat_id").unwrap_or_default();
    wa_ingest::chat_by_id(db, &id)
        .await?
        .ok_or_else(|| ApiError::http(404, "That WhatsApp chat no longer exists"))
}

/// Makes the lead (the apply of lead.from_whatsapp).
pub async fn make_lead(db: &mut PgConnection, actor: &AuthUser, p: &Value) -> ApiResult<Value> {
    let chat = chat_of(db, p).await?;
    let LeadPlan { mut chat, who, .. } = match lead_plan(db, chat, p).await? {
        Ok(plan) => plan,
        Err(why) => return Err(ApiError::http(409, why)),
    };
    let convo = channels::conversation_text(db, &chat, None, 12).await?;
    let name = text(p, "name");
    let (lead_id, contact_id, made_contact) = match who {
        Who::New => {
            let by = channels::IntakeBy {
                origin: ORIGIN,
                name: name.as_deref(),
                actor: Some(actor),
            };
            let lead = channels::create_intake_by(db, &mut chat, &convo, None, by)
                .await?
                .ok_or_else(|| ApiError::http(409, "The lead was made but is gone already"))?;
            (lead.id, lead.contact_id, true)
        }
        Who::Known { contact, link } => {
            if let Some(cand) = link {
                identity_match::link_chat(db, &mut chat, &cand, "certain", false, None, "linked")
                    .await?;
            }
            let lead = email_intake::open_lead_by(
                db,
                &contact,
                "whatsapp",
                &convo,
                chat.language.as_deref(),
                Some(&chat.chat_id),
                ORIGIN,
            )
            .await?;
            sqlx::query(
                "UPDATE wa_chats SET contact_id = $2, lead_id = $3, updated_at = now() WHERE id = $1",
            )
            .bind(chat.id)
            .bind(contact.id)
            .bind(lead.id)
            .execute(&mut *db)
            .await?;
            sqlx::query(
                "UPDATE wa_messages SET contact_id = $2, lead_id = $3, updated_at = now() WHERE chat_id = $1",
            )
            .bind(&chat.chat_id)
            .bind(contact.id)
            .bind(lead.id)
            .execute(&mut *db)
            .await?;
            sqlx::query(
                "UPDATE leads SET created_by_id = $2, updated_at = now() WHERE id = $1 AND created_by_id IS NULL",
            )
            .bind(lead.id)
            .bind(actor.id)
            .execute(&mut *db)
            .await?;
            (lead.id, Some(contact.id), false)
        }
    };
    let mut org_name: Option<String> = None;
    if let Some(org) = text(p, "organization") {
        let country = phones::derive_locale(db, chat.phone.as_deref(), None)
            .await?
            .get("country")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(o) = super::intake::organization_for(db, &org, country.as_deref()).await? {
            sqlx::query("UPDATE leads SET organization_id = $2, updated_at = now() WHERE id = $1")
                .bind(lead_id)
                .bind(o.id)
                .execute(&mut *db)
                .await?;
            if let Some(cid) = contact_id {
                sqlx::query(
                    "UPDATE contacts SET organization_id = $2, updated_at = now() \
                     WHERE id = $1 AND organization_id IS NULL",
                )
                .bind(cid)
                .bind(o.id)
                .execute(&mut *db)
                .await?;
            }
            org_name = Some(o.name);
        }
    }
    if let Some(notes) = text(p, "notes") {
        sqlx::query(
            "INSERT INTO activities (id, entity_type, entity_id, lead_id, contact_id, activity_type, subject, \
             content, completed_at, is_completed, created_by_id, assigned_to_id, channel_data) \
             VALUES ($1, 'lead', $2, $2, $3, 'note', 'Notes', $4, now(), true, $5, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(lead_id)
        .bind(contact_id)
        .bind(&notes)
        .bind(actor.id)
        .bind(json!({"source": "whatsapp_lead", "chat_id": chat.chat_id}))
        .execute(&mut *db)
        .await?;
    }
    super::commit(db).await?;
    let title: Option<String> = sqlx::query_scalar("SELECT title FROM leads WHERE id = $1")
        .bind(lead_id)
        .fetch_one(&mut *db)
        .await?;
    let title = title.unwrap_or_default();
    tracing::info!(lead_id = %lead_id, chat_id = %chat.chat_id, user_id = %actor.id, made_contact,
                   "nary_whatsapp_lead_made");
    Ok(json!({
        "id": lead_id.to_string(),
        "lead_id": lead_id.to_string(),
        "contact_id": contact_id.map(|c| c.to_string()),
        "organization": org_name,
        "new_contact": made_contact,
        "label": format!("Lead '{title}' made from the WhatsApp chat"),
    }))
}

async fn _describe_lead(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let chat = chat_of(db, p).await?;
    let label = lead_label(&chat);
    Ok(match lead_plan(db, chat, p).await? {
        Ok(plan) => json!({"label": label, "lines": plan.details}),
        Err(why) => json!({"label": label, "error": why}),
    })
}

async fn _apply_lead(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    make_lead(db, actor, p).await
}

/// Puts the details on a card the autonomy helper made.
fn dress(out: &mut Value, details: &[String]) {
    if let Some(Value::Object(card)) = out.get_mut("proposal") {
        card.insert("details".into(), json!(details));
    }
}

/// `propose_whatsapp_lead`: a card that makes a lead from a WhatsApp chat (Nary's intake, by hand).
pub async fn _h_propose_whatsapp_lead(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !can(user, LEAD_PERMISSION) {
        return err("Making a lead needs the permission to edit leads (crm:edit_lead).");
    }
    if text(&args, "chat_id").is_none() && text(&args, "phone").is_none() {
        return err("Give chat_id (list_recent_whatsapp or nary_work_queue) or the phone number.");
    }
    let Some(chat) = find_chat(db, &args).await? else {
        return err(
            "No such WhatsApp chat in Nary's list: give the chat id from list_recent_whatsapp (or the number of a \
             chat that wrote to us).",
        );
    };
    let mut payload = Map::new();
    payload.insert("chat_id".into(), json!(chat.chat_id));
    for k in ["name", "organization", "notes"] {
        if let Some(v) = text(&args, k) {
            payload.insert(k.into(), json!(v));
        }
    }
    let clean = match super::actions::get(FROM_WHATSAPP)
        .and_then(|a| a.validate(&Value::Object(payload)))
    {
        Ok(c) => c,
        Err(e) => return err(api_msg(&e)),
    };
    let summary = lead_label(&chat);
    let plan = match lead_plan(db, chat, &clean).await? {
        Ok(p) => p,
        Err(why) => return err(why),
    };
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, FROM_WHATSAPP, clean, summary, None)
            .await?;
    dress(&mut out, &plan.details);
    Ok(out)
}

// ─── propose_held_chat_decision ───────────────────────────────────────────────

fn held_details(chat: &WaChat, business: bool, reason: Option<&str>) -> Vec<String> {
    let mut d = vec![format!("Held WhatsApp chat: {}", chat_label(chat))];
    if business {
        d.push("Business: released to Nary; its waiting messages are processed (while Nary is paused they wait \
                for the resume, and you can make the lead now with propose_whatsapp_lead)."
            .into());
    } else {
        d.push(
            "Personal: excluded for good; everything stored from it (messages, media) is deleted and Nary never \
             reads it."
                .into(),
        );
    }
    if let Some(r) = reason {
        d.push(format!("Why: {r}"));
    }
    d
}

fn held_label(chat: &WaChat, business: bool) -> String {
    format!(
        "Held chat {}: {}",
        chat_label(chat),
        if business { "business" } else { "personal" }
    )
}

async fn held_chat(db: &mut PgConnection, p: &Value) -> ApiResult<Result<WaChat, String>> {
    let chat = chat_of(db, p).await?;
    if chat.status != "held" {
        return Ok(Err(format!(
            "{} is not held (it is {}): there is nothing to decide.",
            chat_label(&chat),
            chat.status
        )));
    }
    Ok(Ok(chat))
}

fn is_business(p: &Value) -> bool {
    p.get("decision").and_then(Value::as_str) == Some("business")
}

async fn _describe_held(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let business = is_business(p);
    Ok(match held_chat(db, p).await? {
        Ok(chat) => json!({"label": held_label(&chat, business),
                           "lines": held_details(&chat, business, text(p, "reason").as_deref())}),
        Err(why) => json!({"label": "Decide a held WhatsApp chat", "error": why}),
    })
}

async fn _apply_held(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    let chat = match held_chat(db, p).await? {
        Ok(c) => c,
        Err(why) => return Err(ApiError::http(409, why)),
    };
    let business = is_business(p);
    let done = channels::decide_held(db, &chat.chat_id, business, Some(actor)).await?;
    super::commit(db).await?;
    tracing::info!(chat_id = %chat.chat_id, business, user_id = %actor.id, "nary_held_decided_by_hand");
    Ok(
        json!({"id": done.id.to_string(), "chat_id": done.chat_id, "status": done.status,
              "label": format!("{} is {}", chat_label(&chat),
                               if business { "business: released" } else { "personal: excluded and deleted" })}),
    )
}

/// `propose_held_chat_decision`: the Owner's business-or-personal answer for a held chat, as a card.
pub async fn _h_propose_held_chat_decision(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !can(user, HELD_PERMISSION) {
        return err("Deciding a held chat needs the permission to manage Nary (nary:manage).");
    }
    let decision = text(&args, "decision")
        .map(|d| d.to_lowercase())
        .unwrap_or_default();
    if !matches!(decision.as_str(), "business" | "personal") {
        return err("decision must be business or personal.");
    }
    let Some(chat) = find_chat(db, &args).await? else {
        return err(
            "No such WhatsApp chat: give the chat id of a held chat (nary_work_queue lists them).",
        );
    };
    let mut payload = Map::new();
    payload.insert("chat_id".into(), json!(chat.chat_id));
    payload.insert("decision".into(), json!(decision));
    if let Some(r) = text(&args, "reason") {
        payload.insert("reason".into(), json!(r));
    }
    let clean =
        match super::actions::get(HELD_DECIDE).and_then(|a| a.validate(&Value::Object(payload))) {
            Ok(c) => c,
            Err(e) => return err(api_msg(&e)),
        };
    let chat = match held_chat(db, &clean).await? {
        Ok(c) => c,
        Err(why) => return err(why),
    };
    let business = decision == "business";
    let details = held_details(&chat, business, text(&clean, "reason").as_deref());
    let summary = held_label(&chat, business);
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, HELD_DECIDE, clean, summary, None)
            .await?;
    dress(&mut out, &details);
    Ok(out)
}

/// A held-chat card that deletes what was stored (personal): on the Owner's full-power connection it waits for
/// nary_apply like a merge or a delete (mcp_access::gated).
pub fn deletes_chat(proposal: &Value) -> bool {
    proposal.get("action").and_then(Value::as_str) == Some(HELD_DECIDE)
        && proposal
            .get("payload")
            .and_then(|p| p.get("decision"))
            .and_then(Value::as_str)
            != Some("business")
}

// ─── propose_lead_merge ───────────────────────────────────────────────────────

async fn load_lead(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Lead>> {
    Ok(
        sqlx::query_as("SELECT * FROM leads WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// `propose_lead_merge`: the existing lead.merge action (the daily merge pass's card) raised by a person.
pub async fn _h_propose_lead_merge(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !can(user, LEAD_PERMISSION) {
        return err("Merging leads needs the permission to edit leads (crm:edit_lead).");
    }
    let id = |k: &str| text(&args, k).and_then(|s| Uuid::parse_str(&s).ok());
    let (Some(keep_id), Some(merge_id)) = (id("keep_lead_id"), id("merge_lead_id")) else {
        return err(
            "keep_lead_id and merge_lead_id must be lead ids (search_records or nary_work_queue).",
        );
    };
    if keep_id == merge_id {
        return err("keep_lead_id and merge_lead_id are the same lead: nothing to merge.");
    }
    for (lid, role) in [(keep_id, "keep"), (merge_id, "merge")] {
        let Some(lead) = load_lead(db, lid).await? else {
            return err(format!("Lead {lid} not found."));
        };
        if let Err(e) = crate::crm::access::assert_can_view(user, lead.assigned_to_id) {
            return err(format!(
                "You can't see '{}': {}",
                lead.title.unwrap_or_default(),
                api_msg(&e)
            ));
        }
        if role == "merge" {
            if let Some(st) = lead
                .status
                .as_deref()
                .filter(|s| matches!(*s, "disqualified" | "promoted"))
            {
                return err(format!(
                    "'{}' is already {st}: it can't be merged away.",
                    lead.title.unwrap_or_default()
                ));
            }
        }
    }
    let payload = json!({"keep_id": keep_id.to_string(), "drop_id": merge_id.to_string()});
    let action = match super::actions::get(LEAD_MERGE) {
        Ok(a) => a,
        Err(e) => return err(api_msg(&e)),
    };
    let described = match &action.describe {
        Some(d) => d(db, Some(user), &payload).await?,
        None => json!({"label": action.label}),
    };
    let summary = described["label"]
        .as_str()
        .unwrap_or("Merge two leads")
        .to_string();
    let s = |v: &Value| v.as_str().unwrap_or("?").to_string();
    let details = vec![
        format!("Kept: {}", s(&described["before"]["keep"])),
        format!("Merged: {}", s(&described["before"]["duplicate"])),
        format!(
            "After: the merged lead is {}",
            s(&described["after"]["duplicate"])
        ),
    ];
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, LEAD_MERGE, payload, summary, None)
            .await?;
    dress(&mut out, &details);
    Ok(out)
}

// ─── propose_lead_promote (5 Nov 2026) ────────────────────────────────────────

/// `propose_lead_promote`: a card that promotes an open lead to a deal (the existing lead.promote action), so
/// The MCP client can do it while Nary is paused. The lead must be new, triaging, qualifying or nurture
/// (crm::stages::PROMOTABLE_LEAD_STATUSES); one already promoted answers with its deal.
pub async fn _h_propose_lead_promote(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    if !can(user, PROMOTE_PERMISSION) {
        return err("Promoting a lead needs the permission to create deals (crm:create_deal).");
    }
    let Some(lead_id) = text(&args, "lead_id").and_then(|s| Uuid::parse_str(&s).ok()) else {
        return err("lead_id must be a lead id (search_records, list_leads or nary_work_queue).");
    };
    let Some(lead) = load_lead(db, lead_id).await? else {
        return err(format!("Lead {lead_id} not found."));
    };
    let title = lead.title.clone().unwrap_or_default();
    if let Err(e) = crate::crm::access::assert_can_view(user, lead.assigned_to_id) {
        return err(format!("You can't see '{title}': {}", api_msg(&e)));
    }
    if let Some(deal_id) = lead.converted_to_deal_id {
        return Ok(
            json!({"ok": true, "unchanged": true, "deal_id": deal_id.to_string(), "message": format!(
                "'{title}' is a deal already (deal {deal_id}): nothing to promote."
            )}),
        );
    }
    let status = lead.status.as_deref().unwrap_or("None");
    if !crate::crm::stages::PROMOTABLE_LEAD_STATUSES.contains(&status) {
        return err(format!(
            "'{title}' is {status}: only a new, triaging, qualifying or nurture lead can be promoted."
        ));
    }
    let payload = json!({"lead_id": lead_id.to_string()});
    let action = match super::actions::get(PROMOTE) {
        Ok(a) => a,
        Err(e) => return err(api_msg(&e)),
    };
    let described = match &action.describe {
        Some(d) => d(db, Some(user), &payload).await?,
        None => json!({"label": action.label}),
    };
    let org: Option<String> = match lead.organization_id {
        Some(o) => {
            sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
                .bind(o)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let mut details = vec![format!("Lead: {title} ({status})")];
    if let Some(o) = &org {
        details.push(format!("Organization: {o}"));
    }
    if let Some(l) = lead.segment.as_deref() {
        details.push(format!("Segment: {l}"));
    }
    details.push(format!(
        "After: {}",
        described["after"]["deal"].as_str().unwrap_or("a new deal")
    ));
    let summary = format!("Promote '{title}' to a deal");
    let mut out =
        crate::ai::tools::catalog::_autonomy_card(db, user, PROMOTE, payload, summary, None)
            .await?;
    dress(&mut out, &details);
    Ok(out)
}

// ─── score_lead ───────────────────────────────────────────────────────────────

/// What the lead's conversations say, for the BANT scoring: its WhatsApp chats, its email threads, else the
/// message it came with. Returns (transcript, the chat it is scored on).
pub async fn lead_transcript(
    db: &mut PgConnection,
    lead: &Lead,
) -> ApiResult<(String, Option<String>)> {
    let mut parts: Vec<String> = Vec::new();
    let chats: Vec<String> = sqlx::query_scalar(
        "SELECT chat_id FROM wa_chats WHERE (lead_id = $1 OR (contact_id = $2 AND $2 IS NOT NULL)) \
         AND status = 'active' AND staff_user_id IS NULL \
         ORDER BY last_inbound_at DESC NULLS LAST LIMIT 2",
    )
    .bind(lead.id)
    .bind(lead.contact_id)
    .fetch_all(&mut *db)
    .await?;
    for c in &chats {
        let t = super::history::transcript(db, c, "summary").await?;
        if !t.trim().is_empty() {
            parts.push(format!("WhatsApp:\n{t}"));
        }
    }
    let threads: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM crm_email_threads WHERE linked_lead_id = $1 ORDER BY updated_at DESC LIMIT 2",
    )
    .bind(lead.id)
    .fetch_all(&mut *db)
    .await?;
    for t in threads {
        let text = super::playbooks::email_transcript(db, t, 20).await?;
        if !text.trim().is_empty() {
            parts.push(format!("Email:\n{text}"));
        }
    }
    if parts.is_empty() {
        if let Some(m) = lead
            .raw_message
            .clone()
            .or_else(|| lead.description.clone())
            .filter(|m| !m.trim().is_empty())
        {
            parts.push(format!("[customer] {m}"));
        }
    }
    Ok((parts.join("\n\n"), chats.into_iter().next()))
}

fn truthy_arg(args: &Value, k: &str) -> bool {
    match args.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1"),
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    }
}

/// `score_lead`: Nary's BANT score of a lead from its conversations; with save=true it is stored as Nary stores it
/// (a nary_scores row, the lead's ai_score, the Hot interrupt). Saving is a plain write (no money, nothing to a
/// customer): over MCP only at act and full power (mcp_access::ACT_ARGS).
pub async fn _h_score_lead(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let Some(lead_id) = text(&args, "lead_id").and_then(|s| Uuid::parse_str(&s).ok()) else {
        return err("lead_id must be a lead id (search_records or nary_work_queue).");
    };
    let Some(mut lead) = load_lead(db, lead_id).await? else {
        return err(format!("Lead {lead_id} not found."));
    };
    if let Err(e) = crate::crm::access::assert_can_view(user, lead.assigned_to_id) {
        return err(format!("You can't see this lead: {}", api_msg(&e)));
    }
    let save = truthy_arg(&args, "save");
    if save && !can(user, LEAD_PERMISSION) {
        return err("Saving a score needs the permission to edit leads (crm:edit_lead); score without save.");
    }
    let (transcript, chat_id) = lead_transcript(db, &lead).await?;
    if transcript.trim().is_empty() {
        return err(
            "Nothing to score: the lead has no WhatsApp chat, email thread or message yet.",
        );
    }
    let previous = scoring::latest(db, Some(lead.id), None).await?;
    let Some(a) = scoring::assess(db, &transcript, None, "").await? else {
        return err("The model gave no usable score; try again.");
    };
    let mut saved = false;
    if save {
        // its own transaction: the score is kept whatever happens to the rest of the turn
        match super::session_pool() {
            Ok(pool) => {
                let mut tx = pool.begin().await?;
                scoring::store(&mut tx, &a, Some(&mut lead), None, chat_id.as_deref()).await?;
                tx.commit().await?;
            }
            Err(_) => {
                scoring::store(db, &a, Some(&mut lead), None, chat_id.as_deref()).await?;
            }
        }
        saved = true;
        tracing::info!(lead_id = %lead.id, user_id = %user.id, total = a.total, class = %a.class,
                       "nary_lead_scored_by_hand");
    }
    let table = scoring::classes(db).await?;
    let classes: Map<String, Value> = table
        .iter()
        .map(|(k, (lo, hi))| (k.clone(), json!([lo, hi])))
        .collect();
    let bant: Map<String, Value> = a
        .parts
        .iter()
        .map(|(p, n)| (p.to_string(), json!(n)))
        .collect();
    Ok(json!({
        "lead_id": lead.id.to_string(),
        "title": lead.title,
        "bant": bant,
        "total": a.total,
        "max": 12,
        "class": a.class,
        "classes": classes,
        "red_flags": a.red_flags,
        "green_flags": a.green_flags,
        "rationale": a.rationale,
        "intent": a.intent,
        "product": a.product,
        "cascaded": a.cascaded,
        "disqualify_suggested": a.disqualify,
        "previous": previous.map(|p| json!({"total": p.total, "class": p.class,
                                            "at": p.created_at.to_rfc3339()})),
        "saved": saved,
        "note": if saved {
            "Saved on the lead as Nary saves its scores (CRM shows it; a new Hot lead alerts the Owner)."
        } else {
            "Not saved: call again with save=true to store it on the lead."
        },
    }))
}

// ─── The actions ──────────────────────────────────────────────────────────────

/// The actions of this module (registered by actions::module_actions). lead.merge is the existing action.
pub fn actions() -> Vec<Action> {
    vec![
        Action::new(
            FROM_WHATSAPP,
            "Make a lead from a WhatsApp chat",
            Schema::strict(
                "LeadFromWhatsapp",
                vec![
                    Field::req("chat_id", Ty::Str),
                    Field::opt("name", Ty::Str),
                    Field::opt("organization", Ty::Str),
                    Field::opt("notes", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_lead(db, a, p))),
        )
        .permission(LEAD_PERMISSION)
        .describe(apply_fn(|db, a, p| Box::pin(_describe_lead(db, a, p)))),
        Action::new(
            HELD_DECIDE,
            "Decide a held WhatsApp chat (business or personal)",
            Schema::strict(
                "HeldChatDecision",
                vec![
                    Field::req("chat_id", Ty::Str),
                    Field::req("decision", Ty::Literal(&["business", "personal"])),
                    Field::opt("reason", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_held(db, a, p))),
        )
        .permission(HELD_PERMISSION)
        .describe(apply_fn(|db, a, p| Box::pin(_describe_held(db, a, p)))),
    ]
}
