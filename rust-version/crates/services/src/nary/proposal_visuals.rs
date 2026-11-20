//! visuals V2: Nary's charts and diagrams in proposals (Rust only, after the Python freeze).
//!
//! ```text
//!     nary.visual_spec    Sonnet turns a brief (what the figure must show) into a figure spec: a chart, a
//!                         diagram, or a custom_svg drawing when the built-in kinds cannot draw it
//!     guard               visuals::guard (catalog names, no money, no accuracy figures,
//!                         sources; ERP refs resolved) refuses or warns; a refusal goes back to the model
//!     nary.visual_check   Sonnet vision looks at the PNG against the brief (overlaps, cut-off or unreadable
//!                         labels, missing items, wrong flow); problems go back to the model
//!     attempts            proposal.visuals.max_attempts in all; after that the last drawing is kept with its
//!                         warnings and the Owner is told; nothing drawn at all: no figure
//! ```
//!
//! [`make_figure`] is the pipeline; [`draft_figures`] adds the draft's figures (always a project timeline and
//! a system architecture diagram, proposal.visuals.default_figures, plus the ones the draft asked for);
//! [`change_by_instruction`] and [`redraw`] serve the editor's "Ask Nary to change this figure" and "Redraw".

use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json};
use granary_db::rows::documents::SalesDocument;
use granary_db::rows::nary::AgentRun;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::actions::py_truthy;
use crate::documents::core_::{self as core, WHERE};
use crate::documents::figures::{self, Drawn, FigureChange, NewFigure};
use crate::visuals::spec::FigureSize;

pub const SPEC_JOB: &str = "nary.visual_spec";
pub const CHECK_JOB: &str = "nary.visual_check";

/// Which figure when (the drafting rules, the spec job and V3's tool descriptions share it).
pub const WHICH_FIGURE: &str = "Proposal figures, which one when: a project timeline (gantt) always; a system \
architecture diagram always; a network diagram when sites and links matter; a process diagram for a workflow; \
other charts only with numbers from the ERP, the customer (say where they come from) or your own analysis of ERP \
data or a customer file (say how); custom_svg only for what the built-in kinds cannot draw. Pictures from the \
customer's files or the document library go in as images (propose_proposal_image). Never a price, a currency or \
an accuracy or precision figure in a figure.";

/// The spec job's instructions.
pub fn spec_rules() -> String {
    format!(
        "Design ONE figure for a Solution Proposal as a JSON spec (the schema given). {WHICH_FIGURE} \
Charts: bar, stacked_bar, hbar, line, area, combo, scatter, heatmap, waterfall, radar, donut, gantt \
(tasks with start and length in time units from 0, milestones), milestones. Diagrams: architecture or network \
(nodes with icons, grouped in sites, customer systems, cloud or zones; edges with their link kind), process \
(steps left to right, lanes as groups, no cycles). A catalog product node carries its catalog sku and the \
product's name or short name as its label; only products listed under PRODUCTS. Labels short (two to four \
words), in the figure's language. A source is required for chart numbers: erp (proposal.phases, \
requirements.<key>), customer (note: where they come from) or analysis (note: how they were worked out). \
custom_svg: one <svg> with a viewBox, shapes, paths and text only. No em dashes. caption: one plain sentence."
    )
}

/// The spec job's answer: {spec, caption}.
pub fn spec_answer_schema() -> Value {
    // 22 Oct: the full figure schema (224 nodes) is far past Claude's grammar limit for structured output;
    // the spec travels as JSON text with its shape in the description, and the guard checks it after.
    json!({"type": "object",
           "properties": {"spec_json": json_text_field("The figure spec", &crate::visuals::spec::spec_schema()),
                          "caption": {"type": "string"}},
           "required": ["spec_json", "caption"], "additionalProperties": false})
}

/// A required string field that carries one JSON object of the given shape (quoted in the description).
pub fn json_text_field(what: &str, shape: &Value) -> Value {
    json!({"type": "string",
           "description": format!("{what} as one JSON object (as text), with this shape (JSON schema): {shape}")})
}

/// The object of a JSON-text answer field, or the same field as an object (older answers, test fakes).
pub fn answer_object(answer: &Value, text_field: &str, object_field: &str) -> Option<Value> {
    json_text_object(answer.get(text_field))
        .or_else(|| answer.get(object_field).filter(|v| v.is_object()).cloned())
}

