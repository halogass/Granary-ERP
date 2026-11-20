//! backend/app/core/settings_store.py: business numbers and rates come from `system_settings` rows,
//! never from code. A missing or malformed value is a 400 naming the setting and where to fix it.

use std::collections::HashMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::PgConnection;

use crate::errors::ApiError;
use crate::validation::types::PyDec;
use crate::validation::{json_loads, PyInt, PyObj};

/// `SettingError(key, problem, where, location=...)`: HTTP 400,
/// "Setting '{key}' {problem}. Set it in {place}." where place is `location` or "Settings → {where}".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingError {
    pub key: String,
    pub detail: String,
}

impl SettingError {
    pub fn new(key: &str, problem: &str, where_: &str) -> SettingError {
        SettingError::at(key, problem, &format!("Settings → {where_}"))
    }

    /// `location=`: a full path from the sidebar for values kept outside Settings.
    pub fn at(key: &str, problem: &str, location: &str) -> SettingError {
        SettingError {
            key: key.to_string(),
            detail: format!("Setting '{key}' {problem}. Set it in {location}."),
        }
    }
}

impl From<SettingError> for ApiError {
    fn from(e: SettingError) -> ApiError {
        ApiError::http(400, e.detail)
    }
}

impl std::fmt::Display for SettingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for SettingError {}

/// Errors from these helpers: the setting problem itself, or the database failing.
#[derive(Debug)]
pub enum StoreError {
    Setting(SettingError),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> StoreError {
        StoreError::Db(e)
    }
}

