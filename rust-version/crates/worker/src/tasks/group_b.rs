//! W5-B's tasks of backend/app/tasks.py (docs/WAVE5_BRIEF.md): nary_channels_tick, nary_playbooks_tick,
//! nary_sop_revisions, nary_style_refresh, erp_guide_sync, nary_tick, fire_due_reminders,
//! propose_lead_merges, generate_followup_reminders, weekly_followup_digest, enrich_organization_task,
//! bulk_ai_field_task, refresh_stale_ai_fields, research_task.
//!
//! `erp_guide_sync` is also enqueued when a worker starts (tasks.py `_erp_guide_on_start`, ported in
//! crate::startup).

use std::sync::RwLock;

use chrono::{Datelike, TimeDelta, Timelike, Utc};
use futures::future::BoxFuture;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{query_permissions, AuthUser};
use granary_core::errors::ApiError;
use granary_db::rows::crm::Organization;
use granary_services::ai::enrichment::{self, EnrichError};
use granary_services::approvals::notify;

use crate::registry::{Outcome, Param, TaskCtx, TaskDef, TaskResult};
use crate::tasks::common::{_already_ran, _mark_ran, head, py_str, MARK_RAN_LABEL};
use crate::tasks::err;

pub const TASKS: &[TaskDef] = &[
    TaskDef::new("app.tasks.nary_channels_tick", &[], |ctx| {
        Box::pin(nary_channels_tick(ctx))
    }),
    TaskDef::new("app.tasks.nary_playbooks_tick", &[], |ctx| {
        Box::pin(nary_playbooks_tick(ctx))
    }),
    TaskDef::new(
        "app.tasks.nary_sop_revisions",
        &[Param::req("codes"), Param::req("user_id")],
        |ctx| Box::pin(nary_sop_revisions(ctx)),
    ),
    TaskDef::new("app.tasks.nary_style_refresh", &[], |ctx| {
        Box::pin(nary_style_refresh(ctx))
    }),
    TaskDef::new("app.tasks.erp_guide_sync", &[], |ctx| {
        Box::pin(erp_guide_sync(ctx))
    }),
    TaskDef::new("app.tasks.nary_tick", &[], |ctx| Box::pin(nary_tick(ctx))),
    TaskDef::new("app.tasks.fire_due_reminders", &[], |ctx| {
        Box::pin(fire_due_reminders(ctx))
    }),
    TaskDef::new("app.tasks.propose_lead_merges", &[], |ctx| {
        Box::pin(propose_lead_merges(ctx))
    }),
    TaskDef::new("app.tasks.generate_followup_reminders", &[], |ctx| {
        Box::pin(generate_followup_reminders(ctx))
    }),
    TaskDef::new("app.tasks.weekly_followup_digest", &[], |ctx| {
        Box::pin(weekly_followup_digest(ctx))
    }),
    TaskDef::new(
        "app.tasks.enrich_organization_task",
        &[Param::req("org_id"), Param::opt("user_id", "null")],
        |ctx| Box::pin(enrich_organization_task(ctx)),
    ),
    TaskDef::new(
        "app.tasks.bulk_ai_field_task",
        &[
            Param::req("object_type"),
            Param::req("record_ids"),
            Param::req("user_id"),
            Param::opt("attribute_key", "null"),
            Param::opt("action", "\"attribute\""),
        ],
        |ctx| Box::pin(bulk_ai_field_task(ctx)),
    ),
    TaskDef::new("app.tasks.refresh_stale_ai_fields", &[], |ctx| {
        Box::pin(refresh_stale_ai_fields(ctx))
    }),
    TaskDef::new(
        "app.tasks.research_task",
        &[Param::req("run_id"), Param::opt("user_id", "null")],
        |ctx| Box::pin(research_task(ctx)),
    ),
];

// ─── helpers ─────────────────────────────────────────────────────────────────

/// `uuid.UUID(text)`: a ValueError ("badly formed hexadecimal UUID string") raises the task.
fn parse_uuid(text: &str) -> Result<Uuid, anyhow::Error> {
    Uuid::parse_str(text.trim_matches(|c: char| c.is_whitespace()))
        .map_err(|_| anyhow::anyhow!("badly formed hexadecimal UUID string"))
}

