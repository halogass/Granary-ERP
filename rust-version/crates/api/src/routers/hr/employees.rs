//! hr.py: the job-title catalogue, employees and termination requests.

use crate::extract::{pg_uuid, pg_uuid_opt};
use std::collections::BTreeSet;

use axum::extract::Path;
use axum::response::Response;
use serde_json::{json, Value};
use uuid::Uuid;

use granary_core::auth::InternalUser;
use granary_core::db::Db;
use granary_core::validation::{PyJson, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::hr::Employee;
use rust_decimal::Decimal;

use super::{full_name, has, obj, py_date_or_422, require, short_id, utc_now};
use crate::resp;

/// `JOB_TITLES` (core/permissions.py), in order.
pub const JOB_TITLES: &[&str] = &[
    "COO",
    "CFO / Finance Director",
    "VP / Head of Sales",
    "VP / Head of Engineering",
    "Technical Lead / Architect",
    "Lead Product Engineer",
    "Fullstack Developer (Web)",
    "Frontend Developer",
    "Backend Developer",
    "Mobile Developer",
    "AI/ML Engineer",
    "DevOps Engineer",
    "Firmware / Embedded Engineer",
    "Software/AI Engineer (Op Tech)",
    "QA / Test Engineer",
    "Product Manager (Internal)",
    "UI/UX Designer",
    "Business Development Representative (BDR)",
    "Channel/Partner Manager",
    "Customer Success Manager",
    "Technical Pre-sales / Solutions Architect",
    "Marketing & Content Specialist",
    "Project Manager",
    "Operations & Sales Support",
    "Finance & Operations Admin",
    "HR Generalist",
    "Manufacturing & Production Lead",
    "Production Technician",
    "Warehouse / Inventory Controller",
    "Helpdesk Technician / Support Specialist",
    "Maintenance Technician / Facilities Tech",
];

/// `JOB_TITLE_ROLE_MAP` (core/permissions.py).
pub fn job_title_roles(title: &str) -> &'static [&'static str] {
    match title {
        "COO" => &["executive", "team_lead"],
        "CFO / Finance Director" => &["executive", "finance_admin"],
        "VP / Head of Sales" => &["executive", "sales_bizdev", "team_lead"],
        "VP / Head of Engineering" => &["executive", "engineer", "team_lead"],
        "Technical Lead / Architect" => &["engineer", "team_lead"],
        "Lead Product Engineer" => &["engineer", "team_lead"],
        "Fullstack Developer (Web)" => &["engineer"],
        "Frontend Developer" => &["engineer"],
        "Backend Developer" => &["engineer"],
        "Mobile Developer" => &["engineer"],
        "AI/ML Engineer" => &["engineer"],
        "DevOps Engineer" => &["engineer", "team_lead"],
        "Firmware / Embedded Engineer" => &["engineer"],
        "Software/AI Engineer (Op Tech)" => &["engineer", "project_manager"],
        "QA / Test Engineer" => &["engineer"],
        "Product Manager (Internal)" => &["project_manager"],
        "UI/UX Designer" => &["engineer", "marketing"],
        "Business Development Representative (BDR)" => &["sales_bizdev"],
        "Channel/Partner Manager" => &["sales_bizdev"],
        "Customer Success Manager" => &["sales_bizdev"],
        "Technical Pre-sales / Solutions Architect" => &["engineer", "sales_bizdev"],
        "Marketing & Content Specialist" => &["marketing"],
        "Project Manager" => &["project_manager", "team_lead"],
        "Operations & Sales Support" => &["sales_bizdev", "project_manager"],
        "Finance & Operations Admin" => &["finance_admin"],
        "HR Generalist" => &["hr_admin"],
        "Manufacturing & Production Lead" => &["manufacturing_tech", "team_lead"],
        "Production Technician" => &["manufacturing_tech"],
        "Warehouse / Inventory Controller" => &["inventory_controller"],
        "Helpdesk Technician / Support Specialist" => &["helpdesk_support"],
        "Maintenance Technician / Facilities Tech" => &["maintenance_facilities_tech"],
        _ => &[],
    }
}

/// `ROLE_PERMISSIONS` (core/permissions.py), as the seed carries it.
static ROLE_PERMISSIONS: std::sync::LazyLock<Vec<(String, Vec<String>)>> =
    std::sync::LazyLock::new(|| {
        #[derive(serde::Deserialize)]
        struct Seed {
            role_permissions: Vec<(String, Vec<String>)>,
        }
        serde_json::from_str::<Seed>(include_str!("../../../../db/src/seed/seed_data.json"))
            .expect("seed_data.json role_permissions")
            .role_permissions
    });

