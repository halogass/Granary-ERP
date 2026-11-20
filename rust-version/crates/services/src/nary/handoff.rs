//! Port of backend/app/services/nary/handoff.py: won, handoff, lost and partners (Nary Phase 3
//! item D).
//!
//! ```text
//!     on Won      Opus (nary.handoff) compiles the handoff package from the whole history: scope sold
//!                 and NOT sold, success criteria in the customer's own words, commitments, sensitivities,
//!                 red flags. Stored as an internal note on the deal; the Owner is asked to approve the
//!                 kickoff (handoff.kickoff): a project for the deal, its PM from the role table, and the
//!                 kickoff tasks (nary.handoff.kickoff_tasks).
//!     on Lost     a gracious reply to the customer (lost.reply.send, a card), and an analysis note. The
//!                 re-contact reminder comes from the loss reason (crm.funnel.loss_reasons), as before.
//!     partners    a new lead in a partner's territory (country, vertical) or a nary.partners.routing rule
//!                 gives partner.route (a card); deal registrations near the end of their protection window
//!                 give a warning; a conflicting registration gives the evidence, ERP timestamps first.
//! ```
//!
//! Port note: Python registers on_deal_staged ("deal.staged") and on_lead_created ("lead.created") with
//! `@events.subscribe`; the events port wires these functions in.

use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use granary_compat::pyjson::{self, PyDateTime, PyValue};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json};
use granary_db::rows::crm::{Activity, Contact, Deal, Lead, Organization};
use granary_db::rows::nary::{AgentRun, DomainEvent, NaryInsight, WaChat, WaMessage};
use granary_db::rows::partner::{PartnerAccount, PartnerDealRegistration};
use granary_db::rows::project::Project;
use granary_db::rows::sales::{Quotation, QuotationItem};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::notify::{notify, Notice};
use crate::nary::commit;
use crate::nary::replies::{
    self, cut, dumps_unicode, get, get_row, py_str, py_type, truthy, Conversation,
};

pub const WHERE: &str = "Nary & AI";
pub const SECTIONS: [&str; 8] = [
    "scope_sold",
    "scope_not_sold",
    "success_criteria",
    "commitments",
    "sensitivities",
    "red_flags",
    "contacts",
    "open_items",
];

pub static HANDOFF_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    let strings = json!({"type": "array", "items": {"type": "string"}});
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string"},
            "scope_sold": strings,
            "scope_not_sold": strings,
            "success_criteria": {"type": "array", "items": {"type": "object", "properties": {
                "quote": {"type": "string"}, "source": {"type": "string"}}, "required": ["quote", "source"], "additionalProperties": false}},
            "commitments": strings,
            "sensitivities": strings,
            "red_flags": strings,
            "contacts": strings,
            "open_items": strings,
        },
        "required": ["summary", "scope_sold", "scope_not_sold", "success_criteria", "commitments", "sensitivities",
                     "red_flags", "contacts", "open_items"],
        "additionalProperties": false,
    })
});

pub static LOST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({"type": "object", "properties": {"reply": {"type": ["string", "null"]}, "analysis": {"type": "string"},
                                            "lesson": {"type": "string"}},
           "required": ["reply", "analysis", "lesson"], "additionalProperties": false})
});

const HANDOFF_PROMPT: &str = "Compile the handoff package for the delivery team from the \
deal's history. Scope sold, and scope NOT sold (what was asked or discussed \
but not bought). Success criteria quoted in the customer's own words, with \
where they said it. Commitments we made. Sensitivities (people, politics, \
timing). Red flags. Key contacts. Open items. Only what the history shows.";

fn ymd(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d").to_string()
}

fn internal(m: String) -> ApiError {
    ApiError::internal(anyhow::anyhow!(m))
}

