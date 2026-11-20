//! Library notifications.
//!
//! - A new live version: one bell notification (no email) to every active internal user who can view the
//!   document's category (documents:view, the category's roles; directors and doc admins see all), except the
//!   person who uploaded it.
//! - The daily review reminder (worker beat): a document whose review_by is within
//!   documents.review_reminder_days or past gets one reminder to its owner, at most once every 7 days
//!   (library_documents.last_reminded_at). Never blocks anything.

use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::errors::ApiResult;
use granary_core::settings_store::require_int;

use super::{Document, Version, WHERE};
use crate::approvals::notify::{notify, Notice};

/// The bell type of library notices.
pub const TYPE: &str = "document_library";

/// The link of a document page in the ERP.
pub fn doc_link(id: Uuid) -> String {
    format!("/knowledge/documents/{id}")
}

/// Active internal users who may view documents of this category.
pub async fn viewers_of_category(db: &mut PgConnection, category_id: Uuid) -> ApiResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT u.id FROM users u \
          WHERE u.is_active = true AND (u.portal_type = 'internal' OR u.portal_type IS NULL OR u.portal_type = '') \
            AND (u.is_superadmin = true OR u.is_system_admin = true \
                 OR EXISTS (SELECT 1 FROM user_roles ur JOIN role_permissions rpm ON rpm.role_id = ur.role_id \
                            JOIN permissions p ON p.id = rpm.permission_id \
                            WHERE ur.user_id = u.id AND p.code IN ('documents:manage_all', 'documents:admin')) \
                 OR EXISTS (SELECT 1 FROM user_permissions up JOIN permissions p ON p.id = up.permission_id \
                            WHERE up.user_id = u.id AND p.code IN ('documents:manage_all', 'documents:admin')) \
                 OR ((EXISTS (SELECT 1 FROM user_roles ur JOIN role_permissions rpm ON rpm.role_id = ur.role_id \
                              JOIN permissions p ON p.id = rpm.permission_id \
                              WHERE ur.user_id = u.id AND p.code = 'documents:view') \
                      OR EXISTS (SELECT 1 FROM user_permissions up JOIN permissions p ON p.id = up.permission_id \
                                 WHERE up.user_id = u.id AND p.code = 'documents:view')) \
                     AND EXISTS (SELECT 1 FROM doc_categories c WHERE c.id = $1 \
                                 AND (cardinality(c.visible_roles) = 0 \
                                      OR EXISTS (SELECT 1 FROM user_roles ur JOIN roles r ON r.id = ur.role_id \
                                                 WHERE ur.user_id = u.id AND r.slug = ANY(c.visible_roles)))))) \
          ORDER BY u.id",
    )
    .bind(category_id)
    .fetch_all(&mut *db)
    .await?)
}

/// A version went live: tell the staff who can see it (one bell each, no email).
pub async fn new_live_version(db: &mut PgConnection, doc: &Document, v: &Version) -> ApiResult<()> {
    let mut ids = viewers_of_category(db, doc.category_id).await?;
    ids.retain(|id| Some(*id) != v.uploaded_by);
    if ids.is_empty() {
        return Ok(());
    }
    let title = format!("New version of {}: {}", doc.title, v.label);
    let body = v.change_note.clone();
    let link = doc_link(doc.id);
    let id_s = doc.id.to_string();
    notify(
        db,
        ids,
        Notice::new(&title, &body, &link, "library_document", &id_s)
            .priority("low")
            .email(false)
            .type_(TYPE),
    )
    .await
}

/// The daily review reminder (see the module doc). Returns how many owners were reminded.
pub async fn review_reminders(db: &mut PgConnection) -> ApiResult<usize> {
    let days = require_int(db, "documents.review_reminder_days", WHERE)
        .await?
        .max(0);
    let due: Vec<(Uuid, String, Uuid, chrono::NaiveDate)> = sqlx::query_as(
        "SELECT d.id, d.title, d.owner_id, d.review_by FROM library_documents d \
           JOIN users u ON u.id = d.owner_id AND u.is_active = true \
          WHERE d.is_deleted = false AND d.review_by IS NOT NULL \
            AND d.review_by <= CURRENT_DATE + make_interval(days => $1::int) \
            AND (d.last_reminded_at IS NULL OR d.last_reminded_at < now() - interval '7 days') \
          ORDER BY d.review_by, d.id",
    )
    .bind(days as i32)
    .fetch_all(&mut *db)
    .await?;
    let today: chrono::NaiveDate = sqlx::query_scalar("SELECT CURRENT_DATE")
        .fetch_one(&mut *db)
        .await?;
    for (id, title, owner, review_by) in &due {
        let when = review_by.format("%-d %b %Y").to_string();
        let (t, body) = if *review_by < today {
            (
                format!("Review overdue: {title}"),
                format!("{title} was due for review on {when}. Check it is still right, then upload a new version or set a new review date."),
            )
        } else {
            (
                format!("Review due: {title}"),
                format!("{title} is due for review on {when}. Check it is still right, then upload a new version or set a new review date."),
            )
        };
        let link = doc_link(*id);
        let id_s = id.to_string();
        notify(
            db,
            [*owner],
            Notice::new(&t, &body, &link, "library_document", &id_s)
                .priority("normal")
                .email(true)
                .type_(TYPE),
        )
        .await?;
        sqlx::query("UPDATE library_documents SET last_reminded_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *db)
            .await?;
    }
    Ok(due.len())
}
