//! granary-services: port of backend/app/services. One module per Python module, same names and nesting, so
//! the Python import cycles (approvals <-> verification <-> documents, crm <-> ai <-> nary) are module cycles
//! inside one crate. Conventions (wave 2):
//! - a function taking the SQLAlchemy session takes `db: &mut sqlx::PgConnection` (pass `&mut *tx`);
//! - errors are `granary_core::errors::ApiError` (HTTPException/AppException carry the exact Python detail);
//! - settings via `granary_core::settings_store`, env config via `granary_core::config::settings()`,
//!   Redis via `granary_core::redis_client::get_redis()`, HTTP via `crate::http::client()`;
//! - names stay the Python names (snake_case), public functions keep Python's parameter order.

pub mod ai;
pub mod approvals;
pub mod attachments_access;
pub mod catalog; // 23 Oct: catalog writes (the editor and Nary's catalog.change)
pub mod collab;
pub mod crm;
pub mod crm_inbox;
pub mod crm_mailboxes;
pub mod crm_match;
pub mod doc_library; // doclib A1
pub mod documents;
pub mod email;
pub mod email_crypto;
pub mod email_inline;
pub mod email_marketing;
pub mod email_signature;
pub mod email_templates;
pub mod face;
pub mod fx;
pub mod gl_posting;
pub mod google_calendar;
pub mod http;
pub mod imap_sync;
pub mod inproc;
pub mod invoicing;
pub mod later;
pub mod manufacturing; // generated unit serials
pub mod nary;
pub mod numbering;
pub mod pdf;
pub mod pdf_sign;
pub mod pdftools;
pub mod pricing;
pub mod pricing_rules; // 5 Nov: E7 pricing rules (2x full cost, Rules on Mini, HW-only 1.3x, software 1.5x)
pub mod quotation_expiry;
pub mod quotation_notes; // 4 Nov: the house notes templates per product family and the notes check (migration 0158)
pub mod quotation_tax; // 2 Nov: line taxes, the "prices exclude tax" note and the notes/tax gate
pub mod redis_lease;
pub mod research;
pub mod rnd_gitlab; // 17 Nov: R&D backlog items <-> GitLab issues, two-way (migration 0179)
pub mod safe_fetch; // security 26 Oct: staff-typed URLs fetched with the research guard
pub mod safe_files; // security 26 Oct: the type a stored file is served with
pub mod secret_audit; // security 26 Oct: placeholder secrets logged and told to the Owner at startup
pub mod segment; // the optional sales segment of leads, deals and catalog items (crm.segments)
pub mod storage;
pub mod stt_client;
pub mod tasks;
pub mod totp; // 2 Nov: TOTP two-factor sign-in and the step-up API (migration 0148)
pub mod upload_links; // 5 Nov 2026: one-time upload links (the drop page; migration 0163)
pub mod verification;
pub mod visuals; // visuals V1
pub mod wa_export;
pub mod wa_signature;
pub mod websocket_manager;
pub mod whatsapp;
