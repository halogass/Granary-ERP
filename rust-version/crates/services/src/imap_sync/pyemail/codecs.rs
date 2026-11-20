//! `bytes.decode(charset, errors)` as the Python that runs the backend does it, plus the binascii decoders
//! email uses (`a2b_base64` strict and lenient, `a2b_qp`) and `urllib.parse.unquote*`.
//!
//! Codec lookup follows `codecs.lookup`: the name is normalised (`_Py_normalize_encoding`), looked up in
//! `encodings.aliases`, then imported as `encodings.<module>`. Every single-byte codec decodes through the
//! table generated from Python (codec_tables.rs), so those are exact; ascii, latin-1, utf-8(-sig),
//! utf-16/32 are implemented here and exact. The CJK codecs go through encoding_rs: well-formed input
//! decodes the same, error handling inside malformed multi-byte sequences may differ. utf-7, idna, punycode
//! and the unicode_escape codecs are approximations (never seen as a mail charset).

use super::codec_tables as tables;
use super::pystr::{encode_utf8_escaped, esc, unesc};

/// Python's error handler argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Errors {
    Strict,
    Replace,
    SurrogateEscape,
}

/// What `bytes.decode` raises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeErr {
    /// LookupError: unknown encoding / not a text encoding.
    Lookup(String),
    /// UnicodeDecodeError.
    Decode(String),
    /// UnicodeEncodeError: the encoding name itself is not encodable (it holds surrogates).
    Encode(String),
    /// Another ValueError (embedded NUL in the name, the `undefined` codec).
    Value(String),
}

impl std::fmt::Display for DecodeErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeErr::Lookup(m)
            | DecodeErr::Decode(m)
            | DecodeErr::Encode(m)
            | DecodeErr::Value(m) => f.write_str(m),
        }
    }
}

/// The codec a name resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    Ascii,
    Latin1,
    Utf8,
    Utf8Sig,
    Utf16,
    Utf16Le,
    Utf16Be,
    Utf32,
    Utf32Le,
    Utf32Be,
    Utf7,
    Table(usize),
    Rs(&'static encoding_rs::Encoding),
    Undefined,
    Approx,
}

/// `_Py_normalize_encoding`: lower-case ASCII alnum and '.', any run of other characters becomes one '_'
/// (none at the start, a trailing run dropped).
pub fn normalize_encoding(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut punct = false;
    // The C code walks the UTF-8 bytes; a non-ASCII byte is punctuation.
    for &b in name.as_bytes() {
        if b.is_ascii_alphanumeric() || b == b'.' {
            if punct && !out.is_empty() {
                out.push('_');
            }
            punct = false;
            out.push(b.to_ascii_lowercase() as char);
        } else {
            punct = true;
        }
    }
    out
}

fn alias(name: &str) -> Option<&'static str> {
    tables::ALIASES
        .binary_search_by(|(k, _)| (*k).cmp(name))
        .ok()
        .map(|i| tables::ALIASES[i].1)
}

fn module_codec(module: &str) -> Option<Codec> {
    if let Some(i) = tables::TABLES.iter().position(|(m, _)| *m == module) {
        return Some(Codec::Table(i));
    }
    if module == "charmap" {
        return Some(Codec::Latin1);
    }
    if !tables::MULTI_BYTE.contains(&module) {
        return None;
    }
    use encoding_rs as e;
    Some(match module {
        "ascii" => Codec::Ascii,
        "latin_1" => Codec::Latin1,
        "utf_8" => Codec::Utf8,
        "utf_8_sig" => Codec::Utf8Sig,
        "utf_16" => Codec::Utf16,
        "utf_16_le" => Codec::Utf16Le,
        "utf_16_be" => Codec::Utf16Be,
        "utf_32" => Codec::Utf32,
        "utf_32_le" => Codec::Utf32Le,
        "utf_32_be" => Codec::Utf32Be,
        "utf_7" => Codec::Utf7,
        "undefined" => Codec::Undefined,
        "big5" | "big5hkscs" | "cp950" => Codec::Rs(e::BIG5),
        "cp932" | "shift_jis" | "shift_jis_2004" | "shift_jisx0213" => Codec::Rs(e::SHIFT_JIS),
        "cp949" | "euc_kr" | "johab" => Codec::Rs(e::EUC_KR),
        "euc_jp" | "euc_jis_2004" | "euc_jisx0213" => Codec::Rs(e::EUC_JP),
        "gb2312" | "gbk" | "hz" => Codec::Rs(e::GBK),
        "gb18030" => Codec::Rs(e::GB18030),
        "iso2022_jp" | "iso2022_jp_1" | "iso2022_jp_2" | "iso2022_jp_2004" | "iso2022_jp_3"
        | "iso2022_jp_ext" | "iso2022_kr" => Codec::Rs(e::ISO_2022_JP),
        _ => Codec::Approx, // idna, punycode, raw_unicode_escape, unicode_escape
    })
}

