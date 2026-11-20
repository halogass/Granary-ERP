//! Text measuring with the bundled IBM Plex Sans (the same font resvg draws with), wrapping and ellipsis.
//! Browsers without Plex fall back to Arial, which is narrower, so a label that fits here fits there.

use once_cell::sync::Lazy;

pub static FONT_REGULAR: &[u8] = include_bytes!("fonts/IBMPlexSans-400.ttf");
pub static FONT_SEMIBOLD: &[u8] = include_bytes!("fonts/IBMPlexSans-600.ttf");

struct Widths {
    upem: f64,
    /// Advance per char for U+0000..U+024F (0 = missing glyph).
    table: Vec<u16>,
    fallback: f64,
}

fn load(data: &'static [u8]) -> Widths {
    let face = ttf_parser::Face::parse(data, 0).expect("bundled font");
    let upem = face.units_per_em() as f64;
    let table = (0u32..0x2200)
        .map(|c| {
            char::from_u32(c)
                .and_then(|ch| face.glyph_index(ch))
                .and_then(|g| face.glyph_hor_advance(g))
                .unwrap_or(0)
        })
        .collect();
    Widths {
        upem,
        table,
        fallback: 0.6,
    }
}

static REGULAR: Lazy<Widths> = Lazy::new(|| load(FONT_REGULAR));
static SEMIBOLD: Lazy<Widths> = Lazy::new(|| load(FONT_SEMIBOLD));

/// Width of `s` at `size` px.
pub fn width(s: &str, size: f64, bold: bool) -> f64 {
    let w = if bold { &*SEMIBOLD } else { &*REGULAR };
    let mut units = 0.0;
    for ch in s.chars() {
        let adv = w.table.get(ch as usize).copied().unwrap_or(0);
        units += if adv == 0 {
            w.fallback * w.upem
        } else {
            adv as f64
        };
    }
    units / w.upem * size
}

pub const ELLIPSIS: &str = "\u{2026}";

/// `s` cut to fit `max_w` with an ellipsis (unchanged when it fits).
pub fn truncate(s: &str, max_w: f64, size: f64, bold: bool) -> String {
    let s = s.trim();
    if width(s, size, bold) <= max_w {
        return s.to_string();
    }
    let ell = width(ELLIPSIS, size, bold);
    let mut out = String::new();
    let mut w = 0.0;
    for ch in s.chars() {
        let cw = width(&ch.to_string(), size, bold);
        if w + cw + ell > max_w {
            break;
        }
        out.push(ch);
        w += cw;
    }
    let out = out.trim_end().to_string();
    if out.is_empty() {
        return String::new();
    }
    out + ELLIPSIS
}

/// Word wrap into at most `max_lines` lines of `max_w`; the last line gets an ellipsis when text is left.
/// A single word longer than a line is cut.
pub fn wrap(s: &str, max_w: f64, size: f64, bold: bool, max_lines: usize) -> Vec<String> {
    let max_lines = max_lines.max(1);
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < words.len() {
        let cand = if cur.is_empty() {
            words[i].to_string()
        } else {
            format!("{cur} {}", words[i])
        };
        if width(&cand, size, bold) <= max_w {
            cur = cand;
            i += 1;
            continue;
        }
        if cur.is_empty() {
            // One word wider than the line.
            cur = truncate(words[i], max_w, size, bold);
            i += 1;
        }
        lines.push(std::mem::take(&mut cur));
        if lines.len() == max_lines {
            break;
        }
    }
    if lines.len() < max_lines && !cur.is_empty() {
        lines.push(cur);
    } else if i < words.len() || !cur.is_empty() {
        // Text left over: ellipsis on the last line.
        if let Some(last) = lines.last_mut() {
            let rest: Vec<&str> = std::iter::once(last.as_str())
                .chain(words[i.min(words.len())..].iter().copied())
                .collect();
            let joined = rest.join(" ");
            let mut t = truncate(&joined, max_w, size, bold);
            if !t.ends_with(ELLIPSIS) {
                t = truncate(&(joined + " " + ELLIPSIS), max_w, size, bold);
            }
            *last = t;
        }
    }
    lines
}

/// The widest of the lines.
pub fn max_width(lines: &[String], size: f64, bold: bool) -> f64 {
    lines
        .iter()
        .map(|l| width(l, size, bold))
        .fold(0.0, f64::max)
}
