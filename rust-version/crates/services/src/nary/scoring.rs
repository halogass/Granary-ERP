//! Port of backend/app/services/nary/scoring.py.
//!
//! BANT scoring (Nary Phase 3 item A).
//!
//! Haiku (job nary.score) scores Budget, Authority, Need and Timeline 0-3 each from the conversation,
//! against the rubric in nary.score.rubric, and names the company Fit red and green flags it sees. In the
//! same call it says what the customer's latest message asks for (the intent the reply playbook acts
//! on) and which product family it is about. Code adds the total and the class (nary.score.classes).
//!
//! The cascade is a rule, not the model's confidence: a total within nary.score.boundary_margin of a
//! class boundary is scored again by Sonnet (nary.cascade_review), and a refusal goes there too.
//!
//! A lead that becomes Hot interrupts the Owner (working hours only; otherwise the morning brief).

use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json, SettingError};
use granary_db::rows::crm::{Deal, Lead};
use granary_db::rows::nary::{AgentRun, NaryScore};

use crate::ai::base::ChatResult;
use crate::whatsapp::py_truthy;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

pub const WHERE: &str = "Nary & AI";

pub const INTENTS: &[&str] = &[
    "question",
    "price",
    "quotation",
    "detailed",
    "meeting",
    "slot_pick",
    "accept",
    "reject",
    "objection",
    "other",
];
pub const PARTS: &[&str] = &["budget", "authority", "need", "timeline"];

/// `nary.score.classes` as Python's dict: class -> (lowest, highest), in the setting's key order.
pub type ClassTable = Vec<(String, (i64, i64))>;

/// `_questions_keys(db)`: the product families of nary.reply.questions, in order.
pub async fn questions_keys(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    let q = require_json(db, "nary.reply.questions", WHERE).await?;
    match q.as_object() {
        Some(m) if !m.is_empty() => Ok(m.keys().cloned().collect()),
        _ => Err(SettingError::new(
            "nary.reply.questions",
            "must be {product: {id: [q1, q2], en: [q1, q2]}}",
            WHERE,
        )
        .into()),
    }
}

pub fn schema(products: &[String]) -> Value {
    let part = json!({"type": "integer", "enum": [0, 1, 2, 3]});
    let mut product_enum: Vec<Value> = products.iter().map(|p| Value::String(p.clone())).collect();
    product_enum.push(Value::String("unknown".into()));
    let mut props = serde_json::Map::new();
    for p in PARTS {
        props.insert(p.to_string(), part.clone());
    }
    props.insert(
        "red_flags".into(),
        json!({"type": "array", "items": {"type": "string"}}),
    );
    props.insert(
        "green_flags".into(),
        json!({"type": "array", "items": {"type": "string"}}),
    );
    props.insert("disqualify".into(), json!({"type": "boolean"}));
    props.insert("rationale".into(), json!({"type": "string"}));
    props.insert("intent".into(), json!({"type": "string", "enum": INTENTS}));
    props.insert(
        "product".into(),
        json!({"type": "string", "enum": product_enum}),
    );
    let mut required: Vec<Value> = PARTS.iter().map(|p| Value::String(p.to_string())).collect();
    for k in [
        "red_flags",
        "green_flags",
        "disqualify",
        "rationale",
        "intent",
        "product",
    ] {
        required.push(Value::String(k.into()));
    }
    json!({
        "type": "object",
        "properties": Value::Object(props),
        "required": required,
        "additionalProperties": false,
    })
}

/// Why Python's `int(x)` / `x[i]` failed: TypeError and ValueError and IndexError are caught by `classes`,
/// a KeyError (indexing a dict with 0) is not.
#[derive(Debug, PartialEq)]
pub(crate) enum PyFail {
    Caught,
    KeyError,
}

/// `int(v)` for a JSON value (bool, int, float truncated toward zero, a decimal string).
pub(crate) fn py_int(v: &Value) -> Result<i64, PyFail> {
    match v {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => {
            let t = n.to_string();
            if !t.contains(['.', 'e', 'E']) {
                return t.parse::<i64>().map_err(|_| PyFail::Caught);
            }
            let f: f64 = t.parse().map_err(|_| PyFail::Caught)?;
            if !f.is_finite() {
                return Err(PyFail::Caught);
            }
            Ok(f.trunc() as i64)
        }
        Value::String(s) => py_int_str(s).ok_or(PyFail::Caught),
        _ => Err(PyFail::Caught),
    }
}

