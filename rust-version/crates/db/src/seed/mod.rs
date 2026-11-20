//! Port of backend/app/seed.py: the database seed that runs on every container start (idempotent).
//!
//! Creates the permissions, the default roles and their permission links, the superadmin (with an Owner
//! employee record), this month's document sequences, the rate card, the
//! system settings defaults, the SOP approval policies, the chart of accounts, the GL posting roles,
//! the default taxes and this year's fiscal periods. Same order, same idempotency rules, one
//! transaction, as Python.
//!
//! The large data sets (the settings defaults, the permissions and the roles that hold them, the approval
//! policies and the column defaults) live in `seed_data.json` next to this file; edit it by hand. The small
//! literal tables are written out below.
//!
//! Rows are inserted with `jsonb_populate_record`, so a JSON value is converted to the column type
//! by Postgres exactly as the explicit value SQLAlchemy would send; columns not given keep their
//! server default, as with SQLAlchemy.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Datelike, NaiveDate, Utc};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

/// The settings seed.py reads (`app.config.Settings`).
#[derive(Debug, Clone)]
pub struct SeedEnv {
    pub superadmin_email: String,
    pub superadmin_name: String,
    pub superadmin_password: String,
}

impl SeedEnv {
    /// Read SUPERADMIN_EMAIL / SUPERADMIN_NAME / SUPERADMIN_PASSWORD like pydantic-settings does:
    /// the process environment first (names matched case-insensitively), then `.env` in the
    /// current directory, then the defaults in app/config.py. SUPERADMIN_PASSWORD has no default.
    pub fn from_env() -> Result<Self> {
        let dotenv = read_dotenv(".env");
        let get = |name: &str| -> Option<String> {
            std::env::var(name)
                .ok()
                .or_else(|| {
                    std::env::vars()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                        .map(|(_, v)| v)
                })
                .or_else(|| {
                    dotenv
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                        .map(|(_, v)| v.clone())
                })
        };
        Ok(Self {
            superadmin_email: get("SUPERADMIN_EMAIL")
                .unwrap_or_else(|| "owner@example.com".to_string()),
            superadmin_name: get("SUPERADMIN_NAME").unwrap_or_else(|| "Robert Smith".to_string()),
            superadmin_password: get("SUPERADMIN_PASSWORD")
                .context("SUPERADMIN_PASSWORD is required (app.config.Settings has no default)")?,
        })
    }
}

fn read_dotenv(path: &str) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (k, v) = line.split_once('=')?;
            let v = v.trim();
            let v = if v.len() >= 2
                && ((v.starts_with('"') && v.ends_with('"'))
                    || (v.starts_with('\'') && v.ends_with('\'')))
            {
                &v[1..v.len() - 1]
            } else {
                v
            };
            Some((k.trim().to_string(), v.to_string()))
        })
        .collect()
}

/// What one run did (rows inserted / updated per step).
#[derive(Debug, Default, Clone)]
pub struct SeedReport {
    pub permissions_created: usize,
    pub roles_created: usize,
    pub role_permissions_added: usize,
    pub superadmin_created: bool,
    pub superadmin_backfilled: bool,
    pub owner_employee_created: bool,
    pub tax_config_created: bool,
    pub document_sequences_created: usize,
    pub rate_card_created: usize,
    pub system_settings_created: usize,
    pub system_settings_updated: usize,
    pub approval_policies_created: usize,
    pub discount_allowances_created: usize,
    pub accounts_created: usize,
    pub gl_roles_created: usize,
    pub taxes_created: usize,
    pub fiscal_periods_created: usize,
}

// ── data generated from Python (seed_data.json) ─────────────────────────────

#[derive(Deserialize)]
struct SeedData {
    permissions: Vec<(String, String)>,
    role_permissions: Vec<(String, Vec<String>)>,
    system_settings: Vec<(String, String, String, String, bool, String)>,
    rate_source: String,
    rates: Vec<(String, String, String)>,
    approval_policies: Vec<PolicySpec>,
    discount_allowances: Vec<Map<String, Value>>,
    column_defaults: HashMap<String, Map<String, Value>>,
}

#[derive(Deserialize)]
struct PolicySpec {
    policy: Map<String, Value>,
    steps: Vec<Map<String, Value>>,
}

