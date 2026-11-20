//! `pydantic.networks.validate_email` on top of email-validator 2.2.0 (`check_deliverability=False`),
//! returning the normalized address or the exact `EmailSyntaxError` text.
//!
//! The ASCII path is a line-by-line port of email_validator/syntax.py. Internationalized domains are only
//! approximated (lower-casing instead of full UTS-46/IDNA 2008 processing); every address the ERP sees in
//! practice is ASCII after the @-sign.

use unicode_normalization::UnicodeNormalization;

const MAX_EMAIL_LENGTH: usize = 2048;
const EMAIL_MAX_LENGTH: usize = 254;
const LOCAL_PART_MAX_LENGTH: usize = 64;
const DNS_LABEL_LENGTH_LIMIT: usize = 63;
const DOMAIN_MAX_LENGTH: usize = 253;
const SPECIAL_USE_DOMAIN_NAMES: [&str; 6] =
    ["arpa", "invalid", "local", "localhost", "onion", "test"];
const CASE_INSENSITIVE_MAILBOX_NAMES: [&str; 15] = [
    "info",
    "marketing",
    "sales",
    "support",
    "abuse",
    "noc",
    "security",
    "postmaster",
    "hostmaster",
    "usenet",
    "news",
    "webmaster",
    "www",
    "uucp",
    "ftp",
];

fn is_atext(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_!#$%&'*+-/=?^`{|}~".contains(c)
}

fn is_atext_intl(c: char) -> bool {
    is_atext(c) || (c as u32) >= 0x80
}

/// `DOT_ATOM_TEXT` / `DOT_ATOM_TEXT_INTL`: atoms separated by single dots, no leading/trailing dot.
fn is_dot_atom(s: &str, atext: fn(char) -> bool) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|part| !part.is_empty() && part.chars().all(atext))
}

fn is_hostname_label(l: &str) -> bool {
    let b = l.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
}

fn is_dot_atom_hostname(s: &str) -> bool {
    s.split('.').all(is_hostname_label)
}

fn is_atext_hostname_intl(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '.' || (c as u32) >= 0x80
}

/// Rough `unicodedata.category(c)[0]`, enough for the checks below.
fn category_major(c: char) -> char {
    if c.is_alphabetic() {
        'L'
    } else if c.is_numeric() {
        'N'
    } else if c.is_control() {
        'C'
    } else if c.is_whitespace() {
        'Z'
    } else if c.is_ascii_punctuation() {
        // Python splits ASCII punctuation into P* and S*; both display the same way.
        'P'
    } else if (c as u32) >= 0x80 {
        'S'
    } else {
        'C'
    }
}

/// `safe_character_display`.
fn safe_char(c: char) -> String {
    if c == '\\' {
        return "\"\\\"".to_string();
    }
    if matches!(category_major(c), 'L' | 'N' | 'P' | 'S') {
        return super::types::py_repr_str(&c.to_string());
    }
    match c {
        ' ' => "SPACE".into(),
        '\u{a0}' => "NO-BREAK SPACE".into(),
        _ => {
            if (c as u32) < 0xFFFF {
                format!("U+{:04X}", c as u32)
            } else {
                format!("U+{:08X}", c as u32)
            }
        }
    }
}

fn sorted_set(chars: impl Iterator<Item = char>) -> String {
    let mut v: Vec<String> = chars.map(safe_char).collect();
    v.sort();
    v.dedup();
    v.join(", ")
}

fn length_reason(len: usize, limit: usize) -> String {
    let diff = len - limit;
    format!(
        "({diff} character{} too many)",
        if diff > 1 { "s" } else { "" }
    )
}

type R<T> = Result<T, String>;

fn split_at_unquoted(text: &str, specials: &[char]) -> R<(String, String)> {
    let mut inside_quote = false;
    let mut escaped = false;
    let mut left = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let nfc_first = chars[i..].iter().collect::<String>().nfc().next();
        if nfc_first != Some(c) {
            left.push(c);
        } else if inside_quote {
            left.push(c);
            if c == '\\' && !escaped {
                escaped = true;
            } else if c == '"' && !escaped {
                inside_quote = false;
                escaped = false;
            } else {
                escaped = false;
            }
        } else if c == '"' {
            left.push(c);
            inside_quote = true;
        } else if specials.contains(&c) {
            break;
        } else {
            left.push(c);
        }
    }
    if left.chars().count() == chars.len() {
        return Err("An email address must have an @-sign.".into());
    }
    let right: String = chars[left.chars().count()..].iter().collect();
    Ok((left, right))
}

