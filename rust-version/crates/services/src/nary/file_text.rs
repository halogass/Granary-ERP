//! Nary batch 4 (D1, Rust only): the text of customer files, read locally (no AI cost).
//!
//! - [`kind_of`]: pdf | image | docx | xlsx | text | other, from the name and the mime type.
//! - DOCX ([`docx_units`]): word/document.xml as Markdown: headings (`#` by the style's heading level, the
//!   style names from word/styles.xml so a localised Word works too), list items, paragraphs, tables as
//!   Markdown tables.
//! - XLSX ([`xlsx_units`]): every sheet (workbook order) as a Markdown table, shared and inline strings,
//!   booleans, dates (from the cell's number format), at most `max_rows` rows and [`MAX_COLS`] columns.
//! - Text files ([`text_units`]): UTF-8 (a BOM dropped, bad bytes replaced).
//! - The zip reader under DOCX/XLSX caps what one entry may inflate to ([`MAX_ENTRY_BYTES`]): a customer
//!   file is untrusted, and a small zip can hold gigabytes.
//! - [`page`]: the extracted units (pages, sheets) cut into reads of `max_chars`, with where to go on.
//!
//! The XML is read with quick-xml (no network, no external program).

use std::collections::HashMap;
use std::io::Read;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde_json::{json, Value};

/// The most one zip entry may inflate to (a DOCX body or an XLSX sheet is far smaller).
pub const MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;
/// Columns kept per sheet.
pub const MAX_COLS: usize = 40;

/// One page, sheet or part of a file's text.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// "Page 3", "Sheet: Prices", "Document", "Image"
    pub label: String,
    /// text | vision | table | skipped | note
    pub mode: String,
    pub text: String,
}

impl Unit {
    pub fn new(label: impl Into<String>, mode: &str, text: impl Into<String>) -> Unit {
        Unit {
            label: label.into(),
            mode: mode.to_string(),
            text: text.into(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"label": self.label, "mode": self.mode, "text": self.text})
    }

    pub fn from_json(v: &Value) -> Unit {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Unit {
            label: s("label"),
            mode: s("mode"),
            text: s("text"),
        }
    }
}

/// The units as stored (JSON array).
pub fn units_json(units: &[Unit]) -> Value {
    Value::Array(units.iter().map(Unit::to_json).collect())
}

/// The stored units back.
pub fn units_from(v: Option<&Value>) -> Vec<Unit> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(Unit::from_json).collect())
        .unwrap_or_default()
}

/// The whole text with its page or sheet markers (stored next to the units, for search and the CRM).
pub fn joined(units: &[Unit]) -> String {
    units
        .iter()
        .map(|u| format!("=== {} ===\n{}", u.label, u.text))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn ext_of(filename: &str) -> String {
    let lower = filename.to_lowercase();
    match lower.rfind('.') {
        Some(i) if i + 1 < lower.len() => lower[i + 1..].to_string(),
        _ => String::new(),
    }
}

fn base_mime(mime: Option<&str>) -> String {
    mime.unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

pub const IMAGE_EXTS: [&str; 10] = [
    "jpg", "jpeg", "png", "webp", "gif", "heic", "heif", "bmp", "tif", "tiff",
];
pub const TEXT_EXTS: [&str; 9] = [
    "txt", "md", "csv", "tsv", "json", "srt", "vtt", "log", "xml",
];
pub const DOCX_MIME: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
pub const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// pdf | image | docx | xlsx | text | other: the extension first, then the mime type.
pub fn kind_of(filename: &str, mime: Option<&str>) -> &'static str {
    let ext = ext_of(filename);
    match ext.as_str() {
        "pdf" => return "pdf",
        "docx" => return "docx",
        "xlsx" | "xlsm" => return "xlsx",
        e if IMAGE_EXTS.contains(&e) => return "image",
        e if TEXT_EXTS.contains(&e) => return "text",
        _ => {}
    }
    let m = base_mime(mime);
    match m.as_str() {
        "application/pdf" => "pdf",
        DOCX_MIME => "docx",
        XLSX_MIME | "application/vnd.ms-excel.sheet.macroenabled.12" => "xlsx",
        "application/json" | "application/xml" | "application/x-subrip" => "text",
        m if m.starts_with("image/") => "image",
        m if m.starts_with("text/") => "text",
        _ => "other",
    }
}

// ─── zip ──────────────────────────────────────────────────────────────────────

fn u16le(data: &[u8], at: usize) -> Result<usize, String> {
    data.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
        .ok_or_else(|| "the file is not a valid zip (DOCX/XLSX) file".to_string())
}

fn u32le(data: &[u8], at: usize) -> Result<usize, String> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .ok_or_else(|| "the file is not a valid zip (DOCX/XLSX) file".to_string())
}

