//! backend/app/routers/system.py, mounted at `/api/v1/system` (backend/app/main.py).
//!
//! Not behind INTERNAL_ONLY: routes guard themselves (several are public: `/company`, `/branding`,
//! `/settings/letterhead` GET, `/tax-configs/active`).
//!
//! Split by the Python file's sections: settings, tax configs/rules, audit log, notifications, dashboard
//! KPIs and currencies here; AI settings in `system/ai.rs`, branding and letterhead in
//! `system/branding.rs`, email configuration in `system/email_cfg.rs`.
//!
//! `/settings/letterhead` is reserved in Python (`reserve_path`, GET/POST/DELETE): PATCH and PUT answer
//! 405 with `Allow`, as axum's static route does.

pub mod ai;
pub mod branding;
pub mod email_cfg;

use crate::extract::pg_uuid;
use axum::extract::Path;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use granary_compat::pyjson::{self, DumpOpts, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::{AuthUser, CurrentUser, InternalUser};
use granary_core::db::Db;
use granary_core::validation::{json_loads, PyJson, PyQuery};
use granary_core::{ApiError, ApiResult, AppState};

use super::at;
use crate::resp;

/// The prefix main.py mounts this router at.
pub const PREFIX: &str = "/api/v1/system";

/// The routes of `router` in system.py.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(&at(PREFIX, "/company"), get(get_company_settings))
        .route(&at(PREFIX, "/settings"), get(list_settings))
        .route(&at(PREFIX, "/settings/{key}"), patch(update_setting))
        .route(
            &at(PREFIX, "/ai-settings"),
            get(ai::get_ai_settings).put(ai::update_ai_settings),
        )
        .route(&at(PREFIX, "/ai-settings/models"), get(ai::list_ai_models))
        .route(
            &at(PREFIX, "/ai-settings/test"),
            post(ai::test_ai_connection),
        )
        .route(
            &at(PREFIX, "/ai-settings/test-provider"),
            post(ai::test_ai_provider),
        )
        .route(&at(PREFIX, "/ai-usage"), get(ai::ai_usage))
        .route(
            &at(PREFIX, "/ai-settings/reindex"),
            post(ai::reindex_kb_embeddings_endpoint),
        )
        .route(
            &at(PREFIX, "/branding"),
            get(branding::get_branding).patch(branding::update_branding),
        )
        .route(
            &at(PREFIX, "/branding/logo"),
            post(branding::upload_logo).delete(branding::delete_logo),
        )
        .route(
            &at(PREFIX, "/branding/favicon"),
            post(branding::upload_favicon).delete(branding::delete_favicon),
        )
        .route(
            &at(PREFIX, "/settings/letterhead"),
            get(branding::get_letterhead)
                .post(branding::upload_letterhead)
                .delete(branding::delete_letterhead),
        )
        .route(
            &at(PREFIX, "/tax-configs"),
            get(list_tax_configs).post(create_tax_config),
        )
        .route(
            &at(PREFIX, "/tax-configs/active"),
            get(get_active_tax_config),
        )
        .route(
            &at(PREFIX, "/tax-configs/{config_id}/activate"),
            post(activate_tax_config),
        )
        .route(
            &at(PREFIX, "/tax-configs/{config_id}/rules"),
            post(add_tax_rule),
        )
        .route(&at(PREFIX, "/tax-rules/{rule_id}"), patch(update_tax_rule))
        .route(&at(PREFIX, "/audit-log"), get(get_audit_log))
        .route(
            &at(PREFIX, "/email-config"),
            get(email_cfg::get_email_config).patch(email_cfg::update_email_config),
        )
        .route(
            &at(PREFIX, "/email-config/test"),
            post(email_cfg::test_email_config),
        )
        .route(
            &at(PREFIX, "/email-config/test-imap"),
            post(email_cfg::test_crm_imap),
        )
        // R&D 26 Oct: the CRM inboxes (hello@, sales@...)
        .route(
            &at(PREFIX, "/crm-mailboxes"),
            get(email_cfg::list_crm_mailboxes).post(email_cfg::create_crm_mailbox),
        )
        .route(
            &at(PREFIX, "/crm-mailboxes/{mailbox_id}"),
            patch(email_cfg::update_crm_mailbox),
        )
        .route(
            &at(PREFIX, "/notifications"),
            get(list_notifications).post(create_notification),
        )
        .route(
            &at(PREFIX, "/notifications/unread-count"),
            get(get_unread_count),
        )
        .route(
            &at(PREFIX, "/notifications/{notification_id}/read"),
            patch(mark_notification_read),
        )
        .route(&at(PREFIX, "/notifications/read-all"), post(mark_all_read))
        .route(&at(PREFIX, "/dashboard-kpis"), get(get_dashboard_kpis))
        .route(
            &at(PREFIX, "/currencies"),
            get(get_currency_settings).patch(update_currency_settings),
        )
        .route(&at(PREFIX, "/currencies/sync-now"), post(trigger_fx_sync))
        .route(
            &at(PREFIX, "/currencies/test-api-key"),
            post(test_fx_api_key),
        )
}

/// `_require_admin(current_user)`.
pub fn require_admin(user: &AuthUser) -> ApiResult<()> {
    if !(user.is_superadmin || user.is_system_admin) {
        return Err(ApiError::http(403, "System admin required"));
    }
    Ok(())
}

pub const SETTING_MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

pub const ENCRYPTED_SETTING_KEYS: &[&str] = &[
    "hr.face_api_key",
    "ai.anthropic.api_key",
    "ai.gemini.api_key",
    "ai.openai.api_key",
    "ai.deepseek.api_key",
    "ai.search.api_key",
];

