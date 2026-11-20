//! Port of backend/app/services/nary/actions.py.
//!
//! The action registry: everything Nary (or an MCP client) can do to the world.
//!
//! Each action declares its payload schema, the permission the acting user needs, whether it is
//! customer-facing, a server-side `apply()` and, for the approval card, a `describe()` that shows what
//! would change. The executor (executor.rs) decides, from the autonomy level, whether an action is
//! stored as a shadow draft, waits as a card, or is applied.
//!
//! Customer-facing actions are applied only when CUSTOMER_SENDING_ENABLED is on and shadow mode allows
//! it (executor.propose). The channel actions (acknowledgement, WhatsApp and email sends, re-engagement,
//! identity links) are registered in channel_actions.rs.
//!
//! Rust shape of the registry:
//! - Payload schemas are [`Schema`] values (a Pydantic model: fields with a [`Ty`] and a default, and
//!   `extra="forbid"` for `_Strict` models). [`Schema::validate`] returns `model_dump(mode="json")` or the
//!   exact `str(ValidationError)` text Pydantic 2.10 prints (type codes, truncated input reprs, docs links).
//! - Python registers actions on import. Here [`REGISTRY`] is built on first use from [`builtin_actions`] and
//!   each action module's `pub fn actions() -> Vec<Action>` (channel_actions, playbook_actions,
//!   proposal_actions, in Python's import order), see [`module_actions`]. [`register`] adds or replaces one at
//!   run time (tests).
//! - `apply` / `describe` are async closures taking `(db, actor, payload)`; `actor` is `Option<&AuthUser>`
//!   (Python passes None at times, e.g. `schedule_message`'s describe).
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, RwLock};

use granary_compat::pyjson::{self, PyValue};
use granary_compat::text::utils::py_uuid;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::validation::types::{
    literal_message, MSG_DICT_TYPE, MSG_INT_TYPE, MSG_LIST_TYPE,
};
use granary_core::validation::{Cx, DateTimeIn, Loc, PyObj, Validate, MSG_EXTRA, MSG_MISSING};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::governance::{is_http_exception, py_str, PyException};

/// Safety switch in code, not a setting. Phase 1 kept it off; Phase 2 turns it on together with the
/// channels. Shadow mode (nary.shadow_mode, on) still keeps every customer action a draft, except the
/// ones in nary.shadow_exempt_actions (the acknowledgement), and nary.ack.enabled gates that one.
pub const CUSTOMER_SENDING_ENABLED: bool = true;

/// `CustomerSendingDisabled(action)`: an HTTPException 409.
#[allow(non_snake_case)]
pub fn CustomerSendingDisabled(action: &str) -> ApiError {
    ApiError::http(
        409,
        format!("'{action}' would reach a customer; sending is off in this phase."),
    )
}

// ─── Schemas (Pydantic models) ────────────────────────────────────────────────

/// A field type of an action payload model.
#[derive(Clone, Debug)]
pub enum Ty {
    /// `str`
    Str,
    /// `int`
    Int,
    /// `StrictInt`: an int only (not a bool, a float or a numeric string)
    StrictInt,
    /// `bool`
    Bool,
    /// `dict` (values of any type)
    Dict,
    /// `datetime` (dumped in JSON mode: ISO 8601, "Z" for UTC)
    DateTime,
    /// `Any`
    Any,
    /// `Literal["a", "b"]`
    Literal(&'static [&'static str]),
    /// `list[T]`
    List(Box<Ty>),
    /// `T | None`
    Opt(Box<Ty>),
}

impl Ty {
    pub fn list(t: Ty) -> Ty {
        Ty::List(Box::new(t))
    }
    pub fn opt(t: Ty) -> Ty {
        Ty::Opt(Box::new(t))
    }
}

/// One model field. `default: None` means required.
#[derive(Clone, Debug)]
pub struct Field {
    pub name: &'static str,
    pub ty: Ty,
    pub default: Option<Value>,
    /// R&D 26 Oct: a field added after the Python freeze: when it is not given it stays out of the payload
    /// (no `null`), so every payload without it is what it always was.
    pub late: bool,
}

impl Field {
    /// `name: T` (required).
    pub fn req(name: &'static str, ty: Ty) -> Field {
        Field {
            name,
            ty,
            default: None,
            late: false,
        }
    }
    /// `name: T | None = None`.
    pub fn opt(name: &'static str, ty: Ty) -> Field {
        Field {
            name,
            ty: Ty::opt(ty),
            default: Some(Value::Null),
            late: false,
        }
    }
    /// [`Field::opt`] added after the Python freeze: absent stays absent.
    pub fn late(name: &'static str, ty: Ty) -> Field {
        Field {
            late: true,
            ..Field::opt(name, ty)
        }
    }
    /// `name: T = default`.
    pub fn default(name: &'static str, ty: Ty, default: Value) -> Field {
        Field {
            name,
            ty,
            default: Some(default),
            late: false,
        }
    }
}

/// `model_config` extra handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    /// Pydantic's default: unknown keys are dropped.
    Ignore,
    /// `extra="forbid"` (`_Strict`).
    Forbid,
    /// `extra="allow"`: unknown keys are kept, after the fields.
    Allow,
}