/// (name, method, compressed size, local header offset) of every entry of the central directory.
fn zip_entries(data: &[u8]) -> Result<Vec<(String, usize, usize, usize)>, String> {
    let bad = || "the file is not a valid zip (DOCX/XLSX) file".to_string();
    if data.len() < 22 {
        return Err(bad());
    }
    let lo = data.len().saturating_sub(22 + 65535);
    let eocd = (lo..=data.len() - 22)
        .rev()
        .find(|&i| data[i..i + 4] == *b"PK\x05\x06")
        .ok_or_else(bad)?;
    let count = u16le(data, eocd + 10)?;
    let mut at = u32le(data, eocd + 16)?;
    let mut out = Vec::new();
    for _ in 0..count {
        if data.get(at..at + 4) != Some(b"PK\x01\x02") {
            return Err(bad());
        }
        let method = u16le(data, at + 10)?;
        let csize = u32le(data, at + 20)?;
        let nlen = u16le(data, at + 28)?;
        let elen = u16le(data, at + 30)?;
        let clen = u16le(data, at + 32)?;
        let local = u32le(data, at + 42)?;
        let name = data.get(at + 46..at + 46 + nlen).ok_or_else(bad)?;
        out.push((
            String::from_utf8_lossy(name).into_owned(),
            method,
            csize,
            local,
        ));
        at += 46 + nlen + elen + clen;
    }
    Ok(out)
}

/// The entry names of a zip's central directory (29 Oct: upload_file tells a DOCX from an XLSX by them).
pub fn zip_names(data: &[u8]) -> Result<Vec<String>, String> {
    Ok(zip_entries(data)?.into_iter().map(|e| e.0).collect())
}

/// One entry of a zip (stored or deflated), at most `cap` bytes once inflated. None when it is not there.
pub fn zip_entry(data: &[u8], name: &str, cap: usize) -> Result<Option<Vec<u8>>, String> {
    let bad = || "the file is not a valid zip (DOCX/XLSX) file".to_string();
    for (n, method, csize, local) in zip_entries(data)? {
        if n != name {
            continue;
        }
        if data.get(local..local + 4) != Some(b"PK\x03\x04") {
            return Err(bad());
        }
        let start = local + 30 + u16le(data, local + 26)? + u16le(data, local + 28)?;
        let raw = data.get(start..start + csize).ok_or_else(bad)?;
        return match method {
            0 => {
                if raw.len() > cap {
                    return Err("a part of the file is too large to read".into());
                }
                Ok(Some(raw.to_vec()))
            }
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(raw)
                    .take(cap as u64 + 1)
                    .read_to_end(&mut out)
                    .map_err(|_| bad())?;
                if out.len() > cap {
                    return Err("a part of the file is too large to read".into());
                }
                Ok(Some(out))
            }
            _ => Err("the file uses a zip compression that can't be read".into()),
        };
    }
    Ok(None)
}

// ─── XML helpers ──────────────────────────────────────────────────────────────

fn local(e: &BytesStart) -> Vec<u8> {
    e.local_name().as_ref().to_vec()
}

/// The value of the attribute with this local name (any prefix).
fn attr(e: &BytesStart, name: &[u8]) -> Option<String> {
    for a in e.attributes().with_checks(false).flatten() {
        if a.key.local_name().as_ref() == name {
            return Some(match a.unescape_value() {
                Ok(v) => v.into_owned(),
                Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
            });
        }
    }
    None
}

fn xml_text(t: &quick_xml::events::BytesText) -> String {
    match t.unescape() {
        Ok(v) => v.into_owned(),
        Err(_) => String::from_utf8_lossy(t).into_owned(),
    }
}

// ─── Markdown ─────────────────────────────────────────────────────────────────

