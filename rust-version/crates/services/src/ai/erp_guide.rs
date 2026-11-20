//! Port of backend/app/services/ai/erp_guide.py: the ERP guide, how Granary works, for the AI chat
//! (Nary), built from the code.
//!
//! Two kinds of section:
//! - narrative: what each module is for and how work flows through it, written once in the repo
//!   (`erp_guide/<key>.md`, first line "# Title", second paragraph = the one-line summary). The 15 files are
//!   byte-identical copies of backend/app/services/ai/erp_guide/*.md, embedded at build time;
//! - generated: roles and permissions (the PERMISSIONS / ROLE_PERMISSIONS data the seed carries, generated
//!   from core/permissions.py), every CRM picklist (crm::stages::registry), the fields a change card may
//!   write (crm::writes), the settings that hold business numbers (system_settings labels), and the API map.
//!
//! The API map: Python builds it from FastAPI's `app.openapi()`. The Rust services crate has no router
//! table (routers are wave 4), so the section text is a snapshot recorded from the Python app by
//! compat-tests/golden/gen_ai_a4.py (`erp_guide/api_map.txt`); the wave-4 wire-up may replace it with the
//! live route table.
//!
//! sync() keeps one AIKnowledgeDoc per section (system_key "erp_guide:<key>"), reindexed only when its
//! text changes; sections that disappear are deleted.

use std::collections::{BTreeSet, HashMap, HashSet};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{str_value, PyValue};
use granary_compat::text::patterns::guide_area_split;
use granary_compat::text::pystr;
use granary_core::errors::ApiError;
use granary_core::settings_store::get_optional;

pub const WHERE: &str = "Nary & AI → AI chat";
pub const KEY_PREFIX: &str = "erp_guide:";

/// Which narrative section explains each permission module (core/permissions.py "module:action").
pub const MODULE_SECTIONS: [(&str, &str); 20] = [
    ("system", "settings-and-roles"),
    ("crm", "crm"),
    ("sales", "sales"),
    ("finance", "finance"),
    ("projects", "projects"),
    ("rnd", "projects"),
    ("hr", "hr"),
    ("manufacturing", "manufacturing"),
    ("inventory", "manufacturing"),
    ("marketing", "marketing"),
    ("partner_mgmt", "partners-and-vendors"),
    ("vendor_mgmt", "partners-and-vendors"),
    ("kb", "knowledge-and-ai"),
    ("nary", "nary"),
    ("approvals", "approvals-and-signing"),
    ("verification", "approvals-and-signing"),
    ("sign", "approvals-and-signing"),
    ("fleet", "operations"),
    ("maintenance", "operations"),
    ("helpdesk", "operations"),
];

/// `MODULE_SECTIONS.get(module)`.
pub fn module_section(module: &str) -> Option<&'static str> {
    MODULE_SECTIONS
        .iter()
        .find(|(m, _)| *m == module)
        .map(|(_, s)| *s)
}

/// The narrative files (stem, text), as `DIR.glob("*.md")` finds them.
const NARRATIVE_FILES: [(&str, &str); 14] = [
    (
        "approvals-and-signing",
        include_str!("erp_guide/approvals-and-signing.md"),
    ),
    ("crm", include_str!("erp_guide/crm.md")),
    ("finance", include_str!("erp_guide/finance.md")),
    ("hr", include_str!("erp_guide/hr.md")),
    (
        "knowledge-and-ai",
        include_str!("erp_guide/knowledge-and-ai.md"),
    ),
    ("manufacturing", include_str!("erp_guide/manufacturing.md")),
    ("marketing", include_str!("erp_guide/marketing.md")),
    ("nary", include_str!("erp_guide/nary.md")),
    ("operations", include_str!("erp_guide/operations.md")),
    ("overview", include_str!("erp_guide/overview.md")),
    (
        "partners-and-vendors",
        include_str!("erp_guide/partners-and-vendors.md"),
    ),
    ("projects", include_str!("erp_guide/projects.md")),
    ("sales", include_str!("erp_guide/sales.md")),
    (
        "settings-and-roles",
        include_str!("erp_guide/settings-and-roles.md"),
    ),
];

