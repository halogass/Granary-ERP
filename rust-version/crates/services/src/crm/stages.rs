//! Port of backend/app/services/crm/stages.py: the sales funnel's one vocabulary, opportunity (deal) stages
//! and intake (lead) statuses.
//!
//! The keys, their order and what they mean (closed, won, a side state) are code: hooks, rules and Nary
//! depend on that meaning, so they are not editable. Everything numeric or company-specific is a setting
//! under `crm.funnel.*`: the default probability per stage, the loss / disqualification reason picklists
//! with their re-contact intervals, the lead source picklist and the default lead owner.
//!
//! Every deal stage change goes through [`change_deal_stage`] (REST create / PATCH / stage, promotion,
//! partner approval and the AI/MCP write path), so the close date, probability, partner hook, stage
//! history and re-contact reminder behave the same on every path. The UI reads all of it from
//! GET /crm/stages ([`registry`]).
#![allow(clippy::too_many_arguments)]

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::{pystr, relativedelta};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, require_json, SettingError, StoreError};
use granary_db::rows::crm::Deal;

use super::attributes::py;

pub const WHERE: &str = "Sales & CRM → Funnel";

/// `Stage` (frozen dataclass).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage {
    pub key: &'static str,
    pub label: &'static str,
    pub order: i64,
    pub is_closed: bool,
    pub is_won: bool,
    /// a side state (nurture / lost / disqualified), not a rung of the ladder
    pub side: bool,
}

const fn st(
    key: &'static str,
    label: &'static str,
    order: i64,
    is_closed: bool,
    is_won: bool,
    side: bool,
) -> Stage {
    Stage {
        key,
        label,
        order,
        is_closed,
        is_won,
        side,
    }
}

/// The ladder from the sales SOPs and the website's engagement ladder (design §5.2), then the side states.
pub const DEAL_STAGES: [Stage; 12] = [
    st("new", "New", 1, false, false, false),
    st("qualifying", "Qualifying", 2, false, false, false),
    st("discovery", "Discovery", 3, false, false, false),
    st("assessment", "Assessment", 4, false, false, false),
    st("proposal", "Proposal", 5, false, false, false),
    st("contract", "Contract", 6, false, false, false),
    st("won", "Won", 7, true, true, false),
    st("handover", "Handover", 8, true, true, false),
    st("onboarding", "Onboarding", 9, true, true, false),
    st("nurture", "Nurture", 10, false, false, true),
    st("lost", "Lost", 11, true, false, true),
    st("disqualified", "Disqualified", 12, true, false, true),
];

pub const WON_STAGES: &[&str] = &["won", "handover", "onboarding"];
pub const CLOSED_STAGES: &[&str] = &["won", "handover", "onboarding", "lost", "disqualified"];
/// Open and on the ladder: what the dashboard counts as active and the stale-deal rule watches.
pub const ACTIVE_STAGES: &[&str] = &[
    "new",
    "qualifying",
    "discovery",
    "assessment",
    "proposal",
    "contract",
];
/// Stages that need reasons, and the picklist setting the reasons come from.
pub const REASON_STAGES: &[(&str, &str)] = &[
    ("lost", "crm.funnel.loss_reasons"),
    ("disqualified", "crm.funnel.disqualify_reasons"),
];

/// `REASON_STAGES.get(stage)`.
pub fn reason_setting(stage: &str) -> Option<&'static str> {
    REASON_STAGES
        .iter()
        .find(|(s, _)| *s == stage)
        .map(|(_, k)| *k)
}

/// `LeadStatus` (frozen dataclass).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeadStatus {
    pub key: &'static str,
    pub label: &'static str,
    pub order: i64,
    pub is_closed: bool,
    /// being worked (dashboard "open", the stale-lead rule)
    pub is_active: bool,
}

const fn ls(
    key: &'static str,
    label: &'static str,
    order: i64,
    is_closed: bool,
    is_active: bool,
) -> LeadStatus {
    LeadStatus {
        key,
        label,
        order,
        is_closed,
        is_active,
    }
}

