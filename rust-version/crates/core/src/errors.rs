//! The backend's error responses: `AppException` and its subclasses (backend/app/core/exceptions.py),
//! FastAPI's `HTTPException`, the request-validation 422, `IntegrityError` and the last-resort 500, each
//! rendered exactly as the handlers registered in `register_exception_handlers` render them (Starlette
//! `JSONResponse`: compact separators, UTF-8 kept, dict key order kept).

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::validation::LineError;

/// Everything a handler can fail with.
#[derive(Debug)]
pub enum ApiError {
    /// `AppException(status_code, detail, code)`: body `{"detail": ..., "code": ...}`.
    App {
        status: StatusCode,
        detail: String,
        code: Option<&'static str>,
    },
    /// `HTTPException(status_code, detail, headers)`: body `{"detail": ...}`; detail may be any JSON.
    Http {
        status: StatusCode,
        detail: Value,
        headers: Vec<(HeaderName, HeaderValue)>,
    },
    /// `RequestValidationError`: the 422 with per-field errors.
    RequestValidation(Vec<LineError>),
    /// `sqlalchemy.exc.IntegrityError`: the text of the database error (asyncpg's message).
    Integrity(String),
    /// Bad caller input caught centrally (exceptions.py `_invalid_input`): a malformed id compared with a
    /// UUID column (the `DBAPIError` handler) or a router parsing caller text with `uuid.UUID(...)` /
    /// `date.fromisoformat(...)` (the `ValueError` handler). 422 in the request-validation shape.
    /// `field: None` is `_bad_uuid_field(request)`: the first `id` / `*_id` path parameter that is not a
    /// UUID, else "id"; [`resolve_invalid_id_field`] fills it in per request.
    InvalidInput {
        field: Option<String>,
        message: &'static str,
    },
    /// Anything else: 500, logged.
    Internal(anyhow::Error),
}