/// `await db.get(User, uuid)` as the ORM loads it (no `is_active` filter), with the permissions
/// `_collect_permissions(user)` would read from its roles.
async fn get_user(db: &mut PgConnection, id: Uuid) -> Result<Option<AuthUser>, ApiError> {
    let user: Option<AuthUser> = sqlx::query_as(
        "SELECT id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, \
         is_system_admin, portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, \
         created_at, updated_at FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(mut user) = user else {
        return Ok(None);
    };
    user.perms = query_permissions(db, user.id).await?;
    Ok(Some(user))
}

/// A settings/clock error (`SettingError` / database error) raised by the task.
fn serr(e: granary_core::settings_store::StoreError) -> anyhow::Error {
    err(ApiError::from(e))
}

/// `type(exc).__name__` and `str(exc)` of the exception Python would have raised for this error.
fn py_exception(e: &ApiError) -> (&'static str, String) {
    match e {
        ApiError::App { detail, .. } => ("AppException", detail.clone()),
        ApiError::Http { status, detail, .. } => {
            let text = match detail {
                Value::String(s) => s.clone(),
                other => py_str(other),
            };
            // SettingError is the HTTPException subclass whose detail reads "Setting '<key>' ... Set it in".
            let name = if status.as_u16() == 400
                && text.starts_with("Setting '")
                && text.contains(". Set it in ")
            {
                "SettingError"
            } else {
                "HTTPException"
            };
            (name, format!("{}: {text}", status.as_u16()))
        }
        ApiError::Integrity(s) => ("IntegrityError", s.clone()),
        ApiError::InvalidInput { message, .. } => ("ValueError", message.to_string()),
        ApiError::RequestValidation(_) => ("RequestValidationError", e.to_string()),
        ApiError::Internal(inner) => ("Exception", inner.to_string()),
    }
}

/// Python `str(list)` items: a list argument's members as `str`.
fn str_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => py_str(other),
            })
            .collect(),
        Value::Null => Vec::new(),
        other => vec![py_str(other)],
    }
}

// ─── Nary ────────────────────────────────────────────────────────────────────

/// `nary_channels_tick()`: Nary's channels (services/nary/channels.py): missed [whatsapp] forms, takeover
/// check-ins, owed acknowledgements, the WhatsApp health check, the gap check and the history import,
/// each at the interval its setting gives.
pub async fn nary_channels_tick(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let out = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::nary::channels::tick(&mut conn)
            .await
            .map_err(err)?
    };
    Outcome::done(py_str(&out))
}

/// `nary_playbooks_tick()`: Nary's sales playbooks (services/nary/playbooks.py): meeting reminders and
/// briefs, the follow-up cadence, unsigned-contract nudges, partner registration checks.
pub async fn nary_playbooks_tick(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let out = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::nary::playbooks::tick(&mut conn)
            .await
            .map_err(err)?
    };
    Outcome::done(py_str(&out))
}

/// `nary_sop_revisions(codes, user_id)`: draft SOP revisions with Nary (services/nary/sop_revisions.py);
/// started from the Nary page.
pub async fn nary_sop_revisions(ctx: TaskCtx) -> TaskResult {
    let codes = str_list(ctx.arg("codes"));
    // `db.get(User, user_id)` with a str: a malformed id is a database error (the task raises).
    let user_id = parse_uuid(&ctx.str_arg("user_id"))?;
    let db = ctx.session();
    let out = {
        let mut conn = db.conn().await.map_err(err)?;
        let Some(user) = granary_services::nary::roles::load_actor(&mut conn, user_id)
            .await
            .map_err(err)?
        else {
            return Outcome::done("user not found");
        };
        granary_services::nary::sop_revisions::draft_many(&mut conn, &codes, &user)
            .await
            .map_err(err)?
    };
    notify::deliver_outbox_now().await;
    Outcome::done(py_str(&out))
}

/// `nary_style_refresh()`: propose a refreshed style guide (WhatsApp + email) for the Owner every
/// nary.style.refresh_days.
pub async fn nary_style_refresh(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let out = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::nary::history::refresh_style(&mut conn)
            .await
            .map_err(err)?
    };
    Outcome::done(py_str(&out))
}

/// `erp_guide_sync()`: keep the ERP guide (services/ai/erp_guide.py) in AI Knowledge in step with the code
/// and settings.
pub async fn erp_guide_sync(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let out = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::ai::erp_guide::sync(&mut conn)
            .await
            .map_err(err)?
    };
    Outcome::done(py_str(&out))
}