/// `int(str)`: surrounding whitespace, a sign, decimal digits (any script) with single underscores between.
fn py_int_str(s: &str) -> Option<i64> {
    use granary_compat::text::pystr;
    let t = pystr::strip(s);
    let (neg, body) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut n: i64 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = pystr::decimal_value(c)? as i64;
        n = n.checked_mul(10)?.checked_add(d)?;
    }
    Some(if neg { -n } else { n })
}

/// `v[i]` for a JSON value (list element, or one character of a string).
fn py_index(v: &Value, i: usize) -> Result<Value, PyFail> {
    match v {
        Value::Array(a) => a.get(i).cloned().ok_or(PyFail::Caught),
        Value::String(s) => s
            .chars()
            .nth(i)
            .map(|c| Value::String(c.to_string()))
            .ok_or(PyFail::Caught),
        Value::Object(_) => Err(PyFail::KeyError),
        _ => Err(PyFail::Caught),
    }
}

/// `classes(db)`: the class table, or SettingError when it isn't {class: [lowest, highest]}.
pub async fn classes(db: &mut PgConnection) -> ApiResult<ClassTable> {
    let raw = require_json(db, "nary.score.classes", WHERE).await?;
    classes_from(&raw)
}

pub fn classes_from(raw: &Value) -> ApiResult<ClassTable> {
    let bad = || -> ApiError {
        SettingError::new(
            "nary.score.classes",
            "must be {class: [lowest, highest]}",
            WHERE,
        )
        .into()
    };
    let Some(map) = raw.as_object() else {
        return Err(bad());
    };
    let mut out = ClassTable::new();
    for (k, v) in map {
        let pair = (|| -> Result<(i64, i64), PyFail> {
            let lo = py_int(&py_index(v, 0)?)?;
            let hi = py_int(&py_index(v, 1)?)?;
            Ok((lo, hi))
        })();
        match pair {
            Ok(p) => out.push((k.clone(), p)),
            // KeyError is not in the caught tuple: Python answers 500.
            Err(PyFail::KeyError) => {
                return Err(ApiError::internal(anyhow::anyhow!("KeyError: 0")));
            }
            Err(PyFail::Caught) => return Err(bad()),
        }
    }
    if out.is_empty() {
        return Err(SettingError::new("nary.score.classes", "is empty", WHERE).into());
    }
    Ok(out)
}

pub fn classify(total: i64, table: &[(String, (i64, i64))]) -> String {
    for (name, (lo, hi)) in table {
        if *lo <= total && total <= *hi {
            return name.clone();
        }
    }
    // min() keeps the first of equal keys.
    let mut best = &table[0];
    for item in table.iter().skip(1) {
        if item.1 .0 < best.1 .0 {
            best = item;
        }
    }
    best.0.clone()
}

/// True when the total is within `margin` points of the edge between two classes.
pub fn near_boundary(total: i64, table: &[(String, (i64, i64))], margin: i64) -> bool {
    let mut edges: Vec<i64> = table.iter().map(|(_, (lo, _))| *lo).collect();
    edges.sort();
    // each class's lowest total, but the bottom one
    edges
        .iter()
        .skip(1)
        .any(|edge| (total as f64 - (*edge as f64 - 0.5)).abs() <= margin as f64 - 0.5)
}

/// The fixed scoring instructions, from the rubric and flag settings.
pub async fn instructions(db: &mut PgConnection) -> ApiResult<String> {
    let rubric = require_json(db, "nary.score.rubric", WHERE).await?;
    let red = require_json(db, "nary.score.red_flags", WHERE).await?;
    let green = require_json(db, "nary.score.green_flags", WHERE).await?;
    Ok(instructions_from(&rubric, &red, &green))
}

