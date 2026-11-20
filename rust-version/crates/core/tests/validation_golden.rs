//! Replays compat-tests/golden/validation/http_cases.json (recorded from FastAPI 0.115.5 + Pydantic 2.10.2
//! with the backend's own exception handlers) against the same endpoints built with
//! granary_core, and requires the same status, headers and response bytes.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Path;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::NaiveDate;
use granary_core::validation::v::{self, DecC, ListC, NumC, StrC};
use granary_core::validation::{
    py_float_repr, DateTimeIn, EmailStr, Inputs, NoParams, PyJson, PyPath, PyQuery,
};
use granary_core::{py_literal, py_model, ApiError, AppState};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use tower::ServiceExt;
use uuid::Uuid;

/// Cases where the port knowingly differs from Python (documented in validation/mod.rs).
const KNOWN_DEVIATIONS: &[(&str, &str)] = &[
    (
        "count_19",
        "int fields are i64; Python accepts 12345678901234567890",
    ),
    (
        "count_20",
        "int fields are i64; Python accepts 99999999999999999999",
    ),
    (
        "price_5",
        "rust_decimal keeps Decimal('1e2') as 100, Python str() gives 1E+2",
    ),
    (
        "price_15",
        "rust_decimal keeps Decimal('1E-7') as 0.0000001",
    ),
    ("count_24", "non-ASCII digits"),
    (
        "price_12",
        "rust_decimal has no negative zero: Decimal('-0') becomes 0",
    ),
];

// ── canonical echo (the reference canon()) ───────────────────────────────

trait Canon {
    fn canon(&self) -> Value;
}

impl Canon for String {
    fn canon(&self) -> Value {
        Value::String(self.clone())
    }
}
impl Canon for i64 {
    fn canon(&self) -> Value {
        Value::String(self.to_string())
    }
}
impl Canon for f64 {
    fn canon(&self) -> Value {
        Value::String(py_float_repr(*self))
    }
}
impl Canon for bool {
    fn canon(&self) -> Value {
        Value::Bool(*self)
    }
}
impl Canon for Decimal {
    fn canon(&self) -> Value {
        Value::String(self.to_string())
    }
}
impl Canon for Uuid {
    fn canon(&self) -> Value {
        Value::String(self.to_string())
    }
}
impl Canon for NaiveDate {
    fn canon(&self) -> Value {
        Value::String(self.format("%Y-%m-%d").to_string())
    }
}
impl Canon for DateTimeIn {
    fn canon(&self) -> Value {
        Value::String(self.isoformat())
    }
}
impl Canon for EmailStr {
    fn canon(&self) -> Value {
        Value::String(self.0.clone())
    }
}
impl<T: Canon> Canon for Option<T> {
    fn canon(&self) -> Value {
        match self {
            Some(x) => x.canon(),
            None => Value::Null,
        }
    }
}
impl<T: Canon> Canon for Vec<T> {
    fn canon(&self) -> Value {
        Value::Array(self.iter().map(Canon::canon).collect())
    }
}
impl<T: Canon> Canon for IndexMap<String, T> {
    fn canon(&self) -> Value {
        Value::Object(self.iter().map(|(k, v)| (k.clone(), v.canon())).collect())
    }
}
impl Canon for Value {
    /// Any / dict values: ints -> str, floats -> repr (already repr text), recursively.
    fn canon(&self) -> Value {
        match self {
            Value::Number(n) => Value::String(n.to_string()),
            Value::Array(a) => Value::Array(a.iter().map(Canon::canon).collect()),
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, v)| (k.clone(), v.canon())).collect())
            }
            other => other.clone(),
        }
    }
}
impl Canon for Map<String, Value> {
    fn canon(&self) -> Value {
        Value::Object(self.clone()).canon()
    }
}

macro_rules! canon_struct {
    ($t:ty { $($f:ident),* }) => {
        impl Canon for $t {
            fn canon(&self) -> Value {
                let mut m = Map::new();
                $( m.insert(granary_core::validation::field_name(stringify!($f)).to_string(), self.$f.canon()); )*
                Value::Object(m)
            }
        }
    };
}

// ── the models of the reference recording ──────────────────────────────────────────

py_literal! { pub enum KindAbc { A = "a", B = "b", C = "c" } }
py_literal! { pub enum OneX { X = "x" } }
py_literal! { pub enum KindXy { X = "x", Y = "y" } }

impl Canon for KindAbc {
    fn canon(&self) -> Value {
        Value::String(self.as_str().into())
    }
}
impl Canon for OneX {
    fn canon(&self) -> Value {
        Value::String(self.as_str().into())
    }
}
impl Canon for KindXy {
    fn canon(&self) -> Value {
        Value::String(self.as_str().into())
    }
}

py_model! {
    pub struct Inner {
        pub code: String,
        pub qty: i64 = 1 => v::int_c(NumC::new().ge("1")),
    }
}
canon_struct!(Inner { code, qty });

