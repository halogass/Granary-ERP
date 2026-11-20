//! Port of backend/app/services/imap_sync.py: IMAP pieces shared by the personal mailbox and the CRM
//! shared inbox.
//!
//!   * finding the real name of each standard folder (Sent is "Sent", "Sent Items", "INBOX.Sent",
//!     "[Gmail]/Sent Mail"... depending on the server);
//!   * quoting mailbox names, which IMAP requires as soon as a name has a space in it;
//!   * reading and setting the \Seen flag, so a message read in Thunderbird or webmail shows as read here,
//!     and one read here shows as read there;
//!   * moving deleted mail to Trash, as a mail client's Delete key does;
//!   * saving a copy of what we send into the server's Sent folder, as every mail client does.
//!
//! The connection object is [`aioimap::Imap4`], a protocol-level reproduction of aioimaplib (the raw
//! response lines are what these functions parse). [`pyemail`] is Python's `email` package, used by the
//! pollers to read the fetched messages.

pub mod aioimap;
pub mod pyemail;

use std::collections::{BTreeMap, BTreeSet};

use once_cell::sync::Lazy;
use regex::Regex;

pub use aioimap::{Imap4, ImapError, Response};

/// Standard mailbox roles -> SPECIAL-USE attribute (lower-case).
pub const ROLE_SPECIAL: [(&str, &str); 5] = [
    ("SENT", "\\sent"),
    ("DRAFTS", "\\drafts"),
    ("SPAM", "\\junk"),
    ("TRASH", "\\trash"),
    ("ARCHIVE", "\\archive"),
];

/// Name fallbacks per role (lower-case, compared with the whole name or its last path segment).
pub fn role_names(role: &str) -> &'static [&'static str] {
    match role {
        "SENT" => &["sent", "sent items", "sent mail", "sent messages"],
        "DRAFTS" => &["drafts", "draft"],
        "SPAM" => &["junk", "spam", "bulk mail", "junk e-mail", "junk email"],
        "TRASH" => &[
            "trash",
            "deleted",
            "deleted items",
            "deleted messages",
            "bin",
        ],
        "ARCHIVE" => &["archive", "archives", "all mail"],
        _ => &[],
    }
}

pub const FOLDER_ORDER: [&str; 6] = ["INBOX", "SENT", "DRAFTS", "ARCHIVE", "SPAM", "TRASH"];
pub const FLAG_BATCH: usize = 500;

/// `resolve_folders()`'s dict: role -> the server's mailbox name, in insertion order (INBOX first, then
/// the roles in ROLE_SPECIAL order).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folders(pub Vec<(String, String)>);

impl Folders {
    pub fn get(&self, role: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(r, _)| r == role)
            .map(|(_, n)| n.as_str())
    }
}

static MAILBOX_PLAIN: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9_.\-/\[\]]+$").unwrap());

/// `quote_mailbox(name)`: IMAP needs a quoted string for any name with spaces or specials.
pub fn quote_mailbox(name: &str) -> String {
    if MAILBOX_PLAIN.is_match(name) {
        return name.to_string();
    }
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `line.decode(errors="ignore")`.
fn decode_ignore(line: &[u8]) -> String {
    let mut out = String::new();
    let mut rest = line;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let good = e.valid_up_to();
                out.push_str(std::str::from_utf8(&rest[..good]).unwrap());
                let bad = e.error_len().unwrap_or(rest.len() - good);
                rest = &rest[good + bad..];
                if rest.is_empty() {
                    return out;
                }
            }
        }
    }
}

// (\(([^)]*)\)\s+("(?:[^"\\]|\\.)*"|NIL|\S+)\s+(.*)$, re.I) with re.match semantics (anchored at start).
static LIST_LINE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)^\(([^)]*)\)\s+("(?:[^"\\]|\\.)*"|NIL|\S+)\s+(.*)$"#).unwrap());

