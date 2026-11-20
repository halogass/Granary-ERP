//! finance.py: payment schedules (staged / milestone payments).

use crate::extract::{pg_uuid, pg_uuid_opt};
use axum::extract::Path;
use axum::response::Response;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_core::auth::{AuthUser, CurrentUser};
use granary_core::clock::today_local;
use granary_core::db::Db;
use granary_core::money::pct_of;
use granary_core::settings_store::currency_places;
use granary_core::validation::{PyInt, PyJson, PyObj, PyQuery};
use granary_core::{ApiError, ApiResult};
use granary_db::rows::finance::PaymentSchedule;
use granary_services::invoicing::{self, ItemSpec};
use granary_services::numbering::next_document_number;

use super::invoices::{insert_invoice, NewInvoice};
use super::{
    conn_commit, dstr, f, f_truthy, fromiso, now_utc, opt_text, py_str, require_perm, Dict,
};
use crate::extract::parse_uuid;
use crate::resp;

granary_core::py_model! {
    /// `class ScheduleCreate(BaseModel)`.
    pub struct ScheduleCreate {
        pub quotation_id: Option<String> = None,
        pub project_id: Option<String> = None,
        pub milestones: Vec<Dict>,
        pub currency: Option<String> = None,
    }
}

/// `D(value)` of a milestone value: `Decimal(str(x))`; Err is InvalidOperation.
fn milestone_decimal(v: &PyObj) -> Result<Decimal, ()> {
    match v {
        PyObj::Int(PyInt::Small(i)) => Ok(Decimal::from(*i)),
        PyObj::Int(PyInt::Big(s)) => s.parse::<Decimal>().map_err(|_| ()),
        PyObj::Float(x) if x.is_finite() => Ok(granary_core::money::D(*x)),
        PyObj::Str(s) => granary_core::validation::types::PyDec::parse(s)
            .and_then(|d| d.to_decimal())
            .ok_or(()),
        _ => Err(()),
    }
}

/// `m.get(key) not in (None, "")`.
fn given<'a>(m: &'a Dict, key: &str) -> Option<&'a PyObj> {
    match m.get(key) {
        None | Some(PyObj::None) => None,
        Some(PyObj::Str(s)) if s.is_empty() => None,
        Some(v) => Some(v),
    }
}

fn unprocessable(detail: String) -> ApiError {
    ApiError::http(422, detail)
}