/// The deal's story as text: activities (emails, calls, notes), WhatsApp messages of its contact,
/// quotation lines, and Nary's earlier notes. Oldest first. (Python's default limit is 120.)
pub async fn deal_history(db: &mut PgConnection, deal: &Deal, limit: i64) -> ApiResult<String> {
    let mut parts: Vec<String> = vec![];
    // Only what is linked to this deal, its lead or its contact: a deal without a contact matched every
    // contact-less activity (contact_id IS NULL), other customers' notes among them. Python leaves a missing
    // lead/contact out of the OR; `= NULL` is never true, which is the same filter.
    let acts: Vec<Activity> = sqlx::query_as(
        "SELECT * FROM activities WHERE deal_id = $1 OR lead_id = $2 OR contact_id = $3 \
         ORDER BY created_at DESC LIMIT $4",
    )
    .bind(deal.id)
    .bind(deal.lead_id)
    .bind(deal.contact_id)
    .bind(limit)
    .fetch_all(&mut *db)
    .await?;
    for a in acts.iter().rev() {
        parts.push(format!(
            "[{}] {} {}: {} {}",
            ymd(&a.created_at),
            a.activity_type,
            a.direction.as_deref().unwrap_or(""),
            a.subject.as_deref().unwrap_or(""),
            cut(a.content.as_deref().unwrap_or(""), 600)
        ));
    }
    if let Some(contact_id) = deal.contact_id {
        let msgs: Vec<WaMessage> = sqlx::query_as(
            "SELECT * FROM wa_messages WHERE contact_id = $1 ORDER BY wa_ts DESC LIMIT $2",
        )
        .bind(contact_id)
        .bind(limit)
        .fetch_all(&mut *db)
        .await?;
        for m in msgs.iter().rev() {
            let ts = m.wa_ts.as_ref().ok_or_else(|| {
                internal(
                    "TypeError: unsupported format string passed to NoneType.__format__".into(),
                )
            })?;
            parts.push(format!(
                "[{}] WhatsApp {}: {}",
                ymd(ts),
                if m.from_me { "us" } else { "customer" },
                cut(&crate::nary::media::text_of(m), 600)
            ));
        }
    }
    let quotations: Vec<Quotation> =
        sqlx::query_as("SELECT * FROM quotations WHERE deal_id = $1 AND is_deleted = false")
            .bind(deal.id)
            .fetch_all(&mut *db)
            .await?;
    for q in quotations {
        let items: Vec<QuotationItem> =
            sqlx::query_as("SELECT * FROM quotation_items WHERE quotation_id = $1")
                .bind(q.id)
                .fetch_all(&mut *db)
                .await?;
        let lines: Vec<String> = items
            .iter()
            .map(|i| {
                format!(
                    "{} x {}{}",
                    i.quantity, // str(Decimal) of a NUMERIC: the column's scale, as rust_decimal prints it
                    i.name,
                    if i.is_optional == Some(true) {
                        " (optional)"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        let number = q
            .full_number
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| q.number.clone());
        parts.push(format!(
            "Quotation {number} ({}): {}",
            q.status.as_deref().unwrap_or("None"),
            lines.join("; ")
        ));
    }
    let notes: Vec<NaryInsight> = sqlx::query_as(
        "SELECT * FROM nary_insights WHERE deal_id = $1 AND kind != 'handoff' ORDER BY created_at",
    )
    .bind(deal.id)
    .fetch_all(&mut *db)
    .await?;
    for n in notes {
        parts.push(format!(
            "Nary note ({}): {}",
            n.kind,
            cut(&dumps_unicode(&n.content)?, 1500)
        ));
    }
    Ok(parts.join("\n"))
}

async fn insight(
    db: &mut PgConnection,
    deal_id: Uuid,
    kind: &str,
) -> ApiResult<Option<NaryInsight>> {
    let rows: Vec<NaryInsight> =
        sqlx::query_as("SELECT * FROM nary_insights WHERE deal_id = $1 AND kind = $2")
            .bind(deal_id)
            .bind(kind)
            .fetch_all(&mut *db)
            .await?;
    if rows.len() > 1 {
        return Err(internal(
            "Multiple rows were found when one or none was required".into(),
        ));
    }
    Ok(rows.into_iter().next())
}

/// The handoff package (Opus), stored as an internal note; the kickoff card follows.
pub async fn compile_package(
    db: &mut PgConnection,
    deal: &Deal,
    run: Option<&mut AgentRun>,
) -> ApiResult<Option<NaryInsight>> {
    if let Some(existing) = insight(db, deal.id, "handoff").await? {
        return Ok(Some(existing));
    }
    let mut started;
    let run: &mut AgentRun = match run {
        Some(r) => r,
        None => {
            started = deps::start_run(
                db,
                "event",
                "nary.handoff",
                Some(&deal.id.to_string()),
                None,
            )
            .await?;
            &mut started
        }
    };
    let Some(row) = build_package(db, deal, run).await? else {
        return Ok(None);
    };
    let rationale = format!(
        "{} is won. Start the project with the handoff package and the kickoff tasks.",
        deal.title
    );
    replies::_propose(
        db,
        "handoff.kickoff",
        json!({"deal_id": deal.id.to_string()}),
        Some(&*run),
        None,
        Some(&rationale),
        None,
    )
    .await?;
    Ok(Some(row))
}

/// 3 Nov 2026 (propose_kickoff, on request): the deal's handoff package, compiled now when it has none (the same
/// model call and note as on Won), without raising the kickoff card (the tool makes that card itself).
pub async fn package_on_request(
    db: &mut PgConnection,
    deal: &Deal,
) -> ApiResult<Option<NaryInsight>> {
    if let Some(existing) = insight(db, deal.id, "handoff").await? {
        return Ok(Some(existing));
    }
    let mut run = deps::start_run(
        db,
        "manual",
        "nary.handoff",
        Some(&deal.id.to_string()),
        None,
    )
    .await?;
    build_package(db, deal, &mut run).await
}

/// The package itself (Opus), stored as the deal's internal handoff note; the run is finished. Commits.
async fn build_package(
    db: &mut PgConnection,
    deal: &Deal,
    run: &mut AgentRun,
) -> ApiResult<Option<NaryInsight>> {
    let system = crate::nary::persona::system_prompt("staff", HANDOFF_PROMPT, "", "");
    let content = deps::information("crm_history", &deal_history(db, deal, 120).await?, 120000);
    let data = deps::ask_json(
        db,
        "nary.handoff",
        &system,
        &content,
        &HANDOFF_SCHEMA,
        Some(&mut *run),
    )
    .await?;
    let Some(data) = data.filter(truthy) else {
        deps::finish_run(db, run, "failed", None, Some("no usable package")).await?;
        commit(db).await?;
        return Ok(None);
    };
    let mut pkg = Map::new();
    for k in ["summary"].iter().chain(SECTIONS.iter()) {
        pkg.insert((*k).into(), data.get(*k).cloned().unwrap_or(Value::Null));
    }
    let row: NaryInsight = sqlx::query_as(
        "INSERT INTO nary_insights (id, kind, deal_id, content, status, model, run_id) \
         VALUES ($1, 'handoff', $2, $3, 'draft', $4, $5) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(deal.id)
    .bind(Value::Object(pkg))
    .bind(get(&data, "_model").map(py_str))
    .bind(run.id)
    .fetch_one(&mut *db)
    .await?;
    deps::finish_run(db, run, "done", Some("handoff package compiled"), None).await?;
    commit(db).await?;
    Ok(Some(row))
}

/// `uuid.UUID(str(x))`: a bad id is Python's ValueError (a 500).
fn parse_uuid(s: &str) -> ApiResult<Uuid> {
    granary_compat::text::utils::py_uuid(s)
        .ok_or_else(|| internal("ValueError: badly formed hexadecimal UUID string".into()))
}

/// `"; ".join(values)`: every item must be a str (TypeError otherwise).
fn join_strs(v: Option<&Value>, sep: &str) -> ApiResult<String> {
    let items = match v.filter(|x| truthy(x)) {
        None => return Ok(String::new()),
        Some(Value::Array(a)) => a.clone(),
        Some(other) => {
            return Err(internal(format!(
                "TypeError: can only join an iterable ({})",
                py_type(other)
            )))
        }
    };
    let mut out = vec![];
    for (i, it) in items.iter().enumerate() {
        match it {
            Value::String(s) => out.push(s.clone()),
            other => {
                return Err(internal(format!(
                    "TypeError: sequence item {i}: expected str instance, {} found",
                    py_type(other)
                )))
            }
        }
    }
    Ok(out.join(sep))
}

/// Apply handoff.kickoff: a project for the deal, PM from the role table, kickoff tasks.
/// `kickoff(db, actor, deal_id)`: only `actor.id` is used, so the port takes the id.
pub async fn kickoff(db: &mut PgConnection, actor_id: Uuid, deal_id: &str) -> ApiResult<Value> {
    let Some(mut deal) = get_row::<Deal>(db, "deals", parse_uuid(deal_id)?).await? else {
        return Err(ApiError::http(404, "Deal not found"));
    };
    let existing: Vec<Project> =
        sqlx::query_as("SELECT * FROM projects WHERE deal_id = $1 AND is_deleted = false")
            .bind(deal.id)
            .fetch_all(&mut *db)
            .await?;
    if existing.len() > 1 {
        return Err(internal(
            "Multiple rows were found when one or none was required".into(),
        ));
    }
    if let Some(p) = existing.first() {
        return Ok(json!({"project_id": p.id.to_string(),
                         "label": format!("Project {} already exists", p.project_number.as_deref().unwrap_or("None"))}));
    }
    let package = insight(db, deal.id, "handoff").await?;
    let content = package
        .as_ref()
        .map(|p| p.content.clone())
        .unwrap_or(json!({}));
    let pm = deps::holder(db, "PM").await?.unwrap_or(actor_id);
    let tech = deps::holder(db, "Tech Lead").await?;
    let mut objectives = vec![];
    if let Some(Value::Array(criteria)) = get(&content, "success_criteria").filter(|v| truthy(v)) {
        for c in criteria {
            let quote = match c {
                Value::Object(m) => m.get("quote").cloned().unwrap_or(Value::Null),
                other => {
                    return Err(internal(format!(
                        "AttributeError: '{}' object has no attribute 'get'",
                        py_type(other)
                    )))
                }
            };
            objectives.push(format!("- {}", py_str(&quote)));
        }
    }
    let or_list = |k: &str| {
        get(&content, k)
            .filter(|v| truthy(v))
            .cloned()
            .unwrap_or(json!([]))
    };
    let notes = format!(
        "Not sold: {}",
        join_strs(get(&content, "scope_not_sold"), "; ")?
    );
    let number = crate::numbering::next_document_number(db, "project").await?;
    let project_id = Uuid::new_v4();
    // Python passes None for unset values; SQLAlchemy then leaves the column to its default (currency USD).
    sqlx::query(
        "INSERT INTO projects (id, name, project_number, project_type, deal_id, customer_org_id, customer_contact_id, \
         status, pm_id, tech_lead_id, created_by_id, currency, budget, cost_to_date, description, objectives, \
         deliverables, risks, notes, is_deleted) \
         VALUES ($1, $2, $3, 'installation', $4, $5, $6, 'planning', $7, $8, $9, COALESCE($10, 'USD'), $11, 0, $12, $13, \
         $14, $15, $16, false)",
    )
    .bind(project_id)
    .bind(cut(&deal.title, 255))
    .bind(&number)
    .bind(deal.id)
    .bind(deal.organization_id)
    .bind(deal.contact_id)
    .bind(pm)
    .bind(tech)
    .bind(actor_id)
    .bind(&deal.currency)
    .bind(deal.value)
    .bind(get(&content, "summary").map(py_str))
    .bind(objectives.join("\n"))
    .bind(or_list("scope_sold"))
    .bind(or_list("red_flags"))
    .bind(&notes)
    .execute(&mut *db)
    .await?;
    let tasks = require_json(db, "nary.handoff.kickoff_tasks", WHERE).await?;
    let tasks = match tasks {
        Value::Array(a) => a,
        v if !truthy(&v) => vec![],
        other => {
            return Err(internal(format!(
                "TypeError: '{}' object is not iterable",
                py_type(&other)
            )))
        }
    };
    for (i, title) in tasks.iter().enumerate() {
        let Value::String(title) = title else {
            return Err(internal(format!(
                "TypeError: '{}' object is not subscriptable",
                py_type(title)
            )));
        };
        sqlx::query(
            "INSERT INTO project_tasks (id, project_id, title, status, priority, assigned_to_id, created_by_id, sort_order, \
             tags, actual_hours, is_deleted) VALUES ($1, $2, $3, 'todo', 'high', $4, $5, $6, $7, 0, false)",
        )
        .bind(Uuid::new_v4())
        .bind(project_id)
        .bind(cut(title, 500))
        .bind(pm)
        .bind(actor_id)
        .bind(i as i32)
        .bind(vec!["kickoff".to_string()])
        .execute(&mut *db)
        .await?;
    }
    if let Some(p) = &package {
        if p.status != "final" {
            sqlx::query(
                "UPDATE nary_insights SET status = 'final', updated_at = now() WHERE id = $1",
            )
            .bind(p.id)
            .execute(&mut *db)
            .await?;
        }
    }
    if deal.stage == "won" {
        match crate::crm::stages::change_deal_stage(
            db,
            &mut deal,
            "handover",
            Some(actor_id),
            "nary",
            None,
            None,
            None,
        )
        .await
        {
            Ok(_) => {}
            // A StageError is a plain ValueError here (nothing catches it): a 500.
            Err(crate::crm::stages::StageFail::Stage(e)) => {
                return Err(internal(format!("ValueError: {}", e.0)))
            }
            Err(crate::crm::stages::StageFail::Api(e)) => return Err(e),
        }
    }
    commit(db).await?;
    Ok(json!({"project_id": project_id.to_string(), "label": format!("Project {number} started")}))
}

/// The `@events.subscribe(...)` registrations of this module (on_deal_staged, then on_lead_created).
/// Both are Nary's own work and wait while it is paused (29 Oct 2026): a won or lost deal means Opus and customer
/// cards, and the partner routing of a new lead is a partner.route card the pause refuses (run while paused it
/// would be lost, run on resume it is raised).
pub fn subscriptions() -> Vec<crate::nary::events::Subscription> {
    use crate::nary::events::{subscriber, Subscription};
    vec![
        Subscription::new(
            &["deal.staged"],
            subscriber(|db, ev| Box::pin(on_deal_staged(db, ev))),
        ),
        Subscription::new(
            &["lead.created"],
            subscriber(|db, ev| Box::pin(on_lead_created(db, ev))),
        ),
    ]
}

/// Subscriber of "deal.staged".
pub async fn on_deal_staged(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    let payload = &event.payload;
    let to = get(payload, "to")
        .filter(|v| truthy(v))
        .or_else(|| get(payload, "stage"))
        .cloned()
        .unwrap_or(Value::Null);
    let deal = match event.entity_id {
        Some(id) => get_row::<Deal>(db, "deals", id).await?,
        None => None,
    };
    let Some(deal) = deal else { return Ok(()) };
    if to.as_str() == Some("won") {
        compile_package(db, &deal, None).await?;
    } else if to.as_str() == Some("lost") {
        on_lost(db, &deal).await?;
    }
    Ok(())
}

/// A gracious reply (a card) and an analysis note for the monthly loss review.
pub async fn on_lost(db: &mut PgConnection, deal: &Deal) -> ApiResult<Option<Value>> {
    let seen: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM nary_insights WHERE deal_id = $1 AND kind = 'loss' LIMIT 1",
    )
    .bind(deal.id)
    .fetch_optional(&mut *db)
    .await?;
    if seen.is_some() {
        return Ok(None);
    }
    let mut run = deps::start_run(
        db,
        "event",
        "nary.lost_reply",
        Some(&deal.id.to_string()),
        None,
    )
    .await?;
    let reasons: Value = match &deal.loss_reasons {
        Some(r) if truthy(r) => r.clone(),
        _ => match deal.loss_reason.as_deref().filter(|r| !r.is_empty()) {
            Some(r) => json!([r]),
            None => json!([]),
        },
    };
    let contact: Option<Contact> = match deal.contact_id {
        Some(id) => get_row(db, "contacts", id).await?,
        None => None,
    };
    let chat: Option<WaChat> = match deal.contact_id {
        Some(id) => {
            sqlx::query_as(
                "SELECT * FROM wa_chats WHERE contact_id = $1 AND status = 'active' LIMIT 1",
            )
            .bind(id)
            .fetch_optional(&mut *db)
            .await?
        }
        None => None,
    };
    let language = chat
        .as_ref()
        .and_then(|c| c.language.clone())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "en".into());
    let extra = format!(
        "The deal was lost. Write a short, gracious closing message to the customer (in {}): thank them, leave the \
         door open, no pressure, no discount. reply null if the reasons say we should not contact them (for example \
         spam or they asked not to be contacted). Then, for the Owner only: 'analysis' (why it was lost, from the \
         history) and 'lesson' (one line for next time).",
        if language == "en" { "English" } else { "the customer's language" }
    );
    let channel = if chat.is_some() { "whatsapp" } else { "email" };
    let system = crate::nary::writing::customer_prompt(db, Some(channel), &extra, "").await?;
    let content = format!(
        "LOSS REASONS: {}\nNOTES: {}\n\nHISTORY:\n{}",
        pyjson::repr(&PyValue::from(&reasons)).map_err(ApiError::internal)?,
        deal.loss_notes.as_deref().unwrap_or(""),
        deps::information("crm_history", &deal_history(db, deal, 40).await?, 30000)
    );
    let data = deps::ask_json(
        db,
        "nary.lost_reply",
        &system,
        &content,
        &LOST_SCHEMA,
        Some(&mut run),
    )
    .await?;
    let Some(data) = data.filter(truthy) else {
        deps::finish_run(db, &mut run, "failed", None, Some("nothing usable")).await?;
        commit(db).await?;
        return Ok(None);
    };
    let field = |k: &str| {
        data.get(k)
            .cloned()
            .ok_or_else(|| internal(format!("KeyError: '{k}'")))
    };
    let note =
        json!({"reasons": reasons, "analysis": field("analysis")?, "lesson": field("lesson")?});
    sqlx::query(
        "INSERT INTO nary_insights (id, kind, deal_id, content, status, model, run_id) VALUES ($1, 'loss', $2, $3, 'draft', $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(deal.id)
    .bind(note)
    .bind(get(&data, "_model").map(py_str))
    .bind(run.id)
    .execute(&mut *db)
    .await?;
    deps::finish_run(db, &mut run, "done", Some("loss analysed"), None).await?;
    commit(db).await?;
    let mut card = None;
    let has_email = contact
        .as_ref()
        .is_some_and(|c| c.email.as_deref().is_some_and(|e| !e.is_empty()));
    if get(&data, "reply").is_some_and(truthy) && (chat.is_some() || has_email) {
        let mut conv = Conversation::new(if chat.is_some() { "whatsapp" } else { "email" }, "");
        conv.contact = contact.clone();
        conv.chat = chat.clone();
        let rationale = format!("A gracious close for {}", deal.title);
        card = replies::_propose(
            db,
            "lost.reply.send",
            conv.reply_payload(&py_str(&data["reply"])),
            Some(&run),
            None,
            Some(&rationale),
            None,
        )
        .await?;
    }
    Ok(Some(json!({"card_id": card.map(|c| c.id.to_string())})))
}

// ─── Partners ─────────────────────────────────────────────────────────────────

/// An active partner whose territory (or a routing rule) covers this country and vertical.
pub async fn partner_for(
    db: &mut PgConnection,
    country: Option<&str>,
    industry: Option<&str>,
) -> ApiResult<Option<PartnerAccount>> {
    let rules = require_json(db, "nary.partners.routing", WHERE).await?;
    let rules = match rules {
        Value::Array(a) => a,
        v if !truthy(&v) => vec![],
        other => {
            return Err(internal(format!(
                "TypeError: '{}' object is not iterable",
                py_type(&other)
            )))
        }
    };
    let country_up = country.unwrap_or("").to_uppercase();
    let industry_low = granary_compat::text::pystr::lower(industry.unwrap_or(""));
    for r in &rules {
        let region_ok = match get(r, "region").filter(|v| truthy(v)) {
            None => true,
            Some(Value::String(s)) => country_up == s.to_uppercase(),
            Some(Value::Array(list)) => {
                let mut ups = vec![];
                for x in list {
                    match x {
                        Value::String(s) => ups.push(s.to_uppercase()),
                        other => {
                            return Err(internal(format!(
                                "AttributeError: '{}' object has no attribute 'upper'",
                                py_type(other)
                            )))
                        }
                    }
                }
                ups.contains(&country_up)
            }
            Some(other) => {
                return Err(internal(format!(
                    "TypeError: '{}' object is not iterable",
                    py_type(other)
                )))
            }
        };
        if !region_ok {
            continue;
        }
        let industry_ok = match get(r, "industry").filter(|v| truthy(v)) {
            None => true,
            Some(v) => industry_low == granary_compat::text::pystr::lower(&py_str(v)),
        };
        if !industry_ok {
            continue;
        }
        let Some(pid) = get(r, "partner_account_id").filter(|v| truthy(v)) else {
            continue;
        };
        let p: Option<PartnerAccount> =
            get_row(db, "partner_accounts", parse_uuid(&py_str(pid))?).await?;
        if let Some(p) = p {
            if p.status.as_deref() == Some("active") && !p.is_deleted {
                return Ok(Some(p));
            }
        }
    }
    let Some(country) = country.filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let partners: Vec<PartnerAccount> = sqlx::query_as(
        "SELECT * FROM partner_accounts WHERE status = 'active' AND is_deleted = false",
    )
    .fetch_all(&mut *db)
    .await?;
    let country_up = country.to_uppercase();
    for p in partners {
        let countries: Vec<String> = p
            .territory_countries
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|c| c.to_uppercase())
            .collect();
        let verticals: Vec<String> = p
            .territory_verticals
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|v| granary_compat::text::pystr::lower(v))
            .collect();
        if countries.contains(&country_up)
            && (verticals.is_empty() || verticals.contains(&industry_low))
        {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// Subscriber of "lead.created".
pub async fn on_lead_created(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    let lead: Option<Lead> = match event.entity_id {
        Some(id) => get_row(db, "leads", id).await?,
        None => None,
    };
    let Some(lead) = lead.filter(|l| l.partner_id.is_none()) else {
        return Ok(());
    };
    let org: Option<Organization> = match lead.organization_id {
        Some(id) => get_row(db, "organizations", id).await?,
        None => None,
    };
    let contact: Option<Contact> = match lead.contact_id {
        Some(id) => get_row(db, "contacts", id).await?,
        None => None,
    };
    let from_attr = lead
        .attribution
        .as_ref()
        .and_then(|a| get(a, "country"))
        .filter(|v| truthy(v))
        .cloned();
    let country: Option<String> = match from_attr {
        Some(Value::String(s)) => Some(s),
        Some(other) => {
            return Err(internal(format!(
                "AttributeError: '{}' object has no attribute 'upper'",
                py_type(&other)
            )))
        }
        None => org
            .as_ref()
            .and_then(|o| o.country.clone())
            .filter(|c| !c.is_empty())
            .or_else(|| {
                contact
                    .as_ref()
                    .and_then(|c| c.country.clone())
                    .filter(|c| !c.is_empty())
            }),
    };
    let Some(country) = country else {
        return Ok(());
    };
    if country.to_uppercase() == "ID" {
        let rules = require_json(db, "nary.partners.routing", WHERE).await?;
        if !truthy(&rules) {
            return Ok(());
        }
    }
    let partner = partner_for(
        db,
        Some(&country),
        org.as_ref().and_then(|o| o.industry.as_deref()),
    )
    .await?;
    let Some(partner) = partner else {
        return Ok(());
    };
    let rationale = format!(
        "{} is in {}'s territory ({country}).",
        lead.title.as_deref().unwrap_or("None"),
        partner.company_name.as_deref().unwrap_or("None")
    );
    replies::_propose(
        db,
        "partner.route",
        json!({"lead_id": lead.id.to_string(), "partner_account_id": partner.id.to_string()}),
        None,
        None,
        Some(&rationale),
        None,
    )
    .await?;
    Ok(())
}

/// `route_to_partner(db, actor, lead_id, partner_account_id)`: only `actor.id` is used.
pub async fn route_to_partner(
    db: &mut PgConnection,
    actor_id: Uuid,
    lead_id: &str,
    partner_account_id: &str,
) -> ApiResult<Value> {
    let lead: Option<Lead> = get_row(db, "leads", parse_uuid(lead_id)?).await?;
    let partner: Option<PartnerAccount> =
        get_row(db, "partner_accounts", parse_uuid(partner_account_id)?).await?;
    let (Some(lead), Some(partner)) = (lead, partner) else {
        return Err(ApiError::http(404, "Lead or partner not found"));
    };
    let contact: Option<Contact> = match lead.contact_id {
        Some(id) => get_row(db, "contacts", id).await?,
        None => None,
    };
    let org: Option<Organization> = match lead.organization_id {
        Some(id) => get_row(db, "organizations", id).await?,
        None => None,
    };
    let customer_name = match &contact {
        Some(c) => [Some(c.first_name.clone()), c.last_name.clone()]
            .into_iter()
            .flatten()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        None => lead
            .title
            .clone()
            .ok_or_else(|| internal("TypeError: 'NoneType' object is not subscriptable".into()))?,
    };
    let country = lead
        .attribution
        .as_ref()
        .and_then(|a| get(a, "country"))
        .filter(|v| truthy(v))
        .map(py_str)
        .or_else(|| {
            org.as_ref()
                .and_then(|o| o.country.clone())
                .filter(|c| !c.is_empty())
        });
    sqlx::query(
        "INSERT INTO partner_leads (id, partner_id, assigned_by_id, customer_name, customer_email, customer_company, \
         customer_country, product_interest, notes, status, assigned_at, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'new', $10, false)",
    )
    .bind(Uuid::new_v4())
    .bind(partner.id)
    .bind(actor_id)
    .bind(cut(&customer_name, 255))
    .bind(contact.as_ref().and_then(|c| c.email.clone()))
    .bind(org.as_ref().map(|o| o.name.clone()))
    .bind(country)
    .bind(lead.interested_products.clone().unwrap_or_default())
    .bind(cut(lead.description.as_deref().unwrap_or(""), 2000))
    .bind(Utc::now())
    .execute(&mut *db)
    .await?;
    sqlx::query("UPDATE leads SET partner_id = $1, updated_at = now() WHERE id = $2")
        .bind(partner.id)
        .bind(lead.id)
        .execute(&mut *db)
        .await?;
    commit(db).await?;
    Ok(
        json!({"label": format!("{} routed to {}", lead.title.as_deref().unwrap_or("None"),
                               partner.company_name.as_deref().unwrap_or("None"))}),
    )
}

/// Registration windows ending soon give a warning; conflicts give the evidence (once each).
pub async fn partner_watch(db: &mut PgConnection) -> ApiResult<i64> {
    let warn = require_int(db, "nary.partners.warn_days", WHERE).await?;
    // date.today(): the process's local date.
    let today = chrono::Local::now().date_naive();
    let mut told = 0;
    let regs: Vec<PartnerDealRegistration> = sqlx::query_as(
        "SELECT * FROM partner_deal_registrations WHERE status IN ('approved', 'extended')",
    )
    .fetch_all(&mut *db)
    .await?;
    for r in regs {
        let end = r.extended_until.or(r.expires_at);
        let Some(end) = end else { continue };
        if today <= end
            && end <= today + chrono::Duration::days(warn)
            && deps::due(
                db,
                &format!("nary.partner_warn.{}", r.id),
                60 * 24 * 365,
                Utc::now(),
            )
            .await?
        {
            let approvers = deps::approver_ids(db).await?;
            let iso = pyjson::date_isoformat(end);
            let title = format!(
                "Deal registration {} ends {iso}",
                r.registration_number.as_deref().unwrap_or("None")
            );
            let body = format!(
                "{}: the partner's protection ends in {} day(s). Extend it or let it lapse.",
                r.title,
                (end - today).num_days()
            );
            let rid = r.id.to_string();
            notify(
                db,
                approvers,
                Notice::new(&title, &body, "/partners", "partner_registration", &rid)
                    .type_("nary_partner")
                    .email(false),
            )
            .await?;
            told += 1;
        }
    }
    let conflicts: Vec<PartnerDealRegistration> = sqlx::query_as(
        "SELECT * FROM partner_deal_registrations WHERE conflicting_registration_id IS NOT NULL AND status = 'pending'",
    )
    .fetch_all(&mut *db)
    .await?;
    for r in conflicts {
        let seen: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM nary_insights WHERE kind = 'partner_conflict' AND (content ->> 'registration_id') = $1",
        )
        .bind(r.id.to_string())
        .fetch_optional(&mut *db)
        .await?;
        if seen.is_some() {
            continue;
        }
        let other: Option<PartnerDealRegistration> = match r.conflicting_registration_id {
            Some(id) => get_row(db, "partner_deal_registrations", id).await?,
            None => None,
        };
        let evidence = conflict_evidence(db, &r, other.as_ref()).await?;
        sqlx::query(
            "INSERT INTO nary_insights (id, kind, deal_id, content, status) VALUES ($1, 'partner_conflict', $2, $3, 'final')",
        )
        .bind(Uuid::new_v4())
        .bind(r.deal_id)
        .bind(evidence)
        .execute(&mut *db)
        .await?;
        let approvers = deps::approver_ids(db).await?;
        let title = format!("Registration conflict: {}", r.title);
        let rid = r.id.to_string();
        notify(
            db,
            approvers,
            Notice::new(
                &title,
                "Nary collected the evidence (ERP timestamps first). You decide.",
                "/partners",
                "partner_registration",
                &rid,
            )
            .type_("nary_partner")
            .email(false),
        )
        .await?;
        told += 1;
    }
    commit(db).await?;
    Ok(told)
}

fn iso(d: Option<DateTime<Utc>>) -> Value {
    d.map(|d| Value::from(PyDateTime::from_chrono_utc(&d).isoformat()))
        .unwrap_or(Value::Null)
}

/// What the ERP knew and when: the strongest evidence in a registration dispute.
pub async fn conflict_evidence(
    db: &mut PgConnection,
    reg: &PartnerDealRegistration,
    other: Option<&PartnerDealRegistration>,
) -> ApiResult<Value> {
    let org_id = reg
        .customer_org_id
        .or(other.and_then(|o| o.customer_org_id));
    let (mut first_lead, mut first_deal, mut first_activity) = (None, None, None);
    if let Some(org_id) = org_id {
        first_lead = sqlx::query_scalar(
            "SELECT created_at FROM leads WHERE organization_id = $1 ORDER BY created_at LIMIT 1",
        )
        .bind(org_id)
        .fetch_optional(&mut *db)
        .await?;
        first_deal = sqlx::query_scalar(
            "SELECT created_at FROM deals WHERE organization_id = $1 ORDER BY created_at LIMIT 1",
        )
        .bind(org_id)
        .fetch_optional(&mut *db)
        .await?;
        first_activity = sqlx::query_scalar(
            "SELECT activities.created_at FROM activities JOIN contacts ON contacts.id = activities.contact_id \
             WHERE contacts.organization_id = $1 ORDER BY activities.created_at LIMIT 1",
        )
        .bind(org_id)
        .fetch_optional(&mut *db)
        .await?;
    }
    Ok(
        json!({"registration_id": reg.id.to_string(), "registration": reg.registration_number,
              "registered_at": iso(Some(reg.registered_at)),
              "other": other.map(|o| o.registration_number.clone()).unwrap_or(None),
              "other_registered_at": other.map(|o| iso(Some(o.registered_at))).unwrap_or(Value::Null),
              "erp_first_lead": iso(first_lead), "erp_first_deal": iso(first_deal),
              "erp_first_activity": iso(first_activity)}),
    )
}

/// Adapters onto the other modules' ports.
mod deps {
    use super::*;

    pub async fn start_run(
        db: &mut PgConnection,
        trigger_type: &str,
        job: &str,
        trigger_ref: Option<&str>,
        sop_ref: Option<&str>,
    ) -> ApiResult<AgentRun> {
        crate::nary::runs::start_run(db, trigger_type, job, trigger_ref, "internal", sop_ref).await
    }
    pub async fn finish_run(
        db: &mut PgConnection,
        run: &mut AgentRun,
        status: &str,
        outcome: Option<&str>,
        error: Option<&str>,
    ) -> ApiResult<()> {
        crate::nary::runs::finish_run(db, run, status, outcome, error).await
    }
    pub fn information(source: &str, text: &str, limit: usize) -> String {
        crate::nary::llm::information(source, text, limit)
    }
    pub async fn ask_json(
        db: &mut PgConnection,
        job: &str,
        system: &str,
        content: &str,
        schema: &Value,
        run: Option<&mut AgentRun>,
    ) -> ApiResult<Option<Value>> {
        crate::nary::llm::ask_json(db, job, system, content, schema, run, None, None).await
    }
    pub async fn holder(db: &mut PgConnection, sop_role: &str) -> ApiResult<Option<Uuid>> {
        crate::nary::roles::holder(db, sop_role).await
    }
    pub async fn approver_ids(db: &mut PgConnection) -> ApiResult<Vec<Uuid>> {
        crate::nary::roles::approver_ids(db).await
    }
    pub async fn due(
        db: &mut PgConnection,
        key: &str,
        minutes: i64,
        now: DateTime<Utc>,
    ) -> ApiResult<bool> {
        crate::nary::channels::_due(db, key, minutes, now).await
    }
}