fn lookup(charset: &str) -> Result<Codec, DecodeErr> {
    if charset
        .chars()
        .any(|c| unesc(c).is_some() || (0xD800..=0xDFFF).contains(&(c as u32)))
    {
        return Err(DecodeErr::Encode(
            "'utf-8' codec can't encode character: surrogates not allowed".into(),
        ));
    }
    if charset.contains('\0') {
        return Err(DecodeErr::Value("embedded null character".into()));
    }
    let norm = normalize_encoding(charset);
    let aliased = alias(&norm).or_else(|| alias(&norm.replace('.', "_")));
    let mut modnames: Vec<String> = Vec::new();
    if let Some(a) = aliased {
        modnames.push(a.to_string());
    }
    modnames.push(norm.clone());
    for m in modnames {
        if m.is_empty() || m.contains('.') {
            continue;
        }
        if tables::NON_TEXT.contains(&m.as_str()) {
            return Err(DecodeErr::Lookup(format!(
                "'{m}' is not a text encoding; use codecs.decode() to handle arbitrary codecs"
            )));
        }
        if let Some(c) = module_codec(&m) {
            return Ok(c);
        }
    }
    Err(DecodeErr::Lookup(format!("unknown encoding: {charset}")))
}

/// Whether Python knows `charset` as a text encoding.
pub fn is_known(charset: &str) -> bool {
    lookup(charset).is_ok()
}

struct Out {
    s: String,
    errors: Errors,
    codec: &'static str,
}

impl Out {
    fn new(errors: Errors, codec: &'static str, cap: usize) -> Out {
        Out {
            s: String::with_capacity(cap),
            errors,
            codec,
        }
    }

    /// An undecodable run of bytes.
    fn bad(&mut self, bytes: &[u8], reason: &str) -> Result<(), DecodeErr> {
        match self.errors {
            Errors::Strict => Err(DecodeErr::Decode(format!(
                "'{}' codec can't decode bytes: {}",
                self.codec, reason
            ))),
            Errors::Replace => {
                self.s.push('\u{FFFD}');
                Ok(())
            }
            Errors::SurrogateEscape => {
                for &b in bytes {
                    if b < 0x80 {
                        return Err(DecodeErr::Decode(format!(
                            "'{}' codec can't decode bytes: {}",
                            self.codec, reason
                        )));
                    }
                }
                for &b in bytes {
                    self.s.push(esc(b));
                }
                Ok(())
            }
        }
    }
}

fn decode_utf8(data: &[u8], out: &mut Out) -> Result<(), DecodeErr> {
    let mut rest = data;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.s.push_str(s);
                return Ok(());
            }
            Err(e) => {
                let good = e.valid_up_to();
                out.s
                    .push_str(std::str::from_utf8(&rest[..good]).expect("valid prefix"));
                // Same maximal-subpart rule as CPython: an invalid start byte or a truncated sequence is
                // one error covering `error_len` bytes (the rest of the input when truncated at the end).
                let bad = e.error_len().unwrap_or(rest.len() - good);
                out.bad(&rest[good..good + bad], "invalid utf-8")?;
                rest = &rest[good + bad..];
                if rest.is_empty() {
                    return Ok(());
                }
            }
        }
    }
}

fn decode_utf16(data: &[u8], big: bool, out: &mut Out) -> Result<(), DecodeErr> {
    let mut i = 0;
    let unit = |i: usize| -> u16 {
        if big {
            u16::from_be_bytes([data[i], data[i + 1]])
        } else {
            u16::from_le_bytes([data[i], data[i + 1]])
        }
    };
    while i + 1 < data.len() {
        let u = unit(i);
        if !(0xD800..=0xDFFF).contains(&u) {
            out.s.push(char::from_u32(u as u32).expect("bmp"));
            i += 2;
            continue;
        }
        if u >= 0xDC00 {
            out.bad(&data[i..i + 2], "illegal encoding")?;
            i += 2;
            continue;
        }
        if i + 3 >= data.len() {
            // A high surrogate with nothing (or one byte) after it: truncated data to the end.
            out.bad(&data[i..], "unexpected end of data")?;
            return Ok(());
        }
        let u2 = unit(i + 2);
        if (0xDC00..=0xDFFF).contains(&u2) {
            let cp = 0x10000 + (((u as u32) - 0xD800) << 10) + ((u2 as u32) - 0xDC00);
            out.s.push(char::from_u32(cp).expect("astral"));
            i += 4;
        } else {
            out.bad(&data[i..i + 2], "illegal UTF-16 surrogate")?;
            i += 2;
        }
    }
    if i < data.len() {
        out.bad(&data[i..], "truncated data")?;
    }
    Ok(())
}

