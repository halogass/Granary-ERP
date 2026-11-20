//! The IMAP client the Python backend uses (aioimaplib 2.0.1), reproduced at the protocol level: the same
//! command lines, the same `Response(result, lines)` shape (untagged lines with "* " removed, async
//! commands' untagged name stripped, literals as their own line, the tagged text last), the same
//! capability bookkeeping and the same timeouts. services/imap_sync.py and the poller read those raw lines
//! (`_raw_payload` takes the largest chunk, `seen_in` greps FLAGS), so a typed IMAP library would not give
//! the same results; async-imap is therefore not used here.
//!
//! Timeouts: connecting (TCP and TLS), every read and every write are bounded by [`TIMEOUT`], so a
//! stalled mail server cannot hold a sync forever: UID commands and APPEND by inactivity, the others in
//! total. A connection that cannot be opened, or a greeting that is neither OK nor PREAUTH, surfaces when
//! the hello wait expires. A timeout is [`ImapError::Timeout`], whose text says what happened.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;

/// Connect, read and write timeout of every IMAP call. A constant, not a setting: no IMAP timeout setting
/// exists, and 10 s (aioimaplib's `TIMEOUT_SECONDS`) is ample for a healthy server.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// What a timeout says (the sync records it as the inbox's error and logs it).
pub const TIMEOUT_TEXT: &str =
    "The mail server did not answer within 10 seconds (connect, read or write timed out).";

/// aioimaplib's `Response` namedtuple.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub result: String,
    pub lines: Vec<Vec<u8>>,
}

impl Response {
    pub fn ok(&self) -> bool {
        self.result == "OK"
    }
}

/// What aioimaplib (or the socket under it) raises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImapError {
    /// The server did not connect, answer or take a write within [`TIMEOUT`].
    Timeout,
    /// aioimaplib.Abort(reason).
    Abort(String),
    /// aioimaplib.Error / an OS-level failure on an open connection.
    Error(String),
}

impl std::fmt::Display for ImapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImapError::Timeout => f.write_str(TIMEOUT_TEXT),
            ImapError::Abort(m) | ImapError::Error(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ImapError {}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    NonAuth,
    Auth,
    Selected,
    Logout,
}

/// A connection (`aioimaplib.IMAP4` / `IMAP4_SSL`).
pub struct Imap4 {
    stream: Box<dyn Stream>,
    buf: Vec<u8>,
    tagpre: String,
    tagnum: u64,
    state: State,
    capabilities: BTreeSet<String>,
    /// Test seam: capabilities hidden from `has_capability` (an old server without MOVE, ...).
    hidden: Vec<String>,
}

fn is_sync(name: &str) -> bool {
    matches!(
        name,
        "APPEND"
            | "AUTHENTICATE"
            | "CLOSE"
            | "COMPRESS"
            | "ENABLE"
            | "EXAMINE"
            | "IDLE"
            | "LOGIN"
            | "LOGOUT"
            | "MOVE"
            | "SELECT"
            | "SETACL"
            | "SETQUOTA"
            | "SUBSCRIBE"
            | "UNSUBSCRIBE"
            | "DELAY"
    )
}

/// `int2ap(random.randint(4096, 65535))`.
fn tag_prefix() -> String {
    let mut num: u32 = rand::random::<u32>() % (65535 - 4096 + 1) + 4096;
    let ap = b"ABCDEFGHIJKLMNOP";
    let mut s = String::new();
    while num > 0 {
        s.push(ap[(num % 16) as usize] as char);
        num /= 16;
    }
    s
}

