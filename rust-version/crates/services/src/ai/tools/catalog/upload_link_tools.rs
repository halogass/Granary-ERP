//! 5 Nov 2026 (Owner-approved plan, Stream B): one-time upload links (crate::upload_links), for files that are in
//! an AI chat and must reach the ERP: the MCP client can't upload over HTTP and a video in base64 is
//! impractical, so the AI client makes a link and the Owner drops the files on its page.
//!
//! create_upload_link, get_upload_link, revoke_upload_link: same permissions as upload_file; over MCP only at
//! act and full power (mcp_access::ACT_TOOLS).

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use serde_json::{json, Value};
use sqlx::PgConnection;

use super::handlers::{py_str, truthy};
use super::upload_tools::{record_arg, UPLOAD_PERMS};
use super::{handler, Tool};
use crate::upload_links;

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn text(args: &Value, k: &str) -> String {
    match args.get(k) {
        Some(v) if truthy(Some(v)) => py_str(v).trim().to_string(),
        _ => String::new(),
    }
}

fn link_id_schema() -> Value {
    json!({"type": "object", "properties": {
        "link_id": {"type": "string", "description": "The link_id create_upload_link gave."}},
        "required": ["link_id"]})
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "create_upload_link",
            concat!(
                "Make a one-time upload link for files that are in this AI chat (a video, photos, a PDF the ",
                "Owner gave you) and must go into the ERP. You can't upload them yourself from a chat, and you must ",
                "never base64 a video: create a link and give the Owner its page_url; they open it (phone or ",
                "computer) and drop the files. A client that can make HTTP requests (curl, a coding agent) may PUT ",
                "the raw bytes to upload_url instead (how_to.curl). entity_type lead, deal, contact or organization ",
                "with its entity_id: each file becomes an attachment of that record, uploaded by you; staging (no ",
                "entity_id): the files wait, for a customer message. The link works files.upload_link.minutes and ",
                "takes files.upload_link.max_files files; allowed and capped as upload_file (images, PDF, Word, ",
                "Excel up to nary.upload.max_mb; MP4, MOV, WebM up to nary.upload.video_max_mb). When the Owner says ",
                "the files are up, call get_upload_link: its files carry the file_ids for propose_customer_message ",
                "attach {\"type\": \"file\", \"id\": file_id}.",
            ),
            json!({"type": "object", "properties": {
                "entity_type": {"type": "string", "enum": ["lead", "deal", "contact", "organization", "staging"],
                                "description": "The record the files go on, or staging (no record yet)."},
                "entity_id": {"type": "string", "description": "That record's id; leave out for staging."},
                "note": {"type": "string", "description": "Optional: what the files are for (kept on the link)."}},
                "required": ["entity_type"]}),
            handler!(_h_create_upload_link),
            UPLOAD_PERMS,
        ),
        Tool::new(
            "get_upload_link",
            concat!(
                "The files an upload link (create_upload_link) has taken so far, with their file_id, name, size, ",
                "type and a video's length, and whether the link is still open. Pass a file_id to ",
                "propose_customer_message attach as {\"type\": \"file\", \"id\": file_id}.",
            ),
            link_id_schema(),
            handler!(_h_get_upload_link),
            UPLOAD_PERMS,
        ),
        Tool::new(
            "revoke_upload_link",
            "Close an upload link so it takes no more files (the files it took stay). Use when the Owner is done or the link was shared by mistake.",
            link_id_schema(),
            handler!(_h_revoke_upload_link),
            UPLOAD_PERMS,
        ),
    ]
}

/// `create_upload_link`.
pub async fn _h_create_upload_link(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let (etype, eid) = record_arg(&args, "entity");
    // over MCP the router marks the call (_via claude): the audit says mcp, else nary
    let origin = if crate::nary::uploads::via_of(&args) == "mcp" {
        "mcp"
    } else {
        "nary"
    };
    if etype.is_empty() {
        return err("entity_type (lead, deal, contact, organization or staging) is required");
    }
    match upload_links::create(db, user, &etype, &eid, &text(&args, "note"), origin).await? {
        Ok(v) => Ok(v),
        Err(why) => err(why),
    }
}

/// `get_upload_link`.
pub async fn _h_get_upload_link(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    match upload_links::get(db, user, &text(&args, "link_id")).await? {
        Ok(v) => Ok(v),
        Err(why) => err(why),
    }
}

/// `revoke_upload_link`.
pub async fn _h_revoke_upload_link(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    match upload_links::revoke(db, user, &text(&args, "link_id")).await? {
        Ok(v) => Ok(v),
        Err(why) => err(why),
    }
}
