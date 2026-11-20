//! `custom_svg`: a drawing Nary or the MCP client wrote, reduced to an allow-list before it is stored or
//! drawn. The output is rebuilt from the parsed tree (never copied through), so anything not listed here
//! cannot reach a page:
//!
//! - elements: svg, g, defs, linearGradient, radialGradient, stop, clipPath, rect, circle, ellipse, line,
//!   polyline, polygon, path, text, tspan, title, desc, marker, use; anything else is dropped with its
//!   whole subtree (script, foreignObject, image, style, a, iframe, animate, ...);
//! - attributes: geometry, transform, paint and stroke, opacity, font and text placement, markers,
//!   viewBox, id; `href` only on `use` and only to a local `#id`; `url(...)` only as `url(#id)`;
//!   event handlers, `style`, `class` and everything else are dropped;
//! - size and element caps; DOCTYPE, processing instructions and comments are dropped; a viewBox is
//!   forced from width / height when missing.

use std::fmt::Write;

use once_cell::sync::Lazy;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use regex::Regex;

use super::spec::CustomSvgSpec;
use super::svg::esc;
use super::{Result, VisualError};

/// `proposal.visuals.custom_svg_max_kb` default.
pub const DEFAULT_MAX_KB: usize = 300;
/// Cap when drawing an already guarded spec (the Setting may be raised above the default).
pub const RENDER_MAX_KB: usize = 4096;
/// Most elements a drawing may have.
pub const MAX_ELEMENTS: usize = 4000;
/// Deepest nesting.
pub const MAX_DEPTH: usize = 32;

const ELEMENTS: [&str; 20] = [
    "svg",
    "g",
    "defs",
    "linearGradient",
    "radialGradient",
    "stop",
    "clipPath",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "path",
    "text",
    "tspan",
    "title",
    "desc",
    "marker",
    "use",
];

const ATTRIBUTES: [&str; 72] = [
    // geometry
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "fx",
    "fy",
    "width",
    "height",
    "d",
    "points",
    "dx",
    "dy",
    "rotate",
    "textLength",
    "lengthAdjust",
    "offset",
    "pathLength",
    // coordinate systems
    "viewBox",
    "preserveAspectRatio",
    "transform",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "clipPathUnits",
    "markerWidth",
    "markerHeight",
    "markerUnits",
    "refX",
    "refY",
    "orient",
    // paint
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-miterlimit",
    "opacity",
    "stop-color",
    "stop-opacity",
    "clip-path",
    "clip-rule",
    "visibility",
    "paint-order",
    // text
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "font-variant",
    "text-anchor",
    "dominant-baseline",
    "alignment-baseline",
    "baseline-shift",
    "letter-spacing",
    "word-spacing",
    "text-decoration",
    // markers, ids
    "marker-start",
    "marker-mid",
    "marker-end",
    "id",
    "xmlns",
];

static URL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)url\s*\(").unwrap());
static LOCAL_URL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*url\(\s*#[A-Za-z_][A-Za-z0-9_.:-]*\s*\)\s*(none|[#a-zA-Z0-9(),. ]*)?$")
        .unwrap()
});
static LOCAL_HREF: Lazy<Regex> = Lazy::new(|| Regex::new(r"^#[A-Za-z_][A-Za-z0-9_.:-]*$").unwrap());
static ID: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_.:-]{0,63}$").unwrap());
static NUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*(\d+(?:\.\d+)?)\s*(px)?\s*$").unwrap());

/// A sanitised drawing: the rebuilt SVG, every piece of text in it (for the text guards) and what was
/// removed (for warnings).
#[derive(Clone, Debug, PartialEq)]
pub struct Sanitized {
    pub svg: String,
    pub texts: Vec<String>,
    pub removed: Vec<String>,
}

/// The SVG of a custom spec (sanitised again: what is stored is never trusted).
pub fn render(spec: &CustomSvgSpec, max_kb: usize) -> Result<String> {
    Ok(sanitize(&spec.svg, max_kb)?.svg)
}

fn note(removed: &mut Vec<String>, s: String) {
    if !removed.contains(&s) && removed.len() < 30 {
        removed.push(s);
    }
}