// ─── System Settings ─────────────────────────────────────────────────────────

/// `GET "/company"`: every company.* setting as a key-value dict. Public (print/PDF utilities).
pub async fn get_company_settings(db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT system_settings.key, system_settings.value FROM system_settings \
         WHERE system_settings.category = 'company'",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Map::new();
    for (k, v) in rows {
        out.insert(k, Value::String(v.unwrap_or_default()));
    }
    Ok(resp::json(Value::Object(out)))
}

/// `GET "/settings"`: every setting for an admin, the public ones for everyone else.
pub async fn list_settings(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let all = user.is_superadmin || user.is_system_admin;
    let mut conn = db.conn().await?;
    let cols = "SELECT system_settings.key, system_settings.value, system_settings.value_type, \
                system_settings.category, system_settings.label, system_settings.is_public FROM system_settings";
    let sql = if all {
        cols.to_string()
    } else {
        format!("{cols} WHERE system_settings.is_public = true")
    };
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
        Option<bool>,
    )> = sqlx::query_as(&sql).fetch_all(&mut *conn).await?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(key, value, value_type, category, label, is_public)| {
            let shown = if ENCRYPTED_SETTING_KEYS.contains(&key.as_str())
                && value.as_deref().is_some_and(|v| !v.is_empty())
            {
                Some(SETTING_MASK.to_string())
            } else {
                value
            };
            json!({
                "key": key,
                "value": shown,
                "value_type": value_type,
                "category": category,
                "label": label,
                "is_public": is_public,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class UpdateSettingRequest(BaseModel)`.
    pub struct UpdateSettingRequest {
        pub value: String,
    }
}

/// `_guard_functional_currency_change(db, new_value)`: 409 when the functional currency would change while
/// posted entries exist.
async fn guard_functional_currency_change(
    conn: &mut PgConnection,
    new_value: Option<&str>,
) -> ApiResult<()> {
    let Some(new_value) = new_value.filter(|v| !v.is_empty()) else {
        return Ok(());
    };
    if new_value.to_uppercase() == granary_services::fx::functional_currency(conn).await? {
        return Ok(());
    }
    let posted: Option<(Uuid,)> = sqlx::query_as(
        "SELECT journal_entries.id FROM journal_entries WHERE journal_entries.status IN ('posted', 'reversed') \
         AND journal_entries.is_deleted = false LIMIT 1",
    )
    .fetch_optional(&mut *conn)
    .await?;
    if posted.is_some() {
        return Err(ApiError::http(
            409,
            "The functional currency can't be changed once journal entries are posted: every \
             posted balance is recorded in it, and changing it would reinterpret them all in \
             the new currency. Changing it needs a new set of books.",
        ));
    }
    Ok(())
}

/// `PATCH "/settings/{key}"`: system admins only; sensitive keys are stored encrypted.
pub async fn update_setting(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(key): Path<String>,
    PyJson(body): PyJson<UpdateSettingRequest>,
) -> ApiResult<Response> {
    update_setting_impl(user, db, key, body).await
}

async fn update_setting_impl(
    user: AuthUser,
    db: Db,
    key: String,
    body: UpdateSettingRequest,
) -> ApiResult<Response> {
    if !(user.is_superadmin || user.is_system_admin) {
        return Err(ApiError::http(403, "System admin required"));
    }
    let mut conn = db.conn().await?;
    let found: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT system_settings.id, system_settings.key FROM system_settings WHERE system_settings.key = $1",
    )
    .bind(&key)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, setting_key)) = found else {
        return Err(ApiError::not_found("SystemSetting", Some(&key)));
    };
    if key == "currency.functional" {
        // same rule as PATCH /system/currencies
        guard_functional_currency_change(&mut conn, Some(&body.value)).await?;
    }
    // The logo and favicon are served to every visitor: only a checked PNG, JPEG, WebP or ICO data URL.
    if let Some(Err(why)) = branding::validate_image_setting(&key, &body.value) {
        return Err(ApiError::http(400, why));
    }
    // 4 Nov 2026: a quotation notes template (or its families, texts, phrases) is checked before it is saved
    if let Some(Err(why)) = granary_services::quotation_notes::validate_setting(&key, &body.value) {
        return Err(ApiError::http(400, format!("{key}: {why}")));
    }
    // 5 Nov 2026 (E7): the pricing rules' thresholds and SKU lists are checked before they are saved
    if let Some(Err(why)) = granary_services::pricing_rules::validate_setting(&key, &body.value) {
        return Err(ApiError::http(400, why));
    }
    // 6 Nov 2026 (migration 0166): the passive face check's ranges, palette and settle < flash time
    if granary_services::face::passive::KEYS.contains(&key.as_str()) {
        let current =
            granary_core::settings_store::values(&mut conn, &granary_services::face::passive::KEYS)
                .await?;
        if let Some(Err(why)) =
            granary_services::face::passive::validate_setting(&key, &body.value, &current)
        {
            return Err(ApiError::http(400, why));
        }
    }
    let sensitive = ENCRYPTED_SETTING_KEYS.contains(&key.as_str());
    let value = if sensitive && !body.value.is_empty() {
        granary_services::email_crypto::encrypt(&body.value)
    } else {
        body.value.clone()
    };
    sqlx::query(
        "UPDATE system_settings SET value = $1, updated_by_id = $2, updated_at = now() \
         WHERE system_settings.id = $3",
    )
    .bind(&value)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    // Never return encrypted values: a placeholder instead.
    let display = if sensitive && !value.is_empty() {
        SETTING_MASK.to_string()
    } else {
        value
    };
    Ok(resp::json(json!({"key": setting_key, "value": display})))
}

// ─── Tax Config ──────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow, Debug, Clone)]
struct TaxConfigRow {
    id: Uuid,
    name: String,
    country_code: String,
    country_name: String,
    currency_code: String,
    currency_symbol: Option<String>,
    currency_format: Option<String>,
    decimal_separator: Option<String>,
    thousands_separator: Option<String>,
    fiscal_year_start_month: Option<i32>,
    date_format: Option<String>,
    is_active: bool,
}

#[derive(sqlx::FromRow, Debug, Clone)]
struct TaxRuleRow {
    config_id: Uuid,
    id: Uuid,
    code: String,
    name: String,
    tax_type: String,
    rate: Decimal,
    applies_to_sales: Option<bool>,
    applies_to_purchases: Option<bool>,
    applies_to_services: Option<bool>,
    applies_to_products: Option<bool>,
    is_withholding: Option<bool>,
    show_on_invoice: Option<bool>,
    minimum_taxable_amount: Option<Decimal>,
    reporting_code: Option<String>,
    is_active: Option<bool>,
}

const CONFIG_COLS: &str = "tax_configs.id, tax_configs.name, tax_configs.country_code, tax_configs.country_name, \
    tax_configs.currency_code, tax_configs.currency_symbol, tax_configs.currency_format, \
    tax_configs.decimal_separator, tax_configs.thousands_separator, tax_configs.fiscal_year_start_month, \
    tax_configs.date_format, tax_configs.is_active";

/// `TaxConfig.rules` (lazy="selectin"): one query for all configs, rows in the database's order.
async fn rules_of(conn: &mut PgConnection, ids: &[Uuid]) -> ApiResult<Vec<TaxRuleRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let marks: Vec<String> = (1..=ids.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "SELECT tax_rules.config_id, tax_rules.id, tax_rules.code, tax_rules.name, tax_rules.tax_type, \
         tax_rules.rate, tax_rules.applies_to_sales, tax_rules.applies_to_purchases, \
         tax_rules.applies_to_services, tax_rules.applies_to_products, tax_rules.is_withholding, \
         tax_rules.show_on_invoice, tax_rules.minimum_taxable_amount, tax_rules.reporting_code, \
         tax_rules.is_active FROM tax_rules WHERE tax_rules.config_id IN ({})",
        marks.join(", ")
    );
    let mut q = sqlx::query_as::<_, TaxRuleRow>(&sql);
    for id in ids {
        q = q.bind(id);
    }
    Ok(q.fetch_all(&mut *conn).await?)
}

/// `GET "/tax-configs"`.
pub async fn list_tax_configs(CurrentUser(_user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let configs: Vec<TaxConfigRow> =
        sqlx::query_as(&format!("SELECT {CONFIG_COLS} FROM tax_configs"))
            .fetch_all(&mut *conn)
            .await?;
    let ids: Vec<Uuid> = configs.iter().map(|c| c.id).collect();
    let rules = rules_of(&mut conn, &ids).await?;
    let out: Vec<Value> = configs
        .iter()
        .map(|c| {
            let rs: Vec<Value> = rules
                .iter()
                .filter(|r| r.config_id == c.id)
                .map(|r| {
                    let services = r.applies_to_services.unwrap_or(false);
                    let products = r.applies_to_products.unwrap_or(false);
                    let applies_to = if services && !products {
                        "services"
                    } else if products && !services {
                        "products"
                    } else {
                        "all"
                    };
                    json!({
                        "id": r.id.to_string(), "code": r.code, "name": r.name,
                        "rate": resp::dec_float(&r.rate),
                        "applies_to": applies_to,
                        "is_inclusive": false,
                        "is_active": r.is_active,
                    })
                })
                .collect();
            json!({
                "id": c.id.to_string(), "name": c.name, "country_code": c.country_code,
                "country_name": c.country_name, "currency_code": c.currency_code,
                "currency_symbol": c.currency_symbol, "is_active": c.is_active,
                "fiscal_year_start_month": c.fiscal_year_start_month,
                "date_format": c.date_format,
                "rules": rs,
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET "/tax-configs/active"`: public.
pub async fn get_active_tax_config(db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    // batch 2 T3: Python's scalar_one_or_none() raised (a 500) when several configs were active (prod had 71
    // from a seed that re-created it on every start). The config is one per company (its rules carry the tax
    // types): the newest active one answers, deterministically (migration 0111 leaves only that one active).
    let found: Vec<TaxConfigRow> = sqlx::query_as(&format!(
        "SELECT {CONFIG_COLS} FROM tax_configs WHERE tax_configs.is_active = true \
         ORDER BY tax_configs.effective_from DESC, tax_configs.created_at DESC, tax_configs.id DESC LIMIT 1"
    ))
    .fetch_all(&mut *conn)
    .await?;
    let Some(c) = found.into_iter().next() else {
        return Err(ApiError::http(404, "No active tax configuration found"));
    };
    let rules = rules_of(&mut conn, &[c.id]).await?;
    let rs: Vec<Value> = rules
        .iter()
        .map(|r| {
            // float(x) if x else None: a zero minimum is None too
            let min = r
                .minimum_taxable_amount
                .filter(|m| !m.is_zero())
                .map(|m| resp::dec_float(&m))
                .unwrap_or(Value::Null);
            json!({
                "id": r.id.to_string(), "code": r.code, "name": r.name, "tax_type": r.tax_type,
                "rate": resp::dec_float(&r.rate), "applies_to_sales": r.applies_to_sales,
                "applies_to_purchases": r.applies_to_purchases, "applies_to_services": r.applies_to_services,
                "applies_to_products": r.applies_to_products, "is_withholding": r.is_withholding,
                "show_on_invoice": r.show_on_invoice,
                "minimum_taxable_amount": min,
                "reporting_code": r.reporting_code,
            })
        })
        .collect();
    Ok(resp::json(json!({
        "id": c.id.to_string(),
        "name": c.name,
        "country_code": c.country_code,
        "country_name": c.country_name,
        "currency_code": c.currency_code,
        "currency_symbol": c.currency_symbol,
        "currency_format": c.currency_format,
        "decimal_separator": c.decimal_separator,
        "thousands_separator": c.thousands_separator,
        "fiscal_year_start_month": c.fiscal_year_start_month,
        "date_format": c.date_format,
        "rules": rs,
    })))
}

granary_core::py_model! {
    /// `class TaxConfigCreate(BaseModel)`.
    pub struct TaxConfigCreate {
        pub name: String,
        pub country_code: String,
        pub country_name: String,
        pub currency_code: String,
        pub currency_symbol: String = "$".to_string(),
        pub currency_format: String = "{symbol}{amount}".to_string(),
        pub decimal_separator: String = ".".to_string(),
        pub thousands_separator: String = ",".to_string(),
        pub fiscal_year_start_month: i64 = 1,
        pub date_format: String = "DD/MM/YYYY".to_string(),
        /// ISO date string
        pub effective_from: String,
    }
}

fn require_admin_body(user: &AuthUser) -> ApiResult<()> {
    require_admin(user)
}

/// `POST "/tax-configs"` (201): an inactive config.
pub async fn create_tax_config(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<TaxConfigCreate>,
) -> ApiResult<Response> {
    require_admin_body(&user)?;
    // date.fromisoformat raises ValueError (the central 422) on anything else
    let effective: NaiveDate =
        granary_core::settings_store::py_date_fromisoformat(&body.effective_from)
            .ok_or_else(ApiError::invalid_date)?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO tax_configs (name, country_code, country_name, currency_code, currency_symbol, \
         currency_format, decimal_separator, thousands_separator, fiscal_year_start_month, date_format, \
         is_active, effective_from, created_by_id, id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, false, $11, $12, $13)",
    )
    .bind(&body.name)
    .bind(&body.country_code)
    .bind(&body.country_name)
    .bind(&body.currency_code)
    .bind(&body.currency_symbol)
    .bind(&body.currency_format)
    .bind(&body.decimal_separator)
    .bind(&body.thousands_separator)
    .bind(body.fiscal_year_start_month)
    .bind(&body.date_format)
    .bind(effective)
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "name": body.name}),
    ))
}

