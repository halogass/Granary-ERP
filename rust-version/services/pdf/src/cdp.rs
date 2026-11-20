//! A small Chrome DevTools Protocol client: launch Chromium the way Puppeteer 21 does, talk to it over its
//! WebSocket (flattened sessions), and drive pages with the same commands Puppeteer sends for `newPage`,
//! request interception, `setContent` + `waitUntil`, `evaluate`, `pdf`, `screenshot` and `close`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio_tungstenite::tungstenite::Message;

/// An error as the Node service would surface it: `err.message`, or None when JavaScript threw a value
/// that has no message (the route then answers `{}`).
#[derive(Debug, Clone, PartialEq)]
pub struct JsError(pub Option<String>);

impl JsError {
    pub fn msg(m: impl Into<String>) -> Self {
        JsError(Some(m.into()))
    }
}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_deref().unwrap_or("undefined"))
    }
}

type Pending = HashMap<u64, (String, oneshot::Sender<Result<Value, JsError>>)>;
type Handler = Arc<dyn Fn(&Connection, &str, &Value) + Send + Sync>;

pub struct Connection {
    out: mpsc::UnboundedSender<String>,
    next_id: AtomicU64,
    pending: Mutex<Pending>,
    handlers: Mutex<HashMap<String, Handler>>,
    closed: AtomicBool,
    closed_notify: Notify,
}

impl Connection {
    async fn connect(ws_url: &str) -> Result<Arc<Self>, String> {
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(None)
            .max_frame_size(None);
        let (ws, _) = tokio_tungstenite::connect_async_with_config(ws_url, Some(config), true)
            .await
            .map_err(|e| format!("failed to connect to {ws_url}: {e}"))?;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let conn = Arc::new(Connection {
            out: tx,
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            handlers: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            closed_notify: Notify::new(),
        });
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                if sink.send(Message::text(m)).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let reader = Arc::downgrade(&conn);
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let Ok(msg) = msg else { break };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Some(conn) = reader.upgrade() else { break };
                conn.dispatch(&text);
            }
            if let Some(conn) = reader.upgrade() {
                conn.on_close();
            }
        });
        Ok(conn)
    }

    fn dispatch(&self, text: &str) {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return;
        };
        if let Some(id) = v.get("id").and_then(Value::as_u64) {
            let entry = self.pending.lock().ok().and_then(|mut p| p.remove(&id));
            if let Some((method, tx)) = entry {
                let res = match v.get("error") {
                    Some(err) => {
                        let mut m = err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if let Some(data) = err.get("data") {
                            m.push(' ');
                            m.push_str(
                                data.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| data.to_string())
                                    .as_str(),
                            );
                        }
                        Err(JsError::msg(format!("Protocol error ({method}): {m}")))
                    }
                    None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(res);
            }
            return;
        }
        let Some(method) = v.get("method").and_then(Value::as_str) else {
            return;
        };
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        let session = v.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let handler = self
            .handlers
            .lock()
            .ok()
            .and_then(|h| h.get(session).cloned());
        if let Some(h) = handler {
            h(self, method, &params);
        }
    }

    fn on_close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let pending: Vec<_> = self
            .pending
            .lock()
            .map(|mut p| p.drain().collect())
            .unwrap_or_default();
        for (_, (method, tx)) in pending {
            let _ = tx.send(Err(JsError::msg(format!(
                "Protocol error ({method}): Target closed"
            ))));
        }
        let handlers: Vec<Handler> = self
            .handlers
            .lock()
            .map(|h| h.values().cloned().collect())
            .unwrap_or_default();
        for h in handlers {
            h(self, "__disconnected", &Value::Null);
        }
        self.closed_notify.notify_waiters();
    }

    pub fn is_connected(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    fn message(&self, session: Option<&str>, method: &str, params: Value) -> (u64, String) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = Value::String(s.to_string());
        }
        (id, msg.to_string())
    }

    /// Send a command and wait for its result.
    pub async fn send(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, JsError> {
        if !self.is_connected() {
            return Err(JsError::msg(format!(
                "Protocol error ({method}): Target closed"
            )));
        }
        let (id, text) = self.message(session, method, params);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            p.insert(id, (method.to_string(), tx));
        }
        if self.out.send(text).is_err() {
            return Err(JsError::msg(format!(
                "Protocol error ({method}): Target closed"
            )));
        }
        rx.await.unwrap_or_else(|_| {
            Err(JsError::msg(format!(
                "Protocol error ({method}): Target closed"
            )))
        })
    }

    /// Send a command without waiting (event handlers answering Fetch.requestPaused).
    pub fn send_nowait(&self, session: &str, method: &str, params: Value) {
        let (_, text) = self.message(Some(session), method, params);
        let _ = self.out.send(text);
    }

    fn set_handler(&self, session: &str, h: Handler) {
        if let Ok(mut m) = self.handlers.lock() {
            m.insert(session.to_string(), h);
        }
    }

    fn remove_handler(&self, session: &str) {
        if let Ok(mut m) = self.handlers.lock() {
            m.remove(session);
        }
    }
}