fn cell_md(s: &str) -> String {
    s.trim()
        .replace('|', "\\|")
        .replace("\r\n", "<br>")
        .replace(['\n', '\r'], "<br>")
}

/// Rows as a Markdown table: the first row is the header; short rows are padded.
pub fn md_table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    let line = |r: &[String]| {
        let mut cells: Vec<String> = r.iter().map(|c| cell_md(c)).collect();
        cells.resize(cols, String::new());
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![line(&rows[0])];
    out.push(format!("|{}", " --- |".repeat(cols)));
    for r in &rows[1..] {
        out.push(line(r));
    }
    out.join("\n")
}

// ─── DOCX ─────────────────────────────────────────────────────────────────────

/// style id -> style name (lower case), from word/styles.xml.
fn docx_styles(xml: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut r = Reader::from_str(xml);
    let mut current: Option<String> = None;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match local(&e).as_slice() {
                b"style" => current = attr(&e, b"styleId"),
                b"name" => {
                    if let (Some(id), Some(name)) = (current.clone(), attr(&e, b"val")) {
                        out.insert(id, name.to_lowercase());
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) if e.local_name().as_ref() == b"style" => current = None,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// The Markdown prefix of a paragraph style: "# ".."###### " for headings and the title, "- " for lists.
fn para_prefix(style: Option<&str>, styles: &HashMap<String, String>, listed: bool) -> String {
    let name = style
        .map(|s| styles.get(s).cloned().unwrap_or_else(|| s.to_lowercase()))
        .unwrap_or_default();
    let compact = name.replace(' ', "");
    if compact == "title" {
        return "# ".into();
    }
    if compact == "subtitle" {
        return "## ".into();
    }
    if let Some(rest) = compact.strip_prefix("heading") {
        if let Some(d) = rest.chars().next().and_then(|c| c.to_digit(10)) {
            let level = d.clamp(1, 6) as usize;
            return format!("{} ", "#".repeat(level));
        }
    }
    if listed || compact.starts_with("list") {
        return "- ".into();
    }
    String::new()
}

/// A DOCX as Markdown, one unit ("Document"). Err: why it could not be read.
pub fn docx_units(data: &[u8]) -> Result<Vec<Unit>, String> {
    let body = zip_entry(data, "word/document.xml", MAX_ENTRY_BYTES)?
        .ok_or_else(|| "the file is not a Word document (no word/document.xml)".to_string())?;
    let styles = match zip_entry(data, "word/styles.xml", MAX_ENTRY_BYTES) {
        Ok(Some(s)) => docx_styles(&String::from_utf8_lossy(&s)),
        _ => HashMap::new(),
    };
    let xml = String::from_utf8_lossy(&body).into_owned();
    let mut r = Reader::from_str(&xml);
    let mut blocks: Vec<String> = Vec::new();
    let mut para = String::new();
    let mut style: Option<String> = None;
    let mut listed = false;
    let mut in_t = false;
    let mut skip_depth = 0usize; // inside deleted text or field instructions
    let mut tbl_depth = 0usize;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    loop {
        let ev = match r.read_event() {
            Ok(ev) => ev,
            Err(_) => return Err("the Word document is damaged (its XML can't be read)".into()),
        };
        match ev {
            Event::Start(e) => match local(&e).as_slice() {
                b"t" => in_t = true,
                b"delText" | b"instrText" => skip_depth += 1,
                b"p" => {
                    para.clear();
                    style = None;
                    listed = false;
                }
                b"tbl" => {
                    tbl_depth += 1;
                    if tbl_depth == 1 {
                        rows.clear();
                    }
                }
                b"tr" if tbl_depth == 1 => row = Vec::new(),
                b"tc" if tbl_depth == 1 => cell.clear(),
                b"pStyle" => style = attr(&e, b"val"),
                b"numPr" => listed = true,
                _ => {}
            },
            Event::Empty(e) => match local(&e).as_slice() {
                b"pStyle" => style = attr(&e, b"val"),
                b"numPr" => listed = true,
                b"tab" if skip_depth == 0 => para.push('\t'),
                b"br" | b"cr" if skip_depth == 0 => para.push('\n'),
                _ => {}
            },
            Event::Text(t) => {
                if in_t && skip_depth == 0 {
                    para.push_str(&xml_text(&t));
                }
            }
            Event::CData(c) => {
                if in_t && skip_depth == 0 {
                    para.push_str(&String::from_utf8_lossy(&c));
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"t" => in_t = false,
                b"delText" | b"instrText" => skip_depth = skip_depth.saturating_sub(1),
                b"p" => {
                    let text = para.trim().to_string();
                    if tbl_depth > 0 {
                        if !text.is_empty() {
                            if !cell.is_empty() {
                                cell.push('\n');
                            }
                            cell.push_str(&text);
                        }
                    } else if !text.is_empty() {
                        let prefix = para_prefix(style.as_deref(), &styles, listed);
                        blocks.push(format!("{prefix}{text}"));
                    }
                    para.clear();
                }
                b"tc" if tbl_depth == 1 => row.push(std::mem::take(&mut cell)),
                b"tr" if tbl_depth == 1 => {
                    if row.iter().any(|c| !c.trim().is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    }
                }
                b"tbl" => {
                    tbl_depth = tbl_depth.saturating_sub(1);
                    if tbl_depth == 0 && !rows.is_empty() {
                        blocks.push(md_table(&rows));
                        rows.clear();
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    let text = blocks.join("\n\n");
    Ok(vec![Unit::new("Document", "text", text)])
}

// ─── XLSX ─────────────────────────────────────────────────────────────────────

/// "AB12" -> column index 27 (from 0).
fn col_index(cell_ref: &str) -> Option<usize> {
    let mut n = 0usize;
    let mut any = false;
    for c in cell_ref.chars() {
        if c.is_ascii_alphabetic() {
            n = n * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1);
            any = true;
        } else {
            break;
        }
    }
    if any {
        Some(n - 1)
    } else {
        None
    }
}

fn shared_strings(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut r = Reader::from_str(xml);
    let mut cur = String::new();
    let (mut in_si, mut in_t, mut in_rph) = (false, false, false);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => match local(&e).as_slice() {
                b"si" => {
                    in_si = true;
                    cur.clear();
                }
                b"t" => in_t = true,
                b"rPh" => in_rph = true,
                _ => {}
            },
            Ok(Event::Empty(e)) if local(&e).as_slice() == b"si" => out.push(String::new()),
            Ok(Event::Text(t)) if in_si && in_t && !in_rph => cur.push_str(&xml_text(&t)),
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"si" => {
                    in_si = false;
                    out.push(std::mem::take(&mut cur));
                }
                b"t" => in_t = false,
                b"rPh" => in_rph = false,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Built-in number formats that are dates or times.
fn builtin_date(id: u32) -> bool {
    (14..=22).contains(&id)
        || (45..=47).contains(&id)
        || (27..=36).contains(&id)
        || (50..=58).contains(&id)
}

/// Whether a custom number format shows a date or a time ("dd/mm/yyyy", "h:mm"), ignoring quoted text,
/// [colour] blocks and escaped characters.
fn custom_is_date(code: &str) -> bool {
    let mut out = String::new();
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for d in chars.by_ref() {
                    if d == '"' {
                        break;
                    }
                }
            }
            '[' => {
                for d in chars.by_ref() {
                    if d == ']' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                chars.next();
            }
            other => out.push(other.to_ascii_lowercase()),
        }
    }
    out.contains('y')
        || out.contains('d')
        || (out.contains('h') && out.contains('m'))
        || out.contains("mmm")
}

/// Per cell style index (the `s` attribute): is it a date format? From xl/styles.xml.
fn date_styles(xml: &str) -> Vec<bool> {
    let mut custom: HashMap<u32, bool> = HashMap::new();
    let mut xfs: Vec<bool> = Vec::new();
    let mut r = Reader::from_str(xml);
    let mut in_cellxfs = false;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match local(&e).as_slice() {
                b"numFmt" => {
                    if let (Some(id), Some(code)) = (
                        attr(&e, b"numFmtId").and_then(|v| v.parse::<u32>().ok()),
                        attr(&e, b"formatCode"),
                    ) {
                        custom.insert(id, custom_is_date(&code));
                    }
                }
                b"cellXfs" => in_cellxfs = true,
                b"xf" if in_cellxfs => {
                    let id = attr(&e, b"numFmtId")
                        .and_then(|v| v.parse::<u32>().ok())
                        .unwrap_or(0);
                    xfs.push(custom.get(&id).copied().unwrap_or_else(|| builtin_date(id)));
                }
                _ => {}
            },
            Ok(Event::End(e)) if e.local_name().as_ref() == b"cellXfs" => in_cellxfs = false,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    xfs
}

/// An Excel serial date as "YYYY-MM-DD" (and " HH:MM" when it has a time).
fn excel_date(serial: f64, date1904: bool) -> Option<String> {
    if !serial.is_finite() || !(0.0..2_958_466.0).contains(&serial) {
        return None;
    }
    let base = if date1904 {
        chrono::NaiveDate::from_ymd_opt(1904, 1, 1)?
    } else {
        // 1899-12-30 absorbs Excel's phantom 29 Feb 1900 for every date after it
        chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?
    };
    let days = serial.floor() as i64;
    let secs = ((serial - serial.floor()) * 86400.0).round() as i64;
    let dt =
        base.and_hms_opt(0, 0, 0)? + chrono::Duration::days(days) + chrono::Duration::seconds(secs);
    if secs == 0 {
        Some(dt.format("%Y-%m-%d").to_string())
    } else if days == 0 {
        Some(dt.format("%H:%M").to_string())
    } else {
        Some(dt.format("%Y-%m-%d %H:%M").to_string())
    }
}

/// A number as written in the sheet ("1.5", "1200"), without float noise.
fn number_text(v: &str) -> String {
    match v.trim().parse::<f64>() {
        Ok(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
        Ok(f) => {
            let s = format!("{f}");
            if s.len() > 17 {
                format!("{f:.6}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            } else {
                s
            }
        }
        Err(_) => v.to_string(),
    }
}

/// The rows of one sheet (up to `max_rows` non-empty rows): (rows, non-empty rows in the sheet).
fn sheet_rows(
    xml: &str,
    strings: &[String],
    dates: &[bool],
    date1904: bool,
    max_rows: usize,
) -> (Vec<Vec<String>>, usize) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut total = 0usize;
    let mut r = Reader::from_str(xml);
    let mut row: Vec<String> = Vec::new();
    let mut col = 0usize;
    let mut ctype = String::new();
    let mut cstyle = 0usize;
    let mut value = String::new();
    let (mut in_v, mut in_is_t) = (false, false);
    let mut in_cell = false;
    let mut next_col = 0usize;
    let push_cell = |row: &mut Vec<String>, col: usize, text: String| {
        if col >= MAX_COLS {
            return;
        }
        if row.len() <= col {
            row.resize(col + 1, String::new());
        }
        row[col] = text;
    };
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => match local(&e).as_slice() {
                b"row" => {
                    row = Vec::new();
                    next_col = 0;
                }
                b"c" => {
                    in_cell = true;
                    col = attr(&e, b"r")
                        .and_then(|v| col_index(&v))
                        .unwrap_or(next_col);
                    next_col = col + 1;
                    ctype = attr(&e, b"t").unwrap_or_default();
                    cstyle = attr(&e, b"s").and_then(|v| v.parse().ok()).unwrap_or(0);
                    value.clear();
                }
                b"v" if in_cell => in_v = true,
                b"t" if in_cell => in_is_t = true,
                _ => {}
            },
            Ok(Event::Empty(e)) if local(&e).as_slice() == b"c" => {
                // an empty cell: only moves the column on
                next_col = attr(&e, b"r")
                    .and_then(|v| col_index(&v))
                    .map(|c| c + 1)
                    .unwrap_or(next_col + 1);
            }
            Ok(Event::Text(t)) if in_v || in_is_t => value.push_str(&xml_text(&t)),
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"v" => in_v = false,
                b"t" => in_is_t = false,
                b"c" => {
                    in_cell = false;
                    let text = match ctype.as_str() {
                        "s" => value
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| strings.get(i).cloned())
                            .unwrap_or_default(),
                        "b" => if value.trim() == "1" { "TRUE" } else { "FALSE" }.to_string(),
                        "str" | "inlineStr" | "e" => value.clone(),
                        _ => {
                            if value.trim().is_empty() {
                                String::new()
                            } else if dates.get(cstyle).copied().unwrap_or(false) {
                                value
                                    .trim()
                                    .parse::<f64>()
                                    .ok()
                                    .and_then(|f| excel_date(f, date1904))
                                    .unwrap_or_else(|| number_text(&value))
                            } else {
                                number_text(&value)
                            }
                        }
                    };
                    push_cell(&mut row, col, text);
                }
                b"row" if row.iter().any(|c| !c.trim().is_empty()) => {
                    total += 1;
                    if rows.len() < max_rows {
                        rows.push(std::mem::take(&mut row));
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    // drop columns that are empty in every kept row
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let keep: Vec<usize> = (0..width)
        .filter(|&c| {
            rows.iter()
                .any(|r| r.get(c).is_some_and(|v| !v.trim().is_empty()))
        })
        .collect();
    let rows = rows
        .into_iter()
        .map(|r| {
            keep.iter()
                .map(|&c| r.get(c).cloned().unwrap_or_default())
                .collect()
        })
        .collect();
    (rows, total)
}

/// An XLSX: one unit per sheet ("Sheet: name"), each a Markdown table of at most `max_rows` rows.
pub fn xlsx_units(data: &[u8], max_rows: usize) -> Result<Vec<Unit>, String> {
    let wb = zip_entry(data, "xl/workbook.xml", MAX_ENTRY_BYTES)?
        .ok_or_else(|| "the file is not an Excel workbook (no xl/workbook.xml)".to_string())?;
    let wb = String::from_utf8_lossy(&wb).into_owned();
    // sheets in workbook order: (name, relationship id, hidden)
    let mut sheets: Vec<(String, String, bool)> = Vec::new();
    let mut date1904 = false;
    let mut r = Reader::from_str(&wb);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match local(&e).as_slice() {
                b"sheet" => {
                    let name = attr(&e, b"name").unwrap_or_default();
                    let rid = attr(&e, b"id").unwrap_or_default();
                    let hidden = attr(&e, b"state").is_some_and(|s| s != "visible");
                    sheets.push((name, rid, hidden));
                }
                b"workbookPr" => {
                    date1904 = attr(&e, b"date1904").is_some_and(|v| v == "1" || v == "true");
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let mut targets: HashMap<String, String> = HashMap::new();
    if let Some(rels) = zip_entry(data, "xl/_rels/workbook.xml.rels", MAX_ENTRY_BYTES)? {
        let rels = String::from_utf8_lossy(&rels).into_owned();
        let mut r = Reader::from_str(&rels);
        loop {
            match r.read_event() {
                Ok(Event::Start(e)) | Ok(Event::Empty(e))
                    if local(&e).as_slice() == b"Relationship" =>
                {
                    if let (Some(id), Some(t)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                        let path = match t.strip_prefix('/') {
                            Some(abs) => abs.to_string(),
                            None => format!("xl/{t}"),
                        };
                        targets.insert(id, path);
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
    }
    let strings = match zip_entry(data, "xl/sharedStrings.xml", MAX_ENTRY_BYTES)? {
        Some(s) => shared_strings(&String::from_utf8_lossy(&s)),
        None => vec![],
    };
    let dates = match zip_entry(data, "xl/styles.xml", MAX_ENTRY_BYTES)? {
        Some(s) => date_styles(&String::from_utf8_lossy(&s)),
        None => vec![],
    };
    let mut units = Vec::new();
    for (i, (name, rid, hidden)) in sheets.iter().enumerate() {
        let path = targets
            .get(rid)
            .cloned()
            .unwrap_or_else(|| format!("xl/worksheets/sheet{}.xml", i + 1));
        let Some(xml) = zip_entry(data, &path, MAX_ENTRY_BYTES)? else {
            continue;
        };
        let (rows, total) = sheet_rows(
            &String::from_utf8_lossy(&xml),
            &strings,
            &dates,
            date1904,
            max_rows,
        );
        let mut text = if rows.is_empty() {
            "(empty sheet)".to_string()
        } else {
            md_table(&rows)
        };
        if total > rows.len() {
            text.push_str(&format!(
                "\n\n(Only the first {} of {total} rows are shown; nary.files.sheet_max_rows.)",
                rows.len()
            ));
        }
        let label = if *hidden {
            format!("Sheet: {name} (hidden)")
        } else {
            format!("Sheet: {name}")
        };
        units.push(Unit::new(label, "table", text));
    }
    if units.is_empty() {
        return Err("the workbook has no sheets that can be read".into());
    }
    Ok(units)
}

// ─── text ─────────────────────────────────────────────────────────────────────

/// A text file (UTF-8; a BOM dropped, bad bytes replaced).
pub fn text_units(data: &[u8]) -> Vec<Unit> {
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
    let text = String::from_utf8_lossy(data).replace("\r\n", "\n");
    vec![Unit::new("Text", "text", text.trim().to_string())]
}

// ─── paging ───────────────────────────────────────────────────────────────────

/// One read of the units: from unit `page` (1-based) at character `offset` of it, whole units (each with its
/// marker) until `max_chars`; a unit longer than what is left is cut and the next read goes on inside it.
/// Returns (text, the page shown last, next (page, offset) or None at the end).
pub fn page(
    units: &[Unit],
    page: usize,
    offset: usize,
    max_chars: usize,
) -> (String, usize, Option<(usize, usize)>) {
    let max_chars = max_chars.max(200);
    let mut out = String::new();
    let mut used = 0usize;
    let mut last = page.max(1);
    let mut idx = page.max(1) - 1;
    let mut off = offset;
    while idx < units.len() {
        let u = &units[idx];
        let chars: Vec<char> = u.text.chars().collect();
        if off >= chars.len() && !(off == 0 && chars.is_empty()) {
            idx += 1;
            off = 0;
            continue;
        }
        let header = if off == 0 {
            format!("=== {} ===\n", u.label)
        } else {
            format!("=== {} (continued) ===\n", u.label)
        };
        let room = max_chars.saturating_sub(used + header.chars().count());
        if room == 0 && used > 0 {
            return (out, last, Some((idx + 1, off)));
        }
        let take = room.max(1).min(chars.len() - off.min(chars.len()));
        if used > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&header);
        out.extend(chars[off..off + take].iter());
        used += header.chars().count() + take + 2;
        last = idx + 1;
        if off + take < chars.len() {
            return (out, last, Some((idx + 1, off + take)));
        }
        idx += 1;
        off = 0;
        if used >= max_chars {
            break;
        }
    }
    let next = if idx < units.len() {
        Some((idx + 1, 0))
    } else {
        None
    };
    (out, last, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_of("a.PDF", None), "pdf");
        assert_eq!(kind_of("scan", Some("image/jpeg")), "image");
        assert_eq!(kind_of("x.docx", None), "docx");
        assert_eq!(kind_of("x.xlsx", None), "xlsx");
        assert_eq!(kind_of("x.csv", None), "text");
        assert_eq!(kind_of("x.doc", Some("application/msword")), "other");
    }

    #[test]
    fn columns_and_dates() {
        assert_eq!(col_index("A1"), Some(0));
        assert_eq!(col_index("AB12"), Some(27));
        assert_eq!(excel_date(45940.0, false).as_deref(), Some("2025-10-10"));
        assert!(custom_is_date("dd/mm/yyyy;@"));
        assert!(!custom_is_date("#,##0.00\" days\""));
        assert_eq!(number_text("1200.0"), "1200");
        assert_eq!(number_text("1.5"), "1.5");
    }

    #[test]
    fn paging_goes_on_where_it_stopped() {
        let units = vec![
            Unit::new("Page 1", "text", "x".repeat(300)),
            Unit::new("Page 2", "text", "y".repeat(300)),
        ];
        let (t, last, next) = page(&units, 1, 0, 250);
        assert!(t.starts_with("=== Page 1 ===\n"));
        assert_eq!(last, 1);
        let (p, o) = next.unwrap();
        assert_eq!(p, 1);
        let (t2, _, next2) = page(&units, p, o, 10_000);
        assert!(t2.starts_with("=== Page 1 (continued) ==="));
        assert!(t2.contains("=== Page 2 ===") && next2.is_none());
        let all: String = units.iter().map(|u| u.text.clone()).collect();
        let got: String = format!("{t}{t2}")
            .chars()
            .filter(|c| *c == 'x' || *c == 'y')
            .collect();
        assert_eq!(got, all);
    }

    #[test]
    fn markdown_tables_escape_pipes() {
        let t = md_table(&[vec!["Item".into(), "Qty".into()], vec!["Cam | 4MP".into()]]);
        assert_eq!(t, "| Item | Qty |\n| --- | --- |\n| Cam \\| 4MP |  |");
    }
}