/// `POST "/tax-configs/{config_id}/activate"`: deactivates every other config.
pub async fn activate_tax_config(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(config_id): Path<String>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    let id = pg_uuid(&config_id)?;
    let found: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT tax_configs.id, tax_configs.name FROM tax_configs WHERE tax_configs.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, name)) = found else {
        return Err(ApiError::not_found("TaxConfig", Some(&config_id)));
    };
    // batch 2 T3: every other active config (however many) is switched off in the same transaction
    sqlx::query(
        "UPDATE tax_configs SET is_active = false, updated_at = now() \
         WHERE tax_configs.is_active = true AND tax_configs.id <> $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE tax_configs SET is_active = true, updated_by_id = $1, updated_at = now() \
         WHERE tax_configs.id = $2",
    )
    .bind(user.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(
        json!({"detail": format!("Tax config '{name}' is now active")}),
    ))
}

// ─── Tax Rules ───────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// `class TaxRuleCreate(BaseModel)`.
    pub struct TaxRuleCreate {
        pub code: String,
        pub name: String,
        pub tax_type: String,
        pub rate: Decimal,
        pub description: Option<String> = None,
        pub applies_to_sales: bool = false,
        pub applies_to_purchases: bool = false,
        pub applies_to_services: bool = false,
        pub applies_to_products: bool = false,
        pub applies_to_imports: bool = false,
        pub is_withholding: bool = false,
        pub show_on_invoice: bool = true,
        pub is_compound: bool = false,
        pub minimum_taxable_amount: Option<Decimal> = None,
        pub reporting_code: Option<String> = None,
    }
}