fn decode_utf32(data: &[u8], big: bool, out: &mut Out) -> Result<(), DecodeErr> {
    let mut i = 0;
    while i + 3 < data.len() {
        let b = [data[i], data[i + 1], data[i + 2], data[i + 3]];
        let cp = if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        };
        match char::from_u32(cp) {
            Some(c) => out.s.push(c),
            None => out.bad(&data[i..i + 4], "code point not in range")?,
        }
        i += 4;
    }
    if i < data.len() {
        out.bad(&data[i..], "truncated data")?;
    }
    Ok(())
}

fn decode_utf7(data: &[u8], out: &mut Out) -> Result<(), DecodeErr> {
    // Approximation of Python's utf_7 decoder for well-formed input.
    let b64 = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut i = 0;
    while i < data.len() {
        let c = data[i];
        if c == b'+' {
            i += 1;
            if i < data.len() && data[i] == b'-' {
                out.s.push('+');
                i += 1;
                continue;
            }
            let mut bits: u32 = 0;
            let mut nbits = 0;
            let mut units: Vec<u16> = Vec::new();
            while i < data.len() {
                match b64(data[i]) {
                    Some(v) => {
                        bits = (bits << 6) | v;
                        nbits += 6;
                        if nbits >= 16 {
                            nbits -= 16;
                            units.push(((bits >> nbits) & 0xFFFF) as u16);
                        }
                        i += 1;
                    }
                    None => break,
                }
            }
            if i < data.len() && data[i] == b'-' {
                i += 1;
            }
            for r in char::decode_utf16(units.iter().copied()) {
                match r {
                    Ok(ch) => out.s.push(ch),
                    Err(_) => out.bad(&[], "ill-formed sequence")?,
                }
            }
        } else if c < 0x80 {
            out.s.push(c as char);
            i += 1;
        } else {
            out.bad(&data[i..i + 1], "unexpected special character")?;
            i += 1;
        }
    }
    Ok(())
}

fn decode_rs(
    enc: &'static encoding_rs::Encoding,
    data: &[u8],
    out: &mut Out,
) -> Result<(), DecodeErr> {
    let mut decoder = enc.new_decoder_without_bom_handling();
    let mut buf = String::with_capacity(data.len() * 3 + 16);
    let mut rest = data;
    loop {
        buf.clear();
        buf.reserve(rest.len() * 3 + 16);
        let (res, read) = decoder.decode_to_string_without_replacement(rest, &mut buf, true);
        out.s.push_str(&buf);
        match res {
            encoding_rs::DecoderResult::InputEmpty => return Ok(()),
            encoding_rs::DecoderResult::OutputFull => {
                rest = &rest[read..];
            }
            encoding_rs::DecoderResult::Malformed(bad, extra) => {
                let end = read - extra as usize;
                let start = end.saturating_sub(bad as usize);
                out.bad(&rest[start..end], "illegal multibyte sequence")?;
                rest = &rest[read..];
                if rest.is_empty() {
                    return Ok(());
                }
            }
        }
    }
}

/// `data.decode(charset, errors)`.
pub fn decode(data: &[u8], charset: &str, errors: Errors) -> Result<String, DecodeErr> {
    if data.is_empty() {
        // PyUnicode_Decode returns '' for empty input before looking the codec up.
        if charset.chars().any(|c| unesc(c).is_some()) {
            return Err(DecodeErr::Encode(
                "'utf-8' codec can't encode character: surrogates not allowed".into(),
            ));
        }
        if charset.contains('\0') {
            return Err(DecodeErr::Value("embedded null character".into()));
        }
        return Ok(String::new());
    }
    let codec = lookup(charset)?;
    decode_with(codec, data, errors)
}

