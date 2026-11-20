//! Port of backend/app/services/fx.py: exchange rate lookup and ExchangeRate-API auto-sync.
//!
//! Every rate is a Decimal. Python divides with the default decimal context (28 significant digits,
//! ROUND_HALF_EVEN) and then quantizes to RATE_QUANTUM (12 places) with the same context rounding, so
//! `py_div_quantize` reproduces that double rounding exactly (rust_decimal's own division does not).

use std::sync::RwLock;

use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::clock::today_local;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::validation::{json_loads, py_float_repr, PyObj};

/// `RATE_QUANTUM`: ExchangeRate.rate is Numeric(24,12).
pub const RATE_PLACES: u32 = 12;

pub const SUPPORTED_CURRENCIES: [&str; 7] = ["USD", "EUR", "GBP", "JPY", "SGD", "AUD", "CNY"];

/// The production ExchangeRate-API host. Tests may point the fetch elsewhere with
/// `set_api_base_for_tests`; nothing in production sets it.
const API_BASE: &str = "https://v6.exchangerate-api.com";

static API_BASE_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);
static API_KEY_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);

/// Test hook only (wiremock): replaces `https://v6.exchangerate-api.com` in the fetch URL.
#[doc(hidden)]
pub fn set_api_base_for_tests(base: Option<&str>) {
    *API_BASE_OVERRIDE.write().unwrap() = base.map(|s| s.trim_end_matches('/').to_string());
}

/// Test hook only: stands in for `cfg.EXCHANGERATE_API_KEY` (Python tests monkeypatch the setting).
/// `Some("")` means "the env key is blank".
#[doc(hidden)]
pub fn set_env_api_key_for_tests(key: Option<&str>) {
    *API_KEY_OVERRIDE.write().unwrap() = key.map(|s| s.to_string());
}

fn api_base() -> String {
    API_BASE_OVERRIDE
        .read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| API_BASE.to_string())
}

/// The ExchangeRate-API host in effect (`https://v6.exchangerate-api.com`, or the test override); used by
/// routers/system.py's `/currencies/test-api-key`.
pub fn exchangerate_api_base() -> String {
    api_base()
}

fn env_api_key() -> String {
    if let Some(k) = API_KEY_OVERRIDE.read().unwrap().clone() {
        return k;
    }
    granary_core::config::settings()
        .EXCHANGERATE_API_KEY
        .clone()
}

// ─── Python Decimal division ─────────────────────────────────────────────────

const PREC: usize = 28;

/// Rounds a digit string (most significant first) to `keep` digits, ROUND_HALF_EVEN, with `sticky`
/// telling whether anything non-zero lies beyond the given digits. Returns the kept digits and how
/// many positions the value shifted left (1 when a carry added a digit that was then dropped).
fn round_half_even(digits: &[u8], keep: usize, sticky: bool) -> (Vec<u8>, usize) {
    if digits.len() <= keep {
        return (digits.to_vec(), 0);
    }
    let mut kept: Vec<u8> = digits[..keep].to_vec();
    let guard = digits[keep];
    let rest_nonzero = sticky || digits[keep + 1..].iter().any(|d| *d != 0);
    let last_odd = kept.last().map(|d| d % 2 == 1).unwrap_or(false);
    let up = guard > 5 || (guard == 5 && (rest_nonzero || last_odd));
    if up {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, 1);
                break;
            }
            i -= 1;
            if kept[i] == 9 {
                kept[i] = 0;
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    (kept, 0)
}