/// Response extension telling the request-session middleware (and CORS) how the request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorOutcome {
    /// Raised inside the app: rolled back, still passes through CORS like any handled exception.
    Handled,
    /// The `Exception` handler's 500: Starlette runs it in ServerErrorMiddleware, outside CORS, so the
    /// response carries no CORS headers.
    Unhandled,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    /// `NotFoundError(resource, resource_id)`.
    pub fn not_found(resource: &str, resource_id: Option<&str>) -> ApiError {
        let detail = match resource_id {
            Some(id) if !id.is_empty() => format!("{resource} '{id}' not found"),
            _ => format!("{resource} not found"),
        };
        ApiError::App {
            status: StatusCode::NOT_FOUND,
            detail,
            code: Some("NOT_FOUND"),
        }
    }

    /// `ConflictError(detail)`.
    pub fn conflict(detail: impl Into<String>) -> ApiError {
        ApiError::App {
            status: StatusCode::CONFLICT,
            detail: detail.into(),
            code: Some("CONFLICT"),
        }
    }

    /// `ForbiddenError()` ("Access denied") or `ForbiddenError(detail)`.
    pub fn forbidden(detail: Option<&str>) -> ApiError {
        ApiError::App {
            status: StatusCode::FORBIDDEN,
            detail: detail.unwrap_or("Access denied").to_string(),
            code: Some("FORBIDDEN"),
        }
    }

    /// `ValidationError(detail)` (the app's own, not Pydantic's).
    pub fn validation(detail: impl Into<String>) -> ApiError {
        ApiError::App {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail: detail.into(),
            code: Some("VALIDATION_ERROR"),
        }
    }

    /// `BusinessRuleError(detail)`.
    pub fn business_rule(detail: impl Into<String>) -> ApiError {
        ApiError::App {
            status: StatusCode::BAD_REQUEST,
            detail: detail.into(),
            code: Some("BUSINESS_RULE_VIOLATION"),
        }
    }

    /// `AppException(status_code, detail, code)` used directly.
    pub fn app(status: u16, detail: impl Into<String>, code: Option<&'static str>) -> ApiError {
        ApiError::App {
            status: status_code(status),
            detail: detail.into(),
            code,
        }
    }

    /// `HTTPException(status_code=..., detail="...")`.
    pub fn http(status: u16, detail: impl Into<String>) -> ApiError {
        ApiError::Http {
            status: status_code(status),
            detail: Value::String(detail.into()),
            headers: vec![],
        }
    }

    /// `HTTPException(status_code=..., detail={...})`.
    pub fn http_json(status: u16, detail: Value) -> ApiError {
        ApiError::Http {
            status: status_code(status),
            detail,
            headers: vec![],
        }
    }

    /// `HTTPException(status_code=...)` with no detail: Starlette uses the HTTP reason phrase.
    pub fn http_default(status: u16) -> ApiError {
        let s = status_code(status);
        ApiError::Http {
            status: s,
            detail: Value::String(reason_phrase(s).to_string()),
            headers: vec![],
        }
    }

    /// Adds a response header (`HTTPException(..., headers={...})`).
    pub fn with_header(mut self, name: &'static str, value: &str) -> ApiError {
        if let ApiError::Http { headers, .. } = &mut self {
            if let Ok(v) = HeaderValue::from_str(value) {
                headers.push((HeaderName::from_static(name), v));
            }
        }
        self
    }

    /// A malformed id (exceptions.py: "Invalid id format"); the field is resolved from the path.
    pub fn invalid_id() -> ApiError {
        ApiError::InvalidInput {
            field: None,
            message: MSG_INVALID_ID,
        }
    }

    /// Bad ISO date/datetime text parsed in a router (`date.fromisoformat` / `datetime.fromisoformat`).
    pub fn invalid_date() -> ApiError {
        ApiError::InvalidInput {
            field: Some("date".to_string()),
            message: MSG_INVALID_DATE,
        }
    }

    pub fn internal(e: impl Into<anyhow::Error>) -> ApiError {
        ApiError::Internal(e.into())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::App { status, .. } | ApiError::Http { status, .. } => *status,
            ApiError::RequestValidation(_) | ApiError::InvalidInput { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            ApiError::Integrity(_) => StatusCode::CONFLICT,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The JSON body the Python handler sends.
    pub fn body(&self) -> Value {
        match self {
            ApiError::App { detail, code, .. } => json!({"detail": detail, "code": code}),
            ApiError::Http { detail, .. } => json!({"detail": detail}),
            ApiError::RequestValidation(errors) => {
                let errs: Vec<Value> = errors
                    .iter()
                    .map(|e| json!({"field": e.field(), "message": e.msg}))
                    .collect();
                json!({"detail": "Validation failed", "errors": errs, "code": "VALIDATION_ERROR"})
            }
            ApiError::InvalidInput { field, message } => json!({
                "detail": "Validation failed",
                "errors": [{"field": field.as_deref().unwrap_or("id"), "message": message}],
                "code": "VALIDATION_ERROR",
            }),
            ApiError::Integrity(orig) => {
                json!({"detail": integrity_detail(orig), "code": "CONFLICT"})
            }
            ApiError::Internal(_) => {
                json!({"detail": "Internal server error", "code": "INTERNAL_ERROR"})
            }
        }
    }
}

pub const MSG_INVALID_ID: &str = "Invalid id format";
pub const MSG_INVALID_DATE: &str = "Invalid date format, expected YYYY-MM-DD (ISO 8601)";

/// Response extension: an [`ApiError::InvalidInput`] whose field is the request's bad path parameter.
#[derive(Clone, Copy, Debug)]
pub struct InvalidIdField(pub &'static str);

/// exceptions.py `_bad_uuid_field(request)`: the first path parameter named `id` or `*_id` whose value
/// `uuid.UUID()` refuses, else "id". `params` in path order (Starlette's `path_params`).
pub fn bad_uuid_field<'a>(params: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    for (name, value) in params {
        if (name == "id" || name.ends_with("_id"))
            && granary_compat::text::utils::py_uuid(value).is_none()
        {
            return name.to_string();
        }
    }
    "id".to_string()
}

/// Path parameters the Rust router names differently from Python (axum allows one name per position under
/// a shared prefix): (route template, Rust name, Python name). The 422 names the Python parameter.
const PARAM_RENAMES: &[(&str, &str, &str)] = &[
    (
        "/api/v1/approvals/policies/{request_id}",
        "request_id",
        "policy_id",
    ),
    (
        "/api/v1/finance/invoices/{invoice_id}/convert-to-commercial",
        "invoice_id",
        "pi_id",
    ),
    ("/api/v1/messaging/messages/{id}", "id", "other_user_id"),
    ("/api/v1/messaging/messages/{id}/react", "id", "message_id"),
    (
        "/api/v1/messaging/messages/{id}/reactions",
        "id",
        "message_id",
    ),
];

fn python_param_name<'a>(route: &str, name: &'a str) -> &'a str {
    PARAM_RENAMES
        .iter()
        .find(|(r, n, _)| *r == route && *n == name)
        .map(|(_, _, py)| *py)
        .unwrap_or(name)
}