/// `parse_list_line(line)`: one LIST response line -> (attributes, delimiter, name).
pub fn parse_list_line(line: &[u8]) -> (BTreeSet<String>, Option<String>, Option<String>) {
    let s = decode_ignore(line);
    // Python's `.` excludes \n and `$` may match before a final \n; LIST lines hold no newline.
    // Python's `$` also matches before a final newline.
    let t = s.strip_suffix('\n').unwrap_or(&s);
    let caps = match LIST_LINE.captures(t) {
        Some(c) => c,
        None => return (BTreeSet::new(), None, None),
    };
    let attrs: BTreeSet<String> = granary_compat::text::pystr::split(&caps[1])
        .into_iter()
        .map(granary_compat::text::pystr::lower)
        .collect();
    let g2 = &caps[2];
    let delim = if g2.to_uppercase() != "NIL" {
        Some(g2.trim_matches('"').to_string())
    } else {
        None
    };
    let mut name = granary_compat::text::pystr::strip(&caps[3]).to_string();
    let n = name.chars().count();
    if n >= 2 && name.starts_with('"') && name.ends_with('"') {
        name = name[1..name.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
    }
    (attrs, delim, Some(name))
}

fn last_segment(name: &str, delim: Option<&str>) -> String {
    // `re.split(re.escape(delim), name) if delim else re.split(r"[./]", name)`
    let parts: Vec<&str> = match delim {
        Some(d) if !d.is_empty() => name.split(d).collect(),
        _ => name.split(['.', '/']).collect(),
    };
    let last = parts.last().copied().unwrap_or(name);
    granary_compat::text::pystr::lower(granary_compat::text::pystr::strip(last))
}

/// `resolve_folders(imap)`: each standard role -> the server's mailbox name. INBOX always present.
pub async fn resolve_folders(imap: &mut Imap4) -> Folders {
    match imap.list("\"\"", "*").await {
        Ok(r) => folders_from_list(&r.lines),
        Err(_) => Folders(vec![("INBOX".to_string(), "INBOX".to_string())]),
    }
}

/// The mapping `resolve_folders` builds from the LIST response lines.
pub fn folders_from_list(lines: &[Vec<u8>]) -> Folders {
    let mut mapping = vec![("INBOX".to_string(), "INBOX".to_string())];
    let mut parsed: Vec<(BTreeSet<String>, Option<String>, String)> = Vec::new();
    for line in lines {
        let (attrs, delim, name) = parse_list_line(line);
        if let Some(name) = name {
            if !name.is_empty() && name.to_uppercase() != "INBOX" && !attrs.contains("\\noselect") {
                parsed.push((attrs, delim, name));
            }
        }
    }
    for (role, special) in ROLE_SPECIAL {
        let names = role_names(role);
        let mut found = parsed
            .iter()
            .find(|(at, _, _)| at.contains(special))
            .map(|(_, _, n)| n.clone());
        if found.is_none() {
            found = parsed
                .iter()
                .find(|(_, _, n)| names.contains(&granary_compat::text::pystr::lower(n).as_str()))
                .map(|(_, _, n)| n.clone())
                .or_else(|| {
                    parsed
                        .iter()
                        .find(|(_, d, n)| names.contains(&last_segment(n, d.as_deref()).as_str()))
                        .map(|(_, _, n)| n.clone())
                });
        }
        if let Some(f) = found {
            mapping.push((role.to_string(), f));
        }
    }
    Folders(mapping)
}

/// `select(imap, mailbox)`: SELECT with the name quoted when needed.
pub async fn select(imap: &mut Imap4, mailbox: &str) -> Result<Response, ImapError> {
    imap.select(&quote_mailbox(mailbox)).await
}

/// `parse_uids(data)`: the UIDs of a UID SEARCH response (its first line).
pub fn parse_uids(data: &[Vec<u8>]) -> Vec<i64> {
    let raw: &[u8] = data.first().map(|v| v.as_slice()).unwrap_or(b"");
    // raw.decode() (strict); a non-UTF-8 line raises in Python - IMAP SEARCH lines are ASCII.
    let text = String::from_utf8_lossy(raw);
    granary_compat::text::pystr::split(&text)
        .into_iter()
        .filter(|u| !u.is_empty() && u.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|u| u.parse::<i64>().ok())
        .collect()
}

static FLAGS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"FLAGS \(([^)]*)\)").unwrap());
static UID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"UID (\d+)").unwrap());

/// `seen_in(fetch_lines)`: whether a FETCH response carries \Seen in its FLAGS.
pub fn seen_in(fetch_lines: &[Vec<u8>]) -> bool {
    for it in fetch_lines {
        let s = decode_ignore(it);
        if let Some(m) = FLAGS_RE.captures(&s) {
            return m[1].contains("\\Seen");
        }
    }
    false
}