/// `quoted(arg)`.
pub fn quoted(arg: &str) -> String {
    format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `tagged_status_response_re.match(line)`: `[A-Z0-9]+ ((OK)|(NO)|(BAD))`.
fn tagged_status(line: &[u8]) -> bool {
    let n = line
        .iter()
        .take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        .count();
    if n == 0 {
        return false;
    }
    let rest = &line[n..];
    rest.starts_with(b" OK") || rest.starts_with(b" NO") || rest.starts_with(b" BAD")
}

/// `literal_data_re.match(line)`: `.*\{(\d+)\}$`.
fn literal_size(line: &[u8]) -> Option<usize> {
    let body = line.strip_suffix(b"}")?;
    let open = body.iter().rposition(|&b| b == b'{')?;
    let digits = &body[open + 1..];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    // `.` does not cross a newline: a \n before the brace means no match.
    if line[..open].contains(&b'\n') {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// `message_data_re.match(line)`: `[0-9]+ ((FETCH)|(EXPUNGE))` -> the command name.
fn message_data(line: &[u8]) -> Option<&'static str> {
    let n = line.iter().take_while(|b| b.is_ascii_digit()).count();
    if n == 0 {
        return None;
    }
    let rest = &line[n..];
    if rest.starts_with(b" FETCH") {
        Some("FETCH")
    } else if rest.starts_with(b" EXPUNGE") {
        Some("EXPUNGE")
    } else {
        None
    }
}

fn replace_star_space(line: &[u8]) -> Vec<u8> {
    // line.replace(b'* ', b'')  (every occurrence, as aioimaplib does)
    let mut out = Vec::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if line[i] == b'*' && i + 1 < line.len() && line[i + 1] == b' ' {
            i += 2;
        } else {
            out.push(line[i]);
            i += 1;
        }
    }
    out
}

struct Pending {
    tag: String,
    name: String,
    resp_name: String,
    sync: bool,
    lines: Vec<Vec<u8>>,
    literal: Vec<bool>,
    literal_data: Option<Vec<u8>>,
}

impl Pending {
    fn append(&mut self, line: Vec<u8>, literal: bool) {
        self.lines.push(line);
        self.literal.push(literal);
    }

    /// FetchCommand.wait_data / Command.wait_data (after a literal has been read in full).
    fn wait_data(&self) -> bool {
        if self.name != "FETCH" {
            return false;
        }
        let mut last = 0;
        for (i, l) in self.lines.iter().enumerate() {
            if !self.literal[i] && is_fetch_data(l) {
                last = i;
            }
        }
        let joined: Vec<u8> = self.lines[last..]
            .iter()
            .zip(&self.literal[last..])
            .filter(|(_, lit)| !**lit)
            .flat_map(|(l, _)| l.iter().copied())
            .collect();
        joined.iter().filter(|&&b| b == b'(').count()
            != joined.iter().filter(|&&b| b == b')').count()
    }
}

fn is_fetch_data(line: &[u8]) -> bool {
    // rb'[0-9]+ FETCH \('
    let n = line.iter().take_while(|b| b.is_ascii_digit()).count();
    n > 0 && line[n..].starts_with(b" FETCH (")
}

#[derive(Clone, Copy)]
enum Limit {
    Total(Instant),
    Inactivity,
}

impl Imap4 {
    /// `IMAP4(host, port)` / `IMAP4_SSL(host, port)` followed by `wait_hello_from_server()`.
    pub async fn open(host: &str, port: u16, use_ssl: bool) -> Result<Imap4, ImapError> {
        let deadline = Instant::now() + TIMEOUT;
        let stream = match tokio::time::timeout_at(
            deadline,
            Self::connect_stream(host, port, use_ssl),
        )
        .await
        {
            Ok(Ok(s)) => s,
            _ => {
                // The connection task fails silently; the hello wait expires.
                tokio::time::sleep_until(deadline).await;
                return Err(ImapError::Timeout);
            }
        };
        let mut imap = Imap4 {
            stream,
            buf: Vec::new(),
            tagpre: tag_prefix(),
            tagnum: 0,
            state: State::NonAuth,
            capabilities: BTreeSet::new(),
            hidden: Vec::new(),
        };
        let hello = match tokio::time::timeout_at(deadline, imap.read_line()).await {
            Ok(Ok(l)) => l,
            _ => {
                tokio::time::sleep_until(deadline).await;
                return Err(ImapError::Timeout);
            }
        };
        if contains(&hello, b"PREAUTH") {
            imap.state = State::Auth;
        } else if contains(&hello, b"OK") {
            imap.state = State::NonAuth;
        } else {
            tokio::time::sleep_until(deadline).await;
            return Err(ImapError::Timeout);
        }
        // welcome() runs capability(); an error there leaves the hello waiter un-notified.
        let resp = match imap
            .execute_until("CAPABILITY", None, &[], None, Limit::Total(deadline))
            .await
        {
            Ok(r) => r,
            Err(_) => {
                tokio::time::sleep_until(deadline).await;
                return Err(ImapError::Timeout);
            }
        };
        let first = resp.lines.first().cloned().unwrap_or_default();
        let caps: Vec<String> = match std::str::from_utf8(&first) {
            Ok(s) => s.split_whitespace().map(|x| x.to_string()).collect(),
            Err(_) => {
                tokio::time::sleep_until(deadline).await;
                return Err(ImapError::Timeout);
            }
        };
        imap.capabilities = caps.iter().cloned().collect();
        if !caps
            .iter()
            .any(|c| matches!(c.to_uppercase().as_str(), "IMAP4REV1" | "IMAP4"))
        {
            tokio::time::sleep_until(deadline).await;
            return Err(ImapError::Timeout);
        }
        Ok(imap)
    }

    async fn connect_stream(
        host: &str,
        port: u16,
        use_ssl: bool,
    ) -> Result<Box<dyn Stream>, ImapError> {
        let tcp = TcpStream::connect((host, port))
            .await
            .map_err(|e| ImapError::Error(e.to_string()))?;
        if !use_ssl {
            return Ok(Box::new(tcp));
        }
        let connector = tls_connector()?;
        let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|e| ImapError::Error(e.to_string()))?;
        let tls = connector
            .connect(name, tcp)
            .await
            .map_err(|e| ImapError::Error(e.to_string()))?;
        Ok(Box::new(tls))
    }

    /// `has_capability(cap)`.
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.contains(cap) && !self.hidden.iter().any(|h| h == cap)
    }

    /// Test seam: make `has_capability(cap)` false, like a server that does not offer it.
    pub fn hide_capability(&mut self, cap: &str) {
        self.hidden.push(cap.to_string());
    }

    pub fn capabilities(&self) -> Vec<String> {
        self.capabilities.iter().cloned().collect()
    }

    fn new_tag(&mut self) -> String {
        let t = format!("{}{}", self.tagpre, self.tagnum);
        self.tagnum += 1;
        t
    }

    async fn read_more(&mut self) -> Result<(), ImapError> {
        let mut chunk = [0u8; 16384];
        let n = self
            .stream
            .read(&mut chunk)
            .await
            .map_err(|e| ImapError::Error(e.to_string()))?;
        if n == 0 {
            // aioimaplib waits on a closed connection until its timeout.
            std::future::pending::<()>().await;
        }
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(())
    }

    /// One CRLF-terminated line (without the CRLF).
    async fn read_line(&mut self) -> Result<Vec<u8>, ImapError> {
        loop {
            if let Some(i) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let line = self.buf[..i].to_vec();
                self.buf.drain(..i + 2);
                return Ok(line);
            }
            self.read_more().await?;
        }
    }

    async fn read_exact(&mut self, n: usize) -> Result<Vec<u8>, ImapError> {
        while self.buf.len() < n {
            self.read_more().await?;
        }
        let out = self.buf[..n].to_vec();
        self.buf.drain(..n);
        Ok(out)
    }

    fn check_state(&self, name: &str) -> Result<(), ImapError> {
        let ok = match name {
            "APPEND" | "LIST" | "SELECT" | "CREATE" | "DELETE" => {
                matches!(self.state, State::Auth | State::Selected)
            }
            "CAPABILITY" => matches!(self.state, State::NonAuth | State::Auth | State::Selected),
            "COPY" | "EXPUNGE" | "FETCH" | "MOVE" | "SEARCH" | "STORE" | "UID" => {
                self.state == State::Selected
            }
            "LOGIN" => self.state == State::NonAuth,
            _ => true, // LOGOUT: every state
        };
        if ok {
            Ok(())
        } else {
            Err(ImapError::Abort(format!(
                "command {} illegal in state {}",
                name,
                self.state_name()
            )))
        }
    }

    fn state_name(&self) -> &'static str {
        match self.state {
            State::NonAuth => "NONAUTH",
            State::Auth => "AUTH",
            State::Selected => "SELECTED",
            State::Logout => "LOGOUT",
        }
    }

    /// `execute(Command(name, tag, *args, prefix=...))`.
    async fn execute_until(
        &mut self,
        name: &str,
        prefix: Option<&str>,
        args: &[&str],
        literal_data: Option<Vec<u8>>,
        limit: Limit,
    ) -> Result<Response, ImapError> {
        self.check_state(name)?;
        let tag = self.new_tag();
        let mut line = format!("{tag} ");
        if let Some(p) = prefix {
            line.push_str(p);
            line.push(' ');
        }
        line.push_str(name);
        if !args.is_empty() {
            line.push(' ');
            line.push_str(&args.join(" "));
        }
        line.push_str("\r\n");
        let resp_name = if name == "STORE" {
            "FETCH".to_string()
        } else {
            name.to_string()
        };
        let mut cmd = Pending {
            tag,
            name: name.to_string(),
            resp_name,
            sync: is_sync(name),
            lines: Vec::new(),
            literal: Vec::new(),
            literal_data,
        };
        let mut deadline = match limit {
            Limit::Total(d) => d,
            Limit::Inactivity => Instant::now() + TIMEOUT,
        };
        match tokio::time::timeout_at(deadline, async {
            self.stream.write_all(line.as_bytes()).await?;
            self.stream.flush().await
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(ImapError::Error(e.to_string())),
            Err(_) => return Err(ImapError::Timeout),
        }

        // current_cmd: Some(true) = this command, Some(false) = a dummy 'NIL' command (literal holder).
        let mut current: Option<bool> = None;
        loop {
            let line = match tokio::time::timeout_at(deadline, self.read_line()).await {
                Ok(r) => r?,
                Err(_) => return Err(ImapError::Timeout),
            };
            if let Limit::Inactivity = limit {
                deadline = Instant::now() + TIMEOUT;
            }
            // _handle_line
            let mut target: Option<bool> = None;
            if line.is_empty() {
                // `if not line: return`
            } else if tagged_status(&line) {
                let sp = line.iter().position(|&b| b == b' ').unwrap_or(line.len());
                let tag = String::from_utf8_lossy(&line[..sp]).to_string();
                let response = if sp < line.len() {
                    &line[sp + 1..]
                } else {
                    &line[line.len()..]
                };
                if tag != cmd.tag {
                    let shown = String::from_utf8_lossy(response).to_string();
                    return Err(ImapError::Abort(if cmd.sync {
                        format!(
                            "unexpected tagged response with pending sync command ({} {}) response: b'{}'",
                            cmd.tag, cmd.name, shown
                        )
                    } else {
                        format!("unexpected tagged (b'{tag}') response: b'{shown}'")
                    }));
                }
                let sp2 = response
                    .iter()
                    .position(|&b| b == b' ')
                    .unwrap_or(response.len());
                let result = String::from_utf8_lossy(&response[..sp2]).to_string();
                let text = if sp2 < response.len() {
                    response[sp2 + 1..].to_vec()
                } else {
                    Vec::new()
                };
                cmd.append(text, false);
                return Ok(Response {
                    result,
                    lines: cmd.lines,
                });
            } else if let Some(c) = current {
                if c {
                    cmd.append(line.clone(), false);
                }
                target = Some(c);
            } else if line.first() == Some(&b'*') {
                let l2 = replace_star_space(&line);
                if cmd.sync {
                    cmd.append(l2, false);
                    target = Some(true);
                } else {
                    let (cmd_name, text) = match message_data(&l2) {
                        Some(n) => (n.to_string(), l2.clone()),
                        None => {
                            let sp = l2.iter().position(|&b| b == b' ').unwrap_or(l2.len());
                            let rest = if sp < l2.len() {
                                l2[sp + 1..].to_vec()
                            } else {
                                Vec::new()
                            };
                            (String::from_utf8_lossy(&l2[..sp]).to_string(), rest)
                        }
                    };
                    if cmd_name.to_uppercase() == cmd.resp_name {
                        cmd.append(text, false);
                        target = Some(true);
                    }
                }
            } else if line.first() == Some(&b'+') {
                if cmd.sync && cmd.name == "APPEND" {
                    if let Some(data) = cmd.literal_data.take() {
                        let w = tokio::time::timeout_at(deadline, async {
                            self.stream.write_all(&data).await?;
                            self.stream.write_all(b"\r\n").await?;
                            self.stream.flush().await
                        })
                        .await;
                        match w {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => return Err(ImapError::Error(e.to_string())),
                            Err(_) => return Err(ImapError::Timeout),
                        }
                    }
                } else if cmd.sync {
                    cmd.append(line.clone(), false);
                }
            }
            if let Some(size) = literal_size(&line) {
                let holder = target.unwrap_or(false);
                let data = match tokio::time::timeout_at(deadline, self.read_exact(size)).await {
                    Ok(r) => r?,
                    Err(_) => return Err(ImapError::Timeout),
                };
                if let Limit::Inactivity = limit {
                    deadline = Instant::now() + TIMEOUT;
                }
                if holder {
                    cmd.append(data, true);
                }
                current = Some(holder);
                continue;
            }
            current = if target == Some(true) && cmd.wait_data() {
                Some(true)
            } else {
                None
            };
        }
    }

    fn total() -> Limit {
        Limit::Total(Instant::now() + TIMEOUT)
    }

    /// `login(user, password)`: `LOGIN user "password"`.
    pub async fn login(&mut self, user: &str, password: &str) -> Result<Response, ImapError> {
        let pw = quoted(password);
        let resp = self
            .execute_until("LOGIN", None, &[user, &pw], None, Self::total())
            .await?;
        if resp.ok() {
            self.state = State::Auth;
            for line in &resp.lines {
                if contains(line, b"CAPABILITY") {
                    let s = std::str::from_utf8(line).map_err(|e| {
                        ImapError::Error(format!("'utf-8' codec can't decode bytes: {e}"))
                    })?;
                    for c in s.replace("CAPABILITY", "").split_whitespace() {
                        self.capabilities.insert(c.to_string());
                    }
                }
            }
        }
        Ok(resp)
    }

    /// `logout()`.
    pub async fn logout(&mut self) -> Result<Response, ImapError> {
        let resp = self
            .execute_until("LOGOUT", None, &[], None, Self::total())
            .await?;
        if resp.ok() {
            self.state = State::Logout;
        }
        Ok(resp)
    }

    /// `select(mailbox)` (the argument is sent as given).
    pub async fn select(&mut self, mailbox: &str) -> Result<Response, ImapError> {
        let resp = self
            .execute_until("SELECT", None, &[mailbox], None, Self::total())
            .await?;
        if resp.ok() {
            self.state = State::Selected;
        }
        Ok(resp)
    }

    /// `list(reference_name, mailbox_pattern)`.
    pub async fn list(&mut self, reference: &str, pattern: &str) -> Result<Response, ImapError> {
        self.execute_until("LIST", None, &[reference, pattern], None, Self::total())
            .await
    }

    /// `uid_search(*criteria)`: `UID SEARCH CHARSET utf-8 <criteria>`.
    pub async fn uid_search(&mut self, criteria: &[&str]) -> Result<Response, ImapError> {
        let mut args = vec!["CHARSET", "utf-8"];
        args.extend_from_slice(criteria);
        self.execute_until("SEARCH", Some("UID"), &args, None, Self::total())
            .await
    }

    /// `uid(command, *criteria)`.
    pub async fn uid(&mut self, command: &str, criteria: &[&str]) -> Result<Response, ImapError> {
        if self.state != State::Selected {
            return Err(ImapError::Abort(format!(
                "command UID illegal in state {}",
                self.state_name()
            )));
        }
        let up = command.to_uppercase();
        let limit = Limit::Inactivity;
        match up.as_str() {
            "FETCH" => {
                let args = [criteria.first().copied().unwrap_or(""), criteria.get(1).copied().unwrap_or("")];
                self.execute_until("FETCH", Some("UID"), &args, None, limit).await
            }
            "STORE" => self.execute_until("STORE", Some("UID"), criteria, None, limit).await,
            "COPY" => self.execute_until("COPY", Some("UID"), criteria, None, limit).await,
            "MOVE" => {
                if !self.has_capability("MOVE") {
                    return Err(ImapError::Abort("server has not MOVE capability".into()));
                }
                self.execute_until("MOVE", Some("UID"), criteria, None, Self::total()).await
            }
            "EXPUNGE" => {
                if !self.has_capability("UIDPLUS") {
                    let caps: Vec<String> = self.capabilities.iter().map(|c| format!("'{c}'")).collect();
                    return Err(ImapError::Abort(format!(
                        "EXPUNGE with uids is only valid with UIDPLUS capability. UIDPLUS not in ({{{}}})",
                        caps.join(", ")
                    )));
                }
                self.execute_until("EXPUNGE", Some("UID"), criteria, None, limit).await
            }
            other => Err(ImapError::Abort(format!(
                "command UID only possible with COPY, FETCH, EXPUNGE (w/UIDPLUS) or STORE (was {other})"
            ))),
        }
    }

    /// `create(mailbox_name)`.
    pub async fn create(&mut self, mailbox: &str) -> Result<Response, ImapError> {
        self.execute_until("CREATE", None, &[mailbox], None, Self::total())
            .await
    }

    /// `expunge()`.
    pub async fn expunge(&mut self) -> Result<Response, ImapError> {
        self.execute_until("EXPUNGE", None, &[], None, Self::total())
            .await
    }

    /// `append(message_bytes, mailbox=mailbox, flags=flags)`.
    pub async fn append(
        &mut self,
        message: &[u8],
        mailbox: &str,
        flags: Option<&str>,
    ) -> Result<Response, ImapError> {
        let mut args: Vec<String> = vec![mailbox.to_string()];
        if let Some(f) = flags {
            if f.starts_with('(') && f.ends_with(')') && !f.is_empty() {
                args.push(f.to_string());
            } else {
                args.push(format!("({f})"));
            }
        }
        args.push(format!("{{{}}}", message.len()));
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.execute_until(
            "APPEND",
            None,
            &refs,
            Some(message.to_vec()),
            Limit::Inactivity,
        )
        .await
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn tls_connector() -> Result<tokio_rustls::TlsConnector, ImapError> {
    use tokio_rustls::rustls;
    static CONFIG: once_cell::sync::OnceCell<Arc<rustls::ClientConfig>> =
        once_cell::sync::OnceCell::new();
    let cfg = CONFIG.get_or_try_init(|| -> Result<Arc<rustls::ClientConfig>, ImapError> {
        let mut roots = rustls::RootCertStore::empty();
        let certs = rustls_native_certs::load_native_certs();
        for c in certs.certs {
            let _ = roots.add(c);
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| ImapError::Error(e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Arc::new(cfg))
    })?;
    Ok(tokio_rustls::TlsConnector::from(cfg.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_says_what_happened() {
        assert_eq!(ImapError::Timeout.to_string(), TIMEOUT_TEXT);
        assert!(TIMEOUT_TEXT.contains(&TIMEOUT.as_secs().to_string()));
    }

    /// A server that accepts the connection and then says nothing: `open` gives up after [`TIMEOUT`]
    /// with a timeout instead of waiting forever.
    #[tokio::test]
    async fn a_silent_server_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hold = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(TIMEOUT * 3).await;
            drop(sock);
        });
        let started = std::time::Instant::now();
        let res = tokio::time::timeout(TIMEOUT * 2, Imap4::open("127.0.0.1", port, false))
            .await
            .expect("open must give up on its own");
        assert!(matches!(res, Err(ImapError::Timeout)));
        assert!(started.elapsed() >= TIMEOUT);
        hold.abort();
    }
}