/// `(a / b).quantize(Decimal(1).scaleb(-places))` under Python's default context (prec 28,
/// ROUND_HALF_EVEN for both steps). Err when b is zero (DivisionByZero) or the quantized result needs
/// more than 28 digits (InvalidOperation).
pub fn py_div_quantize(a: Decimal, b: Decimal, places: u32) -> Result<Decimal, String> {
    if b.is_zero() {
        return Err("decimal.DivisionByZero".into());
    }
    let neg = a.is_sign_negative() != b.is_sign_negative() && !a.is_zero();
    if a.is_zero() {
        let mut z = Decimal::ZERO;
        z.rescale(places);
        return Ok(z);
    }
    let ma = a.mantissa().unsigned_abs();
    let mb = b.mantissa().unsigned_abs();
    // value = (ma / mb) * 10^(sb - sa)
    let base_exp: i64 = b.scale() as i64 - a.scale() as i64;
    let q = ma / mb;
    let mut r = ma % mb;
    // All digits produced; exponent of the last digit.
    let mut digits: Vec<u8> = if q == 0 {
        vec![]
    } else {
        q.to_string().bytes().map(|c| c - b'0').collect()
    };
    let mut frac: i64 = 0;
    let sig = |d: &Vec<u8>| d.iter().skip_while(|x| **x == 0).count();
    while r != 0 && sig(&digits) < PREC + 1 && frac < 400 {
        r *= 10;
        digits.push((r / mb) as u8);
        r %= mb;
        frac += 1;
    }
    let sticky = r != 0;
    // Drop leading zeros (they carry no significance).
    let first = digits.iter().position(|d| *d != 0).unwrap_or(digits.len());
    let digits: Vec<u8> = digits[first..].to_vec();
    if digits.is_empty() {
        // Quotient underflowed our window: rounds to zero at any sane number of places.
        let mut z = Decimal::ZERO;
        z.rescale(places);
        return Ok(if neg { -z } else { z });
    }
    // value = int(digits) * 10^exp
    let mut exp: i64 = base_exp - frac;
    // Step 1: round to 28 significant digits.
    let (mut n, _) = if digits.len() > PREC {
        let drop = digits.len() - PREC;
        exp += drop as i64;
        round_half_even(&digits, PREC, sticky)
    } else {
        (digits.clone(), 0)
    };
    if n.len() > PREC {
        // carry made 29 digits: 10^28 -> drop the trailing zero
        n.pop();
        exp += 1;
    }
    // Step 2: quantize to exponent -places (half-even on the 28-digit value, no sticky left).
    let target = -(places as i64);
    let mut out: Vec<u8> = if exp >= target {
        let mut v = n.clone();
        v.extend(std::iter::repeat_n(0, (exp - target) as usize));
        v
    } else {
        let drop = (target - exp) as usize;
        if drop > n.len() {
            vec![0]
        } else if drop == n.len() {
            // everything is beyond the quantum: round 0.<digits>
            let mut padded = vec![0u8];
            padded.extend(n.iter());
            let (k, _) = round_half_even(&padded, 1, false);
            k
        } else {
            let keep = n.len() - drop;
            let (k, _) = round_half_even(&n, keep, false);
            k
        }
    };
    let first = out.iter().position(|d| *d != 0).unwrap_or(out.len());
    out.drain(..first);
    if out.len() > PREC {
        return Err("decimal.InvalidOperation: quantize result has too many digits".into());
    }
    let mut m: i128 = 0;
    for d in &out {
        m = m * 10 + *d as i128;
    }
    if neg {
        m = -m;
    }
    Ok(Decimal::from_i128_with_scale(m, places))
}

/// `Decimal(str(v))` for a number decoded by Python's json: integer literals stay ints, anything with
/// a fraction or exponent is a float first (so `1e-05` becomes `0.00001`, `16000.50` becomes `16000.5`).
pub fn py_json_number_decimal(v: &Value) -> Option<Decimal> {
    match v {
        Value::Number(n) => {
            let lit = n.to_string();
            if lit.contains(['.', 'e', 'E']) {
                let f: f64 = lit.parse().ok()?;
                if !f.is_finite() {
                    return None;
                }
                let r = py_float_repr(f);
                r.parse::<Decimal>()
                    .ok()
                    .or_else(|| Decimal::from_scientific(&r).ok())
            } else {
                lit.parse::<Decimal>().ok()
            }
        }
        Value::String(s) => s.trim().parse::<Decimal>().ok(),
        _ => None,
    }
}

// ─── Rates ───────────────────────────────────────────────────────────────────

/// Why `get_rate` failed: Python's ValueError (no rate) or anything else that propagated.
#[derive(Debug)]
pub enum RateError {
    /// `ValueError("No exchange rate found for X/Y on or before D")`.
    NoRate(String),
    Other(ApiError),
}