static DATA: Lazy<SeedData> = Lazy::new(|| {
    serde_json::from_str(include_str!("seed_data.json")).expect("seed_data.json is valid")
});

// ── literal tables (seed.py) ────────────────────────────────────────────────

/// seed_document_sequences: (sequence_type, prefix, description).
const DOCUMENT_SEQUENCES: &[(&str, &str, &str)] = &[
    // Quotations, proforma and commercial invoices: in the company's own currency (home) or another
    // (foreign), each with its own counter
    ("quotation_home", "QUO", "Sales Quotation"),
    (
        "quotation_foreign",
        "QUO/X",
        "Sales Quotation (foreign currency)",
    ),
    ("proforma_home", "PI", "Proforma Invoice"),
    (
        "proforma_foreign",
        "PI/X",
        "Proforma Invoice (foreign currency)",
    ),
    ("invoice_home", "INV", "Commercial Invoice"),
    (
        "invoice_foreign",
        "INV/X",
        "Commercial Invoice (foreign currency)",
    ),
    // Internal entity numbers
    ("project", "PRJ", "Project Number"),
    ("employee", "EMP", "Employee Number"),
    ("partner_agreement", "PA", "Partner Agreement"),
    ("purchase_order", "PO", "Purchase Order"),
    ("work_order", "WO", "Work Order"),
    ("production", "PRD", "Production Order"),
    ("expense", "EXP", "Expense Report"),
    ("contract", "CTR", "Contract"),
    ("proposal", "PRP", "Solution Proposal"),
    ("rma", "RMA", "Return Merchandise Authorization"),
    ("journal_entry", "JE", "Journal Entry"),
    ("asset", "AST", "Fixed Asset"),
    ("rfq", "RFQ", "Request for Quotation"),
    ("goods_receipt", "GR", "Goods Receipt"),
    ("maintenance_request", "MR", "Maintenance Request"),
    ("maintenance_work_order", "MWO", "Maintenance Work Order"),
    ("helpdesk_ticket", "HD", "Helpdesk Ticket"),
    ("offer_letter", "OL", "Offer Letter"),
    ("memo", "MEMO", "Internal Memo"),
];

/// GL_POSTING_ACCOUNTS: (code, name, category, account_type, normal_balance).
pub const GL_POSTING_ACCOUNTS: &[(&str, &str, &str, &str, &str)] = &[
    ("1450", "Sales Tax Receivable", "assets", "tax", "debit"),
    (
        "2050",
        "Customer Advances",
        "liabilities",
        "payable",
        "credit",
    ),
    (
        "2410",
        "Employee Reimbursements Payable",
        "liabilities",
        "payable",
        "credit",
    ),
    (
        "2450",
        "Payroll Contributions Payable",
        "liabilities",
        "payable",
        "credit",
    ),
    (
        "4500",
        "Foreign Exchange Gain / Loss",
        "revenue",
        "revenue",
        "credit",
    ),
    (
        "4600",
        "Gain / Loss on Asset Disposal",
        "revenue",
        "revenue",
        "credit",
    ),
    (
        "5250",
        "Employer Payroll Contributions",
        "expenses",
        "expense",
        "debit",
    ),
];

/// GL_ROLE_DEFAULTS: posting role -> default account code (dict order).
pub const GL_ROLE_DEFAULTS: &[(&str, &str)] = &[
    ("ar", "1300"),
    ("customer_advances", "2050"),
    ("revenue", "4100"),
    ("sales_tax_output", "2100"),
    ("sales_tax_input", "1450"),
    ("ap", "2000"),
    ("vendor_invoice_expense", "5000"),
    ("withholding_tax_payable", "2300"),
    ("income_tax_payable", "2200"),
    ("contributions_payable", "2450"),
    ("employee_payable", "2410"),
    ("salary_expense", "5200"),
    ("employer_contributions_expense", "5250"),
    ("expense_default", "5950"),
    ("expense.travel", "5600"),
    ("expense.meals", "5600"),
    ("expense.office_supplies", "5400"),
    ("expense.hardware", "5400"),
    ("expense.utilities", "5400"),
    ("expense.software", "5700"),
    ("expense.marketing", "5500"),
    ("expense.professional_services", "5300"),
    ("expense.other", "5950"),
    ("bank_default", "1100"),
    ("fx_gain_loss", "4500"),
    ("asset_disposal_gain_loss", "4600"),
    ("retained_earnings", "3100"),
    ("current_earnings", "3200"),
];

