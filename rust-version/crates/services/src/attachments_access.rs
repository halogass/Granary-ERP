//! Port of backend/app/services/attachments_access.py: who may see a given attachment.
//!
//! Attachments are polymorphic (`entity_type`, `entity_id`), so "may I read this file?" is really "may I
//! read the record it hangs off?". Every surface calls [`assert_can_access`]: the attachment endpoints, the
//! project / R&D / WhatsApp attachment routes and the AI document analyser.
//!
//! Each rule mirrors the record's own visibility:
//!
//!   crm_org/lead/deal/contact  the owner scoping of `crm_records._assert_can_view`
//!   project                    `projects:view_all`, else PM / tech lead / creator / team member
//!   rnd_project                `rnd:view` or `rnd:manage`, else the lead engineer
//!   contract                   `sales:view_contracts` or `sales:create_contract`
//!   kb_article                 the *list* rule in routers/knowledge.py: access_level +
//!                              `kb:view_restricted`, plus private-to-its-creator

use std::collections::HashSet;

use granary_compat::text::utils::py_uuid;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use sqlx::PgConnection;
use uuid::Uuid;

/// Attachment entity_type -> table. The strings are the attachment router's own vocabulary and do NOT
/// match the CRM `OBJECT_TYPES`. Every one of these models has `is_deleted` (SoftDeleteMixin), so `_load`
/// always filters deleted rows out.
const MODELS: [(&str, &str); 9] = [
    ("crm_org", "organizations"),
    ("crm_lead", "leads"),
    ("crm_deal", "deals"),
    ("project", "projects"),
    ("rnd_project", "rnd_projects"),
    ("contract", "contracts"),
    ("kb_article", "kb_articles"),
    // batch 4 D2: product files on catalog items
    ("catalog_product", "product_catalog"),
    // 29 Oct 2026: a contact's own files (upload_file, the contact page)
    ("crm_contact", "contacts"),
];

/// `_DENIED`, `_forbidden()`'s default text (every rule passes its own).
pub const DENIED: &str = "You don't have access to the record this file belongs to";

fn forbidden(detail: &str) -> ApiError {
    ApiError::http(403, detail)
}

/// The columns the rules read, from whichever table the attachment hangs off.
#[derive(Debug, Clone, Default)]
pub struct AttachedRecord {
    pub entity_type: String,
    pub id: Uuid,
    /// crm_*: `assigned_to_id`
    pub assigned_to_id: Option<Uuid>,
    /// project
    pub pm_id: Option<Uuid>,
    pub tech_lead_id: Option<Uuid>,
    /// project, kb_article
    pub created_by_id: Option<Uuid>,
    /// rnd_project
    pub lead_engineer_id: Option<Uuid>,
    /// kb_article
    pub is_private: Option<bool>,
    pub access_level: Option<String>,
}

fn model(entity_type: &str) -> Option<&'static str> {
    MODELS
        .iter()
        .find(|(k, _)| *k == entity_type)
        .map(|(_, t)| *t)
}

async fn _load(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: &str,
) -> ApiResult<AttachedRecord> {
    let table = model(entity_type).expect("caller checked the entity type");
    let uid = py_uuid(entity_id).ok_or_else(|| ApiError::http(404, "Record not found"))?;
    let cols = match entity_type {
        "crm_org" | "crm_lead" | "crm_deal" | "crm_contact" => {
            "id, assigned_to_id, NULL::uuid, NULL::uuid, NULL::uuid, NULL::uuid, NULL::boolean, NULL::text"
        }
        "project" => "id, NULL::uuid, pm_id, tech_lead_id, created_by_id, NULL::uuid, NULL::boolean, NULL::text",
        "rnd_project" => {
            "id, NULL::uuid, NULL::uuid, NULL::uuid, NULL::uuid, lead_engineer_id, NULL::boolean, NULL::text"
        }
        "contract" | "catalog_product" => {
            "id, NULL::uuid, NULL::uuid, NULL::uuid, NULL::uuid, NULL::uuid, NULL::boolean, NULL::text"
        }
        _ => "id, NULL::uuid, NULL::uuid, NULL::uuid, created_by_id, NULL::uuid, is_private, access_level::text",
    };
    type Row = (
        Uuid,
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
        Option<bool>,
        Option<String>,
    );
    let row: Option<Row> = sqlx::query_as(&format!(
        "SELECT {cols} FROM {table} WHERE id = $1 AND is_deleted = false LIMIT 1"
    ))
    .bind(uid)
    .fetch_optional(&mut *db)
    .await?;
    let r = row.ok_or_else(|| ApiError::http(404, "Record not found"))?;
    Ok(AttachedRecord {
        entity_type: entity_type.to_string(),
        id: r.0,
        assigned_to_id: r.1,
        pm_id: r.2,
        tech_lead_id: r.3,
        created_by_id: r.4,
        lead_engineer_id: r.5,
        is_private: r.6,
        access_level: r.7,
    })
}

// ─── per-entity rules ────────────────────────────────────────────────────────