fn unquote(text: &str) -> R<(String, bool)> {
    let chars: Vec<char> = text.chars().collect();
    let mut quoted = false;
    let mut escaped = false;
    let mut value = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if quoted {
            if escaped {
                value.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                if i != chars.len() - 1 {
                    let rest: Vec<String> = chars[i + 1..].iter().map(|c| safe_char(*c)).collect();
                    return Err(format!(
                        "Extra character(s) found after close quote: {}",
                        rest.join(", ")
                    ));
                }
                break;
            } else {
                value.push(c);
            }
        } else if i == 0 && c == '"' {
            quoted = true;
        } else {
            value.push(c);
        }
    }
    Ok((value, quoted))
}

fn check_unsafe_chars(s: &str, allow_space: bool) -> R<()> {
    let mut bad: Vec<char> = Vec::new();
    for c in s.chars() {
        match category_major(c) {
            'L' | 'N' | 'P' | 'S' => {}
            'Z' if c == ' ' || c == '\u{a0}' => {
                if !allow_space {
                    bad.push(c)
                }
            }
            _ => bad.push(c),
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    bad.sort();
    bad.dedup();
    let shown: Vec<String> = bad.into_iter().map(safe_char).collect();
    Err(format!(
        "The email address contains unsafe characters: {}.",
        shown.join(", ")
    ))
}

fn check_dot_atom(label: &str, start_descr: &str, end_descr: &str, is_hostname: bool) -> R<()> {
    if label.ends_with('.') {
        return Err(end_descr.replace("{}", "period"));
    }
    if label.starts_with('.') {
        return Err(start_descr.replace("{}", "period"));
    }
    if label.contains("..") {
        return Err("An email address cannot have two periods in a row.".into());
    }
    if is_hostname {
        if label.ends_with('-') {
            return Err(end_descr.replace("{}", "hyphen"));
        }
        if label.starts_with('-') {
            return Err(start_descr.replace("{}", "hyphen"));
        }
        if label.contains(".-") || label.contains("-.") {
            return Err(
                "An email address cannot have a period and a hyphen next to each other.".into(),
            );
        }
    }
    Ok(())
}

struct LocalPart {
    local_part: String,
    ascii: bool,
}

fn validate_local_part(local: &str, quoted: bool) -> R<LocalPart> {
    let n = local.chars().count();
    if n == 0 {
        return Err("There must be something before the @-sign.".into());
    }
    if n > LOCAL_PART_MAX_LENGTH {
        return Err(format!(
            "The email address is too long before the @-sign {}.",
            length_reason(n, LOCAL_PART_MAX_LENGTH)
        ));
    }
    if is_dot_atom(local, is_atext) {
        return Ok(LocalPart {
            local_part: local.to_string(),
            ascii: true,
        });
    }
    if is_dot_atom(local, is_atext_intl) {
        check_unsafe_chars(local, false)?;
        return Ok(LocalPart {
            local_part: local.to_string(),
            ascii: false,
        });
    }
    if quoted {
        // Quoted local parts are refused below (ALLOW_QUOTED_LOCAL = False) after these checks.
        return Ok(LocalPart {
            local_part: local.to_string(),
            ascii: local.is_ascii(),
        });
    }
    let bad: Vec<char> = local
        .chars()
        .filter(|c| !(*c == '.' || is_atext_intl(*c)))
        .collect();
    if !bad.is_empty() {
        return Err(format!(
            "The email address contains invalid characters before the @-sign: {}.",
            sorted_set(bad.into_iter())
        ));
    }
    check_dot_atom(
        local,
        "An email address cannot start with a {}.",
        "An email address cannot have a {} immediately before the @-sign.",
        false,
    )?;
    Err("The email address contains invalid characters before the @-sign.".into())
}

fn validate_domain(domain: &str) -> R<(String, String)> {
    let bad: Vec<char> = domain
        .chars()
        .filter(|c| !is_atext_hostname_intl(*c))
        .collect();
    if !bad.is_empty() {
        return Err(format!(
            "The part after the @-sign contains invalid characters: {}.",
            sorted_set(bad.into_iter())
        ));
    }
    check_unsafe_chars(domain, false)?;
    // idna.uts46_remap: lower-cases (approximation for non-ASCII input, see module docs).
    let domain: String = domain.nfc().collect::<String>().to_lowercase();
    check_dot_atom(
        &domain,
        "An email address cannot have a {} immediately after the @-sign.",
        "An email address cannot end with a {}.",
        true,
    )?;
    for label in domain.split('.') {
        let b = label.as_bytes();
        if b.len() >= 4 && &b[2..4] == b"--" && !label.to_ascii_lowercase().starts_with("xn") {
            return Err("An email address cannot have two letters followed by two dashes immediately after the @-sign or after a period, except Punycode.".into());
        }
    }
    // Non-ASCII domains: kept as typed (lower-cased); IDNA encoding is not reproduced.
    let ascii_domain = domain.clone();
    let len = ascii_domain.chars().count();
    if len > DOMAIN_MAX_LENGTH {
        return Err(format!(
            "The email address is too long after the @-sign {}.",
            length_reason(len, DOMAIN_MAX_LENGTH)
        ));
    }
    for label in ascii_domain.split('.') {
        let l = label.chars().count();
        if l > DNS_LABEL_LENGTH_LIMIT {
            return Err(format!(
                "After the @-sign, periods cannot be separated by so many characters {}.",
                length_reason(l, DNS_LABEL_LENGTH_LIMIT)
            ));
        }
    }
    if !ascii_domain.contains('.') {
        return Err("The part after the @-sign is not valid. It should have a period.".into());
    }
    if !ascii_domain
        .chars()
        .last()
        .is_some_and(|c| c.is_alphabetic())
    {
        return Err(
            "The part after the @-sign is not valid. It is not within a valid top-level domain."
                .into(),
        );
    }
    if domain.is_ascii() && !is_dot_atom_hostname(&ascii_domain) {
        return Err(
            "The email address contains invalid characters after the @-sign after IDNA encoding."
                .into(),
        );
    }
    for d in SPECIAL_USE_DOMAIN_NAMES {
        if ascii_domain == d || ascii_domain.ends_with(&format!(".{d}")) {
            return Err(
                "The part after the @-sign is a special-use or reserved name that cannot be used with email.".into(),
            );
        }
    }
    Ok((ascii_domain, domain))
}

/// Returns the normalized email, or the reason text Pydantic puts after
/// "value is not a valid email address: ".
pub fn validate_email(value: &str) -> R<String> {
    if value.chars().count() > MAX_EMAIL_LENGTH {
        return Err(format!(
            "Length must not exceed {MAX_EMAIL_LENGTH} characters"
        ));
    }
    let value = pretty_email_addr(value).unwrap_or(value);
    let email = value.trim();

    // email_validator.split_email
    let (left, right) = split_at_unquoted(email, &['@', '<'])?;
    let (display_name, local_raw, domain_raw) = if right.starts_with('<') {
        let left = left.trim_end().to_string();
        let (display_name, display_quoted) = unquote(&left)?;
        if !display_quoted {
            let bad: Vec<char> = display_name
                .chars()
                .filter(|c| (!(*c == '.' || is_atext(*c)) && *c != ' ') || *c == '.')
                .collect();
            if !bad.is_empty() {
                return Err(format!(
                    "The display name contains invalid characters when not quoted: {}.",
                    sorted_set(bad.into_iter())
                ));
            }
        }
        check_unsafe_chars(&display_name, true)?;
        if !right.contains('>') {
            return Err("An open angle bracket at the start of the email address has to be followed by a close angle bracket at the end.".into());
        }
        let right = right.trim_end_matches(' ');
        if !right.ends_with('>') {
            return Err("There can't be anything after the email address.".into());
        }
        let addr_spec = right[1..].trim_end_matches('>');
        let (l, d) = split_at_unquoted(addr_spec, &['@'])?;
        (Some(display_name), l, d)
    } else {
        (None, left, right)
    };
    let domain_part = domain_raw
        .strip_prefix('@')
        .unwrap_or(&domain_raw)
        .to_string();
    let (local_part, quoted) = unquote(&local_raw)?;

    let original = if quoted {
        format!("\"{local_part}\"@{domain_part}")
    } else {
        format!("{local_part}@{domain_part}")
    };
    let info = validate_local_part(&local_part, quoted)?;
    let mut local = info.local_part;
    let nfc: String = local.nfc().collect();
    if nfc != local {
        validate_local_part(&nfc, quoted)
            .map_err(|e| format!("After Unicode normalization: {e}"))?;
        local = nfc;
    }
    if quoted {
        return Err("Quoting the part before the @-sign is not allowed here.".into());
    }
    if info.ascii && CASE_INSENSITIVE_MAILBOX_NAMES.contains(&local.to_lowercase().as_str()) {
        local = local.to_lowercase();
    }
    if domain_part.is_empty() {
        return Err("There must be something after the @-sign.".into());
    }
    if domain_part.starts_with('[') && domain_part.ends_with(']') {
        // Domain literals are refused (ALLOW_DOMAIN_LITERAL = False) once they parse; the parse errors
        // of email_validator are not reproduced.
        return Err("A bracketed IP address after the @-sign is not allowed here.".into());
    }
    let (ascii_domain, domain) = validate_domain(&domain_part)?;
    let normalized = format!("{local}@{domain}");

    // validate_email_length
    for (addr, reason) in [
        (original.clone(), None),
        (normalized.clone(), Some("after normalization")),
        (
            format!("{local}@{ascii_domain}"),
            Some("when the part after the @-sign is converted to IDNA ASCII"),
        ),
    ] {
        let n_chars = addr.chars().count();
        let n_bytes = addr.len();
        if n_bytes > EMAIL_MAX_LENGTH {
            let diff = n_bytes - EMAIL_MAX_LENGTH;
            let s = if diff > 1 { "s" } else { "" };
            let why = match reason {
                None if n_chars == n_bytes => length_reason(n_chars, EMAIL_MAX_LENGTH),
                None => {
                    let mbpc = addr.chars().map(|c| c.len_utf8()).max().unwrap_or(1);
                    let mchars = std::cmp::max(1, diff / mbpc);
                    if mchars == diff {
                        format!("({diff} character{s} too many)")
                    } else {
                        format!("({mchars}-{diff} character{s} too many)")
                    }
                }
                Some(r) => format!("{r} ({diff} byte{s} too many)"),
            };
            return Err(format!("The email address is too long {why}."));
        }
    }
    if display_name.is_some() {
        return Err(
            "A display name and angle brackets around the email address are not permitted here."
                .into(),
        );
    }
    Ok(normalized)
}

/// `pretty_email_regex.fullmatch(value)`: `Name <addr>` / `"Name" <addr>`, returning `addr`.
fn pretty_email_addr(value: &str) -> Option<&str> {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        let name_chars = r#"[\w!#$%&'*+\-/=?^_`{|}~]"#;
        let unquoted = format!(r"((?:{name_chars}+\s+)*{name_chars}+)");
        let quoted = r#""((?:[^"]|\")+)""#;
        regex::Regex::new(&format!(r"^\s*(?:{unquoted}|{quoted})?\s*<(.+)>\s*$"))
            .expect("pretty email regex")
    });
    RE.captures(value)
        .and_then(|c| c.get(3))
        .map(|m| m.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        assert_eq!(
            validate_email("Foo.Bar@Example.COM").unwrap(),
            "Foo.Bar@example.com"
        );
        assert_eq!(validate_email("John <john@x.com>").unwrap(), "john@x.com");
        assert_eq!(validate_email(" a@b.com ").unwrap(), "a@b.com");
        assert_eq!(validate_email("Info@b.com").unwrap(), "info@b.com");
        assert_eq!(
            validate_email("bad").unwrap_err(),
            "An email address must have an @-sign."
        );
        assert_eq!(
            validate_email("a b@c.com").unwrap_err(),
            "The email address contains invalid characters before the @-sign: SPACE."
        );
    }
}