/// Puppeteer 21's `ChromeLauncher.defaultArgs` with `headless` (true or "new").
pub fn puppeteer_default_args(headless_new: bool) -> Vec<String> {
    let mut a: Vec<String> = [
        "--allow-pre-commit-input",
        "--disable-background-networking",
        "--disable-background-timer-throttling",
        "--disable-backgrounding-occluded-windows",
        "--disable-breakpad",
        "--disable-client-side-phishing-detection",
        "--disable-component-extensions-with-background-pages",
        "--disable-component-update",
        "--disable-default-apps",
        "--disable-dev-shm-usage",
        "--disable-extensions",
        "--disable-field-trial-config",
        "--disable-hang-monitor",
        "--disable-infobars",
        "--disable-ipc-flooding-protection",
        "--disable-popup-blocking",
        "--disable-prompt-on-repost",
        "--disable-renderer-backgrounding",
        "--disable-search-engine-choice-screen",
        "--disable-sync",
        "--enable-automation",
        "--export-tagged-pdf",
        "--generate-pdf-document-outline",
        "--force-color-profile=srgb",
        "--metrics-recording-only",
        "--no-first-run",
        "--password-store=basic",
        "--use-mock-keychain",
        "--disable-features=Translate,AcceptCHFrame,MediaRouter,OptimizationHints,ProcessPerSiteUpToMainFrameThreshold",
        "--enable-features=NetworkServiceInProcess2",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.push(if headless_new {
        "--headless=new".into()
    } else {
        "--headless".into()
    });
    a.push("--hide-scrollbars".into());
    a.push("--mute-audio".into());
    a.push("about:blank".into());
    a
}

/// The Chromium binary: PUPPETEER_EXECUTABLE_PATH (the Node service's variable), else the usual paths.
pub fn executable() -> String {
    if let Ok(p) = std::env::var("PUPPETEER_EXECUTABLE_PATH") {
        if !p.is_empty() {
            return p;
        }
    }
    for p in [
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
    ] {
        if std::path::Path::new(p).exists() {
            return p.to_string();
        }
    }
    "chromium".to_string()
}

pub struct Browser {
    pub conn: Arc<Connection>,
    child: Mutex<Option<Child>>,
    pid: Option<u32>,
    profile: PathBuf,
}

fn temp_profile() -> std::io::Result<PathBuf> {
    use rand::Rng;
    let base = std::env::temp_dir();
    for _ in 0..16 {
        let suffix: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(6)
            .map(char::from)
            .collect();
        let p = base.join(format!("puppeteer_dev_chrome_profile-{suffix}"));
        if std::fs::create_dir(&p).is_ok() {
            return Ok(p);
        }
    }
    Err(std::io::Error::other(
        "could not create a profile directory",
    ))
}

impl Browser {
    /// `puppeteer.launch({ headless, executablePath, args })`.
    pub async fn launch(headless_new: bool, user_args: &[&str]) -> Result<Browser, String> {
        let exe = executable();
        let profile = temp_profile().map_err(|e| e.to_string())?;
        let mut args = puppeteer_default_args(headless_new);
        args.extend(user_args.iter().map(|s| s.to_string()));
        args.push("--remote-debugging-port=0".into());
        args.push(format!("--user-data-dir={}", profile.display()));
        let mut child = match Command::new(&exe)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&profile);
                return Err(format!(
                    "Failed to launch the browser process! spawn {exe} {e}"
                ));
            }
        };
        let pid = child.id();
        let stderr = child.stderr.take();
        let (url_tx, url_rx) = oneshot::channel::<Result<String, String>>();
        tokio::spawn(async move {
            let Some(stderr) = stderr else { return };
            let mut lines = BufReader::new(stderr).lines();
            let mut seen = String::new();
            let mut url_tx = Some(url_tx);
            // Keep reading after the URL so Chromium never blocks on a full stderr pipe.
            while let Ok(Some(line)) = lines.next_line().await {
                if url_tx.is_none() {
                    continue;
                }
                if let Some(i) = line.find("DevTools listening on ws://") {
                    let url = line[i + "DevTools listening on ".len()..]
                        .trim()
                        .to_string();
                    if let Some(tx) = url_tx.take() {
                        let _ = tx.send(Ok(url));
                    }
                    continue;
                }
                seen.push_str(&line);
                seen.push('\n');
            }
            if let Some(tx) = url_tx.take() {
                let _ = tx.send(Err(format!(
                    "Failed to launch the browser process!\n{seen}\n\nTROUBLESHOOTING: https://pptr.dev/troubleshooting\n"
                )));
            }
        });
        let url = match tokio::time::timeout(Duration::from_millis(30_000), url_rx).await {
            Ok(Ok(Ok(u))) => u,
            Ok(Ok(Err(e))) => {
                let _ = child.kill().await;
                let _ = std::fs::remove_dir_all(&profile);
                return Err(e);
            }
            Ok(Err(_)) | Err(_) => {
                let _ = child.kill().await;
                let _ = std::fs::remove_dir_all(&profile);
                return Err("Timed out after 30000 ms while waiting for the WS endpoint URL to appear in stdout!".into());
            }
        };
        let conn = match Connection::connect(&url).await {
            Ok(c) => c,
            Err(e) => {
                let _ = child.kill().await;
                let _ = std::fs::remove_dir_all(&profile);
                return Err(e);
            }
        };
        Ok(Browser {
            conn,
            child: Mutex::new(Some(child)),
            pid,
            profile,
        })
    }

    pub fn is_connected(&self) -> bool {
        self.conn.is_connected()
    }

    /// SIGKILL the browser process (a render that hangs).
    pub fn kill(&self) {
        if let Some(pid) = self.pid {
            // SAFETY: kill(2) on our own child's pid; no memory is touched.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }

    /// `browser.close()`: Browser.close, wait for the process, remove the profile.
    pub async fn close(&self) {
        if self.conn.is_connected() {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                self.conn.send(None, "Browser.close", json!({})),
            )
            .await;
        }
        let child = self.child.lock().ok().and_then(|mut c| c.take());
        if let Some(mut child) = child {
            if tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
            }
        }
        let _ = std::fs::remove_dir_all(&self.profile);
    }

    /// `browser.newPage()`: a fresh about:blank target, with Puppeteer's default 800x600 viewport.
    pub async fn new_page(&self) -> Result<Page, JsError> {
        let conn = self.conn.clone();
        let r = conn
            .send(None, "Target.createTarget", json!({"url": "about:blank"}))
            .await?;
        let target_id = r["targetId"].as_str().unwrap_or_default().to_string();
        let r = conn
            .send(
                None,
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
            )
            .await?;
        let session = r["sessionId"].as_str().unwrap_or_default().to_string();
        let state = Arc::new(PageState {
            session: session.clone(),
            ..PageState::default()
        });
        let page = Page {
            conn: conn.clone(),
            target_id,
            session: session.clone(),
            state: state.clone(),
        };
        let st = state.clone();
        conn.set_handler(
            &session,
            Arc::new(move |c: &Connection, method: &str, params: &Value| {
                st.on_event(c, method, params)
            }),
        );
        let s = Some(session.as_str());
        conn.send(s, "Page.enable", json!({})).await?;
        let tree = conn.send(s, "Page.getFrameTree", json!({})).await?;
        state.load_tree(&tree["frameTree"], None);
        conn.send(
            s,
            "Page.setLifecycleEventsEnabled",
            json!({"enabled": true}),
        )
        .await?;
        conn.send(s, "Runtime.enable", json!({})).await?;
        conn.send(s, "Network.enable", json!({})).await?;
        page.set_viewport(&json!(800), &json!(600)).await?;
        Ok(page)
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            if let Some(child) = c.as_mut() {
                let _ = child.start_kill();
            }
        }
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