/// The intake layer (design §5.1). "promoted" is set only by POST /crm/leads/{id}/promote.
pub const LEAD_STATUSES: [LeadStatus; 6] = [
    ls("new", "New", 1, false, true),
    ls("triaging", "Triaging", 2, false, true),
    ls("qualifying", "Qualifying", 3, false, true),
    ls("nurture", "Nurture", 4, false, false),
    ls("disqualified", "Disqualified", 5, true, false),
    ls("promoted", "Promoted", 6, true, false),
];
pub const ACTIVE_LEAD_STATUSES: &[&str] = &["new", "triaging", "qualifying"];
pub const CLOSED_LEAD_STATUSES: &[&str] = &["disqualified", "promoted"];
/// Where a lead may start (create) and where it may move by hand. Promotion is not a hand move.
pub const LEAD_START_STATUSES: &[&str] = &["new", "triaging", "qualifying", "nurture"];
/// `LEAD_TRANSITIONS` (each target list in sorted order, which is how the registry serves it).
pub const LEAD_TRANSITIONS: &[(&str, &[&str])] = &[
    (
        "new",
        &["disqualified", "nurture", "qualifying", "triaging"],
    ),
    (
        "triaging",
        &["disqualified", "new", "nurture", "qualifying"],
    ),
    ("qualifying", &["disqualified", "nurture", "triaging"]),
    ("nurture", &["disqualified", "qualifying", "triaging"]),
    ("disqualified", &["new", "nurture"]),
    ("promoted", &[]),
];
pub const PROMOTABLE_LEAD_STATUSES: &[&str] = &["new", "triaging", "qualifying", "nurture"];

pub const ACTIVITY_DIRECTIONS: &[&str] = &["inbound", "outbound", "internal"];
pub const ACTIVITY_ENTITIES: &[&str] = &[
    "lead",
    "deal",
    "contact",
    "organization",
    "project",
    "vendor",
];

/// `StageError(ValueError)`: an unknown stage/status, a move that isn't allowed, or missing reasons: a 400
/// for the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageError(pub String);

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StageError {}

impl From<StageError> for ApiError {
    /// routers/crm.py `_bad(e)`: `HTTPException(400, str(e))`.
    fn from(e: StageError) -> ApiError {
        ApiError::http(400, e.0)
    }
}

/// A StageError (what `except StageError` catches) or anything else Python lets propagate.
#[derive(Debug)]
pub enum StageFail {
    Stage(StageError),
    Api(ApiError),
}

pub type StageResult<T> = Result<T, StageFail>;

impl From<StageError> for StageFail {
    fn from(e: StageError) -> Self {
        StageFail::Stage(e)
    }
}
impl From<ApiError> for StageFail {
    fn from(e: ApiError) -> Self {
        StageFail::Api(e)
    }
}
impl From<sqlx::Error> for StageFail {
    fn from(e: sqlx::Error) -> Self {
        StageFail::Api(e.into())
    }
}
impl From<StoreError> for StageFail {
    fn from(e: StoreError) -> Self {
        StageFail::Api(e.into())
    }
}
impl From<SettingError> for StageFail {
    fn from(e: SettingError) -> Self {
        StageFail::Api(e.into())
    }
}
impl From<StageFail> for ApiError {
    fn from(e: StageFail) -> ApiError {
        match e {
            StageFail::Stage(s) => s.into(),
            StageFail::Api(a) => a,
        }
    }
}

fn internal(msg: String) -> StageFail {
    StageFail::Api(ApiError::internal(anyhow::anyhow!(msg)))
}

/// `str(x)` of an optional key the way an f-string prints it (None -> "None").
fn s_or_none(k: Option<&str>) -> &str {
    k.unwrap_or("None")
}

/// `deal_stage(key)`.
pub fn deal_stage(key: Option<&str>) -> Option<&'static Stage> {
    let k = key.unwrap_or("");
    DEAL_STAGES.iter().find(|s| s.key == k)
}

/// `is_won(key)`.
pub fn is_won(key: Option<&str>) -> bool {
    deal_stage(key).is_some_and(|s| s.is_won)
}

/// `is_closed(key)`.
pub fn is_closed(key: Option<&str>) -> bool {
    deal_stage(key).is_some_and(|s| s.is_closed)
}

fn deal_keys() -> String {
    DEAL_STAGES
        .iter()
        .map(|s| s.key)
        .collect::<Vec<_>>()
        .join(", ")
}