impl From<SettingError> for StoreError {
    fn from(e: SettingError) -> StoreError {
        StoreError::Setting(e)
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> ApiError {
        match e {
            StoreError::Setting(s) => s.into(),
            StoreError::Db(d) => d.into(),
        }
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// `_values(db, keys)`: key -> raw value (NULL values present as None).
pub async fn values(
    db: &mut PgConnection,
    keys: &[&str],
) -> Result<HashMap<String, Option<String>>, sqlx::Error> {
    let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT key, value FROM system_settings WHERE key = ANY($1)")
            .bind(&keys)
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().collect())
}

/// `_require(values, key, where)`: the stripped value, or "is not set".
pub fn require_from(
    values: &HashMap<String, Option<String>>,
    key: &str,
    where_: &str,
) -> Result<String, SettingError> {
    match values.get(key) {
        Some(Some(raw)) if !raw.trim().is_empty() => Ok(raw.trim().to_string()),
        _ => Err(SettingError::new(key, "is not set", where_)),
    }
}

/// `_decimal(raw, key, where)`.
pub fn parse_decimal(raw: &str, key: &str, where_: &str) -> Result<Decimal, SettingError> {
    let not_a_number = || SettingError::new(key, &format!("is not a number ('{raw}')"), where_);
    // Python accepts NaN/Infinity here and then fails comparing them; both are refused as not a number.
    let value = PyDec::parse(raw)
        .and_then(|d| d.to_decimal())
        .ok_or_else(not_a_number)?;
    if value < Decimal::ZERO {
        return Err(SettingError::new(key, "can't be negative", where_));
    }
    Ok(value)
}

/// `_json(raw, key, where)`.
pub fn parse_json(raw: &str, key: &str, where_: &str) -> Result<PyObj, SettingError> {
    json_loads(raw.as_bytes()).map_err(|_| SettingError::new(key, "is not valid JSON", where_))
}

/// `require_decimals(db, keys, where)`: all as Decimals, or the first missing/malformed one.
pub async fn require_decimals(
    db: &mut PgConnection,
    keys: &[&str],
    where_: &str,
) -> StoreResult<indexmap::IndexMap<String, Decimal>> {
    let vals = values(db, keys).await?;
    let mut out = indexmap::IndexMap::new();
    for k in keys {
        let raw = require_from(&vals, k, where_)?;
        out.insert(k.to_string(), parse_decimal(&raw, k, where_)?);
    }
    Ok(out)
}

pub async fn require_decimal(
    db: &mut PgConnection,
    key: &str,
    where_: &str,
) -> StoreResult<Decimal> {
    Ok(require_decimals(db, &[key], where_).await?[key])
}

pub async fn require_int(db: &mut PgConnection, key: &str, where_: &str) -> StoreResult<i64> {
    let value = require_decimal(db, key, where_).await?;
    if value != value.trunc() {
        return Err(SettingError::new(key, "must be a whole number", where_).into());
    }
    i64::try_from(value.trunc()).map_err(|_| {
        StoreError::Setting(SettingError::new(
            key,
            &format!("is not a number ('{value}')"),
            where_,
        ))
    })
}

/// `require_json`: the decoded value as Python sees it.
pub async fn require_json_obj(
    db: &mut PgConnection,
    key: &str,
    where_: &str,
) -> StoreResult<PyObj> {
    let vals = values(db, &[key]).await?;
    Ok(parse_json(&require_from(&vals, key, where_)?, key, where_)?)
}

/// `require_json` as serde_json.
pub async fn require_json(
    db: &mut PgConnection,
    key: &str,
    where_: &str,
) -> StoreResult<serde_json::Value> {
    Ok(require_json_obj(db, key, where_).await?.to_json())
}

/// `get_optional`: the stripped value, or None when unset or blank.
pub async fn get_optional(db: &mut PgConnection, key: &str) -> Result<Option<String>, sqlx::Error> {
    let vals = values(db, &[key]).await?;
    Ok(match vals.get(key) {
        Some(Some(raw)) if !raw.trim().is_empty() => Some(raw.trim().to_string()),
        _ => None,
    })
}

/// `date.fromisoformat` for the forms a setting holds (YYYY-MM-DD, YYYYMMDD).
pub fn py_date_fromisoformat(s: &str) -> Option<NaiveDate> {
    if s.len() == 10 && s.as_bytes()[4] == b'-' && s.as_bytes()[7] == b'-' {
        let all_digits = s
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
        if !all_digits {
            return None;
        }
        return NaiveDate::from_ymd_opt(
            s[0..4].parse().ok()?,
            s[5..7].parse().ok()?,
            s[8..10].parse().ok()?,
        );
    }
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        return NaiveDate::from_ymd_opt(
            s[0..4].parse().ok()?,
            s[4..6].parse().ok()?,
            s[6..8].parse().ok()?,
        );
    }
    None
}

/// `optional_date`: a YYYY-MM-DD setting, or None when blank.
pub async fn optional_date(
    db: &mut PgConnection,
    key: &str,
    where_: &str,
) -> StoreResult<Option<NaiveDate>> {
    let Some(raw) = get_optional(db, key).await? else {
        return Ok(None);
    };
    match py_date_fromisoformat(&raw) {
        Some(d) => Ok(Some(d)),
        None => {
            Err(
                SettingError::new(key, &format!("is not a date (YYYY-MM-DD): '{raw}'"), where_)
                    .into(),
            )
        }
    }
}

pub const CURRENCY_DECIMALS_KEY: &str = "currency.decimals";

/// Python `int(x)` on a decoded JSON value; None where Python raises TypeError/ValueError.
fn py_int(v: &PyObj) -> Option<i64> {
    match v {
        PyObj::Int(PyInt::Small(i)) => Some(*i),
        PyObj::Bool(b) => Some(*b as i64),
        PyObj::Float(f) if f.is_finite() => Some(f.trunc() as i64),
        PyObj::Str(s) => {
            let t = s.trim();
            let (neg, body) = match t.as_bytes().first() {
                Some(b'-') => (true, &t[1..]),
                Some(b'+') => (false, &t[1..]),
                _ => (false, t),
            };
            let ok = !body.is_empty()
                && !body.starts_with('_')
                && !body.ends_with('_')
                && !body.contains("__")
                && body.chars().all(|c| c.is_ascii_digit() || c == '_');
            if !ok {
                return None;
            }
            let n: i64 = body.replace('_', "").parse().ok()?;
            Some(if neg { -n } else { n })
        }
        _ => None,
    }
}

/// `currency_places_from(table, currency)`: the pure part of `currency_places`.
pub fn currency_places_from(table: &PyObj, currency: Option<&str>) -> Result<u32, SettingError> {
    let where_ = "Money & tax → Accounting";
    let code = currency.unwrap_or("").to_uppercase();
    let entry = match table {
        PyObj::Dict(d) => d.get(&code),
        _ => None,
    };
    let Some(entry) = entry else {
        let what = if code.is_empty() {
            "this currency".to_string()
        } else {
            code.clone()
        };
        return Err(SettingError::new(
            CURRENCY_DECIMALS_KEY,
            &format!("has no entry for {what}"),
            where_,
        ));
    };
    let Some(places) = py_int(entry) else {
        return Err(SettingError::new(
            CURRENCY_DECIMALS_KEY,
            &format!("has a non-integer entry for {code}"),
            where_,
        ));
    };
    if places < 0 {
        return Err(SettingError::new(
            CURRENCY_DECIMALS_KEY,
            &format!("has a negative entry for {code}"),
            where_,
        ));
    }
    Ok(places as u32)
}

/// `currency_places(db, currency)`: minor-unit digits (JPY 0, USD 2) from `currency.decimals`.
pub async fn currency_places(db: &mut PgConnection, currency: Option<&str>) -> StoreResult<u32> {
    let table = require_json_obj(db, CURRENCY_DECIMALS_KEY, "Money & tax → Accounting").await?;
    Ok(currency_places_from(&table, currency)?)
}

/// A setting stored with `is_encrypted = true`, decrypted with the APP_SECRET_KEY Fernet key like
/// `email_crypto.decrypt(s.value)`. Unset or blank gives None; a value that does not decrypt is an
/// error (Python raises InvalidToken there).
pub async fn get_secret(
    db: &mut PgConnection,
    key: &str,
    app_secret_key: &str,
) -> Result<Option<String>, ApiError> {
    let row: Option<(Option<String>, Option<bool>)> =
        sqlx::query_as("SELECT value, is_encrypted FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(db)
            .await?;
    let Some((Some(value), encrypted)) = row else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if encrypted.unwrap_or(false) {
        return granary_compat::crypto::fernet::decrypt(app_secret_key, &value)
            .map(Some)
            .map_err(|e| {
                ApiError::internal(anyhow::anyhow!(
                    "InvalidToken decrypting setting {key}: {e:?}"
                ))
            });
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages() {
        let e = SettingError::new("tax.vat_rate", "is not set", "Money & tax → Accounting");
        assert_eq!(
            e.detail,
            "Setting 'tax.vat_rate' is not set. Set it in Settings → Money & tax → Accounting."
        );
        let api: ApiError = e.into();
        assert_eq!(api.status().as_u16(), 400);
        assert_eq!(
            serde_json::to_string(&api.body()).unwrap(),
            r#"{"detail":"Setting 'tax.vat_rate' is not set. Set it in Settings → Money & tax → Accounting."}"#
        );
        let e = SettingError::at(
            "x",
            "is not set",
            "Finance → Chart of Accounts → Account mapping",
        );
        assert_eq!(
            e.detail,
            "Setting 'x' is not set. Set it in Finance → Chart of Accounts → Account mapping."
        );
    }

    #[test]
    fn parsing() {
        let mut vals = HashMap::new();
        vals.insert("a".to_string(), Some("  ".to_string()));
        vals.insert("b".to_string(), None);
        assert_eq!(
            require_from(&vals, "a", "W").unwrap_err().detail,
            "Setting 'a' is not set. Set it in Settings → W."
        );
        assert!(require_from(&vals, "b", "W").is_err());
        assert!(require_from(&vals, "c", "W").is_err());
        assert_eq!(
            parse_decimal("1_000.50", "k", "W").unwrap().to_string(),
            "1000.50"
        );
        assert_eq!(
            parse_decimal("abc", "k", "W").unwrap_err().detail,
            "Setting 'k' is not a number ('abc'). Set it in Settings → W."
        );
        assert_eq!(
            parse_decimal("-1", "k", "W").unwrap_err().detail,
            "Setting 'k' can't be negative. Set it in Settings → W."
        );
        assert_eq!(
            parse_json("{x", "k", "W").unwrap_err().detail,
            "Setting 'k' is not valid JSON. Set it in Settings → W."
        );
        assert_eq!(
            py_date_fromisoformat("2026-02-28"),
            NaiveDate::from_ymd_opt(2026, 2, 28)
        );
        assert_eq!(
            py_date_fromisoformat("20260228"),
            NaiveDate::from_ymd_opt(2026, 2, 28)
        );
        assert_eq!(py_date_fromisoformat("2026-2-28"), None);
    }

    #[test]
    fn currency_places_table() {
        let t =
            json_loads(br#"{"JPY": 0, "USD": "2", "EUR": 2.9, "BAD": "x", "NEG": -1}"#).unwrap();
        assert_eq!(currency_places_from(&t, Some("jpy")).unwrap(), 0);
        assert_eq!(currency_places_from(&t, Some("USD")).unwrap(), 2);
        assert_eq!(currency_places_from(&t, Some("EUR")).unwrap(), 2);
        assert_eq!(
            currency_places_from(&t, None).unwrap_err().detail,
            "Setting 'currency.decimals' has no entry for this currency. Set it in Settings → Money & tax → Accounting."
        );
        assert_eq!(
            currency_places_from(&t, Some("GBP")).unwrap_err().detail,
            "Setting 'currency.decimals' has no entry for GBP. Set it in Settings → Money & tax → Accounting."
        );
        assert_eq!(
            currency_places_from(&t, Some("BAD")).unwrap_err().detail,
            "Setting 'currency.decimals' has a non-integer entry for BAD. Set it in Settings → Money & tax → Accounting."
        );
        assert_eq!(
            currency_places_from(&t, Some("NEG")).unwrap_err().detail,
            "Setting 'currency.decimals' has a negative entry for NEG. Set it in Settings → Money & tax → Accounting."
        );
        let list = json_loads(b"[1]").unwrap();
        assert!(currency_places_from(&list, Some("JPY")).is_err());
    }
}
