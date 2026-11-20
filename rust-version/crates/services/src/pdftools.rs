//! Rust equivalents of the PyMuPDF (fitz) operations the backend uses, on permissively licensed pure-Rust
//! libraries: `lopdf` (MIT) holds the PDF object model (open, decrypt, edit, save) and `hayro`
//! (MIT OR Apache-2.0) interprets pages (rendering, text and image positions).
//!
//! - routers/signing.py (flattening signature stamps / field values onto the original):
//!   `fitz.open(stream=...)`, `page.rect`, `page.insert_image`, `page.draw_line`, `page.insert_textbox`,
//!   `fitz.get_text_length`, `doc.tobytes(garbage=4, deflate=True, clean=True)`;
//! - services/ai/pdf_import.py: `doc.is_encrypted`, `doc.authenticate`, `page.get_text()`,
//!   `fitz.open()` + `insert_pdf(from_page, to_page)` + `tobytes()`;
//! - services/verification/signing_flow.py: `page.search_for(anchor)`;
//! - tests: `new_page`, `insert_text`, `get_image_info`, page rendering (`get_pixmap`).
//!
//! The drawing operations (Shape, insert_text, insert_textbox, insert_image) are PyMuPDF Python code that
//! writes content-stream text; they are ported line by line, so the content streams match what PyMuPDF
//! wrote, apart from the last digit of some numbers (`format_g` prints the shortest float form). Base-14
//! fonts are referenced, not embedded: the PDF viewer supplies them, as the PDF standard requires.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::{
    interpret_page, BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image,
    ImageDrawProps, InterpreterCache, InterpreterSettings, SoftMask, TransformExt,
};
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::{Affine, BezPath, Point as KPoint};
use lopdf::{Dictionary, Object, ObjectId, Stream, StringFormat};

mod base14;

/// Error raised by a PDF operation (the text PyMuPDF's exception would carry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfError(pub String);

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PdfError {}

impl From<std::io::Error> for PdfError {
    fn from(e: std::io::Error) -> Self {
        PdfError(e.to_string())
    }
}

impl From<lopdf::Error> for PdfError {
    fn from(e: lopdf::Error) -> Self {
        PdfError(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, PdfError>;

fn err<T>(s: &str) -> Result<T> {
    Err(PdfError(s.to_string()))
}

/// `fitz.Point`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

/// `fitz.Rect`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Rect { x0, y0, x1, y1 }
    }
    /// `Rect.width`: `max(0, x1 - x0)`.
    pub fn width(&self) -> f64 {
        (self.x1 - self.x0).max(0.0)
    }
    /// `Rect.height`: `max(0, y1 - y0)`.
    pub fn height(&self) -> f64 {
        (self.y1 - self.y0).max(0.0)
    }
    /// `Rect.is_empty`: `x0 >= x1 or y0 >= y1`.
    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
    /// `Rect.contains(rect)` for a rect argument (an empty rect is contained by anything).
    pub fn contains(&self, r: &Rect) -> bool {
        r.is_empty() || (self.x0 <= r.x0 && r.x1 <= self.x1 && self.y0 <= r.y0 && r.y1 <= self.y1)
    }
    /// `Rect.intersects(rect)`.
    pub fn intersects(&self, r: &Rect) -> bool {
        !self.is_empty()
            && !r.is_empty()
            && self.x0 < r.x1
            && r.x0 < self.x1
            && self.y0 < r.y1
            && r.y0 < self.y1
    }
}

/// `insert_textbox` / `insert_text` options (PyMuPDF keyword defaults).
#[derive(Clone, Debug)]
pub struct TextOptions {
    pub fontsize: f64,
    pub fontname: String,
    pub color: Option<Vec<f64>>,
    pub align: i32,
}

impl Default for TextOptions {
    fn default() -> Self {
        TextOptions {
            fontsize: 11.0,
            fontname: "helv".into(),
            color: None,
            align: 0,
        }
    }
}

/// `Document.tobytes(...)` options (`garbage`, `deflate`, `clean`; everything else at PyMuPDF's defaults).
/// `garbage` >= 1 drops unreferenced objects and >= 3 also renumbers them; `deflate` compresses the
/// uncompressed streams; `clean` is accepted for compatibility and leaves content streams as written.
#[derive(Clone, Copy, Debug, Default)]
pub struct SaveOptions {
    pub garbage: i32,
    pub deflate: bool,
    pub clean: bool,
}

/// An RGB rendering of a page (`page.get_pixmap(matrix=Matrix(zoom, zoom))`, no alpha), rows packed.
#[derive(Clone, Debug)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub samples: Vec<u8>,
}

/// One entry of `page.get_image_info()` (only the bbox is used).
#[derive(Clone, Debug)]
pub struct ImageInfo {
    pub bbox: Rect,
}

// ─── number formatting: PDF content-stream numbers in MuPDF's %g style ──────

/// `_format_g(value)`: the shortest decimal digits that read back as the same *float* (f32), without
/// exponent and without a leading zero (`.5`, `-0`, `841.8898`).
pub fn format_g(v: f64) -> String {
    let f = v as f32;
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let mut out = String::new();
    if f.is_sign_negative() {
        out.push('-');
    }
    if f == 0.0 {
        out.push('0');
        return out;
    }
    // "d.ddde-x": the shortest round-trip digits and the decimal exponent of the first one
    let sci = format!("{:e}", f.abs());
    let (mant, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let digits: Vec<u8> = mant.bytes().filter(|b| b.is_ascii_digit()).collect();
    let mut point: i32 = exp.parse::<i32>().unwrap_or(0) + 1;
    if point <= 0 {
        out.push('.');
        while point < 0 {
            out.push('0');
            point += 1;
        }
        out.extend(digits.iter().map(|d| *d as char));
    } else {
        let mut nd = digits.len();
        for d in &digits {
            out.push(*d as char);
            nd -= 1;
            point -= 1;
            if point == 0 && nd > 0 {
                out.push('.');
            }
        }
        while point > 0 {
            out.push('0');
            point -= 1;
        }
    }
    out
}

fn format_g_seq(vs: &[f64]) -> String {
    vs.iter()
        .map(|v| format_g(*v))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Python `round(x, 5)` (correctly rounded on the binary value).
fn py_round5(x: f64) -> f64 {
    format!("{:.5}", x).parse().unwrap_or(x)
}

/// `JM_TUPLE`: round to 5 places, tiny values to 0.
fn jm_tuple(vs: &[f64]) -> Vec<f64> {
    vs.iter()
        .map(|&x| if x.abs() >= 1e-4 { py_round5(x) } else { 0.0 })
        .collect()
}

/// `ColorCode(c, f)`.
fn color_code(c: Option<&[f64]>, f: char) -> Result<String> {
    let Some(c) = c else { return Ok(String::new()) };
    if c.is_empty() {
        return Ok(String::new());
    }
    if !matches!(c.len(), 1 | 3 | 4) || c.iter().any(|v| !(0.0..=1.0).contains(v)) {
        return err("need 1, 3 or 4 color components in range 0 to 1");
    }
    let s = format_g_seq(c) + " ";
    Ok(match (c.len(), f) {
        (1, 'c') => s + "G ",
        (1, _) => s + "g ",
        (3, 'c') => s + "RG ",
        (3, _) => s + "rg ",
        (_, 'c') => s + "K ",
        _ => s + "k ",
    })
}

/// Python `str.splitlines()`.
fn py_splitlines(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\r' => {
                out.push(std::mem::take(&mut cur));
                if chars.get(i + 1) == Some(&'\n') {
                    i += 1;
                }
            }
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
        i += 1;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `getTJstr` for simple (non-Symbol/ZapfDingbats) fonts: 2 hex digits per char, `b7` above 255.
fn tj_str(text: &str) -> String {
    let mut s = String::from("[<");
    for c in text.chars() {
        let o = c as u32;
        if o < 256 {
            s.push_str(&format!("{:02x}", o));
        } else {
            s.push_str("b7");
        }
    }
    s.push_str(">]");
    s
}

/// PyMuPDF's `Base14_fontdict` (lower-case keys).
fn base14_name(fontname: &str) -> Option<&'static str> {
    Some(match fontname.to_lowercase().as_str() {
        "helv" | "helvetica" => "Helvetica",
        "heit" | "helvetica-oblique" => "Helvetica-Oblique",
        "hebo" | "helvetica-bold" => "Helvetica-Bold",
        "hebi" | "helvetica-boldoblique" => "Helvetica-BoldOblique",
        "cour" | "courier" => "Courier",
        "coit" | "courier-oblique" => "Courier-Oblique",
        "cobo" | "courier-bold" => "Courier-Bold",
        "cobi" | "courier-boldoblique" => "Courier-BoldOblique",
        "tiro" | "times-roman" => "Times-Roman",
        "tibo" | "times-bold" => "Times-Bold",
        "tiit" | "times-italic" => "Times-Italic",
        "tibi" | "times-bolditalic" => "Times-BoldItalic",
        "symb" | "symbol" => "Symbol",
        "zadb" | "zapfdingbats" => "ZapfDingbats",
        _ => return None,
    })
}

/// The metrics of a Base-14 font by its PDF BaseFont name: the 14 standard names, and the names of
/// the metric-compatible URW fonts earlier versions of this module embedded (subset prefix ignored).
fn base14_metrics(basefont: &str) -> Option<&'static base14::Base14Metrics> {
    let n = match basefont.split_once('+') {
        Some((prefix, rest)) if prefix.len() == 6 => rest,
        _ => basefont,
    };
    let n = match n {
        "NimbusSans-Regular" => "Helvetica",
        "NimbusSans-Italic" | "NimbusSans-Oblique" => "Helvetica-Oblique",
        "NimbusSans-Bold" => "Helvetica-Bold",
        "NimbusSans-BoldItalic" | "NimbusSans-BoldOblique" => "Helvetica-BoldOblique",
        "NimbusMonoPS-Regular" => "Courier",
        "NimbusMonoPS-Italic" => "Courier-Oblique",
        "NimbusMonoPS-Bold" => "Courier-Bold",
        "NimbusMonoPS-BoldItalic" => "Courier-BoldOblique",
        "NimbusRoman-Regular" => "Times-Roman",
        "NimbusRoman-Italic" => "Times-Italic",
        "NimbusRoman-Bold" => "Times-Bold",
        "NimbusRoman-BoldItalic" => "Times-BoldItalic",
        "StandardSymbolsPS" => "Symbol",
        "D050000L" => "ZapfDingbats",
        other => other,
    };
    base14::BASE14.iter().find(|m| m.name == n)
}

fn metrics_of(name: &str) -> Result<&'static base14::Base14Metrics> {
    base14_metrics(name).ok_or_else(|| PdfError(format!("Font '{name}' is unsupported")))
}

/// Advance (em) of the glyph a Base-14 font shows for Unicode `u` (0..=255), as the f32 the font
/// engine works in; `None` when the font has no glyph for it.
fn base14_advance(m: &base14::Base14Metrics, u: u32) -> Option<f32> {
    let w = *m.widths.get(u as usize)?;
    (w > 0).then(|| w as f32 / 1000.0)
}