/// The API map section text recorded from the Python app (see the module docs).
const API_MAP: &str = include_str!("erp_guide/api_map.txt");

/// PERMISSIONS / ROLE_PERMISSIONS (core/permissions.py), as the seed carries them.
#[derive(Deserialize)]
struct PermData {
    permissions: Vec<(String, String)>,
    role_permissions: Vec<(String, Vec<String>)>,
}

static PERMS: Lazy<PermData> = Lazy::new(|| {
    serde_json::from_str(include_str!("../../../db/src/seed/seed_data.json"))
        .expect("seed_data.json permissions")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub key: String,
    pub title: String,
    pub summary: String,
    pub content: String,
}

impl Section {
    fn new(key: &str, title: &str, summary: &str, content: String) -> Section {
        Section {
            key: key.into(),
            title: title.into(),
            summary: summary.into(),
            content,
        }
    }

    /// `sha256(f"{title}\n{content}")` hex.
    pub fn digest(&self) -> String {
        hex::encode(Sha256::digest(
            format!("{}\n{}", self.title, self.content).as_bytes(),
        ))
    }
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `_parse(key, text)`.
pub fn parse(key: &str, text: &str) -> Section {
    let lines = pystr::splitlines(pystr::strip(text));
    let title = match lines.first() {
        Some(l) if l.starts_with('#') => pystr::strip(l.trim_start_matches(['#', ' '])).to_string(),
        _ => key.to_string(),
    };
    let body =
        pystr::strip(&lines.iter().skip(1).copied().collect::<Vec<_>>().join("\n")).to_string();
    let summary = body
        .split("\n\n")
        .find(|p| {
            let s = pystr::strip(p);
            !s.is_empty() && !(s.starts_with('#') || s.starts_with('-') || s.starts_with('|'))
        })
        .map(|p| pystr::strip(p).replace('\n', " "))
        .unwrap_or_default();
    Section {
        key: key.into(),
        title,
        summary: take_chars(&summary, 240),
        content: body,
    }
}

/// The hand-written sections, in file-name order.
pub fn narrative() -> Vec<Section> {
    let mut files: Vec<(&str, &str)> = NARRATIVE_FILES.to_vec();
    files.sort_by(|a, b| a.0.cmp(b.0));
    files.into_iter().map(|(k, t)| parse(k, t)).collect()
}

// ─── Generated sections ───────────────────────────────────────────────────────

fn module_of(code: &str) -> &str {
    code.split(':').next().unwrap_or("")
}

/// `_roles_section()`.
pub fn roles_section() -> Section {
    let mut modules: Vec<(String, Vec<String>)> = Vec::new();
    for (code, label) in &PERMS.permissions {
        let m = module_of(code);
        let line = format!("- `{code}`: {label}");
        match modules.iter_mut().find(|(k, _)| k == m) {
            Some((_, v)) => v.push(line),
            None => modules.push((m.to_string(), vec![line])),
        }
    }
    let mut out: Vec<String> = vec![
        "Every action is gated by a permission code `module:action`. A user has roles; a role is a set of codes. The superadmin has everything. Nary and its tools act with the permissions of the person they act for, never more.".into(),
        String::new(),
        "## Roles".into(),
    ];
    for (role, codes) in &PERMS.role_permissions {
        let mods: BTreeSet<&str> = codes.iter().map(|c| module_of(c)).collect();
        let mods: Vec<&str> = mods.into_iter().collect();
        out.push(format!(
            "- **{role}**: {} permissions in {}",
            codes.len(),
            if mods.is_empty() {
                "none".to_string()
            } else {
                mods.join(", ")
            }
        ));
    }
    for (m, lines) in modules {
        out.push(String::new());
        out.push(format!("## {m}"));
        out.extend(lines);
    }
    Section::new(
        "permissions",
        "Roles and permission codes",
        "Which roles exist and every permission code (module:action) with what it allows.",
        out.join("\n"),
    )
}

/// f-string `{value}` of a JSON value (`str()`; a missing key prints "None").
fn s(v: Option<&Value>) -> String {
    match v {
        None => "None".into(),
        Some(v) => str_value(&PyValue::from(v)),
    }
}

/// `str(r.get('label') or '')`.
fn label_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(v) if crate::crm::attributes::py::truthy(v) => s(Some(v)),
        _ => String::new(),
    }
}

