//! Port of backend/app/services/nary/roles.py.
//!
//! SOP roles -> people (or Nary). Approvals and tasks route through these, so hiring a BDR is one
//! reassignment (design §6).
use granary_core::auth::{load_active_user, query_permissions, AuthUser};
use granary_core::errors::ApiResult;
use sqlx::PgConnection;
use uuid::Uuid;

pub const APPROVER_ROLE: &str = "Owner";

async fn assignment(
    db: &mut PgConnection,
    sop_role: &str,
) -> ApiResult<Option<(bool, Option<Uuid>)>> {
    Ok(
        sqlx::query_as("SELECT is_nary, user_id FROM nary_role_assignments WHERE sop_role = $1")
            .bind(sop_role)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// The user who holds an SOP role, or None when Nary holds it (or nobody is assigned).
pub async fn holder(db: &mut PgConnection, sop_role: &str) -> ApiResult<Option<Uuid>> {
    Ok(match assignment(db, sop_role).await? {
        Some((false, Some(user_id))) => Some(user_id),
        _ => None,
    })
}

pub async fn is_nary(db: &mut PgConnection, sop_role: &str) -> ApiResult<bool> {
    Ok(matches!(assignment(db, sop_role).await?, Some((true, _))))
}

/// A user with their permissions cached, as a request would have them; for work that runs
/// outside a request (Celery, Nary acting for someone).
///
/// Python reads the permissions through `get_user_permissions` (the Redis cache first); here they are
/// read from the database (the same set, never a stale cached one).
pub async fn load_actor(db: &mut PgConnection, user_id: Uuid) -> ApiResult<Option<AuthUser>> {
    let Some(mut user) = load_active_user(db, user_id).await? else {
        return Ok(None);
    };
    user.perms = query_permissions(db, user.id).await?;
    Ok(Some(user))
}

/// Who decides Nary's cards: the Owner role holder, else the Owner lookup used by approvals.
pub async fn approver_ids(db: &mut PgConnection) -> ApiResult<Vec<Uuid>> {
    if let Some(user_id) = holder(db, APPROVER_ROLE).await? {
        return Ok(vec![user_id]);
    }
    let ids = crate::approvals::routing::owner_user_ids(db).await?;
    Ok(ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect())
}

/// Who Nary acts as: the approver (Owner), with their permissions. Every action Nary takes is
/// recorded with origin "nary", so the audit trail still says it was Nary.
pub async fn nary_actor(db: &mut PgConnection) -> ApiResult<Option<AuthUser>> {
    let ids = approver_ids(db).await?;
    match ids.first() {
        Some(id) => load_actor(db, *id).await,
        None => Ok(None),
    }
}