/// A Pydantic model used as an action's payload schema.
#[derive(Clone, Debug)]
pub struct Schema {
    /// The class name (shown in the error text).
    pub model: &'static str,
    pub fields: Vec<Field>,
    pub extra: Extra,
}

/// One Pydantic line error with what `str(ValidationError)` prints.
#[derive(Clone, Debug)]
struct LineErr {
    loc: Vec<Loc>,
    msg: String,
    input: Value,
}

impl Schema {
    /// A `_Strict` model (`extra="forbid"`).
    pub fn strict(model: &'static str, fields: Vec<Field>) -> Schema {
        Schema {
            model,
            fields,
            extra: Extra::Forbid,
        }
    }

    /// `Schema(**(payload or {})).model_dump(mode="json")`, or the `str()` of the exception it raises.
    pub fn validate(&self, payload: &Value) -> Result<Value, String> {
        let empty = Map::new();
        let dict = match payload {
            Value::Object(m) => m,
            v if !py_truthy(v) => &empty,
            other => {
                return Err(format!(
                    "app.services.nary.actions.{}() argument after ** must be a mapping, not {}",
                    self.model,
                    py_type_name(other)
                ))
            }
        };
        let mut errors: Vec<LineErr> = vec![];
        let mut out = Map::new();
        for f in &self.fields {
            match dict.get(f.name) {
                Some(v) => {
                    let mut path = vec![Loc::Key(f.name.to_string())];
                    if let Some(x) = validate_ty(&f.ty, v, &mut path, &mut errors) {
                        out.insert(f.name.to_string(), x);
                    }
                }
                None if f.late => {}
                None => match &f.default {
                    Some(d) => {
                        out.insert(f.name.to_string(), d.clone());
                    }
                    None => errors.push(LineErr {
                        loc: vec![Loc::Key(f.name.to_string())],
                        msg: MSG_MISSING.to_string(),
                        input: Value::Object(dict.clone()),
                    }),
                },
            }
        }
        for (k, v) in dict {
            if self.fields.iter().any(|f| f.name == k) {
                continue;
            }
            match self.extra {
                Extra::Forbid => errors.push(LineErr {
                    loc: vec![Loc::Key(k.clone())],
                    msg: MSG_EXTRA.to_string(),
                    input: v.clone(),
                }),
                Extra::Allow => {
                    out.insert(k.clone(), v.clone());
                }
                Extra::Ignore => {}
            }
        }
        if errors.is_empty() {
            Ok(Value::Object(out))
        } else {
            Err(format_errors(self.model, &errors))
        }
    }
}

fn validate_ty(
    ty: &Ty,
    v: &Value,
    path: &mut Vec<Loc>,
    errors: &mut Vec<LineErr>,
) -> Option<Value> {
    let push = |errors: &mut Vec<LineErr>, path: &Vec<Loc>, msg: String| {
        errors.push(LineErr {
            loc: path.clone(),
            msg,
            input: v.clone(),
        })
    };
    // Scalar types go through the shared Pydantic-compatible validators (same coercions and messages).
    fn core<T: Validate>(v: &Value) -> Result<T, String> {
        let mut cx = Cx::new();
        match T::validate(&PyObj::from_json(v), &mut cx) {
            Some(t) if cx.errors.is_empty() => Ok(t),
            _ => Err(cx
                .errors
                .into_iter()
                .next()
                .map(|e| e.msg)
                .unwrap_or_default()),
        }
    }
    match ty {
        Ty::Any => Some(v.clone()),
        Ty::Opt(inner) => {
            if v.is_null() {
                Some(Value::Null)
            } else {
                validate_ty(inner, v, path, errors)
            }
        }
        Ty::Str => match core::<String>(v) {
            Ok(s) => Some(Value::String(s)),
            Err(m) => {
                push(errors, path, m);
                None
            }
        },
        Ty::Int => match core::<i64>(v) {
            Ok(i) => Some(json!(i)),
            Err(m) => {
                push(errors, path, m);
                None
            }
        },
        // Pydantic's strict int: no coercion, a bool (an int subclass) is refused too.
        Ty::StrictInt => match v {
            Value::Number(n) if n.is_i64() || n.is_u64() => Some(v.clone()),
            _ => {
                push(errors, path, MSG_INT_TYPE.to_string());
                None
            }
        },
        Ty::Bool => match core::<bool>(v) {
            Ok(b) => Some(Value::Bool(b)),
            Err(m) => {
                push(errors, path, m);
                None
            }
        },
        // An int beyond i64 (the shared validator's storage): speedate's "too large" message.
        Ty::DateTime if matches!(v, Value::Number(n) if py_type_name(v) == "int" && n.as_i64().is_none() && n.as_u64().is_none_or(|u| u > i64::MAX as u64)) =>
        {
            push(
                errors,
                path,
                "Input should be a valid datetime, dates after 9999 are not supported as unix timestamps".into(),
            );
            None
        }
        Ty::DateTime => match core::<DateTimeIn>(v) {
            Ok(d) => Some(Value::String(datetime_json(&d))),
            Err(m) => {
                push(errors, path, m);
                None
            }
        },
        Ty::Literal(choices) => match v {
            Value::String(s) if choices.contains(&s.as_str()) => Some(v.clone()),
            _ => {
                push(errors, path, literal_message(choices));
                None
            }
        },
        Ty::Dict => match v {
            Value::Object(_) => Some(v.clone()),
            _ => {
                push(errors, path, MSG_DICT_TYPE.to_string());
                None
            }
        },
        Ty::List(inner) => match v {
            Value::Array(items) => {
                let mut out = vec![];
                let mut ok = true;
                for (i, it) in items.iter().enumerate() {
                    path.push(Loc::Index(i));
                    match validate_ty(inner, it, path, errors) {
                        Some(x) => out.push(x),
                        None => ok = false,
                    }
                    path.pop();
                }
                ok.then_some(Value::Array(out))
            }
            _ => {
                push(errors, path, MSG_LIST_TYPE.to_string());
                None
            }
        },
    }
}