type Allow = Box<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Default)]
struct Frames {
    main: String,
    parent: HashMap<String, Option<String>>,
    events: HashMap<String, HashSet<String>>,
    detached: bool,
    disconnected: bool,
}

#[derive(Default)]
pub struct PageState {
    session: String,
    frames: Mutex<Frames>,
    notify: Notify,
    allow: Mutex<Option<Allow>>,
}

impl PageState {
    fn load_tree(&self, node: &Value, parent: Option<&str>) {
        let id = node["frame"]["id"].as_str().unwrap_or_default().to_string();
        if let Ok(mut f) = self.frames.lock() {
            if parent.is_none() {
                f.main = id.clone();
            }
            f.parent.insert(id.clone(), parent.map(str::to_string));
            f.events.entry(id.clone()).or_default();
        }
        if let Some(children) = node["childFrames"].as_array() {
            for c in children {
                self.load_tree(c, Some(&id));
            }
        }
    }

    fn on_event(&self, conn: &Connection, method: &str, p: &Value) {
        match method {
            "Page.lifecycleEvent" => {
                let id = p["frameId"].as_str().unwrap_or_default().to_string();
                let name = p["name"].as_str().unwrap_or_default().to_string();
                if let Ok(mut f) = self.frames.lock() {
                    let set = f.events.entry(id).or_default();
                    if name == "init" {
                        set.clear();
                    }
                    set.insert(name);
                }
                self.notify.notify_waiters();
            }
            "Page.frameAttached" => {
                let id = p["frameId"].as_str().unwrap_or_default().to_string();
                let parent = p["parentFrameId"].as_str().map(str::to_string);
                if let Ok(mut f) = self.frames.lock() {
                    f.parent.insert(id.clone(), parent);
                    f.events.entry(id).or_default();
                }
                self.notify.notify_waiters();
            }
            "Page.frameDetached" => {
                let id = p["frameId"].as_str().unwrap_or_default().to_string();
                if let Ok(mut f) = self.frames.lock() {
                    if id == f.main {
                        f.detached = true;
                    }
                    f.parent.remove(&id);
                    f.events.remove(&id);
                }
                self.notify.notify_waiters();
            }
            "Fetch.requestPaused" => {
                let id = p["requestId"].as_str().unwrap_or_default().to_string();
                let url = p["request"]["url"].as_str().unwrap_or_default().to_string()
                    + p["request"]["urlFragment"].as_str().unwrap_or("");
                let ok = self
                    .allow
                    .lock()
                    .ok()
                    .and_then(|a| a.as_ref().map(|f| f(&url)))
                    .unwrap_or(true);
                let session = &self.session;
                if ok {
                    conn.send_nowait(session, "Fetch.continueRequest", json!({"requestId": id}));
                } else {
                    conn.send_nowait(
                        session,
                        "Fetch.failRequest",
                        json!({"requestId": id, "errorReason": "BlockedByClient"}),
                    );
                }
            }
            "Fetch.authRequired" => {
                let id = p["requestId"].as_str().unwrap_or_default().to_string();
                conn.send_nowait(
                    &self.session,
                    "Fetch.continueWithAuth",
                    json!({"requestId": id, "authChallengeResponse": {"response": "Default"}}),
                );
            }
            "__disconnected" => {
                if let Ok(mut f) = self.frames.lock() {
                    f.disconnected = true;
                }
                self.notify.notify_waiters();
            }
            _ => {}
        }
    }

