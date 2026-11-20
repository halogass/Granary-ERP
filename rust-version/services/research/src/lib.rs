//! Port of services/research (Python FastAPI, app/main.py): fetch one public web page and hand back
//! readable text plus its links.
//!
//!   GET  /health
//!   POST /fetch   {url, max_chars?} -> {url, final_url, title, markdown, links, engine}
//!
//! Every route but /health needs `Authorization: Bearer <SERVICE_TOKEN>`. Stateless: the backend
//! decides what to fetch and what it means; this service only fetches, safely.
//!
//! Moli renders the page (JavaScript included) and every request it makes goes through the local
//! filtering proxy (proxy.rs), so redirects, scripts and page-made requests to private or internal
//! addresses are refused just like the URL itself. When Moli fails or returns next to nothing, a
//! plain HTTP fetch through the same proxy is tried instead.
//!
//! Environment (as in Python): RESEARCH_SERVICE_TOKEN (or SERVICE_TOKEN), MOLI_BIN, PROXY_PORT,
//! RESEARCH_UA_SUFFIX, MOLI_TIMEOUT_S, MOLI_CONCURRENCY. The listen address is uvicorn's command line
//! in the Python image (0.0.0.0:3004); here HOST and PORT, with the same defaults.

pub mod clean;
mod entities_data;
pub mod guard;
pub mod htmlparse;
pub mod httpc;
pub mod httpxurl;
pub mod moli;
pub mod proxy;
pub mod pyip;
pub mod pyjson_loads;
pub mod pyurl;
pub mod pyutil;
pub mod robots;
pub mod validate;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use serde_json::{json, Value};

use guard::GuardError;
use robots::RobotFileParser;

pub const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
/// politeness: at most one request a second to the same host
pub const HOST_GAP: Duration = Duration::from_secs(1);
/// less than this from Moli -> try the plain fetch
pub const MIN_USEFUL_CHARS: usize = 200;
const ROBOTS_TTL: Duration = Duration::from_secs(3600);
const OPENAPI: &str = include_str!("../assets/openapi.json");

/// The module-level settings of main.py.
#[derive(Debug, Clone)]
pub struct Config {
    pub token: String,
    pub moli: String,
    pub proxy_port: u16,
    pub ua_suffix: String,
    pub moli_timeout_s: f64,
    pub moli_concurrency: usize,
}

impl Config {
    /// Read the environment the way main.py does at import time.
    pub fn from_env() -> Result<Config, String> {
        let get = |k: &str| std::env::var(k).ok();
        let token = get("RESEARCH_SERVICE_TOKEN")
            .filter(|v| !v.is_empty())
            .or_else(|| get("SERVICE_TOKEN").filter(|v| !v.is_empty()))
            .unwrap_or_default();
        let proxy_port = get("PROXY_PORT").unwrap_or_else(|| "8899".into());
        let proxy_port = pyutil::py_int(&proxy_port)?;
        let timeout = get("MOLI_TIMEOUT_S").unwrap_or_else(|| "25".into());
        let moli_timeout_s: f64 = timeout.trim().replace('_', "").parse().map_err(|_| {
            format!(
                "could not convert string to float: {}",
                pyutil::repr_str(&timeout)
            )
        })?;
        let conc = get("MOLI_CONCURRENCY").unwrap_or_else(|| "2".into());
        let moli_concurrency = pyutil::py_int(&conc)?;
        Ok(Config {
            token,
            moli: get("MOLI_BIN").unwrap_or_else(|| "/usr/local/bin/moli".into()),
            proxy_port: u16::try_from(proxy_port)
                .map_err(|_| "PROXY_PORT out of range".to_string())?,
            ua_suffix: get("RESEARCH_UA_SUFFIX").unwrap_or_else(|| "Granary-Research/1.0".into()),
            moli_timeout_s,
            moli_concurrency: moli_concurrency.max(0) as usize,
        })
    }

    /// Identify ourselves, but as a suffix on an ordinary browser string: many company sites answer
    /// a bare bot user agent with 403 while serving the same public page to any browser.
    pub fn user_agent(&self) -> String {
        format!(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36 {}",
            self.ua_suffix
        )
    }
}

