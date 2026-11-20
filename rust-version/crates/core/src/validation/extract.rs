//! axum extractors that read request input the way FastAPI 0.115 does and validate it with Pydantic
//! semantics. Errors become the app's 422 body.
//!
//! - [`PyJson<T>`]: a JSON body parameter (`body: Model`).
//! - [`PyQuery<T>`]: query parameters; declare them as a `py_model!` whose fields are the parameters.
//! - [`PyPath<T>`]: typed path parameters (most routers take `str` ids and call `parse_uuid`, which needs
//!   no extractor).
//! - [`Inputs`]: when one endpoint mixes path, query and body validation and a single 422 must list all of
//!   them in FastAPI's order (path, query, body).
//!
//! Ordering note: FastAPI decodes the JSON body before resolving dependencies, so an invalid JSON body
//! answers 422 even without a valid token. axum runs header extractors (auth) before the body extractor,
//! so here the 401/403 comes first. Only malformed-JSON requests without credentials see the difference.

use axum::body::Body;
use axum::extract::{FromRequest, FromRequestParts, RawPathParams, Request};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use indexmap::IndexMap;

use super::pyobj::{json_loads, JsonLoadError, PyObj};
use super::{Cx, LineError, Loc, Validate, MSG_MISSING};
use crate::errors::ApiError;

/// What FastAPI hands to the body field.
#[derive(Clone, Debug, PartialEq)]
pub enum BodyInput {
    /// Empty body, or JSON `null`.
    Missing,
    Value(PyObj),
}

/// `get_request_handler`'s body handling: JSON when the content type is absent, `application/json` or
/// `application/*+json`; raw bytes otherwise; nothing when the body is empty.
pub fn body_from_bytes(headers: &HeaderMap, bytes: &[u8]) -> Result<BodyInput, ApiError> {
    if bytes.is_empty() {
        return Ok(BodyInput::Missing);
    }
    let is_json = match headers.get(axum::http::header::CONTENT_TYPE) {
        None => true,
        Some(v) => {
            // email.message.Message().get_content_type(): "type/subtype" lower-cased, params dropped;
            // anything without a '/' falls back to text/plain.
            let raw = String::from_utf8_lossy(v.as_bytes()).to_string();
            if raw.is_empty() {
                true
            } else {
                let ctype = raw
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                match ctype.split_once('/') {
                    Some(("application", sub)) => sub == "json" || sub.ends_with("+json"),
                    _ => false,
                }
            }
        }
    };
    if !is_json {
        return Ok(BodyInput::Value(PyObj::Bytes(bytes.to_vec())));
    }
    match json_loads(bytes) {
        Ok(PyObj::None) => Ok(BodyInput::Missing),
        Ok(v) => Ok(BodyInput::Value(v)),
        Err(JsonLoadError::Decode { pos, .. }) => {
            Err(ApiError::RequestValidation(vec![LineError {
                loc: vec![Loc::Key("body".into()), Loc::Index(pos)],
                msg: "JSON decode error".into(),
            }]))
        }
        Err(JsonLoadError::Other) => {
            Err(ApiError::http(400, "There was an error parsing the body"))
        }
    }
}

fn validate_body<T: Validate>(body: &BodyInput, cx: &mut Cx) -> Option<T> {
    cx.at_key("body", |cx| match body {
        BodyInput::Missing => {
            cx.err(MSG_MISSING);
            None
        }
        BodyInput::Value(v) => T::validate(v, cx),
    })
}

async fn read_body(req: Request) -> Result<(HeaderMap, BodyInput), ApiError> {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|_| ApiError::http(400, "There was an error parsing the body"))?;
    let input = body_from_bytes(&parts.headers, &bytes)?;
    Ok((parts.headers, input))
}

/// A required JSON body validated as `T`.
#[derive(Debug, Clone)]
pub struct PyJson<T>(pub T);

impl<S: Send + Sync, T: Validate> FromRequest<S> for PyJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let (_, body) = read_body(req).await?;
        let mut cx = Cx::new();
        match validate_body::<T>(&body, &mut cx) {
            Some(t) if cx.errors.is_empty() => Ok(PyJson(t)),
            _ => Err(ApiError::RequestValidation(cx.errors)),
        }
    }
}

