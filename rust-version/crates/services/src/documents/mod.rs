//! Port of backend/app/services/documents/__init__.py.
//!
//! Sales documents (Nary Phase 4): the generic document model's services. Solution Proposal first.
//!
//! - core_.rs    create, blocks, links to quotations, revisions, the content hash for the approval
//!   (Python `core.py`; `core` is a reserved crate path name in Rust, hence `core_`)
//! - render.rs   the PDF builder (Markdown to sanitised HTML, images as data: URIs, the commercial
//!   block from the linked quotations, cover, header and footer)
//! - assets.rs   images in the private documents bucket
//! - figures.rs  chart and diagram blocks: guard, draw once on save, store as assets (visuals V2)
pub mod assets;
pub mod core_;
pub mod figures; // visuals V2: chart and diagram blocks
pub mod render;