impl From<RateError> for ApiError {
    /// An uncaught ValueError is a 500.
    fn from(e: RateError) -> ApiError {
        match e {
            RateError::NoRate(m) => ApiError::internal(anyhow::anyhow!("ValueError: {m}")),
            RateError::Other(a) => a,
        }
    }
}

impl From<sqlx::Error> for RateError {
    fn from(e: sqlx::Error) -> RateError {
        RateError::Other(e.into())
    }
}

impl From<granary_core::settings_store::StoreError> for RateError {
    fn from(e: granary_core::settings_store::StoreError) -> RateError {
        RateError::Other(e.into())
    }
}

/// `get_rate(db, from_cur, to_cur, on_date)`: the rate on or before `on_date` (default: today in the
/// company time zone), else the reciprocal of the inverse rate (12 places), else `RateError::NoRate`.
pub async fn get_rate(
    db: &mut PgConnection,
    from_cur: &str,
    to_cur: &str,
    on_date: Option<NaiveDate>,
) -> Result<Decimal, RateError> {
    let on_date = match on_date {
        Some(d) => d,
        None => today_local(db).await?,
    };
    let from_cur = from_cur.to_uppercase();
    let to_cur = to_cur.to_uppercase();
    if from_cur == to_cur {
        return Ok(Decimal::ONE);
    }
    let sql = "SELECT rate FROM exchange_rates WHERE from_currency = $1 AND to_currency = $2 \
               AND is_deleted = false AND effective_date <= $3 ORDER BY effective_date DESC LIMIT 1";
    let row: Option<(Decimal,)> = sqlx::query_as(sql)
        .bind(&from_cur)
        .bind(&to_cur)
        .bind(on_date)
        .fetch_optional(&mut *db)
        .await?;
    if let Some((rate,)) = row {
        return Ok(rate);
    }
    // Try inverse rate and compute reciprocal
    let row2: Option<(Decimal,)> = sqlx::query_as(sql)
        .bind(&to_cur)
        .bind(&from_cur)
        .bind(on_date)
        .fetch_optional(&mut *db)
        .await?;
    if let Some((rate,)) = row2 {
        if !rate.is_zero() {
            // 12 places, matching ExchangeRate.rate Numeric(24,12): 1/16000 = 0.0000625 must not
            // round to 0.000063.
            return py_div_quantize(Decimal::ONE, rate, RATE_PLACES)
                .map_err(|e| RateError::Other(ApiError::internal(anyhow::anyhow!(e))));
        }
    }
    Err(RateError::NoRate(format!(
        "No exchange rate found for {from_cur}/{to_cur} on or before {}",
        on_date.format("%Y-%m-%d")
    )))
}

/// `require_rate`: get_rate, or an HTTP 400 telling the user to record the rate. A silent 1.0 would
/// value a USD amount as 1 JPY per dollar in every report, so no caller may fall back to one.
pub async fn require_rate(
    db: &mut PgConnection,
    from_cur: &str,
    to_cur: &str,
    on_date: Option<NaiveDate>,
) -> ApiResult<Decimal> {
    match get_rate(db, from_cur, to_cur, on_date).await {
        Ok(r) => Ok(r),
        Err(RateError::NoRate(_)) => {
            let when = match on_date {
                Some(d) => d,
                None => today_local(db).await?,
            };
            Err(ApiError::http(
                400,
                format!(
                    "No {}/{} exchange rate is on record for {}. \
                     Add it under Finance \u{2192} Exchange Rates, or enter the rate on the document.",
                    from_cur.to_uppercase(),
                    to_cur.to_uppercase(),
                    when.format("%Y-%m-%d")
                ),
            ))
        }
        Err(RateError::Other(e)) => Err(e),
    }
}

/// A settings row as `get_active_currencies` / `get_functional_currency` read it: Python accepts ORM
/// rows (`.key`, `.value`) or dicts (`get("key")`, `get("value", "")`).
pub trait SettingKV {
    fn kv_key(&self) -> Option<&str>;
    fn kv_value(&self) -> Option<&str>;
}

