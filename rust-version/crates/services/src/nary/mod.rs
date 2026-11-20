//! Port of backend/app/services/nary/__init__.py.
//!
//! Nary: the company's AI sales and operations agent (docs/design/nary-agent.md).
//!
//! Phase 1 is the platform: events (events.rs), runs (runs.rs), the action registry (actions.rs), the
//! executor with approvals and autonomy (executor.rs, autonomy.rs), memory and context walls
//! (memory.rs, context.rs, persona.rs), roles and staff chats (roles.rs, identity.rs), and the daily
//! rhythm with governance (rhythm.rs, briefs.rs, governance.rs). Nothing is sent to customers.
//!
//! Shared session helpers for every nary module (Rust only; Python gets them from SQLAlchemy):
//! - [`session_pool`]: the pool a fresh `AsyncSessionLocal()` session is opened from ([`set_session_pool`]
//!   installs it; without it a lazy pool on `DATABASE_URL` is used).
//! - [`commit`] / [`rollback`]: `await db.commit()` / `await db.rollback()` on a caller's connection. When the
//!   connection is inside a sqlx transaction (`pool.begin()`, passed as `&mut *tx`) they run `COMMIT; BEGIN` /
//!   `ROLLBACK; BEGIN`, so the caller carries on in a fresh transaction (SQLAlchemy autobegin). On a plain
//!   connection (autocommit) every statement is already committed and both do nothing.
use std::sync::RwLock;

use granary_core::errors::{ApiError, ApiResult};
use sqlx::{Connection, PgConnection, PgPool};

pub mod ack;
pub mod actions;
pub mod autonomy;
pub mod briefs;
pub mod cadence;
pub mod cards;
pub mod channel_actions;
pub mod channels;
pub mod chat_links;
pub mod commands;
pub mod context;
pub mod costing;
pub mod deals;
pub mod email_intake;
pub mod events;
pub mod executor;
pub mod owner_channel;
// batch 4 D1: Nary reads customer files (store, extract, the list/read tools)
pub mod file_text;
pub mod file_tools;
pub mod files;
pub mod governance;
pub mod handoff;
pub mod heartbeat;
// batch 2 T2: CRM hygiene tools (A2, A4, A9)
pub mod history;
pub mod hygiene;
pub mod identity;
pub mod identity_match;
pub mod intake;
pub mod labels;
pub mod llm;
pub mod long_work; // fix-longapply: long work detached from the request, card leases
pub mod margins;
pub mod media;
// batch 3 R1: meeting recordings and notes files
pub mod meeting_files;
pub mod meetings;
pub mod memory;
pub mod negotiation;
// batch 4 D2: files on the messages Nary sends to customers
pub mod catalog_changes; // 23 Oct: catalog changes by the chat and the MCP client (the Owner only)
pub mod outbound_files;
pub mod product_files; // 5 Nov 2026: the Owner adds a product file (library document) from a card
                       // 29 Oct 2026: upload_file and preview_visual save_to (files your AI client or Nary put on a CRM record)
pub mod persona;
pub mod phones;
pub mod playbook_actions;
pub mod playbooks;
pub mod privacy;
pub mod profiling;
pub mod proposal_actions;
pub mod proposal_edits;
pub mod proposal_lifecycle; // fix-longapply: revise instead of a duplicate, reject a draft
pub mod proposal_quotations; // link a quotation to a proposal (22 Oct 2026)
pub mod proposal_sections; // 23 Oct: a proposal written section by section by the chat or the MCP client
pub mod proposal_visuals; // visuals V2: Nary's charts and diagrams in proposals
pub mod proposals;
pub mod quotes;
pub mod uploads;
// batch 4 D3: change a draft quotation from the chat (quotation.update)
pub mod quotation_updates;
// 1 Nov 2026: reprice to today, withdraw from approval, cancel (quotation.reprice / .withdraw / .cancel)
pub mod quotation_lifecycle;
// 5 Nov 2026 (Stream C): a sent or approved quotation linked to a deal without a revision (quotation.link_deal)
pub mod quotation_links;
// 3 Nov 2026 (Stream B2): deal close-out and meeting tools for the MCP client while Nary is paused (deal.won)
pub mod closeout;
// doclib A3: send a library document to a customer (document.send)
pub mod document_send;
// visuals V3: the proposal figure and edit cards (proposal.visual, proposal.edit)
pub mod proposal_images; // visuals3 P3B: pictures into proposals from the chat and the MCP client
pub mod proposal_visual_actions;
pub mod rates;
pub mod replies;
pub mod reply_alerts;
pub mod reply_guard;
pub mod report;
pub mod rhythm;
pub mod roles;
pub mod runs;
// batch 2 T3: timed customer messages (A5)
pub mod outbound_log;
pub mod scheduled_sends;
pub mod scoring;
pub mod skills;
pub mod slots;
pub mod sop_revisions;
pub mod staff_chat;
pub mod subscribers;
pub mod triage;
pub mod turns;
pub mod wa_ingest;
pub mod wa_send;
pub mod web3forms;
pub mod writing;
// 3 Nov 2026 (Stream A): the work queue for the MCP client while Nary is paused (migration 0154)
pub mod work_queue;
// 3 Nov 2026 (Stream B1): WhatsApp leads, held chats, lead merge and BANT scoring by hand while Nary is paused
pub mod wa_leads;

static SESSION_POOL: RwLock<Option<PgPool>> = RwLock::new(None);

/// Installs the pool fresh sessions are opened from (Python: `AsyncSessionLocal`), replacing any earlier one.
/// Tests install their `fresh_db` pool here.
pub fn set_session_pool(pool: PgPool) {
    *SESSION_POOL.write().unwrap() = Some(pool);
}

/// The pool behind `AsyncSessionLocal()`.
pub fn session_pool() -> ApiResult<PgPool> {
    if let Some(p) = SESSION_POOL.read().unwrap().clone() {
        return Ok(p);
    }
    let url =
        granary_core::state::normalize_database_url(&granary_core::config::settings().DATABASE_URL);
    let pool = PgPool::connect_lazy(&url).map_err(ApiError::from)?;
    *SESSION_POOL.write().unwrap() = Some(pool.clone());
    Ok(pool)
}

/// `await db.commit()` on the caller's connection (see the module docs).
pub async fn commit(db: &mut PgConnection) -> ApiResult<()> {
    if db.is_in_transaction() {
        sqlx::query("COMMIT").execute(&mut *db).await?;
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    Ok(())
}

/// `await db.rollback()` on the caller's connection (see the module docs).
pub async fn rollback(db: &mut PgConnection) -> ApiResult<()> {
    if db.is_in_transaction() {
        sqlx::query("ROLLBACK").execute(&mut *db).await?;
        sqlx::query("BEGIN").execute(&mut *db).await?;
    }
    Ok(())
}

/// `s[:n]` (code points).
pub fn py_slice(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `s[-n:]` (code points; `s[-0:]` is the whole string, as in Python).
pub fn py_slice_tail(s: &str, n: usize) -> &str {
    if n == 0 {
        return s;
    }
    let count = s.chars().count();
    if count <= n {
        return s;
    }
    match s.char_indices().nth(count - n) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}