fn lead_keys() -> String {
    LEAD_STATUSES
        .iter()
        .map(|s| s.key)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `validate_deal_stage(key)`.
pub fn validate_deal_stage(key: Option<&str>) -> Result<String, StageError> {
    match key {
        Some(k) if DEAL_STAGES.iter().any(|s| s.key == k) => Ok(k.to_string()),
        _ => Err(StageError(format!(
            "Unknown deal stage '{}'. Use one of: {}",
            s_or_none(key),
            deal_keys()
        ))),
    }
}

/// `validate_lead_status(key, on_create=False)`.
pub fn validate_lead_status(key: Option<&str>, on_create: bool) -> Result<String, StageError> {
    let k = match key {
        Some(k) if LEAD_STATUSES.iter().any(|s| s.key == k) => k,
        _ => {
            return Err(StageError(format!(
                "Unknown lead status '{}'. Use one of: {}",
                s_or_none(key),
                lead_keys()
            )))
        }
    };
    if on_create && !LEAD_START_STATUSES.contains(&k) {
        return Err(StageError(format!(
            "A new lead can't start as {k}. Use one of: {}",
            LEAD_START_STATUSES.join(", ")
        )));
    }
    Ok(k.to_string())
}

/// `LEAD_TRANSITIONS.get(current, set())`.
pub fn lead_transitions(current: &str) -> &'static [&'static str] {
    LEAD_TRANSITIONS
        .iter()
        .find(|(k, _)| *k == current)
        .map(|(_, v)| *v)
        .unwrap_or(&[])
}

/// `validate_lead_transition(current, new)`.
pub fn validate_lead_transition(current: Option<&str>, new: &str) -> Result<String, StageError> {
    validate_lead_status(Some(new), false)?;
    if new == "promoted" {
        return Err(StageError(
            "A lead is promoted with Promote (POST /crm/leads/{id}/promote), not by its status"
                .into(),
        ));
    }
    // `current or "new"`: an empty status counts as new.
    let cur = match current {
        Some(c) if !c.is_empty() => c,
        _ => "new",
    };
    if !lead_transitions(cur).contains(&new) {
        return Err(StageError(format!(
            "A {} lead can't move to {new}",
            s_or_none(current)
        )));
    }
    Ok(new.to_string())
}

// ─── Settings ─────────────────────────────────────────────────────────────────

/// `isinstance(items, list) and all(isinstance(i, dict) and i.get("key") for i in items)`.
fn keyed_list(items: Value, key: &str, problem: &str) -> StageResult<Vec<Value>> {
    if let Value::Array(a) = &items {
        if a.iter()
            .all(|i| matches!(i, Value::Object(o) if o.get("key").is_some_and(py::truthy)))
        {
            return Ok(a.clone());
        }
    }
    Err(SettingError::new(key, problem, WHERE).into())
}

/// `reason_picklist(db, key)`: [{key, label, recontact_months|null}] from a reason setting; malformed ->
/// SettingError.
pub async fn reason_picklist(db: &mut PgConnection, key: &str) -> StageResult<Vec<Value>> {
    let items = require_json(db, key, WHERE).await?;
    keyed_list(
        items,
        key,
        "must be a JSON list of {key, label, recontact_months}",
    )
}

/// `lead_sources(db)`.
pub async fn lead_sources(db: &mut PgConnection) -> StageResult<Vec<Value>> {
    let key = "crm.funnel.lead_sources";
    let items = require_json(db, key, WHERE).await?;
    keyed_list(items, key, "must be a JSON list of {key, label}")
}

/// `', '.join(values)`: a TypeError (500) when one is not a str.
fn join_strs(values: &[Value]) -> StageResult<String> {
    let mut out: Vec<&str> = Vec::with_capacity(values.len());
    for v in values {
        match v {
            Value::String(s) => out.push(s),
            other => {
                return Err(internal(format!(
                    "TypeError: sequence item: expected str instance, {} found",
                    py::type_name(other)
                )))
            }
        }
    }
    Ok(out.join(", "))
}

fn keys_of(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .map(|i| i.get("key").cloned().unwrap_or(Value::Null))
        .collect()
}

/// `value in keys` for a str value (Python `==`: only an equal str matches).
fn contains_str(keys: &[Value], v: Option<&str>) -> bool {
    match v {
        None => keys.iter().any(|k| k.is_null()),
        Some(s) => keys.iter().any(|k| k.as_str() == Some(s)),
    }
}

/// `validate_lead_source(db, source)`.
pub async fn validate_lead_source(
    db: &mut PgConnection,
    source: Option<&str>,
) -> StageResult<String> {
    let keys = keys_of(&lead_sources(db).await?);
    if !contains_str(&keys, source) {
        return Err(StageError(format!(
            "Unknown lead source '{}'. Use one of: {}",
            s_or_none(source),
            join_strs(&keys)?
        ))
        .into());
    }
    Ok(source.unwrap_or_default().to_string())
}

