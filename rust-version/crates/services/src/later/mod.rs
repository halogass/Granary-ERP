//! Placeholders for functions of modules ported in a LATER wave. A module ported now that must call, say,
//! services/nary/owner_channel.py `raise_topic` puts a `todo!("wave 3: nary.owner_channel.raise_topic")` stub with
//! the Rust signature it needs into its own file here (later/<its module>.rs) and calls that. The wave that
//! ports the real function replaces every call and deletes the stub.
pub mod ai_tools;
pub mod crm_deal_hooks;
pub mod documents;
pub mod nary_actions;
pub mod nary_channels;
pub mod nary_deals;
pub mod nary_sop_revisions;
pub mod nary_staff_chat;
pub mod verification;