/// `POST "/tax-configs/{config_id}/rules"` (201).
pub async fn add_tax_rule(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(config_id): Path<String>,
    PyJson(body): PyJson<TaxRuleCreate>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let cid = pg_uuid(&config_id)?;
    let mut conn = db.conn().await?;
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT tax_configs.id FROM tax_configs WHERE tax_configs.id = $1")
            .bind(cid)
            .fetch_optional(&mut *conn)
            .await?;
    if found.is_none() {
        return Err(ApiError::not_found("TaxConfig", Some(&config_id)));
    }
    let id = Uuid::new_v4();
    let min = body.minimum_taxable_amount;
    // TaxRule(config_id=..., **body.model_dump()) plus the model's column defaults
    sqlx::query(
        "INSERT INTO tax_rules (config_id, code, name, description, rate, is_percentage, is_compound, \
         tax_type, is_withholding, applies_to_sales, applies_to_purchases, applies_to_services, \
         applies_to_products, applies_to_imports, applies_to_vendor_invoices, show_on_quotation, \
         show_on_invoice, show_on_vendor_invoice, minimum_taxable_amount, registration_number_required, \
         reporting_code, display_order, is_active, id) \
         VALUES ($1, $2, $3, $4, $5, true, $6, $7, $8, $9, $10, $11, $12, $13, false, false, $14, false, $15, \
         false, $16, 0, true, $17)",
    )
    .bind(cid)
    .bind(&body.code)
    .bind(&body.name)
    .bind(&body.description)
    .bind(body.rate)
    .bind(body.is_compound)
    .bind(&body.tax_type)
    .bind(body.is_withholding)
    .bind(body.applies_to_sales)
    .bind(body.applies_to_purchases)
    .bind(body.applies_to_services)
    .bind(body.applies_to_products)
    .bind(body.applies_to_imports)
    .bind(body.show_on_invoice)
    .bind(min)
    .bind(&body.reporting_code)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "code": body.code}),
    ))
}

