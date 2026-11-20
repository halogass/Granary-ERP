//! Who may do what in the document library.
//!
//! - view: an internal user with documents:view whose roles meet the category's visible_roles (empty = all);
//!   directors (documents:manage_all) and doc admins (documents:admin) see every category.
//! - edit (metadata, new versions): the owner with documents:create, or documents:manage_all.
//! - approve customer/partner versions: documents:manage_all only (doc admins do not approve).
//! - history (old versions), restore, owner change, category access, trace: doc admins (manage_all or admin).
//!
//! The superadmin user bypasses permissions as everywhere else.

use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};

use super::Document;

pub const PERM_VIEW: &str = "documents:view";
pub const PERM_CREATE: &str = "documents:create";
pub const PERM_MANAGE_ALL: &str = "documents:manage_all";
pub const PERM_ADMIN: &str = "documents:admin";

fn has(user: &AuthUser, code: &str) -> bool {
    user.is_superadmin || user.perms.contains(code)
}

/// documents:manage_all (all directors): edit any document, approve, history, owner, categories, trace.
pub fn is_manager_all(user: &AuthUser) -> bool {
    user.is_internal() && has(user, PERM_MANAGE_ALL)
}

/// A doc admin: documents:manage_all or documents:admin (history, restore, owner, categories, trace).
pub fn is_doc_admin(user: &AuthUser) -> bool {
    user.is_internal() && (is_manager_all(user) || has(user, PERM_ADMIN) || user.is_system_admin)
}

/// May approve a customer/partner version (documents:manage_all only).
pub fn can_approve(user: &AuthUser) -> bool {
    is_manager_all(user)
}

/// documents:create (or a director).
pub fn can_create(user: &AuthUser) -> bool {
    user.is_internal() && (has(user, PERM_CREATE) || is_manager_all(user))
}

/// Sees every category regardless of its roles.
pub fn sees_all_categories(user: &AuthUser) -> bool {
    is_doc_admin(user)
}

/// The role slugs of a user.
pub async fn role_slugs(db: &mut PgConnection, user_id: Uuid) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT r.slug FROM roles r JOIN user_roles ur ON ur.role_id = r.id WHERE ur.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&mut *db)
    .await?)
}

/// May this user see documents of this category at all.
pub async fn can_view_category(
    db: &mut PgConnection,
    user: &AuthUser,
    category_id: Uuid,
) -> ApiResult<bool> {
    if !user.is_internal() || (!has(user, PERM_VIEW) && !is_doc_admin(user)) {
        return Ok(false);
    }
    if sees_all_categories(user) {
        return Ok(true);
    }
    let roles = role_slugs(db, user.id).await?;
    let ok: Option<bool> = sqlx::query_scalar(
        "SELECT cardinality(visible_roles) = 0 OR visible_roles && $2::text[] FROM doc_categories WHERE id = $1",
    )
    .bind(category_id)
    .bind(&roles)
    .fetch_optional(&mut *db)
    .await?;
    Ok(ok.unwrap_or(false))
}

/// May this user see the document (and its live version).
pub async fn can_view(db: &mut PgConnection, user: &AuthUser, doc: &Document) -> ApiResult<bool> {
    if doc.is_deleted {
        return Ok(false);
    }
    can_view_category(db, user, doc.category_id).await
}

/// May this user edit the document's details and upload versions.
pub async fn can_edit(db: &mut PgConnection, user: &AuthUser, doc: &Document) -> ApiResult<bool> {
    if !can_view(db, user, doc).await? {
        return Ok(false);
    }
    if is_manager_all(user) {
        return Ok(true);
    }
    Ok(has(user, PERM_CREATE) && doc.owner_id == Some(user.id))
}

/// 404 when the user may not see it (a hidden document does not exist for them), else the document.
pub async fn viewable(db: &mut PgConnection, user: &AuthUser, id: Uuid) -> ApiResult<Document> {
    let doc = super::load_document(db, id)
        .await?
        .ok_or_else(|| ApiError::http(404, "Document not found"))?;
    if !can_view(db, user, &doc).await? {
        return Err(ApiError::http(404, "Document not found"));
    }
    Ok(doc)
}

/// The document when the user may edit it: 404 when hidden, 403 when only viewable.
pub async fn editable(db: &mut PgConnection, user: &AuthUser, id: Uuid) -> ApiResult<Document> {
    let doc = viewable(db, user, id).await?;
    if !can_edit(db, user, &doc).await? {
        return Err(ApiError::http(
            403,
            "Only the document's owner or a director can change it",
        ));
    }
    Ok(doc)
}

/// 403 unless a doc admin.
pub fn require_doc_admin(user: &AuthUser) -> ApiResult<()> {
    if is_doc_admin(user) {
        Ok(())
    } else {
        Err(ApiError::http(
            403,
            "Only directors and document admins can do this",
        ))
    }
}