py_model! {
    pub struct Item {
        pub name: String => v::str_c(StrC::new().min_length(1).max_length(10)),
        pub count: i64,
        pub ratio: f64 = 0.0,
        pub active: bool = true,
        pub price: Decimal,
        pub amount: Option<Decimal> = None => v::nullable(v::decimal_c(DecC::new().ge("0"))),
        pub pct: i64 = 0 => v::int_c(NumC::new().ge("0").le("100")),
        pub rate: Option<Decimal> = None => v::nullable(v::decimal_c(DecC::new().gt("0").max_digits(5).decimal_places(2))),
        pub id: Option<Uuid> = None,
        pub day: Option<NaiveDate> = None,
        pub at: Option<DateTimeIn> = None,
        pub email: Option<EmailStr> = None,
        pub kind: KindAbc = KindAbc::A,
        pub one: Option<OneX> = None,
        pub note: Option<String> = None,
        pub code: String = "AB" => v::str_c(StrC::new().pattern("^[A-Z]+$")),
        pub tags: Vec<String> = Vec::new(),
        pub nums: Vec<i64> = Vec::new() => v::list_c::<i64>(ListC::new().max_length(3)),
        pub inner: Option<Inner> = None,
        pub inners: Vec<Inner> = Vec::new(),
        pub meta: Option<Map<String, Value>> = None,
        pub labels: Option<IndexMap<String, String>> = None,
        pub anything: Value = Value::Null,
    }
}
canon_struct!(Item {
    name,
    count,
    ratio,
    active,
    price,
    amount,
    pct,
    rate,
    id,
    day,
    at,
    email,
    kind,
    one,
    note,
    code,
    tags,
    nums,
    inner,
    inners,
    meta,
    labels,
    anything
});

py_model! {
    forbid_extra;
    pub struct Strict {
        pub a: i64,
        pub b: String = "x",
    }
}
canon_struct!(Strict { a, b });

py_model! {
    pub struct Q {
        pub limit: i64 = 50 => v::int_c(NumC::new().ge("1").le("200")),
        pub q: Option<String> = None,
        pub day: Option<NaiveDate> = None,
        pub flag: bool = false,
        pub kind: Option<KindXy> = None,
        pub uid: Option<Uuid> = None,
        pub amount: Option<Decimal> = None,
    }
}
canon_struct!(Q {
    limit,
    q,
    day,
    flag,
    kind,
    uid,
    amount
});

py_model! {
    pub struct P {
        pub item_id: Uuid,
        pub n: i64,
    }
}
canon_struct!(P { item_id, n });

py_model! {
    pub struct PqPath { pub n: i64 }
}
py_model! {
    pub struct PqQuery { pub limit: i64 => v::int_c(NumC::new().ge("1")) }
}

fn ok(v: Value) -> Response {
    granary_core::errors::json_response(StatusCode::OK, &json!({"ok": true, "v": v}))
}

async fn err(Path(kind): Path<String>) -> Result<Response, ApiError> {
    Err(match kind.as_str() {
        "notfound" => ApiError::not_found("Invoice", Some("INV-1")),
        "notfound_noid" => ApiError::not_found("Invoice", None),
        "conflict" => ApiError::conflict("Already exists"),
        "forbidden" => ApiError::forbidden(None),
        "forbidden_msg" => ApiError::forbidden(Some("Nope")),
        "validation" => ApiError::validation("Bad value"),
        "business" => ApiError::business_rule("Rule broken"),
        "http" => ApiError::http(400, "Plain message"),
        "http_dict" => ApiError::http_json(409, json!({"message": "x", "items": [1, 2]})),
        "http_headers" => {
            ApiError::http(401, "Not authenticated").with_header("www-authenticate", "Bearer")
        }
        "http_none" => ApiError::http_default(404),
        "unique" => {
            ApiError::Integrity("duplicate key value violates unique constraint \"x\"".into())
        }
        "fk" => ApiError::Integrity("insert violates foreign key constraint".into()),
        "other_integrity" => {
            ApiError::Integrity("null value in column violates not-null constraint".into())
        }
        "crash" => panic!("boom"),
        _ => return Ok(ok(json!(null))),
    })
}

fn state() -> AppState {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .unwrap();
    let mut env = std::collections::HashMap::new();
    for (k, val) in [
        ("app_secret_key", "s"),
        ("database_url", "postgres://x"),
        ("postgres_password", "p"),
        ("jwt_secret_key", "0123456789abcdef0123456789abcdef"),
        ("superadmin_password", "p"),
        ("minio_secret_key", "m"),
    ] {
        env.insert(k.to_string(), val.to_string());
    }
    let settings = Arc::new(granary_core::config::Settings::from_map(&env).unwrap());
    AppState {
        db: pool,
        redis: Arc::new(granary_core::state::Redis::open("redis://127.0.0.1:1/").unwrap()),
        settings,
    }
}