granary_core::py_model! {
    /// `class TaxRuleUpdate(BaseModel)`.
    pub struct TaxRuleUpdate {
        pub code: Option<String> = None,
        pub name: Option<String> = None,
        pub description: Option<String> = None,
        pub rate: Option<Decimal> = None,
        pub applies_to_sales: Option<bool> = None,
        pub applies_to_purchases: Option<bool> = None,
        pub applies_to_services: Option<bool> = None,
        pub applies_to_products: Option<bool> = None,
        pub applies_to_imports: Option<bool> = None,
        pub is_withholding: Option<bool> = None,
        pub show_on_invoice: Option<bool> = None,
        pub is_compound: Option<bool> = None,
        pub minimum_taxable_amount: Option<Decimal> = None,
        pub is_active: Option<bool> = None,
        pub tax_account_code: Option<String> = None,
        pub reporting_code: Option<String> = None,
        pub compound_base_codes: Option<Vec<String>> = None,
    }
}

enum Arg {
    Text(String),
    Bool(bool),
    Num(Decimal),
    List(Vec<String>),
}

/// `PATCH "/tax-rules/{rule_id}"`: the non-null fields of the body.
pub async fn update_tax_rule(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(rule_id): Path<String>,
    PyJson(body): PyJson<TaxRuleUpdate>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let rid = pg_uuid(&rule_id)?;
    let mut conn = db.conn().await?;
    let found: Option<(Uuid, String, Decimal)> = sqlx::query_as(
        "SELECT tax_rules.id, tax_rules.code, tax_rules.rate FROM tax_rules WHERE tax_rules.id = $1",
    )
    .bind(rid)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, code, rate)) = found else {
        return Err(ApiError::not_found("TaxRule", Some(&rule_id)));
    };
    let mut sets: Vec<(&str, Arg)> = Vec::new();
    let t = |v: &Option<String>| v.clone().map(Arg::Text);
    let b = |v: Option<bool>| v.map(Arg::Bool);
    let fields: Vec<(&str, Option<Arg>)> = vec![
        ("code", t(&body.code)),
        ("name", t(&body.name)),
        ("description", t(&body.description)),
        ("rate", body.rate.map(Arg::Num)),
        ("applies_to_sales", b(body.applies_to_sales)),
        ("applies_to_purchases", b(body.applies_to_purchases)),
        ("applies_to_services", b(body.applies_to_services)),
        ("applies_to_products", b(body.applies_to_products)),
        ("applies_to_imports", b(body.applies_to_imports)),
        ("is_withholding", b(body.is_withholding)),
        ("show_on_invoice", b(body.show_on_invoice)),
        ("is_compound", b(body.is_compound)),
        (
            "minimum_taxable_amount",
            body.minimum_taxable_amount.map(Arg::Num),
        ),
        ("is_active", b(body.is_active)),
        ("tax_account_code", t(&body.tax_account_code)),
        ("reporting_code", t(&body.reporting_code)),
        (
            "compound_base_codes",
            body.compound_base_codes.clone().map(Arg::List),
        ),
    ];
    for (k, v) in fields {
        if let Some(v) = v {
            sets.push((k, v));
        }
    }
    if !sets.is_empty() {
        let assigns: Vec<String> = sets
            .iter()
            .enumerate()
            .map(|(i, (k, _))| format!("{k} = ${}", i + 2))
            .collect();
        let sql = format!(
            "UPDATE tax_rules SET {}, updated_at = now() WHERE tax_rules.id = $1",
            assigns.join(", ")
        );
        let mut q = sqlx::query(&sql).bind(id);
        for (_, v) in &sets {
            q = match v {
                Arg::Text(s) => q.bind(s.clone()),
                Arg::Bool(x) => q.bind(*x),
                Arg::Num(d) => q.bind(*d),
                Arg::List(l) => q.bind(l.clone()),
            };
        }
        q.execute(&mut *conn).await?;
    }
    drop(conn);
    db.commit().await?;
    // expire_on_commit=False: the attributes as set (the body's float), else as loaded
    let rate_out = match body.rate {
        Some(r) => resp::dec_float(&r),
        None => resp::dec_float(&rate),
    };
    Ok(resp::json(json!({
        "id": id.to_string(),
        "code": body.code.clone().unwrap_or(code),
        "rate": rate_out,
    })))
}

// ─── Audit Log ───────────────────────────────────────────────────────────────

