//! Port of backend/app/services/ai/tools/catalog.py: tool catalog + permission gating + execution.
//!
//! SECURITY MODEL (the core guarantee): every tool declares `required_any` permission codes. A tool is only
//! advertised to the LLM if the user holds one of them (`tools_for_user`), AND every execution re-checks the
//! same permissions (`run_tool`). Handlers query the same tables the REST endpoints use and apply the same
//! own/all visibility scope, so the assistant can never read anything the user couldn't read in the UI.
//!
//! Layout: `defs.rs` is ALL_TOOLS (names, descriptions, schemas, gates; generated from the Python list),
//! `handlers.rs` the `_h_*` handlers and their helpers. A handler returns the JSON the model sees; an `Err`
//! is what Python raised, turned into `{"error": "Tool failed: ..."}` by `run_tool`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use granary_core::auth::AuthUser;
use granary_core::errors::{ApiError, ApiResult};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

mod defs;
mod handlers;
// batch 2 T1: sales tools added in Rust (Solution Proposal from the chat, quotations)
mod sales_tools;
// batch 2 T3: the activity history of a CRM record (A6)
mod history_tools;
// batch 3 R2: Nary reads staff mailboxes, with permission rules
pub mod mailbox_tools;
// batch 4 D2: rate card, cost sheets and product files
pub mod file_tools;
// doclib A3: the document library tools (search_documents, read_document)
pub mod doc_tools;
// visuals V3: proposals and their figures (list/get_proposal, preview_visual, figure and edit cards)
pub mod proposal_lifecycle_tools;
pub mod proposal_tools; // fix-longapply
                        // visuals3 P3B: propose_proposal_image (a picture into a proposal)
pub mod proposal_image_tools;
// 22 Oct 2026: propose_proposal_quotation (link a quotation to a proposal)
pub mod proposal_quotation_tools;
// 23 Oct 2026: propose_proposal_from_sections (the chat or the MCP client writes the proposal)
pub mod proposal_sections_tools;
// 22 Oct 2026: tools that can outlast the proxy (preview_visual): still working + job_id
pub mod long_tools;
// 23 Oct 2026: render_document_page (a quotation's or proposal's PDF page as a picture)
pub mod document_page_tools;
// 23 Oct 2026: get_catalog_item and propose_catalog_change (catalog changes by the Owner)
pub mod catalog_change_tools;
// 29 Oct 2026: upload_file (a file your AI client or Nary puts on a CRM record, to attach to a customer message)
pub mod upload_tools;
// 5 Nov 2026: one-time upload links (create_upload_link, get_upload_link, revoke_upload_link)
pub mod upload_link_tools;
// 1 Nov 2026: direct proposal authoring (block tools at MCP act / full power and in Nary's chat)
pub mod proposal_authoring_tools;
// 3 Nov 2026 (Stream A): nary_work_queue (what is waiting while internal Nary is paused)
pub mod work_queue_tools;
// 3 Nov 2026: Nary's WhatsApp and lead work by hand while Nary is paused (Stream B1)
pub mod wa_lead_tools;
// 3 Nov 2026 (Stream B2): deal close-out and meeting tools (the MCP client while Nary is paused)
pub mod closeout_tools;
// 4 Nov 2026: the house quotation notes template (quotation_notes_template)
pub mod quotation_notes_tools;
// 5 Nov 2026 (E7): the Owner's pricing override of a draft quotation (set_pricing_override)
pub mod pricing_rules_tools;

pub use handlers::*;

/// What a handler returns: the JSON for the model, or the exception Python would raise.
pub type HandlerFuture<'a> = Pin<Box<dyn Future<Output = ApiResult<Value>> + Send + 'a>>;

/// `Handler = Callable[[Any, User, dict], Awaitable[dict]]`.
pub type Handler = Arc<
    dyn for<'a> Fn(&'a mut PgConnection, &'a AuthUser, Value) -> HandlerFuture<'a> + Send + Sync,
>;

