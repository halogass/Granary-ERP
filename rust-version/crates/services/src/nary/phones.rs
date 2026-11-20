//! Port of backend/app/services/nary/phones.py: phone numbers for intake (E.164, and what the number says
//! about the customer).
//!
//! The pure functions are `granary_compat::text::phones` (golden-tested against Python phonenumbers 8.13.51),
//! re-exported here; only the two that read settings (`default_country`, `derive_locale`) live in this file.

use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, require_json};
use serde_json::{json, Value};
use sqlx::PgConnection;

pub use granary_compat::text::phones::{
    chat_id_for, clean_name, country_of, country_of_timezone, digits, from_display_name,
    phone_spans_in_text, phones_in_text, pretty, to_e164,
};

pub const WHERE: &str = "Nary & AI";

/// `default_country(db)`: nary.identity.default_country upper-cased, "US" when unset.
pub async fn default_country(db: &mut PgConnection) -> Result<String, sqlx::Error> {
    let raw = get_optional(db, "nary.identity.default_country").await?;
    let c = raw.unwrap_or_default().trim().to_uppercase();
    Ok(if c.is_empty() { "US".to_string() } else { c })
}

/// `derive_locale(db, *, e164, timezone)`: country (phone code first, then the browser time zone) and the
/// quoting currency from nary.intake.currency_by_country ({"*": "USD"}).
/// Returned as the Python dict: {"country", "currency", "timezone", "country_source"} in that order.
pub async fn derive_locale(
    db: &mut PgConnection,
    e164: Option<&str>,
    timezone: Option<&str>,
) -> ApiResult<Value> {
    let by_phone = country_of(e164);
    let country = by_phone.clone().or_else(|| country_of_timezone(timezone));
    let table = require_json(db, "nary.intake.currency_by_country", WHERE).await?;
    // `(table or {}).get(country or "", (table or {}).get("*"))`
    let empty = serde_json::Map::new();
    let map = match &table {
        Value::Object(m) => m,
        v if !crate::whatsapp::py_truthy(v) => &empty,
        // a truthy non-dict: `.get` raises AttributeError (a 500), as in Python
        other => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "'{}' object has no attribute 'get'",
                py_type(other)
            )))
        }
    };
    let currency = map
        .get(country.as_deref().unwrap_or(""))
        .cloned()
        .unwrap_or_else(|| map.get("*").cloned().unwrap_or(Value::Null));
    let tz = timezone.filter(|t| !t.is_empty());
    let source = if by_phone.is_some() {
        Value::from("phone")
    } else if country.is_some() {
        Value::from("timezone")
    } else {
        Value::Null
    };
    Ok(json!({"country": country, "currency": currency, "timezone": tz, "country_source": source}))
}

fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Array(_) => "list",
        Value::String(_) => "str",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::Bool(_) => "bool",
        _ => "NoneType",
    }
}