/// seed_chart_of_accounts, before GL_POSTING_ACCOUNTS.
const CHART_OF_ACCOUNTS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "1000",
        "Cash and Cash Equivalents",
        "assets",
        "cash",
        "debit",
    ),
    ("1100", "Bank", "assets", "bank", "debit"),
    ("1200", "Bank (foreign currency)", "assets", "bank", "debit"),
    (
        "1300",
        "Accounts Receivable",
        "assets",
        "receivable",
        "debit",
    ),
    ("1400", "Prepaid Expenses", "assets", "expense", "debit"),
    ("1500", "Inventory", "assets", "expense", "debit"),
    ("1600", "Other Current Assets", "assets", "expense", "debit"),
    ("1700", "Fixed Assets", "assets", "expense", "debit"),
    (
        "1800",
        "Accumulated Depreciation",
        "assets",
        "expense",
        "credit",
    ),
    (
        "2000",
        "Accounts Payable",
        "liabilities",
        "payable",
        "credit",
    ),
    ("2100", "Sales Tax Payable", "liabilities", "tax", "credit"),
    (
        "2200",
        "Income Tax Withheld",
        "liabilities",
        "tax",
        "credit",
    ),
    (
        "2300",
        "Withholding Tax Payable",
        "liabilities",
        "tax",
        "credit",
    ),
    (
        "2400",
        "Salaries Payable",
        "liabilities",
        "payable",
        "credit",
    ),
    (
        "2500",
        "Other Current Liabilities",
        "liabilities",
        "payable",
        "credit",
    ),
    ("2600", "Long-term Debt", "liabilities", "payable", "credit"),
    ("3000", "Share Capital", "equity", "equity", "credit"),
    (
        "3100",
        "Retained Earnings",
        "equity",
        "retained_earnings",
        "credit",
    ),
    (
        "3200",
        "Current Year Earnings",
        "equity",
        "retained_earnings",
        "credit",
    ),
    ("4000", "Sales Revenue", "revenue", "revenue", "credit"),
    ("4100", "Service Revenue", "revenue", "revenue", "credit"),
    (
        "4200",
        "Hardware / Product Revenue",
        "revenue",
        "revenue",
        "credit",
    ),
    (
        "4300",
        "Subscription Revenue",
        "revenue",
        "revenue",
        "credit",
    ),
    ("4400", "Other Revenue", "revenue", "revenue", "credit"),
    ("5000", "Cost of Goods Sold", "expenses", "cogs", "debit"),
    ("5100", "Cost of Services", "expenses", "cogs", "debit"),
    ("5200", "Salaries & Wages", "expenses", "expense", "debit"),
    ("5300", "Contractor Fees", "expenses", "expense", "debit"),
    (
        "5400",
        "Office & Administrative",
        "expenses",
        "expense",
        "debit",
    ),
    (
        "5500",
        "Marketing & Advertising",
        "expenses",
        "expense",
        "debit",
    ),
    (
        "5600",
        "Travel & Accommodation",
        "expenses",
        "expense",
        "debit",
    ),
    (
        "5700",
        "Software & Subscriptions",
        "expenses",
        "expense",
        "debit",
    ),
    ("5800", "Depreciation", "expenses", "expense", "debit"),
    ("5900", "Tax Expense", "expenses", "expense", "debit"),
    (
        "5950",
        "Other Operating Expenses",
        "expenses",
        "expense",
        "debit",
    ),
];

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// No tax rules or taxes are seeded: every country's are different, so the company adds its own
/// (Finance -> Taxes).
fn tax_rules() -> Vec<Value> {
    vec![]
}