impl SettingKV for granary_db::rows::system::SystemSetting {
    fn kv_key(&self) -> Option<&str> {
        Some(&self.key)
    }
    fn kv_value(&self) -> Option<&str> {
        self.value.as_deref()
    }
}

impl SettingKV for (String, Option<String>) {
    fn kv_key(&self) -> Option<&str> {
        Some(&self.0)
    }
    fn kv_value(&self) -> Option<&str> {
        self.1.as_deref()
    }
}

impl SettingKV for (&str, Option<&str>) {
    fn kv_key(&self) -> Option<&str> {
        Some(self.0)
    }
    fn kv_value(&self) -> Option<&str> {
        self.1
    }
}

impl SettingKV for Value {
    /// A dict: `s.get("key")`; `s.get("value", "")` (a missing value is "", a non-string one is treated
    /// like a missing one here).
    fn kv_key(&self) -> Option<&str> {
        self.get("key").and_then(|v| v.as_str())
    }
    fn kv_value(&self) -> Option<&str> {
        match self.get("value") {
            None => Some(""),
            Some(v) => v.as_str(),
        }
    }
}

/// `functional_currency(db)`.
pub async fn functional_currency(db: &mut PgConnection) -> ApiResult<String> {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT key, value FROM system_settings WHERE key = 'currency.functional'")
            .fetch_all(db)
            .await?;
    Ok(get_functional_currency(&rows))
}

/// `get_active_currencies(settings_list)`: the `currency.active` JSON list upper-cased, else
/// USD/EUR. Python only catches JSONDecodeError/TypeError: a list holding a non-string makes
/// `c.upper()` raise AttributeError, which propagates (a 500); that is the Err here.
pub fn get_active_currencies<S: SettingKV>(settings_list: &[S]) -> ApiResult<Vec<String>> {
    for s in settings_list {
        if s.kv_key() == Some("currency.active") {
            let val = s.kv_value().unwrap_or("");
            let val = if val.is_empty() { "[]" } else { val };
            if let Ok(PyObj::List(items)) = json_loads(val.as_bytes()) {
                if !items.is_empty() {
                    let mut out = Vec::with_capacity(items.len());
                    for c in &items {
                        match c {
                            PyObj::Str(c) => out.push(c.to_uppercase()),
                            _ => {
                                return Err(ApiError::internal(anyhow::anyhow!(
                                    "AttributeError: currency.active holds a non-string entry"
                                )))
                            }
                        }
                    }
                    return Ok(out);
                }
            }
        }
    }
    Ok(vec!["USD".into(), "EUR".into()])
}

/// `get_functional_currency(settings_list)`.
pub fn get_functional_currency<S: SettingKV>(settings_list: &[S]) -> String {
    for s in settings_list {
        if s.kv_key() == Some("currency.functional") {
            if let Some(val) = s.kv_value() {
                if !val.is_empty() {
                    return val.to_uppercase();
                }
            }
        }
    }
    "USD".into()
}

// ─── ExchangeRate-API sync ───────────────────────────────────────────────────

/// A failed ExchangeRate-API call as text, without its URL: the URL carries the API key (`/v6/{key}/...`), and
/// this text goes to the logs and to API error bodies.
pub fn request_error_text(e: reqwest::Error) -> String {
    e.without_url().to_string()
}

