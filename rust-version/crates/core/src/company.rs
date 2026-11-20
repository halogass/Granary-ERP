//! Port of backend/app/core/utils.py `get_company_info(db)`: the company settings (SystemSetting rows of
//! category "company") with the fallbacks that keep callers from ever getting empty names/emails/sites.
//! Golden-tested against the Python function: compat-tests/golden/gen_company.py.

use std::collections::HashMap;

use serde_json::{Map, Value};
use sqlx::PgConnection;

/// The dict `get_company_info` returns, in its key order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompanyInfo {
    pub company_name: String,
    pub company_legal: String,
    pub company_trade: String,
    pub company_email: String,
    pub company_phone: String,
    pub company_website: String,
    pub company_address: String,
    pub company_rd_office: String,
    pub letterhead_url: String,
    pub app_url: String,
}

impl CompanyInfo {
    /// The Python dict, keys in Python's order.
    pub fn to_map(&self) -> Map<String, Value> {
        let mut m = Map::new();
        for (k, v) in [
            ("company_name", &self.company_name),
            ("company_legal", &self.company_legal),
            ("company_trade", &self.company_trade),
            ("company_email", &self.company_email),
            ("company_phone", &self.company_phone),
            ("company_website", &self.company_website),
            ("company_address", &self.company_address),
            ("company_rd_office", &self.company_rd_office),
            ("letterhead_url", &self.letterhead_url),
            ("app_url", &self.app_url),
        ] {
            m.insert(k.to_string(), Value::String(v.clone()));
        }
        m
    }
}

/// The body of `get_company_info` after the query: `rows` are (key, value) in the order the database
/// returned them. `{s.key: (s.value or "")}`: a later duplicate key wins (keys are unique in practice).
pub fn company_info_from_rows(rows: &[(String, Option<String>)]) -> CompanyInfo {
    let mut map: HashMap<&str, &str> = HashMap::new();
    for (k, v) in rows {
        map.insert(k.as_str(), v.as_deref().unwrap_or(""));
    }
    // `rows.get(k) or ...`: a missing key and an empty value both fall through.
    let get = |k: &str| map.get(k).copied().filter(|v| !v.is_empty());
    let trade = get("company_trade_name")
        .or_else(|| get("company_name"))
        .unwrap_or("Your Company")
        .to_string();
    let legal = get("company_name")
        .unwrap_or("Your Company Ltd")
        .to_string();
    let email = get("company_email")
        .unwrap_or("hello@example.com")
        .to_string();
    let site = get("company_website")
        .unwrap_or("https://example.com")
        .to_string();
    CompanyInfo {
        company_name: trade.clone(),
        company_legal: legal,
        company_trade: trade,
        company_email: email,
        company_phone: get("company_phone").unwrap_or("").to_string(),
        company_website: site.clone(),
        company_address: get("company_address").unwrap_or("").to_string(),
        company_rd_office: get("company_rd_office").unwrap_or("").to_string(),
        letterhead_url: get("company.letterhead_image").unwrap_or("").to_string(),
        app_url: site,
    }
}

/// `get_company_info(db)`.
pub async fn get_company_info(db: &mut PgConnection) -> Result<CompanyInfo, sqlx::Error> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT system_settings.key, system_settings.value FROM system_settings \
         WHERE system_settings.category = 'company'",
    )
    .fetch_all(&mut *db)
    .await?;
    Ok(company_info_from_rows(&rows))
}
