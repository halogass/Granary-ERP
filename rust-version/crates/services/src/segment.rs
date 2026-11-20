//! The segment of a lead, a deal or a catalog item: an optional, free label a company groups its sales by
//! (for example "retail", "public sector" or a product line). It is stored in the `segment` columns of
//! `leads`, `deals` and `product_catalog`; the API, the AI tools and the CRM change cards call it `segment`.
//!
//! ```text
//!     crm.segments   JSON list of the allowed values, e.g. ["retail", "public_sector"]. Empty (the default):
//!                    any text up to MAX_LEN characters is accepted.
//! ```
//!
//! A segment is never required: null or "" clears it. A new deal starts with its lead's segment.

use granary_core::errors::ApiResult;
use serde_json::Value;
use sqlx::PgConnection;

/// The setting with the allowed values (a JSON list of strings; empty = free text).
pub const SETTING_KEY: &str = "crm.segments";
/// The column holds at most this many characters (product_catalog.segment is VARCHAR(30)).
pub const MAX_LEN: usize = 30;
/// The name the API and the tools use.
pub const FIELD: &str = "segment";
/// The column the value is stored in.
pub const COLUMN: &str = "segment";

/// The allowed values (trimmed, lower case, no empties). A missing or unreadable setting is the empty list.
pub async fn allowed(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    let raw = granary_core::settings_store::get_optional(db, SETTING_KEY).await?;
    Ok(parse_allowed(raw.as_deref()))
}

/// The allowed values of the setting's text.
pub fn parse_allowed(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Checks a setting value before it is saved: a JSON list of short strings.
pub fn validate_setting(key: &str, value: &str) -> Option<Result<(), String>> {
    if key != SETTING_KEY {
        return None;
    }
    if value.trim().is_empty() {
        return Some(Ok(()));
    }
    let ok = match serde_json::from_str::<Value>(value) {
        Ok(Value::Array(a)) => a.iter().all(|v| {
            v.as_str()
                .is_some_and(|s| !s.trim().is_empty() && s.trim().chars().count() <= MAX_LEN)
        }),
        _ => false,
    };
    Some(if ok {
        Ok(())
    } else {
        Err(format!(
            "{SETTING_KEY} must be a JSON list of texts of 1 to {MAX_LEN} characters, e.g. [\"retail\", \"public_sector\"]"
        ))
    })
}

/// The error a bad value gets.
pub fn error(field: &str, allowed: &[String]) -> String {
    if allowed.is_empty() {
        format!("{field} must be a text of at most {MAX_LEN} characters")
    } else {
        format!("{field} must be one of {}", allowed.join(", "))
    }
}

/// Checks a submitted value against the allowed list (pure): `Ok(None)` clears it (null or blank), `Ok(Some(v))`
/// is the stored value (trimmed; lower case when the list is set), otherwise the error.
pub fn check_with(field: &str, v: &Value, allowed: &[String]) -> Result<Option<String>, String> {
    let s = match v {
        Value::Null => return Ok(None),
        Value::String(s) => s.trim(),
        _ => return Err(error(field, allowed)),
    };
    if s.is_empty() {
        return Ok(None);
    }
    if allowed.is_empty() {
        if s.chars().count() > MAX_LEN {
            return Err(error(field, allowed));
        }
        return Ok(Some(s.to_string()));
    }
    let low = s.to_lowercase();
    if allowed.contains(&low) {
        Ok(Some(low))
    } else {
        Err(error(field, allowed))
    }
}

/// [`check_with`] with the setting read from the database.
pub async fn check(
    db: &mut PgConnection,
    field: &str,
    v: &Value,
) -> ApiResult<Result<Option<String>, String>> {
    let allowed = allowed(db).await?;
    Ok(check_with(field, v, &allowed))
}

/// The segment a web form names (its `segment` field), when it is a valid value; otherwise none.
pub async fn from_form(
    db: &mut PgConnection,
    fields: &serde_json::Map<String, Value>,
) -> ApiResult<Option<String>> {
    let Some(v) = fields.get(FIELD) else {
        return Ok(None);
    };
    Ok(check(db, FIELD, v).await?.ok().flatten())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn free_when_no_list() {
        assert_eq!(
            check_with("segment", &json!(" Retail "), &[]),
            Ok(Some("Retail".into()))
        );
        assert_eq!(check_with("segment", &json!(""), &[]), Ok(None));
        assert_eq!(check_with("segment", &Value::Null, &[]), Ok(None));
        assert!(check_with("segment", &json!("x".repeat(31)), &[]).is_err());
        assert!(check_with("segment", &json!(3), &[]).is_err());
    }

    #[test]
    fn list_restricts() {
        let allowed = parse_allowed(Some(r#"["retail", " Public_Sector ", ""]"#));
        assert_eq!(allowed, vec!["retail", "public_sector"]);
        assert_eq!(
            check_with("segment", &json!("RETAIL"), &allowed),
            Ok(Some("retail".into()))
        );
        assert_eq!(
            check_with("segment", &json!("other"), &allowed),
            Err("segment must be one of retail, public_sector".into())
        );
        assert_eq!(check_with("segment", &json!(" "), &allowed), Ok(None));
    }

    #[test]
    fn setting_check() {
        assert_eq!(validate_setting("other.key", "x"), None);
        assert_eq!(validate_setting(SETTING_KEY, "[]"), Some(Ok(())));
        assert_eq!(validate_setting(SETTING_KEY, ""), Some(Ok(())));
        assert_eq!(validate_setting(SETTING_KEY, r#"["a","b"]"#), Some(Ok(())));
        assert!(matches!(
            validate_setting(SETTING_KEY, r#"["a", 1]"#),
            Some(Err(_))
        ));
        assert!(matches!(validate_setting(SETTING_KEY, "{}"), Some(Err(_))));
        assert_eq!(parse_allowed(Some("not json")), Vec::<String>::new());
    }
}