/// The seams main.py's tests monkeypatch (`resolve_public`, `_allowed_by_robots`, `_polite`, `_moli`,
/// `_plain`); production uses [`Real`].
pub trait Hooks: Send + Sync + 'static {
    fn resolve_public<'a>(
        &'a self,
        app: &'a App,
        host: &'a str,
        port: u32,
    ) -> BoxFut<'a, Result<(), GuardError>>;
    fn allowed_by_robots<'a>(
        &'a self,
        app: &'a App,
        url: &'a str,
    ) -> BoxFut<'a, Result<bool, String>>;
    fn polite<'a>(&'a self, app: &'a App, host: &'a str) -> BoxFut<'a, ()>;
    fn moli<'a>(&'a self, app: &'a App, url: &'a str)
        -> BoxFut<'a, Result<Option<String>, String>>;
    fn plain<'a>(&'a self, app: &'a App, url: &'a str) -> BoxFut<'a, Result<Plain, PlainError>>;
}

pub type BoxFut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// An exception the Python route does not catch: Starlette answers 500 "Internal Server Error".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unhandled;

/// `_plain(url)` -> (markdown, title, final url).
#[derive(Debug, Clone)]
pub struct Plain {
    pub markdown: String,
    pub title: Option<String>,
    pub final_url: String,
}

/// What `_plain` raises.
#[derive(Debug, Clone)]
pub enum PlainError {
    /// guard.Blocked (the proxy refused): 422 blocked.
    Blocked(String),
    /// HTTPException(415, ...).
    NotAPage(String),
    /// Any other exception, as `str(exc)`.
    Other(String),
}

/// The production behaviour of every seam.
pub struct Real;

impl Hooks for Real {
    fn resolve_public<'a>(
        &'a self,
        _app: &'a App,
        host: &'a str,
        port: u32,
    ) -> BoxFut<'a, Result<(), GuardError>> {
        Box::pin(async move { guard::resolve_public(host, port).await.map(|_| ()) })
    }

    fn allowed_by_robots<'a>(
        &'a self,
        app: &'a App,
        url: &'a str,
    ) -> BoxFut<'a, Result<bool, String>> {
        Box::pin(app.allowed_by_robots(url))
    }

    fn polite<'a>(&'a self, app: &'a App, host: &'a str) -> BoxFut<'a, ()> {
        Box::pin(app.polite(host))
    }

    fn moli<'a>(
        &'a self,
        app: &'a App,
        url: &'a str,
    ) -> BoxFut<'a, Result<Option<String>, String>> {
        Box::pin(app.moli(url))
    }

    fn plain<'a>(&'a self, app: &'a App, url: &'a str) -> BoxFut<'a, Result<Plain, PlainError>> {
        Box::pin(app.plain(url))
    }
}

type HostSlot = Arc<tokio::sync::Mutex<Option<Instant>>>;

/// The running service: settings, the proxy address and main.py's module-level state.
pub struct App {
    pub config: Config,
    pub proxy_addr: SocketAddr,
    hooks: Box<dyn Hooks>,
    moli_slots: tokio::sync::Semaphore,
    hosts: Mutex<HashMap<String, HostSlot>>,
    robots: Mutex<HashMap<String, (Instant, Option<Arc<RobotFileParser>>)>>,
}

impl App {
    pub fn new(config: Config, proxy_addr: SocketAddr, hooks: Box<dyn Hooks>) -> App {
        let slots = config.moli_concurrency;
        App {
            config,
            proxy_addr,
            hooks,
            moli_slots: tokio::sync::Semaphore::new(slots),
            hosts: Mutex::new(HashMap::new()),
            robots: Mutex::new(HashMap::new()),
        }
    }