/// Windows-1252 code of a Unicode character, `None` when it has none (`fz_windows_1252_from_unicode`).
fn cp1252_from_unicode(c: char) -> Option<u32> {
    let u = c as u32;
    if u < 0x80 || (0xA0..=0xFF).contains(&u) {
        return Some(u);
    }
    base14::CP1252_EXTRA
        .iter()
        .find(|(x, _)| *x == u)
        .map(|(_, code)| *code as u32)
}

/// `fitz.get_text_length(text, fontname, fontsize)` (`util_measure_string`). Quirk reproduced: the loop
/// index walks the Python string by the UTF-8 byte length of each character, so after a non-ASCII
/// character the following character(s) are skipped; and the glyph is looked up by its *cp1252 code*
/// (a code without a glyph measures as the font's missing-glyph advance, which is its space width).
pub fn get_text_length(text: &str, fontname: &str, fontsize: f64) -> Result<f64> {
    let lower = fontname.to_lowercase();
    let Some(base) = base14_name(&lower) else {
        if [
            "china-t", "china-s", "china-ts", "china-ss", "japan", "japan-s", "korea", "korea-s",
        ]
        .contains(&lower.as_str())
        {
            return Ok(text.chars().count() as f64 * fontsize);
        }
        return Err(PdfError(format!("Font '{lower}' is unsupported")));
    };
    if base == "Symbol" || base == "ZapfDingbats" {
        return err("Symbol / ZapfDingbats widths are not supported");
    }
    let m = metrics_of(base)?;
    let notdef = base14_advance(m, 32).unwrap_or(0.0);
    let chars: Vec<char> = text.chars().collect();
    let mut w = 0f64;
    let mut pos = 0usize;
    while pos < chars.len() {
        let c = chars[pos];
        pos += c.len_utf8();
        let code = cp1252_from_unicode(c).unwrap_or(0xB7);
        w += base14_advance(m, code).unwrap_or(notdef) as f64;
    }
    Ok(w * fontsize)
}

// ─── fonts as PyMuPDF records them (doc.FontInfos) ───────────────────────────

#[derive(Clone, Debug)]
struct FontInfo {
    name: String,
    simple: bool,
    ascender: f64,
    descender: f64,
    /// Embedded font program (widths come from it when the name is not a Base-14 font).
    data: Option<Vec<u8>>,
}

/// (ascender, descender) of an OpenType / TrueType font program, in em.
fn sfnt_extents(data: &[u8]) -> Option<(f64, f64)> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let upem = face.units_per_em() as f64;
    if upem <= 0.0 {
        return None;
    }
    Some((
        (face.ascender() as f64 / upem) as f32 as f64,
        (face.descender() as f64 / upem) as f32 as f64,
    ))
}

/// A PDF document (`fitz.Document`).
pub struct Document {
    doc: lopdf::Document,
    font_infos: Vec<(i32, FontInfo)>,
    /// `doc.InsertedImages`: image bytes -> xref.
    inserted_images: HashMap<Vec<u8>, i32>,
    encrypted: bool,
    /// The bytes the document was opened from (kept for `authenticate`).
    source: Option<Arc<Vec<u8>>>,
    /// A serialized form of the current document and its password, for the page interpreter. It is
    /// the opened bytes while nothing has changed, and is rebuilt after an edit.
    snapshot: Mutex<Option<(Arc<Vec<u8>>, String)>>,
}

/// Geometry PyMuPDF's Shape uses for a page.
struct PageGeom {
    /// mediabox_size.y
    height: f64,
    /// cropbox_position (PyMuPDF's flipped cropbox top-left)
    x: f64,
    y: f64,
    /// inverse of page.transformation_matrix (a, b, c, d, e, f)
    ipctm: [f64; 6],
}

fn num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

fn xref_id(xref: i32) -> ObjectId {
    (xref.max(0) as u32, 0)
}

fn xref_num(id: ObjectId) -> i32 {
    id.0 as i32
}

/// A PDF text string (PDFDocEncoding where the text allows it, else UTF-16BE with a BOM).
fn text_string(value: &str) -> Object {
    let simple = value
        .chars()
        .all(|c| matches!(c as u32, 0x09 | 0x0A | 0x0D | 0x20..=0x7E | 0xA1..=0xAC | 0xAE..=0xFF));
    if simple {
        Object::String(
            value.chars().map(|c| c as u8).collect(),
            StringFormat::Literal,
        )
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in value.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, StringFormat::Hexadecimal)
    }
}