/// Pydantic's JSON-mode datetime: `isoformat()` with "Z" for a zero UTC offset.
pub fn datetime_json(d: &DateTimeIn) -> String {
    let s = d.isoformat();
    if d.offset == Some(0) {
        if let Some(stripped) = s.strip_suffix("+00:00") {
            return format!("{stripped}Z");
        }
    }
    s
}

/// The Pydantic error type code of a message (what `str(ValidationError)` prints after `type=`).
fn error_type(msg: &str) -> &'static str {
    use granary_core::validation::types as t;
    match msg {
        m if m == MSG_MISSING => "missing",
        m if m == MSG_EXTRA => "extra_forbidden",
        m if m == t::MSG_STRING_TYPE => "string_type",
        m if m == t::MSG_BOOL_PARSING => "bool_parsing",
        m if m == t::MSG_BOOL_TYPE => "bool_type",
        m if m == t::MSG_INT_PARSING => "int_parsing",
        m if m == t::MSG_INT_FROM_FLOAT => "int_from_float",
        m if m == t::MSG_INT_PARSING_SIZE => "int_parsing_size",
        m if m == t::MSG_INT_TYPE => "int_type",
        m if m == t::MSG_FINITE => "finite_number",
        m if m == t::MSG_FLOAT_TYPE => "float_type",
        m if m == t::MSG_FLOAT_PARSING => "float_parsing",
        m if m == MSG_DICT_TYPE => "dict_type",
        m if m == MSG_LIST_TYPE => "list_type",
        m if m == t::MSG_DATETIME_TYPE => "datetime_type",
        m if m.starts_with("Input should be a valid datetime or date, ") => {
            "datetime_from_date_parsing"
        }
        m if m.starts_with("Input should be a valid datetime, ") => "datetime_parsing",
        m if m.starts_with("Input should be '") => "literal_error",
        m if m.starts_with("String should have at most") => "string_too_long",
        m if m.starts_with("String should have at least") => "string_too_short",
        _ => "value_error",
    }
}

/// `type(v).__name__` of a JSON value.
pub fn py_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() || !n.to_string().contains(['.', 'e', 'E']) {
                "int"
            } else {
                "float"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// pydantic-core `truncate_safe_repr`: reprs over 50 bytes keep the first 25 and last 24 bytes.
fn truncated_repr(v: &Value) -> String {
    let s = pyjson::repr(&PyValue::from(v)).unwrap_or_else(|_| "?".into());
    const MAX: usize = 50;
    if s.len() <= MAX {
        return s;
    }
    let mid = MAX.div_ceil(2);
    let mut end = mid;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut start = s.len() - (MAX - mid - 1);
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("{}...{}", &s[..end], &s[start..])
}

fn format_errors(model: &str, errors: &[LineErr]) -> String {
    let n = errors.len();
    let mut out = format!(
        "{n} validation error{} for {model}",
        if n == 1 { "" } else { "s" }
    );
    for e in errors {
        let loc: Vec<String> = e.loc.iter().map(|l| l.to_string()).collect();
        let ty = error_type(&e.msg);
        out.push_str(&format!(
            "\n{}\n  {} [type={ty}, input_value={}, input_type={}]\n    For further information visit https://errors.pydantic.dev/2.10/v/{ty}",
            loc.join("."),
            e.msg,
            truncated_repr(&e.input),
            py_type_name(&e.input)
        ));
    }
    out
}

/// Python truthiness of a JSON value.
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

// ─── Action ───────────────────────────────────────────────────────────────────

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = ApiResult<T>> + Send + 'a>>;

/// `apply(db, actor, payload) -> dict` / `describe(db, actor, payload) -> dict`.
pub type ApplyFn = Arc<
    dyn for<'a> Fn(&'a mut PgConnection, Option<&'a AuthUser>, &'a Value) -> BoxFut<'a, Value>
        + Send
        + Sync,
>;

/// `entity(payload) -> (entity_type, entity_id)`; an Err is what the Python lambda raises (a bad UUID).
pub type EntityFn = Arc<dyn Fn(&Value) -> ApiResult<(Option<String>, Option<Uuid>)> + Send + Sync>;