    /// Every frame of the tree under the main frame has `event` since its last `init`.
    fn lifecycle_done(&self, event: &str) -> bool {
        let Ok(f) = self.frames.lock() else {
            return false;
        };
        fn check(f: &Frames, id: &str, event: &str) -> bool {
            if !f.events.get(id).is_some_and(|s| s.contains(event)) {
                return false;
            }
            f.parent
                .iter()
                .filter(|(_, p)| p.as_deref() == Some(id))
                .all(|(child, _)| check(f, child, event))
        }
        check(&f, &f.main, event)
    }
}

pub struct Page {
    conn: Arc<Connection>,
    target_id: String,
    session: String,
    state: Arc<PageState>,
}

/// puppeteer's createEvaluationError: the thrown error's message (or the thrown value).
fn evaluation_error(details: &Value) -> JsError {
    let exc = &details["exception"];
    if exc.is_null() {
        return JsError(details["text"].as_str().map(str::to_string));
    }
    let is_error = exc["type"] == "object" && exc["subtype"] == "error";
    if !is_error && exc.get("objectId").is_none() {
        // A thrown primitive: `err.message` is undefined.
        return JsError(None);
    }
    let desc = exc["description"].as_str().unwrap_or("");
    let mut lines: Vec<&str> = desc.split("\n    at ").collect();
    let frames = details["stackTrace"]["callFrames"]
        .as_array()
        .map_or(0, Vec::len);
    let size = frames.min(lines.len().saturating_sub(1));
    lines.truncate(lines.len() - size);
    let name = exc["className"].as_str().unwrap_or("");
    let mut message = lines.join("\n");
    if !name.is_empty() {
        if let Some(rest) = message.strip_prefix(&format!("{name}: ")) {
            message = rest.to_string();
        }
    }
    JsError::msg(message)
}