fn arr(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

/// `', '.join(values)`.
fn join(v: &Value) -> String {
    arr(v)
        .iter()
        .map(|x| s(Some(x)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn items(rows: &Value) -> String {
    arr(rows)
        .iter()
        .filter(|r| r.is_object())
        .map(|r| {
            format!(
                "- `{}`: {}",
                s(r.get("key")),
                label_or_empty(r.get("label"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `_crm_vocab_section(db)`.
pub async fn crm_vocab_section(db: &mut PgConnection) -> Result<Section, ApiError> {
    let reg = crate::crm::stages::registry(db).await?;
    let deal_stages = arr(&reg["deal_stages"])
        .iter()
        .map(|st| {
            format!(
                "- `{}`: {}, probability {}%",
                s(st.get("key")),
                s(st.get("label")),
                s(st.get("probability"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let lead_statuses = arr(&reg["lead_statuses"])
        .iter()
        .map(|st| format!("- `{}`: {}", s(st.get("key")), s(st.get("label"))))
        .collect::<Vec<_>>()
        .join("\n");
    let moves = reg["lead_transitions"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k} → {}", join(v)))
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    let out: Vec<String> = vec![
        "The allowed values (KEYS) of every CRM picklist, read from settings when this guide was built. Cards and the API take the key, never the label.".into(),
        String::new(),
        "## Deal stages (in order)".into(),
        deal_stages,
        String::new(),
        "## Lead statuses".into(),
        lead_statuses,
        format!("A new lead starts as one of: {}.", join(&reg["lead_start_statuses"])),
        format!("Allowed lead moves: {moves}"),
        String::new(),
        "## Lead sources".into(),
        items(&reg["lead_sources"]),
        String::new(),
        "## Lead priorities".into(),
        items(&reg["lead_priorities"]),
        String::new(),
        "## Loss reasons (a lost deal needs at least one)".into(),
        items(&reg["loss_reasons"]),
        String::new(),
        "## Disqualify reasons (a disqualified lead or deal needs one)".into(),
        items(&reg["disqualify_reasons"]),
        String::new(),
        "## Activity types".into(),
        items(&reg["activity_types"]),
        format!(
            "Activity direction: {}. Logged on: {}.",
            join(&reg["activity_directions"]),
            join(&reg["activity_entities"])
        ),
    ];
    Ok(Section::new(
        "crm-picklists",
        "CRM picklists (allowed values)",
        "The exact keys for deal stages, lead statuses and moves, sources, priorities, reasons and activity types.",
        out.join("\n"),
    ))
}

/// `_fields_section()`.
pub fn fields_section() -> Section {
    let mut out: Vec<String> = vec!["What a change card (propose_record_change) may write on each CRM record, and what a create needs. Ids (`*_id`) must be real records (look them up) or `@ref` of a create card in the same answer.".into()];
    for obj in crate::crm::writes::OBJECT_TYPES {
        let mut fields: Vec<&str> = crate::crm::writes::writable(obj).to_vec();
        fields.sort();
        out.push(String::new());
        out.push(format!("## {obj}"));
        out.push(format!("Writable: {}.", fields.join(", ")));
        out.push(format!(
            "Required to create: {}.",
            crate::crm::writes::required_on_create(obj).join(", ")
        ));
    }
    // 5 Nov 2026: the fields a change may write since the Python freeze (crm::writes::RUST_WRITABLE), after the
    // freeze mark so the frozen part stays as Python had it
    out.push(String::new());
    out.push("<!-- added after the Python freeze (Nov 2026) -->".into());
    out.push("## Also writable".into());
    for obj in crate::crm::writes::OBJECT_TYPES {
        let mut extra: Vec<&str> = crate::crm::writes::RUST_WRITABLE
            .iter()
            .filter(|(o, _)| *o == obj)
            .map(|(_, f)| *f)
            .collect();
        if extra.is_empty() {
            continue;
        }
        extra.sort();
        out.push(format!("- {obj}: {}.", extra.join(", ")));
    }
    out.push(
        "segment (leads and deals): optional, a value of the setting crm.segments when it lists any, otherwise free text. A new deal starts in its lead's segment."
            .to_string(),
    );
    Section::new(
        "crm-fields",
        "CRM record fields a card may write",
        "The writable fields of organizations, contacts, leads, deals and activities, and what a create needs.",
        out.join("\n"),
    )
}

/// `_settings_section(db)`.
pub async fn settings_section(db: &mut PgConnection) -> Result<Section, ApiError> {
    let rows: Vec<(Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT system_settings.category, system_settings.key, system_settings.label FROM system_settings \
         WHERE system_settings.is_encrypted = false AND (system_settings.category NOT IN ('nary_state')) \
         ORDER BY system_settings.category, system_settings.key",
    )
    .fetch_all(&mut *db)
    .await?;
    let mut out: Vec<String> = vec!["Every business number, rate, template and switch is a setting (Settings in the ERP), never fixed in code; a missing one stops the feature with a clear error naming the key. Values change, so read the live value through the feature, not from here. Keys by category:".into()];
    // `current = None`: the first row always opens a heading (its category is never Python None here,
    // as NOT IN drops NULL categories).
    let mut current: Option<Option<String>> = None;
    for (cat, key, label) in rows {
        if current.as_ref() != Some(&cat) {
            out.push(String::new());
            out.push(format!(
                "## {}",
                cat.as_deref()
                    .filter(|c| !c.is_empty())
                    .unwrap_or("general")
            ));
            current = Some(cat.clone());
        }
        let label = label.filter(|l| !l.is_empty());
        out.push(format!(
            "- `{key}`{}",
            label.map(|l| format!(": {l}")).unwrap_or_default()
        ));
    }
    Ok(Section::new(
        "settings",
        "Where the settings live",
        "Every setting key by category with what it controls (business numbers are settings, not code).",
        out.join("\n"),
    ))
}

/// `_api_section()` (the recorded snapshot, see the module docs).
pub fn api_section() -> Section {
    Section::new(
        "api",
        "API map",
        "Every API route by area, so you know what the ERP can do.",
        API_MAP.to_string(),
    )
}

/// Every section of the guide, narrative then generated.
pub async fn build(db: &mut PgConnection) -> Result<Vec<Section>, ApiError> {
    let mut sections = narrative();
    sections.push(roles_section());
    sections.push(crm_vocab_section(db).await?);
    sections.push(fields_section());
    sections.push(settings_section(db).await?);
    sections.push(api_section());
    Ok(sections)
}

// ─── Storage (AI Knowledge) ───────────────────────────────────────────────────

pub async fn enabled(db: &mut PgConnection) -> Result<bool, ApiError> {
    let raw = get_optional(db, "ai.erp_guide.enabled").await?;
    let raw = raw
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "true".into());
    Ok(pystr::lower(&raw) == "true")
}

#[derive(sqlx::FromRow)]
struct DocKey {
    id: Uuid,
    system_key: Option<String>,
    content_hash: Option<String>,
}

/// Upsert one knowledge document per section; reindex only what changed; drop what's gone.
pub async fn sync(db: &mut PgConnection) -> Result<Value, ApiError> {
    if !enabled(db).await? {
        return Ok(json!({"status": "off"}));
    }
    let sections = build(db).await?;
    let rows: Vec<DocKey> = sqlx::query_as(
        "SELECT id, system_key, content_hash FROM ai_knowledge_docs WHERE system_key LIKE $1",
    )
    .bind(format!("{KEY_PREFIX}%"))
    .fetch_all(&mut *db)
    .await?;
    // dict keyed by system_key: the last row wins on a duplicate key, as in the Python comprehension.
    let mut order: Vec<String> = Vec::new();
    let mut existing: HashMap<String, DocKey> = HashMap::new();
    for r in rows {
        let k = r.system_key.clone().unwrap_or_default();
        if !existing.contains_key(&k) {
            order.push(k.clone());
        }
        existing.insert(k, r);
    }
    let mut changed: Vec<Uuid> = Vec::new();
    for s in &sections {
        let key = format!("{KEY_PREFIX}{}", s.key);
        let doc = existing.remove(&key);
        let digest = s.digest();
        if let Some(d) = &doc {
            if d.content_hash.as_deref() == Some(digest.as_str()) {
                continue; // unchanged (a failed index is retried from Settings, not every hour)
            }
        }
        let title = format!("ERP guide: {}", s.title);
        let id = match doc {
            None => {
                let id = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO ai_knowledge_docs (title, description, source_type, content, status, error, \
                     is_active, customer_safe, chunk_count, use_count, system_key, content_hash, id) \
                     VALUES ($1, $2, 'markdown', $3, 'processing', NULL, true, false, 0, 0, $4, $5, $6)",
                )
                .bind(&title)
                .bind(&s.summary)
                .bind(&s.content)
                .bind(&key)
                .bind(&digest)
                .bind(id)
                .execute(&mut *db)
                .await?;
                id
            }
            Some(d) => {
                sqlx::query(
                    "UPDATE ai_knowledge_docs SET title=$1, description=$2, content=$3, content_hash=$4, \
                     status='processing', error=NULL, updated_at=now() WHERE id=$5",
                )
                .bind(&title)
                .bind(&s.summary)
                .bind(&s.content)
                .bind(&digest)
                .bind(d.id)
                .execute(&mut *db)
                .await?;
                d.id
            }
        };
        changed.push(id);
    }
    let removed = existing.len();
    let gone: HashSet<Uuid> = existing.values().map(|d| d.id).collect();
    for k in &order {
        if let Some(d) = existing.get(k) {
            if gone.contains(&d.id) {
                sqlx::query("DELETE FROM ai_knowledge_docs WHERE id = $1")
                    .bind(d.id)
                    .execute(&mut *db)
                    .await?;
            }
        }
    }
    crate::crm::followups::py_commit(db).await?;
    for id in &changed {
        // commits; a failed index leaves the text readable by the tool
        process_doc(db, *id).await?;
    }
    Ok(json!({"sections": sections.len(), "updated": changed.len(), "removed": removed}))
}

static WORDS: Lazy<Regex> = Lazy::new(|| Regex::new(r"[a-z0-9]+").unwrap());

fn word_set(s: &str) -> HashSet<String> {
    WORDS.find_iter(s).map(|m| m.as_str().to_string()).collect()
}

/// One section by key or title (fuzzy), or the list of sections. `api:<area>` gives that area's routes.
pub async fn section(db: &mut PgConnection, topic: &str) -> Result<Value, ApiError> {
    let docs: Vec<(Option<String>, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT system_key, title, description, content FROM ai_knowledge_docs WHERE system_key LIKE $1 \
         ORDER BY title",
    )
    .bind(format!("{KEY_PREFIX}%"))
    .fetch_all(&mut *db)
    .await?;
    // (key, title, summary, content); a summary of None prints "None" in the fuzzy match, as in Python.
    let rows: Vec<(String, String, Option<String>, Option<String>)> = if docs.is_empty() {
        build(db)
            .await?
            .into_iter()
            .map(|s| {
                (
                    format!("{KEY_PREFIX}{}", s.key),
                    format!("ERP guide: {}", s.title),
                    Some(s.summary),
                    Some(s.content),
                )
            })
            .collect()
    } else {
        docs.into_iter()
            .map(|(k, t, s, c)| (k.unwrap_or_default(), t, s, c))
            .collect()
    };
    let strip_prefix = |k: &str| {
        k.chars()
            .skip(KEY_PREFIX.chars().count())
            .collect::<String>()
    };
    let topics: Vec<Value> = rows
        .iter()
        .map(|(k, t, s, _)| {
            json!({"topic": strip_prefix(k), "title": t.replace("ERP guide: ", ""),
                   "summary": s})
        })
        .collect();
    let mut want = pystr::lower(pystr::strip(topic));
    if want.is_empty() {
        return Ok(json!({"topics": topics}));
    }
    let mut area: Option<String> = None;
    if want.starts_with("api:") {
        area = Some(pystr::strip(&want[4..]).to_string());
        want = "api".into();
    }
    let words = word_set(&want);
    let score = |row: &(String, String, Option<String>, Option<String>)| -> usize {
        let (k, t, s, _) = row;
        if strip_prefix(k) == want {
            return 100;
        }
        let hay = word_set(&pystr::lower(&format!(
            "{k} {t} {}",
            s.as_deref().unwrap_or("None")
        )));
        words.intersection(&hay).count()
    };
    // max(rows, key=score): the first of the best.
    let mut best = &rows[0];
    let mut best_score = score(best);
    for r in rows.iter().skip(1) {
        let sc = score(r);
        if sc > best_score {
            best = r;
            best_score = sc;
        }
    }
    if best_score == 0 {
        return Ok(json!({"error": format!("No section matches '{topic}'."), "topics": topics}));
    }
    let mut text = best.3.clone().unwrap_or_default();
    if let Some(area) = area.filter(|a| !a.is_empty()) {
        let parts = guide_area_split(&text);
        let head = format!("## {area}");
        let hit: Vec<&String> = parts
            .iter()
            .filter(|p| pystr::lower(p).starts_with(&head))
            .collect();
        text = match hit.first() {
            Some(h) => (*h).clone(),
            None => {
                let areas: Vec<String> = parts
                    .iter()
                    .filter(|p| p.starts_with("## "))
                    .map(|p| {
                        pystr::splitlines(p)
                            .first()
                            .map(|l| l.chars().skip(3).collect::<String>())
                            .unwrap_or_default()
                    })
                    .collect();
                format!("{}\n\nAreas: {}", parts[0], areas.join(", "))
            }
        };
    }
    let limit = 12000;
    let n = text.chars().count();
    Ok(json!({
        "topic": strip_prefix(&best.0),
        "title": best.1.replace("ERP guide: ", ""),
        "text": take_chars(&text, limit) + if n > limit { "\n…(cut; ask for a narrower topic)" } else { "" },
    }))
}

/// The short map in the chat's cached prompt: every section with one line, and how to read one.
pub fn prompt_map() -> String {
    let mut lines: Vec<String> = narrative()
        .iter()
        .map(|s| format!("- {}: {}. {}", s.key, s.title, s.summary))
        .collect();
    lines.extend(
        [
            "- crm-picklists: the exact keys of every CRM picklist.",
            "- crm-fields: what a change card may write on each record.",
            "- permissions: roles and permission codes.",
            "- settings: every setting key by category.",
            "- api: every API route by area (erp_guide('api:<area>')).",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    format!(
        "HOW THE ERP WORKS (the ERP guide). Read a section with erp_guide(topic) before you explain or do something in the ERP you are not sure about; don't guess how a module works.\n{}",
        lines.join("\n")
    )
}

thread_local! {
    /// Test seam for `knowledge.process` (the Python tests monkeypatch it: no embedding provider in tests).
    /// When set, the hook is called with the doc id instead, and the doc is marked "ready" and committed,
    /// as the Python fake does.
    static PROCESS_HOOK: std::cell::RefCell<Option<Box<dyn Fn(Uuid)>>> = const { std::cell::RefCell::new(None) };
}

/// Install (Some) or remove (None) the `knowledge.process` test seam for this thread.
pub fn set_process_hook(hook: Option<Box<dyn Fn(Uuid)>>) {
    PROCESS_HOOK.with(|h| *h.borrow_mut() = hook);
}

/// `knowledge.process(db, doc)`.
pub(crate) async fn process_doc(db: &mut PgConnection, id: Uuid) -> Result<(), ApiError> {
    let hooked = PROCESS_HOOK.with(|h| match &*h.borrow() {
        Some(f) => {
            f(id);
            true
        }
        None => false,
    });
    if hooked {
        sqlx::query("UPDATE ai_knowledge_docs SET status='ready', updated_at=now() WHERE id=$1")
            .bind(id)
            .execute(&mut *db)
            .await?;
        crate::crm::followups::py_commit(db).await?;
        return Ok(());
    }
    let mut doc: granary_db::rows::ai::AIKnowledgeDoc =
        sqlx::query_as("SELECT * FROM ai_knowledge_docs WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *db)
            .await?;
    crate::ai::knowledge::process(db, &mut doc, None, None).await?;
    Ok(())
}