pub fn instructions_from(rubric: &Value, red: &Value, green: &Value) -> String {
    let dump = |v: &Value, opts: DumpOpts| {
        dumps(&PyValue::from(v), &opts).expect("json.dumps of a JSON value")
    };
    format!(
        "Score this sales conversation with BANT. For each part give the points of the rubric \
line that the conversation supports; with no evidence, give 0. Do not guess in the customer's favour.\n\
RUBRIC (points \u{2192} meaning):\n{}\n\
COMPANY FIT red flags: {}\n\
COMPANY FIT green flags: {}\n\
Name only flags the conversation shows. disqualify: true only for the rubric's disqualify case or a clear \
red flag.\n\
intent: what the customer's LAST message asks for: question (about a product or the company), price (asks what \
something costs), quotation (asks for a formal quotation), detailed (describes a project, site or requirements in detail, or a tender/RFP), \
meeting (asks for a call or demo), slot_pick (answers which offered meeting time suits them), accept \
(accepts a quotation or says they will order), reject (declines), objection (pushes back on price, \
timing or terms), other.\n\
product: the product family the conversation is about. rationale: two sentences, plain.",
        dump(rubric, DumpOpts::unicode().indent(1)),
        dump(red, DumpOpts::unicode()),
        dump(green, DumpOpts::unicode()),
    )
}

