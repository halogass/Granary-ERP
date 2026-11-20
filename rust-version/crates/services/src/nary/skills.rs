//! Port of backend/app/services/nary/skills.py: Nary improves itself, safely (the Hermes way, without
//! writing code).
//!
//! Skills: when a task took several attempts, or a person corrected Nary, or someone says "remember how to
//! do this", Nary writes the procedure down: when to use it, the steps (its tools in order), what to watch
//! out for, and where it was learnt. A skill is a company memory (subject_type "skill") that waits for the
//! Owner's approval in Nary → Memory; once approved its title and "when" are listed in the chat's cached
//! prompt (nary.skills.max_in_prompt) and use_skill returns the steps.
//!
//! Tool ideas: when no tool can do what is asked, Nary files the idea in the R&D backlog (the project named
//! in nary.skills.tool_ideas_project, type internal_tool, label nary-idea) for a developer. Nary never
//! writes or changes code; a duplicate idea adds its example to the open item.

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::text::pystr::{self, PY_SPACE_CLASS};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_core::settings_store::{get_optional, require_int, SettingError};
use granary_db::rows::nary::NaryMemory;

pub type Result<T> = std::result::Result<T, ApiError>;

pub const WHERE: &str = "Nary & AI → Skills & style";
pub const SKILL: &str = "skill";
pub const IDEA_LABEL: &str = "nary-idea";

/// `re.match(r"When to use:\s*(.+)", content)`.
static WHEN: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"^When to use:[{PY_SPACE_CLASS}]*(.+)")).unwrap());
/// `re.findall(r"[a-z0-9]+", ...)`.
static WORDS: Lazy<Regex> = Lazy::new(|| Regex::new(r"[a-z0-9]+").unwrap());

fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// `_content(when, steps, watch_out, learnt_from)`.
pub fn _content(
    when: &str,
    steps: &str,
    watch_out: Option<&str>,
    learnt_from: Option<&str>,
) -> String {
    let mut parts = vec![
        format!("When to use: {}", pystr::strip(when)),
        format!("Steps:\n{}", pystr::strip(steps)),
    ];
    let w = pystr::strip(watch_out.unwrap_or(""));
    if !w.is_empty() {
        parts.push(format!("Watch out: {w}"));
    }
    let l = pystr::strip(learnt_from.unwrap_or(""));
    if !l.is_empty() {
        parts.push(format!("Learnt from: {l}"));
    }
    parts.join("\n\n")
}

/// `_when(content)`: the "When to use" line (or the first line), at most 200 characters. Python raises
/// IndexError on an empty content (`"".splitlines()[0]`): an internal error here.
pub fn _when(content: &str) -> Result<String> {
    let line = match WHEN.captures(content) {
        Some(c) => pystr::strip(c.get(1).map(|m| m.as_str()).unwrap_or("")).to_string(),
        None => match pystr::splitlines(content).first() {
            Some(l) => l.to_string(),
            None => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "list index out of range"
                )));
            }
        },
    };
    Ok(head(&line, 200))
}

/// `propose(db, user, *, title, when, steps, watch_out=None, learnt_from=None)`: a new skill, proposed for
/// the Owner (or a waiting proposal with the same title, updated).
pub async fn propose(
    db: &mut PgConnection,
    user: Option<&AuthUser>,
    title: &str,
    when: &str,
    steps: &str,
    watch_out: Option<&str>,
    learnt_from: Option<&str>,
) -> Result<NaryMemory> {
    let title = head(pystr::strip(title), 200);
    if title.is_empty() || pystr::strip(when).is_empty() || pystr::strip(steps).is_empty() {
        return Err(ApiError::http(
            400,
            "A skill needs a title, when to use it, and its steps",
        ));
    }
    let content = _content(when, steps, watch_out, learnt_from);
    // func.lower(title) == title.lower(): SQL lower() on the column, Python's on the value
    let waiting: Option<NaryMemory> = sqlx::query_as(
        "SELECT * FROM nary_memory WHERE layer = 'company' AND subject_type = $1 AND status = 'proposed' \
         AND lower(title) = $2",
    )
    .bind(SKILL)
    .bind(pystr::lower(&title))
    .fetch_optional(&mut *db)
    .await?;
    if let Some(mut w) = waiting {
        sqlx::query("UPDATE nary_memory SET content = $2, updated_at = now() WHERE id = $1")
            .bind(w.id)
            .bind(&content)
            .execute(&mut *db)
            .await?;
        w.content = content;
        return Ok(w);
    }
    let entry = deps::remember(db, &title, &content, user).await?;
    let approvers = deps::approver_ids(db).await?;
    let ntitle = format!("Nary learnt a skill: {title}");
    let body = format!(
        "{}\nApprove it in Nary → Memory to let Nary use it.",
        _when(&content)?
    );
    let eid = entry.id.to_string();
    let mut notice = crate::approvals::notify::Notice::new(
        &ntitle,
        &body,
        "/nary/memory?tab=skills",
        "nary_memory",
        &eid,
    );
    notice.type_ = Some("nary_skill");
    notice.email = false;
    notice.whatsapp = false;
    crate::approvals::notify::notify(db, approvers, notice).await?;
    Ok(entry)
}