/// `_uid_set(uids)`: compact UID set, 1,2,3,7 -> 1:3,7.
pub fn _uid_set(uids: &[i64]) -> String {
    let set: BTreeSet<i64> = uids.iter().copied().collect();
    let mut out: Vec<String> = Vec::new();
    let mut start: Option<i64> = None;
    let mut prev = 0i64;
    for u in set {
        match start {
            None => {
                start = Some(u);
                prev = u;
            }
            Some(s) => {
                if u == prev + 1 {
                    prev = u;
                } else {
                    out.push(if s != prev {
                        format!("{s}:{prev}")
                    } else {
                        s.to_string()
                    });
                    start = Some(u);
                    prev = u;
                }
            }
        }
    }
    if let Some(s) = start {
        out.push(if s != prev {
            format!("{s}:{prev}")
        } else {
            s.to_string()
        });
    }
    out.join(",")
}

/// `fetch_seen(imap, uids)`: UID -> read? for the given UIDs in the selected mailbox. UIDs the server no
/// longer has are absent; a failed batch is skipped (logged).
pub async fn fetch_seen(imap: &mut Imap4, uids: &[i64]) -> BTreeMap<i64, bool> {
    let mut out = BTreeMap::new();
    let set: BTreeSet<i64> = uids.iter().copied().filter(|&u| u != 0).collect();
    let sorted: Vec<i64> = set.into_iter().collect();
    for chunk in sorted.chunks(FLAG_BATCH) {
        let resp = match imap.uid("fetch", &[&_uid_set(chunk), "(UID FLAGS)"]).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %truncate(&e.to_string(), 200), "imap.fetch_flags_failed");
                continue;
            }
        };
        for line in &resp.lines {
            let s = decode_ignore(line);
            let um = UID_RE.captures(&s);
            let fm = FLAGS_RE.captures(&s);
            if let (Some(um), Some(fm)) = (um, fm) {
                if let Ok(uid) = um[1].parse::<i64>() {
                    out.insert(uid, fm[1].contains("\\Seen"));
                }
            }
        }
    }
    out
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `set_seen(imap, uids, seen)`: add or remove \Seen on messages in the selected mailbox.
pub async fn set_seen(imap: &mut Imap4, uids: &[i64], seen: bool) -> Result<(), ImapError> {
    let uids: Vec<i64> = uids.iter().copied().filter(|&u| u != 0).collect();
    if !uids.is_empty() {
        imap.uid(
            "store",
            &[
                &_uid_set(&uids),
                if seen { "+FLAGS" } else { "-FLAGS" },
                "(\\Seen)",
            ],
        )
        .await?;
    }
    Ok(())
}

/// `append_sent(imap, raw)`: save a copy of a sent message into the server's Sent folder, marked read.
/// Best-effort: false (logged) when the server has no Sent folder or refuses.
pub async fn append_sent(imap: &mut Imap4, raw: &[u8]) -> bool {
    let mailbox = match resolve_folders(imap).await.get("SENT") {
        Some(m) => m.to_string(),
        None => {
            tracing::info!(
                reason = "no Sent folder on the server",
                "imap.append_sent_skipped"
            );
            return false;
        }
    };
    let resp = match imap
        .append(raw, &quote_mailbox(&mailbox), Some("(\\Seen)"))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %truncate(&e.to_string(), 200), "imap.append_sent_failed");
            return false;
        }
    };
    let ok = resp.result == "OK";
    if !ok {
        let shown = format!("{:?}", resp.lines);
        tracing::warn!(response = %truncate(&shown, 200), "imap.append_sent_refused");
    }
    ok
}

