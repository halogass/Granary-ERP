//! Request helpers the routers share, on top of granary_core (auth extractors in `granary_core::auth`, the DB
//! session in `granary_core::db::Db`, JSON/query/path validation in `granary_core::validation`).
//!
//! - [`parse_uuid`]: core/utils.py `parse_uuid(val, field)` (400 "Invalid {field} format").
//! - [`pg_uuid`] / [`router_uuid`]: a malformed id reaching asyncpg / `uuid.UUID()` in a router, the
//!   central 422 "Invalid id format" (exceptions.py); bad ISO dates: `ApiError::invalid_date()`.
//! - [`ClientIp`]: core/utils.py `get_client_ip(request)` (X-Real-IP, else the peer address).
//! - [`PyForm`]: `Form(...)` / `File(...)` / `UploadFile` parameters (multipart or urlencoded), with
//!   FastAPI 0.115's rules for an empty value and its 422 texts.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request};
use axum::http::header;
use axum::http::request::Parts;
use uuid::Uuid;

use granary_core::errors::ApiError;
use granary_core::validation::{Cx, Loc, MSG_MISSING};

/// `parse_uuid(val, field)`: `uuid.UUID(val)` acceptance; 400 `Invalid {field} format` otherwise.
/// Python's default field is "ID": `parse_uuid(lead_id)` is `parse_uuid(&lead_id, "ID")`.
pub fn parse_uuid(val: &str, field: &str) -> Result<Uuid, ApiError> {
    granary_compat::text::utils::parse_uuid(val, field)
        .map_err(|detail| ApiError::http(400, detail))
}

/// asyncpg's text-to-uuid parsing (pgproto `pg_uuid_bytes_from_str`): 32..36 bytes, hyphens anywhere,
/// exactly 32 hex digits. A `str` compared with (or written to) a UUID column; anything else is the
/// asyncpg DataError, which the DBAPIError handler answers 422 "Invalid id format" (field resolved from
/// the path, see `granary_core::errors::bad_uuid_field`).
pub fn pg_uuid(s: &str) -> Result<Uuid, ApiError> {
    asyncpg_uuid(s).ok_or_else(ApiError::invalid_id)
}

/// [`pg_uuid`] for an `Optional[str]` (None stays NULL).
pub fn pg_uuid_opt(s: Option<&str>) -> Result<Option<Uuid>, ApiError> {
    s.map(pg_uuid).transpose()
}

/// The asyncpg acceptance rule of [`pg_uuid`], without the error.
pub fn asyncpg_uuid(s: &str) -> Option<Uuid> {
    let b = s.as_bytes();
    if b.len() < 32 || b.len() > 36 {
        return None;
    }
    let mut hex = String::with_capacity(32);
    for &c in b {
        if c == b'-' {
            continue;
        }
        if !c.is_ascii_hexdigit() {
            return None;
        }
        hex.push(c as char);
    }
    if hex.len() != 32 {
        return None;
    }
    Uuid::parse_str(&hex).ok()
}

/// `uuid.UUID(text)` on a router line: CPython's acceptance; a ValueError is the 422 "Invalid id format"
/// of exceptions.py `value_error_handler` (field resolved from the path).
pub fn router_uuid(s: &str) -> Result<Uuid, ApiError> {
    granary_compat::text::utils::py_uuid(s).ok_or_else(ApiError::invalid_id)
}

/// `get_client_ip(request)`: the stripped X-Real-IP header when present and non-empty, else the peer host
/// (None when the server was not started with connect info).
#[derive(Clone, Debug)]
pub struct ClientIp(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Starlette decodes header values as latin-1.
        let real_ip: Option<String> = parts
            .headers
            .get("x-real-ip")
            .map(|v| v.as_bytes().iter().map(|b| *b as char).collect());
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string());
        Ok(ClientIp(granary_compat::text::utils::get_client_ip(
            real_ip.as_deref(),
            peer.as_deref(),
        )))
    }
}

/// An uploaded file (`UploadFile`): filename, the part's content type, the bytes.
#[derive(Clone, Debug)]
pub struct UploadFile {
    pub filename: String,
    pub content_type: Option<String>,
    pub data: Bytes,
}

/// One form value: a text field or a file part (a part with a filename).
#[derive(Clone, Debug)]
pub enum FormValue {
    Text(String),
    File(UploadFile),
}

/// The request's form (`await request.form()`), items in arrival order. A body that is neither
/// `multipart/form-data` nor `application/x-www-form-urlencoded` gives an empty form (FastAPI then reports
/// every required field as missing). A malformed multipart body is 400 "There was an error parsing the body".
/// Must be the last handler argument (it consumes the body).
///
/// ```ignore
/// async fn upload(user: CurrentUser, Db(db): Db, form: PyForm) -> ApiResult<Response> {
///     let mut cx = Cx::new();
///     let entity_type = form.req_text(&mut cx, "entity_type");
///     let file = form.req_file(&mut cx, "file");
///     form.check(cx)?;                       // one 422 listing every problem, in parameter order
///     let (entity_type, file) = (entity_type.unwrap(), file.unwrap());
///     ...
/// }
/// ```
#[derive(Clone, Debug, Default)]
pub struct PyForm {
    pub items: Vec<(String, FormValue)>,
}

impl PyForm {
    /// `form.get(name)`: the last value with that name (Starlette's multidict).
    pub fn get(&self, name: &str) -> Option<&FormValue> {
        self.items
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
    }