/// `latest(db, *, lead_id=None, deal_id=None)`: the newest score of the deal (when given) else the lead.
pub async fn latest(
    db: &mut PgConnection,
    lead_id: Option<Uuid>,
    deal_id: Option<Uuid>,
) -> ApiResult<Option<NaryScore>> {
    if lead_id.is_none() && deal_id.is_none() {
        return Ok(None);
    }
    let (col, id) = match deal_id {
        Some(d) => ("deal_id", d),
        None => ("lead_id", lead_id.unwrap()),
    };
    Ok(sqlx::query_as::<_, NaryScore>(&format!(
        "SELECT * FROM nary_scores WHERE {col} = $1 ORDER BY created_at DESC LIMIT 1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

/// `score(db, *, transcript, lead=None, deal=None, chat_id=None, run=None, context="")`: score and store.
/// Returns None when the model gave nothing usable. `lead.ai_score` is updated in the database and on the
/// struct, as Python updates the loaded Lead (a Deal has no ai_score: `hasattr` skips it).
///
/// 3 Nov 2026: the two halves are [`assess`] (the model's scoring, nothing stored) and [`store`] (the row, the
/// lead's ai_score, the Hot interrupt), so the MCP client's score_lead can show a score without saving it.
#[allow(clippy::too_many_arguments)]
pub async fn score(
    db: &mut PgConnection,
    transcript: &str,
    lead: Option<&mut Lead>,
    deal: Option<&Deal>,
    chat_id: Option<&str>,
    run: Option<&mut AgentRun>,
    context: &str,
) -> ApiResult<Option<NaryScore>> {
    let Some(a) = assess(db, transcript, run, context).await? else {
        return Ok(None);
    };
    store(db, &a, lead, deal, chat_id).await.map(Some)
}

/// What [`assess`] read from the model: the BANT parts, the total and class, flags and the rest.
#[derive(Debug, Clone)]
pub struct Assessment {
    /// (part, 0-3) in [`PARTS`] order
    pub parts: Vec<(&'static str, i64)>,
    pub total: i64,
    pub class: String,
    pub red_flags: Value,
    pub green_flags: Value,
    pub rationale: String,
    pub intent: String,
    pub product: Option<String>,
    pub model: Option<String>,
    pub cascaded: bool,
    /// the model suggests disqualifying ("disqualify suggested" is added to the red flags when stored)
    pub disqualify: bool,
    pub run_id: Option<Uuid>,
}

/// The model's BANT scoring of a transcript (job nary.score, the cascade near a class boundary). Nothing is
/// stored. None when the model gave nothing usable.
pub async fn assess(
    db: &mut PgConnection,
    transcript: &str,
    run: Option<&mut AgentRun>,
    context: &str,
) -> ApiResult<Option<Assessment>> {
    use crate::nary::{llm, persona};
    let table = classes(db).await?;
    let margin = require_int(db, "nary.score.boundary_margin", WHERE).await?;
    let products = questions_keys(db).await?;

    let esc_table = table.clone();
    let escalate = move |result: &ChatResult| -> Option<String> {
        let text = if result.text.is_empty() {
            ""
        } else {
            result.text.as_str()
        };
        let Ok(data) = granary_core::validation::json_loads(text.as_bytes()).map(|o| o.to_json())
        else {
            return Some("unreadable".into());
        };
        // `data.get(p)` on a non-object is an AttributeError in Python (not caught): read as unreadable here.
        let Value::Object(data) = data else {
            return Some("unreadable".into());
        };
        let mut total = 0i64;
        for p in PARTS {
            match data.get(*p).filter(|v| py_truthy(v)) {
                None => {}
                Some(v) => match py_int(v) {
                    Ok(n) => total += n,
                    Err(_) => return Some("unreadable".into()),
                },
            }
        }
        if near_boundary(total, &esc_table, margin) {
            Some(format!("total {total} is near a class boundary"))
        } else {
            None
        }
    };

    let content = (if context.is_empty() {
        String::new()
    } else {
        format!("{context}\n\n")
    }) + "CONVERSATION:\n"
        + &llm::information_default("customer", transcript);
    let system = persona::system_prompt("staff", &instructions(db).await?, "", "");
    let run_id = run.as_ref().map(|r| r.id);
    let data = llm::ask_json(
        db,
        "nary.score",
        &system,
        &content,
        &schema(&products),
        run,
        Some("nary.cascade_review"),
        Some(&escalate),
    )
    .await?;
    let Some(data) = data else {
        return Ok(None);
    };
    let part = |p: &str| -> ApiResult<i64> {
        match data.get(p).filter(|v| py_truthy(v)) {
            None => Ok(0),
            Some(v) => py_int(v)
                .map(|n| n.clamp(0, 3))
                .map_err(|_| ApiError::internal(anyhow::anyhow!("ValueError: int({v})"))),
        }
    };
    let parts: Vec<(&str, i64)> = PARTS
        .iter()
        .map(|p| part(p).map(|n| (*p, n)))
        .collect::<ApiResult<_>>()?;
    let total: i64 = parts.iter().map(|(_, n)| n).sum();
    let klass = classify(total, &table);
    let flags = |k: &str| match data.get(k) {
        Some(v) if py_truthy(v) => v.clone(),
        _ => json!([]),
    };
    let rationale = match data.get("rationale") {
        Some(Value::String(s)) => crate::nary::py_slice(s, 2000).to_string(),
        Some(v) if py_truthy(v) => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "TypeError: rationale is not a string"
            )))
        }
        _ => String::new(),
    };
    let intent = match data.get("intent").and_then(|v| v.as_str()) {
        Some(i) if INTENTS.contains(&i) => i.to_string(),
        _ => "other".to_string(),
    };
    let product = match data.get("product") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s == "unknown" => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
    };
    let model = data
        .get("_model")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let cascaded = data.get("_escalated").is_some_and(py_truthy);
    Ok(Some(Assessment {
        parts,
        total,
        class: klass,
        red_flags: flags("red_flags"),
        green_flags: flags("green_flags"),
        rationale,
        intent,
        product,
        model,
        cascaded,
        disqualify: data.get("disqualify").is_some_and(py_truthy),
        run_id,
    }))
}