fn role_permissions(role: &str) -> &'static [String] {
    ROLE_PERMISSIONS
        .iter()
        .find(|(r, _)| r == role)
        .map(|(_, p)| p.as_slice())
        .unwrap_or(&[])
}

/// `GET /job-titles` (public): each title with its default roles and their permissions (sorted).
pub async fn list_job_titles() -> ApiResult<Response> {
    let out: Vec<Value> = JOB_TITLES
        .iter()
        .map(|title| {
            let roles = job_title_roles(title);
            let mut perms: BTreeSet<&str> = BTreeSet::new();
            for role in roles {
                perms.extend(role_permissions(role).iter().map(String::as_str));
            }
            json!({"title": title, "default_roles": roles,
                   "default_permissions": perms.into_iter().collect::<Vec<_>>()})
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

granary_core::py_model! {
    /// `class EmployeeCreate(BaseModel)`.
    pub struct EmployeeCreate {
        pub user_id: Option<String> = None,
        pub first_name: Option<String> = None,
        pub last_name: Option<String> = None,
        pub full_name: Option<String> = None,
        pub email: String,
        pub phone: Option<String> = None,
        pub department: Option<String> = None,
        pub position: Option<String> = None,
        pub job_title: Option<String> = None,
        pub employment_type: String = "full_time",
        pub start_date: String,
        pub reports_to_id: Option<String> = None,
        pub base_salary: Option<Decimal> = None,
        pub salary_currency: Option<String> = None,
        pub create_user: bool = false,
        pub temp_password: Option<String> = None,
    }
}

granary_core::py_model! {
    /// `class EmployeeUpdate(BaseModel)`.
    pub struct EmployeeUpdate {
        pub first_name: Option<String> = None,
        pub last_name: Option<String> = None,
        pub department: Option<String> = None,
        pub position: Option<String> = None,
        pub job_title: Option<String> = None,
        pub phone: Option<String> = None,
        pub reports_to_id: Option<String> = None,
        /// Granary: the salary payroll pays, with hr:manage_compensation only.
        pub base_salary: Option<Decimal> = None,
        pub salary_currency: Option<String> = None,
    }
}

/// Granary: setting a salary is compensation work. Payroll pays `base_salary`, so only the superadmin or
/// a holder of hr:manage_compensation sets it (hr:manage_employees alone edits the rest of the record).
fn require_salary_rights(user: &granary_core::auth::AuthUser) -> ApiResult<()> {
    if user.is_superadmin || has(user, "hr:manage_compensation") {
        Ok(())
    } else {
        Err(ApiError::http(
            403,
            "Permission required: hr:manage_compensation",
        ))
    }
}

/// A salary amount: not negative.
fn check_salary(salary: Option<Decimal>) -> ApiResult<()> {
    if salary.is_some_and(|s| s.is_sign_negative() && !s.is_zero()) {
        return Err(ApiError::http(400, "base_salary cannot be negative"));
    }
    Ok(())
}

/// A currency code as sent (" idr " -> "IDR"); `None` when blank. Three letters, as ISO 4217.
fn salary_currency_code(raw: Option<&str>) -> ApiResult<Option<String>> {
    let Some(c) = raw.map(str::trim).filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let c = c.to_ascii_uppercase();
    if c.len() != 3 || !c.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return Err(ApiError::http(
            400,
            "salary_currency must be a three-letter currency code",
        ));
    }
    Ok(Some(c))
}

granary_core::py_model! {
    /// `class TerminationRequestCreate(BaseModel)`.
    pub struct TerminationRequestCreate {
        pub employee_id: String,
        pub reason: String,
        pub last_working_date: String,
        pub notes: Option<String> = None,
    }
}

granary_core::py_model! {
    /// Query of `GET /employees`.
    pub struct EmployeesQ {
        pub q: Option<String> = None,
        pub limit: i64 = 50,
        pub offset: i64 = 0,
    }
}

/// `select(Employee).where(Employee.id == employee_id, Employee.is_deleted == False)`.
pub(crate) async fn live_employee(
    conn: &mut sqlx::PgConnection,
    employee_id: &str,
) -> ApiResult<Option<Employee>> {
    let id = pg_uuid(employee_id)?;
    Ok(
        sqlx::query_as("SELECT * FROM employees WHERE id = $1 AND is_deleted = false")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// `GET /employees`.
pub async fn list_employees(
    InternalUser(user): InternalUser,
    db: Db,
    PyQuery(q): PyQuery<EmployeesQ>,
) -> ApiResult<Response> {
    if !(user.is_superadmin
        || user.is_system_admin
        || has(&user, "hr:view_all_employees")
        || has(&user, "hr:manage_employees"))
    {
        return Err(ApiError::http(
            403,
            "Permission required: hr:view_all_employees",
        ));
    }
    let mut conn = db.conn().await?;
    let mut sql = String::from("SELECT * FROM employees WHERE is_deleted = false");
    let pattern =
        q.q.as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{}%", granary_compat::text::utils::escape_like(s)));
    if pattern.is_some() {
        sql.push_str(
            " AND (first_name ILIKE $3 ESCAPE '\\' OR last_name ILIKE $3 ESCAPE '\\' \
             OR email ILIKE $3 ESCAPE '\\')",
        );
    }
    sql.push_str(" ORDER BY created_at LIMIT $1 OFFSET $2");
    let mut query = sqlx::query_as::<_, Employee>(&sql)
        .bind(q.limit)
        .bind(q.offset);
    if let Some(p) = &pattern {
        query = query.bind(p);
    }
    let rows = query.fetch_all(&mut *conn).await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|e| {
            obj(vec![
                ("id", json!(e.id.to_string())),
                (
                    "employee_id",
                    json!(e
                        .employee_number
                        .clone()
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| short_id(e.id))),
                ),
                (
                    "full_name",
                    json!(full_name(&e.first_name, e.last_name.as_deref())),
                ),
                ("first_name", json!(e.first_name)),
                ("last_name", json!(e.last_name)),
                ("email", json!(e.email)),
                ("phone", json!(e.phone)),
                ("department", json!(e.department)),
                (
                    "reports_to_id",
                    json!(e.reports_to_id.map(|u| u.to_string())),
                ),
                ("job_title", json!(e.position)),
                ("employment_type", json!(e.employment_type)),
                ("status", json!(e.status)),
                (
                    "start_date",
                    json!(resp::date_iso_opt(e.start_date.as_ref())),
                ),
                ("user_id", json!(e.user_id.map(|u| u.to_string()))),
            ])
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `POST /employees` (201).
pub async fn create_employee(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<EmployeeCreate>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_employees")?;
    if body.base_salary.is_some() {
        require_salary_rights(&user)?;
    }
    check_salary(body.base_salary)?;
    // Resolve first/last name
    let mut first_name = body.first_name.clone();
    let mut last_name = body.last_name.clone();
    if first_name.as_deref().is_none_or(str::is_empty) {
        if let Some(full) = body.full_name.as_deref().filter(|s| !s.is_empty()) {
            let full = full.trim();
            let mut parts = full.splitn(2, ' ');
            first_name = Some(parts.next().unwrap_or("").to_string());
            last_name = parts.next().map(str::to_string);
        }
    }
    let first_name = match first_name.filter(|s| !s.is_empty()) {
        Some(f) => f,
        None => return Err(ApiError::http(400, "first_name or full_name is required")),
    };
    // The page requires a password with "create login"; an API caller that leaves it blank used to get
    // user_created: true with no account made.
    if body.create_user
        && granary_compat::text::pystr::strip(body.temp_password.as_deref().unwrap_or(""))
            .is_empty()
    {
        return Err(ApiError::http(
            400,
            "temp_password is required when create_user is set",
        ));
    }
    let position = body
        .position
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| body.job_title.clone());

    let mut conn = db.conn().await?;
    // A new employee is paid in the company's functional currency unless the request names one.
    let salary_currency = match salary_currency_code(body.salary_currency.as_deref())? {
        Some(c) => c,
        None => granary_services::fx::functional_currency(&mut conn).await?,
    };
    let emp_number =
        granary_services::numbering::next_document_number(&mut conn, "employee").await?;
    let start_date = py_date_or_422(&body.start_date)?;
    let employee_id = Uuid::new_v4();
    // flush: the str ids are bound to UUID columns here.
    let user_id = pg_uuid_opt(body.user_id.as_deref())?;
    let reports_to_id = pg_uuid_opt(body.reports_to_id.as_deref())?;
    sqlx::query(
        "INSERT INTO employees (id, employee_number, user_id, first_name, last_name, email, phone, department, \
         position, employment_type, start_date, reports_to_id, base_salary, salary_currency, status, \
         created_by_id, id_type, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, 'active', $15, 'national_id', false)",
    )
    .bind(employee_id)
    .bind(&emp_number)
    .bind(user_id)
    .bind(&first_name)
    .bind(&last_name)
    .bind(&body.email)
    .bind(&body.phone)
    .bind(&body.department)
    .bind(&position)
    .bind(&body.employment_type)
    .bind(start_date)
    .bind(reports_to_id)
    .bind(body.base_salary)
    .bind(&salary_currency)
    .bind(user.id)
    .execute(&mut *conn)
    .await?;

    // Auto-link to an existing internal User with the same email when no explicit user_id or create_user flag
    let no_user_id = body.user_id.as_deref().is_none_or(str::is_empty);
    if no_user_id && !body.create_user && !body.email.is_empty() {
        let found: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM users WHERE email = $1 AND portal_type = 'internal' AND is_active = true LIMIT 2",
        )
        .bind(&body.email)
        .fetch_all(&mut *conn)
        .await?;
        if found.len() > 1 {
            return Err(multiple_rows());
        }
        if let Some((uid,)) = found.first() {
            sqlx::query("UPDATE employees SET user_id = $2, updated_at = now() WHERE id = $1")
                .bind(employee_id)
                .bind(uid)
                .execute(&mut *conn)
                .await?;
        }
    }

    // Optionally create linked user account
    let mut user_created = false;
    if let Some(temp_password) = body.temp_password.as_deref().filter(|_| body.create_user) {
        let existing: Vec<(Uuid,)> =
            sqlx::query_as("SELECT id FROM users WHERE email = $1 LIMIT 2")
                .bind(&body.email)
                .fetch_all(&mut *conn)
                .await?;
        if existing.len() > 1 {
            return Err(multiple_rows());
        }
        if !existing.is_empty() {
            return Err(ApiError::http(
                400,
                format!("A user with email {} already exists", body.email),
            ));
        }
        let full = full_name(&first_name, last_name.as_deref());
        let hash = granary_compat::crypto::password::hash_password(temp_password)
            .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
        let new_user_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, name, password_hash, phone, department, position, \
             must_change_password, is_active, portal_type, email_signature_enabled, is_superadmin, \
             is_system_admin) VALUES ($1, $2, $3, $4, $5, $6, $7, true, true, 'internal', true, false, false)",
        )
        .bind(new_user_id)
        .bind(&body.email)
        .bind(&full)
        .bind(&hash)
        .bind(&body.phone)
        .bind(&body.department)
        .bind(&position)
        .execute(&mut *conn)
        .await?;
        sqlx::query("UPDATE employees SET user_id = $2, updated_at = now() WHERE id = $1")
            .bind(employee_id)
            .bind(new_user_id)
            .execute(&mut *conn)
            .await?;
        user_created = true;

        // Auto-assign role bundle based on job title
        if let Some(pos) = position.as_deref().filter(|p| !p.is_empty()) {
            let role_names: Vec<String> =
                job_title_roles(pos).iter().map(|s| s.to_string()).collect();
            if !role_names.is_empty() {
                let role_ids: Vec<(Uuid,)> =
                    sqlx::query_as("SELECT id FROM roles WHERE name = ANY($1)")
                        .bind(&role_names)
                        .fetch_all(&mut *conn)
                        .await?;
                for (rid,) in role_ids {
                    // Security 26 Oct: like user creation, someone who is not an admin hands out only roles
                    // whose permissions they hold (an HR admin could make a CFO login for themselves)
                    if !(user.is_superadmin || user.is_system_admin) {
                        let missing: Option<String> = sqlx::query_scalar(
                            "SELECT p.code FROM role_permissions rpm JOIN permissions p ON p.id = rpm.permission_id \
                             WHERE rpm.role_id = $1 AND NOT (p.code = ANY($2)) LIMIT 1",
                        )
                        .bind(rid)
                        .bind(user.perms.iter().cloned().collect::<Vec<String>>())
                        .fetch_optional(&mut *conn)
                        .await?;
                        if let Some(code) = missing {
                            tracing::warn!(role_id = %rid, missing = %code, by = %user.id,
                                           "hr_employee_role_not_granted");
                            continue;
                        }
                    }
                    sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
                        .bind(new_user_id)
                        .bind(rid)
                        .execute(&mut *conn)
                        .await?;
                }
            }
        }
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({
            "id": employee_id.to_string(),
            "first_name": first_name,
            "user_created": user_created,
        }),
    ))
}

