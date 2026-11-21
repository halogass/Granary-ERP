//! partner.py, training: admin module management and the portal's module list, detail, start, quiz and
//! certifications.

use crate::extract::pg_uuid;
use std::collections::HashMap;

use axum::extract::Path;
use axum::response::Response;
use chrono::Utc;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_core::auth::CurrentUser;
use granary_core::db::Db;
use granary_core::validation::PyJson;
use granary_core::{ApiError, ApiResult};
use granary_db::rows::partner::{
    PartnerAccount, PartnerCertification, PartnerModuleProgress, TrainingChapter, TrainingModule,
    TrainingQuizQuestion,
};

use super::admin::{setattr_update, ColKind};
use super::portal::portal_partner;
use super::{
    col_int, col_str, one_or_none, py_500, py_repr, require_partner_portal, require_perm, tier_rank,
};
use crate::resp::{self, iso, iso_opt};

type Body = Map<String, Value>;

granary_core::py_model! {
    /// `class TrainingModuleCreate(BaseModel)`.
    pub struct TrainingModuleCreate {
        pub title: String,
        pub description: Option<String> = None,
        pub min_tier: String = "authorized",
        pub partner_types: Vec<String> = Vec::new(),
        pub pass_score: i64 = 70,
    }
}

// ─── Admin ────────────────────────────────────────────────────────────────────