granary_core::py_model! {
    /// The query of `get_audit_log`.
    pub struct AuditQ {
        pub entity_type: Option<String> = None,
        pub entity_id: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `GET "/audit-log"`: system admins only, newest first.
pub async fn get_audit_log(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<AuditQ>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut wheres: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    if let Some(t) = q.entity_type.as_deref().filter(|s| !s.is_empty()) {
        args.push(t.to_string());
        wheres.push(format!("audit_logs.entity_type = ${}", args.len()));
    }
    if let Some(e) = q.entity_id.as_deref().filter(|s| !s.is_empty()) {
        args.push(e.to_string());
        wheres.push(format!("audit_logs.entity_id = ${}", args.len()));
    }
    let where_sql = if wheres.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", wheres.join(" AND "))
    };
    let sql = format!(
        "SELECT audit_logs.id, audit_logs.entity_type, audit_logs.entity_id, audit_logs.action, \
         audit_logs.user_id, audit_logs.changed_fields, audit_logs.created_at FROM audit_logs{where_sql} \
         ORDER BY audit_logs.created_at DESC LIMIT ${} OFFSET ${}",
        args.len() + 1,
        args.len() + 2
    );
    let mut query = sqlx::query(&sql);
    for a in &args {
        query = query.bind(a);
    }
    let mut conn = db.conn().await?;
    let rows = query
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&mut *conn)
        .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            // str(l.entity_id) if l.entity_id is not None else None
            let entity_id: Option<String> = r.get("entity_id");
            json!({
                "id": r.get::<Uuid, _>("id").to_string(),
                "entity_type": r.get::<String, _>("entity_type"),
                "entity_id": entity_id,
                "action": r.get::<String, _>("action"),
                "user_id": r.get::<Option<Uuid>, _>("user_id").map(|u| u.to_string()),
                "changed_fields": r.get::<Option<Vec<String>>, _>("changed_fields"),
                "created_at": resp::iso(&r.get::<DateTime<Utc>, _>("created_at")),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

// ─── Notifications ───────────────────────────────────────────────────────────

granary_core::py_model! {
    /// The query of `list_notifications`.
    pub struct NotificationsQ {
        pub unread_only: bool = false,
        pub limit: i64 = 30,
    }
}

/// `GET "/notifications"`: the caller's, newest first.
pub async fn list_notifications(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<NotificationsQ>,
) -> ApiResult<Response> {
    let unread = if q.unread_only {
        " AND notifications.is_read = false"
    } else {
        ""
    };
    let sql = format!(
        "SELECT notifications.id, notifications.type, notifications.title, notifications.body, \
         notifications.link, notifications.entity_type, notifications.entity_id, notifications.is_read, \
         notifications.priority, notifications.created_at FROM notifications \
         WHERE notifications.user_id = $1{unread} ORDER BY notifications.created_at DESC LIMIT $2"
    );
    let mut conn = db.conn().await?;
    let rows = sqlx::query(&sql)
        .bind(user.id)
        .bind(q.limit)
        .fetch_all(&mut *conn)
        .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<Uuid, _>("id").to_string(),
                "type": r.get::<String, _>("type"),
                "title": r.get::<String, _>("title"),
                "body": r.get::<Option<String>, _>("body"),
                "link": r.get::<Option<String>, _>("link"),
                "entity_type": r.get::<Option<String>, _>("entity_type"),
                "entity_id": r.get::<Option<String>, _>("entity_id"),
                "is_read": r.get::<Option<bool>, _>("is_read"),
                "priority": r.get::<Option<String>, _>("priority"),
                "created_at": resp::iso_opt(r.get::<Option<DateTime<Utc>>, _>("created_at").as_ref()),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `GET "/notifications/unread-count"`.
pub async fn get_unread_count(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE notifications.user_id = $1 AND notifications.is_read = false",
    )
    .bind(user.id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(resp::json(json!({"count": count})))
}

/// `PATCH "/notifications/{notification_id}/read"`.
pub async fn mark_notification_read(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(notification_id): Path<String>,
) -> ApiResult<Response> {
    let id = pg_uuid(&notification_id)?;
    let mut conn = db.conn().await?;
    let found: Option<(Uuid,)> = sqlx::query_as(
        "SELECT notifications.id FROM notifications WHERE notifications.id = $1 AND notifications.user_id = $2",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id,)) = found else {
        return Err(ApiError::http(404, "Notification not found"));
    };
    sqlx::query(
        "UPDATE notifications SET is_read = true, read_at = $1, updated_at = now() WHERE notifications.id = $2",
    )
    .bind(resp::iso(&Utc::now()))
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"id": id.to_string(), "is_read": true})))
}

/// `POST "/notifications/read-all"`.
pub async fn mark_all_read(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    sqlx::query(
        "UPDATE notifications SET is_read = true, read_at = $1, updated_at = now() \
         WHERE notifications.user_id = $2 AND notifications.is_read = false",
    )
    .bind(resp::iso(&Utc::now()))
    .bind(user.id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json(json!({"status": "ok"})))
}

/// A body value bound to a text column: a str, NULL, or asyncpg's DataError (a 500).
fn text_arg(k: &str, v: Option<&Value>) -> ApiResult<Option<String>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(ApiError::internal(anyhow::anyhow!(
            "DataError: invalid input for query argument ({k}): {other} (expected str)"
        ))),
    }
}

