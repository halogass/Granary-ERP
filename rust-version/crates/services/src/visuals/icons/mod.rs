//! The 2D icon set: one simple line drawing per icon name, on a 24 x 24 grid. Catalog products (a node with
//! a sku) are drawn in red, systems and network in blue, people and places in slate.

use super::svg::n;
use super::theme;

/// Every icon name a diagram node may use.
pub const ICON_NAMES: [&str; 18] = [
    "person",
    "user",
    "server",
    "cloud",
    "database",
    "laptop",
    "phone",
    "erp",
    "api",
    "sensor",
    "networkSwitch",
    "router4g",
    "screen",
    "rack",
    "box",
    "wall",
    "door",
    "gate",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Infra,
    Site,
}

pub fn tone(name: &str) -> Tone {
    match name {
        "screen" | "rack" | "networkSwitch" | "router4g" | "server" | "cloud" | "database"
        | "laptop" | "phone" | "erp" | "api" | "sensor" => Tone::Infra,
        _ => Tone::Site,
    }
}

/// Stroke colour, tint fill for an icon's tone.
pub fn colors(name: &str) -> (&'static str, &'static str) {
    match tone(name) {
        Tone::Infra => (theme::BLUE, theme::BLUE_TINT),
        Tone::Site => (theme::SLATE, theme::SLATE_TINT),
    }
}

pub fn is_known(name: &str) -> bool {
    ICON_NAMES.contains(&name)
}

/// Path data on the 24 grid. `F` marks a filled detail (drawn with the stroke colour).
fn shapes(name: &str) -> &'static str {
    match name {
        "person" | "user" => "M12 4a3 3 0 1 0 0.01 0 M6 21v-4a6 6 0 0 1 12 0v4",
        "screen" => "M3 4h18v12H3z M12 16v4 M8 20h8",
        "rack" => "M5 2h14v20H5z M8 5h8 M8 8h8 M8 11h8 M8 14h8 M8 19h2",
        "networkSwitch" => "M2 8h20v8H2z M5 11h2v2H5z M8.5 11h2v2h-2z M12 11h2v2h-2z M15.5 11h2v2h-2z|F M20 10.5a0.6 0.6 0 1 0 0.01 0",
        "router4g" => "M3 13h18v6H3z M7 13V8 M17 13V8 M9 5.5a4.5 4.5 0 0 1 6 0 M10.5 7.5a2 2 0 0 1 3 0 M6 16h1 M9 16h1",
        "box" => "M4 7l8-4 8 4v10l-8 4-8-4z M4 7l8 4 8-4 M12 11v10",
        "wall" => "M3 5h18v14H3z M3 9.7h18 M3 14.3h18 M9 5v4.7 M15 5v4.7 M6 9.7v4.6 M12 9.7v4.6 M18 9.7v4.6 M9 14.3V19 M15 14.3V19",
        "server" => "M4 3h16v6H4z M4 9h16v6H4z M4 15h16v6H4z M7 6h4 M7 12h4 M7 18h4|F M16.5 6a0.8 0.8 0 1 0 0.01 0 M16.5 12a0.8 0.8 0 1 0 0.01 0 M16.5 18a0.8 0.8 0 1 0 0.01 0",
        "cloud" => "M7 18h10.5a4 4 0 0 0 0.5-8a6 6 0 0 0-11.5 1.5A3.3 3.3 0 0 0 7 18z",
        "database" => "M5 6c0-3 14-3 14 0v12c0 3-14 3-14 0z M5 6c0 3 14 3 14 0 M5 12c0 3 14 3 14 0",
        "laptop" => "M5 5h14v10H5z M2 18h20l-2-3H4z",
        "phone" => "M7 2h10v20H7z M10.5 19h3 M7 5h10",
        "door" => "M6 3h12v18H6z M3 21h18 M14.5 12h1",
        "gate" => "M3 4v17 M21 4v17 M3 8h18 M3 17h18 M7 8v9 M11 8v9 M15 8v9 M19 8v9",
        "erp" => "M3 4h18v16H3z M3 8h18 M6 11h5v6H6z M13 11h5 M13 14h5 M13 17h3|F M5 6a0.7 0.7 0 1 0 0.01 0 M7.5 6a0.7 0.7 0 1 0 0.01 0",
        "api" => "M8 6l-5 6 5 6 M16 6l5 6-5 6 M13.5 4l-3 16",
        "sensor" => "M12 10a2 2 0 1 0 0.01 0 M8.5 8.5a5 5 0 0 0 0 7 M15.5 8.5a5 5 0 0 1 0 7 M5.5 5.5a9 9 0 0 0 0 13 M18.5 5.5a9 9 0 0 1 0 13",
        _ => "M4 4h16v16H4z M4 4l16 16 M20 4L4 20",
    }
}

/// An icon at (x, y) (top left), `size` px square. `stroke` overrides the tone colour.
pub fn draw(name: &str, x: f64, y: f64, size: f64, stroke: Option<&str>) -> String {
    let (c, _) = colors(name);
    let c = stroke.unwrap_or(c);
    let scale = size / 24.0;
    let (lines, filled) = match shapes(name).split_once("|F") {
        Some((l, f)) => (l, f.trim()),
        None => (shapes(name), ""),
    };
    let mut out = format!(
        r#"<g transform="translate({} {}) scale({})" fill="none" stroke="{c}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"><path d="{lines}"/>"#,
        n(x),
        n(y),
        n(scale),
        n(1.6)
    );
    if !filled.is_empty() {
        out.push_str(&format!(
            r#"<path d="{filled}" fill="{c}" stroke-width="1.2"/>"#
        ));
    }
    out.push_str("</g>");
    out
}