/// `_picklist(db, key)`.
async fn picklist(db: &mut PgConnection, key: &str) -> StageResult<Vec<Value>> {
    let items = require_json(db, key, WHERE).await?;
    keyed_list(items, key, "must be a JSON list of {key, label}")
}

/// `lead_priorities(db)`.
pub async fn lead_priorities(db: &mut PgConnection) -> StageResult<Vec<Value>> {
    picklist(db, "crm.funnel.lead_priorities").await
}

/// `activity_types(db)`.
pub async fn activity_types(db: &mut PgConnection) -> StageResult<Vec<Value>> {
    picklist(db, "crm.activity_types").await
}

/// `validate_lead_priority(db, priority)`.
pub async fn validate_lead_priority(
    db: &mut PgConnection,
    priority: Option<&str>,
) -> StageResult<String> {
    let keys = keys_of(&lead_priorities(db).await?);
    if !contains_str(&keys, priority) {
        return Err(StageError(format!(
            "Unknown lead priority '{}'. Use one of: {}",
            s_or_none(priority),
            join_strs(&keys)?
        ))
        .into());
    }
    Ok(priority.unwrap_or_default().to_string())
}

/// `validate_activity(db, fields)`: an activity's type, direction and the kind of record it is logged on.
/// `fields` is the submitted dict (only the keys present are checked).
pub async fn validate_activity(
    db: &mut PgConnection,
    fields: &Map<String, Value>,
) -> StageResult<()> {
    if let Some(at) = fields.get("activity_type") {
        let keys = keys_of(&activity_types(db).await?);
        if !keys.iter().any(|k| k == at) {
            return Err(StageError(format!(
                "Unknown activity type '{}'. Use one of: {}",
                py::str_of(at),
                join_strs(&keys)?
            ))
            .into());
        }
    }
    let direction = fields.get("direction");
    let dir_empty = matches!(direction, None | Some(Value::Null))
        || direction.and_then(|d| d.as_str()) == Some("");
    if !dir_empty {
        let d = direction.unwrap();
        if !ACTIVITY_DIRECTIONS.iter().any(|x| d.as_str() == Some(x)) {
            return Err(StageError(format!(
                "Unknown direction '{}'. Use one of: {}",
                py::str_of(d),
                ACTIVITY_DIRECTIONS.join(", ")
            ))
            .into());
        }
    }
    if let Some(et) = fields.get("entity_type") {
        if !ACTIVITY_ENTITIES.iter().any(|x| et.as_str() == Some(x)) {
            return Err(StageError(format!(
                "Unknown entity_type '{}'. Use one of: {}",
                py::str_of(et),
                ACTIVITY_ENTITIES.join(", ")
            ))
            .into());
        }
    }
    Ok(())
}

/// `validate_reasons(db, stage, reasons)`: the reason keys a lost / disqualified deal needs: at least one,
/// all from the picklist, de-duplicated in order. `reasons` is what the caller got (a str, a list, None).
pub async fn validate_reasons(
    db: &mut PgConnection,
    stage: &str,
    reasons: Option<&Value>,
) -> StageResult<Vec<Value>> {
    let setting = reason_setting(stage).ok_or_else(|| internal(format!("KeyError: '{stage}'")))?;
    let raw: Vec<Value> = match reasons {
        None => vec![],
        Some(Value::String(s)) => vec![Value::String(s.clone())],
        Some(v) if !py::truthy(v) => vec![],
        Some(v) => py::iterate(v)?,
    };
    let reasons: Vec<Value> = raw.into_iter().filter(py::truthy).collect();
    if reasons.is_empty() {
        return Err(StageError(format!("A {stage} deal needs at least one reason")).into());
    }
    let known = keys_of(&reason_picklist(db, setting).await?);
    let unknown: Vec<Value> = reasons
        .iter()
        .filter(|r| !known.contains(r))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        let mut sorted: Vec<String> = Vec::new();
        for k in &known {
            let s = py::str_of(k);
            if !sorted.contains(&s) {
                sorted.push(s);
            }
        }
        sorted.sort();
        return Err(StageError(format!(
            "Unknown {stage} reason(s): {}. Use one of: {}",
            join_strs(&unknown)?,
            sorted.join(", ")
        ))
        .into());
    }
    let mut out: Vec<Value> = Vec::new();
    for r in reasons {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    Ok(out)
}