/// `POST "/notifications"` (201): an admin pushes an alert to a user.
pub async fn create_notification(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<Map<String, Value>>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let key_error = |k: &str| ApiError::internal(anyhow::anyhow!("KeyError: '{k}'"));
    let user_id = body.get("user_id").ok_or_else(|| key_error("user_id"))?;
    let type_ = text_arg("type", Some(body.get("type").unwrap_or(&json!("system"))))?;
    let title = text_arg(
        "title",
        Some(body.get("title").ok_or_else(|| key_error("title"))?),
    )?;
    let text = text_arg("body", body.get("body"))?;
    let link = text_arg("link", body.get("link"))?;
    let entity_type = text_arg("entity_type", body.get("entity_type"))?;
    let entity_id = text_arg("entity_id", body.get("entity_id"))?;
    let priority = text_arg(
        "priority",
        Some(body.get("priority").unwrap_or(&json!("normal"))),
    )?;
    // UUID column: a str is parsed (a malformed one fails the statement: a 500), None is NULL
    let uid: Option<Uuid> = match user_id {
        Value::Null => None,
        Value::String(s) => Some(pg_uuid(s)?),
        other => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "DataError: invalid input for query argument (user_id): {other}"
            )))
        }
    };
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO notifications (user_id, type, title, body, link, entity_type, entity_id, is_read, \
         priority, id) VALUES ($1, $2, $3, $4, $5, $6, $7, false, $8, $9)",
    )
    .bind(uid)
    .bind(type_)
    .bind(title)
    .bind(text)
    .bind(link)
    .bind(entity_type)
    .bind(entity_id)
    .bind(priority)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, json!({"id": id.to_string()})))
}

// ─── Dashboard KPIs ──────────────────────────────────────────────────────────

/// `GET "/dashboard-kpis"`: live company counts for the dashboard. Internal users only: partner and
/// vendor portal accounts get 403 (security triage #20).
pub async fn get_dashboard_kpis(InternalUser(user): InternalUser, db: Db) -> ApiResult<Response> {
    use granary_services::crm::stages::{ACTIVE_LEAD_STATUSES, ACTIVE_STAGES};
    let mut conn = db.conn().await?;
    let open_leads: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM leads WHERE leads.is_deleted = false AND leads.status = ANY($1)",
    )
    .bind(ACTIVE_LEAD_STATUSES)
    .fetch_one(&mut *conn)
    .await?;
    let active_deals: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM deals WHERE deals.is_deleted = false AND deals.stage = ANY($1)",
    )
    .bind(ACTIVE_STAGES)
    .fetch_one(&mut *conn)
    .await?;
    let today = granary_core::clock::today_local(&mut conn).await?;
    let overdue_invoices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM invoices WHERE invoices.is_deleted = false \
         AND (invoices.status NOT IN ('paid', 'cancelled', 'void')) AND invoices.due_date < $1",
    )
    .bind(today)
    .fetch_one(&mut *conn)
    .await?;
    let active_projects: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM projects WHERE projects.is_deleted = false \
         AND projects.status IN ('planning', 'active', 'on_hold')",
    )
    .fetch_one(&mut *conn)
    .await?;
    let unread: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE notifications.user_id = $1 AND notifications.is_read = false",
    )
    .bind(user.id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(resp::json(json!({
        "open_leads": open_leads,
        "active_deals": active_deals,
        "overdue_invoices": overdue_invoices,
        "active_projects": active_projects,
        "unread_notifications": unread,
    })))
}

// ─── Currency Settings ────────────────────────────────────────────────────────

pub const ALL_SUPPORTED: &[&str] = &["USD", "EUR", "GBP", "JPY", "SGD", "AUD", "CNY"];

/// `GET "/currencies"`.
pub async fn get_currency_settings(CurrentUser(_user): CurrentUser, db: Db) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let settings: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT system_settings.key, system_settings.value FROM system_settings WHERE system_settings.key IN \
         ('currency.functional', 'currency.active', 'currency.api_key', 'currency.last_synced', 'currency.decimals')",
    )
    .fetch_all(&mut *conn)
    .await?;
    // minor-unit digits per currency; json.loads(... or "{}"), {} on a decode error
    let raw = settings
        .iter()
        .find(|(k, _)| k == "currency.decimals")
        .map(|(_, v)| v.clone().unwrap_or_default())
        .unwrap_or_default();
    let raw = if raw.is_empty() {
        "{}".to_string()
    } else {
        raw
    };
    let decimals = match json_loads(raw.as_bytes()) {
        Ok(obj) => obj.to_json(),
        Err(_) => json!({}),
    };
    let active = granary_services::fx::get_active_currencies(&settings)?;
    let functional = granary_services::fx::get_functional_currency(&settings);
    let last_synced = settings
        .iter()
        .find(|(k, _)| k == "currency.last_synced")
        .and_then(|(_, v)| v.clone());
    let has_api_key = settings
        .iter()
        .any(|(k, v)| k == "currency.api_key" && v.as_deref().is_some_and(|v| !v.is_empty()));
    Ok(resp::json(json!({
        "functional": functional,
        "active": active,
        "all_supported": ALL_SUPPORTED,
        "has_api_key": has_api_key,
        "last_synced": last_synced,
        "decimals": decimals,
    })))
}

granary_core::py_model! {
    /// `class CurrencySettingsUpdate(BaseModel)`.
    pub struct CurrencySettingsUpdate {
        pub functional: Option<String> = None,
        pub active: Option<Vec<String>> = None,
        pub api_key: Option<String> = None,
    }
}

