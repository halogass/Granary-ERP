//! Port of backend/app/services/wa_export.py.
//!
//! Export a WhatsApp conversation as a plain-text or PDF transcript.
//!
//! Times are shown in the company time zone (`tz`, from the `timezone` setting; the caller passes it).
//!
//! Messages are not stored in the ERP (they are read live from the wwebjs service), so everything here
//! works off the message dicts that `/whatsapp/messages` returns, as `serde_json::Value` objects:
//!     {id, body, fromMe, timestamp, type, author, hasMedia, media: {mimetype, filename, data}, quoted}
//!
//! This module only shapes data. The router renders the PDF through the PDF service
//! (`generate_pdf("whatsapp_transcript", {...company info, "messages": build_pdf_messages(...)})`) or
//! returns `build_txt(...)` as text/plain; there is no zip.
//!
//! Image re-encoding (`_shrink_image`) used Pillow; here it uses the `image` crate. The result has the same
//! size in pixels and the same JPEG quality setting, but the bytes differ: Pillow reads JPEGs in draft
//! (DCT-scaled) mode, resizes with its own bicubic convolution and encodes with libjpeg (optimized Huffman
//! tables), while this uses `image`'s CatmullRom resize and its pure-Rust encoder. The golden test checks the
//! sizes exactly and the pixels within a small mean difference (see tests/wa_export_golden.rs).

use chrono::{DateTime, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use granary_compat::crypto::pyb64;
use granary_compat::pyfmt;
use granary_compat::pyjson::{PyDateTime, PyTz};
use granary_compat::text::pystr;
use serde_json::{json, Map, Value};

/// Embedded images are re-encoded so a long chat doesn't produce a 200 MB PDF payload.
pub const MAX_IMAGE_EDGE: u32 = 900; // px on the longest side
pub const JPEG_QUALITY: u8 = 72;
pub const MAX_EMBEDDED_IMAGES: usize = 300; // beyond this, images fall back to a placeholder

const TYPE_LABELS: [(&str, &str); 9] = [
    ("image", "image"),
    ("video", "video"),
    ("document", "document"),
    ("audio", "audio"),
    ("ptt", "voice note"),
    ("sticker", "sticker"),
    ("location", "location"),
    ("vcard", "contact card"),
    ("revoked", "message deleted"),
];

// ---------------------------------------------------------------------------------- Python value rules

/// Python truthiness of a JSON value (None, False, 0, "", [], {} are falsy).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `x or fallback` for a value used as text.
fn or_value(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| truthy(Some(x)))
}

/// `str(x)` for the JSON scalars the wwebjs service sends (strings as-is).
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => match n.as_f64() {
                Some(f) if n.is_f64() => pystr::float_repr(f),
                _ => n.to_string(),
            },
        },
        other => other.to_string(),
    }
}

/// `x or {}` as a dict (a non-dict truthy value has no `.get`; treated as empty here).
fn dict_of(v: Option<&Value>) -> Option<&Map<String, Value>> {
    match v {
        Some(Value::Object(o)) => Some(o),
        _ => None,
    }
}

/// `s[:n]` (code points).
fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `int(ts)` for a timestamp value. Strings that `int()` would reject (and other types) count as no time
/// here, where Python would raise.
fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => {
            let t = pystr::strip(s).replace('_', "");
            t.parse::<i64>().ok()
        }
        _ => None,
    }
}

/// A company-time datetime as Python carries it (`datetime` with a ZoneInfo).
#[derive(Debug, Clone, Copy)]
pub struct Local {
    pub utc: DateTime<Utc>,
    pub py: PyDateTime,
    /// `utcoffset()` in seconds, as ZoneInfo gives it (fold=0).
    pub offset: i32,
}

impl Local {
    /// `isoformat()`.
    pub fn isoformat(&self) -> String {
        PyDateTime {
            local: self.py.local,
            tz: Some(PyTz::Fixed(self.offset)),
        }
        .isoformat()
    }

    pub fn strftime(&self, fmt: &str) -> String {
        pyfmt::strftime(&self.py, fmt)
    }
}

