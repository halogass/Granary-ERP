//! Golden replay of compat-tests/golden/pricing/scenarios.json, first recorded from the retired Python
//! pricing service: every call runs in its own transaction (the world's SQL, the call, ROLLBACK) and must give
//! exactly the recorded output. Re-record with GOLDEN_BLESS=1 after an intended change and review the diff.
#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::collections::HashSet;

use chrono::{NaiveDate, Utc};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_services::pricing;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::Connection as _;

pub fn user(perms: &[&str], superadmin: bool) -> AuthUser {
    AuthUser {
        id: uuid::Uuid::nil(),
        email: "golden@example.com".into(),
        name: "Golden".into(),
        phone: None,
        avatar_url: None,
        department: None,
        position: None,
        is_active: true,
        is_superadmin: superadmin,
        is_system_admin: false,
        portal_type: "internal".into(),
        partner_account_id: None,
        vendor_id: None,
        last_login_at: None,
        must_change_password: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        perms: perms.iter().map(|s| s.to_string()).collect(),
    }
}

fn err_out(e: ApiError) -> Value {
    match e {
        ApiError::Http { status, detail, .. } => {
            json!({"error": {"status": status.as_u16(), "detail": detail}})
        }
        _ => json!({"error": "500"}),
    }
}

fn date(v: &Value) -> NaiveDate {
    NaiveDate::parse_from_str(v.as_str().unwrap(), "%Y-%m-%d").unwrap()
}

fn dec_opt(v: &Value) -> Option<Decimal> {
    v.as_str().map(|s| s.parse().unwrap())
}

fn s_opt(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

async fn run_call(conn: &mut sqlx::PgConnection, call: &Value) -> Value {
    let fn_ = call["fn"].as_str().unwrap();
    let res: Result<Value, ApiError> = async {
        match fn_ {
            "price_for" | "tier_discount" => {
                let id: uuid::Uuid = call["product"].as_str().unwrap().parse().unwrap();
                let p: granary_db::rows::sales::ProductCatalog =
                    sqlx::query_as("SELECT * FROM product_catalog WHERE id = $1").bind(id).fetch_one(&mut *conn).await?;
                if fn_ == "price_for" {
                    let q = pricing::price_for(conn, &p, call["currency"].as_str().unwrap(), date(&call["on_date"])).await?;
                    let s = |d: Decimal| granary_compat::pyjson::PyDecimal::from(d).to_string();
                    Ok(json!({"ok": {
                        "amount": s(q.amount), "currency": q.currency, "source": q.source,
                        "rate": q.rate.map(s), "valid_from": q.valid_from.map(|d| d.format("%Y-%m-%d").to_string()),
                        "base_currency": q.base_currency, "base_amount": q.base_amount.map(s)}}))
                } else {
                    let qty: Decimal = call["qty"].as_str().unwrap().parse().unwrap();
                    let t = pricing::tier_discount(conn, &p, qty).await?;
                    Ok(json!({"ok": granary_compat::pyjson::PyDecimal::from(t).to_string()}))
                }
            }
            "is_partner_org" => Ok(json!({"ok": pricing::is_partner_org(conn, call["org"].as_str()).await?})),
            "default_tax" => {
                let t = pricing::default_tax(conn, call["org"].as_str(), call["currency"].as_str()).await?;
                Ok(json!({"ok": t.map(|t| json!({"id": t.id, "code": t.code,
                    "pct": granary_compat::pyjson::PyDecimal::from(t.pct).to_string(), "name": t.name}))}))
            }
            "price_lines" => {
                let mut lines = Vec::new();
                for (i, d) in call["lines"].as_array().unwrap().iter().enumerate() {
                    lines.push(pricing::line_in_from_dict(d, i)?);
                }
                let u = match &call["user"] {
                    Value::Null => None,
                    Value::String(s) if s == "super" => Some(user(&[], true)),
                    Value::Array(a) => {
                        let p: Vec<&str> = a.iter().map(|x| x.as_str().unwrap()).collect();
                        Some(user(&p, false))
                    }
                    other => panic!("user {other}"),
                };
                let check: Option<HashSet<String>> = call["check_keys"]
                    .as_array()
                    .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect());
                let res = pricing::price_lines(
                    conn,
                    call["currency"].as_str().unwrap(),
                    date(&call["on_date"]),
                    call["customer_org_id"].as_str(),
                    &lines,
                    u.as_ref(),
                    call["enforce_permissions"].as_bool().unwrap(),
                    call["strict"].as_bool().unwrap(),
                    check.as_ref(),
                    s_opt(&call["header_discount_type"]).as_deref(),
                    dec_opt(&call["header_discount_value"]),
                )
                .await?;
                let mut out = pricing::result_json(&res);
                let terms = match pricing::suggest_payment_terms(conn, &res).await {
                    Ok(t) => t,
                    Err(e) => err_out(e),
                };
                out.as_object_mut().unwrap().insert("payment_terms".into(), terms);
                Ok(json!({ "ok": out }))
            }
            other => panic!("unknown fn {other}"),
        }
    }
    .await;
    res.unwrap_or_else(err_out)
}