/// Wraps a closure `|db, user, args| Box::pin(async_fn(db, user, args))` as a [`Handler`].
pub fn handler<F>(f: F) -> Handler
where
    F: for<'a> Fn(&'a mut PgConnection, &'a AuthUser, Value) -> HandlerFuture<'a>
        + Send
        + Sync
        + 'static,
{
    Arc::new(f)
}

/// `handler!(async_fn)`: a [`Handler`] from `async fn(&mut PgConnection, &AuthUser, Value) -> ApiResult<Value>`.
#[macro_export]
macro_rules! ai_tool_handler {
    ($f:path) => {{
        // A named fn (not a closure): the future's Send bound is proven for its own lifetime, which avoids
        // rustc's "Send is not general enough" on higher-ranked closures.
        fn __tool_handler<'a>(
            db: &'a mut ::sqlx::PgConnection,
            user: &'a ::granary_core::auth::AuthUser,
            args: ::serde_json::Value,
        ) -> $crate::ai::tools::catalog::HandlerFuture<'a> {
            Box::pin($f(db, user, args))
        }
        $crate::ai::tools::catalog::handler(__tool_handler)
    }};
}
pub(crate) use ai_tool_handler as handler;

/// `@dataclass class Tool`.
#[derive(Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub handler: Handler,
    /// Empty = no permission gate.
    pub required_any: Vec<String>,
}