fn default_taxes() -> Vec<Value> {
    vec![]
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn qi(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Insert one ORM-style row: the model's Python-side defaults, overridden by `values`, plus a new
/// uuid4 primary key. Returns the id.
async fn insert_row(conn: &mut PgConnection, table: &str, values: Value) -> Result<Uuid> {
    let Value::Object(values) = values else {
        bail!("insert_row({table}): values must be an object");
    };
    let id = Uuid::new_v4();
    let mut row = Map::new();
    row.insert("id".into(), Value::String(id.to_string()));
    if let Some(defaults) = DATA.column_defaults.get(table) {
        for (k, v) in defaults {
            row.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in values {
        row.insert(k, v);
    }
    let cols: Vec<String> = row.keys().map(|k| qi(k)).collect();
    let cols = cols.join(", ");
    let sql = format!(
        "INSERT INTO {t} ({cols}) SELECT {cols} FROM jsonb_populate_record(NULL::{t}, $1::jsonb)",
        t = qi(table)
    );
    sqlx::query(&sql)
        .bind(sqlx::types::Json(Value::Object(row)))
        .execute(&mut *conn)
        .await
        .with_context(|| format!("insert into {table}"))?;
    Ok(id)
}

/// SQLAlchemy `scalar_one_or_none()`: more than one row is an error.
fn one_or_none<T>(mut rows: Vec<T>, what: &str) -> Result<Option<T>> {
    match rows.len() {
        0 => Ok(None),
        1 => Ok(rows.pop()),
        _ => Err(anyhow!(
            "Multiple rows were found when one or none was required ({what})"
        )),
    }
}

/// Python `str.title()` (ASCII is all the role names need, but cased-ness follows Unicode).
fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for ch in s.chars() {
        let cased = ch.is_lowercase() || ch.is_uppercase();
        if cased {
            if prev_cased {
                out.extend(ch.to_lowercase());
            } else {
                out.extend(ch.to_uppercase());
            }
        } else {
            out.push(ch);
        }
        prev_cased = cased;
    }
    out
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(28)
}

// ── steps ───────────────────────────────────────────────────────────────────

async fn seed_permissions(
    conn: &mut PgConnection,
    report: &mut SeedReport,
) -> Result<HashMap<String, Uuid>> {
    let existing: HashMap<String, Uuid> = sqlx::query("SELECT code, id FROM permissions")
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| (r.get::<String, _>(0), r.get::<Uuid, _>(1)))
        .collect();
    let mut created = HashMap::new();
    for (code, description) in &DATA.permissions {
        let module = code.split(':').next().unwrap_or(code);
        if let Some(id) = existing.get(code) {
            // A description corrected in the seed reaches installations seeded before (the Roles screen shows it).
            sqlx::query(
                "UPDATE permissions SET name = $2, description = $2 WHERE id = $1 \
                 AND description IS DISTINCT FROM $2",
            )
            .bind(id)
            .bind(description)
            .execute(&mut *conn)
            .await?;
            created.insert(code.clone(), *id);
        } else {
            let id = insert_row(
                conn,
                "permissions",
                json!({"code": code, "name": description, "description": description, "module": module}),
            )
            .await?;
            created.insert(code.clone(), id);
            report.permissions_created += 1;
        }
    }
    Ok(created)
}

async fn seed_roles(
    conn: &mut PgConnection,
    perms: &HashMap<String, Uuid>,
    report: &mut SeedReport,
) -> Result<HashMap<String, Uuid>> {
    let all: Vec<(Uuid, String, String)> = sqlx::query("SELECT id, name, slug FROM roles")
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect();
    // dict comprehensions: the last row with a given name / slug wins.
    let mut by_name = HashMap::new();
    let mut by_slug = HashMap::new();
    for (id, name, slug) in &all {
        by_name.insert(name.clone(), *id);
        by_slug.insert(slug.clone(), *id);
    }
    let mut roles = HashMap::new();
    for (role_name, _) in &DATA.role_permissions {
        let found = by_name.get(role_name).or_else(|| by_slug.get(role_name));
        let id = match found {
            Some(id) => *id,
            None => {
                let id = insert_row(
                    conn,
                    "roles",
                    json!({
                        "name": role_name,
                        "slug": role_name,
                        "description": format!("Default {} role", py_title(&role_name.replace('_', " "))),
                        "is_system_role": true,
                    }),
                )
                .await?;
                report.roles_created += 1;
                id
            }
        };
        roles.insert(role_name.clone(), id);
    }

    // Sync role -> permission associations (add missing only).
    for (role_name, codes) in &DATA.role_permissions {
        let role_id = roles[role_name];
        let current: HashSet<String> = sqlx::query(
            "SELECT p.code FROM permissions p JOIN role_permissions rpm ON rpm.permission_id = p.id \
             WHERE rpm.role_id = $1",
        )
        .bind(role_id)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
        let mut seen = HashSet::new();
        for code in codes {
            if current.contains(code) || !seen.insert(code.clone()) {
                continue;
            }
            if let Some(perm_id) = perms.get(code) {
                sqlx::query(
                    "INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)",
                )
                .bind(role_id)
                .bind(perm_id)
                .execute(&mut *conn)
                .await?;
                report.role_permissions_added += 1;
            }
        }
    }
    Ok(roles)
}

/// Permissions added in Rust after the Python freeze (not in seed_data.json, which mirrors Python): code,
/// description, the roles that get it. Their migrations grant them on existing databases; this covers a
/// fresh one, where the roles only exist once the seed made them.
const RUST_ONLY_PERMISSIONS: &[(&str, &str, &[&str])] = &[
    // batch 3 R2 (migration 0113): Nary reads staff mailboxes
    (
        "email:read_staff_mailboxes",
        "Have Nary read staff mailboxes",
        &["superadmin"],
    ),
    // doclib A1 (migration 0117): the document library
    (
        "documents:view",
        "See the document library (each category may be limited to some roles)",
        &[
            "superadmin",
            "executive",
            "team_lead",
            "sales_bizdev",
            "marketing",
            "engineer",
            "project_manager",
            "finance_admin",
            "hr_admin",
            "manufacturing_tech",
            "system_admin",
            "helpdesk_support",
            "inventory_controller",
            "maintenance_facilities_tech",
        ],
    ),
    (
        "documents:create",
        "Create documents and upload new versions of the documents they own",
        &[
            "team_lead",
            "project_manager",
            "marketing",
            "sales_bizdev",
            "executive",
            "superadmin",
        ],
    ),
    (
        "documents:manage_all",
        "Edit any document, reassign owners, approve customer and partner versions, old versions and restore, category access, trace a leaked copy",
        &["executive", "superadmin"],
    ),
    (
        "documents:admin",
        "Old versions and restore, owner change, category access, trace a leaked copy (no approval)",
        &["system_admin", "superadmin"],
    ),
];

async fn seed_rust_only_permissions(
    conn: &mut PgConnection,
    roles: &HashMap<String, Uuid>,
    report: &mut SeedReport,
) -> Result<()> {
    for (code, description, role_names) in RUST_ONLY_PERMISSIONS {
        let existing: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM permissions WHERE code = $1")
                .bind(code)
                .fetch_optional(&mut *conn)
                .await?;
        let perm_id = match existing {
            Some(id) => id,
            None => {
                let module = code.split(':').next().unwrap_or(code);
                report.permissions_created += 1;
                insert_row(
                    conn,
                    "permissions",
                    json!({"code": code, "name": description, "description": description, "module": module}),
                )
                .await?
            }
        };
        for role in *role_names {
            let Some(role_id) = roles.get(*role) else {
                continue;
            };
            let added = sqlx::query(
                "INSERT INTO role_permissions (role_id, permission_id) SELECT $1, $2 \
                 WHERE NOT EXISTS (SELECT 1 FROM role_permissions WHERE role_id = $1 AND permission_id = $2)",
            )
            .bind(role_id)
            .bind(perm_id)
            .execute(&mut *conn)
            .await?;
            report.role_permissions_added += added.rows_affected() as usize;
        }
    }
    Ok(())
}

async fn seed_superadmin(
    conn: &mut PgConnection,
    env: &SeedEnv,
    roles: &HashMap<String, Uuid>,
    report: &mut SeedReport,
) -> Result<()> {
    let rows: Vec<(Uuid, String, Option<String>, Option<String>)> =
        sqlx::query("SELECT id, email, position, department FROM users WHERE is_superadmin = true")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect();
    let superadmin_id = match one_or_none(rows, "users.is_superadmin")? {
        None => {
            let hash = granary_compat::crypto::password::hash_password(&env.superadmin_password)
                .map_err(|e| anyhow!("hash_password: {e:?}"))?;
            let id = insert_row(
                conn,
                "users",
                json!({
                    "email": env.superadmin_email,
                    "name": env.superadmin_name,
                    "password_hash": hash,
                    "is_superadmin": true,
                    "is_system_admin": true,
                    "is_active": true,
                    "portal_type": "internal",
                    "department": "Executive",
                    "position": "Owner",
                }),
            )
            .await?;
            let superadmin_role = roles
                .get("superadmin")
                .context("ROLE_PERMISSIONS has no superadmin role")?;
            sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
                .bind(id)
                .bind(superadmin_role)
                .execute(&mut *conn)
                .await?;
            report.superadmin_created = true;
            println!("[seed] Superadmin created: {}", env.superadmin_email);
            id
        }
        Some((id, email, position, department)) => {
            // Backfill position/department if not set (existing installs).
            let set_position = position.as_deref().unwrap_or("").is_empty();
            let set_department = department.as_deref().unwrap_or("").is_empty();
            if set_position || set_department {
                sqlx::query(
                    "UPDATE users SET position = CASE WHEN $2 THEN 'Owner' ELSE position END, \
                     department = CASE WHEN $3 THEN 'Executive' ELSE department END, \
                     updated_at = now() WHERE id = $1",
                )
                .bind(id)
                .bind(set_position)
                .bind(set_department)
                .execute(&mut *conn)
                .await?;
                report.superadmin_backfilled = true;
            }
            println!("[seed] Superadmin already exists: {email}");
            id
        }
    };

    // Ensure the superadmin has an Employee record as Owner.
    let emp: Vec<Uuid> = sqlx::query("SELECT id FROM employees WHERE user_id = $1")
        .bind(superadmin_id)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| r.get(0))
        .collect();
    if one_or_none(emp, "employees.user_id")?.is_none() {
        let mut parts = env.superadmin_name.splitn(2, ' ');
        let first = parts.next().unwrap_or("");
        let last = parts.next().unwrap_or("");
        insert_row(
            conn,
            "employees",
            json!({
                "user_id": superadmin_id.to_string(),
                "employee_number": "EMP/00001",
                "first_name": first,
                "last_name": last,
                "email": env.superadmin_email,
                "department": "Executive",
                "position": "Owner",
                "employment_type": "full_time",
                "status": "active",
                "start_date": "2024-01-01",
            }),
        )
        .await?;
        report.owner_employee_created = true;
        println!(
            "[seed] Owner employee record created for {}",
            env.superadmin_name
        );
    }
    Ok(())
}

/// No country tax configuration is seeded: the company adds its own (Finance -> Taxes).
async fn seed_tax_config(_conn: &mut PgConnection, _report: &mut SeedReport) -> Result<()> {
    debug_assert!(tax_rules().is_empty());
    Ok(())
}

async fn seed_document_sequences(conn: &mut PgConnection, report: &mut SeedReport) -> Result<()> {
    let now = Utc::now();
    let (year, month) = (now.year(), now.month() as i32);
    for (sequence_type, prefix, _description) in DOCUMENT_SEQUENCES {
        let rows: Vec<Uuid> = sqlx::query(
            "SELECT id FROM document_sequences WHERE sequence_type = $1 AND year = $2 AND month = $3",
        )
        .bind(sequence_type)
        .bind(year)
        .bind(month)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| r.get(0))
        .collect();
        if one_or_none(rows, "document_sequences")?.is_none() {
            insert_row(
                conn,
                "document_sequences",
                json!({
                    "sequence_type": sequence_type,
                    "prefix": prefix,
                    "year": year,
                    "month": month,
                    "current_sequence": 0,
                    "sequence_padding": 5,
                }),
            )
            .await?;
            report.document_sequences_created += 1;
        }
    }
    println!("[seed] Document sequences initialized");
    Ok(())
}