/// Decodes a PDF text string (UTF-16BE / UTF-8 with BOM, else PDFDocEncoding read as Latin-1).
fn decode_text_string(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let units: Vec<u16> = b[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if b.len() >= 3 && b[..3] == [0xEF, 0xBB, 0xBF] {
        return String::from_utf8_lossy(&b[3..]).into_owned();
    }
    b.iter().map(|&c| c as char).collect()
}

fn new_dict(pairs: Vec<(&str, Object)>) -> Dictionary {
    let mut d = Dictionary::new();
    for (k, v) in pairs {
        d.set(k, v);
    }
    d
}

/// The PDF dictionary of a non-embedded Base-14 font (WinAnsi encoding for the text fonts).
fn base14_font_dict(basefont: &str) -> Dictionary {
    let mut d = new_dict(vec![
        ("Type", Object::Name(b"Font".to_vec())),
        ("Subtype", Object::Name(b"Type1".to_vec())),
        ("BaseFont", Object::Name(basefont.as_bytes().to_vec())),
    ]);
    if basefont != "Symbol" && basefont != "ZapfDingbats" {
        d.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
    }
    d
}

impl Document {
    fn from_lopdf(doc: lopdf::Document, encrypted: bool, source: Option<Arc<Vec<u8>>>) -> Self {
        let snapshot = match (&source, encrypted) {
            (Some(s), false) => Some((s.clone(), String::new())),
            _ => None,
        };
        Document {
            doc,
            font_infos: vec![],
            inserted_images: HashMap::new(),
            encrypted,
            source,
            snapshot: Mutex::new(snapshot),
        }
    }

    /// `fitz.open()`: a new empty PDF.
    pub fn new() -> Result<Self> {
        let mut doc = lopdf::Document::with_version("1.7");
        let pages_id = doc.add_object(new_dict(vec![
            ("Type", Object::Name(b"Pages".to_vec())),
            ("Kids", Object::Array(vec![])),
            ("Count", Object::Integer(0)),
        ]));
        let catalog_id = doc.add_object(new_dict(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference(pages_id)),
        ]));
        doc.trailer.set("Root", Object::Reference(catalog_id));
        Ok(Self::from_lopdf(doc, false, None))
    }

    /// `fitz.open(stream=data, filetype="pdf")`.
    pub fn open(stream: &[u8]) -> Result<Self> {
        if stream.is_empty() {
            return err("Cannot open empty stream.");
        }
        let fail = || PdfError("Failed to open stream".into());
        let doc = lopdf::Document::load_mem(stream).map_err(|_| fail())?;
        // still carrying /Encrypt: the empty user password did not open it
        let encrypted = doc.is_encrypted();
        if !encrypted && doc.catalog().is_err() {
            return Err(fail());
        }
        Ok(Self::from_lopdf(
            doc,
            encrypted,
            Some(Arc::new(stream.to_vec())),
        ))
    }

    /// The document for editing: the interpreter snapshot is rebuilt on the next read.
    fn edit(&mut self) -> &mut lopdf::Document {
        if let Ok(mut s) = self.snapshot.lock() {
            *s = None;
        }
        &mut self.doc
    }

    pub fn page_count(&self) -> usize {
        if self.encrypted {
            return 0;
        }
        self.doc.get_pages().len()
    }

    /// `doc.is_encrypted`: true until a successful `authenticate`.
    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    /// `doc.authenticate(password)` (truthy result).
    pub fn authenticate(&mut self, password: &str) -> bool {
        if !self.encrypted {
            return true;
        }
        let Some(src) = self.source.clone() else {
            return false;
        };
        let opts = lopdf::LoadOptions::with_password(password);
        match lopdf::Document::load_mem_with_options(&src, opts) {
            Ok(doc) if !doc.is_encrypted() && doc.catalog().is_ok() => {
                self.doc = doc;
                self.encrypted = false;
                if let Ok(mut s) = self.snapshot.lock() {
                    *s = Some((src, password.to_string()));
                }
                true
            }
            _ => false,
        }
    }

    fn check_page(&self, pno: usize) -> Result<()> {
        if pno >= self.page_count() {
            return err("page not in document");
        }
        Ok(())
    }

    fn page_id(&self, pno: usize) -> Result<ObjectId> {
        self.check_page(pno)?;
        self.doc
            .get_pages()
            .get(&(pno as u32 + 1))
            .copied()
            .ok_or(PdfError("page not in document".into()))
    }

    fn pages_root(&self) -> Result<ObjectId> {
        Ok(self.doc.catalog()?.get(b"Pages")?.as_reference()?)
    }

    /// The dictionary of an indirect object (a dictionary or a stream's dictionary).
    fn dict(&self, id: ObjectId) -> Option<&Dictionary> {
        match self.doc.get_object(id).ok()? {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    fn dict_mut(&mut self, id: ObjectId) -> Result<&mut Dictionary> {
        match self.edit().get_object_mut(id)? {
            Object::Dictionary(d) => Ok(d),
            Object::Stream(s) => Ok(&mut s.dict),
            _ => err("object is not a dictionary"),
        }
    }

    /// Follows references.
    fn deref<'a>(&'a self, o: &'a Object) -> &'a Object {
        let mut o = o;
        for _ in 0..32 {
            match o {
                Object::Reference(id) => match self.doc.get_object(*id) {
                    Ok(x) => o = x,
                    Err(_) => return &Object::Null,
                },
                _ => return o,
            }
        }
        &Object::Null
    }

    fn deref_dict<'a>(&'a self, o: &'a Object) -> Option<&'a Dictionary> {
        match self.deref(o) {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    /// `pdf_dict_get_inheritable`: the object that holds `key` (the page or an ancestor) and the value.
    fn inheritable(&self, page: ObjectId, key: &[u8]) -> Option<(ObjectId, &Object)> {
        let mut id = page;
        for _ in 0..64 {
            let d = self.dict(id)?;
            if let Ok(v) = d.get(key) {
                return Some((id, v));
            }
            id = d.get(b"Parent").ok()?.as_reference().ok()?;
        }
        None
    }

    fn rect_of(&self, o: Option<&Object>) -> Option<[f64; 4]> {
        let a = match self.deref(o?) {
            Object::Array(a) if a.len() >= 4 => a,
            _ => return None,
        };
        let mut v = [0f64; 4];
        for (i, slot) in v.iter_mut().enumerate() {
            // MuPDF reads box coordinates as floats
            *slot = num(self.deref(&a[i])).unwrap_or(0.0) as f32 as f64;
        }
        Some([
            v[0].min(v[2]),
            v[1].min(v[3]),
            v[0].max(v[2]),
            v[1].max(v[3]),
        ])
    }

    fn rotate_of(&self, page: ObjectId) -> i64 {
        self.inheritable(page, b"Rotate")
            .and_then(|(_, o)| num(self.deref(o)))
            .unwrap_or(0.0) as i64
    }

    /// `doc.new_page(width=..., height=...)` appended at the end; returns its number.
    pub fn new_page(&mut self, width: f64, height: f64) -> Result<usize> {
        let root = self.pages_root()?;
        let contents = self
            .edit()
            .add_object(Stream::new(Dictionary::new(), Vec::new()));
        let page = new_dict(vec![
            ("Type", Object::Name(b"Page".to_vec())),
            ("Parent", Object::Reference(root)),
            (
                "MediaBox",
                Object::Array(vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Real(width as f32),
                    Object::Real(height as f32),
                ]),
            ),
            ("Resources", Object::Dictionary(Dictionary::new())),
            ("Contents", Object::Reference(contents)),
        ]);
        let pid = self.edit().add_object(page);
        self.append_to_page_tree(root, pid)?;
        Ok(self.page_count() - 1)
    }

    fn append_to_page_tree(&mut self, root: ObjectId, pid: ObjectId) -> Result<()> {
        let rd = self.dict_mut(root)?;
        match rd.get_mut(b"Kids") {
            Ok(Object::Array(k)) => k.push(Object::Reference(pid)),
            _ => rd.set("Kids", Object::Array(vec![Object::Reference(pid)])),
        }
        let count = rd.get(b"Count").ok().and_then(num).unwrap_or(0.0) as i64;
        rd.set("Count", Object::Integer(count + 1));
        Ok(())
    }

    /// `page.rect` (fz_bound_page): the visible box (CropBox within the MediaBox) at the origin,
    /// width and height swapped for 90 / 270 degree rotations.
    pub fn page_rect(&self, pno: usize) -> Result<Rect> {
        let pid = self.page_id(pno)?;
        let mut mb = self
            .rect_of(self.inheritable(pid, b"MediaBox").map(|x| x.1))
            .unwrap_or([0.0, 0.0, 612.0, 792.0]);
        if mb[2] - mb[0] <= 0.0 || mb[3] - mb[1] <= 0.0 {
            mb = [0.0, 0.0, 612.0, 792.0];
        }
        let mut b = mb;
        if let Some(cb) = self.rect_of(self.inheritable(pid, b"CropBox").map(|x| x.1)) {
            let i = [
                cb[0].max(mb[0]),
                cb[1].max(mb[1]),
                cb[2].min(mb[2]),
                cb[3].min(mb[3]),
            ];
            if i[2] > i[0] && i[3] > i[1] {
                b = i;
            }
        }
        let (w, h) = ((b[2] - b[0]) as f32, (b[3] - b[1]) as f32);
        let rot = self.rotate_of(pid).rem_euclid(360);
        let (w, h) = if rot == 90 || rot == 270 {
            (h, w)
        } else {
            (w, h)
        };
        Ok(Rect::new(0.0, 0.0, w as f64, h as f64))
    }

    fn geom(&self, pno: usize) -> Result<PageGeom> {
        let pid = self.page_id(pno)?;
        // JM_mediabox: normalised, default letter, < 1pt degenerate -> unit rect
        let mut mb = self
            .rect_of(self.inheritable(pid, b"MediaBox").map(|x| x.1))
            .unwrap_or([0.0, 0.0, 612.0, 792.0]);
        if mb[2] - mb[0] <= 0.0 || mb[3] - mb[1] <= 0.0 {
            mb = [0.0, 0.0, 612.0, 792.0];
        }
        if mb[2] - mb[0] < 1.0 || mb[3] - mb[1] < 1.0 {
            mb = [0.0, 0.0, 1.0, 1.0];
        }
        let cb = match self.rect_of(self.inheritable(pid, b"CropBox").map(|x| x.1)) {
            Some(c) if c[2] > c[0] && c[3] > c[1] => c,
            _ => mb,
        };
        let rotate = self.rotate_of(pid);
        // page.transformation_matrix: rotation 0 -> pdf_page_transform (flip y about the cropbox),
        // otherwise PyMuPDF's Matrix(1, 0, 0, -1, 0, cropbox.height)
        let pctm = if rotate.rem_euclid(360) == 0 {
            [1.0, 0.0, 0.0, -1.0, -cb[0], cb[3]]
        } else {
            [1.0, 0.0, 0.0, -1.0, 0.0, cb[3] - cb[1]]
        };
        let ipctm = invert(pctm);
        Ok(PageGeom {
            height: mb[3],
            x: cb[0],
            y: mb[3] - cb[3],
            ipctm,
        })
    }

    // ── page interpretation (hayro) ──

    /// Serialized bytes (and password) of the current document for the page interpreter.
    fn snapshot(&self) -> Result<(Arc<Vec<u8>>, String)> {
        if self.encrypted {
            return err("document closed or encrypted");
        }
        let mut guard = self
            .snapshot
            .lock()
            .map_err(|_| PdfError("document state is unavailable".into()))?;
        if let Some(s) = guard.as_ref() {
            return Ok(s.clone());
        }
        let mut copy = self.doc.clone();
        let mut out = Vec::new();
        copy.save_to(&mut out)?;
        let s = (Arc::new(out), String::new());
        *guard = Some(s.clone());
        Ok(s)
    }

    /// Runs `f` on page `pno` as the page interpreter reads it.
    fn with_page<R>(&self, pno: usize, f: impl FnOnce(&Page<'_>) -> R) -> Result<R> {
        self.check_page(pno)?;
        let (bytes, password) = self.snapshot()?;
        let pdf = Pdf::new_with_password(bytes, &password)
            .map_err(|e| PdfError(format!("the PDF could not be read: {e:?}")))?;
        let pages = pdf.pages();
        let page = pages
            .get(pno)
            .ok_or(PdfError("page not in document".into()))?;
        Ok(f(page))
    }

    /// Text characters and image boxes of a page, in page coordinates (origin top left, y down).
    fn structured(&self, pno: usize) -> Result<TextDevice> {
        self.with_page(pno, |page| {
            let cache = InterpreterCache::new();
            let transform = page.initial_transform(true).to_kurbo();
            let (w, h) = page.render_dimensions();
            let mut ctx = Context::new(
                transform,
                hayro::kurbo::Rect::new(0.0, 0.0, w as f64, h as f64),
                &cache,
                page.xref(),
                InterpreterSettings::default(),
            );
            let mut dev = TextDevice::default();
            interpret_page(page, &mut ctx, &mut dev);
            dev
        })
    }

    /// `page.get_text()`: each text line followed by a newline, in content order.
    pub fn get_text(&self, pno: usize) -> Result<String> {
        let rect = self.page_rect(pno)?;
        let tp = self.structured(pno)?;
        let mut out = String::new();
        for line in &tp.lines {
            let mut last: Option<char> = None;
            for ch in &line.chars {
                if rects_overlap(&rect, &quad_bbox(&ch.quad)) {
                    last = Some(ch.c);
                    out.push(ch.c);
                }
            }
            if matches!(last, Some(c) if c != '\n') {
                out.push('\n');
            }
        }
        Ok(out)
    }

    /// `page.search_for(needle)`: rectangles of the hits (`JM_search_stext_page` + same-line joining).
    pub fn search_for(&self, pno: usize, needle: &str) -> Result<Vec<Rect>> {
        let rect = self.page_rect(pno)?;
        let tp = self.structured(pno)?;
        if needle.is_empty() {
            return Ok(vec![]);
        }
        // haystack: in-rect chars, '\n' after every line and after the (single) block
        let mut hay: Vec<char> = Vec::new();
        for line in &tp.lines {
            for ch in &line.chars {
                if rects_overlap(&rect, &quad_bbox(&ch.quad)) {
                    hay.push(ch.c);
                }
            }
            hay.push('\n');
        }
        hay.push('\n');
        let needle_chars: Vec<char> = needle.chars().collect();
        let mut quads: Vec<[f64; 8]> = Vec::new();
        let mut haystack = 0usize;
        let Some((mut begin, mut end)) = find_string(&hay, 0, &needle_chars) else {
            return Ok(vec![]);
        };
        let mut inside = false;
        'outer: for line in &tp.lines {
            for ch in &line.chars {
                if !rects_overlap(&rect, &quad_bbox(&ch.quad)) {
                    continue;
                }
                loop {
                    if !inside && haystack >= begin {
                        inside = true;
                    }
                    if inside {
                        if haystack < end {
                            on_highlight_char(&mut quads, line, ch);
                            break;
                        }
                        inside = false;
                        match find_string(&hay, haystack, &needle_chars) {
                            None => break 'outer,
                            Some((b2, e2)) => {
                                begin = b2;
                                end = e2;
                                continue;
                            }
                        }
                    }
                    break;
                }
                haystack += 1;
            }
            haystack += 1;
        }
        // Quad.rect, then join overlapping rects on the same line
        let mut val: Vec<Rect> = quads.iter().map(quad_bbox).collect();
        let mut i = 0;
        while i + 1 < val.len() {
            let (v1, v2) = (val[i], val[i + 1]);
            let inter = Rect::new(
                v1.x0.max(v2.x0),
                v1.y0.max(v2.y0),
                v1.x1.min(v2.x1),
                v1.y1.min(v2.y1),
            );
            if v1.y1 != v2.y1 || inter.is_empty() {
                i += 1;
                continue;
            }
            val[i] = Rect::new(
                v1.x0.min(v2.x0),
                v1.y0.min(v2.y0),
                v1.x1.max(v2.x1),
                v1.y1.max(v2.y1),
            );
            val.remove(i + 1);
        }
        Ok(val)
    }

    /// `page.get_image_info()` bboxes: every image drawn on the page, in content order.
    pub fn get_image_info(&self, pno: usize) -> Result<Vec<ImageInfo>> {
        let tp = self.structured(pno)?;
        Ok(tp
            .images
            .into_iter()
            .map(|bbox| ImageInfo { bbox })
            .collect())
    }

    /// `page.get_pixmap(matrix=fitz.Matrix(zoom, zoom))` as packed RGB rows on white.
    pub fn render_page(&self, pno: usize, zoom: f64) -> Result<RgbImage> {
        let rect = self.page_rect(pno)?;
        // fz_round_rect: the pixel box covering the scaled page
        let dim = |v: f64| (v * zoom - 0.001).ceil().max(1.0);
        let (w, h) = (dim(rect.width()), dim(rect.height()));
        if w > u16::MAX as f64 || h > u16::MAX as f64 {
            return err("the page is too large to render at this zoom");
        }
        let (w, h) = (w as u16, h as u16);
        let samples = self.with_page(pno, |page| {
            let cache = hayro::RenderCache::new();
            let mut ctx = hayro::vello_cpu::RenderContext::new(w, h);
            let transform = Affine::scale(zoom) * page.initial_transform(true).to_kurbo();
            hayro::render_into(
                page,
                &cache,
                &InterpreterSettings::default(),
                &hayro::RenderSettings::default(),
                &mut ctx,
                transform,
            );
            ctx.flush();
            let mut pixmap = hayro::vello_cpu::Pixmap::new(w, h);
            ctx.render_with(
                &mut pixmap,
                &mut hayro::vello_cpu::Resources::default(),
                hayro::vello_cpu::RasterizerSettings {
                    target_init: hayro::vello_cpu::TargetInit::Clear(
                        hayro::vello_cpu::color::palette::css::WHITE,
                    ),
                    ..Default::default()
                },
            );
            // opaque white background: premultiplied RGBA equals straight RGBA
            let rgba = pixmap.data_as_u8_slice();
            let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
            for px in rgba.as_chunks::<4>().0 {
                rgb.extend_from_slice(&px[..3]);
            }
            rgb
        })?;
        Ok(RgbImage {
            width: w as u32,
            height: h as u32,
            samples,
        })
    }

    // ── resources ──

    /// Id of the (indirect) dictionary stored under `key` in object `owner`, created when missing; a
    /// direct dictionary there is moved into its own object (same content, so other readers see no
    /// difference).
    fn dict_entry_id(&mut self, owner: ObjectId, key: &str) -> Result<ObjectId> {
        let cur = self
            .dict(owner)
            .ok_or(PdfError("object is not a dictionary".into()))?
            .get(key.as_bytes())
            .ok()
            .cloned();
        let target = match cur {
            Some(Object::Reference(r)) if self.dict(r).is_some() => return Ok(r),
            Some(Object::Dictionary(d)) => d,
            _ => Dictionary::new(),
        };
        let id = self.edit().add_object(Object::Dictionary(target));
        self.dict_mut(owner)?.set(key, Object::Reference(id));
        Ok(id)
    }

    /// The page's /Resources sub-dictionary `sub` (Font, XObject), created as needed. Resources
    /// inherited from the page tree are shared with the other pages, as MuPDF did.
    fn resource_dict_id(&mut self, pno: usize, sub: &str) -> Result<ObjectId> {
        let pid = self.page_id(pno)?;
        let owner = self
            .inheritable(pid, b"Resources")
            .map(|x| x.0)
            .unwrap_or(pid);
        let res = self.dict_entry_id(owner, "Resources")?;
        self.dict_entry_id(res, sub)
    }

    /// Names in the page's resource sub-dictionary `sub` (empty when absent).
    fn resource_entry(&self, pno: usize, sub: &[u8], name: &[u8]) -> Option<Object> {
        let pid = self.page_id(pno).ok()?;
        let (_, res) = self.inheritable(pid, b"Resources")?;
        let res = self.deref_dict(res)?;
        let d = self.deref_dict(res.get(sub).ok()?)?;
        d.get(name).ok().cloned()
    }

    // ── fonts ──

    /// `_get_font_properties` for a font already in the PDF: non-embedded -> ascender .96 / descender
    /// -.24 (0.8 / -0.2 times 1.2); embedded -> the font program's metrics.
    fn existing_font_info(&self, id: ObjectId) -> Result<FontInfo> {
        let d = self.dict(id).ok_or(PdfError("xref is not a font".into()))?;
        let name_of = |o: Option<&Object>| -> Vec<u8> {
            match o.map(|o| self.deref(o)) {
                Some(Object::Name(n)) => n.clone(),
                _ => vec![],
            }
        };
        let name = String::from_utf8_lossy(&name_of(d.get(b"BaseFont").ok())).into_owned();
        let subtype = name_of(d.get(b"Subtype").ok());
        let simple = matches!(subtype.as_slice(), b"Type1" | b"MMType1" | b"TrueType");
        let mut desc = d
            .get(b"FontDescriptor")
            .ok()
            .and_then(|o| self.deref_dict(o));
        if desc.is_none() {
            if let Some(Object::Array(a)) = d.get(b"DescendantFonts").ok().map(|o| self.deref(o)) {
                if let Some(df) = a.first().and_then(|o| self.deref_dict(o)) {
                    desc = df
                        .get(b"FontDescriptor")
                        .ok()
                        .and_then(|o| self.deref_dict(o));
                }
            }
        }
        let mut data = None;
        if let Some(desc) = desc {
            for k in [&b"FontFile"[..], b"FontFile2", b"FontFile3"] {
                if let Some(Object::Stream(s)) = desc.get(k).ok().map(|o| self.deref(o)) {
                    data = Some(stream_data(s)?);
                    break;
                }
            }
        }
        let (ascender, descender) = match &data {
            Some(buf) => match sfnt_extents(buf) {
                Some(e) => e,
                // a Type 1 / bare CFF program: the Base-14 metrics when it is one of those fonts
                None => match base14_metrics(&name) {
                    Some(m) => (m.ascender as f64, m.descender as f64),
                    None => (0.8 * 1.2, -0.2 * 1.2),
                },
            },
            None => (0.8 * 1.2, -0.2 * 1.2),
        };
        Ok(FontInfo {
            name,
            simple,
            ascender,
            descender,
            data,
        })
    }

    /// `page.insert_font(fontname)` for Base-14 names; returns the font xref and records its FontInfo.
    fn insert_font(&mut self, pno: usize, fontname: &str) -> Result<(i32, FontInfo)> {
        // CheckFont: the page already references a font under this name
        if let Some(Object::Reference(fid)) = self.resource_entry(pno, b"Font", fontname.as_bytes())
        {
            let xref = xref_num(fid);
            if let Some((_, fi)) = self.font_infos.iter().find(|(x, _)| *x == xref) {
                return Ok((xref, fi.clone()));
            }
            let fi = self.existing_font_info(fid)?;
            self.font_infos.push((xref, fi.clone()));
            return Ok((xref, fi));
        }
        let Some(bf) = base14_name(fontname) else {
            return Err(PdfError(format!(
                "font '{fontname}' is not supported here (Base-14 only)"
            )));
        };
        let m = metrics_of(bf)?;
        let fid = self.edit().add_object(base14_font_dict(bf));
        let xref = xref_num(fid);
        let fonts = self.resource_dict_id(pno, "Font")?;
        self.dict_mut(fonts)?.set(fontname, Object::Reference(fid));
        let fi = FontInfo {
            name: bf.to_string(),
            simple: true,
            ascender: m.ascender as f64,
            descender: m.descender as f64,
            data: None,
        };
        if !self.font_infos.iter().any(|(x, _)| *x == xref) {
            self.font_infos.push((xref, fi.clone()));
        }
        Ok((xref, fi))
    }

    /// `doc.get_char_widths(xref, limit)`: the advance of the glyph for each code point `0..limit`
    /// (at least 256), 0 where the font has none.
    fn char_widths(&self, fi: &FontInfo, limit: usize) -> Result<Vec<f64>> {
        let n = limit.max(256);
        if let Some(m) = base14_metrics(&fi.name) {
            return Ok((0..n)
                .map(|i| base14_advance(m, i as u32).unwrap_or(0.0) as f64)
                .collect());
        }
        let Some(buf) = &fi.data else {
            return Err(PdfError(format!(
                "font '{}' is not supported (not embedded and not a Base-14 font)",
                fi.name
            )));
        };
        let face = ttf_parser::Face::parse(buf, 0).map_err(|_| {
            PdfError(format!(
                "font '{}' cannot be measured: only embedded TrueType / OpenType programs are supported",
                fi.name
            ))
        })?;
        let upem = face.units_per_em() as f32;
        Ok((0..n)
            .map(|i| {
                char::from_u32(i as u32)
                    .and_then(|c| face.glyph_index(c))
                    .filter(|g| g.0 > 0)
                    .and_then(|g| face.glyph_hor_advance(g))
                    .map(|a| (a as f32 / upem) as f64)
                    .unwrap_or(0.0)
            })
            .collect())
    }

    // ── content streams ──

    /// The page's /Contents as a list of stream objects.
    fn content_streams(&self, pno: usize) -> Result<Vec<&Stream>> {
        let pid = self.page_id(pno)?;
        let page = self
            .dict(pid)
            .ok_or(PdfError("page is not a dictionary".into()))?;
        let mut out = Vec::new();
        match page.get(b"Contents").ok().map(|o| self.deref(o)) {
            Some(Object::Array(a)) => {
                for o in a {
                    if let Object::Stream(s) = self.deref(o) {
                        out.push(s);
                    }
                }
            }
            Some(Object::Stream(s)) => out.push(s),
            _ => {}
        }
        Ok(out)
    }

    /// `TOOLS._insert_contents` + `doc.update_stream(xref, data)` (deflated when that is smaller).
    fn insert_contents(
        &mut self,
        pno: usize,
        data: &[u8],
        overlay: bool,
        compress: bool,
    ) -> Result<()> {
        let pid = self.page_id(pno)?;
        let (buf, filtered) = if compress && data.len() > 30 {
            let z = zlib_best(data);
            if z.len() < data.len() {
                (z, true)
            } else {
                (data.to_vec(), false)
            }
        } else {
            (data.to_vec(), false)
        };
        let mut dict = Dictionary::new();
        if filtered {
            dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        }
        let sid = self.edit().add_object(Stream::new(dict, buf));
        let new = Object::Reference(sid);
        let contents = self
            .dict(pid)
            .and_then(|d| d.get(b"Contents").ok())
            .cloned();
        // an indirect array is edited in place
        if let Some(Object::Reference(r)) = contents {
            if let Ok(Object::Array(a)) = self.edit().get_object_mut(r) {
                if overlay {
                    a.push(new);
                } else {
                    a.insert(0, new);
                }
                return Ok(());
            }
        }
        let arr = match contents {
            Some(Object::Array(mut a)) => {
                if overlay {
                    a.push(new);
                } else {
                    a.insert(0, new);
                }
                a
            }
            Some(other) if !other.is_null() => {
                if overlay {
                    vec![other, new]
                } else {
                    vec![new, other]
                }
            }
            _ => vec![new],
        };
        self.dict_mut(pid)?.set("Contents", Object::Array(arr));
        Ok(())
    }

    /// `page.wrap_contents()`: balance q/Q around the existing content.
    fn wrap_contents(&mut self, pno: usize) -> Result<()> {
        let (push, pop) = self.count_q_balance(pno)?;
        if push > 0 {
            self.insert_contents(pno, "q\n".repeat(push).as_bytes(), false, false)?;
        }
        if pop > 0 {
            let mut s = "\nQ".repeat(pop);
            s.push('\n');
            self.insert_contents(pno, s.as_bytes(), true, false)?;
        }
        Ok(())
    }

    /// `pdf_count_q_balance`: operators `q`/`Q` over the page's content streams.
    fn count_q_balance(&self, pno: usize) -> Result<(usize, usize)> {
        let mut data = Vec::new();
        for s in self.content_streams(pno)? {
            data.extend(stream_data(s)?);
            data.push(b'\n');
        }
        // MuPDF's balance processor (as PyMuPDF 1.24.11 reports it): q/Q nesting, the lowest level reached,
        // the lowest level any other operator ran at; q's still open at the end are closed by the
        // interpreter (end level capped at 0); unguarded operators need one extra q/Q around everything.
        let (mut depth, mut min_q, mut min_op_q) = (0i64, 0i64, 1i64);
        for tok in content_operators(&data) {
            match tok.as_slice() {
                b"q" => depth += 1,
                b"Q" => {
                    depth -= 1;
                    min_q = min_q.min(depth);
                }
                _ => min_op_q = min_op_q.min(depth),
            }
        }
        let end_q = depth.min(0);
        let mut prepend = -min_q;
        let mut append = end_q - min_q;
        if min_op_q == min_q {
            prepend += 1;
            append += 1;
        }
        Ok((prepend.max(0) as usize, append.max(0) as usize))
    }

    /// Shape.commit(overlay=True).
    fn commit(&mut self, pno: usize, totalcont: &str) -> Result<()> {
        if totalcont.is_empty() {
            return Ok(());
        }
        self.wrap_contents(pno)?;
        self.insert_contents(pno, totalcont.as_bytes(), true, true)
    }

    /// `page.insert_text(point, text, fontsize=..., fontname=..., color=...)`; returns the line count.
    pub fn insert_text(
        &mut self,
        pno: usize,
        point: Point,
        text: &str,
        opts: &TextOptions,
    ) -> Result<i32> {
        if text.is_empty() {
            return Ok(0);
        }
        let lines = py_splitlines(text);
        if lines.is_empty() {
            return Ok(0);
        }
        let g = self.geom(pno)?;
        let fname = opts.fontname.trim_start_matches('/').to_string();
        let (_, fi) = self.insert_font(pno, &fname)?;
        let fontsize = opts.fontsize;
        let lheight = if fi.ascender - fi.descender <= 1.0 {
            fontsize * 1.2
        } else {
            fontsize * (fi.ascender - fi.descender)
        };
        let tab: Vec<String> = lines.iter().map(|t| tj_str(t)).collect();
        let color = opts.color.as_deref();
        let color_str = color_code(color, 'c')?;
        let fill_str = color_code(color, 'f')?;
        let top = g.height - point.y - g.y;
        let left = point.x + g.x;
        let mut space = top;
        let mut nres = format!(
            "\nq\nBT\n1 0 0 1 {} Tm\n/{} {} Tf ",
            format_g_seq(&[left, top]),
            fname,
            format_g(fontsize)
        );
        if color.is_some() {
            nres += &color_str;
            nres += &fill_str;
        }
        nres += &tab[0];
        let mut nlines = 1;
        if tab.len() > 1 {
            nres += &format!("TJ\n0 -{} TD\n", format_g(lheight));
        } else {
            nres += "TJ";
        }
        for (i, t) in tab.iter().enumerate().skip(1) {
            if space < lheight {
                break;
            }
            if i > 1 {
                nres += "\nT* ";
            }
            nres += t;
            nres += "TJ";
            space -= lheight;
            nlines += 1;
        }
        nres += "\nET\nQ\n";
        self.commit(pno, &nres)?;
        Ok(nlines)
    }

    /// `page.insert_textbox(rect, text, fontsize=..., color=..., fontname=..., align=...)`: the unused
    /// height (>= 0) or the negative deficit (nothing written then).
    pub fn insert_textbox(
        &mut self,
        pno: usize,
        rect: Rect,
        buffer: &str,
        opts: &TextOptions,
    ) -> Result<f64> {
        if rect.is_empty() {
            return err("text box must be finite and not empty");
        }
        let color = opts.color.as_deref();
        let color_str = color_code(color, 'c')?;
        let fill_str = color_code(color, 'f')?;
        if buffer.is_empty() {
            return Ok(rect.height());
        }
        let g = self.geom(pno)?;
        let fname = opts.fontname.trim_start_matches('/').to_string();
        let (_, fi) = self.insert_font(pno, &fname)?;
        let fontsize = opts.fontsize;
        let (ascender, descender) = (fi.ascender, fi.descender);
        let lheight_factor = if ascender - descender <= 1.0 {
            1.2
        } else {
            ascender - descender
        };
        let lheight = fontsize * lheight_factor;

        let maxcode = buffer.chars().map(|c| c as u32).max().unwrap_or(0) as usize;
        let t0: String = if fi.simple && maxcode > 255 {
            buffer
                .chars()
                .map(|c| if (c as u32) < 256 { c } else { '?' })
                .collect()
        } else {
            buffer.to_string()
        };
        let t0 = py_splitlines(&t0);
        let glyphs = self.char_widths(&fi, maxcode + 1)?;
        let pixlen = |x: &str| -> f64 {
            x.chars()
                .map(|c| glyphs.get(c as usize).copied().unwrap_or(0.0))
                .sum::<f64>()
                * fontsize
        };
        let blen = glyphs[32] * fontsize;

        let c_pnt = Point::new(0.0, fontsize * ascender);
        let point = Point::new(rect.x0 + c_pnt.x, rect.y0 + c_pnt.y);
        let maxwidth = rect.width();
        let maxheight = rect.height();

        let mut just_tab: Vec<bool> = Vec::new();
        let mut text = String::new();
        let nlines_in = t0.len();
        for (i, line) in t0.iter().enumerate() {
            let expanded = line.replace('\t', " ");
            let line_t: Vec<&str> = expanded.split(' ').collect();
            let mut lbuff = String::new();
            let mut rest = maxwidth;
            for word in line_t {
                let pl_w = pixlen(word);
                if rest >= pl_w {
                    lbuff.push_str(word);
                    lbuff.push(' ');
                    rest -= pl_w + blen;
                    continue;
                }
                if !lbuff.is_empty() {
                    lbuff = lbuff.trim_end().to_string() + "\n";
                    text.push_str(&lbuff);
                    just_tab.push(true);
                }
                lbuff = String::new();
                #[allow(unused_assignments)]
                {
                    rest = maxwidth;
                }
                if pl_w <= maxwidth {
                    lbuff = format!("{word} ");
                    rest = maxwidth - pl_w - blen;
                    continue;
                }
                if let Some(last) = just_tab.last_mut() {
                    *last = false;
                }
                for c in word.chars() {
                    let cs = c.to_string();
                    if pixlen(&lbuff) <= maxwidth - pixlen(&cs) {
                        lbuff.push(c);
                    } else {
                        lbuff.push('\n');
                        text.push_str(&lbuff);
                        just_tab.push(false);
                        lbuff = cs;
                    }
                }
                lbuff.push(' ');
                rest = maxwidth - pixlen(&lbuff);
            }
            if !lbuff.is_empty() {
                text.push_str(lbuff.trim_end());
                just_tab.push(false);
            }
            if i + 1 < nlines_in {
                text.push('\n');
            }
        }
        if text.ends_with('\n') {
            text.pop();
        }
        let lb_count = text.matches('\n').count() + 1;
        let text_height = lheight * lb_count as f64 - descender * fontsize;
        let mut more = text_height - maxheight;
        if more > 1e-5 {
            return Ok(-more);
        }
        more = more.abs();
        if more < 1e-5 {
            more = 0.0;
        }
        let mut nres = String::from("\nq\nBT\n");
        let text_t = py_splitlines(&text);
        if let Some(last) = just_tab.last_mut() {
            *last = false;
        }
        for (i, t) in text_t.iter().enumerate() {
            let mut spacing = 0.0;
            let pl = maxwidth - pixlen(t);
            let f = i as f64 * lheight_factor;
            let mut pnt = Point::new(point.x + c_pnt.x * f, point.y + c_pnt.y * f);
            match opts.align {
                1 => pnt = Point::new(pnt.x + pl / 2.0, pnt.y),
                2 => pnt = Point::new(pnt.x + pl, pnt.y),
                3 => {
                    let spaces = t.matches(' ').count();
                    if spaces > 0 && just_tab.get(i).copied().unwrap_or(false) {
                        spacing = pl / spaces as f64;
                    }
                }
                _ => {}
            }
            let top = g.height - pnt.y - g.y;
            let left = pnt.x + g.x;
            nres += &format!(
                "1 0 0 1 {} Tm /{} {} Tf ",
                format_g_seq(&[left, top]),
                fname,
                format_g(fontsize)
            );
            if opts.align == 3 {
                nres += &format!("{} Tw ", format_g(spacing));
            }
            if color.is_some() {
                nres += &color_str;
                nres += &fill_str;
            }
            nres += &format!("{}TJ\n", tj_str(t));
        }
        nres += "ET\nQ\n";
        self.commit(pno, &nres)?;
        Ok(more)
    }

    /// `page.draw_line(p1, p2, color=..., width=...)`.
    pub fn draw_line(
        &mut self,
        pno: usize,
        p1: Point,
        p2: Point,
        color: Option<&[f64]>,
        width: f64,
    ) -> Result<Point> {
        let g = self.geom(pno)?;
        let tr = |p: Point| -> [f64; 2] { transform_f32(p.x, p.y, &g.ipctm) };
        let mut draw = format!("{} m\n", format_g_seq(&jm_tuple(&tr(p1))));
        draw += &format!("{} l\n", format_g_seq(&jm_tuple(&tr(p2))));
        // finish(color=color, width=width, closePath=False)
        let (color, width) = if width == 0.0 {
            (None, width)
        } else if color.is_none() {
            (None, 0.0)
        } else {
            (color, width)
        };
        let color_str = color_code(color, 'c')?;
        if width != 1.0 && width != 0.0 {
            draw += &format!("{} w\n", format_g(width));
        }
        if color.is_some() {
            draw += &color_str;
        }
        draw += "S\n";
        let total = format!("\nq\n{draw}Q\n");
        self.commit(pno, &total)?;
        Ok(p2)
    }

    /// An image XObject for encoded image bytes: baseline JPEG (grey or RGB) is kept as it is
    /// (DCTDecode); anything else is decoded and stored deflated, with an SMask for transparency.
    fn add_image_object(&mut self, stream: &[u8]) -> Result<(ObjectId, i32, i32)> {
        let img = image::load_from_memory(stream)
            .map_err(|e| PdfError(format!("cannot decode the image: {e}")))?;
        let (w, h) = (img.width(), img.height());
        let base = |cs: &str, filter: &str| {
            new_dict(vec![
                ("Type", Object::Name(b"XObject".to_vec())),
                ("Subtype", Object::Name(b"Image".to_vec())),
                ("Width", Object::Integer(w as i64)),
                ("Height", Object::Integer(h as i64)),
                ("ColorSpace", Object::Name(cs.as_bytes().to_vec())),
                ("BitsPerComponent", Object::Integer(8)),
                ("Filter", Object::Name(filter.as_bytes().to_vec())),
            ])
        };
        let is_jpeg = matches!(image::guess_format(stream), Ok(image::ImageFormat::Jpeg));
        if is_jpeg && matches!(img.color(), image::ColorType::L8 | image::ColorType::Rgb8) {
            let cs = if img.color() == image::ColorType::L8 {
                "DeviceGray"
            } else {
                "DeviceRGB"
            };
            let s = Stream::new(base(cs, "DCTDecode"), stream.to_vec()).with_compression(false);
            let id = self.edit().add_object(s);
            return Ok((id, w as i32, h as i32));
        }
        let gray = !img.color().has_color();
        let (pixels, cs) = if gray {
            (img.to_luma8().into_raw(), "DeviceGray")
        } else {
            (img.to_rgb8().into_raw(), "DeviceRGB")
        };
        let mut dict = base(cs, "FlateDecode");
        if img.color().has_alpha() {
            let alpha: Vec<u8> = img.to_rgba8().pixels().map(|p| p.0[3]).collect();
            if alpha.iter().any(|a| *a < 255) {
                let mut md = base("DeviceGray", "FlateDecode");
                md.remove(b"Type");
                md.set("Type", Object::Name(b"XObject".to_vec()));
                let mask = Stream::new(md, zlib_best(&alpha)).with_compression(false);
                let mid = self.edit().add_object(mask);
                dict.set("SMask", Object::Reference(mid));
            }
        }
        let s = Stream::new(dict, zlib_best(&pixels)).with_compression(false);
        let id = self.edit().add_object(s);
        Ok((id, w as i32, h as i32))
    }

    /// `page.insert_image(rect, stream=png_bytes)` (keep_proportion=True, overlay=True).
    pub fn insert_image(&mut self, pno: usize, rect: Rect, stream: &[u8]) -> Result<i32> {
        if rect.is_empty() {
            return err("rect must be finite and not empty");
        }
        let g = self.geom(pno)?;
        // clip = r * ~page.transformation_matrix  (in PDF coordinates)
        let m = g.ipctm;
        let pts = [(rect.x0, rect.y0), (rect.x1, rect.y1)];
        let tp: Vec<(f64, f64)> = pts
            .iter()
            .map(|&(x, y)| {
                let t = transform_f32(x, y, &m);
                (t[0], t[1])
            })
            .collect();
        let clip = Rect::new(
            tp[0].0.min(tp[1].0),
            tp[0].1.min(tp[1].1),
            tp[0].0.max(tp[1].0),
            tp[0].1.max(tp[1].1),
        );
        let imgname = self.free_image_name(pno)?;
        self.wrap_contents(pno)?;
        let (xref, w, h) = match self.inserted_images.get(stream) {
            Some(&x) => {
                let d = self
                    .dict(xref_id(x))
                    .ok_or(PdfError("image object vanished".into()))?;
                let w = d.get(b"Width").ok().and_then(num).unwrap_or(0.0) as i32;
                let h = d.get(b"Height").ok().and_then(num).unwrap_or(0.0) as i32;
                (x, w, h)
            }
            None => {
                let (id, w, h) = self.add_image_object(stream)?;
                let x = xref_num(id);
                self.inserted_images.insert(stream.to_vec(), x);
                (x, w, h)
            }
        };
        let xobjects = self.resource_dict_id(pno, "XObject")?;
        self.dict_mut(xobjects)?
            .set(imgname.as_str(), Object::Reference(xref_id(xref)));
        let mat = calc_image_matrix(w as f32, h as f32, &clip);
        let s = format!(
            "\nq\n{} cm\n/{} Do\nQ\n",
            format_g_seq(&mat.iter().map(|v| *v as f64).collect::<Vec<_>>()),
            imgname
        );
        self.insert_contents(pno, s.as_bytes(), true, false)?;
        Ok(xref)
    }

    /// First free `fzImg<n>` among the page's image / xobject / font resource names.
    fn free_image_name(&self, pno: usize) -> Result<String> {
        let pid = self.page_id(pno)?;
        let mut names = Vec::new();
        fn collect(doc: &Document, res: &Dictionary, names: &mut Vec<String>, depth: usize) {
            if depth > 8 {
                return;
            }
            for key in [&b"XObject"[..], b"Font"] {
                let Some(d) = res.get(key).ok().and_then(|o| doc.deref_dict(o)) else {
                    continue;
                };
                for (k, v) in d.iter() {
                    names.push(String::from_utf8_lossy(k).into_owned());
                    if key == b"XObject" {
                        if let Some(sub) = doc
                            .deref_dict(v)
                            .and_then(|x| x.get(b"Resources").ok())
                            .and_then(|o| doc.deref_dict(o))
                        {
                            collect(doc, sub, names, depth + 1);
                        }
                    }
                }
            }
        }
        if let Some(res) = self
            .inheritable(pid, b"Resources")
            .and_then(|(_, o)| self.deref_dict(o))
        {
            collect(self, res, &mut names, 0);
        }
        let mut i = 0;
        loop {
            let n = format!("fzImg{i}");
            if !names.contains(&n) {
                return Ok(n);
            }
            i += 1;
        }
    }

    /// Deep copy of `obj` from `src` into this document; `map` holds the objects already copied.
    fn graft(
        &mut self,
        src: &lopdf::Document,
        obj: &Object,
        map: &mut HashMap<ObjectId, ObjectId>,
    ) -> Object {
        match obj {
            Object::Reference(id) => {
                if let Some(n) = map.get(id) {
                    return Object::Reference(*n);
                }
                let n = self.edit().new_object_id();
                map.insert(*id, n);
                let o = src.get_object(*id).cloned().unwrap_or(Object::Null);
                let c = self.graft(src, &o, map);
                self.edit().objects.insert(n, c);
                Object::Reference(n)
            }
            Object::Array(a) => Object::Array(a.iter().map(|x| self.graft(src, x, map)).collect()),
            Object::Dictionary(d) => Object::Dictionary(self.graft_dict(src, d, map)),
            Object::Stream(s) => {
                let mut c = s.clone();
                c.dict = self.graft_dict(src, &s.dict, map);
                Object::Stream(c)
            }
            other => other.clone(),
        }
    }

    fn graft_dict(
        &mut self,
        src: &lopdf::Document,
        d: &Dictionary,
        map: &mut HashMap<ObjectId, ObjectId>,
    ) -> Dictionary {
        let mut out = Dictionary::new();
        for (k, v) in d.iter() {
            // links back into the source page tree are not followed
            if k.as_slice() == b"Parent" {
                continue;
            }
            let c = self.graft(src, v, map);
            out.set(k.clone(), c);
        }
        out
    }

    /// `doc.insert_pdf(src, from_page=..., to_page=...)` appended at the end (page_merge: page
    /// attributes and non-link/popup/widget annotations are grafted; link re-creation is not ported).
    pub fn insert_pdf(&mut self, src: &Document, from_page: usize, to_page: usize) -> Result<()> {
        let src_count = src.page_count();
        if src_count == 0 {
            return Ok(());
        }
        let fp = from_page.min(src_count - 1);
        let tp = to_page.min(src_count - 1);
        let pages: Vec<usize> = if fp <= tp {
            (fp..=tp).collect()
        } else {
            (tp..=fp).rev().collect()
        };
        let root = self.pages_root()?;
        let mut map: HashMap<ObjectId, ObjectId> = HashMap::new();
        for p in pages {
            let src_pid = src.page_id(p)?;
            let mut page_dict = new_dict(vec![
                ("Type", Object::Name(b"Page".to_vec())),
                ("Parent", Object::Reference(root)),
            ]);
            for key in [
                "Contents",
                "Resources",
                "MediaBox",
                "CropBox",
                "BleedBox",
                "TrimBox",
                "ArtBox",
                "Rotate",
                "UserUnit",
            ] {
                if let Some((_, obj)) = src.inheritable(src_pid, key.as_bytes()) {
                    let obj = obj.clone();
                    let copied = self.graft(&src.doc, &obj, &mut map);
                    page_dict.set(key, copied);
                }
            }
            let annots = src
                .dict(src_pid)
                .and_then(|d| d.get(b"Annots").ok())
                .map(|o| src.deref(o).clone());
            if let Some(Object::Array(old)) = annots {
                let mut new_annots = Vec::new();
                for o in &old {
                    let Some(ad) = src.deref_dict(o) else {
                        continue;
                    };
                    if ad.has(b"IRT") {
                        continue;
                    }
                    let st = match ad.get(b"Subtype").ok().map(|s| src.deref(s)) {
                        Some(Object::Name(n)) => n.clone(),
                        _ => vec![],
                    };
                    if st == b"Link" || st == b"Popup" || st == b"Widget" {
                        continue;
                    }
                    let mut ad = ad.clone();
                    ad.remove(b"Popup");
                    ad.remove(b"P");
                    let c = self.graft(&src.doc, &Object::Dictionary(ad), &mut map);
                    let id = self.edit().add_object(c);
                    new_annots.push(Object::Reference(id));
                }
                if !old.is_empty() {
                    page_dict.set("Annots", Object::Array(new_annots));
                }
            }
            let pid = self.edit().add_object(page_dict);
            self.append_to_page_tree(root, pid)?;
        }
        Ok(())
    }

    /// `doc.tobytes(garbage=..., deflate=..., clean=...)`.
    pub fn tobytes(&self, opts: SaveOptions) -> Result<Vec<u8>> {
        if self.encrypted {
            return err("document closed or encrypted");
        }
        if self.page_count() < 1 {
            return err("cannot save with zero pages");
        }
        let mut doc = self.doc.clone();
        // a missing file identifier gets one (two copies of 16 random bytes)
        if doc.trailer.get(b"ID").is_err() {
            let rnd: [u8; 16] = rand::random();
            let id = Object::String(rnd.to_vec(), StringFormat::Hexadecimal);
            doc.trailer.set("ID", Object::Array(vec![id.clone(), id]));
        }
        if opts.garbage >= 1 {
            doc.prune_objects();
        }
        if opts.garbage >= 3 {
            doc.renumber_objects();
        }
        if opts.deflate {
            doc.compress();
        }
        let mut out = Vec::new();
        doc.save_to(&mut out)?;
        Ok(out)
    }

    /// Decoded content streams of a page, in /Contents order (`[doc.xref_stream(x) for x in page.get_contents()]`).
    pub fn page_contents(&self, pno: usize) -> Result<Vec<Vec<u8>>> {
        self.content_streams(pno)?
            .into_iter()
            .map(stream_data)
            .collect()
    }
}

/// A stream's decoded bytes.
fn stream_data(s: &Stream) -> Result<Vec<u8>> {
    let filtered = s.filters().map(|f| !f.is_empty()).unwrap_or(false);
    if filtered {
        Ok(s.decompressed_content()?)
    } else {
        Ok(s.content.clone())
    }
}

/// `fz_transform_point` (PyMuPDF's point * matrix goes through MuPDF floats).
fn transform_f32(x: f64, y: f64, m: &[f64; 6]) -> [f64; 2] {
    let (x, y) = (x as f32, y as f32);
    let m: Vec<f32> = m.iter().map(|v| *v as f32).collect();
    [
        (x * m[0] + y * m[2] + m[4]) as f64,
        (x * m[1] + y * m[3] + m[5]) as f64,
    ]
}

fn invert(m: [f64; 6]) -> [f64; 6] {
    let det = m[0] * m[3] - m[1] * m[2];
    if det == 0.0 {
        return [0.0; 6];
    }
    let a = m[3] / det;
    let b = -m[1] / det;
    let c = -m[2] / det;
    let d = m[0] / det;
    let e = -(m[4] * a + m[5] * c);
    let f = -(m[4] * b + m[5] * d);
    [a, b, c, d, e, f]
}

/// `calc_image_matrix(width, height, clip, rotate=0, keep=True)` in MuPDF float arithmetic.
fn calc_image_matrix(width: f32, height: f32, tr: &Rect) -> [f32; 6] {
    let (x0, y0, x1, y1) = (tr.x0 as f32, tr.y0 as f32, tr.x1 as f32, tr.y1 as f32);
    let trw = x1 - x0;
    let trh = y1 - y0;
    let large = width.max(height);
    // Python divides ints/floats in double; the results are stored back into floats by fz_make_*
    let fw = width as f64 / large as f64;
    let fh = height as f64 / large as f64;
    let small = fw.min(fh);
    let (trw64, trh64) = (trw as f64, trh as f64);
    let (w, h) = if fw < 1.0 {
        if trw64 / fw > trh64 / fh {
            (trh64 * small, trh64)
        } else {
            (trw64, trw64 / small)
        }
    } else if fw != fh {
        if trw64 / fw > trh64 / fh {
            (trh64 / small, trh64)
        } else {
            (trw64, trw64 * small)
        }
    } else {
        (trw64, trh64)
    };
    let (cx, cy) = (
        ((x0 + x1) as f64 / 2.0) as f32,
        ((y0 + y1) as f64 / 2.0) as f32,
    );
    // [1 0 0 1 -.5 -.5] * scale(w, h) * translate(cx, cy)  (rotate 0 = identity)
    let (w, h) = (w as f32, h as f32);
    [w, 0.0, 0.0, h, -0.5 * w + cx, -0.5 * h + cy]
}

/// zlib at the best level.
fn zlib_best(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// Operator tokens of a content stream (strings, hex strings, names, arrays, dicts and inline image
/// data skipped), enough for q/Q counting.
fn content_operators(data: &[u8]) -> Vec<Vec<u8>> {
    let is_ws = |b: u8| matches!(b, b' ' | b'\n' | b'\r' | b'\t' | b'\x0c' | b'\0');
    let is_delim = |b: u8| {
        matches!(
            b,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if is_ws(b) {
            i += 1;
        } else if b == b'%' {
            while i < data.len() && data[i] != b'\n' && data[i] != b'\r' {
                i += 1;
            }
        } else if b == b'(' {
            let mut depth = 0;
            while i < data.len() {
                match data[i] {
                    b'\\' => i += 1,
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        } else if b == b'<'
            || b == b'>'
            || b == b'['
            || b == b']'
            || b == b'{'
            || b == b'}'
            || b == b')'
        {
            i += 1;
            if b == b'<' && data.get(i) != Some(&b'<') {
                while i < data.len() && data[i] != b'>' {
                    i += 1;
                }
                i += 1;
            }
        } else if b == b'/' {
            i += 1;
            while i < data.len() && !is_ws(data[i]) && !is_delim(data[i]) {
                i += 1;
            }
        } else {
            let start = i;
            while i < data.len() && !is_ws(data[i]) && !is_delim(data[i]) {
                i += 1;
            }
            let tok = data[start..i].to_vec();
            if tok == b"ID" {
                // inline image data runs to "EI" preceded by whitespace
                i += 1;
                while i + 2 < data.len()
                    && !(is_ws(data[i]) && data[i + 1] == b'E' && data[i + 2] == b'I')
                {
                    i += 1;
                }
                i += 3;
                continue;
            }
            if !tok.is_empty()
                && !tok[0].is_ascii_digit()
                && tok[0] != b'-'
                && tok[0] != b'+'
                && tok[0] != b'.'
            {
                out.push(tok);
            }
        }
    }
    out
}

// ─── structured text (lines of positioned characters, like MuPDF's stext) ────

/// One character: its Unicode value, quad (ul, ur, ll, lr in page coordinates) and font size.
#[derive(Clone, Debug)]
struct TextChar {
    c: char,
    quad: [f64; 8],
    size: f64,
}

#[derive(Clone, Debug)]
struct TextLine {
    /// writing direction (unit vector, page coordinates)
    dir: (f64, f64),
    chars: Vec<TextChar>,
    /// where the next character of this line would start
    pen: (f64, f64),
}

/// Collects the characters (grouped into lines) and the image boxes the interpreter draws.
#[derive(Default)]
struct TextDevice {
    lines: Vec<TextLine>,
    images: Vec<Rect>,
    /// (ascender, descender) per font program
    extents: HashMap<u128, (f64, f64)>,
}

fn apply(m: &Affine, x: f64, y: f64) -> (f64, f64) {
    let p = *m * KPoint::new(x, y);
    (p.x, p.y)
}

/// Gap (in font sizes) beyond which a space is assumed between two characters of a line.
const SPACE_DIST: f64 = 0.15;
/// Gap (in font sizes) beyond which the next character starts a new line (table columns).
const SPACE_MAX_DIST: f64 = 0.8;
/// Baseline offset (in font sizes) beyond which a character starts a new line.
const BASE_MAX_DIST: f64 = 0.8;

impl TextDevice {
    /// Ascender / descender of the glyph's font (em): the embedded OpenType / TrueType metrics, else
    /// Helvetica's (the standard fonts and Type 1 programs).
    fn extents(&mut self, glyph: &Glyph<'_>) -> (f64, f64) {
        let helv = base14_metrics("Helvetica")
            .map_or((1.075, -0.299), |m| (m.ascender as f64, m.descender as f64));
        let Glyph::Outline(o) = glyph else {
            return helv;
        };
        let Some(fd) = o.font_data() else {
            return helv;
        };
        if let Some(e) = self.extents.get(&fd.cache_key) {
            return *e;
        }
        let bytes: &[u8] = (*fd.data).as_ref();
        let e = sfnt_extents(bytes)
            .or_else(|| {
                fd.postscript_name
                    .as_deref()
                    .and_then(base14_metrics)
                    .map(|m| (m.ascender as f64, m.descender as f64))
            })
            .unwrap_or(helv);
        self.extents.insert(fd.cache_key, e);
        e
    }

    fn add_char(
        &mut self,
        c: char,
        quad: [f64; 8],
        origin: (f64, f64),
        adv: (f64, f64),
        size: f64,
    ) {
        let len = (adv.0 * adv.0 + adv.1 * adv.1).sqrt();
        let dir = if len > 1e-9 {
            (adv.0 / len, adv.1 / len)
        } else {
            self.lines.last().map_or((1.0, 0.0), |l| l.dir)
        };
        let pen = (origin.0 + adv.0, origin.1 + adv.1);
        if let Some(line) = self.lines.last_mut() {
            let same_dir = (dir.0 * line.dir.0 + dir.1 * line.dir.1) > 0.999;
            let d = (origin.0 - line.pen.0, origin.1 - line.pen.1);
            let along = d.0 * line.dir.0 + d.1 * line.dir.1;
            let perp = d.0 * line.dir.1 - d.1 * line.dir.0;
            if same_dir
                && perp.abs() < BASE_MAX_DIST * size
                && along > -SPACE_DIST * size
                && along < SPACE_MAX_DIST * size
            {
                let last = line.chars.last().map(|x| x.c);
                if along > SPACE_DIST * size && last != Some(' ') && c != ' ' {
                    // a gap the PDF does not fill with a space character
                    let (p0, p1) = (line.pen, origin);
                    let (u, l) = (
                        (quad[0] - quad[4], quad[1] - quad[5]),
                        (quad[4] - origin.0, quad[5] - origin.1),
                    );
                    let q = [
                        p0.0 + l.0 + u.0,
                        p0.1 + l.1 + u.1,
                        p1.0 + l.0 + u.0,
                        p1.1 + l.1 + u.1,
                        p0.0 + l.0,
                        p0.1 + l.1,
                        p1.0 + l.0,
                        p1.1 + l.1,
                    ];
                    line.chars.push(TextChar {
                        c: ' ',
                        quad: q,
                        size,
                    });
                }
                line.chars.push(TextChar { c, quad, size });
                line.pen = pen;
                return;
            }
        }
        self.lines.push(TextLine {
            dir,
            chars: vec![TextChar { c, quad, size }],
            pen,
        });
    }
}

impl<'a> Device<'a> for TextDevice {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph_run(
        &mut self,
        run: &hayro::hayro_interpret::font::GlyphRun<'_, 'a>,
        props: DrawProps<'a>,
        _: &DrawMode,
    ) {
        for g in run.glyphs() {
            let m = props.transform * g.transform();
            let advance = match &**g {
                Glyph::Outline(o) => o.advance_width(),
                Glyph::Type3(t) => t.advance_width(),
            }
            .unwrap_or(0.0) as f64;
            let origin = apply(&m, 0.0, 0.0);
            let up = apply(&m, 0.0, 1000.0);
            let size = ((up.0 - origin.0).powi(2) + (up.1 - origin.1).powi(2)).sqrt();
            let (mut asc, mut dsc) = self.extents(g);
            // JM_char_quad: fonts whose ascender - descender is under 1 em get a 1 em box
            if asc - dsc < 1.0 {
                if asc < 1e-3 {
                    dsc = -0.1;
                    asc = 0.9;
                } else {
                    let h = asc - dsc;
                    asc /= h;
                    dsc /= h;
                }
            }
            let text: Vec<char> = match g.as_unicode() {
                Some(hayro::hayro_interpret::hayro_cmap::BfString::Char(c)) => vec![c],
                Some(hayro::hayro_interpret::hayro_cmap::BfString::String(s)) => {
                    s.chars().collect()
                }
                None => vec![char::from_u32(g.char_code()).unwrap_or('\u{FFFD}')],
            };
            let n = text.len().max(1) as f64;
            for (k, c) in text.into_iter().enumerate() {
                let (a0, a1) = (advance * k as f64 / n, advance * (k + 1) as f64 / n);
                let ul = apply(&m, a0, asc * 1000.0);
                let ur = apply(&m, a1, asc * 1000.0);
                let ll = apply(&m, a0, dsc * 1000.0);
                let lr = apply(&m, a1, dsc * 1000.0);
                let o = apply(&m, a0, 0.0);
                let e = apply(&m, a1, 0.0);
                self.add_char(
                    c,
                    [ul.0, ul.1, ur.0, ur.1, ll.0, ll.1, lr.0, lr.1],
                    o,
                    (e.0 - o.0, e.1 - o.1),
                    size,
                );
            }
        }
    }
    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        if let Image::Raster(_) = image {
            let (w, h) = (image.width() as f64, image.height() as f64);
            let pts = [
                apply(&props.transform, 0.0, 0.0),
                apply(&props.transform, w, 0.0),
                apply(&props.transform, 0.0, h),
                apply(&props.transform, w, h),
            ];
            let xs = pts.map(|p| p.0);
            let ys = pts.map(|p| p.1);
            self.images.push(Rect::new(
                xs.iter().cloned().fold(f64::INFINITY, f64::min),
                ys.iter().cloned().fold(f64::INFINITY, f64::min),
                xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            ));
        }
    }
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

fn quad_bbox(q: &[f64; 8]) -> Rect {
    let xs = [q[0], q[2], q[4], q[6]];
    let ys = [q[1], q[3], q[5], q[7]];
    Rect::new(
        xs.iter().cloned().fold(f64::INFINITY, f64::min),
        ys.iter().cloned().fold(f64::INFINITY, f64::min),
        xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    )
}

fn rects_overlap(a: &Rect, b: &Rect) -> bool {
    !(a.x0 >= b.x1 || a.y0 >= b.y1 || a.x1 <= b.x0 || a.y1 <= b.y0)
}

/// `canon(c)`.
fn canon(c: u32) -> u32 {
    if c == 0xA0
        || c == 0x2028
        || c == 0x2029
        || c == '\r' as u32
        || c == '\n' as u32
        || c == '\t' as u32
    {
        return ' ' as u32;
    }
    if ('A' as u32..='Z' as u32).contains(&c) {
        return c - 'A' as u32 + 'a' as u32;
    }
    c
}

/// `chartocanon(s[pos:])`: (1, canonical code); empty input -> (1, 0).
fn chartocanon(s: &[char], pos: usize) -> (usize, u32) {
    match s.get(pos) {
        None => (1, 0),
        Some(c) => (1, canon(*c as u32)),
    }
}

/// `match_string(h0, n0)` on `hay[start..]`: end index (relative) of the match, if the needle matches here.
fn match_string(hay: &[char], start: usize, needle: &[char]) -> Option<usize> {
    let mut h = 0usize;
    let mut n = 0usize;
    let mut e = h;
    let (dh, mut hc) = chartocanon(hay, start + h);
    h += dh;
    let (dn, mut nc) = chartocanon(needle, n);
    n += dn;
    while hc == nc {
        e = h;
        if hc == ' ' as u32 {
            loop {
                let (dh, c) = chartocanon(hay, start + h);
                h += dh;
                hc = c;
                if hc != ' ' as u32 {
                    break;
                }
            }
        } else {
            let (dh, c) = chartocanon(hay, start + h);
            h += dh;
            hc = c;
        }
        if nc == ' ' as u32 {
            loop {
                let (dn, c) = chartocanon(needle, n);
                n += dn;
                nc = c;
                if nc != ' ' as u32 {
                    break;
                }
            }
        } else {
            let (dn, c) = chartocanon(needle, n);
            n += dn;
            nc = c;
        }
        if hc == 0 && nc == 0 {
            break;
        }
    }
    if nc != 0 {
        None
    } else {
        Some(e)
    }
}

/// `find_string(s[from:], needle)` with absolute indexes.
fn find_string(hay: &[char], from: usize, needle: &[char]) -> Option<(usize, usize)> {
    for i in from..hay.len() {
        if let Some(end) = match_string(hay, i, needle) {
            return Some((i, end + i));
        }
    }
    None
}

/// `on_highlight_char`: extend the last quad when the char continues it, else start a new one.
fn on_highlight_char(quads: &mut Vec<[f64; 8]>, line: &TextLine, ch: &TextChar) {
    let vfuzz = ch.size * 0.1;
    let hfuzz = ch.size * 0.2;
    let q = ch.quad;
    let (dx, dy) = line.dir;
    let hdist = |a: (f64, f64), b: (f64, f64)| ((b.0 - a.0) * dx + (b.1 - a.1) * dy).abs();
    let vdist = |a: (f64, f64), b: (f64, f64)| ((b.0 - a.0) * dy + (b.1 - a.1) * dx).abs();
    if let Some(end) = quads.last_mut() {
        let (e_lr, e_ur) = ((end[6], end[7]), (end[2], end[3]));
        let (c_ll, c_ul) = ((q[4], q[5]), (q[0], q[1]));
        if hdist(e_lr, c_ll) < hfuzz
            && vdist(e_lr, c_ll) < vfuzz
            && hdist(e_ur, c_ul) < hfuzz
            && vdist(e_ur, c_ul) < vfuzz
        {
            end[2] = q[2];
            end[3] = q[3];
            end[6] = q[6];
            end[7] = q[7];
            return;
        }
    }
    quads.push(q);
}

// doclib A2: document library stamping (a footer line on every page, an invisible fingerprint line and
// Info dictionary entries). Not a PyMuPDF port: written for the library's watermark.

/// Text a Base-14 simple font (Latin encoding) can show: Latin-1 printable characters, anything else `?`.
pub fn latin1_safe(text: &str) -> String {
    text.chars()
        .map(|c| match c as u32 {
            0x20..=0x7E | 0xA0..=0xFF => c,
            0x09 | 0x0A | 0x0D => ' ',
            _ => '?',
        })
        .collect()
}

impl Document {
    /// Adds Helvetica (WinAnsi encoding) to the document once; the returned xref goes to `stamp_footer`.
    pub fn add_stamp_font(&mut self) -> Result<i32> {
        let id = self.edit().add_object(base14_font_dict("Helvetica"));
        Ok(xref_num(id))
    }

    /// Width of `text` (already `latin1_safe`) in Helvetica at `fontsize`.
    pub fn helvetica_width(text: &str, fontsize: f64) -> Result<f64> {
        let m = metrics_of("Helvetica")?;
        let mut w = 0f64;
        for c in text.chars() {
            if let Some(a) = base14_advance(m, c as u32) {
                w += a as f64;
            }
        }
        Ok(w * fontsize)
    }

    /// The page's visible box (the CropBox, else the MediaBox) and its /Rotate (0, 90, 180, 270).
    fn visible_box(&self, pno: usize) -> Result<([f64; 4], i32)> {
        let pid = self.page_id(pno)?;
        let mut mb = self
            .rect_of(self.inheritable(pid, b"MediaBox").map(|x| x.1))
            .unwrap_or([0.0, 0.0, 612.0, 792.0]);
        if mb[2] - mb[0] <= 0.0 || mb[3] - mb[1] <= 0.0 {
            mb = [0.0, 0.0, 612.0, 792.0];
        }
        let cb = match self.rect_of(self.inheritable(pid, b"CropBox").map(|x| x.1)) {
            Some(c) if c[2] > c[0] && c[3] > c[1] => c,
            _ => mb,
        };
        let rotate = self.rotate_of(pid).rem_euclid(360) as i32;
        let rotate = (rotate / 90) * 90;
        Ok((cb, rotate))
    }

    /// Writes `footer` centered at the bottom of the page as the reader sees it (the page rotation is
    /// honoured), in Helvetica `fontsize` with fill gray `gray` (0 black .. 1 white), and `hidden` (when
    /// given) as invisible text (render mode 3) at the bottom left, which text extraction still finds.
    /// `font_xref` comes from `add_stamp_font`.
    pub fn stamp_footer(
        &mut self,
        pno: usize,
        font_xref: i32,
        footer: &str,
        fontsize: f64,
        gray: f64,
        hidden: Option<&str>,
    ) -> Result<()> {
        let ([x0, y0, x1, y1], rotate) = self.visible_box(pno)?;
        let (w, h) = (x1 - x0, y1 - y0);
        // Display space (u right, v up) -> PDF user space, per rotation.
        let (a, b, c, d, e, f) = match rotate {
            90 => (0.0, 1.0, -1.0, 0.0, x0 + w, y0),
            180 => (-1.0, 0.0, 0.0, -1.0, x0 + w, y0 + h),
            270 => (0.0, -1.0, 1.0, 0.0, x0, y0 + h),
            _ => (1.0, 0.0, 0.0, 1.0, x0, y0),
        };
        let disp_w = if rotate == 90 || rotate == 270 { h } else { w };
        let disp_h = if rotate == 90 || rotate == 270 { w } else { h };
        // Shrink the font when the line would not fit the page width (minus a small margin).
        let text = latin1_safe(footer);
        let mut size = fontsize.max(1.0);
        let mut tw = Self::helvetica_width(&text, size)?;
        let room = (disp_w - 24.0).max(10.0);
        if tw > room {
            size = (size * room / tw).max(1.0);
            tw = Self::helvetica_width(&text, size)?;
        }
        let margin = (disp_h * 0.012).clamp(6.0, 14.0);
        let tx = ((disp_w - tw) / 2.0).max(2.0);
        let hex = |s: &str| -> String { s.chars().map(|c| format!("{:02x}", c as u32)).collect() };
        let fname = "GranaryStampF";
        let mut cont = format!(
            "\nq\n{} cm\nBT\n/{fname} {} Tf\n{} g\n1 0 0 1 {} Tm\n0 Tr\n[<{}>] TJ\nET\n",
            format_g_seq(&[a, b, c, d, e, f]),
            format_g(size),
            format_g(gray.clamp(0.0, 1.0)),
            format_g_seq(&[tx, margin]),
            hex(&text)
        );
        if let Some(hidden) = hidden {
            let ht = latin1_safe(hidden);
            cont += &format!(
                "BT\n/{fname} 4 Tf\n3 Tr\n1 0 0 1 {} Tm\n[<{}>] TJ\nET\n",
                format_g_seq(&[4.0, (margin - 5.0).max(1.0)]),
                hex(&ht)
            );
        }
        cont += "Q\n";
        // Register the font under our own resource name. Resources shared by several pages (inherited or
        // one indirect dictionary) get the same name -> same font, which is harmless.
        let fonts = self.resource_dict_id(pno, "Font")?;
        self.dict_mut(fonts)?
            .set(fname, Object::Reference(xref_id(font_xref)));
        self.commit(pno, &cont)
    }

    /// The id of the Info dictionary, created (indirect, in the trailer) when missing.
    fn info_id(&mut self, create: bool) -> Result<Option<ObjectId>> {
        match self.doc.trailer.get(b"Info").ok().cloned() {
            Some(Object::Reference(r)) if self.dict(r).is_some() => return Ok(Some(r)),
            Some(Object::Dictionary(d)) => {
                let id = self.edit().add_object(Object::Dictionary(d));
                self.edit().trailer.set("Info", Object::Reference(id));
                return Ok(Some(id));
            }
            _ => {}
        }
        if !create {
            return Ok(None);
        }
        let id = self.edit().add_object(Dictionary::new());
        self.edit().trailer.set("Info", Object::Reference(id));
        Ok(Some(id))
    }

    /// Sets a text entry of the Info dictionary.
    pub fn set_info(&mut self, key: &str, value: &str) -> Result<()> {
        let Some(id) = self.info_id(true)? else {
            return err("cannot create the Info dictionary");
        };
        self.dict_mut(id)?.set(key, text_string(value));
        Ok(())
    }

    /// A text entry of the Info dictionary.
    pub fn get_info(&mut self, key: &str) -> Result<Option<String>> {
        let info = match self.doc.trailer.get(b"Info").ok() {
            Some(o) => self.deref_dict(o),
            None => None,
        };
        let Some(info) = info else {
            return Ok(None);
        };
        Ok(match info.get(key.as_bytes()).ok().map(|v| self.deref(v)) {
            Some(Object::String(b, _)) => Some(decode_text_string(b)),
            Some(Object::Name(n)) => Some(String::from_utf8_lossy(n).into_owned()),
            Some(_) => Some(String::new()),
            None => None,
        })
    }

    /// Removes the Info dictionary entirely (tests: a copy whose metadata was stripped).
    pub fn clear_info(&mut self) -> Result<()> {
        self.edit().trailer.remove(b"Info");
        Ok(())
    }
}