/// The check job's prompt for a figure that must show `brief`.
pub fn check_prompt(brief: &str, spec: &Value) -> String {
    let spec_s = serde_json::to_string(spec).unwrap_or_default();
    format!(
        "This is a chart or diagram for a customer proposal. Check the picture against what it must show. \
Problems are: labels that overlap or are cut off or unreadable, items of the brief or the spec that are \
missing, a wrong flow or order, an empty or broken drawing. Answer as JSON {{\"matches\": bool, \
\"problems\": [..]}} (problems in a few plain words each).\n\nWHAT IT MUST SHOW:\n{}\n\nSPEC:\n{}",
        super::py_slice(brief, 4000),
        super::py_slice(&spec_s, 8000)
    )
}

/// One figure to make.
#[derive(Debug, Clone, Default)]
pub struct FigureRequest {
    /// "chart" or "diagram" when the figure must be one of them; None = either.
    pub block_type: Option<String>,
    /// What the figure must show.
    pub brief: String,
    pub caption: Option<String>,
    pub size: FigureSize,
    /// A spec to try first (deterministic figures, a redraw, the spec being changed).
    pub start_spec: Option<Value>,
    /// The change asked for (with `start_spec`: change that figure).
    pub instruction: Option<String>,
    /// Facts the model may use (products, sections, sheet).
    pub context: String,
    pub language: String,
}