/// Wraps an async fn / closure as an [`ApplyFn`].
pub fn apply_fn<F>(f: F) -> ApplyFn
where
    F: for<'a> Fn(&'a mut PgConnection, Option<&'a AuthUser>, &'a Value) -> BoxFut<'a, Value>
        + Send
        + Sync
        + 'static,
{
    Arc::new(f)
}

pub fn entity_fn<F>(f: F) -> EntityFn
where
    F: Fn(&Value) -> ApiResult<(Option<String>, Option<Uuid>)> + Send + Sync + 'static,
{
    Arc::new(f)
}

#[derive(Clone)]
pub struct Action {
    pub name: String,
    pub label: String,
    pub schema: Schema,
    pub apply: ApplyFn,
    /// A code from app/core/permissions.py, or None.
    pub permission: Option<String>,
    pub customer_facing: bool,
    pub describe: Option<ApplyFn>,
    pub entity: EntityFn,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Action")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("permission", &self.permission)
            .field("customer_facing", &self.customer_facing)
            .finish()
    }
}

impl Action {
    /// `Action(name, label, schema, apply)` with the dataclass defaults.
    pub fn new(name: &str, label: &str, schema: Schema, apply: ApplyFn) -> Action {
        Action {
            name: name.to_string(),
            label: label.to_string(),
            schema,
            apply,
            permission: None,
            customer_facing: false,
            describe: None,
            entity: Arc::new(|_| Ok((None, None))),
        }
    }
    pub fn permission(mut self, p: &str) -> Action {
        self.permission = Some(p.to_string());
        self
    }
    pub fn customer_facing(mut self, on: bool) -> Action {
        self.customer_facing = on;
        self
    }
    pub fn describe(mut self, d: ApplyFn) -> Action {
        self.describe = Some(d);
        self
    }
    pub fn entity(mut self, e: EntityFn) -> Action {
        self.entity = e;
        self
    }

    /// `validate(payload)`: the clean payload, or HTTPException 422 "{name}: {pydantic error}".
    pub fn validate(&self, payload: &Value) -> ApiResult<Value> {
        self.schema
            .validate(payload)
            .map_err(|e| ApiError::http(422, format!("{}: {e}", self.name)))
    }

    /// `check_permission(user)`: superadmins pass; else 403 "Permission required: {code}".
    pub fn check_permission(&self, user: Option<&AuthUser>) -> ApiResult<()> {
        let Some(perm) = &self.permission else {
            return Ok(());
        };
        let Some(user) = user else {
            // `_collect_permissions(None)` fails in Python.
            return Err(ApiError::internal(PyException::new(
                "AttributeError",
                "'NoneType' object has no attribute 'roles'",
            )));
        };
        if user.is_superadmin {
            return Ok(());
        }
        if !user.perms.contains(perm) {
            return Err(ApiError::http(403, format!("Permission required: {perm}")));
        }
        Ok(())
    }
}

// ─── Registry ─────────────────────────────────────────────────────────────────

/// `REGISTRY`: the actions in registration order (a later registration of a name replaces it in place).
pub static REGISTRY: LazyLock<RwLock<Vec<Arc<Action>>>> = LazyLock::new(|| {
    let mut list: Vec<Arc<Action>> = vec![];
    for a in builtin_actions().into_iter().chain(module_actions()) {
        insert(&mut list, Arc::new(a));
    }
    RwLock::new(list)
});

fn insert(list: &mut Vec<Arc<Action>>, a: Arc<Action>) {
    match list.iter_mut().find(|x| x.name == a.name) {
        Some(slot) => *slot = a,
        None => list.push(a),
    }
}

/// The actions the other modules register on import (Python: the imports at the end of actions.py:
/// channel_actions, playbook_actions, proposal_actions). Each module exposes `pub fn actions() -> Vec<Action>`.
fn module_actions() -> Vec<Action> {
    let mut out = super::channel_actions::actions();
    out.extend(super::playbook_actions::actions());
    out.extend(super::proposal_actions::actions());
    // batch 2 T3: cancel a timed customer message (A5)
    out.extend(super::scheduled_sends::actions());
    // batch 2 T2: merge organizations, delete a contact (A2, A9)
    out.extend(super::hygiene::actions());
    // batch 4 D3: change a draft quotation from the chat
    out.extend(super::quotation_updates::actions());
    // doclib A3: send a library document to a customer
    out.extend(super::document_send::actions());
    // visuals V3: proposal figures and "Ask Nary to change" from the chat
    out.extend(super::proposal_visual_actions::actions());
    // fix-longapply: revise a proposal, reject a draft
    out.extend(super::proposal_lifecycle::actions());
    // 22 Oct 2026: link a quotation to a proposal, or take one off
    out.extend(super::proposal_quotations::actions());
    // 23 Oct 2026: a proposal written from the given sections (the chat, the MCP client)
    out.extend(super::proposal_sections::actions());
    // 23 Oct 2026: catalog changes by the chat and the MCP client (the Owner only)
    out.extend(super::catalog_changes::actions());
    // R&D 26 Oct 2026: merge a deal entered twice (the newer one into the older)
    out.extend(crate::crm::deal_duplicates::actions());
    // R&D 26 Oct 2026 (second batch): delete a typed-in activity, link an email thread
    out.extend(crate::crm::activity_void::actions());
    // 1 Nov 2026: reprice a draft quotation to today, withdraw one from approval, cancel one
    out.extend(super::quotation_lifecycle::actions());
    // 3 Nov 2026 (Stream B1): a lead from a WhatsApp chat, a held chat decided, by hand while Nary is paused
    out.extend(super::wa_leads::actions());
    // 3 Nov 2026 (Stream B2): a deal moved to Won by hand (propose_deal_won)
    out.extend(super::closeout::actions());
    // 5 Nov 2026 (Stream A): a file added to a catalog product's Product files (the Owner only)
    out.extend(super::product_files::actions());
    // 5 Nov 2026 (Stream C): link a sent or approved quotation to a deal without a revision
    out.extend(super::quotation_links::actions());
    out
}