async fn seed_rate_card(conn: &mut PgConnection, report: &mut SeedReport) -> Result<()> {
    if sqlx::query("SELECT id FROM nary_rate_card LIMIT 1")
        .fetch_optional(&mut *conn)
        .await?
        .is_some()
    {
        return Ok(());
    }
    for (role, seniority, cost) in &DATA.rates {
        let cost: rust_decimal::Decimal = cost.parse().context("rate card cost")?;
        insert_row(
            conn,
            "nary_rate_card",
            json!({
                "role": role,
                "seniority": seniority,
                "cost_base": cost.to_string(),
                "source": DATA.rate_source,
                "valid_from": "2026-10-01",
                "status": "active",
            }),
        )
        .await?;
        report.rate_card_created += 1;
    }
    println!("[seed] Rate card: {} rates", DATA.rates.len());
    Ok(())
}

async fn seed_system_settings(
    conn: &mut PgConnection,
    env: &SeedEnv,
    report: &mut SeedReport,
) -> Result<()> {
    for (key, value, value_type, category, is_public, label) in &DATA.system_settings {
        let owner_key = key == "crm.funnel.default_lead_owner_email";
        let value: &str = if owner_key && value.is_empty() {
            &env.superadmin_email // the Owner owns leads nobody brought in
        } else {
            value
        };
        let rows: Vec<(Uuid, Option<String>)> =
            sqlx::query("SELECT id, value FROM system_settings WHERE key = $1")
                .bind(key)
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|r| (r.get(0), r.get(1)))
                .collect();
        match one_or_none(rows, "system_settings.key")? {
            Some((id, existing)) => {
                // Migration 082 ran on an install with no users yet. SQLAlchemy writes only a change.
                let existing = existing.unwrap_or_default();
                if owner_key && existing.trim().is_empty() && existing != value {
                    sqlx::query(
                        "UPDATE system_settings SET value = $2, updated_at = now() WHERE id = $1",
                    )
                    .bind(id)
                    .bind(value)
                    .execute(&mut *conn)
                    .await?;
                    report.system_settings_updated += 1;
                }
            }
            None => {
                insert_row(
                    conn,
                    "system_settings",
                    json!({
                        "key": key,
                        "value": value,
                        "value_type": value_type,
                        "category": category,
                        "is_public": is_public,
                        "label": label,
                    }),
                )
                .await?;
                report.system_settings_created += 1;
            }
        }
    }
    println!("[seed] System settings initialized");
    Ok(())
}

