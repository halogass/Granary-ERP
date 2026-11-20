//! backend/app/routers/crm_records.py, mounted at `/api/v1/crm` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! CRM attribute schema + record surface (timeline, AI brief, AI attribute runs), confirmed AI writes,
//! filtered search + bulk AI runs, duplicate detection + merge.

use std::time::Duration;

use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};
use granary_db::rows::crm_attributes::CRMAttributeDef;
use granary_services::crm::attributes::{self, py, AI_KINDS, FIELD_TYPES, OBJECT_TYPES};
use granary_services::crm::bulk_ai::{self, BulkFail};
use granary_services::crm::{dedupe, search, timeline, writes};

use super::at;
use super::crm::{col_val, row_map, same_as_current, write_columns, Kind, SqlVal};
use crate::extract::parse_uuid;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/crm";

/// The routes of `router` in crm_records.py. Paths are absolute (`super::at(PREFIX, "/...")`): several Python
/// routers share a prefix, which axum's `nest` cannot express.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            &at(PREFIX, "/attributes"),
            get(list_attributes).post(create_attribute),
        )
        .route(
            &at(PREFIX, "/attributes/{attribute_id}"),
            patch(update_attribute).delete(delete_attribute),
        )
        .route(
            &at(PREFIX, "/records/{object_type}/{record_id}/timeline"),
            get(record_timeline),
        )
        .route(
            &at(PREFIX, "/records/{object_type}/{record_id}/ai-fields"),
            get(record_ai_fields),
        )
        .route(
            &at(
                PREFIX,
                "/records/{object_type}/{record_id}/ai-fields/{attribute_key}/run",
            ),
            post(run_ai_field),
        )
        .route(
            &at(PREFIX, "/records/{object_type}/{record_id}/brief"),
            post(record_brief),
        )
        .route(&at(PREFIX, "/ai/apply"), post(apply_ai_change))
        .route(&at(PREFIX, "/ai/preview"), post(preview_ai_change))
        .route(&at(PREFIX, "/search/schema"), get(search_schema))
        .route(&at(PREFIX, "/search/filters"), post(search_filters))
        .route(
            &at(PREFIX, "/records/bulk/ai-fields/run"),
            post(bulk_run_ai_field),
        )
        .route(&at(PREFIX, "/duplicates/scan"), post(duplicates_scan))
        .route(&at(PREFIX, "/duplicates"), get(duplicates_list))
        .route(&at(PREFIX, "/duplicates/preview"), get(duplicates_preview))
        .route(&at(PREFIX, "/duplicates/merge"), post(duplicates_merge))
        .route(
            &at(PREFIX, "/duplicates/{candidate_id}/dismiss"),
            post(duplicates_dismiss),
        )
}

/// `_MODELS`: object type -> table.
fn table_of(object_type: &str) -> Option<&'static str> {
    match object_type {
        "organization" => Some("organizations"),
        "contact" => Some("contacts"),
        "lead" => Some("leads"),
        "deal" => Some("deals"),
        _ => None,
    }
}

/// `_require(user, *codes)`.
fn require(user: &AuthUser, codes: &[&str]) -> ApiResult<()> {
    if user.is_superadmin || codes.iter().any(|c| user.perms.contains(*c)) {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        format!("Permission required: {}", codes.join(" or ")),
    ))
}

/// `_check_object_type(object_type)`.
fn check_object_type(object_type: &str) -> ApiResult<()> {
    if !OBJECT_TYPES.contains(&object_type) {
        return Err(ApiError::http(
            400,
            format!("object_type must be one of {}", OBJECT_TYPES.join(", ")),
        ));
    }
    Ok(())
}