/// The pipeline's result.
#[derive(Debug, Clone)]
pub struct FigureOutcome {
    /// The last drawing that passed the guard (kept even when the check still complains).
    pub drawn: Option<Drawn>,
    pub caption: String,
    /// {ok, notes, at}: the vision check (None when proposal.visuals.vision_check is off).
    pub checked: Option<Value>,
    pub attempts: i64,
    /// What was still wrong at the end (guard refusals or check problems).
    pub problems: Vec<String>,
    pub ok: bool,
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// proposal.visuals.vision_check.
pub async fn vision_enabled(db: &mut PgConnection) -> ApiResult<bool> {
    Ok(py_truthy(
        &require_json(db, "proposal.visuals.vision_check", WHERE).await?,
    ))
}

/// `nary.visual_check` on a PNG: {"ok", "notes", "at"}.
pub async fn vision_check(
    db: &mut PgConnection,
    png: &[u8],
    brief: &str,
    spec: &Value,
    run: Option<&mut AgentRun>,
) -> ApiResult<Value> {
    use crate::ai::base::FileRequest;
    use crate::ai::registry::{use_ai, UseAiOptions};
    let prompt = check_prompt(brief, spec);
    let ai = use_ai(db, CHECK_JOB, UseAiOptions::run(run.as_ref().map(|r| r.id)))
        .await
        .map_err(ApiError::from)?;
    let result = ai
        .analyze_file(db, FileRequest::new(&prompt, png.to_vec(), "image/png"))
        .await
        .map_err(ApiError::from)?;
    if let Some(run) = run {
        let output = json!({"text": result.text});
        super::runs::add_step(
            db,
            run,
            "llm",
            Some(CHECK_JOB),
            super::runs::StepOpts {
                output: Some(&output),
                result: Some(&result),
                cost: ai.last_cost(),
                model: Some(ai.model()),
                ..Default::default()
            },
        )
        .await?;
    }
    let raw = if result.text.trim().is_empty() {
        "{}".to_string()
    } else {
        strip_fence(&result.text)
    };
    let v = parse_verdict(&raw);
    let notes: Vec<String> = v["problems"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| {
                    p.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| p.to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    let ok = v["matches"].as_bool().unwrap_or(false);
    Ok(json!({"ok": ok, "notes": notes, "at": now_iso()}))
}

/// A JSON answer wrapped in a Markdown fence.
/// The vision check's answer: {matches, problems}; an unreadable answer is a failed check.
pub fn parse_verdict(raw: &str) -> Value {
    let unreadable = json!({"matches": false, "problems": ["the check gave no readable answer"]});
    let Ok(Value::Object(m)) = serde_json::from_str::<Value>(raw) else {
        return unreadable;
    };
    let matches = m.get("matches").map(py_truthy).unwrap_or(false);
    // `for x in v or []`, as Python iterates: a list's items, a dict's keys, a string's characters
    let problems: Vec<Value> = match m.get("problems") {
        Some(v) if py_truthy(v) => match v {
            Value::Array(a) => a.clone(),
            Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
            Value::String(t) => t.chars().map(|c| Value::String(c.to_string())).collect(),
            _ => vec![],
        },
        _ => vec![],
    };
    json!({"matches": matches, "problems": problems})
}

fn strip_fence(t: &str) -> String {
    let s = t.trim();
    if let Some(rest) = s.strip_prefix("```") {
        let rest = rest.trim_start_matches("json");
        return rest.trim_end_matches("```").trim().to_string();
    }
    s.to_string()
}

/// The products the model may put in a diagram: the active catalog (sku, name, short name, family).
pub async fn catalog_lines(db: &mut PgConnection, limit: i64) -> ApiResult<String> {
    let rows: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT sku, name, short_name, family FROM product_catalog \
         WHERE COALESCE(is_active, true) ORDER BY family NULLS LAST, sku LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(sku, name, short, fam)| {
            format!(
                "- {sku}: {name}{}{}",
                short
                    .filter(|s| !s.is_empty())
                    .map(|s| format!(" (short: {s})"))
                    .unwrap_or_default(),
                fam.filter(|s| !s.is_empty())
                    .map(|s| format!(" [family {s}]"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The pipeline: spec (model or `start_spec`), guard, draw, vision check, fix; at most
/// proposal.visuals.max_attempts tries. Nothing is stored.
pub async fn make_figure(
    db: &mut PgConnection,
    doc: Option<&SalesDocument>,
    req: &FigureRequest,
    mut run: Option<&mut AgentRun>,
) -> ApiResult<FigureOutcome> {
    let max_attempts = require_int(db, "proposal.visuals.max_attempts", WHERE)
        .await?
        .max(1);
    let check = vision_enabled(db).await?;
    let products = catalog_lines(db, 80).await?;
    let system = super::persona::system_prompt("staff", &spec_rules(), "", "");
    let answer_schema = spec_answer_schema();
    let mut out = FigureOutcome {
        drawn: None,
        caption: req.caption.clone().unwrap_or_default(),
        checked: None,
        attempts: 0,
        problems: Vec::new(),
        ok: false,
    };
    let mut last_spec: Option<Value> = req.start_spec.clone();
    let mut feedback: Vec<String> = Vec::new();
    for attempt in 1..=max_attempts {
        out.attempts = attempt;
        // 1. the spec: the given one first (when there is no change to make), else the model's
        let spec = if attempt == 1 && req.start_spec.is_some() && req.instruction.is_none() {
            req.start_spec.clone().unwrap_or(Value::Null)
        } else {
            let mut content = format!(
                "LANGUAGE: {}\nFIGURE TYPE: {}\nWHAT IT MUST SHOW:\n{}\n",
                "en",
                req.block_type.as_deref().unwrap_or("chart or diagram"),
                super::py_slice(&req.brief, 6000)
            );
            if let Some(i) = &req.instruction {
                content += &format!("\nCHANGE ASKED FOR:\n{}\n", super::py_slice(i, 3000));
            }
            if let Some(s) = &last_spec {
                content += &format!(
                    "\nCURRENT SPEC (change only what is needed):\n{}\n",
                    super::py_slice(&serde_json::to_string(s).unwrap_or_default(), 20000)
                );
            }
            if !feedback.is_empty() {
                content += &format!(
                    "\nTHE LAST TRY WAS REFUSED OR LOOKED WRONG. Fix: {}\n",
                    feedback.join("; ")
                );
            }
            if !req.context.is_empty() {
                content += &format!("\nFACTS:\n{}\n", super::py_slice(&req.context, 12000));
            }
            content += &format!("\nPRODUCTS (catalog; sku: name):\n{products}\n");
            let answer = super::llm::ask_json(
                db,
                SPEC_JOB,
                &system,
                &content,
                &answer_schema,
                run.as_deref_mut(),
                None,
                None,
            )
            .await?;
            let Some(answer) = answer else {
                feedback = vec!["no usable spec came back".into()];
                out.problems = feedback.clone();
                continue;
            };
            if out.caption.is_empty() {
                if let Some(c) = answer.get("caption").and_then(Value::as_str) {
                    out.caption = c.chars().take(500).collect();
                }
            }
            answer_object(&answer, "spec_json", "spec").unwrap_or(Value::Null)
        };
        last_spec = Some(spec.clone());
        // 2. guard and draw
        let drawn =
            match figures::try_draw(db, doc, req.block_type.as_deref(), &spec, req.size).await? {
                Ok(d) => d,
                Err(problems) => {
                    feedback = problems.clone();
                    out.problems = problems;
                    continue;
                }
            };
        last_spec = Some(drawn.spec.to_value());
        // 3. look at it
        if !check {
            out.drawn = Some(drawn);
            out.problems.clear();
            out.ok = true;
            return Ok(out);
        }
        let brief = if req.brief.is_empty() {
            out.caption.clone()
        } else {
            req.brief.clone()
        };
        let verdict = vision_check(
            db,
            &drawn.png,
            &brief,
            &drawn.spec.to_value(),
            run.as_deref_mut(),
        )
        .await?;
        let ok = verdict["ok"].as_bool().unwrap_or(false);
        let notes: Vec<String> = verdict["notes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|n| n.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        out.drawn = Some(drawn);
        out.checked = Some(verdict);
        if ok {
            out.problems.clear();
            out.ok = true;
            return Ok(out);
        }
        feedback = if notes.is_empty() {
            vec!["the picture does not show the brief".into()]
        } else {
            notes
        };
        out.problems = feedback.clone();
    }
    Ok(out)
}

/// Tells the Owner a figure was kept with warnings, or could not be made (an error card).
pub async fn tell_owner(
    db: &mut PgConnection,
    doc: &SalesDocument,
    what: &str,
    outcome: &FigureOutcome,
) -> ApiResult<()> {
    let number = doc
        .full_number
        .clone()
        .unwrap_or_else(|| doc.number.clone());
    let (title, body) = if outcome.drawn.is_some() {
        (
            format!("Figure kept with warnings: {number}"),
            format!(
                "Nary drew \"{what}\" for {number} but after {} tries it still has problems: {}. It is in the \
                 proposal; check it, edit its data or ask Nary to change it.",
                outcome.attempts,
                outcome.problems.join("; ")
            ),
        )
    } else {
        (
            format!("Figure not added: {number}"),
            format!(
                "Nary could not make \"{what}\" for {number} after {} tries: {}. The proposal goes without it; \
                 add it in the editor or ask Nary again with more detail.",
                outcome.attempts,
                outcome.problems.join("; ")
            ),
        )
    };
    super::governance::error_card(
        db,
        &format!("proposal_figure:{}:{}", doc.id, super::py_slice(what, 60)),
        &title,
        &body,
        &format!("/sales/proposals/{}", doc.id),
    )
    .await?;
    Ok(())
}

// ------------------------------------------------------------------------------------------ drafting

/// The block fields the draft schema gains (visuals V2).
/// A nullable string field carrying one JSON object whose shape is the given (strict) schema, quoted in the
/// description so the model writes the same fields.
pub fn as_json_text(shape: Value) -> Value {
    let what = shape
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut inner = shape;
    if let Some(o) = inner.as_object_mut() {
        o.remove("description");
        o.insert("type".into(), json!("object"));
    }
    json!({"type": ["string", "null"],
           "description": format!("{what}. One JSON object (as text) with this shape: {inner}")})
}

/// The object a JSON-text field carries (an object is taken as it is; anything else is None).
pub fn json_text_object(v: Option<&Value>) -> Option<Value> {
    match v? {
        Value::String(t) => match serde_json::from_str::<Value>(t) {
            Ok(o @ Value::Object(_)) => Some(o),
            _ => None,
        },
        o @ Value::Object(_) => Some(o.clone()),
        _ => None,
    }
}

pub fn extend_draft_schema(base: &Value) -> Value {
    let mut s = base.clone();
    if let Some(item) = s
        .pointer_mut("/properties/sections/items/properties/blocks/items")
        .and_then(Value::as_object_mut)
    {
        if let Some(Value::Array(e)) = item
            .get_mut("properties")
            .and_then(|p| p.get_mut("type"))
            .and_then(|t| t.get_mut("enum"))
        {
            e.push(json!("chart"));
            e.push(json!("diagram"));
        }
        if let Some(Value::Object(p)) = item.get_mut("properties") {
            // 22 Oct: rows were {"type": "array", "items": {}} and the grammar filled them with numbers ([0, 0]
            // in a kv block crashed the PDF); they travel as JSON text with the shape per block type
            p.insert("rows".into(), json!({"type": ["string", "null"],
                "description": "JSON text. kv: [[\"label\", \"value\"], ...]; table: [[\"cell\", ...], ...] (one list per row, \
same order as columns); coverage: [{\"requirement\": \"\", \"solution\": \"\", \"status\": \"included|optional_custom|not_in_scope|to_confirm\", \"notes\": \"\"}, ...]; null for other blocks"}));
            p.insert("figure".into(), json!({"type": ["string", "null"],
                "description": "chart / diagram: what the figure must show (a brief; the spec is made from it)"}));
            p.insert("caption".into(), json!({"type": ["string", "null"]}));
            p.insert(
                "size".into(),
                json!({"anyOf": [{"type": "string", "enum": ["full", "half"]}, {"type": "null"}]}),
            );
        }
    }
    if let Some(Value::Object(p)) = s.get_mut("properties") {
        // 22 Oct: nested objects here pushed the draft's structured output past Claude's grammar limit ("The
        // compiled grammar is too large"); phases travel as JSON text (shape in the description) and are parsed
        // back by `json_text_object`.
        p.insert("phases".into(), as_json_text(json!({"type": ["object", "null"],
            "description": "the implementation plan's phases as a timeline, from order (0) to acceptance",
            "properties": {
                "time_unit": {"type": "string", "enum": ["day", "week", "month"]},
                "tasks": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}, "start": {"type": "number"}, "length": {"type": "number"},
                    "group": {"type": ["string", "null"]}}, "required": ["name", "start", "length"],
                    "additionalProperties": false}},
                "milestones": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}, "at": {"type": "number"}}, "required": ["name", "at"],
                    "additionalProperties": false}}},
            "required": ["time_unit", "tasks", "milestones"], "additionalProperties": false})));
    }
    s
}

/// The drafting rules' figure paragraph (visuals V2).
pub const DRAFT_FIGURE_RULES: &str = " Figures: give phases (the implementation plan as a timeline: tasks \
with start and length in one time unit, milestones such as acceptance); the timeline and the system \
architecture diagram are added from them and from the products, so do not ask for those two. Add a chart or \
diagram block (figure: what it must show, caption, size full or half) only where another figure helps the \
reader: a network diagram when sites and links matter, a process diagram for a workflow, a chart only with \
the customer's or the ERP's numbers. The figure maker may draw a custom_svg when no built-in kind fits. Never \
guess a number that is not in the brief.";

/// A figure the draft asked for, placed at `position` in `section` (blocks before it in that section).
#[derive(Debug, Clone)]
pub struct DraftFigure {
    pub section: String,
    pub position: i64,
    pub block_type: String,
    pub brief: String,
    pub caption: Option<String>,
    pub size: FigureSize,
}

/// What the draft gave for its figures.
#[derive(Debug, Clone, Default)]
pub struct DraftPlan {
    pub figures: Vec<DraftFigure>,
    /// {time_unit, tasks, milestones} from the model (also kept on the document: cover.phases).
    pub phases: Option<Value>,
    /// Facts for the architecture diagram (sections, sheet, deal products).
    pub context: String,
}

fn timeline_title(_language: &str) -> &'static str {
    "Project timeline"
}

