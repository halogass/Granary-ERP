//! 29 Oct 2026 (Owner decisions): `upload_file`, a file an MCP client or Nary's chat puts on a lead, deal, contact or
//! organization (nary::uploads), so propose_customer_message can attach it by its file_id.
//!
//! Over MCP it is listed only at act and full power (mcp_access::ACT_TOOLS): a read or propose connection
//! never stores files.

use granary_core::auth::AuthUser;
use granary_core::errors::ApiResult;
use serde_json::{json, Value};
use sqlx::PgConnection;

use super::handlers::{py_str, truthy};
use super::{handler, Tool};
use crate::nary::uploads;

/// The CRM permissions that let a person add files to a record (attachments_access then checks the record).
pub const UPLOAD_PERMS: &[&str] = &["crm:view_all", "crm:view_own"];

fn err(msg: impl Into<String>) -> ApiResult<Value> {
    Ok(json!({"error": msg.into()}))
}

fn text(args: &Value, k: &str) -> String {
    match args.get(k) {
        Some(v) if truthy(Some(v)) => py_str(v).trim().to_string(),
        _ => String::new(),
    }
}

/// The record's {entity_type, entity_id}: given flat, or as `entity` / `save_to`.
pub fn record_arg(args: &Value, nested: &str) -> (String, String) {
    let inner = args.get(nested).filter(|v| v.is_object());
    let pick = |k: &str| match inner {
        Some(o) => {
            let v = text(o, k);
            if v.is_empty() {
                text(args, k)
            } else {
                v
            }
        }
        None => text(args, k),
    };
    (pick("entity_type"), pick("entity_id"))
}

/// The JSON schema of a target record (upload_file's fields, preview_visual's save_to).
pub fn record_props() -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert(
        "entity_type".into(),
        json!({"type": "string", "enum": ["lead", "deal", "contact", "organization"],
               "description": "The record the file goes on."}),
    );
    m.insert(
        "entity_id".into(),
        json!({"type": "string", "description": "That record's id (search_records, get_whatsapp_chats or the deal give it)."}),
    );
    m
}

pub fn tools() -> Vec<Tool> {
    let mut props = record_props();
    props.insert(
        "file_name".into(),
        json!({"type": "string", "description": "The file's name, e.g. 'site photo.png' (the extension is set from the content)."}),
    );
    props.insert(
        "content_base64".into(),
        json!({"type": "string", "description": "The file's bytes in base64 (a data: URL is fine). Give this OR url."}),
    );
    props.insert(
        "url".into(),
        json!({"type": "string", "description": "A public https address to download the file from. Give this OR content_base64."}),
    );
    props.insert(
        "mime_type".into(),
        json!({"type": "string", "description": "Optional; the type is always judged from the content."}),
    );
    vec![Tool::new(
        "upload_file",
        concat!(
            "Put a file you made or found (a picture you rendered, a PDF, a Word or Excel file, a video) into the ",
            "ERP on a lead, deal, contact or organization. It becomes a normal attachment of that record (its ",
            "Attachments list, marked as uploaded via your AI client or Nary) and is kept whether or not a message is sent. ",
            "Give content_base64 (the bytes in base64) or a public https url, plus file_name; for a large video ",
            "prefer url. Allowed, judged by the content and not the name: PNG, JPEG, WebP or GIF images, PDF, Word ",
            "(.docx), Excel (.xlsx) up to nary.upload.max_mb; MP4 (H.264/AAC plays on WhatsApp), MOV or WebM ",
            "videos up to nary.upload.video_max_mb (not converted; the answer gives the length); never SVG or ",
            "HEIC. Returns file_id: to send the file, pass it to propose_customer_message attach as ",
            "{\"type\": \"file\", \"id\": file_id}. On WhatsApp a picture goes as a photo, an MP4 up to ",
            "nary.wa.video_max_mb as a playable video, a larger video and other files as documents; an email ",
            "attaches files up to nary.attach.max_total_mb in total and sends a larger one as a download link. ",
            "A picture drawn with preview_visual can be stored the same way with its save_to.",
        ),
        json!({"type": "object", "properties": Value::Object(props),
               "required": ["file_name", "entity_type", "entity_id"]}),
        handler!(_h_upload_file),
        UPLOAD_PERMS,
    )]
}

/// `upload_file`.
pub async fn _h_upload_file(
    db: &mut PgConnection,
    user: &AuthUser,
    args: Value,
) -> ApiResult<Value> {
    let (etype, eid) = record_arg(&args, "entity");
    if etype.is_empty() || eid.is_empty() {
        return err("entity_type (lead, deal, contact or organization) and entity_id are required");
    }
    let b64 = text(&args, "content_base64");
    let url = text(&args, "url");
    if b64.is_empty() == url.is_empty() {
        return err("Give the file as content_base64 or as url (one of them)");
    }
    // the record first: no download for a record the person can't add files to
    if let Err(why) = uploads::target(db, user, &etype, &eid).await? {
        return err(why);
    }
    // 1 Nov 2026: read up to the larger cap (videos); store() checks the type's own cap
    let max = match uploads::read_cap(db).await? {
        Ok(m) => m,
        Err(why) => return err(why),
    };
    let (data, url_name) = if url.is_empty() {
        match uploads::decode_base64(&b64, max) {
            Ok(d) => (d, String::new()),
            Err(why) => return err(why),
        }
    } else {
        match uploads::fetch(&url, max).await {
            Ok(x) => x,
            Err(why) => return err(why),
        }
    };
    let mut name = text(&args, "file_name");
    if name.is_empty() {
        name = url_name;
    }
    let via = uploads::via_of(&args);
    let mut out = match uploads::store(db, user, &etype, &eid, &name, data, via).await? {
        Ok(v) => v,
        Err(why) => return err(why),
    };
    let claimed = text(&args, "mime_type").to_ascii_lowercase();
    let got = out["mime_type"].as_str().unwrap_or("").to_string();
    if !claimed.is_empty() && claimed != got && !(claimed == "image/jpg" && got == "image/jpeg") {
        out["warning"] = json!(format!(
            "The content is a {}, not {claimed}: it was stored as what it is.",
            out["type"].as_str().unwrap_or("file")
        ));
    }
    Ok(out)
}