/// `str.title()` of an object type (ASCII lowercase words).
fn title(t: &str) -> String {
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `_load_record(db, object_type, record_id)`: a live record's `assigned_to_id`.
async fn load_record(
    conn: &mut PgConnection,
    object_type: &str,
    record_id: Uuid,
) -> ApiResult<Option<Uuid>> {
    let table = table_of(object_type)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{object_type}'")))?;
    let rec: Option<(Option<Uuid>,)> = sqlx::query_as(&format!(
        "SELECT assigned_to_id FROM {table} WHERE id = $1 AND is_deleted = false LIMIT 1"
    ))
    .bind(record_id)
    .fetch_optional(&mut *conn)
    .await?;
    match rec {
        Some((owner,)) => Ok(owner),
        None => Err(ApiError::not_found(
            &title(object_type),
            Some(&record_id.to_string()),
        )),
    }
}

/// `_load_record` + `_assert_can_view`.
async fn visible_record(
    conn: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    record_id: Uuid,
) -> ApiResult<()> {
    let owner = load_record(conn, object_type, record_id).await?;
    granary_services::crm::access::assert_can_view(user, owner)
}

// ─── Attribute definitions ───────────────────────────────────────────────────

granary_core::py_model! {
    /// `class AttributeDefBody(BaseModel)`.
    pub struct AttributeDefBody {
        pub object_type: String,
        pub key: String,
        pub label: String,
        pub description: Option<String> = None,
        pub field_type: String,
        pub options: Option<Map<String, Value>> = None,
        pub is_ai: bool = false,
        pub ai_kind: Option<String> = None,
        pub ai_config: Option<Map<String, Value>> = None,
        pub sort_order: i64 = 0,
        pub is_active: bool = true,
    }
}

/// `_validate_def(body)`.
fn validate_def(body: &AttributeDefBody) -> ApiResult<()> {
    check_object_type(&body.object_type)?;
    if !FIELD_TYPES.contains(&body.field_type.as_str()) {
        return Err(ApiError::http(
            400,
            format!("field_type must be one of {}", FIELD_TYPES.join(", ")),
        ));
    }
    attributes::validate_key(&body.key).map_err(|e| ApiError::http(400, e.0))?;
    if body.is_ai {
        if !body
            .ai_kind
            .as_deref()
            .is_some_and(|k| AI_KINDS.contains(&k))
        {
            return Err(ApiError::http(
                400,
                format!("ai_kind must be one of {}", AI_KINDS.join(", ")),
            ));
        }
        if body.ai_kind.as_deref() == Some("classify") {
            let from = |m: &Option<Map<String, Value>>| -> Option<Value> {
                m.as_ref()
                    .and_then(|m| m.get("choices"))
                    .filter(|c| py::truthy(c))
                    .cloned()
            };
            let choices = from(&body.options).or_else(|| from(&body.ai_config));
            if choices.is_none() {
                return Err(ApiError::http(
                    400,
                    "A classify attribute needs at least one choice",
                ));
            }
        }
    }
    Ok(())
}

granary_core::py_model! {
    pub struct ListAttributesQ {
        pub object_type: Option<String> = None,
        pub include_inactive: bool = false,
    }
}

/// `GET /attributes`: the attribute schema the record pages render from.
pub async fn list_attributes(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListAttributesQ>,
) -> ApiResult<Response> {
    require(&user, &["crm:view_all", "crm:view_own"])?;
    let types: Vec<String> = match q.object_type.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => {
            check_object_type(t)?;
            vec![t.to_string()]
        }
        None => OBJECT_TYPES.iter().map(|t| t.to_string()).collect(),
    };
    let mut conn = db.conn().await?;
    let mut out = Vec::new();
    for t in &types {
        for d in attributes::load_defs(&mut conn, t, !q.include_inactive).await? {
            out.push(attributes::serialize_def(&d));
        }
    }
    Ok(resp::json(json!({"items": out})))
}

