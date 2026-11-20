//! A Solution Proposal written by the ERP chat or the MCP client itself, section by section (23 Oct 2026, the
//! Owner: "write it like our last proposal" instead of handing it to the internal drafter).
//!
//! ```text
//!     proposal.from_sections   op create: the sections the caller wrote (text, kv, table and coverage blocks,
//!                              chart and diagram briefs), the phases for the timeline,
//!                              optional requirement sheet changes, quotations to link, and the cost sheet. Applied
//!                              by the drafter's own second half (proposals::build_from_draft): the same document,
//!                              figures and cost sheet, what failed listed in `partial`.
//!                              op replace_section: one section's text, kv, table and coverage blocks replaced as
//!                              given, with no model call; its figures and pictures stay (a {type: keep, block_id}
//!                              entry places one), the other sections and their block ids are untouched.
//! ```
//!
//! Starts at Ask (migration 0126), the requester rule of proposal.from_chat (sales:create_quotation or the Owner).
//! Every block is checked when the card is made and again when it is applied: an unknown or generated section,
//! money, an accuracy figure or a product name the catalog does not have is refused per block
//! (visuals::guard::prose_problems); em dashes are removed. One proposal per deal, as proposal.from_chat.

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::documents::{SalesDocument, SalesDocumentBlock};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::{apply_fn, entity_fn, Action, Field, Schema, Ty};
use crate::ai::tools::catalog::proposal_tools::number_of;
use crate::documents::core_ as core;
use crate::visuals::guard::{self, GuardContext};

pub const FROM_SECTIONS: &str = "proposal.from_sections";

/// The blocks the caller writes as they are (no model).
pub const CONTENT_TYPES: [&str; 4] = ["text", "kv", "table", "coverage"];
const COVERAGE_STATUSES: [&str; 4] = ["included", "optional_custom", "not_in_scope", "to_confirm"];

fn schema() -> Schema {
    Schema::strict(
        "ProposalFromSections",
        vec![
            Field::req("op", Ty::Literal(&["create", "replace_section"])),
            Field::opt("deal_id", Ty::Str),
            Field::opt("document_id", Ty::Str),
            Field::opt("section", Ty::Str),
            Field::opt("title", Ty::Str),
            Field::opt("subtitle", Ty::Str),
            Field::opt("language", Ty::Literal(&["en"])),
            Field::opt("sections", Ty::list(Ty::Dict)),
            Field::opt("blocks", Ty::list(Ty::Dict)),
            Field::opt("phases", Ty::Dict),
            Field::opt("requirements", Ty::Dict),
            Field::opt("quotation_ids", Ty::list(Ty::Str)),
            Field::opt("cost_sheet", Ty::Bool),
            Field::opt("new_proposal", Ty::Bool),
        ],
    )
}

/// The actions of this module (registered by actions::module_actions).
pub fn actions() -> Vec<Action> {
    vec![Action::new(
        FROM_SECTIONS,
        "Write a Solution Proposal from the given sections",
        schema(),
        apply_fn(|db, a, p| Box::pin(apply(db, a, p))),
    )
    .describe(apply_fn(|db, _a, p| Box::pin(describe(db, p))))
    .entity(entity_fn(|p| {
        let id = |k: &str| {
            p.get(k)
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
        };
        Ok(match (id("deal_id"), id("document_id")) {
            (Some(d), _) => (Some("deal".into()), Some(d)),
            (None, Some(d)) => (Some("sales_document".into()), Some(d)),
            _ => (None, None),
        })
    }))]
}

fn s<'a>(p: &'a Value, k: &str) -> &'a str {
    p.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