fn architecture_title(_language: &str) -> &'static str {
    "System architecture"
}

/// The deterministic gantt spec of the draft's phases (source erp proposal.phases).
pub fn timeline_spec(phases: &Value, language: &str) -> Option<Value> {
    let tasks = phases
        .get("tasks")
        .and_then(Value::as_array)
        .filter(|t| !t.is_empty())?;
    Some(json!({
        "kind": "gantt", "title": timeline_title(language), "language": "en",
        "unit": null, "x_label": null, "y_label": null, "categories": [], "series": [],
        "tasks": tasks,
        "milestones": phases.get("milestones").cloned().unwrap_or_else(|| json!([])),
        "time_unit": phases.get("time_unit").cloned().unwrap_or_else(|| json!("week")),
        "source": {"type": "erp", "ref": "proposal.phases"}
    }))
}

/// The products in play for a deal: its quotations' catalog lines and its product interests.
pub async fn deal_products(db: &mut PgConnection, deal_id: Option<Uuid>) -> ApiResult<String> {
    let Some(deal_id) = deal_id else {
        return Ok(String::new());
    };
    let rows: Vec<(String, String, Option<rust_decimal::Decimal>)> = sqlx::query_as(
        "SELECT p.sku, p.name, SUM(qi.quantity) FROM quotation_items qi \
         JOIN quotations q ON q.id = qi.quotation_id JOIN product_catalog p ON p.id = qi.product_id \
         WHERE q.deal_id = $1 AND COALESCE(q.status, '') NOT IN ('superseded', 'rejected') \
         GROUP BY p.sku, p.name ORDER BY p.sku",
    )
    .bind(deal_id)
    .fetch_all(&mut *db)
    .await?;
    let interests: Option<Option<Vec<String>>> =
        sqlx::query_scalar("SELECT product_interests FROM deals WHERE id = $1")
            .bind(deal_id)
            .fetch_optional(&mut *db)
            .await?;
    let mut lines: Vec<String> = rows
        .into_iter()
        .map(|(sku, name, q)| {
            format!(
                "- {sku}: {name} x{}",
                q.map(|d| d.normalize().to_string())
                    .unwrap_or_else(|| "1".into())
            )
        })
        .collect();
    if let Some(i) = interests.flatten().filter(|i| !i.is_empty()) {
        lines.push(format!("- product interests on the deal: {}", i.join(", ")));
    }
    Ok(lines.join("\n"))
}