/// `nary_tick()`: Nary's rhythm: briefs whose time has come, the heartbeat, expiring cards (settings
/// nary.*). A failure becomes an error card for the Owner, then the task raises.
pub async fn nary_tick(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let res = {
        match db.conn().await {
            Ok(mut conn) => granary_services::nary::rhythm::tick(&mut conn).await,
            Err(e) => Err(e),
        }
    };
    match res {
        Ok(out) => {
            // The weekly report / loss review emails queued by the tick.
            notify::deliver_outbox_now().await;
            Outcome::done(py_str(&out))
        }
        Err(exc) => {
            let (name, text) = py_exception(&exc);
            {
                let mut conn = db.conn().await.map_err(err)?;
                granary_services::nary::governance::error_card(
                    &mut conn,
                    "nary.tick",
                    "Nary's schedule failed",
                    &format!("{name}: {text}"),
                    "/nary/runs",
                )
                .await
                .map_err(err)?;
            }
            Err(err(exc))
        }
    }
}

// ─── CRM follow-ups ──────────────────────────────────────────────────────────

/// `fire_due_reminders()`: deliver follow-ups that have fallen due (bell + WhatsApp), and email the ones
/// still open a day later.
pub async fn fire_due_reminders(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let result = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::crm::followups::fire_due(&mut conn, None)
            .await
            .map_err(err)?
    };
    notify::deliver_outbox_now().await;
    if result.quiet != 0 {
        return Outcome::done("Outside delivery hours: nothing sent");
    }
    Outcome::done(format!(
        "Fired {} follow-up(s), escalated {}",
        result.due, result.escalated
    ))
}

/// `propose_all(db, actor)` with `actor=None` (no Owner to act as): the cards are proposed without an
/// actor, as Python passes `None` through to `executor.propose`.
async fn propose_all_without_actor(db: &mut PgConnection) -> Result<i64, ApiError> {
    use granary_services::nary::executor::{propose, ProposeOpts};
    let mut made = 0;
    for (keep, drop, why) in granary_services::crm::lead_merge::find_pairs(db).await? {
        let word = why.split_whitespace().nth(1).unwrap_or("");
        let title = |t: &Option<String>| t.clone().unwrap_or_else(|| "None".to_string());
        let rationale = format!(
            "Two open leads for the {word}: '{}' duplicates '{}'. Merging keeps one lead with everything on it.",
            title(&drop.title),
            title(&keep.title)
        );
        propose(
            db,
            "lead.merge",
            serde_json::json!({"keep_id": keep.id.hyphenated().to_string(),
                "drop_id": drop.id.hyphenated().to_string()}),
            None::<&AuthUser>,
            ProposeOpts {
                rationale: Some(&rationale),
                sop_ref: None,
                ..Default::default()
            },
        )
        .await?;
        made += 1;
    }
    Ok(made)
}

/// `propose_lead_merges()`: once a day: two open leads of the same person -> a lead.merge card for the
/// Owner (services/crm/lead_merge.py).
pub async fn propose_lead_merges(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let today = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_core::clock::today_local(&mut conn)
            .await
            .map_err(serr)?
            .to_string()
    };
    let paused = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::nary::governance::is_paused(&mut conn)
            .await
            .map_err(err)?
    };
    if paused
        || _already_ran(&db, "crm.lead_merge.last_pass", &today)
            .await
            .map_err(err)?
    {
        return Outcome::done("Skipped (paused or already ran today)");
    }
    let made = {
        let mut conn = db.conn().await.map_err(err)?;
        let actor = granary_services::nary::roles::nary_actor(&mut conn)
            .await
            .map_err(err)?;
        match actor {
            Some(actor) => granary_services::crm::lead_merge::propose_all(&mut conn, &actor)
                .await
                .map_err(err)?,
            None => propose_all_without_actor(&mut conn).await.map_err(err)?,
        }
    };
    // R&D 26 Oct: and two open deals of one customer -> a deal.merge card (the newer one held meanwhile)
    let deals = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::crm::deal_duplicates::scan(&mut conn, None)
            .await
            .map_err(err)?
    };
    _mark_ran(
        &db,
        "crm.lead_merge.last_pass",
        &today,
        "crm",
        "Internal marker: last duplicate-lead pass",
    )
    .await
    .map_err(err)?;
    db.commit().await.map_err(err)?;
    if deals == 0 {
        return Outcome::done(format!("Raised {made} merge card(s)"));
    }
    Outcome::done(format!(
        "Raised {made} merge card(s), {deals} deal merge card(s)"
    ))
}