impl Page {
    fn s(&self) -> Option<&str> {
        Some(&self.session)
    }

    async fn send(&self, method: &str, params: Value) -> Result<Value, JsError> {
        let mut r = self.conn.send(self.s(), method, params).await;
        if let Err(JsError(Some(m))) = &mut r {
            if m.ends_with("Cannot find context with specified id")
                || m.ends_with("Inspected target navigated or closed")
            {
                *m = "Execution context was destroyed, most likely because of a navigation.".into();
            }
        }
        r
    }

    /// `page.setViewport({ width, height })` (values passed through as given, like Puppeteer).
    pub async fn set_viewport(&self, width: &Value, height: &Value) -> Result<(), JsError> {
        self.send(
            "Emulation.setDeviceMetricsOverride",
            json!({"mobile": false, "width": width, "height": height, "deviceScaleFactor": 1,
                   "screenOrientation": {"angle": 0, "type": "portraitPrimary"}}),
        )
        .await?;
        self.send(
            "Emulation.setTouchEmulationEnabled",
            json!({"enabled": false}),
        )
        .await?;
        Ok(())
    }

    /// `page.setJavaScriptEnabled(false)`.
    pub async fn disable_javascript(&self) -> Result<(), JsError> {
        self.send(
            "Emulation.setScriptExecutionDisabled",
            json!({"value": true}),
        )
        .await
        .map(|_| ())
    }