/// Owner scoping, the same rule as `crm_records._assert_can_view`.
fn _check_crm(user: &AuthUser, rec: &AttachedRecord, perms: &HashSet<String>) -> ApiResult<()> {
    if perms.contains("crm:view_all") {
        return Ok(());
    }
    if !perms.contains("crm:view_own") {
        return Err(forbidden("Permission required: crm:view_own"));
    }
    if let Some(owner) = rec.assigned_to_id {
        if owner != user.id {
            return Err(forbidden("This record is assigned to someone else"));
        }
    }
    Ok(())
}

/// `projects:view_all`, else the people actually on the project.
async fn _check_project(
    db: &mut PgConnection,
    user: &AuthUser,
    rec: &AttachedRecord,
    perms: &HashSet<String>,
) -> ApiResult<()> {
    if perms.contains("projects:view_all") {
        return Ok(());
    }
    if [rec.pm_id, rec.tech_lead_id, rec.created_by_id].contains(&Some(user.id)) {
        return Ok(());
    }
    let is_member: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM project_members WHERE project_id = $1 AND user_id = $2 LIMIT 1",
    )
    .bind(rec.id)
    .bind(user.id)
    .fetch_optional(&mut *db)
    .await?;
    if is_member.is_none() {
        return Err(forbidden("You are not on this project"));
    }
    Ok(())
}

fn _check_rnd_project(
    user: &AuthUser,
    rec: &AttachedRecord,
    perms: &HashSet<String>,
) -> ApiResult<()> {
    if perms.contains("rnd:view") || perms.contains("rnd:manage") {
        return Ok(());
    }
    if rec.lead_engineer_id == Some(user.id) {
        return Ok(());
    }
    Err(forbidden("Permission required: rnd:view"))
}

/// Flat permission, matching `GET /sales/contracts/{id}`.
fn _check_contract(perms: &HashSet<String>) -> ApiResult<()> {
    if !(perms.contains("sales:view_contracts") || perms.contains("sales:create_contract")) {
        return Err(forbidden("Permission required: sales:view_contracts"));
    }
    Ok(())
}

// batch 4 D2: product files follow the catalog's read rule (and Nary's approver, who sends them).
fn _check_catalog_product(perms: &HashSet<String>) -> ApiResult<()> {
    if !(perms.contains("sales:view_catalog")
        || perms.contains("sales:manage_catalog")
        || perms.contains("nary:approve"))
    {
        return Err(forbidden("Permission required: sales:view_catalog"));
    }
    Ok(())
}

/// The *list* rule in routers/knowledge.py, stricter than the single-article GET.
fn _check_kb_article(
    user: &AuthUser,
    rec: &AttachedRecord,
    perms: &HashSet<String>,
) -> ApiResult<()> {
    // Batch E: the knowledge base read rule (kb:view, or an editing permission that includes reading).
    if !["kb:view", "kb:create", "kb:publish", "kb:manage_sops"]
        .iter()
        .any(|p| perms.contains(*p))
    {
        return Err(forbidden("Permission required: kb:view"));
    }
    if rec.is_private == Some(true) && rec.created_by_id != Some(user.id) {
        return Err(forbidden("This article is private"));
    }
    let level = match rec.access_level.as_deref() {
        Some(l) if !l.is_empty() => l,
        _ => "all_staff",
    };
    if !matches!(level, "all_staff" | "department") && !perms.contains("kb:view_restricted") {
        return Err(forbidden("Permission required: kb:view_restricted"));
    }
    Ok(())
}

/// Raise unless `user` may read the record this attachment hangs off. Returns the record (None for a
/// superadmin asking about an entity type without a rule, as Python).
///
/// Superadmins pass, as everywhere else. Portal users never do. `user.perms` is the request's
/// permission set (`_collect_permissions(user)`).
pub async fn assert_can_access(
    db: &mut PgConnection,
    user: &AuthUser,
    entity_type: &str,
    entity_id: &str,
) -> ApiResult<Option<AttachedRecord>> {
    if user.is_superadmin {
        return if model(entity_type).is_some() {
            Ok(Some(_load(db, entity_type, entity_id).await?))
        } else {
            Ok(None)
        };
    }

    if !user.is_internal() {
        return Err(forbidden("Internal users only"));
    }

    if model(entity_type).is_none() {
        // An entity_type nobody has a rule for is refused rather than waved through.
        return Err(forbidden(&format!(
            "Attachments on '{entity_type}' are not readable through this API"
        )));
    }

    let rec = _load(db, entity_type, entity_id).await?;
    let perms = &user.perms;
    match entity_type {
        "crm_org" | "crm_lead" | "crm_deal" | "crm_contact" => _check_crm(user, &rec, perms)?,
        "project" => _check_project(db, user, &rec, perms).await?,
        "rnd_project" => _check_rnd_project(user, &rec, perms)?,
        "contract" => _check_contract(perms)?,
        "catalog_product" => _check_catalog_product(perms)?,
        _ => _check_kb_article(user, &rec, perms)?,
    }
    Ok(Some(rec))
}