/// `generate_followup_reminders()`: raise follow-ups from the rules (no reply, idle lead, stuck deal,
/// unanswered quotation) once a day.
pub async fn generate_followup_reminders(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let today = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_core::clock::today_local(&mut conn)
            .await
            .map_err(serr)?
            .to_string()
    };
    if _already_ran(&db, "crm.followup.last_rule_pass", &today)
        .await
        .map_err(err)?
    {
        return Outcome::done("Rules already ran today");
    }
    let made = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::crm::followups::generate(&mut conn)
            .await
            .map_err(err)?
    };
    _mark_ran(
        &db,
        "crm.followup.last_rule_pass",
        &today,
        "crm",
        MARK_RAN_LABEL,
    )
    .await
    .map_err(err)?;
    let parts: Vec<String> = made
        .iter()
        .filter(|(_, n)| *n != 0)
        .map(|(rule, n)| format!("{n} {rule}"))
        .collect();
    let joined = if parts.is_empty() {
        "no follow-ups".to_string()
    } else {
        parts.join(", ")
    };
    Outcome::done(format!("Raised {joined}"))
}

/// Python `int(text)` for a str: ValueError text when it is not an integer.
fn py_int(text: &str) -> Result<i64, anyhow::Error> {
    granary_compat::crypto::mcp::py_int_str(text).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid literal for int() with base 10: {}",
            granary_compat::pyjson::repr(&granary_compat::pyjson::PyValue::Str(text.to_string()))
                .unwrap_or_else(|_| format!("'{text}'"))
        )
    })
}

/// `weekly_followup_digest()`: once a week: everything overdue across the team, in one message to the Owner.
pub async fn weekly_followup_digest(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let (cfg, now) = {
        let mut conn = db.conn().await.map_err(err)?;
        let cfg = granary_services::crm::followups::settings(&mut conn)
            .await
            .map_err(err)?;
        let now = granary_core::clock::now_local(&mut conn)
            .await
            .map_err(serr)?;
        (cfg, now)
    };
    // `hour, _, _minute = str(cfg["digest_time"]).partition(":")`
    let hour = cfg
        .digest_time
        .split_once(':')
        .map(|(h, _)| h)
        .unwrap_or(&cfg.digest_time);
    // `now.isoweekday() != int(weekday) or now.hour != int(hour or 8)` (short-circuit: the hour is only
    // parsed on the digest weekday).
    if i64::from(now.weekday().number_from_monday()) != cfg.digest_weekday
        || i64::from(now.hour()) != if hour.is_empty() { 8 } else { py_int(hour)? }
    {
        return Outcome::done("Not the digest hour");
    }
    let iso = now.iso_week();
    let week = format!("{}-W{}", iso.year(), iso.week());
    if _already_ran(&db, "crm.followup.last_digest", &week)
        .await
        .map_err(err)?
    {
        return Outcome::done("Digest already sent this week");
    }
    let total = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::crm::followups::weekly_digest(&mut conn, None)
            .await
            .map_err(err)?
    };
    _mark_ran(
        &db,
        "crm.followup.last_digest",
        &week,
        "crm",
        MARK_RAN_LABEL,
    )
    .await
    .map_err(err)?;
    notify::deliver_outbox_now().await;
    if total != 0 {
        Outcome::done(format!("Digest sent for {total} overdue follow-up(s)"))
    } else {
        Outcome::done("Nothing overdue: no digest")
    }
}

// ─── CRM enrichment (company research) ───────────────────────────────────────

