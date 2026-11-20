//! A tiny SVG writer. Every text and attribute value from a spec goes through [`esc`]; numbers through
//! [`n`] (two decimals at most, so the same spec gives the same bytes).

use std::fmt::Write;

use super::text;
use super::theme;

/// XML escape for text content and attribute values.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // Control characters are not allowed in XML 1.0.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            // No em dashes in anything the ERP prints.
            '\u{2014}' => out.push_str(", "),
            c => out.push(c),
        }
    }
    out
}

/// A coordinate: rounded to 2 decimals, trailing zeros dropped, never "-0".
pub fn n(v: f64) -> String {
    let v = if v.is_finite() {
        (v * 100.0).round() / 100.0
    } else {
        0.0
    };
    let v = if v == 0.0 { 0.0 } else { v };
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

impl Anchor {
    fn as_str(&self) -> &'static str {
        match self {
            Anchor::Start => "start",
            Anchor::Middle => "middle",
            Anchor::End => "end",
        }
    }
}

/// Text style for [`Doc::text`].
#[derive(Clone, Copy, Debug)]
pub struct TextStyle<'a> {
    pub size: f64,
    pub fill: &'a str,
    pub bold: bool,
    pub anchor: Anchor,
    /// A white outline behind the glyphs (readable over lines and markers).
    pub halo: bool,
}

impl<'a> TextStyle<'a> {
    pub fn new(size: f64, fill: &'a str) -> Self {
        TextStyle {
            size,
            fill,
            bold: false,
            anchor: Anchor::Start,
            halo: false,
        }
    }
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    pub fn halo(mut self) -> Self {
        self.halo = true;
        self
    }
    pub fn anchor(mut self, a: Anchor) -> Self {
        self.anchor = a;
        self
    }
}

/// An SVG document being written.
pub struct Doc {
    pub width: f64,
    pub height: f64,
    body: String,
    defs: String,
}

impl Doc {
    pub fn new(width: f64) -> Doc {
        Doc {
            width,
            height: 0.0,
            body: String::new(),
            defs: String::new(),
        }
    }

    pub fn body_mut(&mut self) -> &mut String {
        &mut self.body
    }

    pub fn raw(&mut self, s: &str) {
        self.body.push_str(s);
    }

    pub fn def(&mut self, s: &str) {
        self.defs.push_str(s);
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, rx: f64, attrs: &str) {
        let _ = write!(
            self.body,
            r#"<rect x="{}" y="{}" width="{}" height="{}"{} {attrs}/>"#,
            n(x),
            n(y),
            n(w.max(0.0)),
            n(h.max(0.0)),
            if rx > 0.0 {
                format!(r#" rx="{}""#, n(rx))
            } else {
                String::new()
            }
        );
    }

    pub fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, attrs: &str) {
        let _ = write!(
            self.body,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" {attrs}/>"#,
            n(x1),
            n(y1),
            n(x2),
            n(y2)
        );
    }

    pub fn circle(&mut self, cx: f64, cy: f64, r: f64, attrs: &str) {
        let _ = write!(
            self.body,
            r#"<circle cx="{}" cy="{}" r="{}" {attrs}/>"#,
            n(cx),
            n(cy),
            n(r)
        );
    }

    pub fn path(&mut self, d: &str, attrs: &str) {
        let _ = write!(self.body, r#"<path d="{d}" {attrs}/>"#);
    }

    /// One line of text (escaped here).
    pub fn text(&mut self, x: f64, y: f64, s: &str, st: TextStyle) {
        if s.is_empty() {
            return;
        }
        let _ = write!(
            self.body,
            r#"<text x="{}" y="{}" font-size="{}" fill="{}"{}{}{}>{}</text>"#,
            n(x),
            n(y),
            n(st.size),
            st.fill,
            if st.bold { r#" font-weight="600""# } else { "" },
            if st.halo {
                r#" stroke="white" stroke-width="3" stroke-linejoin="round" paint-order="stroke""#
            } else {
                ""
            },
            if st.anchor != Anchor::Start {
                format!(r#" text-anchor="{}""#, st.anchor.as_str())
            } else {
                String::new()
            },
            esc(s)
        );
    }

    /// Several lines, the first baseline at `y`, `lh` apart.
    pub fn lines(&mut self, x: f64, y: f64, lines: &[String], lh: f64, st: TextStyle) {
        for (i, l) in lines.iter().enumerate() {
            self.text(x, y + lh * i as f64, l, st);
        }
    }

    /// A text that must fit `max_w`: truncated with an ellipsis when it does not.
    pub fn text_fit(&mut self, x: f64, y: f64, s: &str, max_w: f64, st: TextStyle) {
        let t = text::truncate(s, max_w, st.size, st.bold);
        self.text(x, y, &t, st);
    }

    /// The finished document: viewBox, white paper, the bundled font family, no external references.
    pub fn finish(self, title: &str) -> String {
        let h = self.height.ceil();
        let mut out = String::with_capacity(self.body.len() + 512);
        let _ = write!(
            out,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" font-family="{font}" role="img">"#,
            w = n(self.width),
            h = n(h),
            font = esc(theme::FONT_FAMILY)
        );
        if !title.is_empty() {
            let _ = write!(out, "<title>{}</title>", esc(title));
        }
        if !self.defs.is_empty() {
            let _ = write!(out, "<defs>{}</defs>", self.defs);
        }
        let _ = write!(
            out,
            r#"<rect x="0" y="0" width="{}" height="{}" fill="{}"/>"#,
            n(self.width),
            n(h),
            theme::PAPER
        );
        out.push_str(&self.body);
        out.push_str("</svg>");
        out
    }
}
