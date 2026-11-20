//! Port of backend/app/services/ai/__init__.py (package-level code, if any, goes here).
pub mod anthropic;
pub mod assistant;
pub mod attributes;
pub mod base;
pub mod cascade;
pub mod catalog_digest;
// batch 4 D3: withdraw_cards and one waiting quotation card per deal or customer
pub mod card_withdraw;
pub mod documents;
pub mod drafting;
pub mod embeddings;
pub mod enrichment;
pub mod erp_guide;
pub mod gemini;
pub mod knowledge;
pub mod mcp_access;
pub mod ollama;
pub mod openai;
pub mod pdf_import;
pub mod records;
pub mod registry;
pub mod schema;
pub mod scoring;
pub mod style;
pub mod suggestions;
pub mod tools;
// visuals V3: pictures in tool results (MCP image content, image blocks for the model)
pub mod tool_images;
pub mod websearch;
