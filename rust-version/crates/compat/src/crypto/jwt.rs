//! `app/core/auth.py` JWTs, reproducing python-jose 3.3.0 (`jwt.encode` / `jwt.decode`) byte for byte.
//!
//! Layout jose produces for HS256:
//! - header: `json.dumps({"typ": "JWT", "alg": alg}, separators=(",", ":"), sort_keys=True)`
//!   = `{"alg":"HS256","typ":"JWT"}`
//! - claims: `json.dumps(claims, separators=(",", ":"))`, insertion order, ensure_ascii; datetime
//!   `exp`/`iat`/`nbf` become `timegm(dt.utctimetuple())` (whole seconds, fraction dropped)
//! - each segment base64url without padding; signature HMAC over `header.claims`.
//!
//! Decoding mirrors `jose.jwt.decode(token, key, algorithms=[alg])` with default options, and
//! `decode_token` returns an empty map on any error as the Python does on `JWTError`.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::{Map, Value};

use super::json_min::{self, COMPACT};
use super::pyb64::jose_b64url_decode;
use super::{b64url_nopad, compare_digest_bytes};

/// The JWT settings of `app/config.py` (`JWT_SECRET_KEY`, `JWT_ALGORITHM`, expiry lengths).
#[derive(Debug, Clone)]
pub struct JwtConfig {
    pub secret: String,
    /// `JWT_ALGORITHM`, default `"HS256"`. HS256/HS384/HS512 are supported (jose's HMAC family).
    pub algorithm: String,
    /// `JWT_ACCESS_TOKEN_EXPIRE_MINUTES`, default 60.
    pub access_minutes: i64,
    /// `JWT_REFRESH_TOKEN_EXPIRE_DAYS`, default 30.
    pub refresh_days: i64,
}

impl JwtConfig {
    pub fn new(secret: impl Into<String>) -> Self {
        JwtConfig {
            secret: secret.into(),
            algorithm: "HS256".into(),
            access_minutes: 60,
            refresh_days: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwtError {
    /// `jws.sign` on an algorithm outside the HMAC family (jose raises JWSError / JWKError).
    UnsupportedAlgorithm(String),
    /// Anything jose reports as `JWTError` (bad segments, padding, signature, claims, expiry).
    Invalid(&'static str),
}

fn hmac_sign(alg: &str, key: &[u8], msg: &[u8]) -> Result<Vec<u8>, JwtError> {
    macro_rules! mac {
        ($h:ty) => {{
            let mut m = <Hmac<$h>>::new_from_slice(key).expect("hmac accepts any key length");
            m.update(msg);
            Ok(m.finalize().into_bytes().to_vec())
        }};
    }
    match alg {
        "HS256" => mac!(sha2::Sha256),
        "HS384" => mac!(sha2::Sha384),
        "HS512" => mac!(sha2::Sha512),
        other => Err(JwtError::UnsupportedAlgorithm(other.to_string())),
    }
}

/// `timegm(dt.utctimetuple())`: whole UTC seconds, the fraction dropped (floor).
pub fn int_date(dt: DateTime<Utc>) -> i64 {
    dt.timestamp()
}

/// `jose.jwt.encode(claims, key, algorithm)` for claims already holding int dates.
pub fn encode(
    claims: &Map<String, Value>,
    secret: &str,
    algorithm: &str,
) -> Result<String, JwtError> {
    let header = format!("{{\"alg\":{},\"typ\":\"JWT\"}}", {
        let mut s = String::new();
        json_min::write_str(&mut s, algorithm);
        s
    });
    let header_b64 = b64url_nopad(header.as_bytes());
    let claims_b64 =
        b64url_nopad(json_min::dumps(&Value::Object(claims.clone()), COMPACT).as_bytes());
    let signing_input = format!("{header_b64}.{claims_b64}");
    let sig = hmac_sign(algorithm, secret.as_bytes(), signing_input.as_bytes())?;
    Ok(format!("{signing_input}.{}", b64url_nopad(&sig)))
}

/// `create_access_token(user_id, extra)` at `now`. `extra` is merged with `dict.update` semantics
/// (an existing key keeps its position, a new key is appended).
pub fn create_access_token_at(
    cfg: &JwtConfig,
    user_id: &str,
    extra: Option<&Map<String, Value>>,
    now: DateTime<Utc>,
) -> Result<String, JwtError> {
    let mut claims = Map::new();
    claims.insert("sub".into(), Value::String(user_id.to_string()));
    claims.insert("type".into(), Value::String("access".into()));
    claims.insert("iat".into(), Value::from(int_date(now)));
    claims.insert(
        "exp".into(),
        Value::from(int_date(
            now + chrono::Duration::minutes(cfg.access_minutes),
        )),
    );
    if let Some(extra) = extra {
        for (k, v) in extra {
            claims.insert(k.clone(), v.clone());
        }
    }
    encode(&claims, &cfg.secret, &cfg.algorithm)
}

pub fn create_access_token(
    cfg: &JwtConfig,
    user_id: &str,
    extra: Option<&Map<String, Value>>,
) -> Result<String, JwtError> {
    create_access_token_at(cfg, user_id, extra, Utc::now())
}

/// `create_refresh_token(user_id)` at `now` with the given `jti` (`str(uuid.uuid4())`).
pub fn create_refresh_token_with(
    cfg: &JwtConfig,
    user_id: &str,
    jti: &str,
    now: DateTime<Utc>,
) -> Result<String, JwtError> {
    let mut claims = Map::new();
    claims.insert("sub".into(), Value::String(user_id.to_string()));
    claims.insert("type".into(), Value::String("refresh".into()));
    claims.insert("jti".into(), Value::String(jti.to_string()));
    claims.insert("iat".into(), Value::from(int_date(now)));
    claims.insert(
        "exp".into(),
        Value::from(int_date(now + chrono::Duration::days(cfg.refresh_days))),
    );
    encode(&claims, &cfg.secret, &cfg.algorithm)
}

pub fn create_refresh_token(cfg: &JwtConfig, user_id: &str) -> Result<String, JwtError> {
    create_refresh_token_with(cfg, user_id, &uuid::Uuid::new_v4().to_string(), Utc::now())
}

/// Python `int(x)` on a decoded JSON claim value. `None` where Python raises.
fn py_int(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(*b as i128),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i as i128)
            } else if let Some(u) = n.as_u64() {
                Some(u as i128)
            } else {
                let text = n.to_string();
                if !text.contains(['.', 'e', 'E']) {
                    text.parse::<i128>().ok()
                } else {
                    let f: f64 = text.parse().ok()?;
                    if f.is_finite() {
                        Some(f.trunc() as i128)
                    } else {
                        None
                    }
                }
            }
        }
        Value::String(s) => super::mcp::py_int_str(s).map(|i| i as i128),
        _ => None,
    }
}