    pub fn proxy_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.proxy_addr.port())
    }

    fn client(&self) -> httpc::Client {
        // Through the filtering proxy too: the fallback must not be a way around the guard.
        httpc::Client::new(self.proxy_addr, &self.config.user_agent())
    }

    /// `_allowed_by_robots(url)`: robots.txt says no -> we don't fetch. An unreadable robots.txt is
    /// treated as no rules. `Err`: `can_fetch` raised (Python answers 500).
    pub async fn allowed_by_robots(&self, url: &str) -> Result<bool, String> {
        let parts = pyurl::urlsplit(url)?;
        let origin = format!("{}://{}", parts.scheme, parts.netloc);
        let cached = self
            .robots
            .lock()
            .map(|m| m.get(&origin).cloned())
            .unwrap_or(None);
        let parser = match cached {
            Some((at, p)) if at.elapsed() <= ROBOTS_TTL => p,
            _ => {
                let mut parser: Option<Arc<RobotFileParser>> = None;
                if let Ok(r) = self
                    .client()
                    .get(&format!("{origin}/robots.txt"), usize::MAX / 4)
                    .await
                {
                    let ct = r
                        .header("content-type")
                        .unwrap_or_else(|| "text/plain".into());
                    if r.status == 200 && ct.contains("text") {
                        let text = r.text();
                        let lines = granary_compat::text::pystr::splitlines(&text);
                        parser = RobotFileParser::parse(&lines).ok().map(Arc::new);
                    }
                }
                if let Ok(mut m) = self.robots.lock() {
                    m.insert(origin, (Instant::now(), parser.clone()));
                }
                parser
            }
        };
        match parser {
            None => Ok(true),
            Some(p) => p.can_fetch(&self.config.user_agent(), url),
        }
    }

    /// `_polite(host)`.
    pub async fn polite(&self, host: &str) {
        let slot = {
            let mut m = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
            m.entry(host.to_string()).or_default().clone()
        };
        let mut last = slot.lock().await;
        if let Some(prev) = *last {
            let due = prev + HOST_GAP;
            let now = Instant::now();
            if due > now {
                tokio::time::sleep(due - now).await;
            }
        }
        *last = Some(Instant::now());
    }

    /// `_moli(url)`.
    pub async fn moli(&self, url: &str) -> Result<Option<String>, String> {
        let args = moli::args(
            self.config.moli_timeout_s,
            &self.proxy_url(),
            MAX_RESPONSE_BYTES,
            &self.config.ua_suffix,
            url,
        );
        let _slot = self.moli_slots.acquire().await.map_err(|e| e.to_string())?;
        moli::run(&self.config.moli, &args, self.config.moli_timeout_s, url).await
    }

    /// `_plain(url)`.
    pub async fn plain(&self, url: &str) -> Result<Plain, PlainError> {
        let r = self
            .client()
            .get(url, MAX_RESPONSE_BYTES)
            .await
            .map_err(|e| PlainError::Other(e.0))?;
        let ct = r.header("content-type").unwrap_or_default();
        if r.status == 403 && ct.starts_with("text/plain") {
            let reason = String::from_utf8_lossy(&r.body);
            return Err(PlainError::Blocked(reason.chars().take(200).collect()));
        }
        if let Some(msg) = r.status_error() {
            return Err(PlainError::Other(msg));
        }
        if !ct.contains("html") && !ct.contains("text") {
            let kind = if ct.is_empty() {
                "unknown type".to_string()
            } else {
                ct
            };
            return Err(PlainError::NotAPage(format!("Not a web page ({kind})")));
        }
        let text = r.text();
        let (markdown, title) = clean::html_to_markdown(&text);
        Ok(Plain {
            markdown,
            title,
            final_url: r.url.to_string(),
        })
    }

    /// `POST /fetch` after the body was validated. `Err(Unhandled)`: an exception Python lets escape (500).
    pub async fn fetch(
        &self,
        authorization: Option<&str>,
        body: validate::FetchBody,
    ) -> Result<(StatusCode, Value), Unhandled> {
        if authorization != Some(format!("Bearer {}", self.config.token).as_str()) {
            return Ok((StatusCode::UNAUTHORIZED, json!({"detail": "unauthorized"})));
        }
        let blocked = |reason: String| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"detail": {"error": "blocked", "reason": reason}}),
            )
        };
        let host = match guard::check_url(&body.url) {
            Err(GuardError::Blocked(r)) => return Ok(blocked(r)),
            Err(GuardError::Internal(_)) => return Err(Unhandled),
            Ok((_, host, port)) => match self.hooks.resolve_public(self, &host, port).await {
                Err(GuardError::Blocked(r)) => return Ok(blocked(r)),
                Err(GuardError::Internal(_)) => return Err(Unhandled),
                Ok(()) => host,
            },
        };

        match self.hooks.allowed_by_robots(self, &body.url).await {
            Err(_) => return Err(Unhandled),
            Ok(false) => {
                return Ok((
                    StatusCode::FORBIDDEN,
                    json!({"detail": {"error": "robots", "reason": "The site's robots.txt disallows this page."}}),
                ))
            }
            Ok(true) => {}
        }

        self.hooks.polite(self, &host).await;
        let max_chars = body.max_chars as usize;
        let mut raw = self
            .hooks
            .moli(self, &body.url)
            .await
            .map_err(|_| Unhandled)?;
        let mut engine = "moli";
        let mut final_url = body.url.clone();
        let mut title: Option<String> = None;
        let mut text = match raw.as_deref() {
            Some(r) if !r.is_empty() => clean::clean_markdown(r, max_chars),
            _ => String::new(),
        };
        if text.chars().count() < MIN_USEFUL_CHARS {
            match self.hooks.plain(self, &body.url).await {
                Err(PlainError::Blocked(r)) => return Ok(blocked(r)),
                Err(PlainError::NotAPage(detail)) => {
                    if text.is_empty() {
                        return Ok((
                            StatusCode::UNSUPPORTED_MEDIA_TYPE,
                            json!({"detail": detail}),
                        ));
                    }
                }
                Err(PlainError::Other(e)) => {
                    if text.is_empty() {
                        let reason: String = e.chars().take(200).collect();
                        return Ok((
                            StatusCode::BAD_GATEWAY,
                            json!({"detail": {"error": "unreachable", "reason": reason}}),
                        ));
                    }
                }
                Ok(p) => {
                    let plain_text = clean::clean_markdown(&p.markdown, max_chars);
                    if plain_text.chars().count() > text.chars().count() {
                        raw = Some(p.markdown);
                        title = p.title;
                        final_url = p.final_url;
                        text = plain_text;
                        engine = "http";
                    }
                }
            }
        }
        let raw = raw.unwrap_or_default();
        let title = match title.filter(|t| !t.is_empty()) {
            Some(t) => Some(t),
            None => clean::title_of(&raw),
        };
        let mut links = clean::links(&raw, &final_url).map_err(|_| Unhandled)?;
        links.truncate(200);
        Ok((
            StatusCode::OK,
            json!({
                "url": body.url,
                "final_url": final_url,
                "title": title,
                "markdown": text,
                "links": links,
                "engine": engine,
            }),
        ))
    }
}

