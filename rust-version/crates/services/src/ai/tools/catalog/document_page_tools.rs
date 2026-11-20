//! 23 Oct 2026: `render_document_page`, a page of a quotation's or a Solution Proposal's PDF as a picture, for
//! Nary in the ERP chat and on WhatsApp and for the MCP client, so the layout, the letterhead or a figure
//! can be checked before anything is sent (Owner: yes, R&D backlog 23 Oct).
//!
//! - which "sent" (default): the latest active verified copy (document_verifications), exactly what the
//!   customer got; when none was issued yet, the draft, and the answer says so.
//! - which "draft": built as it stands now (documents::render::draft_job, the proposal preview's builder),
//!   "Draft, not for distribution" in the footer, nothing issued.
//!
//! Read only, sales:view_quotations. The page goes through nary::proposal_images::picture_of and comes back as
//! a JPEG (tool_images: an image item over MCP, an image block for a model that reads them).

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::proposal_image_tools::pictures_at;
use super::proposal_tools::{find_document, int_arg, text};
use super::{handler, Tool};
use crate::approvals::adapters::get_adapter;

/// The page picture's longest side in pixels (what Claude reads without scaling it down).
const PAGE_EDGE: u32 = 1568;

/// The tools of this module (appended to ALL_TOOLS by defs.rs).
pub fn tools() -> Vec<Tool> {
    vec![Tool::new(
        "render_document_page",
        "See one page of a quotation's or a Solution Proposal's PDF as a picture, to check its layout, the \
         letterhead, a table or a figure. which sent (default) shows the latest verified copy, exactly what the \
         customer got (when nothing was issued yet, the draft); which draft builds the document as it stands now \
         without issuing anything. Gives the page count; ask for another page with page. Read only.",
        json!({
            "type": "object",
            "properties": {
                "type": {"type": "string", "enum": ["quotation", "sales_document"],
                         "description": "quotation, or sales_document for a Solution Proposal."},
                "id": {"type": "string", "description": "The document's id or number."},
                "page": {"type": "integer", "description": "The page to show, 1 = first (default 1)."},
                "which": {"type": "string", "enum": ["sent", "draft"],
                          "description": "sent: the verified copy the customer got (default); draft: as it stands now."},
            },
            "required": ["type", "id"],
        }),
        handler!(_h_render_document_page),
        &["sales:view_quotations"],
    )]
}

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

/// The document's id from its id or number.
async fn resolve(db: &mut PgConnection, kind: &str, raw: &str) -> ApiResult<Option<Uuid>> {
    if kind == "sales_document" {
        return Ok(find_document(db, raw).await?.map(|d| d.id));
    }
    if let Ok(id) = Uuid::parse_str(raw.trim()) {
        return Ok(Some(id));
    }
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM quotations WHERE (full_number = $1 OR number = $1) AND is_deleted = false \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(raw.trim())
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|r| r.0))
}

/// `render_document_page`.
pub async fn _h_render_document_page(
    db: &mut PgConnection,
    _user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let kind = text(&args, "type").unwrap_or_default();
    if !matches!(kind.as_str(), "quotation" | "sales_document") {
        return err("type is quotation or sales_document");
    }
    let Some(raw) = text(&args, "id") else {
        return err("id is required (the document's id or number)");
    };
    let which = text(&args, "which").unwrap_or_else(|| "sent".into());
    if !matches!(which.as_str(), "sent" | "draft") {
        return err("which is sent or draft");
    }
    let page = int_arg(&args, "page").unwrap_or(1);
    if page < 1 {
        return err("page starts at 1");
    }
    let adapter = get_adapter(&kind)?;
    let label = adapter.label;
    let entity = match resolve(db, &kind, &raw).await? {
        Some(id) => crate::verification::render::load(db, &kind, &id.to_string()).await?,
        None => None,
    };
    let Some(entity) = entity else {
        return err(format!(
            "No {} with id or number {raw}",
            label.to_lowercase()
        ));
    };
    let number = adapter.number(&entity);

    // the verified copy the customer got, unless the draft was asked for
    let mut shown = which.clone();
    let mut note: Option<String> = None;
    let mut issued: Option<Value> = None;
    let mut bytes: Option<Vec<u8>> = None;
    if which == "sent" {
        let latest: Option<(String, String, chrono::DateTime<chrono::Utc>, i32)> = sqlx::query_as(
            "SELECT pdf_object_key, short_code, issued_at, revision FROM document_verifications \
             WHERE entity_type = $1 AND entity_id = $2 AND status = 'active' \
               AND coalesce(pdf_object_key, '') <> '' \
             ORDER BY issued_at DESC LIMIT 1",
        )
        .bind(&kind)
        .bind(entity.id())
        .fetch_optional(&mut *db)
        .await?;
        match latest {
            Some((key, code, at, revision)) => {
                match crate::verification::service::get_pdf(&key).await {
                    Ok(b) => {
                        bytes = Some(b);
                        issued = Some(json!({"verification_code": code,
                                             "issued_at": at.to_rfc3339(), "revision": revision}));
                    }
                    Err(e) => {
                        tracing::warn!(key = %key, error = %e, "render_document_page: stored pdf missing");
                        note = Some(
                            "The verified copy could not be read from storage; this is the draft as it \
                             stands now."
                                .into(),
                        );
                        shown = "draft".into();
                    }
                }
            }
            None => {
                note = Some(format!(
                    "{label} {number} has no verified copy yet (nothing was issued or sent): this is the draft \
                     as it stands now."
                ));
                shown = "draft".into();
            }
        }
    }
    let bytes = match bytes {
        Some(b) => b,
        None => {
            let job = crate::documents::render::draft_job(db, &kind, &entity).await?;
            match job.render().await {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!(error = ?e, "render_document_page: draft render failed");
                    return err(format!(
                        "The draft of {label} {number} could not be drawn (the PDF service did not answer); \
                         try again shortly."
                    ));
                }
            }
        }
    };
    let (pic, thumb) = match pictures_at(bytes, Some(page), PAGE_EDGE).await? {
        Ok(p) => p,
        Err(e) => return err(format!("{label} {number}: {e}")),
    };
    let mut out = json!({
        "type": kind, "id": entity.id().to_string(), "number": number,
        "which": shown, "page": pic.page.unwrap_or(page as usize), "pages": pic.pages,
        "picture": format!(
            "Page {} of {} of {label} {number} ({}) is attached as a picture.",
            pic.page.unwrap_or(page as usize),
            pic.pages.unwrap_or(1),
            if shown == "sent" { "the verified copy the customer got" } else { "draft, nothing issued" }
        ),
    });
    if let Some(v) = issued {
        out["verified_copy"] = v;
    }
    if let Some(n) = note {
        out["note"] = json!(n);
    }
    crate::ai::tool_images::attach(&mut out, "image/jpeg", &thumb);
    Ok(out)
}
