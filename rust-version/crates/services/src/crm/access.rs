//! Port of backend/app/services/crm/access.py: record-level CRM access, the owner scoping every CRM
//! surface applies to a single record.

use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};

/// `assert_can_view(user, rec)`. Owner scoping, mirroring `_crm_owner_scope` in services/ai/tools/catalog.py:
/// a user with only crm:view_own may not read a record assigned to someone else.
///
/// `owner` is the record's `assigned_to_id` (Python reads `getattr(rec, "assigned_to_id", None)`, so a
/// record without that column passes `None`).
pub fn assert_can_view(user: &AuthUser, owner: Option<Uuid>) -> ApiResult<()> {
    let perms = &user.perms;
    if user.is_superadmin || perms.contains("crm:view_all") {
        return Ok(());
    }
    if !perms.contains("crm:view_own") {
        return Err(ApiError::http(403, "Permission required: crm:view_own"));
    }
    if let Some(owner) = owner {
        if owner != user.id {
            return Err(ApiError::http(
                403,
                "This record is assigned to someone else",
            ));
        }
    }
    Ok(())
}

// 23 Oct: the activity visibility check moved here from routers/crm.rs, so GET /crm/activities/{id} and Nary's
// get_activity apply the same rule.

/// `_ACTIVITY_PARENTS`, in its key order: an activity's entity type and the table of its record.
pub const ACTIVITY_PARENTS: [(&str, &str); 4] = [
    ("contact", "contacts"),
    ("organization", "organizations"),
    ("lead", "leads"),
    ("deal", "deals"),
];

/// Whether this person sees every CRM record (crm:view_all, or a superadmin).
pub fn crm_sees_all(user: &AuthUser) -> bool {
    user.is_superadmin || user.perms.contains("crm:view_all")
}

/// `_assert_activity_visible(db, user, activity)`: the activity's record must be visible to the person (own
/// scope: unassigned or assigned to them); an activity on no known record only to its author.
pub async fn assert_activity_visible(
    db: &mut sqlx::PgConnection,
    user: &AuthUser,
    a: &granary_db::rows::crm::Activity,
) -> ApiResult<()> {
    if crm_sees_all(user) {
        return Ok(());
    }
    let Some((_, table)) = ACTIVITY_PARENTS.iter().find(|(k, _)| *k == a.entity_type) else {
        if a.created_by_id != Some(user.id) {
            return Err(ApiError::http(
                403,
                "This activity belongs to someone else's record",
            ));
        }
        return Ok(());
    };
    let parent: Option<(Option<Uuid>,)> =
        sqlx::query_as(&format!("SELECT assigned_to_id FROM {table} WHERE id = $1"))
            .bind(a.entity_id)
            .fetch_optional(&mut *db)
            .await?;
    let Some((owner,)) = parent else {
        return Err(ApiError::not_found("Activity", Some(&a.id.to_string())));
    };
    assert_can_view(user, owner)
}

/// 23 Oct: the longest note or activity text the CRM stores (Setting crm.activity.max_chars). A longer one is
/// refused with a clear 422, never cut silently.
pub async fn check_activity_length(
    db: &mut sqlx::PgConnection,
    content: Option<&str>,
) -> ApiResult<()> {
    let Some(text) = content else {
        return Ok(());
    };
    let max = granary_core::settings_store::require_int(
        db,
        "crm.activity.max_chars",
        "Sales & CRM \u{2192} Funnel",
    )
    .await?;
    let len = text.chars().count() as i64;
    if len > max {
        return Err(ApiError::http(
            422,
            format!(
                "The note is {len} characters long, over the {max} the CRM stores in one activity \
                 (crm.activity.max_chars). Split it into two notes, or raise the limit in Settings."
            ),
        ));
    }
    Ok(())
}