/// `POST /payment-schedules` (201): a staged payment schedule for a quotation or project.
pub async fn create_payment_schedule(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyJson(body): PyJson<ScheduleCreate>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:create_invoice")?;
    let mut conn = db.conn().await?;
    let mut currency: Option<String> = body
        .currency
        .as_deref()
        .map(str::to_uppercase)
        .filter(|c| !c.is_empty());
    let mut pct_already = Decimal::ZERO;
    if let Some(qid) = body.quotation_id.as_deref().filter(|q| !q.is_empty()) {
        let quid = parse_uuid(qid, "quotation_id")?;
        let quotation: Option<(Uuid, Option<String>)> = sqlx::query_as(
            "SELECT id, currency FROM quotations WHERE id = $1 AND is_deleted = false",
        )
        .bind(quid)
        .fetch_optional(&mut *conn)
        .await?;
        let Some((q_id, q_currency)) = quotation else {
            return Err(ApiError::not_found("Quotation", Some(qid)));
        };
        // A schedule bills the quotation, so it is in the quotation's currency.
        if let Some(c) = &currency {
            if Some(c.as_str()) != q_currency.as_deref() {
                return Err(ApiError::http(
                    400,
                    format!(
                        "The quotation is in {}, not {c}",
                        q_currency.as_deref().unwrap_or("None")
                    ),
                ));
            }
        }
        currency = q_currency;
        pct_already = sqlx::query_scalar(
            "SELECT coalesce(sum(percentage), 0) FROM payment_schedules \
             WHERE quotation_id = $1 AND is_deleted = false",
        )
        .bind(q_id)
        .fetch_one(&mut *conn)
        .await?;
    }
    let Some(currency) = currency.filter(|c| !c.is_empty()) else {
        return Err(ApiError::http(
            400,
            "currency is required for a schedule without a quotation",
        ));
    };
    let mut pct_total = pct_already;
    let mut parsed: Vec<(Option<Decimal>, Option<Decimal>)> = Vec::new();
    for m in &body.milestones {
        let name = py_str(m.get("name").unwrap_or(&PyObj::None));
        let pct = given(m, "percentage").map(milestone_decimal);
        let amt = given(m, "amount").map(milestone_decimal);
        let (pct, amt) = match (pct.transpose(), amt.transpose()) {
            (Ok(p), Ok(a)) => (p, a),
            _ => {
                return Err(unprocessable(format!(
                    "Milestone '{name}' has a non-numeric value"
                )))
            }
        };
        if pct.is_none() && amt.is_none() {
            return Err(unprocessable(format!(
                "Milestone '{name}' needs a percentage or an amount"
            )));
        }
        if let Some(p) = pct {
            if !(p > Decimal::ZERO && p <= Decimal::ONE_HUNDRED) {
                return Err(unprocessable(format!(
                    "Milestone '{name}': percentage must be above 0 and at most 100"
                )));
            }
        }
        if let Some(a) = amt {
            if a <= Decimal::ZERO {
                return Err(unprocessable(format!(
                    "Milestone '{name}': amount must be positive"
                )));
            }
        }
        pct_total = super::dadd(pct_total, pct.unwrap_or(Decimal::ZERO));
        parsed.push((pct, amt));
    }
    if pct_total > Decimal::ONE_HUNDRED {
        return Err(ApiError::http(
            400,
            format!(
                "The milestones add up to {}% of the quotation",
                dstr(&pct_total)
            ),
        ));
    }
    let quotation_id = pg_uuid_opt(body.quotation_id.as_deref())?;
    let project_id = pg_uuid_opt(body.project_id.as_deref())?;
    let mut ids = Vec::new();
    for (m, (pct, amt)) in body.milestones.iter().zip(parsed) {
        let missing = |k: &str| {
            ApiError::internal(anyhow::anyhow!(
                "KeyError: {}",
                granary_compat::pyjson::repr_str(k)
            ))
        };
        let name = opt_text(Some(m.get("name").ok_or_else(|| missing("name"))?))?;
        // percentage=m.get("percentage") or None (the value was checked above)
        let truthy = |k: &str| m.get(k).map(super::py_truthy).unwrap_or(false);
        let percentage = if truthy("percentage") { pct } else { None };
        let amount = if truthy("amount") { amt } else { None };
        let sequence: i64 = match m.get("sequence").ok_or_else(|| missing("sequence"))? {
            PyObj::Int(PyInt::Small(i)) => *i,
            PyObj::Bool(b) => *b as i64,
            other => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "DataError: invalid input for query argument (sequence): {}",
                    super::py_repr(other)
                )))
            }
        };
        let due_trigger = match m.get("due_trigger") {
            None => Some("on_order".to_string()),
            Some(v) => opt_text(Some(v))?,
        };
        let due_date = match m.get("due_date") {
            Some(v) if super::py_truthy(v) => match v {
                PyObj::Str(s) => Some(fromiso(s)?),
                other => {
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "TypeError: fromisoformat: argument must be str, not {}",
                        super::py_type_name(other)
                    )))
                }
            },
            _ => None,
        };
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO payment_schedules (id, quotation_id, project_id, name, percentage, amount, currency, \
             sequence, due_trigger, due_date, status, created_by_id, is_deleted) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'pending', $11, false)",
        )
        .bind(id)
        .bind(quotation_id)
        .bind(project_id)
        .bind(name)
        .bind(percentage)
        .bind(amount)
        .bind(&currency)
        .bind(sequence as i32)
        .bind(due_trigger)
        .bind(due_date)
        .bind(user.id)
        .execute(&mut *conn)
        .await?;
        ids.push(id.to_string());
    }
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(
        201,
        json!({"created": ids.len(), "ids": ids}),
    ))
}

granary_core::py_model! {
    pub struct ListSchedulesQuery {
        pub quotation_id: Option<String> = None,
        pub project_id: Option<String> = None,
    }
}

