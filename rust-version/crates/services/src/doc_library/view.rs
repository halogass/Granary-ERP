//! The JSON the library screens get (list items and the detail page).

use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;

use super::{access, Document, Version};

/// A version in short (the list's current_version).
pub fn version_short(v: &Version) -> Value {
    json!({
        "id": v.id,
        "label": v.label,
        "change_note": v.change_note,
        "status": v.status,
        "went_live_at": v.went_live_at,
        "file_name": v.file_name,
        "file_mime": v.file_mime,
        "file_size": v.file_size,
        "page_count": v.page_count,
        "duration_seconds": v.duration_seconds,
    })
}

/// A version in full (the history of doc admins).
pub async fn version_full(db: &mut PgConnection, v: &Version) -> ApiResult<Value> {
    let names: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM users WHERE id = ANY($1)")
            .bind(
                [v.uploaded_by, v.approved_by]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>(),
            )
            .fetch_all(&mut *db)
            .await?;
    let who = |id: Option<Uuid>| -> Value {
        match id {
            None => Value::Null,
            Some(id) => json!({
                "id": id,
                "name": names.iter().find(|(u, _)| *u == id).map(|(_, n)| n.clone()),
            }),
        }
    };
    Ok(json!({
        "id": v.id,
        "label": v.label,
        "change_note": v.change_note,
        "status": v.status,
        "file_name": v.file_name,
        "file_mime": v.file_mime,
        "file_size": v.file_size,
        "page_count": v.page_count,
        "duration_seconds": v.duration_seconds,
        "has_source": v.source_object_key.is_some(),
        "source_file_name": v.source_file_name,
        "source_size": v.source_size,
        "uploaded_by": who(v.uploaded_by),
        "approved_by": who(v.approved_by),
        "approved_at": v.approved_at,
        "rejected_reason": v.rejected_reason,
        "went_live_at": v.went_live_at,
        "created_at": v.created_at,
    }))
}

/// One list item (see the brief's item shape). `user` decides can_edit.
pub async fn item(
    db: &mut PgConnection,
    user: Option<&AuthUser>,
    d: &Document,
) -> ApiResult<Value> {
    let cat: Option<(String, String)> =
        sqlx::query_as("SELECT key, name FROM doc_categories WHERE id = $1")
            .bind(d.category_id)
            .fetch_optional(&mut *db)
            .await?;
    let owner: Option<(Uuid, String)> = match d.owner_id {
        Some(o) => {
            sqlx::query_as("SELECT id, name FROM users WHERE id = $1")
                .bind(o)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let variants: Vec<(Uuid, String, String)> = match d.language_group_id {
        Some(g) => sqlx::query_as(
            "SELECT id, language, title FROM library_documents WHERE language_group_id = $1 AND id <> $2 \
             AND is_deleted = false ORDER BY language, title",
        )
        .bind(g)
        .bind(d.id)
        .fetch_all(&mut *db)
        .await?,
        None => vec![],
    };
    let products: Vec<(Option<Uuid>, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as(
            "SELECT lp.product_id, pc.sku, pc.name, lp.family FROM library_document_products lp \
           LEFT JOIN product_catalog pc ON pc.id = lp.product_id WHERE lp.document_id = $1 \
          ORDER BY lp.family NULLS FIRST, pc.sku",
        )
        .bind(d.id)
        .fetch_all(&mut *db)
        .await?;
    let current = super::current_live(db, d.id).await?;
    let pending = super::pending_version(db, d.id).await?;
    let today: chrono::NaiveDate = sqlx::query_scalar("SELECT CURRENT_DATE")
        .fetch_one(&mut *db)
        .await?;
    let can_edit = match user {
        Some(u) => access::can_edit(db, u, d).await?,
        None => false,
    };
    Ok(json!({
        "id": d.id,
        "title": d.title,
        "kind": d.kind,
        "category": cat.map(|(k, n)| json!({"id": d.category_id, "key": k, "name": n})),
        "audiences": d.audiences,
        "owner": owner.map(|(id, name)| json!({"id": id, "name": name})),
        "language": d.language,
        "language_group_id": d.language_group_id,
        "language_variants": variants.iter().map(|(id, l, t)| json!({"id": id, "language": l, "title": t})).collect::<Vec<_>>(),
        "products": products.iter().map(|(id, sku, name, fam)| match id {
            Some(id) => json!({"id": id, "sku": sku, "name": name}),
            None => json!({"family": fam}),
        }).collect::<Vec<_>>(),
        "review_by": d.review_by,
        "needs_review": d.review_by.is_some_and(|r| r <= today),
        "current_version": current.as_ref().map(version_short),
        "pending_version": pending.as_ref().map(|p| json!({"id": p.id, "label": p.label, "change_note": p.change_note, "created_at": p.created_at})),
        "can_edit": can_edit,
        "updated_at": d.updated_at,
        "created_at": d.created_at,
    }))
}

/// The detail page: the item + description, linked articles, versions (doc admins only, else null) and
/// what the user may do.
pub async fn detail(db: &mut PgConnection, user: &AuthUser, d: &Document) -> ApiResult<Value> {
    let mut v = item(db, Some(user), d).await?;
    let articles: Vec<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        "SELECT a.id, a.title, a.slug, a.status FROM library_article_links l JOIN kb_articles a ON a.id = l.article_id \
          WHERE l.document_id = $1 AND a.is_deleted = false ORDER BY a.title",
    )
    .bind(d.id)
    .fetch_all(&mut *db)
    .await?;
    let admin = access::is_doc_admin(user);
    let versions = if admin {
        let mut out = Vec::new();
        for ver in super::versions_of(db, d.id).await? {
            out.push(version_full(db, &ver).await?);
        }
        Value::Array(out)
    } else {
        Value::Null
    };
    let can_edit = v["can_edit"].as_bool().unwrap_or(false);
    let o = v.as_object_mut().expect("an object");
    o.insert("description".into(), json!(d.description));
    o.insert(
        "articles".into(),
        json!(articles
            .iter()
            .map(|(id, t, s, st)| json!({"id": id, "title": t, "slug": s, "status": st}))
            .collect::<Vec<_>>()),
    );
    o.insert("versions".into(), versions);
    o.insert(
        "permissions".into(),
        json!({
            "edit": can_edit,
            "upload": can_edit,
            "approve": access::can_approve(user),
            "history": admin,
            "restore": admin,
            "reassign": admin,
            "trace": admin,
            "delete": access::is_manager_all(user),
            "download_source": can_edit,
        }),
    );
    Ok(v)
}