/// Python's 500s record the exception type too; only "500" is compared.
fn normalize(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        if o.get("error") == Some(&json!("500")) {
            o.remove("type");
        }
    }
    v
}

fn first_diff(path: &str, a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<&String> = x.keys().collect();
            let ky: Vec<&String> = y.keys().collect();
            if kx != ky {
                return Some(format!("{path}: keys {kx:?} != {ky:?}"));
            }
            x.iter()
                .find_map(|(k, v)| first_diff(&format!("{path}.{k}"), v, &y[k]))
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: len {} != {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (p, q))| first_diff(&format!("{path}[{i}]"), p, q))
        }
        _ if a == b => None,
        _ => Some(format!("{path}: rust {a} != python {b}")),
    }
}

#[tokio::test]
async fn pricing_golden_scenarios() {
    const GOLDEN: &str = "pricing/scenarios.json";
    let mut worlds_v = bless::read(GOLDEN);
    let worlds = worlds_v.as_array_mut().unwrap();
    let pool = support::fresh_db("pricing_golden").await;
    let mut conn = pool.acquire().await.unwrap();
    // The seed gives its taxes random ids: the fixtures hold them as SEED-TAX:<code>.
    let seeded: Vec<(uuid::Uuid, String)> = sqlx::query_as("SELECT id, code FROM taxes")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let (mut total, mut bad) = (0, Vec::new());
    for (wi, w) in worlds.iter_mut().enumerate() {
        // The world once per transaction; each call under a savepoint rolled back afterwards (the calls
        // only read, so this is the state Python saw in its fresh transaction per call).
        let mut tx = conn.begin().await.unwrap();
        for stmt in w["sql"].as_array().unwrap() {
            sqlx::raw_sql(stmt.as_str().unwrap())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        for (ci, call) in w["calls"].as_array_mut().unwrap().iter_mut().enumerate() {
            total += 1;
            sqlx::raw_sql("SAVEPOINT call")
                .execute(&mut *tx)
                .await
                .unwrap();
            let got = run_call(&mut tx, call).await;
            sqlx::raw_sql("ROLLBACK TO SAVEPOINT call")
                .execute(&mut *tx)
                .await
                .unwrap();
            let mut raw = got.to_string();
            for (id, code) in &seeded {
                raw = raw.replace(&id.to_string(), &format!("SEED-TAX:{code}"));
            }
            let got: Value = serde_json::from_str(&raw).unwrap();
            if bless::blessing() {
                call["out"] = normalize(got);
                continue;
            }
            let want = normalize(call["out"].clone());
            if let Some(d) = first_diff("", &normalize(got), &want) {
                bad.push(format!("world {wi} call {ci} ({}): {d}", call["fn"]));
            }
        }
        tx.rollback().await.unwrap();
    }
    bless::save(GOLDEN, &worlds_v);
    for b in bad.iter().take(40) {
        eprintln!("{b}");
    }
    assert!(
        bad.is_empty(),
        "{} of {total} golden pricing calls differ",
        bad.len()
    );
    eprintln!("{total} golden pricing calls identical");
}