/// `scalar_one_or_none()` with two rows: MultipleResultsFound, the 500.
pub(crate) fn multiple_rows() -> ApiError {
    ApiError::internal(anyhow::anyhow!(
        "Multiple rows were found when one or none was required"
    ))
}

/// `PATCH /employees/{employee_id}`.
pub async fn update_employee(
    InternalUser(user): InternalUser,
    db: Db,
    Path(employee_id): Path<String>,
    PyJson(body): PyJson<EmployeeUpdate>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_employees")?;
    let salary_currency = salary_currency_code(body.salary_currency.as_deref())?;
    let sets_salary = body.base_salary.is_some() || salary_currency.is_some();
    if sets_salary {
        require_salary_rights(&user)?;
    }
    check_salary(body.base_salary)?;
    let mut conn = db.conn().await?;
    let Some(emp) = live_employee(&mut conn, &employee_id).await? else {
        return Err(ApiError::not_found("Employee", Some(&employee_id)));
    };
    let mut first_name = emp.first_name.clone();
    let mut last_name = emp.last_name.clone();
    let mut department = emp.department.clone();
    let mut position = emp.position.clone();
    let mut phone = emp.phone.clone();
    let mut reports_to_id = emp.reports_to_id;
    if let Some(v) = &body.first_name {
        first_name = v.clone();
    }
    if body.last_name.is_some() {
        last_name = body.last_name.clone();
    }
    if body.department.is_some() {
        department = body.department.clone();
    }
    if body.position.is_some() {
        position = body.position.clone();
    }
    if body.job_title.is_some() {
        position = body.job_title.clone();
    }
    if body.phone.is_some() {
        phone = body.phone.clone();
    }
    if let Some(r) = &body.reports_to_id {
        reports_to_id = if r.is_empty() {
            None
        } else {
            Some(pg_uuid(r)?)
        };
    }
    let changed = first_name != emp.first_name
        || last_name != emp.last_name
        || department != emp.department
        || position != emp.position
        || phone != emp.phone
        || reports_to_id != emp.reports_to_id;

    // Sync changes to linked user account
    if let Some(uid) = emp.user_id {
        let linked: Option<(String, Option<String>, Option<String>)> =
            sqlx::query_as("SELECT name, department, position FROM users WHERE id = $1")
                .bind(uid)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some((old_name, old_dept, old_pos)) = linked {
            let name = full_name(&first_name, last_name.as_deref());
            let dept = if body.department.is_some() {
                department.clone()
            } else {
                old_dept.clone()
            };
            let pos = if body.position.is_some() || body.job_title.is_some() {
                position.clone()
            } else {
                old_pos.clone()
            };
            if name != old_name || dept != old_dept || pos != old_pos {
                sqlx::query(
                    "UPDATE users SET name = $2, department = $3, position = $4, updated_at = now() WHERE id = $1",
                )
                .bind(uid)
                .bind(&name)
                .bind(&dept)
                .bind(&pos)
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    if changed {
        sqlx::query(
            "UPDATE employees SET first_name = $2, last_name = $3, department = $4, position = $5, phone = $6, \
             reports_to_id = $7, updated_at = now() WHERE id = $1",
        )
        .bind(emp.id)
        .bind(&first_name)
        .bind(&last_name)
        .bind(&department)
        .bind(&position)
        .bind(&phone)
        .bind(reports_to_id)
        .execute(&mut *conn)
        .await?;
    }
    // Granary: the salary, only with hr:manage_compensation (checked above). A first salary with no currency
    // is in the company's functional currency.
    let mut salary_out: Option<(Option<Decimal>, Option<String>)> = None;
    if sets_salary {
        let base_salary = body.base_salary.or(emp.base_salary);
        let currency = match salary_currency.or_else(|| emp.salary_currency.clone()) {
            Some(c) => c,
            None => granary_services::fx::functional_currency(&mut conn).await?,
        };
        sqlx::query(
            "UPDATE employees SET base_salary = $2, salary_currency = $3, updated_at = now() WHERE id = $1",
        )
        .bind(emp.id)
        .bind(base_salary)
        .bind(&currency)
        .execute(&mut *conn)
        .await?;
        salary_out = Some((base_salary, Some(currency)));
    }
    drop(conn);
    db.commit().await?;
    let mut out = json!({
        "id": emp.id.to_string(),
        "full_name": full_name(&first_name, last_name.as_deref()),
        "first_name": first_name, "last_name": last_name,
        "department": department, "job_title": position,
        "phone": phone,
    });
    if let Some((salary, currency)) = salary_out {
        out["base_salary"] = match salary {
            Some(s) => resp::dec_float(&s),
            None => Value::Null,
        };
        out["salary_currency"] = json!(currency);
    }
    Ok(resp::json(out))
}

/// `DELETE /employees/{employee_id}` (204): soft delete; the linked login is deactivated.
pub async fn delete_employee(
    InternalUser(user): InternalUser,
    db: Db,
    Path(employee_id): Path<String>,
) -> ApiResult<Response> {
    require(&user, "hr:manage_employees")?;
    let mut conn = db.conn().await?;
    let Some(emp) = live_employee(&mut conn, &employee_id).await? else {
        return Err(ApiError::not_found("Employee", Some(&employee_id)));
    };
    sqlx::query(
        "UPDATE employees SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(emp.id)
    .bind(utc_now())
    .execute(&mut *conn)
    .await?;
    // Deactivate the linked system user so they can no longer log in
    if let Some(uid) = emp.user_id {
        sqlx::query(
            "UPDATE users SET is_active = false, updated_at = now() \
             WHERE id = $1 AND is_active = true AND is_superadmin = false",
        )
        .bind(uid)
        .execute(&mut *conn)
        .await?;
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}

/// `GET /employees/{employee_id}`: any internal user; the salary only with hr:view_compensation.
pub async fn get_employee(
    InternalUser(user): InternalUser,
    db: Db,
    Path(employee_id): Path<String>,
) -> ApiResult<Response> {
    let is_hr = user.is_superadmin
        || user.is_system_admin
        || has(&user, "hr:view_all_employees")
        || has(&user, "hr:manage_employees");
    let mut conn = db.conn().await?;
    let found = live_employee(&mut conn, &employee_id).await?;
    // Same permissions as the employee list, plus the employee's own record. A non-HR caller gets 403
    // whether or not the id exists, so the route does not reveal which ids are real.
    if !is_hr && !found.as_ref().is_some_and(|e| e.user_id == Some(user.id)) {
        return Err(ApiError::http(
            403,
            "Permission required: hr:view_all_employees",
        ));
    }
    let Some(e) = found else {
        return Err(ApiError::not_found("Employee", Some(&employee_id)));
    };
    let show_salary = user.is_superadmin || has(&user, "hr:view_compensation");
    let salary = match e.base_salary {
        Some(s) if show_salary && !s.is_zero() => resp::dec_float(&s),
        _ => Value::Null,
    };
    Ok(resp::json(obj(vec![
        ("id", json!(e.id.to_string())),
        ("first_name", json!(e.first_name)),
        ("last_name", json!(e.last_name)),
        ("email", json!(e.email)),
        ("phone", json!(e.phone)),
        ("department", json!(e.department)),
        ("job_title", json!(e.position)),
        ("employment_type", json!(e.employment_type)),
        ("status", json!(e.status)),
        (
            "start_date",
            json!(resp::date_iso_opt(e.start_date.as_ref())),
        ),
        ("base_salary", json!(salary)),
        (
            "salary_currency",
            json!(if show_salary {
                json!(e.salary_currency)
            } else {
                Value::Null
            }),
        ),
    ])))
}

/// `POST /termination-requests` (201): only the direct senior (hr:initiate_termination), never HR.
pub async fn create_termination_request(
    InternalUser(user): InternalUser,
    db: Db,
    PyJson(body): PyJson<TerminationRequestCreate>,
) -> ApiResult<Response> {
    if !has(&user, "hr:initiate_termination") && !user.is_superadmin {
        return Err(ApiError::forbidden(Some(
            "Only direct seniors can initiate termination requests",
        )));
    }
    let last_working_date = py_date_or_422(&body.last_working_date)?;
    let employee_id = pg_uuid(&body.employee_id)?;
    let id = Uuid::new_v4();
    let mut conn = db.conn().await?;
    sqlx::query(
        "INSERT INTO termination_requests (id, employee_id, initiated_by_id, reason, last_working_date, notes, \
         status, exit_interview_done, assets_returned, accounts_deactivated, final_payment_processed) \
         VALUES ($1, $2, $3, $4, $5, $6, 'pending_owner_approval', false, false, false, false)",
    )
    .bind(id)
    .bind(employee_id)
    .bind(user.id)
    .bind(&body.reason)
    .bind(last_working_date)
    .bind(&body.notes)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"id": id.to_string(), "status": "pending_owner_approval"}),
    ))
}