/// The section a default figure goes in: `prefer` when the template has it, else the first content section.
fn section_for(keys: &[String], prefer: &str) -> Option<String> {
    keys.iter()
        .find(|k| *k == prefer)
        .cloned()
        .or_else(|| keys.first().cloned())
}

/// Adds the draft's figures to a new proposal: the default figures (proposal.visuals.default_figures:
/// timeline, architecture) and the ones the draft asked for. Each failure is logged and told to the Owner,
/// never fatal. Commits after each figure. Returns one line per figure added or missed.
pub async fn draft_figures(
    db: &mut PgConnection,
    doc: &SalesDocument,
    plan: &DraftPlan,
    mut run: Option<&mut AgentRun>,
) -> ApiResult<Vec<String>> {
    let defaults = require_json(db, "proposal.visuals.default_figures", WHERE).await?;
    let defaults: Vec<String> = defaults
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let keys: Vec<String> = core::template(db)
        .await?
        .iter()
        .filter(|s| {
            !s.get("blocks")
                .and_then(Value::as_array)
                .is_some_and(|b| b.iter().any(|x| x == "commercial" || x == "stamp"))
        })
        .filter_map(|s| s.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();
    let lang = doc.language.clone();
    let mut jobs: Vec<(FigureRequest, String, Option<i64>, String)> = Vec::new(); // (req, section, position, label)
    if defaults.iter().any(|d| d == "timeline") {
        if let Some(section) = section_for(&keys, "implementation") {
            let start = plan.phases.as_ref().and_then(|p| timeline_spec(p, &lang));
            let brief = match &plan.phases {
                Some(p) => format!(
                    "{}: a gantt of the implementation phases from order to acceptance: {}",
                    timeline_title(&lang),
                    serde_json::to_string(p).unwrap_or_default()
                ),
                None => format!(
                    "{}: a gantt of the implementation plan from order to acceptance (phases, timing).",
                    timeline_title(&lang)
                ),
            };
            jobs.push((
                FigureRequest {
                    block_type: Some("chart".into()),
                    brief,
                    caption: Some(timeline_title(&lang).into()),
                    start_spec: start,
                    context: plan.context.clone(),
                    language: lang.clone(),
                    ..Default::default()
                },
                section,
                None,
                "timeline".into(),
            ));
        }
    }
    if defaults.iter().any(|d| d == "architecture") {
        // a template without an "architecture" section (the generic one) puts the diagram in "solution"
        let prefer = if keys.iter().any(|k| k == "architecture") {
            "architecture"
        } else {
            "solution"
        };
        if let Some(section) = section_for(&keys, prefer) {
            jobs.push((
                FigureRequest {
                    block_type: Some("diagram".into()),
                    brief: format!(
                        "{}: an architecture diagram of the proposed solution: the customer's site(s) and \
                         existing systems, the products proposed (with their catalog sku), where the \
                         processing runs, how the parts connect (link kinds) and what reaches the internet or \
                         the customer's systems.",
                        architecture_title(&lang)
                    ),
                    caption: Some(architecture_title(&lang).into()),
                    context: plan.context.clone(),
                    language: lang.clone(),
                    ..Default::default()
                },
                section,
                Some(0),
                "architecture".into(),
            ));
        }
    }
    for f in &plan.figures {
        jobs.push((
            FigureRequest {
                block_type: Some(f.block_type.clone()),
                brief: f.brief.clone(),
                caption: f.caption.clone(),
                size: f.size,
                context: plan.context.clone(),
                language: lang.clone(),
                ..Default::default()
            },
            f.section.clone(),
            Some(f.position),
            f.caption
                .clone()
                .unwrap_or_else(|| super::py_slice(&f.brief, 60).to_string()),
        ));
    }
    // the draft's own figures were counted without the default ones placed before them
    let mut added_in: Vec<(String, i64, i64)> = Vec::new(); // (section, at position, count)
    let mut lines = Vec::new();
    for (req, section, position, label) in jobs {
        let outcome = match make_figure(db, Some(doc), &req, run.as_deref_mut()).await {
            Ok(o) => o,
            Err(e) if super::governance::is_http_exception(&e) => {
                tracing::warn!(doc = %doc.id, error = %super::governance::http_detail(&e).unwrap_or_default(), "nary_figure_failed");
                lines.push(format!("{label}: failed"));
                continue;
            }
            Err(e) => return Err(e),
        };
        if let Some(drawn) = outcome.drawn.clone() {
            let shift: i64 = added_in
                .iter()
                .filter(|(s, at, _)| *s == section && position.is_some_and(|p| *at <= p))
                .map(|(_, _, n)| n)
                .sum();
            let pos = position.map(|p| p + shift);
            let inserted = figures::insert_figure(
                db,
                doc,
                NewFigure {
                    section: Some(section.clone()),
                    position: pos,
                    block_type: req.block_type.clone(),
                    caption: outcome.caption.clone(),
                    size: req.size,
                    brief: Some(req.brief.clone()),
                    checked: outcome.checked.clone(),
                    drawn: Some(drawn),
                    warnings: if outcome.ok {
                        vec![]
                    } else {
                        outcome.problems.clone()
                    },
                    ..Default::default()
                },
            )
            .await;
            match inserted {
                Ok(_) => {
                    added_in.push((section.clone(), position.unwrap_or(i64::MAX), 1));
                    lines.push(format!(
                        "{label}: added{}",
                        if outcome.ok { "" } else { " with warnings" }
                    ));
                }
                Err(e) if super::governance::is_http_exception(&e) => {
                    tracing::warn!(doc = %doc.id, error = %super::governance::http_detail(&e).unwrap_or_default(), "nary_figure_insert_failed");
                    lines.push(format!("{label}: failed"));
                    continue;
                }
                Err(e) => return Err(e),
            }
        } else {
            lines.push(format!("{label}: not added"));
        }
        if !outcome.ok {
            tell_owner(db, doc, &label, &outcome).await?;
        }
        super::commit(db).await?;
    }
    Ok(lines)
}

// ----------------------------------------------------------------------------- editor and chat changes

async fn figure_block(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
) -> ApiResult<granary_db::rows::documents::SalesDocumentBlock> {
    let b = core::blocks_of(db, doc.id)
        .await?
        .into_iter()
        .find(|b| b.id == block_id)
        .ok_or_else(|| {
            ApiError::http(
                404,
                "That block is not in this proposal (it may have been saved since)",
            )
        })?;
    if !figures::is_figure(&b.block_type) {
        return Err(ApiError::http(422, "That block is not a chart or diagram"));
    }
    Ok(b)
}

fn editable(doc: &SalesDocument) -> ApiResult<()> {
    if core::EDITABLE.contains(&doc.status.as_str()) {
        return Ok(());
    }
    Err(ApiError::http(
        409,
        format!(
            "A {} proposal can't be edited: make a revision first",
            doc.status
        ),
    ))
}

/// Runs the pipeline for a change to an existing figure (nothing stored): the instruction applied to its
/// spec. V3's change card shows the outcome; [`apply_outcome`] stores it.
pub async fn propose_change(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
    instruction: Option<&str>,
    new_spec: Option<Value>,
    run: Option<&mut AgentRun>,
) -> ApiResult<(FigureRequest, FigureOutcome)> {
    editable(doc)?;
    let b = figure_block(db, doc, block_id).await?;
    let d = &b.data;
    let brief = d
        .get("brief")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| d.get("caption").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let size = if d.get("size").and_then(Value::as_str) == Some("half") {
        FigureSize::Half
    } else {
        FigureSize::Full
    };
    let req = FigureRequest {
        block_type: if new_spec.is_some() {
            None
        } else {
            Some(b.block_type.clone())
        },
        brief: match instruction {
            Some(i) if !brief.is_empty() => format!("{brief}\nChanged: {i}"),
            Some(i) => i.to_string(),
            None => brief,
        },
        caption: d
            .get("caption")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        size,
        start_spec: new_spec.or_else(|| d.get("spec").cloned()),
        instruction: instruction
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty()),
        context: String::new(),
        language: doc.language.clone(),
    };
    let outcome = make_figure(db, Some(doc), &req, run).await?;
    Ok((req, outcome))
}

