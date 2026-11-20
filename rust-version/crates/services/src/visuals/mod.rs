//! Proposal figures (phase 1): charts and 2D diagrams drawn from a small JSON spec into a self-contained,
//! deterministic SVG (and a PNG through resvg with the bundled IBM Plex Sans).
//!
//! - [`spec`]: the shared contract (`chart` / `diagram` block data), serde and the JSON schema for the model;
//! - [`guard`]: the rules a customer figure must pass (schema and ranges, no money, no accuracy figures,
//!   catalog product names, sources; ERP refs resolved);
//! - [`chart`], [`diagram`]: the drawings; [`sanitize`]: the allow-list for `custom_svg`;
//! - [`theme`], [`i18n`], [`text`], [`icons`]: colours, words, text measuring and the 2D icon set;
//! - [`raster`]: SVG to PNG.
//!
//! Every string from a spec is escaped ([`svg::esc`]): customer text is data, never markup.

pub mod chart;
pub mod diagram;
pub mod guard;
pub mod i18n;
pub mod icons;
pub mod raster;
pub mod sanitize;
pub mod spec;
pub mod svg;
pub mod text;
pub mod theme;

pub use guard::{guard, load_context, CatalogProduct, ErpData, GuardContext, GuardReport};
pub use spec::*;

use granary_core::errors::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum VisualError {
    /// The spec cannot be drawn (missing data, bad custom SVG); the message is for the user or the model.
    #[error("{0}")]
    Invalid(String),
    /// Rasterising failed.
    #[error("figure rendering failed: {0}")]
    Render(String),
}

pub type Result<T> = std::result::Result<T, VisualError>;

impl From<VisualError> for ApiError {
    fn from(e: VisualError) -> ApiError {
        match e {
            VisualError::Invalid(m) => ApiError::validation(m),
            VisualError::Render(m) => ApiError::internal(anyhow::anyhow!(m)),
        }
    }
}

/// Draw options. `size` picks the canvas: a half width figure uses a narrower canvas so its text stays
/// readable when printed at half the A4 text width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderOptions {
    pub size: FigureSize,
}

/// The SVG of a spec at full width.
pub fn render_svg(spec: &Spec) -> Result<String> {
    render_svg_with(spec, RenderOptions::default())
}

/// The SVG of a spec drawn for a block of `size` (use the block's size when storing it).
pub fn render_svg_sized(spec: &Spec, size: FigureSize) -> Result<String> {
    render_svg_with(spec, RenderOptions { size })
}

pub fn render_svg_with(spec: &Spec, opts: RenderOptions) -> Result<String> {
    match spec {
        Spec::Chart(c) => chart::render(c, opts),
        Spec::Diagram(d) => diagram::render(d, opts),
        // The size cap (a Setting) is enforced by the guard; drawing re-sanitises with a generous cap.
        Spec::CustomSvg(c) => sanitize::render(c, sanitize::RENDER_MAX_KB),
    }
}

/// A PNG of the spec (full width canvas), `width` pixels wide.
pub fn render_png(spec: &Spec, width: u32) -> Result<Vec<u8>> {
    raster::svg_to_png(&render_svg(spec)?, width)
}

/// A PNG of the spec drawn for a block of `size`, `width` pixels wide.
pub fn render_png_sized(spec: &Spec, size: FigureSize, width: u32) -> Result<Vec<u8>> {
    raster::svg_to_png(&render_svg_sized(spec, size)?, width)
}