async fn seed_approval_policies(conn: &mut PgConnection, report: &mut SeedReport) -> Result<()> {
    let existing: HashSet<String> = sqlx::query("SELECT entity_type FROM approval_policies")
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    let mut created = 0;
    for spec in &DATA.approval_policies {
        let entity_type = spec.policy.get("entity_type").and_then(Value::as_str);
        if entity_type.is_some_and(|e| existing.contains(e)) {
            continue;
        }
        let policy_id = insert_row(
            conn,
            "approval_policies",
            Value::Object(spec.policy.clone()),
        )
        .await?;
        for step in &spec.steps {
            let mut step = step.clone();
            step.insert("policy_id".into(), Value::String(policy_id.to_string()));
            insert_row(conn, "approval_policy_steps", Value::Object(step)).await?;
        }
        created += 1;
    }
    report.approval_policies_created = created;
    if created > 0 {
        println!("[seed] {created} approval policies created");
    }

    let has_allowances = sqlx::query("SELECT id FROM discount_allowances LIMIT 1")
        .fetch_optional(&mut *conn)
        .await?
        .is_some();
    if !has_allowances {
        for spec in &DATA.discount_allowances {
            let mut row = spec.clone();
            row.insert("sop_reference".into(), Value::Null);
            insert_row(conn, "discount_allowances", Value::Object(row)).await?;
            report.discount_allowances_created += 1;
        }
        println!(
            "[seed] {} discount allowances created",
            DATA.discount_allowances.len()
        );
    }
    Ok(())
}