/// `POST /attributes` (201).
pub async fn create_attribute(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<AttributeDefBody>,
) -> ApiResult<Response> {
    require(&user, &["crm:manage_attributes"])?;
    validate_def(&body)?;
    let mut conn = db.conn().await?;
    let dupe: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM crm_attribute_defs WHERE object_type = $1 AND key = $2 LIMIT 1",
    )
    .bind(&body.object_type)
    .bind(&body.key)
    .fetch_optional(&mut *conn)
    .await?;
    if dupe.is_some() {
        return Err(ApiError::http(
            409,
            format!("'{}' already exists on {}", body.key, body.object_type),
        ));
    }
    let sort_order = i32::try_from(body.sort_order).map_err(|_| {
        ApiError::internal(anyhow::anyhow!(
            "invalid input for query argument: {} (value out of int32 range)",
            body.sort_order
        ))
    })?;
    // JSONB None is the JSON null (none_as_null=False).
    let json_or_null =
        |m: &Option<Map<String, Value>>| m.clone().map(Value::Object).unwrap_or(Value::Null);
    let defn: CRMAttributeDef = sqlx::query_as(
        "INSERT INTO crm_attribute_defs (object_type, key, label, description, field_type, options, is_ai, \
         ai_kind, ai_config, sort_order, is_active, created_by_id, id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now(), now()) RETURNING *",
    )
    .bind(&body.object_type)
    .bind(&body.key)
    .bind(&body.label)
    .bind(&body.description)
    .bind(&body.field_type)
    .bind(json_or_null(&body.options))
    .bind(body.is_ai)
    .bind(&body.ai_kind)
    .bind(json_or_null(&body.ai_config))
    .bind(sort_order)
    .bind(body.is_active)
    .bind(user.id)
    .bind(Uuid::new_v4())
    .fetch_one(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, attributes::serialize_def(&defn)))
}