/// Stores an outcome on an existing figure block (keeps its place); returns the block.
pub async fn apply_outcome(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
    req: &FigureRequest,
    outcome: &FigureOutcome,
) -> ApiResult<Value> {
    let Some(drawn) = outcome.drawn.clone() else {
        return Err(ApiError::http(
            422,
            format!(
                "Nary couldn't draw that figure: {}",
                outcome.problems.join("; ")
            ),
        ));
    };
    figures::update_figure(
        db,
        doc,
        block_id,
        FigureChange {
            caption: Some(outcome.caption.clone()).filter(|c| !c.is_empty()),
            size: Some(req.size),
            brief: Some(req.brief.clone()).filter(|b| !b.is_empty()),
            checked: outcome.checked.clone(),
            drawn: Some(drawn),
            warnings: if outcome.ok {
                vec![]
            } else {
                outcome.problems.clone()
            },
            ..Default::default()
        },
    )
    .await
}

/// "Ask Nary to change this figure" (editor): the change made and stored at once (the Owner asked for it in
/// the editor; Nary's own chat changes go through a card). {block, ok, problems, checked}.
pub async fn change_by_instruction(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
    instruction: &str,
) -> ApiResult<Value> {
    if instruction.trim().is_empty() {
        return Err(ApiError::http(400, "Say what to change"));
    }
    let doc_ref = doc.id.to_string();
    let mut run =
        super::runs::start_run(db, "manual", SPEC_JOB, Some(&doc_ref), "internal", None).await?;
    let (req, outcome) =
        propose_change(db, doc, block_id, Some(instruction), None, Some(&mut run)).await?;
    let result = apply_outcome(db, doc, block_id, &req, &outcome).await;
    let (status, note) = match &result {
        Ok(_) if outcome.ok => ("done", "figure changed".to_string()),
        Ok(_) => (
            "done",
            format!(
                "figure changed with warnings: {}",
                outcome.problems.join("; ")
            ),
        ),
        Err(_) => (
            "failed",
            format!("figure not changed: {}", outcome.problems.join("; ")),
        ),
    };
    super::runs::finish_run(
        db,
        &mut run,
        status,
        Some(super::py_slice(&note, 500)),
        None,
    )
    .await?;
    let block = result?;
    Ok(
        json!({"block": block, "ok": outcome.ok, "problems": outcome.problems, "checked": outcome.checked}),
    )
}

