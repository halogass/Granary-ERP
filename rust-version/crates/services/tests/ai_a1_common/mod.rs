//! A scripted HTTP server for the AI provider tests (the Rust twin of the server the goldens were
//! first recorded against): answers each request with the next scripted response and
//! records what it got.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Default)]
struct Inner {
    script: VecDeque<(u16, Vec<(String, String)>, String)>,
    seen: Vec<Seen>,
}

#[derive(Clone)]
pub struct Scripted {
    inner: Arc<Mutex<Inner>>,
    pub base: String,
}

async fn handle(
    State(s): State<Arc<Mutex<Inner>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut g = s.lock().unwrap();
    g.seen.push(Seen {
        method: method.to_string(),
        path: uri
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_default(),
        headers: headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_lowercase(),
                    v.to_str().unwrap_or("").to_string(),
                )
            })
            .collect(),
        body: String::from_utf8_lossy(&body).into_owned(),
    });
    let (status, hdrs, text) = g.script.pop_front().unwrap_or((
        599,
        vec![("content-type".into(), "text/plain".into())],
        "no scripted response".into(),
    ));
    let mut resp = (StatusCode::from_u16(status).unwrap(), text).into_response();
    for (k, v) in hdrs {
        resp.headers_mut().insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(&v).unwrap(),
        );
    }
    resp
}

impl Scripted {
    pub async fn start() -> Scripted {
        let inner = Arc::new(Mutex::new(Inner::default()));
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Scripted {
            inner,
            base: format!("http://{addr}"),
        }
    }

    /// Replaces the script and forgets what was seen.
    pub fn script(&self, responses: Vec<(u16, Vec<(String, String)>, String)>) {
        let mut g = self.inner.lock().unwrap();
        g.script = responses.into();
        g.seen.clear();
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.inner.lock().unwrap().seen.clone()
    }

    pub fn unused(&self) -> usize {
        self.inner.lock().unwrap().script.len()
    }
}