    /// `page.setRequestInterception(true)` + a `request` handler that continues or aborts ("blockedbyclient").
    pub async fn intercept(
        &self,
        allow: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Result<(), JsError> {
        if let Ok(mut a) = self.state.allow.lock() {
            *a = Some(Box::new(allow));
        }
        self.send("Network.setCacheDisabled", json!({"cacheDisabled": true}))
            .await?;
        self.send(
            "Fetch.enable",
            json!({"handleAuthRequests": true, "patterns": [{"urlPattern": "*"}]}),
        )
        .await?;
        Ok(())
    }

    /// The page's global object in the main world.
    async fn global(&self) -> Result<String, JsError> {
        let r = self
            .send("Runtime.evaluate", json!({"expression": "globalThis"}))
            .await?;
        r["result"]["objectId"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| JsError::msg("no global object"))
    }

    /// `page.evaluate(fn, ...args)` in the main world; returns the value (by value).
    pub async fn call(&self, function: &str, args: Vec<Value>) -> Result<Value, JsError> {
        let object_id = self.global().await?;
        let arguments: Vec<Value> = args.into_iter().map(|v| json!({"value": v})).collect();
        let r = self
            .send(
                "Runtime.callFunctionOn",
                json!({"functionDeclaration": function, "objectId": object_id, "arguments": arguments,
                       "returnByValue": true, "awaitPromise": true, "userGesture": true}),
            )
            .await;
        let r = match r {
            Err(JsError(Some(m)))
                if m.contains("Object couldn't be returned by value")
                    || m.contains("Object reference chain is too long") =>
            {
                return Ok(Value::Null)
            }
            other => other?,
        };
        if let Some(d) = r.get("exceptionDetails") {
            return Err(evaluation_error(d));
        }
        Ok(r["result"]["value"].clone())
    }

    /// `page.evaluate("expression")`.
    pub async fn eval(&self, expression: &str) -> Result<Value, JsError> {
        let r = self
            .send(
                "Runtime.evaluate",
                json!({"expression": expression, "returnByValue": true, "awaitPromise": true, "userGesture": true}),
            )
            .await?;
        if let Some(d) = r.get("exceptionDetails") {
            return Err(evaluation_error(d));
        }
        Ok(r["result"]["value"].clone())
    }

    /// `page.setContent(html, { waitUntil })`: document.open/write/close, then the lifecycle event on every
    /// frame (`networkIdle` for "networkidle0", `load` for the default), 30 s navigation timeout.
    pub async fn set_content(&self, html: Value, wait_event: &str) -> Result<(), JsError> {
        // 10 Nov 2026: document.write waits for a stylesheet or script that blocks parsing; never forever
        match tokio::time::timeout(
            Duration::from_millis(20_000),
            self.call(
                "html => { document.open(); document.write(html); document.close(); }",
                vec![html],
            ),
        )
        .await
        {
            Ok(r) => {
                r?;
            }
            Err(_) => {
                return Err(JsError::msg(
                    "Writing the HTML into the page timed out after 20000 ms",
                ))
            }
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(30_000);
        loop {
            let notified = self.state.notify.notified();
            {
                let f = self
                    .state
                    .frames
                    .lock()
                    .map_err(|_| JsError::msg("page state poisoned"))?;
                if f.disconnected {
                    return Err(JsError::msg(
                        "Navigation failed because browser has disconnected!",
                    ));
                }
                if f.detached {
                    return Err(JsError::msg("Navigating frame was detached"));
                }
            }
            if self.state.lifecycle_done(wait_event) {
                return Ok(());
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(JsError::msg("Navigation timeout of 30000 ms exceeded"));
            }
        }
    }

    /// `page.pdf(options)`: Page.printToPDF with Puppeteer's parameters, read back from the stream.
    pub async fn pdf(&self, params: Value) -> Result<Vec<u8>, JsError> {
        let work = async {
            let r = self.send("Page.printToPDF", params).await?;
            let handle = r["stream"]
                .as_str()
                .ok_or_else(|| JsError::msg("`stream` is missing from `Page.printToPDF"))?
                .to_string();
            let mut out = Vec::new();
            loop {
                let chunk = self.send("IO.read", json!({"handle": handle})).await?;
                let data = chunk["data"].as_str().unwrap_or("");
                if chunk["base64Encoded"].as_bool().unwrap_or(false) {
                    out.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .map_err(|e| JsError::msg(e.to_string()))?,
                    );
                } else {
                    out.extend_from_slice(data.as_bytes());
                }
                if chunk["eof"].as_bool().unwrap_or(false) {
                    break;
                }
            }
            let _ = self.send("IO.close", json!({"handle": handle})).await;
            Ok::<_, JsError>(out)
        };
        match tokio::time::timeout(Duration::from_millis(30_000), work).await {
            Ok(r) => r,
            Err(_) => Err(JsError::msg("Timed out after waiting 30000ms")),
        }
    }

    /// `page.addScriptTag({ content })`.
    pub async fn add_script(&self, content: &str) -> Result<(), JsError> {
        self.call(
            "(type, content) => { const s = document.createElement('script'); s.type = type; s.text = content; document.head.appendChild(s); }",
            vec![json!("text/javascript"), json!(content)],
        )
        .await
        .map(|_| ())
    }

    /// `(await page.$(selector)).screenshot({ type: "png" })` for the first element matching `selector`.
    pub async fn element_screenshot(
        &self,
        selector: &str,
        viewport: (f64, f64),
    ) -> Result<Vec<u8>, JsError> {
        let check = |v: &Value| -> Result<(f64, f64, f64, f64), JsError> {
            match v.as_str() {
                Some("null") => {
                    return Err(JsError::msg(
                        "Cannot read properties of null (reading 'screenshot')",
                    ))
                }
                Some(_) => {
                    return Err(JsError::msg(
                        "Node is either not visible or not an HTMLElement",
                    ))
                }
                None => {}
            }
            let g = |k: &str| v[k].as_f64().unwrap_or(0.0);
            let (x, y, w, h) = (g("x"), g("y"), g("width"), g("height"));
            if w == 0.0 {
                return Err(JsError::msg("Node has 0 width."));
            }
            if h == 0.0 {
                return Err(JsError::msg("Node has 0 height."));
            }
            Ok((x, y, w, h))
        };
        let (_, _, w, h) = check(&self.rect(selector).await?)?;
        let mut resized = false;
        if w > viewport.0 || h > viewport.1 {
            let nw = viewport.0.max(w.ceil());
            let nh = viewport.1.max(h.ceil());
            self.set_viewport(&json!(nw), &json!(nh)).await?;
            resized = true;
        }
        let result = async {
            let visible = self
                .call(
                    "sel => new Promise(resolve => { const e = document.querySelector(sel); const o = new IntersectionObserver(es => { resolve(es[0].intersectionRatio); o.disconnect(); }); o.observe(e); })",
                    vec![json!(selector)],
                )
                .await?;
            if visible.as_f64().unwrap_or(0.0) != 1.0 {
                let doc = self.send("DOM.getDocument", json!({"depth": 0})).await?;
                let node = self
                    .send("DOM.querySelector", json!({"nodeId": doc["root"]["nodeId"], "selector": selector}))
                    .await?;
                let _ = self.send("DOM.scrollIntoViewIfNeeded", json!({"nodeId": node["nodeId"]})).await;
            }
            let (x, y, w, h) = check(&self.rect(selector).await?)?;
            let page = self.call("() => [window.visualViewport.pageLeft, window.visualViewport.pageTop]", vec![]).await?;
            let (cx, cy) = (x + page[0].as_f64().unwrap_or(0.0), y + page[1].as_f64().unwrap_or(0.0));
            // roundRectangle(normalizeRectangle(clip))
            let rx = crate::jsval::js_round(cx);
            let ry = crate::jsval::js_round(cy);
            let rw = crate::jsval::js_round(w + cx - rx);
            let rh = crate::jsval::js_round(h + cy - ry);
            self.send("Page.bringToFront", json!({})).await?;
            let r = self
                .send(
                    "Page.captureScreenshot",
                    json!({"format": "png", "clip": {"x": rx, "y": ry, "width": rw, "height": rh, "scale": 1},
                           "captureBeyondViewport": true}),
                )
                .await?;
            base64::engine::general_purpose::STANDARD
                .decode(r["data"].as_str().unwrap_or(""))
                .map_err(|e| JsError::msg(e.to_string()))
        }
        .await;
        if resized {
            let _ = self
                .set_viewport(&json!(viewport.0), &json!(viewport.1))
                .await;
        }
        result
    }

    /// The element's bounding box (or why there is none), as Puppeteer's boundingBox reads it.
    async fn rect(&self, selector: &str) -> Result<Value, JsError> {
        self.call(
            "sel => { const e = document.querySelector(sel); if (!e) return 'null'; if (!(e instanceof Element)) return 'notel';
              if (e.getClientRects().length === 0) return 'hidden'; const r = e.getBoundingClientRect();
              return { x: r.x, y: r.y, width: r.width, height: r.height }; }",
            vec![json!(selector)],
        )
        .await
    }

    /// `page.close()`.
    pub async fn close(&self) {
        let _ = self
            .conn
            .send(
                None,
                "Target.closeTarget",
                json!({"targetId": self.target_id}),
            )
            .await;
        self.conn.remove_handler(&self.session);
    }
}
