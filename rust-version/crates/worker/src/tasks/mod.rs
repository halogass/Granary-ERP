//! Every `@celery.task` of backend/app/tasks.py, by module. Each module exports `TASKS: &[TaskDef]`;
//! [`all`] is the registry's content.
//!
//! Ownership during wave 5 (see docs/WAVE5_BRIEF.md "Template (W5-F)"): `common`, `crm`, `nary` are W5-F's
//! (do not edit; ask the lead), `group_a` belongs to W5-A and `group_b` to W5-B (each may split its file
//! into a `group_x/` directory of submodules).

pub mod common;
pub mod crm;
pub mod group_a;
pub mod group_b;
pub mod nary;

use crate::registry::TaskDef;

/// Every registered task.
pub fn all() -> Vec<TaskDef> {
    [crm::TASKS, nary::TASKS, group_a::TASKS, group_b::TASKS]
        .into_iter()
        .flatten()
        .copied()
        .collect()
}

/// A task error from anything printable (ApiError, InboxError, ...): the text Python's traceback would end
/// with. Use it as `.map_err(err)?`.
pub fn err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}