fn decode_with(codec: Codec, data: &[u8], errors: Errors) -> Result<String, DecodeErr> {
    match codec {
        Codec::Ascii => {
            let mut out = Out::new(errors, "ascii", data.len());
            for (i, &b) in data.iter().enumerate() {
                if b < 0x80 {
                    out.s.push(b as char);
                } else {
                    out.bad(&data[i..i + 1], "ordinal not in range(128)")?;
                }
            }
            Ok(out.s)
        }
        Codec::Latin1 => Ok(data.iter().map(|&b| b as char).collect()),
        Codec::Utf8 => {
            let mut out = Out::new(errors, "utf-8", data.len());
            decode_utf8(data, &mut out)?;
            Ok(out.s)
        }
        Codec::Utf8Sig => {
            let mut out = Out::new(errors, "utf-8", data.len());
            let body = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
            decode_utf8(body, &mut out)?;
            Ok(out.s)
        }
        Codec::Utf16 | Codec::Utf16Le | Codec::Utf16Be => {
            let mut out = Out::new(errors, "utf-16", data.len());
            let (body, big) = match codec {
                Codec::Utf16 => {
                    if data.starts_with(b"\xFF\xFE") {
                        (&data[2..], false)
                    } else if data.starts_with(b"\xFE\xFF") {
                        (&data[2..], true)
                    } else {
                        (data, false) // native order (little-endian)
                    }
                }
                Codec::Utf16Be => (data, true),
                _ => (data, false),
            };
            decode_utf16(body, big, &mut out)?;
            Ok(out.s)
        }
        Codec::Utf32 | Codec::Utf32Le | Codec::Utf32Be => {
            let mut out = Out::new(errors, "utf-32", data.len());
            let (body, big) = match codec {
                Codec::Utf32 => {
                    if data.starts_with(b"\xFF\xFE\x00\x00") {
                        (&data[4..], false)
                    } else if data.starts_with(b"\x00\x00\xFE\xFF") {
                        (&data[4..], true)
                    } else {
                        (data, false)
                    }
                }
                Codec::Utf32Be => (data, true),
                _ => (data, false),
            };
            decode_utf32(body, big, &mut out)?;
            Ok(out.s)
        }
        Codec::Utf7 => {
            let mut out = Out::new(errors, "utf7", data.len());
            decode_utf7(data, &mut out)?;
            Ok(out.s)
        }
        Codec::Table(i) => {
            let (name, table) = &tables::TABLES[i];
            let mut out = Out::new(errors, name, data.len());
            for (j, &b) in data.iter().enumerate() {
                match table[b as usize] {
                    Some(c) => out.s.push(c),
                    None => out.bad(&data[j..j + 1], "character maps to <undefined>")?,
                }
            }
            Ok(out.s)
        }
        Codec::Rs(enc) => {
            let mut out = Out::new(errors, "multibytecodec", data.len());
            decode_rs(enc, data, &mut out)?;
            Ok(out.s)
        }
        Codec::Undefined => Err(DecodeErr::Value("undefined encoding".into())),
        Codec::Approx => {
            let mut out = Out::new(errors, "codec", data.len());
            for (i, &b) in data.iter().enumerate() {
                if b < 0x80 {
                    out.s.push(b as char);
                } else {
                    out.bad(&data[i..i + 1], "ordinal not in range(128)")?;
                }
            }
            Ok(out.s)
        }
    }
}

// ─── binascii ────────────────────────────────────────────────────────────────

fn b64_value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// `binascii.a2b_base64(data, strict_mode=strict)`; Err is binascii.Error.
pub fn a2b_base64(data: &[u8], strict: bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 3);
    if strict && data.first() == Some(&b'=') {
        return Err("Leading padding not allowed".into());
    }
    let mut quad_pos = 0u8;
    let mut leftchar: u8 = 0;
    let mut pads = 0u8;
    let mut padding_started = false;
    for (i, &c) in data.iter().enumerate() {
        if c == b'=' {
            padding_started = true;
            if strict && quad_pos == 0 {
                return Err("Excess padding not allowed".into());
            }
            if quad_pos >= 2 {
                pads += 1;
                if quad_pos + pads >= 4 {
                    if strict && i + 1 < data.len() {
                        return Err("Excess data after padding".into());
                    }
                    return Ok(out);
                }
            }
            continue;
        }
        let v = match b64_value(c) {
            Some(v) => v,
            None => {
                if strict {
                    return Err("Only base64 data is allowed".into());
                }
                continue;
            }
        };
        if strict && padding_started {
            return Err("Discontinuous padding not allowed".into());
        }
        pads = 0;
        match quad_pos {
            0 => {
                quad_pos = 1;
                leftchar = v;
            }
            1 => {
                quad_pos = 2;
                out.push((leftchar << 2) | (v >> 4));
                leftchar = v & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push((leftchar << 4) | (v >> 2));
                leftchar = v & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push((leftchar << 6) | v);
                leftchar = 0;
            }
        }
    }
    if quad_pos != 0 {
        if quad_pos == 1 {
            return Err(format!(
                "Invalid base64-encoded string: number of data characters ({}) cannot be 1 more than a multiple of 4",
                out.len() / 3 * 4 + 1
            ));
        }
        return Err("Incorrect padding".into());
    }
    Ok(out)
}