/// `sanitize(svg, max_kb)`: refuse what cannot be made safe (too big, not an SVG, broken XML), strip the
/// rest down to the allow-list.
pub fn sanitize(input: &str, max_kb: usize) -> Result<Sanitized> {
    let max_kb = if max_kb == 0 { DEFAULT_MAX_KB } else { max_kb };
    if input.len() > max_kb * 1024 {
        return Err(VisualError::Invalid(format!(
            "the drawing is {} KB; at most {max_kb} KB",
            input.len().div_ceil(1024)
        )));
    }
    let mut reader = Reader::from_str(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;

    let mut out = String::with_capacity(input.len());
    let mut texts: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    // Kept element names (for closing tags); None = this level is a dropped subtree.
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut skip_depth = 0usize;
    let mut elements = 0usize;
    let mut seen_root = false;
    let mut root_closed = false;
    let mut text_depth = 0usize;
    let mut cur_text = String::new();

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| VisualError::Invalid(format!("the drawing is not valid SVG/XML: {e}")))?;
        match ev {
            Event::Eof => break,
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if skip_depth > 0 {
                    skip_depth += 1;
                    stack.push(None);
                    continue;
                }
                if root_closed {
                    return Err(VisualError::Invalid(
                        "the drawing has content after the closing </svg>".into(),
                    ));
                }
                elements += 1;
                check_limits(elements, stack.len())?;
                if !seen_root {
                    if name != "svg" {
                        return Err(VisualError::Invalid(
                            "the drawing must be one <svg> element".into(),
                        ));
                    }
                    seen_root = true;
                    open_tag(&mut out, &e, &name, true, &mut removed)?;
                    out.push('>');
                    stack.push(Some(name));
                    continue;
                }
                if !ELEMENTS.contains(&name.as_str()) {
                    note(&mut removed, format!("<{name}> element"));
                    skip_depth = 1;
                    stack.push(None);
                    continue;
                }
                open_tag(&mut out, &e, &name, false, &mut removed)?;
                out.push('>');
                if matches!(name.as_str(), "text" | "title" | "desc") && text_depth == 0 {
                    cur_text.clear();
                }
                if matches!(name.as_str(), "text" | "title" | "desc" | "tspan") {
                    text_depth += 1;
                }
                stack.push(Some(name));
            }
            Event::Empty(_) if skip_depth > 0 => {}
            Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if !seen_root {
                    return Err(VisualError::Invalid(
                        "the drawing must be one <svg> element".into(),
                    ));
                }
                if root_closed {
                    return Err(VisualError::Invalid(
                        "the drawing has content after the closing </svg>".into(),
                    ));
                }
                elements += 1;
                check_limits(elements, stack.len())?;
                if !ELEMENTS.contains(&name.as_str()) || name == "svg" {
                    note(&mut removed, format!("<{name}> element"));
                    continue;
                }
                open_tag(&mut out, &e, &name, false, &mut removed)?;
                out.push_str("/>");
            }
            Event::End(_) => match stack.pop() {
                Some(None) => {
                    skip_depth = skip_depth.saturating_sub(1);
                }
                Some(Some(name)) => {
                    if matches!(name.as_str(), "text" | "title" | "desc" | "tspan") {
                        text_depth = text_depth.saturating_sub(1);
                        if text_depth == 0 {
                            let t = cur_text.split_whitespace().collect::<Vec<_>>().join(" ");
                            if !t.is_empty() {
                                texts.push(t);
                            }
                            cur_text.clear();
                        }
                    }
                    let _ = write!(out, "</{name}>");
                    if stack.is_empty() {
                        root_closed = true;
                    }
                }
                None => {
                    return Err(VisualError::Invalid(
                        "the drawing's tags do not match".into(),
                    ))
                }
            },
            Event::Text(t) => {
                if skip_depth > 0 || stack.is_empty() {
                    continue;
                }
                let s = t.unescape().map_err(|e| {
                    VisualError::Invalid(format!("the drawing has a bad character reference: {e}"))
                })?;
                if text_depth > 0 {
                    cur_text.push_str(&s);
                    cur_text.push(' ');
                    out.push_str(&esc(&s));
                } else if !s.trim().is_empty() {
                    note(&mut removed, "text outside <text>".into());
                }
            }
            Event::CData(t) => {
                if skip_depth > 0 || text_depth == 0 {
                    continue;
                }
                let s = String::from_utf8_lossy(&t.into_inner()).to_string();
                cur_text.push_str(&s);
                cur_text.push(' ');
                out.push_str(&esc(&s));
            }
            Event::DocType(_) => note(&mut removed, "DOCTYPE".into()),
            Event::PI(_) => note(&mut removed, "processing instruction".into()),
            Event::Comment(_) | Event::Decl(_) => {}
        }
    }
    if !seen_root || !root_closed {
        return Err(VisualError::Invalid(
            "the drawing must be one complete <svg> element".into(),
        ));
    }
    Ok(Sanitized {
        svg: out,
        texts,
        removed,
    })
}