/// `GET /training/modules`: all (non-deleted) modules with their chapter counts.
pub async fn list_training_modules(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:view")?;
    let mut conn = db.conn().await?;
    let rows = sqlx::query(
        "SELECT training_modules.*, coalesce(anon_1.n, 0) AS coalesce_1 \
         FROM training_modules LEFT OUTER JOIN (SELECT training_chapters.module_id AS module_id, count(*) AS n \
         FROM training_chapters GROUP BY training_chapters.module_id) AS anon_1 ON anon_1.module_id = training_modules.id \
         WHERE training_modules.is_deleted = false ORDER BY training_modules.created_at DESC",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let m = <TrainingModule as sqlx::FromRow<_>>::from_row(row)?;
        let n: i64 = row.try_get("coalesce_1")?;
        out.push(json!({
            "id": m.id.to_string(), "title": m.title, "description": m.description,
            "min_tier": m.min_tier, "partner_types": m.partner_types.clone().unwrap_or_default(),
            "is_published": m.is_published, "pass_score": m.pass_score,
            "chapter_count": n,
            "created_at": iso(&m.created_at),
        }));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /training/modules` (201): an unpublished module.
pub async fn create_training_module(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<TrainingModuleCreate>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    super::require_option(super::PARTNER_TIERS, "min_tier", &body.min_tier)?;
    let mut conn = db.conn().await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO training_modules (id, title, description, min_tier, partner_types, is_published, pass_score, \
           created_by_id, is_deleted, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, false, $6, $7, false, now(), now())",
    )
    .bind(id)
    .bind(&body.title)
    .bind(&body.description)
    .bind(&body.min_tier)
    .bind(&body.partner_types)
    .bind(body.pass_score)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "title": body.title}),
    ))
}

/// A live module's id, or `NotFoundError("TrainingModule", module_id)`.
async fn live_module_id(conn: &mut PgConnection, module_id: &str) -> ApiResult<Uuid> {
    let id = pg_uuid(module_id)?;
    let found: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM training_modules WHERE training_modules.id = $1 AND training_modules.is_deleted = false",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    found
        .map(|(id,)| id)
        .ok_or_else(|| ApiError::not_found("TrainingModule", Some(module_id)))
}

/// `PATCH /training/modules/{module_id}`.
pub async fn update_training_module(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(module_id): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    super::require_min_tier_patch(&body)?;
    let mut conn = db.conn().await?;
    let id = live_module_id(&mut conn, &module_id).await?;
    setattr_update(
        &mut conn,
        "training_modules",
        id,
        &body,
        &[
            ("title", ColKind::Str),
            ("description", ColKind::Str),
            ("min_tier", ColKind::Str),
            ("partner_types", ColKind::StrArray),
            ("is_published", ColKind::Bool),
            ("pass_score", ColKind::Int),
            ("thumbnail_url", ColKind::Str),
        ],
    )
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": id.to_string()})))
}

/// `body[key]` (KeyError: 500) on a dict, or a TypeError (500) on anything else.
fn item<'a>(obj: &'a Value, key: &str) -> ApiResult<&'a Value> {
    match obj {
        Value::Object(m) => m
            .get(key)
            .ok_or_else(|| py_500(format!("KeyError: {key:?}"))),
        other => Err(py_500(format!(
            "TypeError: {} indices must be integers",
            py_repr(other)
        ))),
    }
}

/// `for x in value`: the elements Python iterates (a str yields one-character strs, a dict its keys); a
/// value that is not iterable is a TypeError (500).
fn py_iter(v: &Value) -> ApiResult<Vec<Value>> {
    match v {
        Value::Array(items) => Ok(items.clone()),
        Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        Value::Object(m) => Ok(m.keys().map(|k| Value::String(k.clone())).collect()),
        other => Err(py_500(format!(
            "TypeError: {} is not iterable",
            py_repr(other)
        ))),
    }
}

/// `POST /training/modules/{module_id}/chapters` (201): a chapter appended at the end, with its quiz
/// questions.
pub async fn add_training_chapter(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(module_id): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let mid = live_module_id(&mut conn, &module_id).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) AS count_1 FROM training_chapters WHERE training_chapters.module_id = $1",
    )
    .bind(mid)
    .fetch_one(&mut *conn)
    .await?;
    let next_order = count + 1;
    // TrainingChapter(title=body["title"], ...) then flush.
    let title = body
        .get("title")
        .ok_or_else(|| py_500("KeyError: 'title'"))?;
    let title = col_str(Some(title))?;
    let content = col_str(body.get("content"))?;
    let video_url = col_str(body.get("video_url"))?;
    let chapter_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO training_chapters (id, module_id, title, content, video_url, \"order\", created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now(), now())",
    )
    .bind(chapter_id)
    .bind(mid)
    .bind(&title)
    .bind(&content)
    .bind(&video_url)
    .bind(next_order)
    .execute(&mut *conn)
    .await?;
    // The questions loop (KeyError / TypeError raise at once), then the commit's flush of them all.
    let questions = match body.get("questions") {
        None => Vec::new(),
        Some(v) => py_iter(v)?,
    };
    let mut rows = Vec::with_capacity(questions.len());
    for q in &questions {
        let question = item(q, "question")?.clone();
        let options = item(q, "options")?.clone();
        let correct_index = item(q, "correct_index")?.clone();
        let order = match q.get("order") {
            Some(v) => v.clone(),
            None => json!(1),
        };
        rows.push((question, options, correct_index, order));
    }
    let mut typed = Vec::with_capacity(rows.len());
    for (question, options, correct_index, order) in &rows {
        typed.push((
            col_str(Some(question))?,
            options.clone(),
            col_int(Some(correct_index))?,
            col_int(Some(order))?,
        ));
    }
    for (question, options, correct_index, order) in typed {
        sqlx::query(
            "INSERT INTO training_quiz_questions (id, chapter_id, question, options, correct_index, \"order\", \
               created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, now(), now())",
        )
        .bind(Uuid::new_v4())
        .bind(chapter_id)
        .bind(question)
        // JSONB: SQLAlchemy writes a Python None as the JSON null.
        .bind(sqlx::types::Json(options))
        .bind(correct_index)
        .bind(order)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": chapter_id.to_string(), "title": title, "order": next_order}),
    ))
}

/// `DELETE /training/modules/{module_id}` (204): soft delete.
pub async fn delete_training_module(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(module_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let id = live_module_id(&mut conn, &module_id).await?;
    sqlx::query(
        "UPDATE training_modules SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `GET /training/modules/{module_id}/progress`: each partner user's progress on a module (attempts used,
/// last score, status), so staff can see who ran out of quiz attempts.
pub async fn list_training_progress(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(module_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let id = live_module_id(&mut conn, &module_id).await?;
    let max_attempts = granary_core::settings_store::require_int(
        &mut conn,
        MAX_ATTEMPTS_KEY,
        "Sales & CRM \u{2192} Partners",
    )
    .await?;
    let rows = sqlx::query(
        "SELECT partner_module_progress.*, users.name AS user_name, users.email AS user_email, \
           partner_accounts.company_name AS partner_name \
         FROM partner_module_progress JOIN users ON users.id = partner_module_progress.partner_user_id \
         LEFT OUTER JOIN partner_accounts ON partner_accounts.id = users.partner_account_id \
         WHERE partner_module_progress.module_id = $1 ORDER BY partner_module_progress.updated_at DESC",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let p = <PartnerModuleProgress as sqlx::FromRow<_>>::from_row(row)?;
        let user_name: Option<String> = row.try_get("user_name")?;
        let user_email: Option<String> = row.try_get("user_email")?;
        let partner_name: Option<String> = row.try_get("partner_name")?;
        out.push(json!({
            "id": p.id.to_string(), "partner_user_id": p.partner_user_id.to_string(),
            "user_name": user_name, "user_email": user_email, "partner_name": partner_name,
            "status": p.status, "score": p.score, "attempts": p.attempts,
            "attempts_left": (max_attempts - i64::from(p.attempts)).max(0),
            "updated_at": iso(&p.updated_at),
        }));
    }
    Ok(resp::json(Value::Array(out)))
}

/// `POST /training/progress/{progress_id}/reset`: give a partner user their quiz attempts back. A module
/// already passed is not reset (its certificate stands).
pub async fn reset_training_attempts(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(progress_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "partner_mgmt:manage")?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&progress_id)?;
    let p: Option<PartnerModuleProgress> = sqlx::query_as(
        "SELECT * FROM partner_module_progress WHERE partner_module_progress.id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(p) = p else {
        return Err(ApiError::not_found("TrainingProgress", Some(&progress_id)));
    };
    if p.status.as_deref() == Some("passed") {
        return Err(ApiError::http(
            400,
            "This partner already passed the module; there is nothing to reset",
        ));
    }
    sqlx::query(
        "UPDATE partner_module_progress SET attempts = 0, updated_at = now() WHERE id = $1",
    )
    .bind(p.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": p.id.to_string(), "attempts": 0})))
}

// ─── Portal ───────────────────────────────────────────────────────────────────

/// `GET /portal/training/stats`.
pub async fn portal_training_stats(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let partner = portal_partner(&mut conn, &user).await?;
    let rank = tier_rank(partner.tier.as_deref());
    let modules: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, min_tier FROM training_modules \
         WHERE training_modules.is_deleted = false AND training_modules.is_published = true",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut accessible: Vec<Uuid> = Vec::new();
    for (id, min_tier) in modules {
        if tier_rank(min_tier.as_deref()) <= rank && !accessible.contains(&id) {
            accessible.push(id);
        }
    }
    if accessible.is_empty() {
        return Ok(resp::json(json!({"total": 0, "passed": 0})));
    }
    let passed: i64 = sqlx::query_scalar(
        "SELECT count(*) AS count_1 FROM partner_module_progress \
         WHERE partner_module_progress.partner_user_id = $1 AND partner_module_progress.status = 'passed' \
           AND partner_module_progress.module_id = ANY($2)",
    )
    .bind(user.id)
    .bind(&accessible)
    .fetch_one(&mut *conn)
    .await?;
    Ok(resp::json(
        json!({"total": accessible.len(), "passed": passed}),
    ))
}

/// `GET /portal/training`: published modules this partner's tier may take, with the caller's progress.
pub async fn portal_list_training(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let partner = sqlx::query_as::<_, PartnerAccount>(
        "SELECT * FROM partner_accounts WHERE partner_accounts.id = $1",
    )
    .bind(partner_id)
    .fetch_optional(&mut *conn)
    .await?;
    // `partner.tier if partner else "authorized"`, then TIER_ORDER.get(tier, 0).
    let rank = match &partner {
        Some(p) => tier_rank(p.tier.as_deref()),
        None => 0,
    };
    let modules = sqlx::query_as::<_, TrainingModule>(
        "SELECT * FROM training_modules WHERE training_modules.is_deleted = false \
           AND training_modules.is_published = true ORDER BY training_modules.created_at DESC",
    )
    .fetch_all(&mut *conn)
    .await?;
    let progress = sqlx::query_as::<_, PartnerModuleProgress>(
        "SELECT * FROM partner_module_progress WHERE partner_module_progress.partner_user_id = $1",
    )
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    // {str(p.module_id): p}: a later row for the same module replaces an earlier one.
    let mut progress_map: HashMap<Uuid, &PartnerModuleProgress> = HashMap::new();
    for p in &progress {
        progress_map.insert(p.module_id, p);
    }
    let out: Vec<Value> = modules
        .iter()
        .filter(|m| tier_rank(m.min_tier.as_deref()) <= rank)
        .map(|m| {
            let p = progress_map.get(&m.id);
            json!({
                "id": m.id.to_string(),
                "title": m.title,
                "description": m.description,
                "thumbnail_url": m.thumbnail_url,
                "min_tier": m.min_tier,
                "pass_score": m.pass_score,
                "progress_status": match p { Some(p) => json!(p.status), None => json!("not_started") },
                "progress_score": p.and_then(|p| shown_score(p)),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET /portal/training/certifications`: the caller's certificates.
pub async fn portal_list_certifications(
    CurrentUser(user): CurrentUser,
    db: Db,
) -> ApiResult<Response> {
    require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let certs = sqlx::query_as::<_, PartnerCertification>(
        "SELECT * FROM partner_certifications WHERE partner_certifications.partner_user_id = $1 \
         ORDER BY partner_certifications.issued_at DESC",
    )
    .bind(user.id)
    .fetch_all(&mut *conn)
    .await?;
    // selectinload(PartnerCertification.module).
    let mut titles: HashMap<Uuid, String> = HashMap::new();
    let ids: Vec<Uuid> = certs.iter().map(|c| c.module_id).collect();
    if !ids.is_empty() {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, title FROM training_modules WHERE training_modules.id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await?;
        titles.extend(rows);
    }
    let out: Vec<Value> = certs
        .iter()
        .map(|c| {
            json!({
                "id": c.id.to_string(),
                "module_id": c.module_id.to_string(),
                "module_title": titles.get(&c.module_id),
                "badge_slug": c.badge_slug,
                "issued_at": iso_opt(c.issued_at.as_ref()),
                "expires_at": iso_opt(c.expires_at.as_ref()),
                "certificate_url": c.certificate_url,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `selectinload(TrainingModule.chapters).selectinload(TrainingChapter.questions)`: chapters by "order",
/// each with its questions by "order" (one IN query per level, rows in arrival order).
async fn chapters_with_questions(
    conn: &mut PgConnection,
    module_id: Uuid,
) -> ApiResult<Vec<(TrainingChapter, Vec<TrainingQuizQuestion>)>> {
    let chapters = sqlx::query_as::<_, TrainingChapter>(
        "SELECT * FROM training_chapters WHERE training_chapters.module_id IN ($1) \
         ORDER BY training_chapters.\"order\"",
    )
    .bind(module_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut questions: Vec<TrainingQuizQuestion> = Vec::new();
    if !chapters.is_empty() {
        let marks: Vec<String> = (1..=chapters.len()).map(|i| format!("${i}")).collect();
        let sql = format!(
            "SELECT * FROM training_quiz_questions WHERE training_quiz_questions.chapter_id IN ({}) \
             ORDER BY training_quiz_questions.\"order\"",
            marks.join(", ")
        );
        let mut q = sqlx::query_as::<_, TrainingQuizQuestion>(&sql);
        for c in &chapters {
            q = q.bind(c.id);
        }
        questions = q.fetch_all(&mut *conn).await?;
    }
    Ok(chapters
        .into_iter()
        .map(|c| {
            let qs = questions
                .iter()
                .filter(|q| q.chapter_id == c.id)
                .cloned()
                .collect();
            (c, qs)
        })
        .collect())
}

/// The caller's progress row on a module (`scalar_one_or_none`).
async fn progress_of(
    conn: &mut PgConnection,
    user_id: Uuid,
    module_id: Uuid,
) -> ApiResult<Option<PartnerModuleProgress>> {
    let rows = sqlx::query_as::<_, PartnerModuleProgress>(
        "SELECT * FROM partner_module_progress WHERE partner_module_progress.partner_user_id = $1 \
           AND partner_module_progress.module_id = $2 LIMIT 2",
    )
    .bind(user_id)
    .bind(module_id)
    .fetch_all(&mut *conn)
    .await?;
    one_or_none(rows)
}

/// The most quiz submissions a partner user gets per module.
pub const MAX_ATTEMPTS_KEY: &str = "partner.training.max_attempts";

/// A module the calling partner may take, exactly as `GET /portal/training` lists them: not deleted, published,
/// and its `min_tier` at or below the partner's tier. Anything else is "not found".
async fn module_for_partner(
    conn: &mut PgConnection,
    partner_id: Uuid,
    module_id: Uuid,
) -> ApiResult<TrainingModule> {
    let partner = sqlx::query_as::<_, PartnerAccount>(
        "SELECT * FROM partner_accounts WHERE partner_accounts.id = $1",
    )
    .bind(partner_id)
    .fetch_optional(&mut *conn)
    .await?;
    let rank = match &partner {
        Some(p) => tier_rank(p.tier.as_deref()),
        None => 0,
    };
    sqlx::query_as::<_, TrainingModule>(
        "SELECT * FROM training_modules WHERE training_modules.id = $1 AND training_modules.is_deleted = false \
           AND training_modules.is_published = true",
    )
    .bind(module_id)
    .fetch_optional(&mut *conn)
    .await?
    .filter(|m| tier_rank(m.min_tier.as_deref()) <= rank)
    .ok_or_else(|| ApiError::http(404, "Training module not found"))
}

/// The score a partner sees: only once passed (before that the score would tell how many answers were right).
fn shown_score(p: &PartnerModuleProgress) -> Option<i32> {
    if p.status.as_deref() == Some("passed") {
        p.score
    } else {
        None
    }
}

/// `GET /portal/training/{module_id}`: a published module with its chapters, questions (without the
/// answers) and the caller's progress.
pub async fn portal_get_training_module(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(module_id): Path<String>,
) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&module_id)?;
    let module = module_for_partner(&mut conn, partner_id, id).await?;
    let chapters: Vec<Value> = chapters_with_questions(&mut conn, module.id)
        .await?
        .iter()
        .map(|(c, qs)| {
            json!({
                "id": c.id.to_string(),
                "title": c.title,
                "content": c.content,
                "video_url": c.video_url,
                "order": c.order,
                "questions": qs.iter().map(|q| json!({
                    "id": q.id.to_string(), "question": q.question, "options": q.options, "order": q.order,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let progress = progress_of(&mut conn, user.id, id).await?;
    Ok(resp::json(json!({
        "id": module.id.to_string(),
        "title": module.title,
        "description": module.description,
        "pass_score": module.pass_score,
        "chapters": chapters,
        "progress": {
            "status": match &progress { Some(p) => json!(p.status), None => json!("not_started") },
            "current_chapter": match &progress { Some(p) => json!(p.current_chapter), None => json!(0) },
            "score": progress.as_ref().and_then(shown_score),
        },
    })))
}

/// `POST /portal/training/{module_id}/start`.
pub async fn portal_start_training(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(module_id): Path<String>,
) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&module_id)?;
    module_for_partner(&mut conn, partner_id, id).await?;
    let progress = progress_of(&mut conn, user.id, id).await?;
    let now = Utc::now();
    let status: Option<String> = match progress {
        None => {
            // A module that does not exist is the foreign key's IntegrityError (409).
            sqlx::query(
                "INSERT INTO partner_module_progress (id, partner_user_id, module_id, status, current_chapter, \
                   started_at, created_at, updated_at) VALUES ($1, $2, $3, 'in_progress', 0, $4, now(), now())",
            )
            .bind(Uuid::new_v4())
            .bind(user.id)
            .bind(id)
            .bind(now)
            .execute(&mut *conn)
            .await?;
            Some("in_progress".into())
        }
        Some(p) if p.status.as_deref() == Some("not_started") => {
            sqlx::query(
                "UPDATE partner_module_progress SET status = 'in_progress', started_at = $2, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(p.id)
            .bind(now)
            .execute(&mut *conn)
            .await?;
            Some("in_progress".into())
        }
        Some(p) => p.status,
    };
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({ "status": status })))
}

/// `POST /portal/training/{module_id}/submit-quiz`: `{answers: [{question_id, selected_index}, ...]}`.
pub async fn portal_submit_quiz(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(module_id): Path<String>,
    PyJson(body): PyJson<Body>,
) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&module_id)?;
    let module = module_for_partner(&mut conn, partner_id, id).await?;
    let max_attempts = granary_core::settings_store::require_int(
        &mut conn,
        MAX_ATTEMPTS_KEY,
        "Sales & CRM \u{2192} Partners",
    )
    .await?;
    // FOR UPDATE: two submissions at once must not both slip under the cap.
    let progress: Option<PartnerModuleProgress> = one_or_none(
        sqlx::query_as::<_, PartnerModuleProgress>(
            "SELECT * FROM partner_module_progress WHERE partner_module_progress.partner_user_id = $1 \
               AND partner_module_progress.module_id = $2 LIMIT 2 FOR UPDATE",
        )
        .bind(user.id)
        .bind(id)
        .fetch_all(&mut *conn)
        .await?,
    )?;
    let attempts = progress.as_ref().map_or(0, |p| i64::from(p.attempts));
    if attempts >= max_attempts {
        return Err(ApiError::http(
            403,
            format!(
                "You have used all {max_attempts} attempts at this quiz. Contact your partner manager to try again."
            ),
        ));
    }
    let mut all_questions: HashMap<String, i32> = HashMap::new();
    for (_, qs) in chapters_with_questions(&mut conn, module.id).await? {
        for q in qs {
            all_questions.insert(q.id.to_string(), q.correct_index);
        }
    }
    let answers = body.get("answers").cloned().unwrap_or(json!([]));
    if all_questions.is_empty() {
        return Err(ApiError::http(400, "This module has no quiz questions"));
    }
    let Value::Array(answers) = answers else {
        return Err(ApiError::http(400, "answers must be a list"));
    };
    // One answer per question (the first one sent counts), so repeating a right answer cannot push the
    // score past 100. selected_index must be a real integer: true and 0.0 equal 1 and 0 in Python.
    let mut chosen: HashMap<String, Value> = HashMap::new();
    for a in &answers {
        let Value::Object(a) = a else { continue };
        let Some(Value::String(qid)) = a.get("question_id") else {
            continue;
        };
        if all_questions.contains_key(qid) {
            chosen
                .entry(qid.clone())
                .or_insert_with(|| a.get("selected_index").cloned().unwrap_or(Value::Null));
        }
    }
    let correct = chosen
        .iter()
        .filter(|(qid, sel)| match sel {
            Value::Number(n) if n.is_i64() || n.is_u64() => {
                n.as_i64() == Some(i64::from(all_questions[qid.as_str()]))
            }
            _ => false,
        })
        .count() as i64;
    let total = all_questions.len() as i64;
    let score = ((correct as f64 / total as f64) * 100.0).round_ties_even() as i64;
    let Some(pass_score) = module.pass_score else {
        return Err(py_500(
            "TypeError: '>=' not supported between instances of 'int' and 'NoneType'",
        ));
    };
    let passed = score >= pass_score as i64;
    let now = Utc::now();
    let status = if passed { "passed" } else { "failed" };
    match progress {
        None => {
            sqlx::query(
                "INSERT INTO partner_module_progress (id, partner_user_id, module_id, status, current_chapter, score, \
                   started_at, completed_at, attempts, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, 0, $5, $6, $6, 1, now(), now())",
            )
            .bind(Uuid::new_v4())
            .bind(user.id)
            .bind(id)
            .bind(status)
            .bind(score)
            .bind(now)
            .execute(&mut *conn)
            .await?;
        }
        Some(p) => {
            sqlx::query(
                "UPDATE partner_module_progress SET status = $2, score = $3, completed_at = $4, \
                 attempts = attempts + 1, updated_at = now() WHERE id = $1",
            )
            .bind(p.id)
            .bind(status)
            .bind(score)
            .bind(now)
            .execute(&mut *conn)
            .await?;
        }
    }
    let mut cert_id: Option<Uuid> = None;
    if passed {
        let existing: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM partner_certifications WHERE partner_certifications.partner_user_id = $1 \
               AND partner_certifications.module_id = $2 LIMIT 2",
        )
        .bind(user.id)
        .bind(id)
        .fetch_all(&mut *conn)
        .await?;
        cert_id = one_or_none(existing)?.map(|(c,)| c);
        if cert_id.is_none() {
            let new_id = Uuid::new_v4();
            cert_id = Some(new_id);
            let slug: String = module
                .title
                .to_lowercase()
                .replace(' ', "-")
                .chars()
                .take(50)
                .collect();
            sqlx::query(
                "INSERT INTO partner_certifications (id, partner_user_id, module_id, issued_at, badge_slug, \
                   created_at, updated_at) VALUES ($1, $2, $3, $4, $5, now(), now())",
            )
            .bind(new_id)
            .bind(user.id)
            .bind(id)
            .bind(now)
            .bind(slug)
            .execute(&mut *conn)
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    let attempts_left = (max_attempts - attempts - 1).max(0);
    // Before a pass, only the verdict: the score or the number right would let a partner find the answers by
    // changing one answer at a time.
    if !passed {
        return Ok(resp::json(json!({
            "passed": false, "pass_score": pass_score, "attempts_left": attempts_left,
            "certification_issued": false,
        })));
    }
    // The certificate is a PDF the partner downloads with their own token (cert_url is that API path).
    let cert_url =
        cert_id.map(|c| format!("/api/v1/partner/portal/training/certifications/{c}/pdf"));
    Ok(resp::json(json!({
        "score": score, "passed": passed, "correct": correct, "total": total,
        "certification_issued": passed, "attempts_left": attempts_left,
        "certification_id": cert_id.map(|c| c.to_string()), "cert_url": cert_url,
    })))
}

/// `GET /portal/training/certifications/{cert_id}/pdf`: the caller's own certificate as a PDF (another
/// partner user's certificate is a 404).
pub async fn portal_certificate_pdf(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(cert_id): Path<String>,
) -> ApiResult<Response> {
    let partner_id = require_partner_portal(&user)?;
    let id = pg_uuid(&cert_id)?;
    let mut conn = db.conn().await?;
    let cert = sqlx::query_as::<_, PartnerCertification>(
        "SELECT * FROM partner_certifications WHERE partner_certifications.id = $1 \
           AND partner_certifications.partner_user_id = $2",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::http(404, "Certificate not found"))?;
    let module_title: Option<String> =
        sqlx::query_scalar("SELECT title FROM training_modules WHERE training_modules.id = $1")
            .bind(cert.module_id)
            .fetch_optional(&mut *conn)
            .await?;
    let partner_company: Option<String> = sqlx::query_scalar(
        "SELECT company_name FROM partner_accounts WHERE partner_accounts.id = $1",
    )
    .bind(partner_id)
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    let tz = granary_core::clock::local_tz(&mut conn).await?;
    let company: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT key, value FROM system_settings WHERE system_settings.category = 'company'",
    )
    .fetch_all(&mut *conn)
    .await?;
    drop(conn);
    let get = |k: &str| -> String {
        company
            .iter()
            .rev()
            .find(|(key, _)| key == k)
            .and_then(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let or = |a: String, b: String| if a.is_empty() { b } else { a };
    let date = |d: Option<&chrono::DateTime<Utc>>| {
        d.map(|d| d.with_timezone(&tz).format("%-d %B %Y").to_string())
            .unwrap_or_default()
    };
    let number = format!("CERT-{}", cert.id.simple().to_string()[..8].to_uppercase());
    let data = json!({
        "certificate_number": number,
        "holder_name": user.name,
        "partner_company": partner_company.unwrap_or_default(),
        "module_title": module_title.unwrap_or_default(),
        "issued_date": date(cert.issued_at.as_ref()),
        "expires_date": date(cert.expires_at.as_ref()),
        "letterhead_url": get("company.letterhead_image"),
        "company_name": or(or(get("company_trade_name"), get("company_name")), "Your Company".into()),
        "company_website": get("company_website"),
    });
    let pdf = granary_services::pdf::generate_pdf("partner_certificate", &data, None, None).await?;
    Ok(resp::with_headers(
        resp::bytes_response(200, pdf, "application/pdf"),
        &[(
            "content-disposition",
            format!("attachment; filename=\"{number}.pdf\""),
        )],
    ))
}