/// `base64.b64decode(s, validate=validate)`.
pub fn b64decode(s: &[u8], validate: bool) -> Result<Vec<u8>, String> {
    a2b_base64(s, validate)
}

/// `email._encoded_words.decode_b(encoded)`: the decoded bytes (the defects are not kept).
pub fn decode_b(encoded: &[u8]) -> Vec<u8> {
    let pad_err = encoded.len() % 4;
    let mut padded = encoded.to_vec();
    if pad_err != 0 {
        padded.extend_from_slice(&b"==="[..4 - pad_err]);
    }
    if let Ok(v) = b64decode(&padded, true) {
        return v;
    }
    if let Ok(v) = b64decode(encoded, false) {
        return v;
    }
    let mut plus = encoded.to_vec();
    plus.extend_from_slice(b"==");
    if let Ok(v) = b64decode(&plus, false) {
        return v;
    }
    encoded.to_vec()
}

/// `binascii.a2b_qp(data, header=False)` (what `quopri.decodestring` uses).
pub fn a2b_qp(data: &[u8]) -> Vec<u8> {
    let hex = |c: u8| c.is_ascii_hexdigit();
    let val = |c: u8| (c as char).to_digit(16).unwrap() as u8;
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    let n = data.len();
    while i < n {
        if data[i] == b'=' {
            i += 1;
            if i >= n {
                break;
            }
            if data[i] == b'\n' || data[i] == b'\r' {
                if data[i] != b'\n' {
                    while i < n && data[i] != b'\n' {
                        i += 1;
                    }
                }
                if i < n {
                    i += 1;
                }
            } else if data[i] == b'=' {
                out.push(b'=');
                i += 1;
            } else if i + 1 < n && hex(data[i]) && hex(data[i + 1]) {
                out.push((val(data[i]) << 4) | val(data[i + 1]));
                i += 2;
            } else {
                out.push(b'=');
            }
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

/// `bytes.splitlines()` joined: removes \n, \r, \r\n line ends (what get_payload does before base64).
pub fn join_splitlines(data: &[u8]) -> Vec<u8> {
    data.iter()
        .copied()
        .filter(|&b| b != b'\n' && b != b'\r')
        .collect()
}

// ─── urllib.parse ────────────────────────────────────────────────────────────

fn unquote_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let v =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap(), 16).unwrap();
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// `urllib.parse.unquote_to_bytes(s)` for a str: Err when `s` is not UTF-8 encodable (surrogates).
#[allow(clippy::result_unit_err)] // Python raises UnicodeEncodeError; callers only branch on it
pub fn unquote_to_bytes(s: &str) -> Result<Vec<u8>, ()> {
    if s.chars().any(|c| unesc(c).is_some()) {
        return Err(());
    }
    Ok(unquote_bytes(s.as_bytes()))
}

/// `urllib.parse.unquote(s, encoding="latin-1")` (errors="replace", which latin-1 never needs).
pub fn unquote_latin1(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    // Python splits on runs of ASCII and unquotes those; non-ASCII characters pass through.
    let mut out = String::with_capacity(s.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if !run.is_empty() {
            for b in unquote_bytes(run.as_bytes()) {
                out.push(b as char);
            }
            run.clear();
        }
    };
    for c in s.chars() {
        if c.is_ascii() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// `bytes(text, "raw-unicode-escape")`.
pub fn encode_raw_unicode_escape(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        if let Some(b) = unesc(c) {
            out.extend_from_slice(format!("\\udc{:02x}", b).as_bytes());
            continue;
        }
        let cp = c as u32;
        if cp < 0x100 {
            out.push(cp as u8);
        } else if cp < 0x10000 {
            out.extend_from_slice(format!("\\u{:04x}", cp).as_bytes());
        } else {
            out.extend_from_slice(format!("\\U{:08x}", cp).as_bytes());
        }
    }
    out
}

/// `str.encode("utf-8", "surrogateescape")` re-exported for callers of this module.
pub fn utf8_escaped(s: &str) -> Vec<u8> {
    encode_utf8_escaped(s)
}