/// `register(action)`: adds or replaces it.
pub fn register(action: Action) -> Arc<Action> {
    let a = Arc::new(action);
    insert(&mut REGISTRY.write().unwrap(), a.clone());
    a
}

/// Removes an action (tests that registered their own).
pub fn unregister(name: &str) -> Option<Arc<Action>> {
    let mut list = REGISTRY.write().unwrap();
    let i = list.iter().position(|a| a.name == name)?;
    Some(list.remove(i))
}

/// `get(name)`: the action, or HTTPException 400 "Unknown action '{name}'".
pub fn get(name: &str) -> ApiResult<Arc<Action>> {
    REGISTRY
        .read()
        .unwrap()
        .iter()
        .find(|a| a.name == name)
        .cloned()
        .ok_or_else(|| ApiError::http(400, format!("Unknown action '{name}'")))
}

/// `list(REGISTRY.values())`.
pub fn all() -> Vec<Arc<Action>> {
    REGISTRY.read().unwrap().clone()
}

// ─── Helpers for apply/describe ───────────────────────────────────────────────

/// `uuid.UUID(value)`: ValueError on a malformed string.
pub fn to_uuid(v: &Value) -> ApiResult<Uuid> {
    match v {
        Value::String(s) => py_uuid(s)
            .ok_or_else(|| PyException::value_error("badly formed hexadecimal UUID string")),
        other => Err(ApiError::internal(PyException::new(
            "TypeError",
            format!(
                "one of the hex, bytes, bytes_le, fields, or int arguments must be given (got {})",
                py_type_name(other)
            ),
        ))),
    }
}

/// `uuid.UUID(p[key]) if p.get(key) else None`.
pub fn opt_uuid(p: &Value, key: &str) -> ApiResult<Option<Uuid>> {
    match p.get(key) {
        Some(v) if py_truthy(v) => to_uuid(v).map(Some),
        _ => Ok(None),
    }
}

/// `p.get(key)` as an optional string (`None` for null or missing).
pub fn opt_str<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str)
}

/// `actor.id` where Python assumes an actor.
pub fn need_actor(actor: Option<&AuthUser>) -> ApiResult<&AuthUser> {
    actor.ok_or_else(|| {
        ApiError::internal(PyException::new(
            "AttributeError",
            "'NoneType' object has no attribute 'id'",
        ))
    })
}

fn s(p: &Value, key: &str) -> String {
    p.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ─── CRM record change (Nary) and MCP writes ──────────────────────────────────

fn record_change_schema() -> Schema {
    Schema::strict(
        "RecordChange",
        vec![
            Field::req(
                "object_type",
                Ty::Literal(&["organization", "contact", "lead", "deal", "activity"]),
            ),
            Field::req("action", Ty::Literal(&["create", "update"])),
            Field::opt("record_id", Ty::Str),
            Field::req("fields", Ty::Dict),
        ],
    )
}

fn _record_entity(p: &Value) -> ApiResult<(Option<String>, Option<Uuid>)> {
    Ok((Some(s(p, "object_type")), opt_uuid(p, "record_id")?))
}

async fn _describe_record(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    use crate::crm::writes::{build_diff, WriteError};
    let record_id = opt_uuid(p, "record_id")?;
    match build_diff(
        db,
        &s(p, "object_type"),
        &s(p, "action"),
        &p["fields"],
        record_id,
        false,
    )
    .await
    {
        Ok(mut v) => {
            // 21 Nov 2026: the card formats money fields (value, amount...) in the record's currency
            if let Some(c) =
                record_currency(db, &s(p, "object_type"), record_id, &p["fields"]).await?
            {
                if let Value::Object(m) = &mut v {
                    m.entry("currency").or_insert(json!(c));
                }
            }
            Ok(v)
        }
        Err(WriteError::Invalid(m)) => Ok(json!({"error": m})),
        Err(WriteError::Api(e)) => Err(e),
    }
}

/// The currency of a CRM record's money fields: the one being set, else the record's (deals and leads carry
/// `currency`, organizations `revenue_currency`); None for other objects.
async fn record_currency(
    db: &mut PgConnection,
    object_type: &str,
    record_id: Option<Uuid>,
    fields: &Value,
) -> ApiResult<Option<String>> {
    let column = match object_type {
        "deal" | "lead" => "currency",
        "organization" => "revenue_currency",
        _ => return Ok(None),
    };
    if let Some(c) = fields
        .get(column)
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
    {
        return Ok(Some(c.to_string()));
    }
    let Some(id) = record_id else {
        return Ok(None);
    };
    let table = match object_type {
        "deal" => "deals",
        "lead" => "leads",
        _ => "organizations",
    };
    let c: Option<Option<String>> =
        sqlx::query_scalar(&format!("SELECT {column} FROM {table} WHERE id = $1"))
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    Ok(c.flatten().filter(|c| !c.is_empty()))
}

fn _apply_record<'a>(
    db: &'a mut PgConnection,
    actor: Option<&'a AuthUser>,
    p: &'a Value,
    origin: &'static str,
) -> BoxFut<'a, Value> {
    // A boxed future built here (not an `async fn`), so the `origin` argument can be bound per registration.
    Box::pin(async move {
        // apply_change checks the actor's own permission for the object and owner scoping.
        use crate::crm::writes::{apply_change, WriteError};
        let actor = need_actor(actor)?;
        let record_id = opt_uuid(p, "record_id")?;
        let object_type = s(p, "object_type");
        let action = s(p, "action");
        let fields = p["fields"].clone();
        match apply_change(db, actor, &object_type, &action, &fields, record_id, origin).await {
            Ok(v) => Ok(v),
            Err(WriteError::Invalid(m)) => Err(ApiError::http(400, m)),
            Err(WriteError::Api(e)) => Err(e),
        }
    })
}