    /// `form.getlist(name)`.
    pub fn get_all(&self, name: &str) -> Vec<&FormValue> {
        self.items
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v)
            .collect()
    }

    /// `name: str | None = Form(None)`: None when absent; an empty value is `""`. FastAPI 0.115 maps `""`
    /// to the parameter's default, but `_extract_form_body` then copies the raw value back in when that
    /// default is None (or the field is required), so `""` reaches the handler. A non-None default
    /// (`Form("x")`) does replace `""`: use [`PyForm::text_or`].
    pub fn opt_text(&self, name: &str) -> Option<String> {
        match self.get(name) {
            Some(FormValue::Text(s)) => Some(s.clone()),
            // A file sent for a str field: FastAPI validates the UploadFile as str and fails; routers in
            // this codebase never hit that, so treat it as absent.
            _ => None,
        }
    }

    /// `name: str = Form(default)` with a non-None default: the default when absent or `""`.
    pub fn text_or(&self, name: &str, default: &str) -> String {
        match self.opt_text(name) {
            Some(s) if !s.is_empty() => s,
            _ => default.to_string(),
        }
    }

    /// `name: str = Form(...)`: records "Field required" at ("body", name) when absent; `""` is a value.
    pub fn req_text(&self, cx: &mut Cx, name: &str) -> Option<String> {
        let v = self.opt_text(name);
        if v.is_none() {
            field_err(cx, name, MSG_MISSING);
        }
        v
    }

    /// `file: UploadFile | None = File(None)`: a text value where a file is expected is a 422
    /// ("Value error, Expected UploadFile, received: <class 'str'>"), an empty text value included (FastAPI
    /// copies the raw `""` back in and the UploadFile check rejects it).
    pub fn opt_file(&self, cx: &mut Cx, name: &str) -> Option<UploadFile> {
        match self.get(name) {
            Some(FormValue::File(f)) => Some(f.clone()),
            Some(FormValue::Text(_)) => {
                field_err(
                    cx,
                    name,
                    "Value error, Expected UploadFile, received: <class 'str'>",
                );
                None
            }
            _ => None,
        }
    }

    /// `file: UploadFile = File(...)`.
    pub fn req_file(&self, cx: &mut Cx, name: &str) -> Option<UploadFile> {
        let mark = cx.mark();
        let v = self.opt_file(cx, name);
        if v.is_none() && !cx.has_errors_since(mark) {
            field_err(cx, name, MSG_MISSING);
        }
        v
    }

    /// `files: list[UploadFile] = File(...)`: every file part with that name.
    pub fn files(&self, name: &str) -> Vec<UploadFile> {
        self.get_all(name)
            .into_iter()
            .filter_map(|v| match v {
                FormValue::File(f) => Some(f.clone()),
                FormValue::Text(_) => None,
            })
            .collect()
    }

    /// Ends validation: the 422 with everything recorded in `cx`, or Ok.
    pub fn check(&self, cx: Cx) -> Result<(), ApiError> {
        if cx.errors.is_empty() {
            Ok(())
        } else {
            Err(ApiError::RequestValidation(cx.errors))
        }
    }
}

fn field_err(cx: &mut Cx, name: &str, msg: &str) {
    cx.at(Loc::Key("body".into()), |cx| {
        cx.at_key(name, |cx| cx.err(msg))
    });
}

fn parse_error() -> ApiError {
    ApiError::http(400, "There was an error parsing the body")
}

impl<S: Send + Sync> FromRequest<S> for PyForm {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let ctype = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ctype.starts_with("multipart/form-data") {
            let mut mp = axum::extract::Multipart::from_request(req, state)
                .await
                .map_err(|_| parse_error())?;
            let mut items = Vec::new();
            while let Some(field) = mp.next_field().await.map_err(|_| parse_error())? {
                let name = field.name().unwrap_or("").to_string();
                let filename = field.file_name().map(str::to_string);
                let content_type = field.content_type().map(str::to_string);
                let data = field.bytes().await.map_err(|_| parse_error())?;
                let value = match filename {
                    Some(filename) => FormValue::File(UploadFile {
                        filename,
                        content_type,
                        data,
                    }),
                    None => FormValue::Text(String::from_utf8_lossy(&data).into_owned()),
                };
                items.push((name, value));
            }
            Ok(PyForm { items })
        } else if ctype.starts_with("application/x-www-form-urlencoded") {
            let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                .await
                .map_err(|_| parse_error())?;
            let items = form_urlencoded::parse(&bytes)
                .map(|(k, v)| (k.into_owned(), FormValue::Text(v.into_owned())))
                .collect();
            Ok(PyForm { items })
        } else {
            Ok(PyForm::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(items: &[(&str, &str)]) -> PyForm {
        PyForm {
            items: items
                .iter()
                .map(|(k, v)| (k.to_string(), FormValue::Text(v.to_string())))
                .collect(),
        }
    }

    /// Recorded from FastAPI 0.115.5 (urlencoded and multipart behave the same).
    #[test]
    fn empty_values_follow_fastapi() {
        let f = form(&[("title", ""), ("file", "")]);
        let mut cx = Cx::new();
        // str = Form(...): "" is a value.
        assert_eq!(f.req_text(&mut cx, "title").as_deref(), Some(""));
        // str | None = Form(None): "" is a value; absent is None.
        assert_eq!(f.opt_text("title").as_deref(), Some(""));
        assert_eq!(f.opt_text("other"), None);
        // str = Form("dflt"): "" and absent give the default.
        assert_eq!(f.text_or("title", "dflt"), "dflt");
        assert_eq!(f.text_or("other", "dflt"), "dflt");
        assert!(cx.errors.is_empty());
        // UploadFile | None = File(None): "" is "Expected UploadFile".
        assert!(f.opt_file(&mut cx, "file").is_none());
        assert_eq!(cx.errors.len(), 1);
        // UploadFile = File(...): absent is "Field required".
        assert!(f.req_file(&mut cx, "upload").is_none());
        assert_eq!(cx.errors.len(), 2);
    }
}
