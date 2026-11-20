//! Port of backend/app/services/nary/proposal_edits.py (agent N6).
//!
//! "Ask Nary to change…" on a proposal (Nary Phase 4): Opus (nary.proposal_edit) rewrites only the sections
//! the instruction touches; the editor shows the result and the Owner keeps editing. The same guards as the
//! draft: no accuracy figure survives, no price is typed (the commercial section stays generated).

use granary_compat::pyjson::{self, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::documents::SalesDocument;
use once_cell::sync::Lazy;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;

use super::negotiation::dumps_unicode;
use super::proposals::strip_accuracy;
use crate::documents::core_ as core;

pub static SCHEMA: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(r#"{"type": "object", "properties": {"changed_sections": {"type": "array", "items": {"type": "object", "properties": {"key": {"type": "string"}, "blocks": {"type": "array", "items": {"type": "object", "properties": {"block_type": {"type": "string", "enum": ["text", "kv", "table", "coverage", "image"]}, "data": {"type": "object"}}, "required": ["block_type", "data"], "additionalProperties": false}}}, "required": ["key", "blocks"], "additionalProperties": false}}, "summary": {"type": "string"}}, "required": ["changed_sections", "summary"], "additionalProperties": false}"#).unwrap()
});

pub const EDIT_RULES: &str =
    "Edit a Solution Proposal as instructed. Return only the sections you change, \
each with ALL its blocks in order (unchanged blocks of that section included). \
Keep the language and the house style. Never state an accuracy figure or a price. \
Image blocks keep their asset_id. summary: one line on what changed.";

// visuals V2: figures (chart / diagram blocks) in "Ask Nary to change...".
/// The editor's figure form: `Change only the figure "<caption>" (block <id>): <instruction>`.
static FIGURE_ONLY: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
    regex::Regex::new(
        r#"(?s)^\s*Change only the figure "(.*?)" \(block ([0-9a-fA-F-]{32,36})\):\s*(.+)$"#,
    )
    .unwrap()
});

/// (block id, instruction) of the editor's figure form.
pub fn figure_only(instruction: &str) -> Option<(uuid::Uuid, String)> {
    let c = FIGURE_ONLY.captures(instruction)?;
    let id = uuid::Uuid::parse_str(c.get(2)?.as_str()).ok()?;
    let rest = c.get(3)?.as_str().trim().to_string();
    (!rest.is_empty()).then_some((id, rest))
}

/// The edit schema with figure blocks (the Python schema plus chart and diagram).
pub fn schema() -> Value {
    let mut s = SCHEMA.clone();
    if let Some(Value::Array(e)) = s.pointer_mut(
        "/properties/changed_sections/items/properties/blocks/items/properties/block_type/enum",
    ) {
        e.push(json!("chart"));
        e.push(json!("diagram"));
    }
    // A free-form object ({"type": "object"} with no properties) is refused by Claude's structured output
    // ("additionalProperties must be explicitly set to false", 21 Oct): the block's data travels as one
    // JSON text and is parsed back by `as_dict`.
    if let Some(d) =
        s.pointer_mut("/properties/changed_sections/items/properties/blocks/items/properties/data")
    {
        *d = json!({"type": "string", "description": "The block's data as one JSON object, with the same fields as that block type has in the current proposal (for example {\"md\": \"...\"} for text)."});
    }
    s
}

/// The edit rules' figure paragraph.
pub const FIGURE_EDIT_RULES: &str = " Chart and diagram blocks: keep each one exactly as given (all its data) \
unless the instruction is about that figure; to change one, change its spec (same format) or caption and keep \
its other fields; never put a price or an accuracy figure in a figure.";

/// Words that make an instruction about figures (then a figure the model left out is really removed).
const FIGURE_WORDS: [&str; 12] = [
    "figure",
    "chart",
    "diagram",
    "timeline",
    "gantt",
    "graph",
    "drawing",
    "schedule",
    "architecture",
    // visuals2 P2C
    "site plan",
    "floor plan",
    "single-line",
];

fn mentions_figures(instruction: &str) -> bool {
    let low = instruction.to_lowercase();
    FIGURE_WORDS.iter().any(|w| low.contains(w))
}

fn internal(msg: impl Into<String>) -> ApiError {
    ApiError::internal(anyhow::anyhow!(msg.into()))
}

fn key<'a>(d: &'a Value, k: &str) -> ApiResult<&'a Value> {
    d.get(k).ok_or_else(|| internal(format!("KeyError: '{k}'")))
}