// ---------------------------------------------------------------------------------------------------
// HTTP layer: Starlette routing as the Python app has it (openapi.json, /health, /fetch; 404, 405 with
// Allow, redirect_slashes 307), FastAPI's error bodies, Starlette's 500.

fn json_response(status: StatusCode, v: &Value) -> Response<Body> {
    let mut r = Response::new(Body::from(serde_json::to_vec(v).unwrap_or_default()));
    *r.status_mut() = status;
    r.headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    r
}

fn internal_error() -> Response<Body> {
    let mut r = Response::new(Body::from("Internal Server Error"));
    *r.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    r.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

fn method_not_allowed(allow: &'static str) -> Response<Body> {
    let mut r = json_response(
        StatusCode::METHOD_NOT_ALLOWED,
        &json!({"detail": "Method Not Allowed"}),
    );
    r.headers_mut()
        .insert("allow", HeaderValue::from_static(allow));
    r
}

fn latin1(v: &HeaderValue) -> String {
    v.as_bytes().iter().map(|&b| b as char).collect()
}

/// Which route a (decoded) path is: (allowed methods, Allow header).
fn route_for(path: &str) -> Option<(&'static [&'static str], &'static str)> {
    match path {
        "/openapi.json" => Some((&["GET", "HEAD"], "GET, HEAD")),
        "/health" => Some((&["GET"], "GET")),
        "/fetch" => Some((&["POST"], "POST")),
        _ => None,
    }
}