async fn seed_chart_of_accounts(conn: &mut PgConnection, report: &mut SeedReport) -> Result<()> {
    if sqlx::query("SELECT id FROM accounts LIMIT 1")
        .fetch_optional(&mut *conn)
        .await?
        .is_some()
    {
        return Ok(());
    }
    for (code, name, category, acc_type, normal_balance) in
        CHART_OF_ACCOUNTS.iter().chain(GL_POSTING_ACCOUNTS.iter())
    {
        insert_row(
            conn,
            "accounts",
            json!({
                "code": code, "name": name, "category": category,
                "account_type": acc_type, "normal_balance": normal_balance,
                "is_active": true, "allow_direct_posting": true, "currency": "USD",
            }),
        )
        .await?;
        report.accounts_created += 1;
    }
    println!("[seed] Default Chart of Accounts created");
    Ok(())
}

async fn seed_gl_roles(conn: &mut PgConnection, report: &mut SeedReport) -> Result<()> {
    let existing: HashSet<String> = sqlx::query("SELECT role FROM gl_account_roles")
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    for (role, code) in GL_ROLE_DEFAULTS {
        if existing.contains(*role) {
            continue;
        }
        let accts: Vec<Uuid> =
            sqlx::query("SELECT id FROM accounts WHERE code = $1 AND is_deleted = false")
                .bind(code)
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|r| r.get(0))
                .collect();
        if let Some(account_id) = one_or_none(accts, "accounts.code")? {
            insert_row(
                conn,
                "gl_account_roles",
                json!({"role": role, "account_id": account_id.to_string()}),
            )
            .await?;
            report.gl_roles_created += 1;
        }
    }
    Ok(())
}