impl Tool {
    pub fn new(
        name: &str,
        description: &str,
        parameters: Value,
        handler: Handler,
        required_any: &[&str],
    ) -> Tool {
        Tool {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
            handler,
            required_any: required_any.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl std::fmt::Debug for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tool")
            .field("name", &self.name)
            .field("required_any", &self.required_any)
            .finish_non_exhaustive()
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// `user_can_use(user, tool)`.
pub fn user_can_use(user: &AuthUser, tool: &Tool) -> bool {
    if user.is_superadmin {
        return true;
    }
    if tool.required_any.is_empty() {
        return true;
    }
    tool.required_any
        .iter()
        .any(|code| user.perms.contains(code))
}

/// `_crm_owner_scope(user)`: None => may see all CRM records; otherwise restrict to records assigned to
/// this user.
pub fn _crm_owner_scope(user: &AuthUser) -> Option<Uuid> {
    if user.is_superadmin || user.perms.contains("crm:view_all") {
        return None;
    }
    Some(user.id)
}

/// `can_see_costs(user)`: tiers, white-label rules, costs and margins: for cost viewers only
/// (sales:view_costs, the superadmin).
pub fn can_see_costs(user: &AuthUser) -> bool {
    user.is_superadmin || user.perms.contains("sales:view_costs")
}

/// Returned with every confirm-card proposal. Without it some models call the propose tool again to
/// "finish" the job and run the turn out of tool rounds.
pub const _CARD_NOTE: &str =
    "Shown to the user as a confirm card. Nothing has been saved. Do not call this tool \
     again for the same request; tell the user to review the card and confirm.";

// ─── catalog ─────────────────────────────────────────────────────────────────

/// `ALL_TOOLS`.
pub static ALL_TOOLS: Lazy<Vec<Tool>> = Lazy::new(defs::all_tools);

/// `_BY_NAME`: ALL_TOOLS plus the tools registered with [`register_extra`].
static BY_NAME: Lazy<RwLock<HashMap<String, Tool>>> = Lazy::new(|| {
    RwLock::new(
        ALL_TOOLS
            .iter()
            .map(|t| (t.name.clone(), t.clone()))
            .collect(),
    )
});

/// `register_extra(tools)`: tools offered only where a caller passes them (e.g. the Owner's WhatsApp chat),
/// never by tools_for_user; registered so run_tool can still execute them (with the same permission
/// re-check). Python registers nary_owner.OWNER_TOOLS when that module is first imported.
pub fn register_extra(tools: &[Tool]) {
    let mut by_name = BY_NAME.write().unwrap();
    for t in tools {
        by_name.insert(t.name.clone(), t.clone());
    }
}

/// `_BY_NAME.get(name)`.
pub fn tool_by_name(name: &str) -> Option<Tool> {
    BY_NAME.read().unwrap().get(name).cloned()
}

/// Test seam: `monkeypatch.setitem(catalog._BY_NAME, name, tool)` (None removes it).
pub fn set_tool_for_tests(name: &str, tool: Option<Tool>) {
    let mut by_name = BY_NAME.write().unwrap();
    match tool {
        Some(t) => {
            by_name.insert(name.to_string(), t);
        }
        None => {
            by_name.remove(name);
        }
    }
}

/// `tools_for_user(user)`.
pub fn tools_for_user(user: &AuthUser) -> Vec<Tool> {
    ALL_TOOLS
        .iter()
        .filter(|t| user_can_use(user, t))
        .cloned()
        .collect()
}

/// `to_toolspecs(tools)`: provider-neutral declarations for the LLM.
pub fn to_toolspecs(tools: &[Tool]) -> Vec<crate::ai::base::ToolSpec> {
    tools
        .iter()
        .map(|t| {
            crate::ai::base::ToolSpec::new(
                t.name.clone(),
                t.description.clone(),
                t.parameters.clone(),
            )
        })
        .collect()
}

/// `str(e)` of what a handler raised, as `run_tool` puts it in "Tool failed: ...".
///
/// Starlette's HTTPException prints "{status}: {detail}". AppException never calls
/// `Exception.__init__`, so Python prints its constructor arguments; the detail is what every subclass
/// built from a single message prints (NotFoundError's two-argument tuple form is not reproduced).
pub fn exc_str(e: &ApiError) -> String {
    match e {
        ApiError::Http { status, detail, .. } => {
            let d = match detail {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{}: {}", status.as_u16(), d)
        }
        ApiError::App { detail, .. } => detail.clone(),
        ApiError::RequestValidation(_) => "Validation failed".to_string(),
        ApiError::Integrity(orig) => orig.clone(),
        ApiError::InvalidInput { message, .. } => message.to_string(),
        ApiError::Internal(err) => err.to_string(),
    }
}

/// `run_tool(db, user, name, args)`: execute a tool by name. Re-checks permission at execution time
/// (defense in depth).
pub async fn run_tool(db: &mut PgConnection, user: &AuthUser, name: &str, args: Value) -> Value {
    let Some(tool) = tool_by_name(name) else {
        return json!({"error": format!("Unknown tool: {name}")});
    };
    if !user_can_use(user, &tool) {
        return json!({"error": "You don't have permission to use this tool."});
    }
    // `args or {}`.
    let args = match args {
        Value::Null => json!({}),
        Value::Object(ref m) if m.is_empty() => json!({}),
        other => other,
    };
    // A savepoint: a failing query rolls back only this tool, so the turn's session (and the final save of
    // the conversation) is not left in an aborted transaction. Tools never commit. Outside a transaction
    // (a plain connection) there is nothing to protect, and the handler runs as is.
    let in_tx = sqlx::query("SAVEPOINT ai_tool")
        .execute(&mut *db)
        .await
        .is_ok();
    let out = (tool.handler)(&mut *db, user, args).await;
    match out {
        Ok(v) => {
            if in_tx {
                let _ = sqlx::query("RELEASE SAVEPOINT ai_tool")
                    .execute(&mut *db)
                    .await;
            }
            v
        }
        Err(e) => {
            if in_tx {
                let _ = sqlx::query("ROLLBACK TO SAVEPOINT ai_tool")
                    .execute(&mut *db)
                    .await;
                let _ = sqlx::query("RELEASE SAVEPOINT ai_tool")
                    .execute(&mut *db)
                    .await;
            }
            let msg = exc_str(&e);
            // Never leak a stack trace to the model, but log it server-side.
            tracing::warn!(target: "ai.tools", "Tool {} failed: {}", name, msg);
            json!({"error": format!("Tool failed: {}", msg.chars().take(200).collect::<String>())})
        }
    }
}