/// `validate_disqualify_reason(db, reason)`: a disqualified lead's reason, from the disqualification picklist.
pub async fn validate_disqualify_reason(
    db: &mut PgConnection,
    reason: Option<&Value>,
) -> StageResult<Value> {
    Ok(validate_reasons(db, "disqualified", reason)
        .await?
        .remove(0))
}

/// `probability_for(db, stage)`.
pub async fn probability_for(db: &mut PgConnection, stage: &str) -> StageResult<i64> {
    let key = "crm.funnel.stage_probabilities";
    let table = require_json(db, key, WHERE).await?;
    let entry = match &table {
        Value::Object(m) => m.get(stage),
        _ => None,
    };
    let Some(entry) = entry else {
        return Err(SettingError::new(
            key,
            &format!("has no probability for the '{stage}' stage"),
            WHERE,
        )
        .into());
    };
    let value = match py::int_of(entry) {
        Ok(v) => v,
        Err(py::IntErr::Overflow) => {
            return Err(internal(
                "OverflowError: cannot convert float infinity to integer".into(),
            ))
        }
        Err(_) => {
            return Err(SettingError::new(
                key,
                &format!("has a non-integer probability for '{stage}'"),
                WHERE,
            )
            .into())
        }
    };
    if !(0..=100).contains(&value) {
        return Err(SettingError::new(
            key,
            &format!("has a probability outside 0-100 for '{stage}'"),
            WHERE,
        )
        .into());
    }
    Ok(value as i64)
}