/// Serve one request.
pub async fn handle(app: Arc<App>, req: Request<Body>) -> Response<Body> {
    let (parts, body) = req.into_parts();
    let raw_path = parts.uri.path().to_string();
    let path = pyurl::unquote(&raw_path);
    let method = parts.method.clone();
    let Some((methods, allow)) = route_for(&path) else {
        return redirect_or_404(&parts.headers, &path, parts.uri.query());
    };
    if !methods.contains(&method.as_str()) {
        return method_not_allowed(allow);
    }
    match path.as_str() {
        "/openapi.json" => {
            let mut r = Response::new(Body::from(OPENAPI));
            r.headers_mut()
                .insert("content-type", HeaderValue::from_static("application/json"));
            r
        }
        "/health" => json_response(
            StatusCode::OK,
            &json!({"ok": true, "moli": std::path::Path::new(&app.config.moli).exists()}),
        ),
        _ => {
            let bytes: Bytes = match axum::body::to_bytes(body, usize::MAX).await {
                Ok(b) => b,
                Err(_) => {
                    return json_response(
                        StatusCode::BAD_REQUEST,
                        &json!({"detail": "There was an error parsing the body"}),
                    )
                }
            };
            let ct = parts.headers.get("content-type").map(latin1);
            let fetch_body = match validate::fetch_body(ct.as_deref(), &bytes) {
                Ok(b) => b,
                Err(validate::BodyError::Invalid(errors)) => {
                    return json_response(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        &json!({"detail": errors}),
                    )
                }
                Err(validate::BodyError::Parse) => {
                    return json_response(
                        StatusCode::BAD_REQUEST,
                        &json!({"detail": "There was an error parsing the body"}),
                    )
                }
                Err(validate::BodyError::Internal) => return internal_error(),
            };
            let auth = parts.headers.get("authorization").map(latin1);
            match app.fetch(auth.as_deref(), fetch_body).await {
                Ok((status, v)) => json_response(status, &v),
                Err(Unhandled) => internal_error(),
            }
        }
    }
}

fn redirect_or_404(headers: &HeaderMap, path: &str, query: Option<&str>) -> Response<Body> {
    if path != "/" {
        let other = if path.ends_with('/') {
            path.trim_end_matches('/').to_string()
        } else {
            format!("{path}/")
        };
        if route_for(&other).is_some() {
            let host = headers.get("host").map(latin1);
            let mut url = match host {
                Some(h) => format!("http://{h}{other}"),
                None => other.clone(),
            };
            if let Some(q) = query.filter(|q| !q.is_empty()) {
                url.push('?');
                url.push_str(q);
            }
            let location = pyurl::quote(&url, ":/%#?=@[]!$&'()*+,;");
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::TEMPORARY_REDIRECT;
            if let Ok(v) = HeaderValue::from_str(&location) {
                r.headers_mut().insert("location", v);
            }
            r.headers_mut()
                .insert("content-length", HeaderValue::from_static("0"));
            return r;
        }
    }
    json_response(StatusCode::NOT_FOUND, &json!({"detail": "Not Found"}))
}

/// The axum service around [`handle`].
pub fn router(app: Arc<App>) -> axum::Router {
    axum::Router::new().fallback(move |req: Request<Body>| {
        let app = app.clone();
        async move {
            if req.method() == Method::HEAD {
                // HEAD is answered like GET without a body (the server drops it), as uvicorn does.
                let resp = handle(app, req).await;
                let (parts, _) = resp.into_parts();
                return Response::from_parts(parts, Body::empty());
            }
            handle(app, req).await
        }
    })
}

/// Start the filtering proxy and build the app (main.py's lifespan). `Err` when the token is missing.
pub async fn start(config: Config, hooks: Box<dyn Hooks>) -> Result<Arc<App>, String> {
    if config.token.is_empty() {
        return Err(
            "RESEARCH_SERVICE_TOKEN is not set \u{2014} refusing to start without auth".into(),
        );
    }
    let (addr, _task) = proxy::start(config.proxy_port)
        .await
        .map_err(|e| e.to_string())?;
    let app = Arc::new(App::new(config, addr, hooks));
    tracing::info!(
        "research service ready (moli={}, proxy={})",
        app.config.moli,
        app.proxy_url()
    );
    Ok(app)
}