/// `GET /payment-schedules`.
pub async fn list_payment_schedules(
    CurrentUser(user): CurrentUser,
    db: Db,
    PyQuery(q): PyQuery<ListSchedulesQuery>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:view_invoices")?;
    let mut conn = db.conn().await?;
    let mut sql = String::from("SELECT * FROM payment_schedules WHERE is_deleted = false");
    let mut binds: Vec<Uuid> = Vec::new();
    if let Some(v) = q.quotation_id.as_deref().filter(|s| !s.is_empty()) {
        binds.push(pg_uuid(v)?);
        sql.push_str(&format!(" AND quotation_id = ${}", binds.len()));
    }
    if let Some(v) = q.project_id.as_deref().filter(|s| !s.is_empty()) {
        binds.push(pg_uuid(v)?);
        sql.push_str(&format!(" AND project_id = ${}", binds.len()));
    }
    sql.push_str(" ORDER BY sequence");
    let mut query = sqlx::query_as::<_, PaymentSchedule>(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let schedules = query.fetch_all(&mut *conn).await?;
    let mut q_ids: Vec<Uuid> = Vec::new();
    for s in &schedules {
        if let Some(q) = s.quotation_id {
            if !q_ids.contains(&q) {
                q_ids.push(q);
            }
        }
    }
    let mut q_numbers: Vec<(Uuid, String)> = Vec::new();
    if !q_ids.is_empty() {
        let rows: Vec<(Uuid, Option<String>, String)> =
            sqlx::query_as("SELECT id, full_number, number FROM quotations WHERE id = ANY($1)")
                .bind(&q_ids)
                .fetch_all(&mut *conn)
                .await?;
        q_numbers = rows
            .into_iter()
            .map(|(id, full, number)| (id, full.filter(|f| !f.is_empty()).unwrap_or(number)))
            .collect();
    }
    let out: Vec<Value> = schedules
        .iter()
        .map(|s| {
            let qn = s.quotation_id.and_then(|q| {
                q_numbers
                    .iter()
                    .find(|(id, _)| *id == q)
                    .map(|(_, n)| n.clone())
            });
            json!({
                "id": s.id.to_string(), "name": s.name, "sequence": s.sequence,
                "quotation_id": s.quotation_id.map(|u| u.to_string()),
                "quotation_number": qn,
                "project_id": s.project_id.map(|u| u.to_string()),
                "percentage": f_truthy(s.percentage),
                "amount": f_truthy(s.amount),
                "currency": s.currency, "due_trigger": s.due_trigger,
                "due_date": resp::date_iso_opt(s.due_date.as_ref()),
                "status": s.status,
                "invoice_id": s.invoice_id.map(|u| u.to_string()),
                "proforma_invoice_id": s.proforma_invoice_id.map(|u| u.to_string()),
            })
        })
        .collect();
    Ok(resp::json(Value::Array(out)))
}

/// `raise_proforma_for_milestone(schedule_id, body, current_user, db)` without the final commit: the
/// proforma for one milestone. Shared by the route and services/nary/deals.py.
pub async fn raise_proforma_core(
    conn: &mut PgConnection,
    schedule_id: &str,
    body: &Dict,
    user: &AuthUser,
) -> ApiResult<Value> {
    require_perm(user, "finance:create_invoice")?;
    let schedule: Option<PaymentSchedule> =
        sqlx::query_as("SELECT * FROM payment_schedules WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(schedule_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(schedule) = schedule else {
        return Err(ApiError::not_found("PaymentSchedule", Some(schedule_id)));
    };
    if schedule.proforma_invoice_id.is_some() {
        return Err(ApiError::http(
            400,
            "Proforma invoice already raised for this milestone",
        ));
    }
    let Some(currency) = schedule.currency.clone() else {
        // require_rate(db, None, ...) calls None.upper()
        return Err(ApiError::internal(anyhow::anyhow!(
            "AttributeError: 'NoneType' object has no attribute 'upper'"
        )));
    };
    // Compute amount from percentage of quotation total
    let mut amount = schedule.amount;
    let pct = schedule.percentage.filter(|p| !p.is_zero());
    if amount.map(|a| a.is_zero()).unwrap_or(true) {
        if let (Some(pct), Some(qid)) = (pct, schedule.quotation_id) {
            let total: Option<Option<Decimal>> =
                sqlx::query_scalar("SELECT total FROM quotations WHERE id = $1")
                    .bind(qid)
                    .fetch_optional(&mut *conn)
                    .await?;
            if let Some(Some(total)) = total.filter(|t| t.map(|x| !x.is_zero()).unwrap_or(false)) {
                let places = currency_places(conn, Some(&currency)).await?;
                amount = Some(pct_of(total, pct, places));
            }
        }
    }
    let functional = granary_services::fx::functional_currency(conn).await?;
    let today = today_local(conn).await?;
    let rate =
        granary_services::fx::require_rate(conn, &currency, &functional, Some(today)).await?;
    let pi_seq =
        granary_services::numbering::currency_sequence(conn, "proforma", Some(&currency)).await?;
    let number = next_document_number(conn, &pi_seq).await?;
    let today = today_local(conn).await?;
    let new = NewInvoice {
        number: number.clone(),
        full_number: Some(format!("{number}-1")),
        revision: Some(1),
        revision_group: Some(number.clone()),
        invoice_type: Some("proforma".into()),
        customer_org_id: match body.get("customer_org_id") {
            Some(PyObj::Str(s)) => Some(pg_uuid(s)?),
            Some(PyObj::None) | None => None,
            Some(other) => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "ValueError: badly formed customer_org_id {}",
                    super::py_repr(other)
                )))
            }
        },
        customer_name: opt_text(body.get("customer_name"))?,
        customer_address: opt_text(body.get("customer_address"))?,
        customer_tax_id: opt_text(body.get("customer_tax_id"))?,
        quotation_id: schedule.quotation_id,
        project_id: schedule.project_id,
        currency: Some(currency.clone()),
        exchange_rate: Some(rate),
        issue_date: Some(today),
        due_date: schedule.due_date,
        payment_terms_text: Some(format!("Payment milestone: {}", schedule.name)),
        status: Some("draft".into()),
        subtotal: Some(Decimal::ZERO),
        tax_total: Some(Decimal::ZERO),
        total: Some(Decimal::ZERO),
        amount_paid: Some(Decimal::ZERO),
        amount_due: Some(Decimal::ZERO),
        notes: Some(format!(
            "Payment Schedule Ref: {} ({}%)",
            schedule.name,
            schedule
                .percentage
                .map(|p| dstr(&p))
                .unwrap_or_else(|| "None".into())
        )),
        created_by_id: Some(user.id),
        ..Default::default()
    };
    let mut pi = insert_invoice(conn, &new).await?;
    if let Some(amount) = amount.filter(|a| !a.is_zero()) {
        let spec = ItemSpec {
            name: schedule.name.clone(),
            unit: Some("Fixed".into()),
            quantity: Decimal::ONE,
            unit_price: amount,
            description: Some(format!(
                "Payment installment {}: {}",
                schedule.sequence, schedule.name
            )),
            ..Default::default()
        };
        invoicing::write_items(conn, &mut pi, &[spec], None).await?;
    }
    sqlx::query(
        "UPDATE payment_schedules SET proforma_invoice_id = $2, status = 'invoiced', updated_at = now() \
         WHERE id = $1",
    )
    .bind(schedule.id)
    .bind(pi.id)
    .execute(&mut *conn)
    .await?;
    Ok(json!({"id": pi.id.to_string(), "number": pi.number,
              "amount": f(&pi.total.unwrap_or(Decimal::ZERO))}))
}