/// Per-route layer: answers an [`ApiError::InvalidInput`] that had no field with the field named by
/// [`bad_uuid_field`] over the matched route's path parameters.
pub async fn resolve_invalid_id_field(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use axum::extract::FromRequestParts;
    let (mut parts, body) = req.into_parts();
    let route = parts
        .extensions
        .get::<axum::extract::MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    let params: Vec<(String, String)> =
        axum::extract::RawPathParams::from_request_parts(&mut parts, &())
            .await
            .map(|p| {
                p.iter()
                    .map(|(k, v)| (python_param_name(&route, k).to_string(), v.to_string()))
                    .collect()
            })
            .unwrap_or_default();
    let resp = next
        .run(axum::extract::Request::from_parts(parts, body))
        .await;
    let Some(InvalidIdField(message)) = resp.extensions().get::<InvalidIdField>().copied() else {
        return resp;
    };
    let field = bad_uuid_field(params.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    ApiError::InvalidInput {
        field: Some(field),
        message,
    }
    .into_response()
}

/// exceptions.py `_is_invalid_uuid`: asyncpg's client-side "invalid UUID" or Postgres's
/// "invalid input syntax for type uuid".
pub fn is_invalid_uuid_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("invalid uuid") || lower.contains("invalid input syntax for type uuid")
}

/// The three texts of `integrity_error_handler`, decided on `str(exc.orig).lower()`.
pub fn integrity_detail(orig: &str) -> &'static str {
    let lower = orig.to_lowercase();
    if lower.contains("unique") {
        "A record with these values already exists"
    } else if lower.contains("foreign key") {
        "Referenced record does not exist"
    } else {
        "Database constraint violation"
    }
}

fn status_code(s: u16) -> StatusCode {
    StatusCode::from_u16(s).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

/// `http.HTTPStatus(status).phrase`.
pub fn reason_phrase(s: StatusCode) -> &'static str {
    match s.as_u16() {
        413 => "Request Entity Too Large",
        414 => "Request-URI Too Long",
        416 => "Requested Range Not Satisfiable",
        _ => s.canonical_reason().unwrap_or(""),
    }
}

/// Starlette `JSONResponse` rendering.
pub fn json_response(status: StatusCode, body: &Value) -> Response {
    let bytes = serde_json::to_vec(body).expect("serializable");
    let mut resp = (status, bytes).into_response();
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    resp
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let outcome = match &self {
            ApiError::Internal(e) => {
                tracing::error!(error = %e, error_type = "Exception", "unhandled_exception");
                ErrorOutcome::Unhandled
            }
            _ => ErrorOutcome::Handled,
        };
        let mut resp = json_response(self.status(), &self.body());
        if let ApiError::Http { headers, .. } = &self {
            let map: &mut HeaderMap = resp.headers_mut();
            for (k, v) in headers {
                map.insert(k.clone(), v.clone());
            }
        }
        resp.extensions_mut().insert(outcome);
        if let ApiError::InvalidInput {
            field: None,
            message,
        } = &self
        {
            resp.extensions_mut().insert(InvalidIdField(message));
        }
        resp
    }
}

impl From<sqlx::Error> for ApiError {
    /// SQLSTATE class 23 is what SQLAlchemy raises as IntegrityError; every other database error is a 500.
    fn from(e: sqlx::Error) -> ApiError {
        if let sqlx::Error::Database(db) = &e {
            if db.code().is_some_and(|c| c.starts_with("23")) {
                // str(exc.orig) starts with the asyncpg class name (UniqueViolationError,
                // ForeignKeyViolationError, ...), which is what the substring test sees first.
                let class = match db.code().as_deref() {
                    Some("23505") => "UniqueViolationError: ",
                    Some("23503") => "ForeignKeyViolationError: ",
                    _ => "",
                };
                return ApiError::Integrity(format!("{class}{}", db.message()));
            }
            // A malformed id compared with a UUID column on the server side (`$1::uuid`, a text
            // literal): the DBAPIError handler's 422.
            if is_invalid_uuid_text(db.message()) {
                return ApiError::invalid_id();
            }
        }
        ApiError::Internal(e.into())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> ApiError {
        ApiError::Internal(e)
    }
}

impl From<redis::RedisError> for ApiError {
    fn from(e: redis::RedisError) -> ApiError {
        ApiError::Internal(e.into())
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status().as_u16(), self.body())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::Loc;

    #[test]
    fn bodies_keep_python_key_order() {
        assert_eq!(
            serde_json::to_string(&ApiError::not_found("Invoice", Some("INV-1")).body()).unwrap(),
            r#"{"detail":"Invoice 'INV-1' not found","code":"NOT_FOUND"}"#
        );
        assert_eq!(
            serde_json::to_string(&ApiError::app(400, "x", None).body()).unwrap(),
            r#"{"detail":"x","code":null}"#
        );
        let v = ApiError::RequestValidation(vec![LineError {
            loc: vec![
                Loc::Key("body".into()),
                Loc::Key("items".into()),
                Loc::Index(0),
            ],
            msg: "Field required".into(),
        }]);
        assert_eq!(
            serde_json::to_string(&v.body()).unwrap(),
            r#"{"detail":"Validation failed","errors":[{"field":"items -> 0","message":"Field required"}],"code":"VALIDATION_ERROR"}"#
        );
        assert_eq!(
            integrity_detail("NotNullViolationError: null value"),
            "Database constraint violation"
        );
    }
}