/// `fetch_and_store_rates(db)`: one call to ExchangeRate-API (base USD), cross rates for every active
/// currency pair stored in both directions for today (a soft-deleted row for today is revived), the
/// sync time recorded in `currency.last_synced`, then committed (Python's `db.commit()`). Returns the
/// number of pairs stored; 0 (and nothing written) when no key is configured or the API fails.
pub async fn fetch_and_store_rates(conn: &mut PgConnection) -> ApiResult<i64> {
    // Python's `db.commit()` at the end: on a plain connection this is a real transaction committed
    // here; inside a caller's transaction sqlx makes it a savepoint and the caller commits.
    let mut tx = sqlx::Connection::begin(conn).await?;
    let db: &mut PgConnection = &mut tx;
    let mut api_key = env_api_key();
    if api_key.is_empty() {
        // Fall back to the key stored via Settings UI (may be Fernet-encrypted)
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT value FROM system_settings WHERE key = 'currency.api_key'")
                .fetch_optional(&mut *db)
                .await?;
        if let Some((Some(value),)) = row {
            if !value.is_empty() {
                let secret = granary_core::config::settings().APP_SECRET_KEY.clone();
                // email_crypto.decrypt; a value that does not decrypt is already plaintext
                // (pre-encryption installs).
                api_key = granary_compat::crypto::fernet::decrypt(&secret, &value).unwrap_or(value);
            }
        }
    }
    if api_key.is_empty() {
        tracing::warn!("No ExchangeRate API key configured: skipping FX sync");
        return Ok(0);
    }

    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT key, value FROM system_settings WHERE key IN ('currency.active', 'currency.functional')",
    )
    .fetch_all(&mut *db)
    .await?;
    let active = get_active_currencies(&rows)?;
    // Ensure USD is in the active list so we always have USD as base.
    // Python: list(set(active + ["USD"])) - set order is hash-randomised per process, so the order
    // pairs are written in differs run to run; only the insertion order of the rows depends on it.
    let mut currencies: Vec<String> = Vec::new();
    for c in active.into_iter().chain(std::iter::once("USD".to_string())) {
        if !currencies.contains(&c) {
            currencies.push(c);
        }
    }

    let url = format!("{}/v6/{}/latest/USD", api_base(), api_key);
    let data: Value = {
        let resp = crate::http::client()
            .get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("ExchangeRate-API request failed: {}", request_error_text(e));
                return Ok(0);
            }
        };
        let status = resp.status().as_u16();
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("ExchangeRate-API request failed: {}", request_error_text(e));
                return Ok(0);
            }
        };
        if status != 200 {
            let head: String = text.chars().take(200).collect();
            tracing::error!("ExchangeRate-API returned {}: {}", status, head);
            return Ok(0);
        }
        let data: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("ExchangeRate-API request failed: {}", e);
                return Ok(0);
            }
        };
        // `data.get(...)` on a non-dict raises AttributeError inside the try: logged, 0.
        let Some(obj) = data.as_object() else {
            tracing::error!("ExchangeRate-API request failed: response is not an object");
            return Ok(0);
        };
        if obj.get("result").and_then(|v| v.as_str()) != Some("success") {
            tracing::error!("ExchangeRate-API error: {:?}", obj.get("error-type"));
            return Ok(0);
        }
        data
    };

    // Decimal(str(...)) so the API's figures are not pushed through binary floats. Outside Python's
    // try: a missing/odd conversion_rates raises (500).
    let Some(raw_rates) = data.get("conversion_rates").and_then(|v| v.as_object()) else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "KeyError: 'conversion_rates'"
        )));
    };
    let mut conversion_rates: Vec<(String, Decimal)> = Vec::new();
    for (k, v) in raw_rates {
        let Some(d) = py_json_number_decimal(v) else {
            return Err(ApiError::internal(anyhow::anyhow!(
                "decimal.InvalidOperation: conversion rate {k}"
            )));
        };
        conversion_rates.push((k.clone(), d));
    }
    let get = |c: &str| {
        conversion_rates
            .iter()
            .find(|(k, _)| k == c)
            .map(|(_, v)| *v)
    };
    let today = today_local(db).await?;
    let mut count: i64 = 0;

    // Build cross-rate matrix for active currencies
    let mut pairs: Vec<(String, String, Decimal)> = Vec::new();
    for from_c in &currencies {
        let Some(usd_to_from) = get(from_c) else {
            continue;
        };
        if usd_to_from.is_zero() {
            continue;
        }
        for to_c in &currencies {
            if from_c == to_c {
                continue;
            }
            let Some(usd_to_to) = get(to_c) else { continue };
            // rate from_c -> to_c = (1 / usd_to_from) * usd_to_to
            let rate = py_div_quantize(usd_to_to, usd_to_from, RATE_PLACES)
                .map_err(|e| ApiError::internal(anyhow::anyhow!(e)))?;
            pairs.push((from_c.clone(), to_c.clone(), rate));
        }
    }

    for (from_c, to_c, rate) in &pairs {
        let existing: Option<(Uuid, Decimal, Option<String>, bool)> = sqlx::query_as(
            "SELECT id, rate, source, is_deleted FROM exchange_rates \
             WHERE from_currency = $1 AND to_currency = $2 AND effective_date = $3",
        )
        .bind(from_c)
        .bind(to_c)
        .bind(today)
        .fetch_optional(&mut *db)
        .await?;
        // uq_fx_rate covers (from, to, date) whatever is_deleted says, so a soft-deleted row for
        // today is revived and refreshed; inserting beside it would violate the constraint.
        match existing {
            Some((id, old_rate, old_source, is_deleted)) => {
                // The ORM writes only changed columns (and then updated_at).
                let changed = old_rate != *rate
                    || old_source.as_deref() != Some("exchangerate-api")
                    || is_deleted;
                if changed {
                    sqlx::query(
                        "UPDATE exchange_rates SET rate = $2, source = 'exchangerate-api', \
                         is_deleted = false, deleted_at = CASE WHEN $3 THEN NULL ELSE deleted_at END, \
                         updated_at = now() WHERE id = $1",
                    )
                    .bind(id)
                    .bind(rate)
                    .bind(is_deleted)
                    .execute(&mut *db)
                    .await?;
                }
            }
            None => {
                sqlx::query(
                    "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, effective_date, \
                     source, is_deleted) VALUES ($1, $2, $3, $4, $5, 'exchangerate-api', false)",
                )
                .bind(Uuid::new_v4())
                .bind(from_c)
                .bind(to_c)
                .bind(rate)
                .bind(today)
                .execute(&mut *db)
                .await?;
            }
        }
        count += 1;
    }

    // Record last sync timestamp in system settings
    let now_iso = granary_compat::pyjson::PyDateTime::from_chrono_utc(&Utc::now()).isoformat();
    let sync: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM system_settings WHERE key = 'currency.last_synced'")
            .fetch_optional(&mut *db)
            .await?;
    match sync {
        Some((id,)) => {
            sqlx::query("UPDATE system_settings SET value = $2, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(&now_iso)
                .execute(&mut *db)
                .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO system_settings (id, key, value, value_type, category, label, is_public, \
                 is_encrypted) VALUES ($1, 'currency.last_synced', $2, 'string', 'finance', \
                 'Last FX Sync', false, false)",
            )
            .bind(Uuid::new_v4())
            .bind(&now_iso)
            .execute(&mut *db)
            .await?;
        }
    }

    tx.commit().await?;
    tracing::info!("FX sync complete: {} pairs stored", count);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn division() {
        assert_eq!(
            py_div_quantize(d("1"), d("16000"), 12).unwrap().to_string(),
            "0.000062500000"
        );
        assert_eq!(
            py_div_quantize(d("1"), d("3"), 12).unwrap().to_string(),
            "0.333333333333"
        );
        assert_eq!(
            py_div_quantize(d("2"), d("3"), 12).unwrap().to_string(),
            "0.666666666667"
        );
        assert_eq!(
            py_div_quantize(d("16123.45"), d("0.9"), 12)
                .unwrap()
                .to_string(),
            "17914.944444444444"
        );
        assert_eq!(
            py_div_quantize(d("0.9"), d("1"), 12).unwrap().to_string(),
            "0.900000000000"
        );
    }

    /// Security fix L1: a failed call's text (logged, and the test-key route's 502 body) never carries the key.
    #[tokio::test]
    async fn a_failed_request_never_shows_the_api_key() {
        let err = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get("http://127.0.0.1:9/v6/SECRET-FX-KEY-123/latest/USD")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .unwrap_err();
        assert!(err.to_string().contains("SECRET-FX-KEY-123"), "{err}");
        let text = request_error_text(err);
        assert!(!text.contains("SECRET-FX-KEY-123"), "{text}");
        assert!(!text.is_empty());
    }
}