fn _local(ts: Option<&Value>, tz: Tz) -> Option<Local> {
    if !truthy(ts) {
        return None;
    }
    let secs = py_int(ts?)?;
    let utc = Utc.timestamp_opt(secs, 0).single()?;
    Some(Local {
        utc,
        py: PyDateTime::from_utc_in_zone(utc.naive_utc(), tz),
        offset: tz
            .offset_from_utc_datetime(&utc.naive_utc())
            .fix()
            .local_minus_utc(),
    })
}

/// `naive.replace(tzinfo=ZoneInfo)` as an instant. ZoneInfo uses fold=0: an ambiguous wall time takes the
/// earlier (pre-transition) offset, and a wall time inside a gap also takes the offset in force before the
/// transition (so it lands after the gap in UTC). PyDateTime::utcoffset follows PEP 495.
fn localize(naive: NaiveDateTime, tz: Tz) -> Local {
    let py = PyDateTime::in_zone(naive, tz, 0);
    let offset = py.utcoffset().unwrap_or(0);
    Local {
        utc: Utc.from_utc_datetime(&py.to_utc_naive()),
        py,
        offset,
    }
}

// ---------------------------------------------------------------------------------- strptime %Y-%m-%d

/// One `re` alternative of `_strptime`'s pattern, matched at `i`: each element is the set a char must be in.
fn match_seq(chars: &[char], i: usize, seq: &[&dyn Fn(char) -> bool]) -> Option<usize> {
    let mut j = i;
    for test in seq {
        let c = *chars.get(j)?;
        if !test(c) {
            return None;
        }
        j += 1;
    }
    Some(j)
}

fn digit_val(c: char) -> u32 {
    pystr::decimal_value(c).unwrap_or(0)
}

fn number_of(chars: &[char]) -> u32 {
    chars
        .iter()
        .filter(|c| **c != ' ')
        .fold(0, |acc, c| acc * 10 + digit_val(*c))
}

/// `datetime.strptime(s, "%Y-%m-%d")`: CPython's `_strptime` regex (`\d` is any Unicode decimal), first
/// alternative wins, the whole string must be consumed, then the date must exist.
fn strptime_ymd(s: &str) -> Option<NaiveDate> {
    let chars: Vec<char> = s.chars().collect();
    let d = |c: char| pystr::is_decimal(c);
    let lit = |x: char| move |c: char| c == x;
    // (?P<Y>\d\d\d\d)
    let y_end = match_seq(&chars, 0, &[&d, &d, &d, &d])?;
    let year = number_of(&chars[0..y_end]);
    // (?P<m>1[0-2]|0[1-9]|[1-9])
    let one = lit('1');
    let zero = lit('0');
    let r02 = |c: char| ('0'..='2').contains(&c);
    let r19 = |c: char| ('1'..='9').contains(&c);
    let r12 = |c: char| c == '1' || c == '2';
    let r01 = |c: char| c == '0' || c == '1';
    let three = lit('3');
    let space = lit(' ');
    let dash = lit('-');
    let m_alts: [&[&dyn Fn(char) -> bool]; 3] = [&[&one, &r02], &[&zero, &r19], &[&r19]];
    // (?P<d>3[01]|[12]\d|0[1-9]|[1-9]| [1-9])
    let d_alts: [&[&dyn Fn(char) -> bool]; 5] = [
        &[&three, &r01],
        &[&r12, &d],
        &[&zero, &r19],
        &[&r19],
        &[&space, &r19],
    ];
    match_seq(&chars, y_end, &[&dash])?;
    for m_alt in m_alts {
        let Some(m_end) = match_seq(&chars, y_end + 1, m_alt) else {
            continue;
        };
        let Some(dash_end) = match_seq(&chars, m_end, &[&dash]) else {
            continue;
        };
        for d_alt in d_alts {
            if let Some(d_end) = match_seq(&chars, dash_end, d_alt) {
                // match() succeeded; leftover characters are "unconverted data remains".
                if d_end != chars.len() {
                    return None;
                }
                let month = number_of(&chars[y_end + 1..m_end]);
                let day = number_of(&chars[dash_end..d_end]);
                if year == 0 {
                    return None;
                }
                return NaiveDate::from_ymd_opt(year as i32, month, day);
            }
        }
    }
    None
}

