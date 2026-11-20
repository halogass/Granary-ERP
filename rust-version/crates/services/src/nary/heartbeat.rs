//! Port of backend/app/services/nary/heartbeat.py.
//!
//! The heartbeat: inside working hours, every nary.heartbeat_minutes, Nary looks at its to-do list.
//!
//! A beat records what is waiting (working memory, open cards) in a run; it costs nothing when there is
//! nothing to do and makes no model call. The playbooks run on events and on their own tick
//! (playbooks.py).
use chrono::Utc;
use granary_compat::pyjson::PyDateTime;
use granary_core::errors::ApiResult;
use serde_json::json;
use sqlx::PgConnection;

use super::runs::{self, StepOpts};

pub async fn beat(db: &mut PgConnection) -> ApiResult<bool> {
    let now = Utc::now();
    let (working,): (i64,) = sqlx::query_as(
        "SELECT count(nary_memory.id) FROM nary_memory WHERE nary_memory.layer = 'working' \
         AND nary_memory.status = 'approved' AND (nary_memory.expires_at IS NULL OR nary_memory.expires_at > $1)",
    )
    .bind(now)
    .fetch_one(&mut *db)
    .await?;
    let (cards,): (i64,) =
        sqlx::query_as("SELECT count(agent_pending_actions.id) FROM agent_pending_actions WHERE agent_pending_actions.status = 'pending'")
            .fetch_one(&mut *db)
            .await?;
    if working == 0 && cards == 0 {
        return Ok(false);
    }
    let trigger_ref = PyDateTime::from_chrono_utc(&now).isoformat();
    let mut run = runs::start_run(
        db,
        "heartbeat",
        "nary.heartbeat",
        Some(&trigger_ref),
        "internal",
        None,
    )
    .await?;
    let input = json!({"working_items": working, "open_cards": cards});
    runs::add_step(
        db,
        &mut run,
        "note",
        Some("to-do"),
        StepOpts {
            input: Some(&input),
            ..Default::default()
        },
    )
    .await?;
    let outcome = format!("{working} working item(s), {cards} card(s) waiting.");
    runs::finish_run(db, &mut run, "skipped", Some(&outcome), None).await?;
    super::commit(db).await?;
    Ok(true)
}
