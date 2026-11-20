//! SVG to PNG with resvg. Only the bundled IBM Plex Sans is loaded (no system fonts) and external files
//! are never read, so the same SVG gives the same pixels on every machine.

use std::sync::Arc;

use once_cell::sync::Lazy;
use resvg::{tiny_skia, usvg};

use super::text::{FONT_REGULAR, FONT_SEMIBOLD};
use super::{Result, VisualError};

/// Widest PNG we make (a check or a print preview never needs more).
pub const MAX_WIDTH: u32 = 4000;

static FONTS: Lazy<Arc<usvg::fontdb::Database>> = Lazy::new(|| {
    let mut db = usvg::fontdb::Database::new();
    db.load_font_data(FONT_REGULAR.to_vec());
    db.load_font_data(FONT_SEMIBOLD.to_vec());
    db.set_sans_serif_family("IBM Plex Sans");
    db.set_serif_family("IBM Plex Sans");
    db.set_monospace_family("IBM Plex Sans");
    Arc::new(db)
});

fn options() -> usvg::Options<'static> {
    usvg::Options {
        font_family: "IBM Plex Sans".into(),
        fontdb: FONTS.clone(),
        // Never load anything a drawing points at.
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    }
}

/// Rasterise `svg` to a PNG `width` px wide (height keeps the aspect ratio), on white.
pub fn svg_to_png(svg: &str, width: u32) -> Result<Vec<u8>> {
    if width == 0 || width > MAX_WIDTH {
        return Err(VisualError::Invalid(format!(
            "PNG width must be 1 to {MAX_WIDTH}"
        )));
    }
    let tree =
        usvg::Tree::from_str(svg, &options()).map_err(|e| VisualError::Render(e.to_string()))?;
    let size = tree.size();
    let scale = width as f32 / size.width();
    let height = (size.height() * scale).round().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| VisualError::Render("empty image".into()))?;
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
        .encode_png()
        .map_err(|e| VisualError::Render(e.to_string()))
}