/// '2026-08-01' -> an aware company-time datetime at the start (or end) of that day.
pub fn parse_day(value: Option<&str>, tz: Tz, end: bool) -> Option<Local> {
    let value = value.filter(|v| !v.is_empty())?;
    let day = strptime_ymd(&head_chars(pystr::strip(value), 10))?;
    let moment = if end {
        day.and_hms_opt(23, 59, 59)?
    } else {
        day.and_hms_opt(0, 0, 0)?
    };
    Some(localize(moment, tz))
}

pub fn filter_range(
    messages: &[Value],
    date_from: Option<&str>,
    date_to: Option<&str>,
    tz: Tz,
) -> Vec<Value> {
    let start = parse_day(date_from, tz, false);
    let end = parse_day(date_to, tz, true);
    if start.is_none() && end.is_none() {
        return messages.to_vec();
    }
    let mut out = Vec::new();
    for m in messages {
        let Some(when) = _local(m.get("timestamp"), tz) else {
            continue;
        };
        if let Some(s) = &start {
            if when.utc < s.utc {
                continue;
            }
        }
        if let Some(e) = &end {
            if when.utc > e.utc {
                continue;
            }
        }
        out.push(m.clone());
    }
    out
}

/// Human placeholder for a non-text message, e.g. '[document: contract.pdf]'.
pub fn media_label(msg: &Value) -> String {
    let ty = or_value(msg.get("type"));
    let key = ty.map(py_str).unwrap_or_default();
    let kind = TYPE_LABELS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| ty.map(py_str).unwrap_or_else(|| "attachment".to_string()));
    let name = dict_of(or_value(msg.get("media"))).and_then(|m| or_value(m.get("filename")));
    match name {
        Some(n) => format!("[{kind}: {}]", py_str(n)),
        None => format!("[{kind}]"),
    }
}

/// Pillow's `thumbnail` size: fit inside `max` x `max` keeping the aspect, never enlarging
/// (`round_aspect` picks floor or ceil by the closest aspect, floor on a tie, at least 1).
pub fn thumbnail_size(width: u32, height: u32, max: u32) -> Option<(u32, u32)> {
    let (x, y) = (max, max);
    if x >= width && y >= height {
        return None;
    }
    let aspect = width as f64 / height as f64;
    let (xf, yf) = (x as f64, y as f64);
    let pick = |number: f64, key: &dyn Fn(f64) -> f64| -> u32 {
        let lo = number.floor();
        let hi = number.ceil();
        let chosen = if key(hi) < key(lo) { hi } else { lo };
        (chosen as u32).max(1)
    };
    if xf / yf >= aspect {
        let nx = pick(yf * aspect, &|n| (aspect - n / yf).abs());
        Some((nx, y))
    } else {
        let ny = pick(xf / aspect, &|n| {
            if n == 0.0 {
                0.0
            } else {
                (aspect - xf / n).abs()
            }
        });
        Some((x, ny))
    }
}

/// The thumbnail `_shrink_image` encodes (Pillow's `thumbnail` + the RGB/L conversion), before JPEG.
pub fn shrink_pixels(raw: &[u8]) -> Option<image::DynamicImage> {
    use image::DynamicImage;
    let img = image::load_from_memory(raw).ok()?;
    // Pillow resizes "P" (palette) and "1" (bilevel) images with NEAREST whatever filter is asked for.
    let img = match thumbnail_size(img.width(), img.height(), MAX_IMAGE_EDGE) {
        Some((w, h)) if pillow_nearest_mode(raw) => {
            DynamicImage::ImageRgba8(pillow_nearest(&img.to_rgba8(), w, h))
        }
        Some((w, h)) => img.resize_exact(w, h, image::imageops::FilterType::CatmullRom),
        None => img,
    };
    // Pillow keeps "RGB" and "L"; everything else is converted to RGB (alpha dropped, not composited).
    let img = match img {
        DynamicImage::ImageRgb8(_) | DynamicImage::ImageLuma8(_) => img,
        other => DynamicImage::ImageRgb8(other.to_rgb8()),
    };
    Some(img)
}