/// `POST /payment-schedules/{schedule_id}/raise-proforma` (201).
pub async fn raise_proforma_for_milestone(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(schedule_id): Path<String>,
    PyJson(body): PyJson<Dict>,
) -> ApiResult<Response> {
    let mut conn = db.conn().await?;
    let out = raise_proforma_core(&mut conn, &schedule_id, &body, &user).await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::json_status(201, out))
}

/// The router function as services/nary/deals.py calls it (installed in later/nary_deals.rs): the
/// caller's connection, and the commit Python's function ends with.
pub fn raise_proforma_hook<'a>(
    schedule_id: &'a str,
    body: Value,
    actor: &'a AuthUser,
    db: &'a mut PgConnection,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ApiResult<Value>> + Send + 'a>> {
    Box::pin(async move {
        let dict = match PyObj::from_json(&body) {
            PyObj::Dict(d) => d,
            _ => Dict::new(),
        };
        let out = raise_proforma_core(db, schedule_id, &dict, actor).await?;
        conn_commit(db).await?;
        Ok(out)
    })
}

/// `DELETE /payment-schedules/{schedule_id}` (204): only pending milestones.
pub async fn delete_payment_schedule(
    CurrentUser(user): CurrentUser,
    db: Db,
    Path(schedule_id): Path<String>,
) -> ApiResult<Response> {
    require_perm(&user, "finance:create_invoice")?;
    let mut conn = db.conn().await?;
    let schedule: Option<PaymentSchedule> =
        sqlx::query_as("SELECT * FROM payment_schedules WHERE id = $1 AND is_deleted = false")
            .bind(pg_uuid(&schedule_id)?)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(schedule) = schedule else {
        return Err(ApiError::not_found("PaymentSchedule", Some(&schedule_id)));
    };
    if schedule.status.as_deref() != Some("pending") {
        return Err(ApiError::http(400, "Only pending schedules can be deleted"));
    }
    sqlx::query(
        "UPDATE payment_schedules SET is_deleted = true, deleted_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(schedule.id)
    .bind(now_utc())
    .execute(&mut *conn)
    .await?;
    drop(conn);
    db.commit().await?;
    Ok(resp::no_content())
}