/// `registry(db)`: everything the UI needs to render and validate the funnel (GET /crm/stages).
pub async fn registry(db: &mut PgConnection) -> StageResult<Value> {
    let probs = require_json(db, "crm.funnel.stage_probabilities", WHERE).await?;
    // `(probs or {}).get(key)`: a truthy non-dict has no .get (AttributeError).
    let probs_map = match &probs {
        Value::Object(m) => Some(m.clone()),
        v if !py::truthy(v) => None,
        _ => {
            return Err(internal(
                "AttributeError: 'list' object has no attribute 'get'".into(),
            ))
        }
    };
    let deal_stages: Vec<Value> = DEAL_STAGES
        .iter()
        .map(|s| {
            json!({
                "key": s.key, "label": s.label, "order": s.order, "is_closed": s.is_closed,
                "is_won": s.is_won, "side": s.side,
                "probability": probs_map.as_ref().and_then(|m| m.get(s.key)).cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    let lead_statuses: Vec<Value> = LEAD_STATUSES
        .iter()
        .map(|s| {
            json!({"key": s.key, "label": s.label, "order": s.order, "is_closed": s.is_closed,
                   "is_active": s.is_active})
        })
        .collect();
    let mut transitions = Map::new();
    for (k, v) in LEAD_TRANSITIONS {
        transitions.insert(k.to_string(), json!(v));
    }
    let lead_sources = lead_sources(db).await?;
    let loss = reason_picklist(db, "crm.funnel.loss_reasons").await?;
    let disq = reason_picklist(db, "crm.funnel.disqualify_reasons").await?;
    let prios = lead_priorities(db).await?;
    let atypes = activity_types(db).await?;
    Ok(json!({
        "deal_stages": deal_stages,
        "lead_statuses": lead_statuses,
        "lead_transitions": Value::Object(transitions),
        "lead_start_statuses": LEAD_START_STATUSES,
        "lead_sources": lead_sources,
        "loss_reasons": loss,
        "disqualify_reasons": disq,
        "lead_priorities": prios,
        "activity_types": atypes,
        "activity_directions": ACTIVITY_DIRECTIONS,
        "activity_entities": ACTIVITY_ENTITIES,
    }))
}

/// `default_lead_owner_id(db)`: the user who owns leads that arrive with nobody behind them (website
/// forms).
pub async fn default_lead_owner_id(db: &mut PgConnection) -> StageResult<Uuid> {
    let key = "crm.funnel.default_lead_owner_email";
    let Some(email) = get_optional(db, key).await? else {
        return Err(SettingError::new(key, "is not set", WHERE).into());
    };
    let user_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT users.id FROM users WHERE lower(users.email) = $1 AND users.is_active = true",
    )
    .bind(pystr::lower(&email))
    .fetch_optional(&mut *db)
    .await?;
    match user_id {
        Some(id) => Ok(id),
        None => {
            Err(SettingError::new(key, &format!("names no active user ('{email}')"), WHERE).into())
        }
    }
}

// ─── The one write path for deal stages ───────────────────────────────────────

/// SQLAlchemy stores a Python `None` assigned to a JSONB column as the JSON `null` (JSON.none_as_null is
/// False), not SQL NULL.
fn json_or_null(v: &Option<Value>) -> Value {
    v.clone().unwrap_or(Value::Null)
}

/// `change_deal_stage(db, deal, new_stage, *, by_id, origin="rest", reasons, note, close_date)`: move a deal
/// to `new_stage` with all its side effects; `Ok(false)` when it was already there.
///
/// Validates the stage (and the reasons a lost / disqualified deal needs), sets the stage's default
/// probability, stamps the close date on closing and clears it (and the reasons) on reopening, fires the
/// partner won / un-won hook when the deal crosses into or out of the won group, records a
/// deal_stage_events row and sets the re-contact reminder the reasons call for. `close_date` is an explicit
/// close date for a closing move (it is also the partner hook's FX date).
///
/// Rust shape of Python's in-memory ORM object: `deal` is the full row. A deal whose id is not in `deals`
/// yet is the "new deal (not flushed yet)" case: it is INSERTed here (created_at / updated_at from now(),
/// written back into `deal`). An existing deal gets its stage columns (stage, probability,
/// actual_close_date, loss_reasons, loss_reason, loss_notes, updated_at) UPDATEd; any other field the
/// caller changed is the caller's to write. The caller commits.
pub async fn change_deal_stage(
    db: &mut PgConnection,
    deal: &mut Deal,
    new_stage: &str,
    by_id: Option<Uuid>,
    origin: &str,
    reasons: Option<&Value>,
    note: Option<&str>,
    close_date: Option<NaiveDate>,
) -> StageResult<bool> {
    validate_deal_stage(Some(new_stage))?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM deals WHERE id = $1)")
        .bind(deal.id)
        .fetch_one(&mut *db)
        .await?;
    let old: Option<String> = if exists {
        Some(deal.stage.clone())
    } else {
        None
    };
    if old.as_deref() == Some(new_stage) {
        return Ok(false);
    }
    let clean_reasons = if reason_setting(new_stage).is_some() {
        Some(validate_reasons(db, new_stage, reasons).await?)
    } else {
        None
    };

    let was_won = is_won(old.as_deref());
    let was_closed = is_closed(old.as_deref());
    deal.stage = new_stage.to_string();
    deal.probability = Some(probability_for(db, new_stage).await? as i32);
    if is_closed(Some(new_stage)) {
        if let Some(cd) = close_date {
            deal.actual_close_date = Some(cd);
        } else if !was_closed || deal.actual_close_date.is_none() {
            deal.actual_close_date = Some(granary_core::clock::today_local(db).await?);
        }
    } else if was_closed {
        deal.actual_close_date = None; // reopened: no close date until it closes again
    }
    // Which reason columns this move assigns (Python only flushes attributes it set).
    let mut set_reasons = false;
    let mut set_notes = false;
    if let Some(cr) = &clean_reasons {
        deal.loss_reasons = Some(Value::Array(cr.clone()));
        deal.loss_reason = cr.first().map(py::str_of);
        set_reasons = true;
        if note.is_some_and(|n| !n.is_empty()) {
            deal.loss_notes = note.map(str::to_string);
            set_notes = true;
        }
    } else if old.as_deref().and_then(reason_setting).is_some() {
        deal.loss_reasons = None;
        deal.loss_reason = None;
        set_reasons = true;
    }

    if !exists {
        insert_deal(db, deal).await?;
    } else {
        let (ca,): (DateTime<Utc>,) = sqlx::query_as(
            "UPDATE deals SET stage = $2, probability = $3, actual_close_date = $4, \
               loss_reasons = CASE WHEN $5 THEN $6::jsonb ELSE loss_reasons END, \
               loss_reason = CASE WHEN $5 THEN $7 ELSE loss_reason END, \
               loss_notes = CASE WHEN $8 THEN $9 ELSE loss_notes END, \
               updated_at = now() \
             WHERE id = $1 RETURNING updated_at",
        )
        .bind(deal.id)
        .bind(&deal.stage)
        .bind(deal.probability)
        .bind(deal.actual_close_date)
        .bind(set_reasons)
        .bind(json_or_null(&deal.loss_reasons))
        .bind(&deal.loss_reason)
        .bind(set_notes)
        .bind(&deal.loss_notes)
        .fetch_one(&mut *db)
        .await?;
        deal.updated_at = ca;
    }
    super::deal_hooks::after_stage_change(db, deal, was_won).await?;

    let reasons_json = match &clean_reasons {
        Some(cr) => Value::Array(cr.clone()),
        None => Value::Null,
    };
    sqlx::query(
        "INSERT INTO deal_stage_events (id, deal_id, from_stage, to_stage, by_id, origin, reasons, note, \
           at, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), now(), now())",
    )
    .bind(Uuid::new_v4())
    .bind(deal.id)
    .bind(&old)
    .bind(new_stage)
    .bind(by_id)
    .bind(origin)
    .bind(&reasons_json)
    .bind(note)
    .execute(&mut *db)
    .await?;

    let payload = json!({
        "from": old, "to": new_stage, "origin": origin,
        "reasons": clean_reasons.clone().unwrap_or_default(),
    });
    crate::nary::events::emit(
        db,
        if old.is_none() {
            "deal.created"
        } else {
            "deal.staged"
        },
        Some("deal"),
        Some(deal.id),
        Some(payload),
        by_id,
    )
    .await?;
    if let Some(cr) = &clean_reasons {
        if !cr.is_empty() {
            recontact_reminder(
                db,
                "deal",
                deal.id,
                Some(deal.title.as_str()),
                new_stage,
                cr,
                by_id,
            )
            .await?;
        }
    }
    Ok(true)
}

/// INSERT of a new deal row (`db.add(deal); await db.flush()`), every column from `deal` except the
/// server-side timestamps.
async fn insert_deal(db: &mut PgConnection, deal: &mut Deal) -> StageResult<()> {
    // SQLAlchemy leaves a None attribute out of the INSERT when its column has a default, so the default
    // fires (a promoted lead without a currency gives a "USD" deal).
    if deal.currency.is_none() {
        deal.currency = Some("USD".into());
    }
    if deal.probability.is_none() {
        deal.probability = Some(20);
    }
    if deal.product_interests.is_none() {
        deal.product_interests = Some(vec![]);
    }
    if deal.product_line_ids.is_none() {
        deal.product_line_ids = Some(vec![]);
    }
    if deal.custom.is_null() {
        deal.custom = serde_json::json!({});
    }
    // the segment (crate::segment): the one given, else its lead's
    if deal.segment.is_none() {
        if let Some(lead_id) = deal.lead_id {
            deal.segment =
                sqlx::query_scalar::<_, Option<String>>("SELECT segment FROM leads WHERE id = $1")
                    .bind(lead_id)
                    .fetch_optional(&mut *db)
                    .await?
                    .flatten();
        }
    }
    let (ca, ua): (DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO deals (id, lead_id, contact_id, organization_id, title, deal_number, stage, value, currency, \
           probability, expected_close_date, actual_close_date, product_interests, product_line_ids, notes, \
           loss_reason, loss_reasons, loss_notes, assigned_to_id, created_by_id, deal_registration_id, custom, \
           is_deleted, deleted_at, segment, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, \
           $22, $23, $24, $25, now(), now()) RETURNING created_at, updated_at",
    )
    .bind(deal.id)
    .bind(deal.lead_id)
    .bind(deal.contact_id)
    .bind(deal.organization_id)
    .bind(&deal.title)
    .bind(&deal.deal_number)
    .bind(&deal.stage)
    .bind(deal.value)
    .bind(&deal.currency)
    .bind(deal.probability)
    .bind(deal.expected_close_date)
    .bind(deal.actual_close_date)
    .bind(&deal.product_interests)
    .bind(&deal.product_line_ids)
    .bind(&deal.notes)
    .bind(&deal.loss_reason)
    .bind(&deal.loss_reasons)
    .bind(&deal.loss_notes)
    .bind(deal.assigned_to_id)
    .bind(deal.created_by_id)
    .bind(deal.deal_registration_id)
    .bind(&deal.custom)
    .bind(deal.is_deleted)
    .bind(deal.deleted_at)
    .bind(&deal.segment)
    .fetch_one(&mut *db)
    .await?;
    deal.created_at = ca;
    deal.updated_at = ua;
    Ok(())
}

/// `datetime.combine(day, time(hour), tzinfo=tz).astimezone(timezone.utc)` (zoneinfo, fold=0).
fn combine_utc(day: NaiveDate, hour: u32, tz: chrono_tz::Tz) -> Option<DateTime<Utc>> {
    let naive = day.and_time(NaiveTime::from_hms_opt(hour, 0, 0)?);
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) => Some(t.with_timezone(&Utc)),
        chrono::LocalResult::Ambiguous(a, _) => Some(a.with_timezone(&Utc)), // fold=0: the first occurrence
        chrono::LocalResult::None => {
            // In a gap fold=0 keeps the offset in force before the transition.
            use chrono::Offset;
            let before = tz
                .from_local_datetime(&(naive - chrono::Duration::hours(3)))
                .earliest()?
                .offset()
                .fix();
            Some(Utc.from_utc_datetime(&(naive - before)))
        }
    }
}

