//! backend/app/routers/note_templates.py, mounted at `/api/v1/note-templates` (backend/app/main.py).
//!
//! Mounted with `dependencies=INTERNAL_ONLY`: the registry wraps these routes in
//! `granary_core::auth::internal_only`, so handlers need no extra check for portal users.
//!
//! Reusable named note templates (HTML) for document makers (quotation, invoice, ...).

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch};
use axum::Router;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use granary_compat::text::pystr;
use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/note-templates";

/// The routes of `router` in note_templates.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, ""), get(list_templates).post(create_template))
        .route(
            &at(PREFIX, "/{template_id}"),
            patch(update_template).delete(delete_template),
        )
}

type TemplateRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<i32>,
    Option<Uuid>,
);

const COLS: &str =
    "note_templates.id, note_templates.name, note_templates.body, note_templates.doc_type, \
                    note_templates.sort_order, note_templates.created_by_id";

fn template_dict(t: &TemplateRow) -> Value {
    json!({
        "id": t.0.to_string(),
        "name": t.1,
        "body": t.2,
        "doc_type": t.3,
        "sort_order": t.4,
        "created_by_id": t.5.map(|u| u.to_string()),
    })
}

/// Security 26 Oct: a template's HTML is cleaned when saved (it is put into the editor of whoever uses it).
fn clean_body(body: &str) -> String {
    granary_services::verification::render::sanitize_html(Some(body))
}

/// Security 26 Oct: a shared template is changed or deleted only by who made it, the Owner or an admin.
async fn may_change(
    db: &Db,
    user: &granary_core::auth::AuthUser,
    t: &TemplateRow,
) -> ApiResult<()> {
    if user.is_superadmin || user.is_system_admin || t.5 == Some(user.id) {
        return Ok(());
    }
    let mut conn = db.conn().await?;
    if granary_services::nary::roles::approver_ids(&mut conn)
        .await?
        .contains(&user.id)
    {
        return Ok(());
    }
    Err(ApiError::http(
        403,
        "Only the person who made this template, the Owner or an admin can change it",
    ))
}

granary_core::py_model! {
    /// The query of `list_templates`.
    pub struct ListQ {
        pub doc_type: Option<String> = None,
    }
}

/// `GET ""`: templates. `doc_type` returns that type plus shared (null doc_type) templates.
pub async fn list_templates(
    CurrentUser(_user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListQ>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let rows: Vec<TemplateRow> = match q.doc_type.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => {
            sqlx::query_as(&format!(
                "SELECT {COLS} FROM note_templates WHERE note_templates.doc_type = $1 \
                 OR note_templates.doc_type IS NULL \
                 ORDER BY note_templates.sort_order, note_templates.name"
            ))
            .bind(d)
            .fetch_all(&mut *conn)
            .await?
        }
        None => {
            sqlx::query_as(&format!(
                "SELECT {COLS} FROM note_templates ORDER BY note_templates.sort_order, note_templates.name"
            ))
            .fetch_all(&mut *conn)
            .await?
        }
    };
    Ok(resp::json(Value::Array(
        rows.iter().map(template_dict).collect(),
    )))
}

granary_core::py_model! {
    /// `class NoteTemplateBody(BaseModel)`.
    pub struct NoteTemplateBody {
        pub name: String,
        pub body: String,
        pub doc_type: Option<String> = None,
        pub sort_order: i64 = 0,
    }
}

/// `POST ""` (201).
pub async fn create_template(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<NoteTemplateBody>,
) -> ApiResult<Response> {
    let id = Uuid::new_v4();
    let name = pystr::strip(&body.name).to_string();
    let doc_type = body.doc_type.clone().filter(|d| !d.is_empty());
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO note_templates (name, body, doc_type, sort_order, created_by_id, id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&name)
    .bind(clean_body(&body.body))
    .bind(&doc_type)
    .bind(body.sort_order)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({
            "id": id.to_string(),
            "name": name,
            "body": clean_body(&body.body),
            "doc_type": doc_type,
            "sort_order": body.sort_order,
        }),
    ))
}

async fn load(db: &Db, template_id: &str) -> ApiResult<TemplateRow> {
    let id = pg_uuid(template_id)?;
    let mut conn = db.conn().await?;
    let row: Option<TemplateRow> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM note_templates WHERE note_templates.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    row.ok_or_else(|| ApiError::not_found("NoteTemplate", Some(template_id)))
}

/// What asyncpg makes of a JSON value bound to a text column: a str, NULL, or its DataError (a 500).
fn text_arg(k: &str, v: &Value) -> ApiResult<Option<String>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        other => Err(ApiError::internal(anyhow::anyhow!(
            "DataError: invalid input for query argument ({k}): {other} (expected str)"
        ))),
    }
}

/// The same for the INTEGER `sort_order` (a bool is an int to asyncpg).
fn int_arg(v: &Value) -> ApiResult<Option<i64>> {
    match v {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(*b as i64)),
        Value::Number(n) if n.is_i64() => Ok(n.as_i64()),
        other => Err(ApiError::internal(anyhow::anyhow!(
            "DataError: invalid input for query argument (sort_order): {other} (an integer is required)"
        ))),
    }
}

/// `PATCH "/{template_id}"`: the body is a free dict; name/body/doc_type/sort_order are copied as sent.
pub async fn update_template(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(template_id): Path<String>,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    let t = load(&db, &template_id).await?;
    may_change(&db, &user, &t).await?;
    let mut out = template_dict(&t);
    let mut sets: Vec<String> = Vec::new();
    let mut texts: Vec<Option<String>> = Vec::new();
    let mut sort: Option<Option<i64>> = None;
    for k in ["name", "body", "doc_type", "sort_order"] {
        if let Some(v) = body.get(k) {
            // setattr(t, k, body[k]): the response shows the value as sent (expire_on_commit=False).
            out[k] = v.clone();
            if k == "sort_order" {
                sort = Some(int_arg(v)?);
                sets.push("sort_order = $S".into());
            } else {
                let mut v = text_arg(k, v)?;
                if k == "body" {
                    v = v.map(|b| clean_body(&b));
                    out[k] = json!(v);
                }
                texts.push(v);
                sets.push(format!("{k} = ${}", texts.len() + 1));
            }
        }
    }
    if !sets.is_empty() {
        let sort_idx = texts.len() + 2;
        let set_sql = sets
            .iter()
            .map(|s| s.replace("$S", &format!("${sort_idx}")))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE note_templates SET {set_sql}, updated_at = now() WHERE note_templates.id = $1"
        );
        let mut q = sqlx::query(&sql).bind(t.0);
        for v in &texts {
            q = q.bind(v);
        }
        if let Some(s) = sort {
            q = q.bind(s);
        }
        let mut conn = db.conn().await?;
        q.execute(&mut *conn).await?;
    }
    db.commit().await?;
    Ok(resp::json(out))
}

/// `DELETE "/{template_id}"` (204).
pub async fn delete_template(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(template_id): Path<String>,
) -> ApiResult<Response> {
    let t = load(&db, &template_id).await?;
    may_change(&db, &user, &t).await?;
    let mut conn = db.conn().await?;
    sqlx::query("DELETE FROM note_templates WHERE note_templates.id = $1")
        .bind(t.0)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
