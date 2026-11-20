//! Nary tasks of backend/app/tasks.py ported by W5-F: `nary_dispatch_events`, `nary_staff_chat_reply`.

use std::time::Duration;

use crate::registry::{Outcome, Param, TaskCtx, TaskDef, TaskResult};
use crate::tasks::common::py_str;
use crate::tasks::err;

pub const TASKS: &[TaskDef] = &[
    TaskDef::new("app.tasks.nary_dispatch_events", &[], |ctx| {
        Box::pin(nary_dispatch_events(ctx))
    }),
    TaskDef::new(
        "app.tasks.nary_staff_chat_reply",
        &[
            Param::req("chat_id"),
            Param::req("message_id"),
            Param::opt("late", "false"),
        ],
        |ctx| Box::pin(nary_staff_chat_reply(ctx)),
    )
    .max_retries(None),
];

/// `nary_dispatch_events()`: hand undispatched domain events to Nary's subscribers
/// (services/nary/events.py). Returns `str(await dispatch_pending())`.
pub async fn nary_dispatch_events(_ctx: TaskCtx) -> TaskResult {
    let out = granary_services::nary::events::dispatch_pending(None)
        .await
        .map_err(err)?;
    Outcome::done(py_str(&out))
}

/// `STAFF_CHAT_RETRY_SECONDS`.
pub const STAFF_CHAT_RETRY_SECONDS: u64 = 3;

/// `nary_staff_chat_reply(self, chat_id, message_id, late=False)` (`bind=True, max_retries=None`): Nary
/// answers a linked member of staff on WhatsApp (services/nary/staff_chat.py), one turn at a time per
/// person: while another answer for them runs, this one waits and tries again.
pub async fn nary_staff_chat_reply(ctx: TaskCtx) -> TaskResult {
    let waited = i64::from(ctx.retries) * STAFF_CHAT_RETRY_SECONDS as i64;
    let chat_id = ctx.str_arg("chat_id");
    let message_id = ctx.str_arg("message_id");
    let late = ctx.truthy("late");
    // `async with AsyncSessionLocal() as db: return await staff_chat.reply_in_turn(...)`: what the reply
    // did not commit itself is rolled back when the session closes.
    let db = ctx.session();
    let out = {
        let mut conn = db.conn().await.map_err(err)?;
        granary_services::nary::staff_chat::reply_in_turn(
            &mut conn,
            &chat_id,
            &message_id,
            late,
            waited,
        )
        .await
        .map_err(err)?
    };
    drop(db);
    if out.get("status").and_then(|s| s.as_str()) == Some("busy") {
        return Ok(Outcome::Retry {
            countdown: Duration::from_secs(STAFF_CHAT_RETRY_SECONDS),
        });
    }
    Ok(Outcome::Done(out))
}
