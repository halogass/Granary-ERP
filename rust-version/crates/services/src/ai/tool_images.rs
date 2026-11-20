//! visuals V3 (21 Oct 2026): pictures in tool results.
//!
//! A tool handler may return images next to its JSON answer under the reserved key [`KEY`]
//! (`"_images": [{"mime": "image/png", "data": "<base64>"}]`), see [`attach`]. Whoever passes the result
//! on takes them out first ([`take`]), so the base64 never reaches the model or the person as text:
//!
//! - MCP (the MCP client, routers/mcp.rs): each image becomes an MCP `image` content item next to the text
//!   item ([`mcp_items`]).
//! - The ERP chat and Nary on WhatsApp (ai/assistant.rs): with a provider that reads images in a tool result
//!   (Anthropic: `tool_result` content blocks) the content is a text block plus one `image` block per picture
//!   ([`for_model`]); otherwise the JSON alone, with a note that a picture was drawn but can't be shown, and
//!   the tool is run with [`VISION_CHECK_ARG`] so its answer carries the vision check's notes instead.

use serde_json::{json, Map, Value};

/// The reserved result key that carries pictures.
pub const KEY: &str = "_images";
/// Set by the chat loop on a tool call when the model can't see pictures in a tool result: a tool that draws
/// one then runs the vision check itself and puts its notes in the text.
pub const VISION_CHECK_ARG: &str = "_vision_check";
/// Tools whose results may carry pictures (the chat loop sets [`VISION_CHECK_ARG`] on them when needed).
pub const IMAGE_TOOLS: &[&str] = &[
    "preview_visual",
    "propose_proposal_visual",
    "propose_proposal_visual_change",
    // visuals3 P3B: the picture going into the proposal
    "propose_proposal_image",
    // 23 Oct: a quotation's or proposal's PDF page
    "render_document_page",
];

/// One picture of a tool result.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolImage {
    pub mime: String,
    /// Standard base64, no data: prefix.
    pub data: String,
}

/// Adds a picture to a tool result (an object; anything else is left alone).
pub fn attach(out: &mut Value, mime: &str, bytes: &[u8]) {
    use base64::Engine as _;
    let Some(m) = out.as_object_mut() else {
        return;
    };
    let item = json!({"mime": mime,
                      "data": base64::engine::general_purpose::STANDARD.encode(bytes)});
    match m.get_mut(KEY) {
        Some(Value::Array(a)) => a.push(item),
        _ => {
            m.insert(KEY.into(), Value::Array(vec![item]));
        }
    }
}

/// Takes the pictures out of a tool result (the key is removed, whatever it held).
pub fn take(out: &mut Value) -> Vec<ToolImage> {
    let Some(m) = out.as_object_mut() else {
        return vec![];
    };
    let Some(Value::Array(items)) = m.remove(KEY) else {
        return vec![];
    };
    items
        .into_iter()
        .filter_map(|i| {
            let mime = i.get("mime").and_then(Value::as_str)?.to_string();
            let data = i.get("data").and_then(Value::as_str)?.to_string();
            (mime.starts_with("image/") && !data.is_empty()).then_some(ToolImage { mime, data })
        })
        .collect()
}

/// The MCP `image` content items of these pictures.
pub fn mcp_items(images: &[ToolImage]) -> Vec<Value> {
    images
        .iter()
        .map(|i| json!({"type": "image", "data": i.data, "mimeType": i.mime}))
        .collect()
}

/// Whether this provider reads pictures inside a tool result.
pub fn provider_sees_images(provider_name: &str) -> bool {
    provider_name == "anthropic"
}

/// What the chat loop puts in a tool message's `content`: the result as is when it has no pictures; with
/// pictures, content blocks (text + images) for a provider that reads them, else the result with a note.
pub fn for_model(mut out: Value, provider_name: &str) -> Value {
    let images = take(&mut out);
    if images.is_empty() {
        return out;
    }
    if provider_sees_images(provider_name) {
        let text = match &out {
            Value::String(s) => s.clone(),
            other => crate::ai::base::sdk::py_dumps_unicode(other),
        };
        let mut blocks = vec![json!({"type": "text", "text": text})];
        for i in &images {
            blocks.push(json!({"type": "image",
                               "source": {"type": "base64", "media_type": i.mime, "data": i.data}}));
        }
        return Value::Array(blocks);
    }
    if let Value::Object(m) = &mut out {
        m.insert(
            "image_note".into(),
            json!(format!(
                "{} picture(s) were drawn but this model can't see them; rely on the warnings and the vision \
                 check notes above.",
                images.len()
            )),
        );
    }
    out
}

/// The tool call's arguments as the chat loop runs them: [`VISION_CHECK_ARG`] set for a picture tool when the
/// model can't see pictures.
pub fn args_for(tool: &str, args: Value, provider_name: &str) -> Value {
    if provider_sees_images(provider_name) || !IMAGE_TOOLS.contains(&tool) {
        return args;
    }
    let mut m = match args {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    m.insert(VISION_CHECK_ARG.into(), json!(true));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_take_and_mcp_items() {
        let mut out = json!({"ok": true});
        attach(&mut out, "image/png", b"\x89PNG");
        assert_eq!(out[KEY][0]["data"], json!("iVBORw=="));
        let imgs = take(&mut out);
        assert_eq!(out, json!({"ok": true}));
        assert_eq!(
            mcp_items(&imgs),
            vec![json!({"type": "image", "data": "iVBORw==", "mimeType": "image/png"})]
        );
    }

    #[test]
    fn for_model_by_provider() {
        let mut out = json!({"ok": true});
        attach(&mut out, "image/png", b"x");
        let a = for_model(out.clone(), "anthropic");
        assert_eq!(a[0]["type"], json!("text"));
        assert_eq!(a[1]["source"]["media_type"], json!("image/png"));
        let o = for_model(out, "openai");
        assert!(o.get(KEY).is_none() && o.get("image_note").is_some());
        assert_eq!(
            args_for("preview_visual", json!({}), "openai")[VISION_CHECK_ARG],
            json!(true)
        );
        assert!(args_for("preview_visual", json!({}), "anthropic")
            .get(VISION_CHECK_ARG)
            .is_none());
    }
}