fn app() -> Router {
    let state = state();
    let router = Router::new()
        .route(
            "/item",
            post(|PyJson(b): PyJson<Item>| async move { ok(b.canon()) }),
        )
        .route(
            "/strict",
            post(|PyJson(b): PyJson<Strict>| async move { ok(b.canon()) }),
        )
        .route(
            "/raw",
            post(|PyJson(b): PyJson<Map<String, Value>>| async move { ok(b.canon()) }),
        )
        .route(
            "/q",
            get(|PyQuery(q): PyQuery<Q>| async move { ok(q.canon()) }),
        )
        .route(
            "/p/{item_id}/{n}",
            get(|PyPath(p): PyPath<P>| async move { ok(p.canon()) }),
        )
        .route(
            "/pq/{n}",
            post(|inputs: Inputs| async move {
                let (p, q, b) = inputs.validate::<PqPath, PqQuery, Strict>()?;
                Ok::<_, ApiError>(ok(
                    json!({"n": p.n.canon(), "limit": q.limit.canon(), "body": b.canon()}),
                ))
            }),
        )
        .route("/err/{kind}", get(err));
    let _ = NoParams;
    granary_core::http::finish_app(router, state, granary_core::http::Cors::new(vec![]))
}

#[tokio::test]
async fn golden_http_cases() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../compat-tests/golden/validation/http_cases.json"
    );
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let app = app();
    let mut failures = Vec::new();
    let mut deviations_seen = 0;
    for c in &cases {
        let name = c["name"].as_str().unwrap();
        let mut req = Request::builder()
            .method(c["method"].as_str().unwrap())
            .uri(c["path"].as_str().unwrap());
        if let Some(ct) = c["content_type"].as_str() {
            req = req.header("content-type", ct);
        }
        let body = match c["body"].as_str() {
            Some(b) => Body::from(b.to_string()),
            None => Body::empty(),
        };
        let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let bytes = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes();
        let got = String::from_utf8_lossy(&bytes).to_string();
        let want = c["response"].as_str().unwrap();
        let want_status = c["status"].as_u64().unwrap() as u16;
        let mut problems = Vec::new();
        if status != want_status {
            problems.push(format!("status {status} != {want_status}"));
        }
        if got != want {
            problems.push(format!("body\n   got  {got}\n   want {want}"));
        }
        for (k, v) in c["headers"].as_object().unwrap() {
            let h = headers.get(k.as_str()).and_then(|h| h.to_str().ok());
            if h != v.as_str() {
                problems.push(format!("header {k}: {h:?} != {v}"));
            }
        }
        let deviation = KNOWN_DEVIATIONS.iter().any(|(n, _)| *n == name);
        if deviation {
            deviations_seen += 1;
            continue;
        }
        if !problems.is_empty() {
            failures.push(format!("{name}: {}", problems.join("; ")));
        }
    }
    assert_eq!(deviations_seen, KNOWN_DEVIATIONS.len());
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[tokio::test]
async fn golden_cors_cases() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../compat-tests/golden/validation/cors_cases.json"
    );
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut env_settings = (*state().settings).clone();
    env_settings.CORS_ORIGINS = vec!["http://localhost:3000".into(), "https://example.com".into()];
    let router = Router::new()
        .route("/health", get(granary_core::http::health))
        .route("/err/{kind}", get(err));
    let app = granary_core::http::finish_app(
        router,
        state(),
        granary_core::http::Cors::new(env_settings.cors_allow_origins()),
    );
    let mut failures = Vec::new();
    for c in &cases {
        let mut req = Request::builder()
            .method(c["method"].as_str().unwrap())
            .uri(c["path"].as_str().unwrap());
        for (k, v) in c["request_headers"].as_object().unwrap() {
            req = req.header(k.as_str(), v.as_str().unwrap());
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let bytes = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes();
        let mut problems = Vec::new();
        if status as u64 != c["status"].as_u64().unwrap() {
            problems.push(format!("status {status}"));
        }
        if String::from_utf8_lossy(&bytes) != c["response"].as_str().unwrap() {
            problems.push(format!("body {}", String::from_utf8_lossy(&bytes)));
        }
        let want = c["headers"].as_object().unwrap();
        for name in [
            "access-control-allow-origin",
            "access-control-allow-credentials",
            "access-control-allow-methods",
            "access-control-allow-headers",
            "access-control-max-age",
            "vary",
            "content-type",
            "allow",
        ] {
            let got = headers.get(name).and_then(|h| h.to_str().ok());
            if got != want.get(name).and_then(|v| v.as_str()) {
                problems.push(format!("{name}: {got:?} != {:?}", want.get(name)));
            }
        }
        if !problems.is_empty() {
            failures.push(format!("{}: {}", c["name"], problems.join("; ")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
