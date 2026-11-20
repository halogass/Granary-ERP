// Row structs, one module per area.
//! One row struct per table, grouped by area.

pub use crate::vector::Vector;

pub mod ai;
pub mod approval;
pub mod assets;
pub mod attachment;
pub mod auth;
pub mod budget;
pub mod collab;
pub mod crm;
pub mod crm_attributes;
pub mod crm_dedupe;
pub mod crm_inbox;
pub mod crm_research;
pub mod doc_library; // doclib A1
pub mod documents;
pub mod email_client;
pub mod email_signature;
pub mod finance;
pub mod fleet;
pub mod helpdesk;
pub mod hr;
pub mod inventory;
pub mod knowledge;
pub mod maintenance;
pub mod manufacturing;
pub mod marketing;
pub mod messaging;
pub mod nary;
pub mod p2p;
pub mod partner;
pub mod portal;
pub mod product_line;
pub mod project;
pub mod rnd;
pub mod sales;
pub mod signing;
pub mod system;
pub mod vendor;
pub mod verification;