fn check_limits(elements: usize, depth: usize) -> Result<()> {
    if elements > MAX_ELEMENTS {
        return Err(VisualError::Invalid(format!(
            "the drawing has more than {MAX_ELEMENTS} elements"
        )));
    }
    if depth >= MAX_DEPTH {
        return Err(VisualError::Invalid(format!(
            "the drawing nests deeper than {MAX_DEPTH} levels"
        )));
    }
    Ok(())
}

/// Write `<name attrs...` (no closing `>`). The root also gets xmlns and a viewBox.
fn open_tag(
    out: &mut String,
    e: &BytesStart,
    name: &str,
    root: bool,
    removed: &mut Vec<String>,
) -> Result<()> {
    let mut attrs: Vec<(String, String)> = Vec::new();
    for a in e.attributes().with_checks(true) {
        let a = a.map_err(|err| {
            VisualError::Invalid(format!("the drawing has a bad attribute: {err}"))
        })?;
        let key = String::from_utf8_lossy(a.key.as_ref()).to_string();
        let val = a
            .unescape_value()
            .map_err(|err| {
                VisualError::Invalid(format!("the drawing has a bad attribute value: {err}"))
            })?
            .to_string();
        let lower = key.to_ascii_lowercase();
        if lower.starts_with("on") {
            note(removed, format!("{key} event handler"));
            continue;
        }
        if key == "href" || key == "xlink:href" {
            if name == "use" && LOCAL_HREF.is_match(val.trim()) {
                attrs.push(("href".into(), val.trim().to_string()));
            } else {
                note(removed, format!("{key}=\"{}\" link", short(&val)));
            }
            continue;
        }
        if key == "xmlns:xlink" {
            continue;
        }
        if !ATTRIBUTES.contains(&key.as_str()) {
            note(removed, format!("{key} attribute"));
            continue;
        }
        if key == "xmlns" {
            continue;
        }
        let vl = val.to_ascii_lowercase();
        if vl.contains("javascript:")
            || vl.contains("data:")
            || vl.contains("expression(")
            || vl.contains('<')
        {
            note(removed, format!("{key} value"));
            continue;
        }
        if URL.is_match(&val) && !LOCAL_URL.is_match(&val) {
            note(
                removed,
                format!("{key}=\"{}\" outside reference", short(&val)),
            );
            continue;
        }
        if key == "id" && !ID.is_match(&val) {
            note(removed, format!("id \"{}\"", short(&val)));
            continue;
        }
        attrs.push((key, val));
    }
    let _ = write!(out, "<{name}");
    if root {
        out.push_str(r#" xmlns="http://www.w3.org/2000/svg""#);
        if !attrs.iter().any(|(k, _)| k == "viewBox") {
            let num = |k: &str| {
                attrs
                    .iter()
                    .find(|(a, _)| a == k)
                    .and_then(|(_, v)| NUM.captures(v))
                    .and_then(|c| c[1].parse::<f64>().ok())
            };
            match (num("width"), num("height")) {
                (Some(w), Some(h)) if w > 0.0 && h > 0.0 => {
                    attrs.push((
                        "viewBox".into(),
                        format!("0 0 {} {}", super::svg::n(w), super::svg::n(h)),
                    ));
                }
                _ => {
                    return Err(VisualError::Invalid(
                        "the drawing needs a viewBox (or a numeric width and height)".into(),
                    ))
                }
            }
        }
        // Text the drawing does not style uses the proposal's font, not the browser's default serif.
        if !attrs.iter().any(|(k, _)| k == "font-family") {
            attrs.push(("font-family".into(), super::theme::FONT_FAMILY.into()));
        }
    }
    for (k, v) in attrs {
        let _ = write!(out, r#" {k}="{}""#, esc(&v));
    }
    Ok(())
}

fn short(s: &str) -> String {
    s.chars().take(40).collect()
}