/// Re-encode a base64 image small enough to embed. None if it can't be read.
pub fn _shrink_image(data_b64: &Value, _mimetype: &str) -> Option<String> {
    use base64::Engine as _;
    // A non-string payload made Python's b64decode raise inside the try: placeholder.
    let Value::String(s) = data_b64 else {
        return None;
    };
    let raw = pyb64::b64decode_str(s).ok()?;
    let img = shrink_pixels(&raw)?;
    let mut buf = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    img.write_with_encoder(enc).ok()?;
    Some(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&buf)
    ))
}

/// Whether Pillow opens these bytes in mode "P" or "1": a GIF (first frame), or a PNG whose IHDR says
/// palette (colour type 3) or 1-bit greyscale.
fn pillow_nearest_mode(raw: &[u8]) -> bool {
    if raw.starts_with(b"GIF87a") || raw.starts_with(b"GIF89a") {
        return true;
    }
    if raw.len() >= 26 && raw.starts_with(b"\x89PNG\r\n\x1a\n") && &raw[12..16] == b"IHDR" {
        let (depth, colour) = (raw[24], raw[25]);
        return colour == 3 || (colour == 0 && depth == 1);
    }
    false
}

/// Pillow's NEAREST resize (an affine transform sampling pixel centres): source = floor((x + 0.5) * scale).
fn pillow_nearest(src: &image::RgbaImage, w: u32, h: u32) -> image::RgbaImage {
    let (sw, sh) = src.dimensions();
    let (fx, fy) = (sw as f64 / w as f64, sh as f64 / h as f64);
    image::RgbaImage::from_fn(w, h, |x, y| {
        let sx = (((x as f64 + 0.5) * fx).floor() as u32).min(sw - 1);
        let sy = (((y as f64 + 0.5) * fy).floor() as u32).min(sh - 1);
        *src.get_pixel(sx, sy)
    })
}

fn is_non_chat(m: &Value) -> bool {
    // `m.get("type") not in ("chat", None, "")`
    match m.get("type") {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !(s == "chat" || s.is_empty()),
        Some(_) => true,
    }
}

fn body_of(m: &Value) -> String {
    match or_value(m.get("body")) {
        Some(v) => pystr::strip(&py_str(v)).to_string(),
        None => String::new(),
    }
}

/// WhatsApp-style transcript: '[01/08/2026 09:12] Diana: hello'.
pub fn build_txt(
    title: &str,
    subtitle: &str,
    meta_lines: &[String],
    messages: &[Value],
    me_label: &str,
    tz: Tz,
) -> String {
    let mut lines: Vec<String> = vec![title.to_string()];
    if !subtitle.is_empty() {
        lines.push(subtitle.to_string());
    }
    lines.extend(meta_lines.iter().cloned());
    lines.push(String::new());

    for m in messages {
        let when = _local(m.get("timestamp"), tz);
        let stamp = match &when {
            Some(w) => w.strftime("%d/%m/%Y %H:%M"),
            None => "unknown time".to_string(),
        };
        let who = if truthy(m.get("fromMe")) {
            me_label.to_string()
        } else if let Some(a) = or_value(m.get("author")) {
            py_str(a)
        } else if !subtitle.is_empty() {
            subtitle.to_string()
        } else {
            "Them".to_string()
        };
        if let Some(q) = dict_of(or_value(m.get("quoted"))) {
            if let Some(qb) = or_value(q.get("body")) {
                lines.push(format!(
                    "[{stamp}] {who} replying to: \"{}\"",
                    head_chars(&py_str(qb), 120)
                ));
            }
        }
        let mut body = body_of(m);
        if truthy(m.get("hasMedia")) || is_non_chat(m) {
            let suffix = if body.is_empty() {
                String::new()
            } else {
                format!(" {body}")
            };
            body = format!("{}{suffix}", media_label(m));
        }
        let text = if body.is_empty() {
            "[no content]".to_string()
        } else {
            body
        };
        let mut parts = pystr::splitlines(&text);
        if parts.is_empty() {
            parts.push("");
        }
        for (i, part) in parts.iter().enumerate() {
            lines.push(if i == 0 {
                format!("[{stamp}] {who}: {part}")
            } else {
                format!("    {part}")
            });
        }
    }

    lines.push(String::new());
    lines.push(format!("{} messages.", messages.len()));
    lines.join("\n") + "\n"
}