/// "Redraw" (editor): guard, draw and check the figure's current spec again (fixing it when the check
/// finds problems). {block, ok, problems, checked}.
pub async fn redraw(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_id: Uuid,
) -> ApiResult<Value> {
    let doc_ref = doc.id.to_string();
    let mut run =
        super::runs::start_run(db, "manual", SPEC_JOB, Some(&doc_ref), "internal", None).await?;
    let (req, outcome) = propose_change(db, doc, block_id, None, None, Some(&mut run)).await?;
    let result = apply_outcome(db, doc, block_id, &req, &outcome).await;
    let status = if result.is_ok() { "done" } else { "failed" };
    super::runs::finish_run(db, &mut run, status, Some("figure redrawn"), None).await?;
    let block = result?;
    Ok(
        json!({"block": block, "ok": outcome.ok, "problems": outcome.problems, "checked": outcome.checked}),
    )
}

/// The editor's "Check" button (`POST /visuals/check`): guard, draw and look at a spec, nothing saved.
/// {ok, notes, at, warnings, png_base64}; a guard refusal is a 422.
pub async fn check_spec(
    db: &mut PgConnection,
    doc: &SalesDocument,
    block_type: Option<&str>,
    spec: &Value,
    brief: &str,
    size: FigureSize,
) -> ApiResult<Value> {
    use base64::Engine as _;
    let drawn = figures::draw(db, Some(doc), block_type, spec, size).await?;
    let brief = if brief.trim().is_empty() {
        drawn.spec.title().to_string()
    } else {
        brief.to_string()
    };
    let doc_ref = doc.id.to_string();
    let mut run =
        super::runs::start_run(db, "manual", CHECK_JOB, Some(&doc_ref), "internal", None).await?;
    let verdict = vision_check(
        db,
        &drawn.png,
        &brief,
        &drawn.spec.to_value(),
        Some(&mut run),
    )
    .await?;
    super::runs::finish_run(db, &mut run, "done", Some("figure checked"), None).await?;
    let mut v = verdict;
    v["warnings"] = json!(drawn.warnings);
    v["png_base64"] = json!(base64::engine::general_purpose::STANDARD.encode(&drawn.png));
    Ok(v)
}