/// Starlette `request.query_params` as a dict: `parse_qsl(keep_blank_values=True)`, last value wins.
pub fn query_dict(query: Option<&str>) -> IndexMap<String, PyObj> {
    let mut out = IndexMap::new();
    if let Some(q) = query {
        for (k, v) in form_urlencoded::parse(q.as_bytes()) {
            out.insert(k.into_owned(), PyObj::Str(v.into_owned()));
        }
    }
    out
}

/// All values of one query parameter (`list[str] = Query(...)`).
pub fn query_list(query: Option<&str>, name: &str) -> Vec<String> {
    match query {
        Some(q) => form_urlencoded::parse(q.as_bytes())
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
            .collect(),
        None => Vec::new(),
    }
}

fn validate_query<T: Validate>(parts: &Parts, cx: &mut Cx) -> Option<T> {
    let dict = PyObj::Dict(query_dict(parts.uri.query()));
    cx.at_key("query", |cx| T::validate(&dict, cx))
}

/// Query parameters validated as the model `T` (one field per parameter).
#[derive(Debug, Clone)]
pub struct PyQuery<T>(pub T);

impl<S: Send + Sync, T: Validate> FromRequestParts<S> for PyQuery<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let mut cx = Cx::new();
        match validate_query::<T>(parts, &mut cx) {
            Some(t) if cx.errors.is_empty() => Ok(PyQuery(t)),
            _ => Err(ApiError::RequestValidation(cx.errors)),
        }
    }
}

async fn path_dict<S: Send + Sync>(parts: &mut Parts, state: &S) -> PyObj {
    let mut out = IndexMap::new();
    if let Ok(params) = RawPathParams::from_request_parts(parts, state).await {
        for (k, v) in params.iter() {
            out.insert(k.to_string(), PyObj::Str(v.to_string()));
        }
    }
    PyObj::Dict(out)
}

/// Path parameters validated as the model `T`.
#[derive(Debug, Clone)]
pub struct PyPath<T>(pub T);

impl<S: Send + Sync, T: Validate> FromRequestParts<S> for PyPath<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let dict = path_dict(parts, state).await;
        let mut cx = Cx::new();
        match cx.at_key("path", |cx| T::validate(&dict, cx)) {
            Some(t) if cx.errors.is_empty() => Ok(PyPath(t)),
            _ => Err(ApiError::RequestValidation(cx.errors)),
        }
    }
}

/// A model with no fields: "this endpoint has no path / query / body parameters".
#[derive(Debug, Clone, Copy, Default)]
pub struct NoParams;

impl Validate for NoParams {
    fn validate(_v: &PyObj, _cx: &mut Cx) -> Option<Self> {
        Some(NoParams)
    }
}

/// Raw path, query and body of a request, validated together so one 422 lists every problem in FastAPI's
/// order. Consumes the body, so it must be the last handler argument.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub path: PyObj,
    pub query: PyObj,
    pub body: BodyInput,
}

impl Inputs {
    /// Validates path params as `P`, query params as `Q` and the body as `B` (use [`NoParams`] for a part
    /// the endpoint does not have; with `B = NoParams` the body is ignored).
    pub fn validate<P: Validate, Q: Validate, B: Validate + 'static>(
        &self,
    ) -> Result<(P, Q, B), ApiError> {
        let mut cx = Cx::new();
        let p = cx.at_key("path", |cx| P::validate(&self.path, cx));
        let q = cx.at_key("query", |cx| Q::validate(&self.query, cx));
        let b = if std::any::TypeId::of::<B>() == std::any::TypeId::of::<NoParams>() {
            B::validate(&PyObj::None, &mut cx)
        } else {
            validate_body::<B>(&self.body, &mut cx)
        };
        match (p, q, b) {
            (Some(p), Some(q), Some(b)) if cx.errors.is_empty() => Ok((p, q, b)),
            _ => Err(ApiError::RequestValidation(cx.errors)),
        }
    }
}

impl<S: Send + Sync> FromRequest<S> for Inputs {
    type Rejection = ApiError;

    async fn from_request(req: Request<Body>, state: &S) -> Result<Self, Self::Rejection> {
        let (mut parts, body) = req.into_parts();
        let path = path_dict(&mut parts, state).await;
        let query = PyObj::Dict(query_dict(parts.uri.query()));
        let req = Request::from_parts(parts, body);
        let (_, body) = read_body(req).await?;
        Ok(Inputs { path, query, body })
    }
}
