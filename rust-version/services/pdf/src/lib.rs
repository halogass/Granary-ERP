//! The Granary PDF service in Rust: the HTTP contract of services/pdf (Node + Puppeteer + Handlebars).

pub mod cdp;
pub mod dates;
pub mod hbs;
pub mod http;
pub mod jsval;
pub mod log;
pub mod render;

use std::path::PathBuf;

/// The assets folder (templates/): PDF_ASSETS_DIR, else ./assets when present, else the
/// crate's own assets folder (development).
pub fn assets_dir() -> PathBuf {
    if let Ok(d) = std::env::var("PDF_ASSETS_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    let local = PathBuf::from("assets");
    if local.join("templates").is_dir() {
        return local;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}