/// `', '.join(out['written'])` (a dict: its keys; a list: its items).
fn written_names(out: &Value) -> String {
    match &out["written"] {
        Value::Object(m) => m.keys().cloned().collect::<Vec<_>>().join(", "),
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => py_str(other),
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

/// `enrich_organization_task(org_id, user_id=None)`: research one company and fill its empty fields,
/// queued when a company is created.
pub async fn enrich_organization_task(ctx: TaskCtx) -> TaskResult {
    let org_id = parse_uuid(&ctx.str_arg("org_id"))?;
    let user_id = ctx.opt_str("user_id").filter(|s| !s.is_empty());
    let db = ctx.session();
    let (name, out) = {
        let mut conn = db.conn().await.map_err(err)?;
        let org: Option<Organization> = sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(err)?;
        let Some(mut org) = org.filter(|o| !o.is_deleted) else {
            return Outcome::done("Company no longer exists");
        };
        let user = match &user_id {
            Some(u) => get_user(&mut conn, parse_uuid(u)?).await.map_err(err)?,
            None => None,
        };
        let out = match enrichment::enrich_organization(
            &mut conn,
            &mut org,
            user.as_ref().map(|u| u.id),
            None,
        )
        .await
        {
            Ok(out) => out,
            Err(EnrichError::Unavailable(e)) => return Outcome::done(format!("Skipped: {e}")),
            Err(e) => return Err(err(e)),
        };
        (org.name.clone(), out)
    };
    db.commit().await.map_err(err)?;
    let written = written_names(&out);
    Outcome::done(format!(
        "{name}: filled {}",
        if written.is_empty() {
            "nothing"
        } else {
            &written
        }
    ))
}

/// `bulk_ai_field_task(object_type, record_ids, user_id, attribute_key=None, action="attribute")`: run one
/// AI attribute (or the web research pass) over a selection, then notify the requester.
pub async fn bulk_ai_field_task(ctx: TaskCtx) -> TaskResult {
    use granary_services::crm::bulk_ai;
    let object_type = ctx.str_arg("object_type");
    let record_ids = str_list(ctx.arg("record_ids"));
    let user_id = parse_uuid(&ctx.str_arg("user_id"))?;
    let attribute_key = ctx.opt_str("attribute_key");
    let action = ctx.str_arg("action");
    let db = ctx.session();
    let summary = {
        let mut conn = db.conn().await.map_err(err)?;
        let Some(user) = get_user(&mut conn, user_id).await.map_err(err)? else {
            return Outcome::done("Requester no longer exists");
        };
        bulk_ai::run_bulk(
            &mut conn,
            &user,
            &object_type,
            &record_ids,
            attribute_key.as_deref(),
            &action,
        )
        .await
        .map_err(err)?
    };
    db.commit().await.map_err(err)?;
    {
        let mut conn = db.conn().await.map_err(err)?;
        bulk_ai::notify_finished(&mut conn, user_id, &object_type, &summary)
            .await
            .map_err(err)?;
    }
    db.commit().await.map_err(err)?;
    let s =
        |v: &Value| granary_compat::pyjson::str_value(&granary_compat::pyjson::PyValue::from(v));
    Outcome::done(format!(
        "{}: {}/{}",
        s(&summary["label"]),
        s(&summary["succeeded"]),
        s(&summary["requested"])
    ))
}

/// What `refresh_stale_ai_fields` calls for one company: `enrichment.enrich_organization(db, org)`.
pub type EnrichFn = for<'a> fn(
    &'a mut PgConnection,
    &'a mut Organization,
) -> BoxFuture<'a, Result<Value, EnrichError>>;

static ENRICH_SEAM: RwLock<Option<EnrichFn>> = RwLock::new(None);

/// Test seam (Python tests monkeypatch `enrichment.enrich_organization`): replaces the per-company call
/// of `refresh_stale_ai_fields`; `None` restores the real one.
pub fn set_refresh_enrich_seam(f: Option<EnrichFn>) {
    *ENRICH_SEAM.write().unwrap_or_else(|p| p.into_inner()) = f;
}

async fn enrich_one(db: &mut PgConnection, org: &mut Organization) -> Result<Value, EnrichError> {
    let seam = *ENRICH_SEAM.read().unwrap_or_else(|p| p.into_inner());
    match seam {
        Some(f) => f(db, org).await,
        None => enrichment::enrich_organization(db, org, None, None).await,
    }
}

/// `refresh_stale_ai_fields()`: daily: re-research the companies whose newest enrichment run is older
/// than the refresh window, oldest first, capped by the daily limit and stopping as soon as AI is
/// unavailable.
pub async fn refresh_stale_ai_fields(ctx: TaskCtx) -> TaskResult {
    let db = ctx.session();
    let today = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_core::clock::today_local(&mut conn)
            .await
            .map_err(serr)?
            .to_string()
    };
    if _already_ran(&db, "crm.enrich.last_refresh", &today)
        .await
        .map_err(err)?
    {
        return Outcome::done("Refresh already ran today");
    }
    let org_ids: Vec<Uuid> = {
        let mut conn = db.conn().await.map_err(err)?;
        let cfg = enrichment::settings(&mut conn).await.map_err(err)?;
        let cutoff = TimeDelta::try_days(cfg.refresh_days)
            .and_then(|d| Utc::now().checked_sub_signed(d))
            .ok_or_else(|| anyhow::anyhow!("date value out of range"))?;
        sqlx::query_scalar(
            "SELECT organizations.id FROM organizations \
             LEFT OUTER JOIN (SELECT crm_ai_field_runs.record_id AS record_id, \
               max(crm_ai_field_runs.created_at) AS last_run FROM crm_ai_field_runs \
               WHERE crm_ai_field_runs.object_type = 'organization' \
                 AND crm_ai_field_runs.attribute_key LIKE 'auto:%' \
               GROUP BY crm_ai_field_runs.record_id) AS anon_1 \
               ON anon_1.record_id = organizations.id \
             WHERE organizations.is_deleted = false \
               AND (anon_1.last_run IS NULL OR anon_1.last_run < $1) \
             ORDER BY anon_1.last_run ASC NULLS FIRST LIMIT $2",
        )
        .bind(cutoff)
        .bind(cfg.daily_limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(err)?
    };

    let mut done = 0;
    let mut skipped: Option<String> = None;
    for org_id in org_ids {
        // A fresh session per company, so one failure never poisons the next.
        let org_db = ctx.session();
        let res: Result<bool, EnrichError> = async {
            let mut conn = org_db.conn().await?;
            let org: Option<Organization> =
                sqlx::query_as("SELECT * FROM organizations WHERE id = $1 AND is_deleted = false")
                    .bind(org_id)
                    .fetch_optional(&mut *conn)
                    .await?;
            let Some(mut org) = org else {
                return Ok(false);
            };
            enrich_one(&mut conn, &mut org).await?;
            Ok(true)
        }
        .await;
        let res = match res {
            Ok(true) => org_db
                .commit()
                .await
                .map(|_| true)
                .map_err(EnrichError::from),
            other => other,
        };
        match res {
            Ok(false) => continue,
            Ok(true) => done += 1,
            Err(EnrichError::Unavailable(e)) => {
                // AI off, no key, no search backend or over budget: stop.
                skipped = Some(e.to_string());
                org_db.rollback().await.map_err(err)?;
                break;
            }
            Err(e) => {
                org_db.rollback().await.map_err(err)?;
                // `logger.warning("enrichment_refresh_failed", org=str(org_id), error=str(e)[:200])`
                tracing::warn!(org = %org_id, error = %head(&e.to_string(), 200), "enrichment_refresh_failed");
            }
        }
    }
    _mark_ran(
        &db,
        "crm.enrich.last_refresh",
        &today,
        "crm",
        MARK_RAN_LABEL,
    )
    .await
    .map_err(err)?;
    Outcome::done(format!(
        "Refreshed {done} company(ies){}",
        skipped
            .map(|s| format!("; stopped: {s}"))
            .unwrap_or_default()
    ))
}

/// `research_task(run_id, user_id=None)`: carry out one queued research run (company or contact) and
/// notify whoever asked.
pub async fn research_task(ctx: TaskCtx) -> TaskResult {
    let user_id = ctx.opt_str("user_id").filter(|s| !s.is_empty());
    let db = ctx.session();
    let run = {
        let mut conn = db.conn().await.map_err(err)?;
        let user = match &user_id {
            Some(u) => get_user(&mut conn, parse_uuid(u)?).await.map_err(err)?,
            None => None,
        };
        let run_id = parse_uuid(&ctx.str_arg("run_id"))?;
        granary_services::research::runs::execute(&mut conn, run_id, user.map(|u| u.id), true)
            .await
            .map_err(err)?
    };
    Outcome::done(format!(
        "Research {} {}: {}",
        run.object_type, run.record_id, run.status
    ))
}