/// `PATCH "/currencies"`.
pub async fn update_currency_settings(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<CurrencySettingsUpdate>,
) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut updates: Vec<(&str, String)> = Vec::new();
    let mut conn = db.conn().await?;
    if let Some(f) = &body.functional {
        if !ALL_SUPPORTED.contains(&f.to_uppercase().as_str()) {
            return Err(ApiError::http(400, format!("Unsupported currency: {f}")));
        }
        guard_functional_currency_change(&mut conn, Some(f)).await?;
        updates.push(("currency.functional", f.to_uppercase()));
    }
    if let Some(active) = &body.active {
        let invalid: Vec<&String> = active
            .iter()
            .filter(|c| !ALL_SUPPORTED.contains(&c.to_uppercase().as_str()))
            .collect();
        if !invalid.is_empty() {
            let items: Vec<String> = invalid.iter().map(|c| pyjson::repr_str(c)).collect();
            return Err(ApiError::http(
                400,
                format!("Unsupported currencies: [{}]", items.join(", ")),
            ));
        }
        let list = PyValue::List(
            active
                .iter()
                .map(|c| PyValue::Str(c.to_uppercase()))
                .collect(),
        );
        let text = pyjson::dumps(&list, &DumpOpts::default())
            .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
        updates.push(("currency.active", text));
    }
    if let Some(k) = &body.api_key {
        updates.push((
            "currency.api_key",
            if k.is_empty() {
                String::new()
            } else {
                granary_services::email_crypto::encrypt(k)
            },
        ));
    }
    for (key, value) in &updates {
        let n = sqlx::query(
            "UPDATE system_settings SET value = $1, updated_by_id = $2, updated_at = now() \
             WHERE system_settings.key = $3",
        )
        .bind(value)
        .bind(user.id)
        .bind(key)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if n == 0 {
            let label = pystr::title(&key.rsplit('.').next().unwrap_or(key).replace('_', " "));
            sqlx::query(
                "INSERT INTO system_settings (key, value, value_type, category, label, is_public, is_encrypted, id) \
                 VALUES ($1, $2, $3, 'finance', $4, false, false, $5)",
            )
            .bind(key)
            .bind(value)
            .bind(if *key == "currency.active" { "json" } else { "string" })
            .bind(label)
            .bind(Uuid::new_v4())
            .execute(&mut *conn)
            .await?;
        }
    }
    drop(conn);
    db.commit().await?;
    let keys: Vec<&str> = updates.iter().map(|(k, _)| *k).collect();
    Ok(resp::json(json!({"status": "updated", "keys": keys})))
}

/// `POST "/currencies/sync-now"` (202): `celery.send_task("app.tasks.sync_exchange_rates")`.
pub async fn trigger_fx_sync(CurrentUser(user): CurrentUser) -> ApiResult<Response> {
    // whoever may enter rates by hand (finance:manage_fx) may also fetch them; the job only writes the rates
    if !user.perms.contains("finance:manage_fx") {
        require_admin(&user)?;
    }
    granary_services::tasks::delay("app.tasks.sync_exchange_rates", Vec::new(), Map::new()).await?;
    Ok(resp::json_status(
        202,
        json!({"status": "queued", "message": "Exchange rate sync has been queued"}),
    ))
}

/// `POST "/currencies/test-api-key"`: one call to ExchangeRate-API with the saved key.
pub async fn test_fx_api_key(CurrentUser(user): CurrentUser, db: Db) -> ApiResult<Response> {
    require_admin(&user)?;
    let mut conn = db.conn().await?;
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT value FROM system_settings WHERE system_settings.key = 'currency.api_key'",
    )
    .fetch_optional(&mut *conn)
    .await?;
    drop(conn);
    let raw = row.and_then(|r| r.0).unwrap_or_default();
    let api_key = if raw.is_empty() {
        String::new()
    } else {
        // backward-compat: the value may already be plaintext
        granary_services::email_crypto::decrypt(&raw).unwrap_or(raw)
    };
    if api_key.is_empty() {
        return Err(ApiError::http(400, "No API key configured"));
    }
    let url = format!(
        "{}/v6/{api_key}/latest/USD",
        granary_services::fx::exchangerate_api_base()
    );
    let failed = |e: String| ApiError::http(502, format!("API request failed: {e}"));
    let resp = granary_services::http::client()
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| failed(granary_services::fx::request_error_text(e)))?;
    let text = resp
        .bytes()
        .await
        .map_err(|e| failed(granary_services::fx::request_error_text(e)))?;
    let data = json_loads(&text)
        .map_err(|e| failed(format!("{e:?}")))?
        .to_json();
    let Value::Object(data) = data else {
        return Err(failed("'list' object has no attribute 'get'".into()));
    };
    if data.get("result").and_then(Value::as_str) == Some("success") {
        let pairs = match data.get("conversion_rates") {
            None => 0,
            Some(Value::Object(m)) => m.len(),
            Some(Value::Array(a)) => a.len(),
            Some(Value::String(s)) => s.chars().count(),
            Some(_) => return Err(failed("object has no len()".into())),
        };
        return Ok(resp::json(json!({
            "status": "ok",
            "base": data.get("base_code").cloned().unwrap_or(Value::Null),
            "pairs": pairs,
        })));
    }
    Ok(resp::json(json!({
        "status": "error",
        "detail": data.get("error-type").cloned().unwrap_or(json!("Unknown error")),
    })))
}

#[cfg(test)]
mod encrypted_keys_tests {
    use super::ENCRYPTED_SETTING_KEYS;

    /// Every Settings row the frontend shows as a secret is encrypted on save (13 Nov 2026: one was not).
    #[test]
    fn every_secret_setting_in_the_registry_is_encrypted_on_save() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../frontend/src/lib/settings-registry.ts"
        );
        let Ok(src) = std::fs::read_to_string(path) else {
            return; // the frontend is not next to this crate (a Docker build): nothing to compare
        };
        let mut missing = Vec::new();
        for line in src.lines().filter(|l| l.contains("type: \"secret\"")) {
            let Some(rest) = line.trim_start().strip_prefix("s(\"") else {
                continue;
            };
            let key = rest.split('"').next().unwrap_or_default();
            if !ENCRYPTED_SETTING_KEYS.contains(&key) {
                missing.push(key.to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "secret settings not in ENCRYPTED_SETTING_KEYS: {missing:?}"
        );
    }
}