/// `jose.jwt.decode(token, secret, algorithms=[algorithm])` with default options, at `now_secs`
/// (jose uses `timegm(datetime.utcnow().utctimetuple())`, whole seconds).
pub fn decode_at(
    token: &str,
    secret: &str,
    algorithm: &str,
    now_secs: i64,
) -> Result<Map<String, Value>, JwtError> {
    let bytes = token.as_bytes();
    // jws._load: rsplit(".", 1) then split(".", 1).
    let dot = bytes
        .iter()
        .rposition(|&b| b == b'.')
        .ok_or(JwtError::Invalid("Not enough segments"))?;
    let (signing_input, crypto_segment) = (&bytes[..dot], &bytes[dot + 1..]);
    let first = signing_input
        .iter()
        .position(|&b| b == b'.')
        .ok_or(JwtError::Invalid("Not enough segments"))?;
    let (header_seg, claims_seg) = (&signing_input[..first], &signing_input[first + 1..]);
    let header_data =
        jose_b64url_decode(header_seg).map_err(|_| JwtError::Invalid("Invalid header padding"))?;
    let header: Value = std::str::from_utf8(&header_data)
        .ok()
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or(JwtError::Invalid("Invalid header string"))?;
    let Value::Object(header) = header else {
        return Err(JwtError::Invalid("Invalid header string"));
    };
    let payload =
        jose_b64url_decode(claims_seg).map_err(|_| JwtError::Invalid("Invalid payload padding"))?;
    let signature = jose_b64url_decode(crypto_segment)
        .map_err(|_| JwtError::Invalid("Invalid crypto padding"))?;

    // jws._verify_signature
    let alg = match header.get("alg") {
        None | Some(Value::Null) => {
            return Err(JwtError::Invalid(
                "No algorithm was specified in the JWS header.",
            ))
        }
        Some(Value::String(s)) if !s.is_empty() => s.as_str(),
        Some(_) => return Err(JwtError::Invalid("The specified alg value is not allowed")),
    };
    if alg != algorithm {
        return Err(JwtError::Invalid("The specified alg value is not allowed"));
    }
    let expected = hmac_sign(alg, secret.as_bytes(), signing_input)
        .map_err(|_| JwtError::Invalid("Invalid or unsupported algorithm"))?;
    if !compare_digest_bytes(&expected, &signature) {
        return Err(JwtError::Invalid("Signature verification failed."));
    }

    let claims: Value = std::str::from_utf8(&payload)
        .ok()
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or(JwtError::Invalid("Invalid payload string"))?;
    let Value::Object(claims) = claims else {
        return Err(JwtError::Invalid(
            "Invalid payload string: must be a json object",
        ));
    };

    // jwt._validate_claims with the default options and no audience/issuer/subject/access_token.
    if let Some(v) = claims.get("iat") {
        py_int(v).ok_or(JwtError::Invalid(
            "Issued At claim (iat) must be an integer.",
        ))?;
    }
    if let Some(v) = claims.get("nbf") {
        let nbf = py_int(v).ok_or(JwtError::Invalid(
            "Not Before claim (nbf) must be an integer.",
        ))?;
        if nbf > now_secs as i128 {
            return Err(JwtError::Invalid("The token is not yet valid (nbf)"));
        }
    }
    if let Some(v) = claims.get("exp") {
        let exp = py_int(v).ok_or(JwtError::Invalid(
            "Expiration Time claim (exp) must be an integer.",
        ))?;
        if exp < now_secs as i128 {
            return Err(JwtError::Invalid("Signature has expired."));
        }
    }
    if claims.contains_key("aud") {
        // With no audience given, any well-formed aud claim fails ("Invalid audience").
        return Err(JwtError::Invalid("Invalid audience"));
    }
    if let Some(v) = claims.get("sub") {
        if !v.is_string() {
            return Err(JwtError::Invalid("Subject must be a string."));
        }
    }
    if let Some(v) = claims.get("jti") {
        if !v.is_string() {
            return Err(JwtError::Invalid("JWT ID must be a string."));
        }
    }
    if claims.contains_key("at_hash") {
        return Err(JwtError::Invalid(
            "No access_token provided to compare against at_hash claim.",
        ));
    }
    Ok(claims)
}

/// `decode_token(token)` at `now_secs`: the claims, or an empty map on any error.
pub fn decode_token_at(cfg: &JwtConfig, token: &str, now_secs: i64) -> Map<String, Value> {
    decode_at(token, &cfg.secret, &cfg.algorithm, now_secs).unwrap_or_default()
}

pub fn decode_token(cfg: &JwtConfig, token: &str) -> Map<String, Value> {
    decode_token_at(cfg, token, Utc::now().timestamp())
}