/// Shape messages for the Handlebars transcript template, embedding images inline.
///
/// Python quirk kept: a message whose `quoted` has `"body": null` raised TypeError (`None[:160]`), which
/// failed the whole export; that is the `Err` here, with Python's message.
pub fn build_pdf_messages(
    messages: &[Value],
    me_label: &str,
    them_label: &str,
    tz: Tz,
) -> Result<Vec<Value>, String> {
    let mut out: Vec<Value> = Vec::new();
    let mut embedded = 0usize;
    let mut last_day: Option<String> = None;

    for m in messages {
        let when = _local(m.get("timestamp"), tz);
        let day = when
            .as_ref()
            .map(|w| w.strftime("%A, %d %B %Y"))
            .unwrap_or_default();
        let quoted = match dict_of(or_value(m.get("quoted"))).and_then(|q| q.get("body")) {
            None => Value::String(String::new()),
            Some(Value::String(s)) => Value::String(head_chars(s, 160)),
            Some(Value::Null) => return Err("'NoneType' object is not subscriptable".into()),
            Some(Value::Array(a)) => {
                // `list[:160] or ""`: a list slices too and lands in the row as a list.
                if a.is_empty() {
                    Value::String(String::new())
                } else {
                    Value::Array(a.iter().take(160).cloned().collect())
                }
            }
            Some(other) => {
                let ty = match other {
                    Value::Bool(_) => "bool",
                    Value::Number(n) if n.is_f64() => "float",
                    Value::Number(_) => "int",
                    _ => "dict",
                };
                return Err(format!("'{ty}' object is not subscriptable"));
            }
        };
        let sender = if truthy(m.get("fromMe")) {
            me_label.to_string()
        } else if let Some(a) = or_value(m.get("author")) {
            py_str(a)
        } else {
            them_label.to_string()
        };
        let day_separator = if Some(&day) != last_day.as_ref() {
            day.clone()
        } else {
            String::new()
        };
        let body = body_of(m);
        let mut image: Option<String> = None;
        let mut placeholder = String::new();
        if !day.is_empty() {
            last_day = Some(day.clone());
        }

        let media = dict_of(or_value(m.get("media")));
        let is_image = m.get("type").and_then(Value::as_str) == Some("image")
            && media.map(|md| truthy(md.get("data"))).unwrap_or(false);
        if is_image && embedded < MAX_EMBEDDED_IMAGES {
            let md = media.expect("checked");
            let mime = or_value(md.get("mimetype"))
                .map(py_str)
                .unwrap_or_else(|| "image/jpeg".into());
            image = _shrink_image(md.get("data").expect("checked"), &mime);
            if image.is_some() {
                embedded += 1;
            }
        }
        if image.is_none() && (truthy(m.get("hasMedia")) || is_non_chat(m)) {
            placeholder = media_label(m);
        }
        if body.is_empty() && image.is_none() && placeholder.is_empty() {
            placeholder = "[no content]".to_string();
        }
        out.push(json!({
            "day_separator": day_separator,
            "time": when.as_ref().map(|w| w.strftime("%H:%M")).unwrap_or_default(),
            "from_me": truthy(m.get("fromMe")),
            "sender": sender,
            "body": body,
            "image": image,
            "placeholder": placeholder,
            "quoted": quoted,
        }));
    }

    Ok(out)
}
