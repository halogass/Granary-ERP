//! Port of backend/app/services/ai/tools/__init__.py.
//!
//! Read-only, permission-mirrored tool registry for the AI assistant.
//!
//! Public surface:
//!   tools_for_user(user)  -> [Tool]      tools the user is allowed to use
//!   to_toolspecs(tools)   -> [ToolSpec]  provider-neutral declarations for the LLM
//!   run_tool(db, user, name, args)       execute a tool (re-checks permission)
pub mod catalog;
pub mod nary_owner;

pub use catalog::{run_tool, to_toolspecs, tools_for_user, user_can_use, Tool, ALL_TOOLS};