/// `approved(db)`: the approved skills, newest first.
pub async fn approved(db: &mut PgConnection) -> Result<Vec<NaryMemory>> {
    Ok(sqlx::query_as(
        "SELECT * FROM nary_memory WHERE layer = 'company' AND subject_type = $1 AND status = 'approved' \
         ORDER BY updated_at DESC",
    )
    .bind(SKILL)
    .fetch_all(&mut *db)
    .await?)
}

/// `prompt_list(db)`: the approved skills for the cached prompt: title and when to use, newest first,
/// capped.
pub async fn prompt_list(db: &mut PgConnection) -> Result<String> {
    let cap = require_int(db, "nary.skills.max_in_prompt", WHERE).await?;
    let rows = approved(db).await?;
    // rows[:cap] (a negative cap counts from the end, as Python slices)
    let n = rows.len() as i64;
    let keep = if cap < 0 {
        (n + cap).max(0)
    } else {
        cap.min(n)
    } as usize;
    let rows = &rows[..keep];
    if rows.is_empty() {
        return Ok(String::new());
    }
    let mut lines = Vec::new();
    for r in rows {
        lines.push(format!(
            "- {}: {}",
            r.title.as_deref().unwrap_or("None"),
            _when(&r.content)?
        ));
    }
    Ok(format!(
        "SKILLS YOU LEARNT (approved by the Owner). When one fits, read it with use_skill(title) and follow it:\n{}",
        lines.join("\n")
    ))
}

/// The approved skills with their whole procedures, for jobs that write without the use_skill tool
/// (background replies, re-engagement, follow-ups): newest first, capped like `prompt_list`. "" when none.
pub async fn prompt_procedures(db: &mut PgConnection) -> Result<String> {
    let cap = require_int(db, "nary.skills.max_in_prompt", WHERE).await?;
    let rows = approved(db).await?;
    let n = rows.len() as i64;
    let keep = if cap < 0 {
        (n + cap).max(0)
    } else {
        cap.min(n)
    } as usize;
    let rows = &rows[..keep];
    if rows.is_empty() {
        return Ok(String::new());
    }
    let blocks: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "### {}\n{}",
                r.title.as_deref().unwrap_or("Skill"),
                pystr::strip(&r.content)
            )
        })
        .collect();
    Ok(format!(
        "SKILLS YOU LEARNT (approved by the Owner). Follow each one whenever it fits this message:\n{}",
        blocks.join("\n\n")
    ))
}

