//! Small pieces of Python behaviour the port needs in several places: `repr(str)` (it appears inside
//! error texts) and `int(str)` (the proxy's CONNECT port).

use granary_compat::text::pystr;

/// `str.isprintable()` for one character: not Cc/Cf/Cs/Co/Cn/Zl/Zp/Zs, except the ASCII space.
pub fn is_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if pystr::is_hidden_category(c) {
        return false;
    }
    !matches!(
        c,
        '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

/// `repr(s)` for a str.
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if c.is_ascii() => out.push(c),
            c if is_printable(c) => out.push(c),
            c if (c as u32) <= 0xff => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) <= 0xffff => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}

/// `int(s)` (base 10) for a str: surrounding whitespace, a sign, single underscores between digits
/// (any Unicode decimal digit counts, as in Python). `Err` is the ValueError text.
pub fn py_int(s: &str) -> Result<i128, String> {
    let err = || format!("invalid literal for int() with base 10: {}", repr_str(s));
    let t = pystr::strip(s);
    let (neg, body) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return Err(err());
    }
    let mut v: i128 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = pystr::decimal_value(c).ok_or_else(err)?;
        v = v.saturating_mul(10).saturating_add(d as i128);
    }
    Ok(if neg { -v } else { v })
}