/// A cell or a short field as text ("" for null); em dashes removed.
fn cell(v: &Value) -> Option<String> {
    let mut t = match v {
        Value::Null => String::new(),
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    guard::clean(&mut t);
    Some(t)
}

/// A list given as a list, or as JSON text (flat function-calling schemas send text).
fn list_of(v: Option<&Value>) -> Option<Vec<Value>> {
    match v? {
        Value::Array(a) => Some(a.clone()),
        Value::String(t) if !t.trim().is_empty() => match serde_json::from_str::<Value>(t) {
            Ok(Value::Array(a)) => Some(a),
            _ => None,
        },
        _ => None,
    }
}

/// The template's sections: (key, generated from the quotation or the stamp).
pub async fn template_sections(db: &mut PgConnection) -> ApiResult<Vec<(String, bool)>> {
    Ok(core::template(db)
        .await?
        .iter()
        .filter_map(|t| {
            let key = t.get("key").and_then(Value::as_str)?.to_string();
            let generated = t
                .get("blocks")
                .and_then(Value::as_array)
                .is_some_and(|b| b.iter().any(|x| x == "commercial" || x == "stamp"));
            Some((key, generated))
        })
        .collect())
}

/// Checks a section key: Err is the refusal.
fn section_ok(key: &str, tmpl: &[(String, bool)]) -> Result<(), String> {
    match tmpl.iter().find(|(k, _)| k == key) {
        None => Err(format!(
            "section '{key}' is not in the proposal template (sections: {})",
            tmpl.iter()
                .filter(|(_, g)| !g)
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Some((_, true)) => Err(format!(
            "section '{key}' is generated (the commercial section from the linked quotations, the stamp): it can't be written"
        )),
        Some(_) => Ok(()),
    }
}

/// Where the blocks may go: new figures (create) or `keep` entries for the section's own
/// figures and pictures (replace_section).
enum Mode<'a> {
    Create,
    Replace { keepable: &'a [Uuid] },
}

fn prose_into(
    ctx: &GuardContext,
    allow_money: bool,
    what: &str,
    text: &str,
    bad: &mut dyn FnMut(String),
) {
    for p in guard::prose_problems_with(what, text, ctx, allow_money) {
        bad(p);
    }
}

/// 1 Nov 2026 (direct proposal authoring, documents::core_ block ops): one section's content blocks given as
/// the tools take them ({type: text|kv|table|coverage|keep, heading, md, rows, columns, block_id}), checked and
/// cleaned like replace_section's. `keepable`: the section's own figures and pictures a keep entry may place.
/// `allow_money`: a price section written by someone who may state prices. Problems are pushed per block.
pub fn clean_section_blocks(
    section: &str,
    blocks: &[Value],
    ctx: &GuardContext,
    keepable: &[Uuid],
    allow_money: bool,
    problems: &mut Vec<String>,
) -> Vec<Value> {
    clean_blocks(
        section,
        blocks,
        ctx,
        &Mode::Replace { keepable },
        allow_money,
        problems,
    )
}

/// The stored data of one cleaned content block (text, kv, table, coverage), as replace_section writes it.
pub fn stored_data(b: &Value) -> ApiResult<(String, Value)> {
    let t = s(b, "type");
    let heading = b.get("heading").cloned().unwrap_or(Value::Null);
    let data = match t {
        "text" => json!({"md": s(b, "md"), "heading": heading}),
        "kv" => json!({"rows": b.get("rows").cloned().unwrap_or(json!([])), "heading": heading}),
        "table" => json!({"columns": b.get("columns").cloned().unwrap_or(json!([])),
                          "rows": b.get("rows").cloned().unwrap_or(json!([])), "heading": heading}),
        _ => json!({"rows": b.get("rows").cloned().unwrap_or(json!([])), "heading": heading}),
    };
    Ok((t.to_string(), core::validate_block(&json!(t), &data)?))
}

/// One section's blocks, checked and cleaned (em dashes removed, rows as lists). Problems are pushed per block.
fn clean_blocks(
    section: &str,
    blocks: &[Value],
    ctx: &GuardContext,
    mode: &Mode<'_>,
    allow_money: bool,
    problems: &mut Vec<String>,
) -> Vec<Value> {
    let mut out = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let t = b.get("type").and_then(Value::as_str).unwrap_or("").trim();
        let at = format!("section {section}, block {} ({t})", i + 1);
        let mut bad = |why: String| problems.push(format!("{at}: {why}"));
        let prose = |what: &str, text: &str, bad: &mut dyn FnMut(String)| {
            prose_into(ctx, allow_money, what, text, bad)
        };
        let heading = match b.get("heading").map(cell) {
            None => None,
            Some(Some(h)) if h.is_empty() => None,
            Some(Some(h)) => {
                prose("the heading", &h, &mut bad);
                Some(h)
            }
            Some(None) => {
                bad("heading must be text".into());
                None
            }
        };
        let mut block = Map::new();
        block.insert("type".into(), json!(t));
        block.insert("heading".into(), json!(heading));
        match t {
            "text" => {
                let md = b.get("md").and_then(cell).unwrap_or_default();
                if md.is_empty() {
                    bad("md (the text, Markdown) is required".into());
                    continue;
                }
                prose("the text", &md, &mut bad);
                block.insert("md".into(), json!(md));
            }
            "kv" => {
                let Some(rows) = list_of(b.get("rows")) else {
                    bad("rows must be a list of [label, value]".into());
                    continue;
                };
                let mut clean = Vec::new();
                for r in &rows {
                    let pair: Option<Vec<String>> = r
                        .as_array()
                        .filter(|a| a.len() == 2)
                        .and_then(|a| a.iter().map(cell).collect());
                    let Some(pair) = pair else {
                        bad(format!("the row {r} is not [label, value]"));
                        continue;
                    };
                    for c in &pair {
                        prose("a row", c, &mut bad);
                    }
                    clean.push(json!(pair));
                }
                block.insert("rows".into(), json!(clean));
            }
            "table" => {
                let columns: Option<Vec<String>> = list_of(b.get("columns"))
                    .and_then(|c| c.iter().map(cell).collect::<Option<Vec<String>>>())
                    .filter(|c| !c.is_empty());
                let Some(columns) = columns else {
                    bad("columns must be a list of column names".into());
                    continue;
                };
                for c in &columns {
                    prose("a column", c, &mut bad);
                }
                let mut clean = Vec::new();
                for r in list_of(b.get("rows")).unwrap_or_default() {
                    let row: Option<Vec<String>> =
                        r.as_array().and_then(|a| a.iter().map(cell).collect());
                    let Some(row) = row else {
                        bad(format!(
                            "the row {r} is not a list of cells (one per column)"
                        ));
                        continue;
                    };
                    for c in &row {
                        prose("a cell", c, &mut bad);
                    }
                    clean.push(json!(row));
                }
                block.insert("columns".into(), json!(columns));
                block.insert("rows".into(), json!(clean));
            }
            "coverage" => {
                let mut clean = Vec::new();
                for r in list_of(b.get("rows")).unwrap_or_default() {
                    let Some(m) = r.as_object() else {
                        bad(format!("the row {r} is not {{requirement, status, notes}}"));
                        continue;
                    };
                    let status = m.get("status").and_then(Value::as_str).unwrap_or("");
                    if !COVERAGE_STATUSES.contains(&status) {
                        bad(format!(
                            "status '{status}' must be one of {}",
                            COVERAGE_STATUSES.join(", ")
                        ));
                        continue;
                    }
                    let mut row = Map::new();
                    for k in ["requirement", "solution", "notes"] {
                        let v = m.get(k).and_then(cell).unwrap_or_default();
                        prose(&format!("the {k}"), &v, &mut bad);
                        row.insert(k.into(), json!(v));
                    }
                    row.insert("status".into(), json!(status));
                    clean.push(Value::Object(row));
                }
                block.insert("rows".into(), json!(clean));
            }
            "chart" | "diagram" if matches!(mode, Mode::Create) => {
                let brief = b.get("figure").and_then(cell).unwrap_or_default();
                if brief.is_empty() {
                    bad("figure (what the figure must show) is required".into());
                    continue;
                }
                let caption = b.get("caption").and_then(cell).unwrap_or_default();
                prose("the caption", &caption, &mut bad);
                let size = match b.get("size").and_then(Value::as_str) {
                    Some("half") => "half",
                    _ => "full",
                };
                block.insert("figure".into(), json!(brief));
                block.insert(
                    "caption".into(),
                    if caption.is_empty() {
                        Value::Null
                    } else {
                        json!(caption)
                    },
                );
                block.insert("size".into(), json!(size));
            }
            "keep" => {
                let Mode::Replace { keepable } = mode else {
                    bad("keep is only for replace_section".into());
                    continue;
                };
                let id = b
                    .get("block_id")
                    .and_then(Value::as_str)
                    .and_then(|x| Uuid::parse_str(x.trim()).ok());
                match id {
                    Some(id) if keepable.contains(&id) => {
                        block.insert("block_id".into(), json!(id.to_string()));
                    }
                    _ => {
                        bad("block_id must be a figure or picture of this section (get_proposal lists them)".into());
                        continue;
                    }
                }
            }
            "chart" | "diagram" | "image" => {
                bad("a new figure or picture in an existing proposal goes through propose_proposal_visual or propose_proposal_image; here only text, kv, table, coverage and keep".into());
                continue;
            }
            _ => {
                bad(match mode {
                    Mode::Create => {
                        "type must be text, kv, table, coverage, chart or diagram".into()
                    }
                    Mode::Replace { .. } => "type must be text, kv, table, coverage or keep".into(),
                });
                continue;
            }
        }
        out.push(Value::Object(block));
    }
    out
}

/// An object given as an object or as JSON text.
fn object_of(v: Option<&Value>) -> Option<Value> {
    super::proposal_visuals::json_text_object(v)
}

/// The payload checked and cleaned (sections and blocks as stored, em dashes removed). Err: every refusal,
/// one line per block. Used by the tool before the card and again when the card is applied (it may have been
/// edited on the Approvals page).
pub async fn clean_payload(db: &mut PgConnection, p: &Value) -> ApiResult<Result<Value, String>> {
    let tmpl = template_sections(db).await?;
    let mut problems = Vec::new();
    let mut out = p.as_object().cloned().unwrap_or_default();
    if s(p, "op") == "replace_section" {
        let doc = super::proposal_visual_actions::load_document(db, p).await?;
        let ctx = guard::load_context(db, Some(doc.id)).await?;
        let key = s(p, "section").to_string();
        if let Err(why) = section_ok(&key, &tmpl) {
            return Ok(Err(why));
        }
        let keepable: Vec<Uuid> = core::blocks_of(db, doc.id)
            .await?
            .into_iter()
            .filter(|b| b.section == key && !CONTENT_TYPES.contains(&b.block_type.as_str()))
            .map(|b| b.id)
            .collect();
        let blocks = list_of(p.get("blocks")).unwrap_or_default();
        if blocks.is_empty() {
            return Ok(Err("blocks: give the section's new blocks".into()));
        }
        let clean = clean_blocks(
            &key,
            &blocks,
            &ctx,
            &Mode::Replace {
                keepable: &keepable,
            },
            false,
            &mut problems,
        );
        out.insert("blocks".into(), json!(clean));
    } else {
        let ctx = guard::load_context(db, None).await?;
        let mut title = s(p, "title").to_string();
        guard::clean(&mut title);
        if title.is_empty() {
            problems.push("title is required".into());
        }
        let mut subtitle = s(p, "subtitle").to_string();
        guard::clean(&mut subtitle);
        for (what, t) in [("the title", &title), ("the subtitle", &subtitle)] {
            problems.extend(guard::prose_problems(what, t, &ctx));
        }
        out.insert("title".into(), json!(title));
        out.insert("subtitle".into(), json!(subtitle));
        let sections = list_of(p.get("sections")).unwrap_or_default();
        if sections.is_empty() {
            problems.push("sections: give at least one section with its blocks".into());
        }
        let mut seen: Vec<String> = Vec::new();
        let mut clean_sections = Vec::new();
        for sec in &sections {
            let key = s(sec, "key").to_string();
            if let Err(why) = section_ok(&key, &tmpl) {
                problems.push(why);
                continue;
            }
            if seen.contains(&key) {
                problems.push(format!(
                    "section '{key}' is given twice: put all its blocks in one"
                ));
                continue;
            }
            seen.push(key.clone());
            let blocks = list_of(sec.get("blocks")).unwrap_or_default();
            let clean = clean_blocks(&key, &blocks, &ctx, &Mode::Create, false, &mut problems);
            clean_sections.push(json!({"key": key, "blocks": clean}));
        }
        out.insert("sections".into(), json!(clean_sections));
        match p.get("phases") {
            None | Some(Value::Null) => {}
            v => match object_of(v) {
                Some(o) => {
                    out.insert("phases".into(), o);
                }
                None => problems.push("phases must be an object".to_string()),
            },
        }
    }
    if !problems.is_empty() {
        return Ok(Err(problems.join("\n")));
    }
    Ok(Ok(Value::Object(out)))
}

/// "2 text, 1 table, 1 chart".
fn counts(blocks: &[Value]) -> String {
    let mut n: Vec<(String, usize)> = Vec::new();
    for b in blocks {
        let t = match b.get("type").and_then(Value::as_str).unwrap_or("") {
            "keep" => "kept figure",
            other => other,
        };
        match n.iter_mut().find(|(k, _)| k == t) {
            Some((_, c)) => *c += 1,
            None => n.push((t.to_string(), 1)),
        }
    }
    n.iter()
        .map(|(k, c)| format!("{c} {k}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The card: the sections with their block counts, the figures to draw, the quotations, the
/// cost sheet (create); the section and its new blocks (replace_section).
pub async fn describe(db: &mut PgConnection, p: &Value) -> ApiResult<Value> {
    let mut lines = Vec::new();
    if s(p, "op") == "replace_section" {
        let doc = super::proposal_visual_actions::load_document(db, p).await?;
        let number = number_of(&doc);
        let blocks = list_of(p.get("blocks")).unwrap_or_default();
        lines.push(format!("New blocks: {}", counts(&blocks)));
        for b in &blocks {
            let t = s(b, "type");
            let head = s(b, "heading");
            let body = match t {
                "text" => super::py_slice(s(b, "md"), 160).to_string(),
                "keep" => format!("figure {} stays", s(b, "block_id")),
                _ => String::new(),
            };
            lines.push(
                format!(
                    "- {t}{}{}",
                    if head.is_empty() {
                        String::new()
                    } else {
                        format!(" \"{head}\"")
                    },
                    if body.is_empty() {
                        String::new()
                    } else {
                        format!(": {body}")
                    }
                )
                .trim_end()
                .to_string(),
            );
        }
        lines.push("The section's figures and pictures stay; the other sections are unchanged. No AI rewrite.".into());
        return Ok(
            json!({"label": format!("Replace section {} of {number}", s(p, "section")),
                         "lines": lines}),
        );
    }
    let (deal, org) = super::proposals::deal_and_org(db, s(p, "deal_id")).await?;
    lines.push(format!(
        "Title: {}{}",
        s(p, "title"),
        if s(p, "subtitle").is_empty() {
            String::new()
        } else {
            format!(" ({})", s(p, "subtitle"))
        }
    ));
    let mut figures = Vec::new();
    for sec in list_of(p.get("sections")).unwrap_or_default() {
        let blocks = list_of(sec.get("blocks")).unwrap_or_default();
        lines.push(format!("- {}: {}", s(&sec, "key"), counts(&blocks)));
        for b in &blocks {
            match s(b, "type") {
                "chart" | "diagram" => figures.push(format!(
                    "{} ({})",
                    if s(b, "caption").is_empty() {
                        super::py_slice(s(b, "figure"), 60)
                    } else {
                        s(b, "caption")
                    },
                    s(b, "type")
                )),
                _ => {}
            }
        }
    }
    let phases = p
        .get("phases")
        .and_then(|ph| ph.get("tasks"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let mut draw =
        vec!["the default figures (Settings: proposal.visuals.default_figures)".to_string()];
    if phases > 0 {
        draw[0] = format!("the timeline from {phases} phase task(s) and the other default figures");
    }
    draw.extend(figures);
    lines.push(format!("Figures to draw: {}", draw.join("; ")));
    let mut quotes = Vec::new();
    for q in p
        .get("quotation_ids")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let Some(id) = q.as_str().and_then(|x| Uuid::parse_str(x).ok()) else {
            continue;
        };
        let row: Option<granary_db::rows::sales::Quotation> =
            sqlx::query_as("SELECT * FROM quotations WHERE id = $1 AND is_deleted = false")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        if let Some(q) = row {
            quotes.push(format!(
                "{} ({})",
                super::proposal_quotations::quote_number(&q),
                super::proposal_quotations::total_of(&q)
            ));
        }
    }
    if !quotes.is_empty() {
        lines.push(format!("Quotations linked: {}", quotes.join(", ")));
    }
    lines.push(if p.get("cost_sheet") == Some(&json!(false)) {
        "Cost sheet: not made".to_string()
    } else {
        "Cost sheet: made (internal, for sales:view_costs)".to_string()
    });
    if p.get("requirements")
        .and_then(Value::as_object)
        .is_some_and(|m| !m.is_empty())
    {
        lines.push("The requirement sheet is updated first with the given changes.".into());
    }
    lines.push(
        "The text is used as written (no AI rewrite); figures and the cost sheet are made by Nary."
            .into(),
    );
    Ok(json!({
        "label": format!("Solution Proposal for {} from your sections",
                         super::proposals::deal_label(&deal, &org)),
        "lines": lines,
    }))
}

/// Applies proposal.from_sections. Commits.
async fn apply(db: &mut PgConnection, actor: Option<&AuthUser>, p: &Value) -> ApiResult<Value> {
    super::proposals::check_requester(db, actor).await?;
    let p = match clean_payload(db, p).await? {
        Ok(p) => p,
        Err(why) => return Err(ApiError::http(422, why)),
    };
    if s(&p, "op") == "replace_section" {
        return apply_replace(db, &p).await;
    }
    apply_create(db, actor, &p).await
}

async fn apply_create(
    db: &mut PgConnection,
    actor: Option<&AuthUser>,
    p: &Value,
) -> ApiResult<Value> {
    use super::proposals as pr;
    let deal_id = s(p, "deal_id").to_string();
    let requirements = p
        .get("requirements")
        .filter(|r| r.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let (deal, _, merged) = pr::preview_sheet(db, &deal_id, &requirements).await?;
    // a retry makes the same document again; a deal with a proposal is not given a second one unless asked
    let card = super::wa_send::current_card();
    let reuse = pr::unfinished_document(db, card, deal.id).await?;
    if reuse.is_none() && !p.get("new_proposal").is_some_and(super::actions::py_truthy) {
        if let Some(d) = pr::live_proposal(db, deal.id).await? {
            return Err(ApiError::http(409, exists_refusal(&d)));
        }
    }
    if requirements.as_object().is_some_and(|m| !m.is_empty()) {
        pr::write_sheet(db, deal.id, &merged, Some("ready")).await?;
        super::commit(db).await?;
    }
    let mut ctx = pr::draft_context(db, &deal_id, None).await?;
    if let Some(l) = p.get("language").and_then(Value::as_str) {
        ctx.language = l.to_string();
    }
    let deal_ref = deal.id.to_string();
    let mut run = super::runs::start_run(
        db,
        "manual",
        "nary.proposal_sections",
        Some(&deal_ref),
        "internal",
        None,
    )
    .await?;
    let mut data = json!({"title": s(p, "title"), "subtitle": s(p, "subtitle"),
                          "sections": p.get("sections").cloned().unwrap_or(json!([]))});
    if let Some(v) = p.get("phases").filter(|v| v.is_object()) {
        data["phases"] = v.clone();
    }
    let cost_sheet = p.get("cost_sheet") != Some(&json!(false));
    let out = pr::build_from_draft(
        db,
        actor,
        &ctx,
        data,
        pr::DraftInto {
            reuse,
            record_on: card,
            instruction: None,
        },
        &mut run,
        cost_sheet,
    )
    .await?;
    let doc_id = out
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut partial: Vec<Value> = out
        .get("partial")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let doc: SalesDocument = sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
        .bind(Uuid::parse_str(&doc_id).map_err(|_| ApiError::http(500, "no document"))?)
        .fetch_one(&mut *db)
        .await?;
    let mut linked = Vec::new();
    for q in p
        .get("quotation_ids")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let qid = q.as_str().unwrap_or("").to_string();
        match core::link_quotation(db, &doc, &qid, None).await {
            Ok(()) => {
                super::commit(db).await?;
                linked.push(qid);
            }
            Err(e) => {
                if !super::governance::is_http_exception(&e) {
                    super::rollback(db).await?;
                }
                partial.push(pr::partial_entry(
                    "quotations",
                    &format!("{qid}: {}", pr::stage_error(&e)),
                ));
            }
        }
    }
    let number = number_of(&doc);
    let link = format!("/sales/proposals/{doc_id}");
    let mut label =
        format!("Proposal {number} written from the given sections: {link} (document_id {doc_id})");
    if !linked.is_empty() {
        label += &format!("; {} quotation(s) linked", linked.len());
    }
    match pr::partial_text(&partial) {
        Some(m) => label += &format!("; but {m}"),
        None if cost_sheet => label += "; figures and cost sheet made",
        None => label += "; figures made",
    }
    let mut result = json!({"label": label, "document_id": doc_id, "number": number, "link": link,
                            "quotations_linked": linked, "partial": partial});
    if actor.is_some_and(crate::ai::tools::catalog::can_see_costs) {
        if let Some(c) = super::costing::latest(db, doc.id).await? {
            let likely = c
                .content
                .get("summary")
                .and_then(|x| x.get("likely"))
                .cloned()
                .unwrap_or(Value::Null);
            result["cost_sheet"] = json!({"currency": c.currency, "likely": likely});
        }
    }
    Ok(result)
}

/// The refusal a second proposal for a deal gets (from this tool's point of view).
pub fn exists_refusal(d: &SalesDocument) -> String {
    format!(
        "{} exists for this deal ({}): change a section with op replace_section (document_id {}), or revise it \
         with propose_proposal_revision; pass new_proposal: true only when the person asked for a separate new \
         proposal.",
        number_of(d),
        d.status,
        d.id
    )
}

/// Replaces one section's content blocks; its figures and pictures stay (where a keep entry puts them, else
/// after the new blocks), every other block keeps its id. Does not commit.
pub async fn replace_section(
    db: &mut PgConnection,
    doc: &SalesDocument,
    section: &str,
    blocks: &[Value],
) -> ApiResult<usize> {
    let order: Vec<String> = template_sections(db)
        .await?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let rank = |k: &str| order.iter().position(|x| x == k).unwrap_or(usize::MAX);
    let all: Vec<SalesDocumentBlock> = core::blocks_of(db, doc.id).await?;
    // the section's new list: Ok(existing block id) or Err((type, data)) for a new block
    let mut list: Vec<Result<Uuid, (String, Value)>> = Vec::new();
    for b in blocks {
        let t = s(b, "type");
        if t == "keep" {
            if let Ok(id) = Uuid::parse_str(s(b, "block_id")) {
                list.push(Ok(id));
            }
            continue;
        }
        list.push(Err(stored_data(b)?));
    }
    let kept: Vec<Uuid> = list
        .iter()
        .filter_map(|x| x.as_ref().ok().copied())
        .collect();
    for b in all.iter().filter(|b| {
        b.section == section
            && !CONTENT_TYPES.contains(&b.block_type.as_str())
            && !kept.contains(&b.id)
    }) {
        list.push(Ok(b.id));
    }
    let keep_ids: Vec<Uuid> = list
        .iter()
        .filter_map(|x| x.as_ref().ok().copied())
        .collect();
    sqlx::query(
        "DELETE FROM sales_document_blocks WHERE document_id = $1 AND section = $2 AND NOT (id = ANY($3))",
    )
    .bind(doc.id)
    .bind(section)
    .bind(&keep_ids)
    .execute(&mut *db)
    .await?;
    // the others keep their order; the section goes where the template puts it
    let others: Vec<&SalesDocumentBlock> = all.iter().filter(|b| b.section != section).collect();
    let at = others
        .iter()
        .position(|b| rank(&b.section) > rank(section))
        .unwrap_or(others.len());
    let mut final_list: Vec<Result<Uuid, (String, Value)>> =
        others[..at].iter().map(|b| Ok(b.id)).collect();
    let n = list.len();
    final_list.extend(list);
    final_list.extend(others[at..].iter().map(|b| Ok(b.id)));
    for (pos, item) in final_list.into_iter().enumerate() {
        match item {
            Ok(id) => {
                sqlx::query(
                    "UPDATE sales_document_blocks SET position = $3, updated_at = now() \
                     WHERE id = $1 AND document_id = $2 AND position <> $3",
                )
                .bind(id)
                .bind(doc.id)
                .bind(pos as i32)
                .execute(&mut *db)
                .await?;
            }
            Err((t, data)) => {
                sqlx::query(
                    "INSERT INTO sales_document_blocks (id, document_id, section, position, block_type, data) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(Uuid::new_v4())
                .bind(doc.id)
                .bind(section)
                .bind(pos as i32)
                .bind(&t)
                .bind(&data)
                .execute(&mut *db)
                .await?;
            }
        }
    }
    sqlx::query("UPDATE sales_documents SET updated_at = now() WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *db)
        .await?;
    Ok(n)
}

async fn apply_replace(db: &mut PgConnection, p: &Value) -> ApiResult<Value> {
    let doc = super::proposal_visual_actions::load_document(db, p).await?;
    let number = number_of(&doc);
    if !core::EDITABLE.contains(&doc.status.as_str()) {
        return Err(ApiError::http(
            409,
            format!(
                "{number} is {}: only a draft or a proposal in review can change. Revise it first.",
                doc.status
            ),
        ));
    }
    let section = s(p, "section").to_string();
    let blocks = list_of(p.get("blocks")).unwrap_or_default();
    let n = replace_section(db, &doc, &section, &blocks).await?;
    super::commit(db).await?;
    let link = format!("/sales/proposals/{}", doc.id);
    Ok(json!({
        "label": format!("{number}: section {section} replaced ({n} blocks); the other sections are unchanged. {link}"),
        "document_id": doc.id.to_string(), "number": number, "link": link, "partial": [],
    }))
}