fn words(text: &str) -> HashSet<String> {
    WORDS
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// `find(db, title)`: the approved skill that best matches a title.
pub async fn find(db: &mut PgConnection, title: &str) -> Result<Value> {
    let rows = approved(db).await?;
    if rows.is_empty() {
        return Ok(json!({"error": "No approved skills yet."}));
    }
    let want = words(&pystr::lower(title));
    let exact = pystr::lower(pystr::strip(title));
    let mut scores = Vec::with_capacity(rows.len());
    for r in &rows {
        let score = if pystr::lower(r.title.as_deref().unwrap_or("")) == exact {
            100
        } else {
            let text = format!(
                "{} {}",
                r.title.as_deref().unwrap_or("None"),
                _when(&r.content)?
            );
            want.intersection(&words(&pystr::lower(&text))).count()
        };
        scores.push(score);
    }
    // max(rows, key=score): the first of the highest
    let mut best = 0;
    for (i, &sc) in scores.iter().enumerate() {
        if sc > scores[best] {
            best = i;
        }
    }
    if scores[best] == 0 {
        let titles: Vec<Value> = rows
            .iter()
            .map(|r| r.title.clone().map(Value::String).unwrap_or(Value::Null))
            .collect();
        return Ok(
            json!({"error": format!("No approved skill matches '{title}'."), "skills": titles}),
        );
    }
    let r = &rows[best];
    Ok(json!({"title": r.title, "procedure": r.content}))
}

/// The setting naming the R&D project that collects Nary's tool ideas.
pub const TOOL_IDEAS_PROJECT_KEY: &str = "nary.skills.tool_ideas_project";

/// Drops propose_tool_idea from a tool list while nary.skills.tool_ideas_project is unset: the tool can only
/// fail until an R&D project is named, so neither the chat nor an MCP client is offered it.
pub async fn drop_unconfigured_tools(
    db: &mut PgConnection,
    tools: &mut Vec<crate::ai::tools::catalog::Tool>,
) -> Result<()> {
    if !tools.iter().any(|t| t.name == "propose_tool_idea") {
        return Ok(());
    }
    if get_optional(db, TOOL_IDEAS_PROJECT_KEY).await?.is_none() {
        tools.retain(|t| t.name != "propose_tool_idea");
    }
    Ok(())
}

/// `file_tool_idea(db, user, *, title, problem, what_it_should_do, example_requests=None)`: an idea for a
/// tool Nary lacks, in the R&D backlog for a developer (never code).
pub async fn file_tool_idea(
    db: &mut PgConnection,
    user: Option<&AuthUser>,
    title: &str,
    problem: &str,
    what_it_should_do: &str,
    example_requests: Option<&str>,
) -> Result<Value> {
    let name = pystr::strip(
        &get_optional(db, "nary.skills.tool_ideas_project")
            .await?
            .unwrap_or_default(),
    )
    .to_string();
    if name.is_empty() {
        return Err(SettingError::new(
            "nary.skills.tool_ideas_project",
            "is not set (the R&D project for Nary's tool ideas)",
            WHERE,
        )
        .into());
    }
    let project: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM rnd_projects WHERE lower(name) = $1 LIMIT 1")
            .bind(pystr::lower(&name))
            .fetch_optional(&mut *db)
            .await?;
    let Some((project_id,)) = project else {
        return Err(SettingError::new(
            "nary.skills.tool_ideas_project",
            &format!("names an R&D project that doesn't exist ('{name}')"),
            WHERE,
        )
        .into());
    };
    let title = head(pystr::strip(title), 255);
    if title.is_empty() || pystr::strip(problem).is_empty() {
        return Err(ApiError::http(
            400,
            "A tool idea needs a title and the problem it solves",
        ));
    }
    let example = pystr::strip(example_requests.unwrap_or("")).to_string();
    let open_item: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, title, description FROM rnd_backlog_items WHERE rnd_project_id = $1 AND lower(title) = $2 \
         AND status NOT IN ('done', 'rejected') LIMIT 1",
    )
    .bind(project_id)
    .bind(pystr::lower(&title))
    .fetch_optional(&mut *db)
    .await?;
    // (a NULL status never matches `NOT IN`, as in SQLAlchemy's notin_)
    if let Some((id, otitle, desc)) = open_item {
        if !example.is_empty() {
            let d = format!(
                "{}\n\nAnother request: {example}",
                desc.as_deref().unwrap_or("")
            );
            sqlx::query(
                "UPDATE rnd_backlog_items SET description = $2, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(pystr::strip(&d))
            .execute(&mut *db)
            .await?;
        }
        return Ok(
            json!({"id": id.to_string(), "title": otitle, "status": "added to the open idea"}),
        );
    }
    let mut description = format!(
        "Problem: {}\n\nWhat it should do: {}",
        pystr::strip(problem),
        pystr::strip(what_it_should_do)
    );
    if !example.is_empty() {
        description += &format!("\n\nExample requests: {example}");
    }
    description += "\n\nFiled by Nary (the AI assistant) because none of its tools could do this.";
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO rnd_backlog_items (id, rnd_project_id, title, item_type, status, priority, reporter_id, \
         labels, description, board_rank, is_deleted, created_at, updated_at) \
         VALUES ($1, $2, $3, 'internal_tool', 'backlog', 'medium', $4, $5, $6, 0, false, now(), now())",
    )
    .bind(id)
    .bind(project_id)
    .bind(&title)
    .bind(user.map(|u| u.id))
    .bind(json!([IDEA_LABEL]))
    .bind(&description)
    .execute(&mut *db)
    .await?;
    Ok(json!({"id": id.to_string(), "title": title, "status": "filed"}))
}

/// Adapters over the other Nary modules (kept in one place so the wiring is easy to audit).
mod deps {
    use super::*;

    /// `memory.remember(db, layer="company", subject_type=SKILL, title=title, content=content,
    /// source_type="nary", source_ref="skill", author=user, by_nary=True)`.
    pub async fn remember(
        db: &mut PgConnection,
        title: &str,
        content: &str,
        user: Option<&AuthUser>,
    ) -> Result<NaryMemory> {
        crate::nary::memory::remember(
            db,
            crate::nary::memory::Remember {
                layer: "company".into(),
                content: content.into(),
                source_type: "nary".into(),
                subject_type: Some(SKILL.into()),
                subject_id: None,
                title: Some(title.into()),
                source_ref: Some("skill".into()),
                author: user.map(|u| u.id),
                by_nary: true,
                expires_at: None,
            },
        )
        .await
    }
    pub async fn approver_ids(db: &mut PgConnection) -> Result<Vec<Uuid>> {
        crate::nary::roles::approver_ids(db).await
    }
}