/// Errors of [`connect`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectError {
    /// PermissionError("IMAP login failed: check the username and password.")
    Login(String),
    /// Anything else aioimaplib raised.
    Imap(ImapError),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Login(m) => f.write_str(m),
            ConnectError::Imap(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

pub const LOGIN_FAILED: &str = "IMAP login failed: check the username and password.";

/// `connect(host, port, use_ssl, user, password)`: a logged-in connection, or a readable reason.
pub async fn connect(
    host: &str,
    port: i64,
    use_ssl: bool,
    user: &str,
    password: &str,
) -> Result<Imap4, ConnectError> {
    let port = u16::try_from(port).map_err(|_| {
        // asyncio: OverflowError/OSError inside the connection task -> the hello wait expires.
        ConnectError::Imap(ImapError::Timeout)
    })?;
    let mut imap = Imap4::open(host, port, use_ssl)
        .await
        .map_err(ConnectError::Imap)?;
    // aioimaplib does not raise on a refused LOGIN (NO/BAD): check the reply, close the connection and
    // say so, instead of letting the next command fail with "illegal in state NONAUTH".
    let resp = match imap.login(user, password).await {
        Ok(r) => r,
        Err(e) => {
            logout(&mut imap).await;
            return Err(ConnectError::Imap(e));
        }
    };
    if resp.result != "OK" {
        logout(&mut imap).await;
        return Err(ConnectError::Login(LOGIN_FAILED.into()));
    }
    Ok(imap)
}

/// `logout(imap)`: never fails.
pub async fn logout(imap: &mut Imap4) {
    let _ = imap.logout().await;
}

/// Errors of the trash functions: TrashError (the server refused; nothing was changed in the ERP) or
/// what aioimaplib raised on the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrashErr {
    Trash(String),
    Imap(ImapError),
}

impl std::fmt::Display for TrashErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrashErr::Trash(m) => f.write_str(m),
            TrashErr::Imap(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TrashErr {}

impl From<ImapError> for TrashErr {
    fn from(e: ImapError) -> TrashErr {
        TrashErr::Imap(e)
    }
}

async fn _expunge(imap: &mut Imap4, uid_set: &str) -> Result<(), ImapError> {
    // UID EXPUNGE removes only these messages; plain EXPUNGE would also purge anything another client had
    // marked deleted but not yet expunged.
    if imap.has_capability("UIDPLUS") {
        imap.uid("expunge", &[uid_set]).await?;
    } else {
        imap.expunge().await?;
    }
    Ok(())
}

/// `move_to_trash(imap, uids)`: move messages from the selected mailbox to Trash, like a mail client's
/// Delete key. true when they went to Trash; a server with no Trash folder gets them flagged \Deleted (not
/// expunged) and false is returned. TrashError when the server refuses.
pub async fn move_to_trash(imap: &mut Imap4, uids: &[i64]) -> Result<bool, TrashErr> {
    let uids: Vec<i64> = uids.iter().copied().filter(|&u| u != 0).collect();
    if uids.is_empty() {
        return Ok(true);
    }
    let uid_set = _uid_set(&uids);
    let trash = resolve_folders(imap)
        .await
        .get("TRASH")
        .map(|s| s.to_string());
    let trash = match trash {
        None => {
            let resp = imap
                .uid("store", &[&uid_set, "+FLAGS", "(\\Deleted)"])
                .await?;
            if !resp.ok() {
                return Err(TrashErr::Trash(
                    "The mail server refused to mark the message deleted.".into(),
                ));
            }
            return Ok(false);
        }
        Some(t) => t,
    };
    let target = quote_mailbox(&trash);
    if imap.has_capability("MOVE") {
        let resp = imap.uid("move", &[&uid_set, &target]).await?;
        if !resp.ok() {
            return Err(TrashErr::Trash(
                "The mail server refused to move the message to Trash.".into(),
            ));
        }
        return Ok(true);
    }
    let resp = imap.uid("copy", &[&uid_set, &target]).await?;
    if !resp.ok() {
        return Err(TrashErr::Trash(
            "The mail server refused to copy the message to Trash.".into(),
        ));
    }
    imap.uid("store", &[&uid_set, "+FLAGS", "(\\Deleted)"])
        .await?;
    _expunge(imap, &uid_set).await?;
    Ok(true)
}

/// `delete_forever(imap, uids)`: permanently remove messages from the selected mailbox.
pub async fn delete_forever(imap: &mut Imap4, uids: &[i64]) -> Result<(), TrashErr> {
    let uids: Vec<i64> = uids.iter().copied().filter(|&u| u != 0).collect();
    if uids.is_empty() {
        return Ok(());
    }
    let uid_set = _uid_set(&uids);
    let resp = imap
        .uid("store", &[&uid_set, "+FLAGS", "(\\Deleted)"])
        .await?;
    if !resp.ok() {
        return Err(TrashErr::Trash(
            "The mail server refused to delete the message.".into(),
        ));
    }
    _expunge(imap, &uid_set).await?;
    Ok(())
}
