//! Colours, fonts and sizes. The palette is the proposal PDF's (services/pdf/assets/templates/proposal.html)
//! plus red #B42318: catalog products in red, infrastructure in blue and slate.

use super::spec::FigureSize;

pub const FONT_FAMILY: &str = "'IBM Plex Sans', Arial, Helvetica, sans-serif";

pub const RED: &str = "#B42318"; // catalog products, first series
pub const BLUE: &str = "#1D4E89"; // infrastructure, second series
pub const SLATE: &str = "#5A6672"; // generic devices, axis text
pub const GREEN: &str = "#1E6B3A";
pub const AMBER: &str = "#8A5A00";
pub const MAROON: &str = "#7A1F2B";
pub const NAVY: &str = "#1D2733";
pub const SAND: &str = "#B08968";

pub const INK: &str = "#1D2733"; // main text
pub const MUTED: &str = "#5A6672"; // secondary text
pub const FAINT: &str = "#8A939D"; // tick labels on light grids
pub const GRID: &str = "#E6E9ED";
pub const AXIS: &str = "#B8C0C8";
pub const PAPER: &str = "#FFFFFF";
pub const RED_TINT: &str = "#FBF0F0";
pub const BLUE_TINT: &str = "#EEF3F9";
pub const SLATE_TINT: &str = "#F3F5F7";
pub const GREEN_TINT: &str = "#EEF6F1";
pub const AMBER_TINT: &str = "#FBF5EA";

/// Series colours in order (named for `colors` in a spec).
pub const PALETTE: [(&str, &str); 8] = [
    ("red", RED),
    ("blue", BLUE),
    ("slate", SLATE),
    ("amber", AMBER),
    ("green", GREEN),
    ("maroon", MAROON),
    ("navy", NAVY),
    ("sand", SAND),
];

/// The colour for series `i`: the spec's `colors[i]` (palette name or #rrggbb) else the palette in order.
pub fn series_color(colors: &[String], i: usize) -> String {
    if let Some(c) = colors.get(i).and_then(|c| color(c)) {
        return c;
    }
    PALETTE[i % PALETTE.len()].1.to_string()
}

/// A palette name or a #rrggbb / #rgb colour, normalised; anything else is None.
pub fn color(c: &str) -> Option<String> {
    let c = c.trim();
    if let Some((_, hex)) = PALETTE.iter().find(|(n, _)| n.eq_ignore_ascii_case(c)) {
        return Some(hex.to_string());
    }
    let hex = c.strip_prefix('#')?;
    if (hex.len() == 6 || hex.len() == 3) && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Some(format!("#{}", hex.to_ascii_uppercase()));
    }
    None
}

/// Canvas and type sizes for one figure size. A full figure prints at about 170 mm, a half one at about
/// 82 mm; the canvases keep the body text near 8 to 9 pt in both.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub width: f64,
    pub pad: f64,
    pub title: f64,
    pub text: f64,
    pub small: f64,
    pub tiny: f64,
}

pub fn metrics(size: FigureSize) -> Metrics {
    match size {
        FigureSize::Full => Metrics {
            width: 680.0,
            pad: 16.0,
            title: 14.0,
            text: 12.0,
            small: 11.0,
            tiny: 10.0,
        },
        FigureSize::Half => Metrics {
            width: 400.0,
            pad: 12.0,
            title: 13.0,
            text: 11.5,
            small: 10.5,
            tiny: 9.5,
        },
    }
}