/// `recontact_reminder(db, entity_type, record, stage, reasons, by_id)`: one
/// reminder at the shortest re-contact interval among the reasons of a lost or disqualified lead / deal
/// (none when no reason has an interval). `record` is passed as its id and title.
pub async fn recontact_reminder(
    db: &mut PgConnection,
    entity_type: &str,
    record_id: Uuid,
    record_title: Option<&str>,
    stage: &str,
    reasons: &[Value],
    by_id: Option<Uuid>,
) -> StageResult<()> {
    use super::followups;
    let setting = reason_setting(stage).ok_or_else(|| internal(format!("KeyError: '{stage}'")))?;
    let items = reason_picklist(db, setting).await?;
    // {i["key"]: i for i in items}: a later item with the same key wins.
    let lookup = |r: &Value| -> StageResult<&Value> {
        items
            .iter()
            .rev()
            .find(|i| i.get("key") == Some(r))
            .ok_or_else(|| internal(format!("KeyError: {}", py::str_of(r))))
    };
    let mut months: Vec<i128> = Vec::new();
    for r in reasons {
        let item = lookup(r)?;
        let rm = item.get("recontact_months");
        let empty =
            matches!(rm, None | Some(Value::Null)) || rm.and_then(|v| v.as_str()) == Some("");
        if !empty {
            let m = py::int_of(rm.unwrap())
                .map_err(|e| internal(format!("int(recontact_months) failed: {e:?}")))?;
            months.push(m);
        }
    }
    let Some(min_months) = months.iter().min().copied() else {
        return Ok(()); // e.g. Qualification Error: genuinely no re-contact
    };
    let owner = followups::owner_for(db, entity_type, record_id)
        .await?
        .or(by_id);
    let Some(owner) = owner else {
        return Ok(());
    };
    let cfg = followups::settings(db).await?;
    let tz = granary_core::clock::local_tz(db).await?;
    let today = granary_core::clock::today_local(db).await?;
    let day = relativedelta::add_months(today, min_months as i64)
        .ok_or_else(|| internal("OverflowError: date value out of range".into()))?;
    let hour = u32::try_from(cfg.due_hour)
        .ok()
        .filter(|h| *h < 24)
        .ok_or_else(|| internal("ValueError: hour must be in 0..23".into()))?;
    let due =
        combine_utc(day, hour, tz).ok_or_else(|| internal("ValueError: bad due time".into()))?;
    let mut labels: Vec<Value> = Vec::new();
    for r in reasons {
        let item = lookup(r)?;
        match item.get("label") {
            Some(l) if py::truthy(l) => labels.push(l.clone()),
            _ => labels.push(r.clone()),
        }
    }
    let labels = join_strs(&labels)?;
    let today_s = granary_compat::pyjson::date_isoformat(today);
    let day_s = granary_compat::pyjson::date_isoformat(day);
    let title = format!(
        "Re-contact: {}",
        match record_title {
            Some(t) if !t.is_empty() => t,
            _ => "untitled",
        }
    );
    let mut new = followups::NewReminder::new(owner, entity_type, &title);
    new.entity_id = Some(record_id);
    new.source = "recontact".into();
    new.note = Some(format!(
        "{} {today_s}, reason: {labels}. Re-approach angle: what has changed that makes us a better fit now?",
        capitalize(stage)
    ));
    new.due_at = Some(due);
    new.rule_key = Some(format!("recontact:{entity_type}:{record_id}:{day_s}"));
    new.created_by_id = by_id;
    followups::create(db, new).await?;
    Ok(())
}

/// `str.capitalize()` for the ASCII stage keys.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &pystr::lower(c.as_str()),
        None => String::new(),
    }
}

/// Convenience for callers holding an `ApiResult`: `except StageError as e: raise HTTPException(400, str(e))`.
pub fn to_api<T>(r: StageResult<T>) -> ApiResult<T> {
    r.map_err(ApiError::from)
}
