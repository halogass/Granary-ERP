//! Celery calls services/nary/channels.py makes, with the test hook its tests install.

use std::sync::{Arc, RwLock};

/// A replacement for the Celery `delay` (tests: Python monkeypatches `tasks.nary_staff_chat_reply.delay`).
pub type DelayHook = Arc<dyn Fn(&str, &str, bool) + Send + Sync>;

static DELAY: RwLock<Option<DelayHook>> = RwLock::new(None);

/// Install (Some) or remove (None) the test replacement of `nary_staff_chat_reply.delay`.
pub fn set_nary_staff_chat_reply_delay(hook: Option<DelayHook>) {
    *DELAY.write().unwrap() = hook;
}

/// `app.tasks.nary_staff_chat_reply.delay(chat_id, message_id, late=True)`: queue the staff-chat reply task
/// (Celery, [`crate::tasks::delay`]) for a message the gap check found late. A broker failure raises, as
/// `.delay` does.
pub async fn nary_staff_chat_reply_delay(
    chat_id: &str,
    message_id: &str,
    late: bool,
) -> granary_core::errors::ApiResult<()> {
    if let Some(h) = DELAY.read().unwrap().clone() {
        h(chat_id, message_id, late);
        return Ok(());
    }
    let mut kwargs = serde_json::Map::new();
    kwargs.insert("late".into(), serde_json::Value::Bool(late));
    crate::tasks::delay(
        "nary_staff_chat_reply",
        vec![chat_id.into(), message_id.into()],
        kwargs,
    )
    .await?;
    Ok(())
}