async fn load_def(conn: &mut PgConnection, attribute_id: &str) -> ApiResult<CRMAttributeDef> {
    let id = parse_uuid(attribute_id, "attribute_id")?;
    sqlx::query_as::<_, CRMAttributeDef>("SELECT * FROM crm_attribute_defs WHERE id = $1 LIMIT 1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::not_found("Attribute", Some(attribute_id)))
}

const ATTR_ALLOWED: [(&str, Kind); 7] = [
    ("label", Kind::Text),
    ("description", Kind::Text),
    ("options", Kind::Json),
    ("ai_config", Kind::Json),
    ("sort_order", Kind::Int),
    ("is_active", Kind::Bool),
    ("field_type", Kind::Text),
];

/// `PATCH /attributes/{attribute_id}`. `object_type` and `key` are the address of every stored value:
/// they never change.
pub async fn update_attribute(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(attribute_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    require(&user, &["crm:manage_attributes"])?;
    let mut conn = db.conn().await?;
    let defn = load_def(&mut conn, &attribute_id).await?;
    // The check reads the in-memory value before the flush binds anything.
    let field_type = body
        .get("field_type")
        .cloned()
        .unwrap_or_else(|| Value::String(defn.field_type.clone()));
    let ok = matches!(&field_type, Value::String(t) if FIELD_TYPES.contains(&t.as_str()));
    if !ok {
        let shown = match &field_type {
            Value::String(t) => t.clone(),
            other => py::str_of(other),
        };
        return Err(ApiError::http(
            400,
            format!("Unsupported field_type {shown}"),
        ));
    }
    let row = row_map(&defn);
    let mut sets: Vec<(String, SqlVal)> = Vec::new();
    for (k, v) in &body {
        let Some((_, kind)) = ATTR_ALLOWED.iter().find(|(n, _)| n == k) else {
            continue;
        };
        let val = col_val(*kind, v)?;
        sets.retain(|(c, _)| c != k);
        if !same_as_current(&row, k, &val) {
            sets.push((k.clone(), val));
        }
    }
    write_columns(&mut conn, "crm_attribute_defs", defn.id, &sets).await?;
    let now: CRMAttributeDef = sqlx::query_as("SELECT * FROM crm_attribute_defs WHERE id = $1")
        .bind(defn.id)
        .fetch_one(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(attributes::serialize_def(&now)))
}

/// `DELETE /attributes/{attribute_id}` (204): deactivate rather than drop.
pub async fn delete_attribute(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(attribute_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, &["crm:manage_attributes"])?;
    let mut conn = db.conn().await?;
    let defn = load_def(&mut conn, &attribute_id).await?;
    if defn.is_active {
        sqlx::query(
            "UPDATE crm_attribute_defs SET is_active = false, updated_at = now() WHERE id = $1",
        )
        .bind(defn.id)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

// ─── Record surface ──────────────────────────────────────────────────────────

granary_core::py_model! {
    pub struct TimelineQ {
        pub limit: i64 = 60,
    }
}

/// `GET /records/{object_type}/{record_id}/timeline`.
pub async fn record_timeline(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((object_type, record_id)): Path<(String, String)>,
    PyQuery(q): PyQuery<TimelineQ>,
) -> ApiResult<Response> {
    check_object_type(&object_type)?;
    require(&user, &["crm:view_all", "crm:view_own"])?;
    let rid = parse_uuid(&record_id, "record_id")?;
    let mut conn = db.conn().await?;
    visible_record(&mut conn, &user, &object_type, rid).await?;
    let items =
        timeline::build_timeline(&mut conn, &user, &object_type, rid, q.limit.min(200)).await?;
    let related = timeline::related_counts(&mut conn, &object_type, rid).await?;
    Ok(resp::json(json!({"items": items, "related": related})))
}

/// `GET /records/{object_type}/{record_id}/ai-fields`: newest AI run per attribute.
pub async fn record_ai_fields(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((object_type, record_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    check_object_type(&object_type)?;
    require(&user, &["crm:view_all", "crm:view_own"])?;
    let rid = parse_uuid(&record_id, "record_id")?;
    let mut conn = db.conn().await?;
    visible_record(&mut conn, &user, &object_type, rid).await?;
    let runs = granary_services::ai::attributes::latest_runs(&mut conn, &object_type, rid).await?;
    Ok(resp::json(json!({"runs": runs})))
}

/// `POST /records/{object_type}/{record_id}/ai-fields/{attribute_key}/run`: recompute one AI attribute.
pub async fn run_ai_field(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((object_type, record_id, attribute_key)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    use granary_services::ai::attributes::{run_attribute, RunError};
    check_object_type(&object_type)?;
    require(
        &user,
        &["crm:edit_contact", "crm:edit_lead", "crm:edit_deal"],
    )?;
    let rid = parse_uuid(&record_id, "record_id")?;
    let mut conn = db.conn().await?;
    visible_record(&mut conn, &user, &object_type, rid).await?;
    let defn: Option<CRMAttributeDef> = sqlx::query_as(
        "SELECT * FROM crm_attribute_defs WHERE object_type = $1 AND key = $2 AND is_active = true LIMIT 1",
    )
    .bind(&object_type)
    .bind(&attribute_key)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(defn) = defn else {
        return Err(ApiError::not_found("Attribute", Some(&attribute_key)));
    };
    match run_attribute(&mut conn, &user, &defn, rid).await {
        Ok(v) => Ok(resp::json(v)),
        Err(RunError::Attr(e)) => Err(ApiError::http(400, e.0)),
        Err(RunError::Other(e)) => Err(e),
    }
}

/// `POST /records/{object_type}/{record_id}/brief`: the AI brief for a record (POST: it spends money).
pub async fn record_brief(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path((object_type, record_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    check_object_type(&object_type)?;
    require(&user, &["crm:view_all", "crm:view_own"])?;
    let rid = parse_uuid(&record_id, "record_id")?;
    let mut conn = db.conn().await?;
    visible_record(&mut conn, &user, &object_type, rid).await?;
    let brief =
        granary_services::ai::records::build_brief(&mut conn, &user, &object_type, rid).await?;
    if let Some(err) = brief.get("error").filter(|e| py::truthy(e)) {
        let detail = match err {
            Value::String(t) => t.clone(),
            other => py::str_of(other),
        };
        return Err(ApiError::http(400, detail));
    }
    Ok(resp::json(brief))
}

// ─── Phase 2: confirmed AI writes ────────────────────────────────────────────

granary_core::py_model! {
    /// `class ApplyChangeBody(BaseModel)`.
    pub struct ApplyChangeBody {
        pub object_type: String,
        pub action: String,
        pub fields: Map<String, Value>,
        pub record_id: Option<String> = None,
        /// the card it came from: resolves "@ref" links and records the outcome
        pub proposal_id: Option<String> = None,
    }
}

/// `_card_message(db, user, proposal_id)`: the assistant answer (in one of this user's chats) that holds
/// the card, and its cards.
async fn card_message(
    conn: &mut PgConnection,
    user: &AuthUser,
    proposal_id: &str,
) -> ApiResult<Option<(Uuid, Value)>> {
    let rows: Vec<(Uuid, Option<Value>)> = sqlx::query_as(
        "SELECT ai_messages.id, ai_messages.tool_calls FROM ai_messages \
         JOIN ai_conversations ON ai_conversations.id = ai_messages.conversation_id \
         WHERE ai_conversations.user_id = $1 AND ai_messages.role = 'assistant' \
         AND (ai_messages.tool_calls -> 'proposals') @> $2 LIMIT 1",
    )
    .bind(user.id)
    .bind(json!([{"proposal_id": proposal_id}]))
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .next()
        .map(|(id, tc)| (id, tc.unwrap_or(Value::Null))))
}

/// `(msg.tool_calls or {}).get("proposals") or []`.
fn proposals_of(tool_calls: &Value) -> Vec<Value> {
    match tool_calls.get("proposals") {
        Some(Value::Array(a)) => a.clone(),
        _ => vec![],
    }
}

/// `POST /ai/apply`: apply a CRM change the user confirmed in an assistant proposal card.
pub async fn apply_ai_change(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ApplyChangeBody>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let proposal_id = body.proposal_id.clone().filter(|p| !p.is_empty());
    let msg = match &proposal_id {
        Some(p) => card_message(&mut conn, &user, p).await?,
        None => None,
    };
    let mut siblings: Vec<Value> = msg
        .as_ref()
        .map(|(_, tc)| proposals_of(tc))
        .unwrap_or_default();
    let sibling_maps: Vec<Map<String, Value>> = siblings
        .iter()
        .filter_map(|p| p.as_object().cloned())
        .collect();
    let fields = writes::resolve_refs(&body.fields, &sibling_maps).map_err(ApiError::from)?;
    let record_id = match body.record_id.as_deref().filter(|r| !r.is_empty()) {
        Some(r) => Some(parse_uuid(r, "record_id")?),
        None => None,
    };
    let out = writes::apply_change(
        &mut conn,
        &user,
        &body.object_type,
        &body.action,
        &Value::Object(fields),
        record_id,
        "ai_confirmed",
    )
    .await
    .map_err(ApiError::from)?;
    // Recorded here too (not only by the card's PATCH), so a card confirmed right after can link to this one.
    if let Some((msg_id, mut tool_calls)) = msg {
        let pid = Value::String(proposal_id.clone().unwrap_or_default());
        let mut changed = false;
        for p in siblings.iter_mut() {
            if p.get("proposal_id") == Some(&pid) {
                if let Value::Object(m) = p {
                    m.insert("status".into(), json!("applied"));
                    m.insert(
                        "result".into(),
                        json!({"id": out.get("id").cloned().unwrap_or(Value::Null),
                               "label": out.get("label").cloned().unwrap_or(Value::Null)}),
                    );
                    changed = true;
                }
            }
        }
        if changed {
            if let Value::Object(tc) = &mut tool_calls {
                tc.insert("proposals".into(), Value::Array(siblings));
            }
            sqlx::query("UPDATE ai_messages SET tool_calls = $2, updated_at = now() WHERE id = $1")
                .bind(msg_id)
                .bind(&tool_calls)
                .execute(&mut *conn)
                .await?;
            drop(conn);
            db.commit().await?;
        }
    }
    Ok(resp::json(out))
}

/// `POST /ai/preview`: re-derive the before/after for a proposal.
pub async fn preview_ai_change(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ApplyChangeBody>,
) -> ApiResult<Response> {
    require(&user, &["crm:view_all", "crm:view_own"])?;
    let record_id = match body.record_id.as_deref().filter(|r| !r.is_empty()) {
        Some(r) => Some(parse_uuid(r, "record_id")?),
        None => None,
    };
    let mut conn = db.conn().await?;
    if let Some(rid) = record_id {
        if table_of(&body.object_type).is_some() {
            // The diff shows the record's current values: the same owner scoping as reading it.
            visible_record(&mut conn, &user, &body.object_type, rid).await?;
        }
    }
    let diff = writes::build_diff(
        &mut conn,
        &body.object_type,
        &body.action,
        &Value::Object(body.fields.clone()),
        record_id,
        false,
    )
    .await
    .map_err(ApiError::from)?;
    Ok(resp::json(diff))
}

// ─── Filtered search + bulk AI runs ──────────────────────────────────────────

granary_core::py_model! {
    pub struct SchemaQ {
        pub object_type: String,
    }
}

/// `GET /search/schema`: fields and operators a filter may reference.
pub async fn search_schema(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<SchemaQ>,
) -> ApiResult<Response> {
    require(&user, &["crm:view_all", "crm:view_own"])?;
    let mut conn = db.conn().await?;
    let fields = search::field_schema(&mut conn, &q.object_type)
        .await
        .map_err(ApiError::from)?;
    Ok(resp::json(
        json!({"object_type": q.object_type, "fields": fields}),
    ))
}

granary_core::py_model! {
    /// `class FilterSearchBody(BaseModel)`.
    pub struct FilterSearchBody {
        pub object_type: String,
        pub filters: Vec<Map<String, Value>> = Vec::new(),
        pub sort: Option<Map<String, Value>> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `POST /search/filters`: run a filter set directly.
pub async fn search_filters(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<FilterSearchBody>,
) -> ApiResult<Response> {
    require(&user, &["crm:view_all", "crm:view_own"])?;
    let filters: Vec<Value> = body.filters.iter().cloned().map(Value::Object).collect();
    let sort = body.sort.clone().map(Value::Object);
    let mut conn = db.conn().await?;
    let items = search::run_filters(
        &mut conn,
        &user,
        &body.object_type,
        &filters,
        body.limit,
        body.offset,
        sort.as_ref(),
    )
    .await
    .map_err(ApiError::from)?;
    let total = search::count_filters(&mut conn, &user, &body.object_type, &filters)
        .await
        .map_err(ApiError::from)?;
    Ok(resp::json(json!({
        "object_type": body.object_type, "count": total, "offset": body.offset, "items": items,
    })))
}

granary_core::py_model! {
    /// `class BulkRunBody(BaseModel)`.
    pub struct BulkRunBody {
        pub object_type: String,
        pub record_ids: Vec<String>,
        pub attribute_key: Option<String> = None,
        /// attribute | enrich (companies) | research (companies, contacts)
        pub action: String = "attribute",
    }
}

fn bulk_400(e: BulkFail) -> ApiError {
    match e {
        BulkFail::Bulk(b) => ApiError::http(400, b.0),
        BulkFail::Db(d) => d.into(),
    }
}

/// `_queue_bulk(body, user)`: true once a worker has the job (an unanswered ping means "run it here").
async fn queue_bulk(body: &BulkRunBody, user: &AuthUser) -> bool {
    if !granary_services::tasks::ping(Duration::from_millis(1500)).await {
        return false;
    }
    granary_services::tasks::delay(
        "bulk_ai_field_task",
        vec![
            json!(body.object_type),
            json!(body.record_ids),
            json!(user.id.to_string()),
            json!(body.attribute_key),
            json!(body.action),
        ],
        Map::new(),
    )
    .await
    .is_ok()
}

/// `POST /records/bulk/ai-fields/run`: queue an AI run across a selection (inline when no worker answers).
pub async fn bulk_run_ai_field(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<BulkRunBody>,
) -> ApiResult<Response> {
    check_object_type(&body.object_type)?;
    require(
        &user,
        &["crm:edit_contact", "crm:edit_lead", "crm:edit_deal"],
    )?;
    let mut conn = db.conn().await?;
    // The cheap checks first: an oversized selection is refused whatever it was going to run.
    let bad = |m: String| ApiError::http(400, m);
    if body.record_ids.is_empty() {
        return Err(bad("Select at least one record.".into()));
    }
    if body.record_ids.len() > bulk_ai::MAX_BULK {
        return Err(bad(format!(
            "Too many records in one run (max {}).",
            bulk_ai::MAX_BULK
        )));
    }
    if body.action == "attribute" {
        if body.attribute_key.as_deref().is_none_or(str::is_empty) {
            return Err(bad("attribute_key is required.".into()));
        }
        bulk_ai::load_attribute(&mut conn, &body.object_type, body.attribute_key.as_deref())
            .await
            .map_err(bulk_400)?;
    } else if body.action == "research" {
        if !matches!(body.object_type.as_str(), "organization" | "contact") {
            return Err(bad("Deep research covers companies and contacts.".into()));
        }
    } else if body.action != "enrich" {
        return Err(bad(
            "action must be 'attribute', 'enrich' or 'research'.".into()
        ));
    } else if body.object_type != "organization" {
        return Err(bad("Web research only applies to companies.".into()));
    }

    if queue_bulk(&body, &user).await {
        let n = body.record_ids.len();
        return Ok(resp::json(json!({
            "queued": true, "count": n,
            "message": format!(
                "Running over {n} record(s); you will get a notification when it finishes."
            ),
        })));
    }
    let summary = bulk_ai::run_bulk(
        &mut conn,
        &user,
        &body.object_type,
        &body.record_ids,
        body.attribute_key.as_deref(),
        &body.action,
    )
    .await
    .map_err(|e| match e {
        BulkFail::Bulk(b) => ApiError::internal(anyhow::anyhow!(b.0)),
        BulkFail::Db(d) => d.into(),
    })?;
    drop(conn);
    db.commit().await?;
    let mut out = Map::new();
    out.insert("queued".into(), Value::Bool(false));
    if let Value::Object(m) = summary {
        for (k, v) in m {
            out.insert(k, v);
        }
    }
    Ok(resp::json(Value::Object(out)))
}

// ─── Phase 5: duplicate detection + merge ────────────────────────────────────

granary_core::py_model! {
    pub struct ScanQ {
        pub object_type: String,
        pub threshold: f64 = 70.0,
    }
}

/// `POST /duplicates/scan`.
pub async fn duplicates_scan(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ScanQ>,
) -> ApiResult<Response> {
    require(&user, &["crm:manage_attributes", "crm:edit_contact"])?;
    let mut conn = db.conn().await?;
    let out = dedupe::scan(&mut conn, &q.object_type, q.threshold, 500)
        .await
        .map_err(ApiError::from)?;
    Ok(resp::json(out))
}

granary_core::py_model! {
    pub struct DuplicatesQ {
        pub object_type: Option<String> = None,
        pub status: String = "open",
        pub limit: i64 = 50,
    }
}

/// `GET /duplicates`.
pub async fn duplicates_list(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<DuplicatesQ>,
) -> ApiResult<Response> {
    require(&user, &["crm:view_all", "crm:view_own"])?;
    let mut conn = db.conn().await?;
    let items = dedupe::list_candidates(&mut conn, q.object_type.as_deref(), &q.status, q.limit)
        .await
        .map_err(ApiError::from)?;
    Ok(resp::json(json!({"items": items})))
}

/// `_merge_access(db, user, object_type, keep_id, merge_id)`: both sides must be live records the user
/// may see.
async fn merge_access(
    conn: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    keep_id: Uuid,
    merge_id: Uuid,
) -> ApiResult<()> {
    if !dedupe::OBJECT_TYPES.contains(&object_type) || keep_id == merge_id {
        return Ok(()); // merge_preview reports it
    }
    let table = table_of(object_type).expect("dedupe type");
    for record_id in [keep_id, merge_id] {
        let rec: Option<(bool, Option<Uuid>)> = sqlx::query_as(&format!(
            "SELECT is_deleted, assigned_to_id FROM {table} WHERE id = $1 LIMIT 1"
        ))
        .bind(record_id)
        .fetch_optional(&mut *conn)
        .await?;
        let Some((deleted, owner)) = rec else {
            return Err(ApiError::not_found(
                &title(object_type),
                Some(&record_id.to_string()),
            ));
        };
        if deleted {
            return Err(ApiError::http(
                400,
                "That record was already merged or deleted.",
            ));
        }
        granary_services::crm::access::assert_can_view(user, owner)?;
    }
    Ok(())
}

granary_core::py_model! {
    pub struct PreviewQ {
        pub object_type: String,
        pub keep_id: String,
        pub merge_id: String,
    }
}

/// `GET /duplicates/preview`: exactly what the merge would do.
pub async fn duplicates_preview(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<PreviewQ>,
) -> ApiResult<Response> {
    require(&user, &["crm:view_all", "crm:view_own"])?;
    let keep = parse_uuid(&q.keep_id, "keep_id")?;
    let merge = parse_uuid(&q.merge_id, "merge_id")?;
    let mut conn = db.conn().await?;
    merge_access(&mut conn, &user, &q.object_type, keep, merge).await?;
    let out = dedupe::merge_preview(&mut conn, &q.object_type, keep, merge)
        .await
        .map_err(ApiError::from)?;
    Ok(resp::json(out))
}

granary_core::py_model! {
    /// `class MergeBody(BaseModel)`.
    pub struct MergeBody {
        pub object_type: String,
        pub keep_id: String,
        pub merge_id: String,
        pub candidate_id: Option<String> = None,
    }
}

/// `POST /duplicates/merge`: irreversible; only reached from an explicit confirmation.
pub async fn duplicates_merge(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<MergeBody>,
) -> ApiResult<Response> {
    require(&user, &["crm:delete_contact", "crm:edit_contact"])?;
    let keep = parse_uuid(&body.keep_id, "keep_id")?;
    let merge = parse_uuid(&body.merge_id, "merge_id")?;
    let mut conn = db.conn().await?;
    merge_access(&mut conn, &user, &body.object_type, keep, merge).await?;
    let candidate = match body.candidate_id.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => Some(parse_uuid(c, "candidate_id")?),
        None => None,
    };
    let out = dedupe::merge_records(&mut conn, &user, &body.object_type, keep, merge, candidate)
        .await
        .map_err(ApiError::from)?;
    Ok(resp::json(out))
}

/// `POST /duplicates/{candidate_id}/dismiss`.
pub async fn duplicates_dismiss(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(candidate_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, &["crm:edit_contact"])?;
    let id = parse_uuid(&candidate_id, "candidate_id")?;
    let mut conn = db.conn().await?;
    let out = dedupe::dismiss(&mut conn, &user, id)
        .await
        .map_err(ApiError::from)?;
    Ok(resp::json(out))
}