async fn seed_taxes(conn: &mut PgConnection, report: &mut SeedReport) -> Result<()> {
    if sqlx::query("SELECT id FROM taxes LIMIT 1")
        .fetch_optional(&mut *conn)
        .await?
        .is_some()
    {
        return Ok(());
    }
    for t in default_taxes() {
        insert_row(conn, "taxes", t).await?;
        report.taxes_created += 1;
    }
    println!("[seed] Default taxes created");
    Ok(())
}

async fn seed_fiscal_periods(conn: &mut PgConnection, report: &mut SeedReport) -> Result<()> {
    if sqlx::query("SELECT id FROM fiscal_periods LIMIT 1")
        .fetch_optional(&mut *conn)
        .await?
        .is_some()
    {
        return Ok(());
    }
    let year = Utc::now().year();
    for m in 1..=12u32 {
        let last_day = last_day_of_month(year, m);
        insert_row(
            conn,
            "fiscal_periods",
            json!({
                "year": year,
                "month": m,
                "name": format!("{} {}", MONTH_NAMES[(m - 1) as usize], year),
                "start_date": format!("{year:04}-{m:02}-01"),
                "end_date": format!("{year:04}-{m:02}-{last_day:02}"),
                "is_locked": false,
            }),
        )
        .await?;
        report.fiscal_periods_created += 1;
    }
    println!("[seed] Fiscal periods created for {year}");
    Ok(())
}

/// `run_seed()`: every step in seed.py's order, in one transaction (rolled back on error).
pub async fn run(pool: &PgPool, env: &SeedEnv) -> Result<SeedReport> {
    let mut report = SeedReport::default();
    let mut tx = pool.begin().await?;
    println!("[seed] Starting database seed...");
    let result: Result<()> = async {
        let conn: &mut PgConnection = &mut tx;
        let perms = seed_permissions(conn, &mut report).await?;
        let roles = seed_roles(conn, &perms, &mut report).await?;
        seed_rust_only_permissions(conn, &roles, &mut report).await?;
        seed_superadmin(conn, env, &roles, &mut report).await?;
        seed_tax_config(conn, &mut report).await?;
        seed_document_sequences(conn, &mut report).await?;
        seed_rate_card(conn, &mut report).await?;
        seed_system_settings(conn, env, &mut report).await?;
        seed_approval_policies(conn, &mut report).await?;
        seed_chart_of_accounts(conn, &mut report).await?;
        seed_gl_roles(conn, &mut report).await?;
        seed_taxes(conn, &mut report).await?;
        seed_fiscal_periods(conn, &mut report).await?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            tx.commit().await?;
            println!("[seed] Seed completed successfully.");
            Ok(report)
        }
        Err(e) => {
            let _ = tx.rollback().await;
            println!("[seed] ERROR: {e:#}");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_parses_and_title_matches_python() {
        assert!(!DATA.permissions.is_empty());
        assert!(DATA.role_permissions.iter().any(|(r, _)| r == "superadmin"));
        assert_eq!(py_title("finance admin"), "Finance Admin");
        assert_eq!(
            py_title("maintenance facilities tech"),
            "Maintenance Facilities Tech"
        );
        assert_eq!(last_day_of_month(2024, 2), 29);
        assert_eq!(last_day_of_month(2026, 12), 31);
    }
}