// ─── Promote a lead ───────────────────────────────────────────────────────────

async fn _apply_promote(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    let lead_id = py_uuid(&s(p, "lead_id")).ok_or_else(|| {
        // asyncpg refuses a malformed UUID string bound to a uuid column.
        ApiError::internal(PyException::new(
            "DataError",
            "invalid input for query argument $1",
        ))
    })?;
    let out = crate::crm::promote::promote_lead(db, lead_id, actor, "nary").await?;
    super::commit(db).await?;
    Ok(out)
}

async fn load_lead(
    db: &mut PgConnection,
    id: Uuid,
) -> ApiResult<Option<granary_db::rows::crm::Lead>> {
    Ok(sqlx::query_as("SELECT * FROM leads WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

/// f-string of an optional text column (`None` prints "None").
fn fs(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "None".into())
}

async fn _describe_promote(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let lead = load_lead(db, to_uuid(&p["lead_id"])?).await?;
    Ok(match lead {
        Some(l) => json!({"label": l.title,
                          "after": {"deal": format!("New deal at Qualifying from '{}'", fs(&l.title))}}),
        None => json!({"label": "unknown lead",
                       "after": {"deal": "New deal at Qualifying from '?'"}}),
    })
}

// ─── Merge two leads of the same person (services/crm/lead_merge.py) ──────────

async fn _apply_merge(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let keep = load_lead(db, to_uuid(&p["keep_id"])?).await?;
    let drop = load_lead(db, to_uuid(&p["drop_id"])?).await?;
    let (Some(mut keep), Some(mut drop)) = (keep, drop) else {
        return Err(ApiError::http(404, "One of the two leads is gone"));
    };
    if keep.is_deleted || drop.is_deleted {
        return Err(ApiError::http(404, "One of the two leads is gone"));
    }
    if let Some(st) = drop
        .status
        .as_deref()
        .filter(|s| matches!(*s, "disqualified" | "promoted"))
    {
        return Err(ApiError::http(
            409,
            format!("'{}' is already {st}", fs(&drop.title)),
        ));
    }
    let moved = crate::crm::lead_merge::merge(db, &mut keep, &mut drop).await?;
    super::commit(db).await?;
    let mut out = Map::new();
    out.insert(
        "label".into(),
        json!(format!("Merged into {}", fs(&keep.title))),
    );
    if let Value::Object(m) = moved {
        out.extend(m);
    }
    Ok(Value::Object(out))
}

async fn _describe_merge(
    db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let keep = load_lead(db, to_uuid(&p["keep_id"])?).await?;
    let drop = load_lead(db, to_uuid(&p["drop_id"])?).await?;
    let k = keep
        .as_ref()
        .map(|l| format!("{} ({}, {})", fs(&l.title), fs(&l.status), l.source))
        .unwrap_or_else(|| "?".into());
    let d = drop
        .as_ref()
        .map(|l| format!("{} ({}, {})", fs(&l.title), fs(&l.status), l.source))
        .unwrap_or_else(|| "?".into());
    Ok(json!({
        "label": format!("Merge '{}' into '{}'",
                         drop.as_ref().map(|l| fs(&l.title)).unwrap_or_else(|| "?".into()),
                         keep.as_ref().map(|l| fs(&l.title)).unwrap_or_else(|| "?".into())),
        "before": {"keep": k, "duplicate": d},
        "after": {"keep": k, "duplicate": "disqualified (duplicate); its activities, chats, emails and \
                                           submissions move to the lead kept"},
    }))
}

// ─── Follow-up reminder ───────────────────────────────────────────────────────

fn add_reminder_schema() -> Schema {
    Schema::strict(
        "AddReminder",
        vec![
            Field::req(
                "entity_type",
                Ty::Literal(&[
                    "contact",
                    "lead",
                    "deal",
                    "quotation",
                    "email_thread",
                    "whatsapp_chat",
                ]),
            ),
            Field::opt("entity_id", Ty::Str),
            Field::opt("chat_id", Ty::Str),
            Field::req("title", Ty::Str),
            Field::opt("note", Ty::Str),
            Field::opt("due_at", Ty::DateTime),
            Field::opt("owner_user_id", Ty::Str),
        ],
    )
}

async fn _apply_reminder(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    use crate::crm::followups::{create, NewReminder};
    // `datetime.fromisoformat(p["due_at"])` of the JSON-mode text; a naive one is bound in the process's
    // local time (asyncpg).
    let due = match p.get("due_at") {
        Some(v) if py_truthy(v) => {
            let mut cx = Cx::new();
            let d = DateTimeIn::validate(&PyObj::from_json(v), &mut cx).ok_or_else(|| {
                PyException::value_error(format!("Invalid isoformat string: {}", py_str(v)))
            })?;
            Some(d.to_utc())
        }
        _ => None,
    };
    let owner = match opt_uuid(p, "owner_user_id")? {
        Some(u) => u,
        None => need_actor(actor)?.id,
    };
    let mut new = NewReminder::new(owner, &s(p, "entity_type"), &s(p, "title"));
    new.entity_id = opt_uuid(p, "entity_id")?;
    new.chat_id = opt_str(p, "chat_id").map(str::to_string);
    new.note = opt_str(p, "note").map(str::to_string);
    new.due_at = due;
    new.source = "manual".into();
    new.created_by_id = Some(need_actor(actor)?.id);
    let reminder = create(db, new).await?;
    super::commit(db).await?;
    Ok(json!({"id": reminder.map(|r| r.id.to_string()), "label": p["title"]}))
}

// ─── Replies ──────────────────────────────────────────────────────────────────

/// `ReplyDraft`: also the payload of the playbook reply actions (playbook_actions.py).
pub fn reply_draft_schema() -> Schema {
    Schema::strict(
        "ReplyDraft",
        vec![
            Field::req("channel", Ty::Literal(&["email", "whatsapp"])),
            Field::opt("contact_id", Ty::Str),
            Field::opt("chat_id", Ty::Str),
            Field::opt("thread_id", Ty::Str),
            // email: an address that isn't (yet) a contact's
            Field::opt("to", Ty::Str),
            // email: the subject sent (a reply keeps threading; without one it answers the thread's subject)
            Field::opt("subject", Ty::Str),
            // R&D 26 Oct: email copies (a thread reply goes to everyone on it by default), blind copies (only
            // with the Owner's yes) and the CRM inbox it goes from (id, address or prefix)
            Field::late("cc", Ty::list(Ty::Str)),
            Field::late("bcc", Ty::list(Ty::Str)),
            Field::late("from_inbox", Ty::Str),
            Field::req("text", Ty::Str),
            // the Owner said "send it" to this final text (executor.send_on_instruction)
            Field::default("by_owner", Ty::Bool, Value::Bool(false)),
            // email: sales documents sent as their verified PDFs [{type, id}]
            Field::opt("attach", Ty::list(Ty::Dict)),
        ],
    )
}

async fn _apply_draft(
    _db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    // A draft is the stored row itself: applying it just keeps it for a person to send.
    Ok(json!({"kept": true, "label": super::py_slice(p["text"].as_str().unwrap_or_default(), 80)}))
}

/// `_apply_send`: channel_actions.send_reply (the playbook reply actions use it too).
pub fn apply_send() -> ApplyFn {
    apply_fn(|db, actor, p| Box::pin(super::channel_actions::send_reply(db, actor, p)))
}

/// `_reply_entity(p)`.
pub fn reply_entity() -> EntityFn {
    entity_fn(|p| match opt_uuid(p, "contact_id")? {
        Some(id) => Ok((Some("contact".into()), Some(id))),
        None => Ok((None, None)),
    })
}

// ─── Quotation draft ──────────────────────────────────────────────────────────

fn quotation_draft_schema() -> Schema {
    // the QuotationCreate body; validated by the sales router
    Schema {
        model: "QuotationDraft",
        fields: vec![Field::req("items", Ty::list(Ty::Dict))],
        extra: Extra::Allow,
    }
}

async fn _apply_quotation(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let actor = need_actor(actor)?;
    let mut out = crate::later::nary_actions::create_quotation(db, p, actor).await?;
    let qid = out.get("id").map(py_str).unwrap_or_default();
    if p.get("proposal_id").is_some_and(py_truthy) {
        // drafted for a Solution Proposal: it presents this quotation
        let doc_id = to_uuid(&p["proposal_id"])?;
        let doc: Option<granary_db::rows::documents::SalesDocument> =
            sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
                .bind(doc_id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some(doc) = doc {
            crate::documents::core_::link_quotation(db, &doc, &qid, None).await?;
            super::commit(db).await?;
        }
    }
    // Straight into the approval flow: a quotation within standing authority is approved
    // at once; otherwise the approver gets it. Once approved, Nary proposes sending it (quotes.py).
    match crate::approvals::engine::submit(db, "quotation", &qid, actor, None, None, None).await {
        Ok(_) => super::commit(db).await?,
        Err(e) if is_http_exception(&e) => {
            // no policy, or it can't be submitted yet: stays a draft
            super::rollback(db).await?;
            if let Value::Object(m) = &mut out {
                m.insert(
                    "approval".into(),
                    json!(super::governance::http_detail(&e).unwrap_or_default()),
                );
            }
        }
        Err(e) => return Err(e),
    }
    Ok(out)
}

async fn _describe_quotation(
    _db: &mut PgConnection,
    _actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    let empty = vec![];
    let items = p["items"].as_array().unwrap_or(&empty);
    let lines: Vec<Value> = items
        .iter()
        .map(|i| {
            let qty = i.get("quantity").map(py_str).unwrap_or_else(|| "1".into());
            let what = ["name", "sku", "product_id"]
                .iter()
                .map(|k| i.get(*k).cloned().unwrap_or(Value::Null))
                .find(py_truthy)
                .unwrap_or_else(|| i.get("product_id").cloned().unwrap_or(Value::Null));
            json!(format!("{qty} × {}", py_str(&what)))
        })
        .collect();
    let label = p
        .get("subject")
        .filter(|v| py_truthy(v))
        .cloned()
        .unwrap_or_else(|| json!("Draft quotation"));
    Ok(
        json!({"label": label, "after": {"lines": lines, "currency": p.get("currency").cloned().unwrap_or(Value::Null)}}),
    )
}

/// The actions actions.py itself registers, in its order.
pub fn builtin_actions() -> Vec<Action> {
    let record_entity = entity_fn(_record_entity);
    let describe_record = apply_fn(|db, a, p| Box::pin(_describe_record(db, a, p)));
    vec![
        Action::new(
            "crm.record_change",
            "Create or update a CRM record",
            record_change_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_record(db, a, p, "nary"))),
        )
        .describe(describe_record.clone())
        .entity(record_entity.clone()),
        Action::new(
            "mcp.crm_write",
            "CRM write from a connected agent (MCP)",
            record_change_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_record(db, a, p, "mcp"))),
        )
        .describe(describe_record)
        .entity(record_entity),
        Action::new(
            "lead.promote",
            "Promote a lead to a deal",
            Schema::strict("PromoteLead", vec![Field::req("lead_id", Ty::Str)]),
            apply_fn(|db, a, p| Box::pin(_apply_promote(db, a, p))),
        )
        .permission("crm:create_deal")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_promote(db, a, p))))
        .entity(entity_fn(|p| {
            Ok((Some("lead".into()), Some(to_uuid(&p["lead_id"])?)))
        })),
        Action::new(
            "lead.merge",
            "Merge two leads of the same person",
            Schema::strict(
                "MergeLeads",
                vec![
                    Field::req("keep_id", Ty::Str),
                    Field::req("drop_id", Ty::Str),
                ],
            ),
            apply_fn(|db, a, p| Box::pin(_apply_merge(db, a, p))),
        )
        .permission("crm:edit_lead")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_merge(db, a, p))))
        .entity(entity_fn(|p| {
            Ok((Some("lead".into()), Some(to_uuid(&p["keep_id"])?)))
        })),
        Action::new(
            "reminder.add",
            "Add a follow-up reminder",
            add_reminder_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_reminder(db, a, p))),
        )
        .entity(entity_fn(|p| {
            Ok((Some(s(p, "entity_type")), opt_uuid(p, "entity_id")?))
        })),
        Action::new(
            "reply.draft",
            "Draft a reply",
            reply_draft_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_draft(db, a, p))),
        )
        .entity(reply_entity()),
        Action::new(
            "email.reply.send",
            "Send an email reply",
            reply_draft_schema(),
            apply_send(),
        )
        .permission("crm:log_activity")
        .customer_facing(true)
        .entity(reply_entity()),
        Action::new(
            "whatsapp.reply.send",
            "Send a WhatsApp reply",
            reply_draft_schema(),
            apply_send(),
        )
        .permission("crm:send_whatsapp")
        .customer_facing(true)
        .entity(reply_entity()),
        Action::new(
            "quotation.create_draft",
            "Create a draft quotation",
            quotation_draft_schema(),
            apply_fn(|db, a, p| Box::pin(_apply_quotation(db, a, p))),
        )
        .permission("sales:create_quotation")
        .describe(apply_fn(|db, a, p| Box::pin(_describe_quotation(db, a, p))))
        .entity(entity_fn(|p| match p.get("deal_id") {
            Some(v) if py_truthy(v) => Ok((Some("deal".into()), Some(to_uuid(v)?))),
            _ => Ok((None, None)),
        })),
    ]
}