fn py_str(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        other => pyjson::str_value(&PyValue::from(other)),
    }
}

/// `dict(x)` of a block's data (a JSON object; anything else is a TypeError/ValueError in Python).
fn as_dict(v: &Value) -> ApiResult<Map<String, Value>> {
    // the schema sends block data as JSON text (see `schema`); an object is still taken as it is
    if let Some(text) = v.as_str() {
        return match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => Ok(m),
            _ => Err(internal("ValueError: block data is not a JSON object")),
        };
    }
    v.as_object()
        .cloned()
        .ok_or_else(|| internal("ValueError: dictionary update sequence element"))
}

/// `apply(db, doc, instruction, user)`: {"summary", "changed", "accuracy_removed"}.
pub async fn apply(
    db: &mut PgConnection,
    doc: &SalesDocument,
    instruction: &str,
    user: Option<&AuthUser>,
) -> ApiResult<Value> {
    let _ = user;
    if !core::EDITABLE.contains(&doc.status.as_str()) {
        return Err(ApiError::http(
            409,
            format!("A {} proposal can't be edited: revise it", doc.status),
        ));
    }
    if pystr::strip(instruction).is_empty() {
        return Err(ApiError::http(400, "Say what to change"));
    }
    if let Some((block_id, what)) = figure_only(instruction) {
        // visuals V2: only that figure, through the figure pipeline and its guards
        let out = super::proposal_visuals::change_by_instruction(db, doc, block_id, &what).await?;
        let section = out["block"]["section"].clone();
        let summary = if out["ok"].as_bool().unwrap_or(false) {
            "Figure changed".to_string()
        } else {
            let p: Vec<String> = out["problems"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            format!("Figure changed; still to check: {}", p.join("; "))
        };
        return Ok(
            json!({"summary": summary, "changed": [section], "accuracy_removed": [],
                         "figure": out}),
        );
    }
    let blocks = core::blocks_of(db, doc.id).await?;
    let current: Vec<Value> = blocks
        .iter()
        .map(|b| json!({"section": b.section, "block_type": b.block_type, "data": b.data}))
        .collect();
    let doc_ref = doc.id.to_string();
    let mut run = super::runs::start_run(
        db,
        "manual",
        "nary.proposal_edit",
        Some(&doc_ref),
        "internal",
        None,
    )
    .await?;
    let shown: Vec<Value> = current
        .iter()
        .filter(|b| !matches!(b["block_type"].as_str(), Some("commercial") | Some("stamp")))
        .cloned()
        .collect();
    let content = format!(
        "LANGUAGE: {}\nINSTRUCTION:\n{}\n\nCURRENT BLOCKS (JSON):\n{}",
        doc.language,
        super::py_slice(instruction, 4000),
        dumps_unicode(&Value::Array(shown))?
    );
    let rules = format!("{EDIT_RULES}{FIGURE_EDIT_RULES}"); // visuals V2
    let system = super::persona::system_prompt("staff", &rules, "", "");
    let data = super::llm::ask_json(
        db,
        "nary.proposal_edit",
        &system,
        &content,
        &schema(),
        Some(&mut run),
        None,
        None,
    )
    .await?;
    let Some(data) = data else {
        super::runs::finish_run(db, &mut run, "failed", None, Some("no usable edit")).await?;
        return Err(ApiError::http(
            502,
            "Nary couldn't make that change. Try saying it differently.",
        ));
    };
    // changed = {s["key"]: s["blocks"]}: a repeated key keeps its first position and its last blocks.
    let mut changed: Vec<(String, Vec<Value>)> = Vec::new();
    for s in key(&data, "changed_sections")?
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let k = py_str(key(&s, "key")?);
        let bl = key(&s, "blocks")?.as_array().cloned().unwrap_or_default();
        match changed.iter_mut().find(|(x, _)| *x == k) {
            Some(slot) => slot.1 = bl,
            None => changed.push((k, bl)),
        }
    }
    let mut removed: Vec<String> = Vec::new();
    let mut figures_refused: Vec<String> = Vec::new();
    let mut kept_figures: Vec<Value> = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    let mut done: Vec<String> = Vec::new();
    for b in &current {
        let section = b["section"].as_str().unwrap_or_default().to_string();
        let generated = matches!(b["block_type"].as_str(), Some("commercial") | Some("stamp"));
        if let Some((_, new_blocks)) = changed
            .iter()
            .find(|(k, _)| *k == section)
            .filter(|_| !generated)
        {
            if !done.contains(&section) {
                for nb in new_blocks {
                    let mut d = as_dict(key(nb, "data")?)?;
                    let bt = key(nb, "block_type")?;
                    if bt
                        .as_str()
                        .is_some_and(crate::documents::figures::is_figure)
                    {
                        // visuals V2: a changed figure goes through the guards; refused: the old one stays
                        let same = current.iter().find(|c| {
                            c["data"].get("svg_asset_id").is_some()
                                && c["data"].get("svg_asset_id") == d.get("svg_asset_id")
                        });
                        let changed_spec =
                            !same.is_some_and(|c| c["data"].get("spec") == d.get("spec"));
                        if changed_spec {
                            let spec = d.get("spec").cloned().unwrap_or(Value::Null);
                            let size = if d.get("size").and_then(Value::as_str) == Some("half") {
                                crate::visuals::spec::FigureSize::Half
                            } else {
                                crate::visuals::spec::FigureSize::Full
                            };
                            let verdict = crate::documents::figures::try_draw(
                                db,
                                Some(doc),
                                bt.as_str(),
                                &spec,
                                size,
                            )
                            .await?;
                            if let Err(problems) = verdict {
                                figures_refused.extend(problems);
                                if let Some(old) = same {
                                    out.push(old.clone());
                                    kept_figures.push(old["data"]["svg_asset_id"].clone());
                                }
                                continue;
                            }
                        }
                        if let Some(old) = same {
                            kept_figures.push(old["data"]["svg_asset_id"].clone());
                        }
                    }
                    if bt.as_str() == Some("text") {
                        let md_in = d
                            .get("md")
                            .filter(|v| super::actions::py_truthy(v))
                            .map(py_str)
                            .unwrap_or_default();
                        let (md, cut) = strip_accuracy(&md_in);
                        removed.extend(cut);
                        d.insert("md".into(), Value::String(md));
                    }
                    out.push(json!({"section": section, "block_type": bt, "data": d}));
                }
                done.push(section);
            }
            continue;
        }
        out.push(b.clone());
    }
    if !mentions_figures(instruction) {
        // visuals V2: a figure the model left out of a section it rewrote stays, at its old place
        for (i, b) in current.iter().enumerate() {
            let is_fig = b["block_type"]
                .as_str()
                .is_some_and(crate::documents::figures::is_figure);
            let id = b["data"]
                .get("svg_asset_id")
                .cloned()
                .unwrap_or(Value::Null);
            if !is_fig || id.is_null() || kept_figures.contains(&id) || out.iter().any(|o| o == b) {
                continue;
            }
            let section = b["section"].clone();
            let before = current[..i]
                .iter()
                .filter(|c| c["section"] == section)
                .count();
            let starts = out
                .iter()
                .position(|o| o["section"] == section)
                .unwrap_or(out.len());
            let in_sec = out.iter().filter(|o| o["section"] == section).count();
            out.insert(starts + before.min(in_sec), b.clone());
        }
    }
    for (k, new_blocks) in &changed {
        // a section that had no blocks yet
        if !done.contains(k) {
            for nb in new_blocks {
                out.push(json!({"section": k, "block_type": key(nb, "block_type")?, "data": as_dict(key(nb, "data")?)?}));
            }
        }
    }
    let tmpl = core::template(db).await?;
    let n = tmpl.len();
    // order = {s["key"]: i}: a duplicated key keeps its last index; unknown sections go last. Stable sort.
    let order_of = |sec: &Value| -> usize {
        tmpl.iter()
            .rposition(|s| s.get("key") == Some(sec))
            .unwrap_or(n)
    };
    out.sort_by_key(|b| order_of(&b["section"]));
    core::replace_blocks(db, doc, &out).await?;
    let summary = py_str(key(&data, "summary")?);
    super::runs::finish_run(
        db,
        &mut run,
        "done",
        Some(super::py_slice(&summary, 500)),
        None,
    )
    .await?;
    let mut keys: Vec<String> = changed.iter().map(|(k, _)| k.clone()).collect();
    keys.sort();
    let mut result =
        json!({"summary": data["summary"], "changed": keys, "accuracy_removed": removed});
    if !figures_refused.is_empty() {
        result["figures_refused"] = json!(figures_refused); // visuals V2
    }
    Ok(result)
}