/// Stores an [`assess`] result as Nary does: the nary_scores row, the lead's ai_score (scored_by nary) and the
/// Owner's Hot interrupt when the class becomes hot.
pub async fn store(
    db: &mut PgConnection,
    a: &Assessment,
    mut lead: Option<&mut Lead>,
    deal: Option<&Deal>,
    chat_id: Option<&str>,
) -> ApiResult<NaryScore> {
    let (parts, total, klass, rationale) = (&a.parts, a.total, &a.class, &a.rationale);
    let previous = latest(db, lead.as_ref().map(|l| l.id), deal.map(|d| d.id)).await?;
    let mut row = sqlx::query_as::<_, NaryScore>(
        "INSERT INTO nary_scores (id, lead_id, deal_id, chat_id, budget, authority, need, timeline, total, class, \
         red_flags, green_flags, rationale, intent, product, model, cascaded, run_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(lead.as_ref().map(|l| l.id))
    .bind(deal.map(|d| d.id))
    .bind(chat_id)
    .bind(parts[0].1 as i32)
    .bind(parts[1].1 as i32)
    .bind(parts[2].1 as i32)
    .bind(parts[3].1 as i32)
    .bind(total as i32)
    .bind(klass)
    .bind(&a.red_flags)
    .bind(&a.green_flags)
    .bind(rationale)
    .bind(&a.intent)
    .bind(&a.product)
    .bind(&a.model)
    .bind(a.cascaded)
    .bind(a.run_id)
    .fetch_one(&mut *db)
    .await?;
    if let Some(lead) = lead.as_deref_mut() {
        let mut ai = match &lead.ai_score {
            Some(Value::Object(m)) => m.clone(),
            _ => serde_json::Map::new(),
        };
        let bant: serde_json::Map<String, Value> = parts
            .iter()
            .map(|(p, n)| (p.to_string(), json!(n)))
            .collect();
        ai.insert("score".into(), json!(total));
        ai.insert("temperature".into(), json!(klass));
        ai.insert("bant".into(), Value::Object(bant));
        ai.insert("rationale".into(), json!(rationale));
        ai.insert("scored_by".into(), json!("nary"));
        let ai = Value::Object(ai);
        sqlx::query("UPDATE leads SET ai_score = $2, updated_at = now() WHERE id = $1")
            .bind(lead.id)
            .bind(&ai)
            .execute(&mut *db)
            .await?;
        lead.ai_score = Some(ai);
    }
    if klass == "hot" && previous.as_ref().is_none_or(|p| p.class != "hot") {
        hot_interrupt(db, &row, lead.as_deref(), deal).await?;
    }
    if a.disqualify {
        let mut red = match &row.red_flags {
            Value::Array(a) => a.clone(),
            _ => vec![],
        };
        red.push(json!("disqualify suggested"));
        row = sqlx::query_as::<_, NaryScore>(
            "UPDATE nary_scores SET red_flags = $2, updated_at = now() WHERE id = $1 RETURNING *",
        )
        .bind(row.id)
        .bind(Value::Array(red))
        .fetch_one(&mut *db)
        .await?;
    }
    Ok(row)
}

/// `_hot_interrupt(db, row, lead, deal)`.
async fn hot_interrupt(
    db: &mut PgConnection,
    row: &NaryScore,
    lead: Option<&Lead>,
    deal: Option<&Deal>,
) -> ApiResult<()> {
    use crate::nary::{owner_channel, rhythm};
    if !owner_channel::enabled(db).await? && !rhythm::is_working_time(db, None).await? {
        return Ok(()); // the morning brief carries it
    }
    let raw_title: Option<String> = match (deal, lead) {
        (Some(d), _) => Some(d.title.clone()),
        (None, Some(l)) => l.title.clone(),
        (None, None) => Some("a conversation".into()),
    };
    let title = raw_title
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "a conversation".into());
    let link = match (deal, lead) {
        (Some(d), _) => format!("/crm/deals/{}", d.id),
        (None, Some(l)) => format!("/crm/leads/{}", l.id),
        _ => "/nary".into(),
    };
    let entity_id = match (deal, lead) {
        (Some(d), _) => d.id.to_string(),
        (None, Some(l)) => l.id.to_string(),
        _ => row.id.to_string(),
    };
    let ref_id = deal.map(|d| d.id).or(lead.map(|l| l.id));
    let full_title = format!("Hot lead: {title} (BANT {}/12)", row.total);
    let body = row.rationale.clone().unwrap_or_default();
    let mut tell = owner_channel::Tell::new(
        &full_title,
        &body,
        &link,
        if deal.is_some() { "deal" } else { "lead" },
        &entity_id,
        "nary_urgent",
    );
    tell.priority = "high";
    tell.urgent_kind = Some("hot_lead");
    tell.facts = Some(
        json!({"bant": [row.budget, row.authority, row.need, row.timeline], "class": row.class}),
    );
    tell.ref_type = Some(if deal.is_some() { "deal" } else { "lead" });
    tell.ref_id = ref_id;
    owner_channel::tell(db, tell).await?;
    Ok(())
}
