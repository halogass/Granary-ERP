//! tests/test_crm_search_filters.py: every operator on built-in and custom fields of each kind does what it
//! says. The HTTP calls are thin wrappers: POST /crm/attributes inserts the definition, /crm/ai/apply is
//! `writes::apply_change`, /crm/search/filters is `search::run_filters` (limit 200).
//!
//! Each test tags its records with a unique name prefix and always filters on it, so rows other tests leave
//! behind never leak into the expected sets.

mod crm_s16_common;

use std::collections::HashSet;

use chrono::{Duration, Utc};
use crm_s16_common::{db, make_person};
use granary_core::auth::AuthUser;
use granary_services::crm::search::{run_filters, SearchFail};
use granary_services::crm::writes::apply_change;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

const TAG: &str = "crm_search_filters";

fn hex8() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

async fn attr(c: &mut PgConnection, object_type: &str, field_type: &str) -> String {
    let key = format!("{field_type}_{}", hex8());
    sqlx::query(
        "INSERT INTO crm_attribute_defs (id, object_type, key, label, field_type, options, is_ai, sort_order, is_active) \
         VALUES ($1, $2, $3, $3, $4, '{}', false, 0, true)",
    )
    .bind(Uuid::new_v4())
    .bind(object_type)
    .bind(&key)
    .bind(field_type)
    .execute(&mut *c)
    .await
    .unwrap();
    key
}

fn f(field: &str, op: &str, value: Value) -> Value {
    json!({"field": field, "op": op, "value": value})
}

async fn ids(
    c: &mut PgConnection,
    user: &AuthUser,
    object_type: &str,
    prefix_field: &str,
    tag: &str,
    filters: &[Value],
) -> HashSet<String> {
    let mut all = vec![f(prefix_field, "starts_with", json!(tag))];
    all.extend(filters.iter().cloned());
    let rows = run_filters(c, user, object_type, &all, 200, 0, None)
        .await
        .unwrap_or_else(|e| panic!("{all:?}: {e:?}"));
    rows.iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

fn set(v: &[&String]) -> HashSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

struct Orgs {
    tag: String,
    number: String,
    date: String,
    checkbox: String,
    text: String,
    a: String,
    b: String,
    c: String,
    recent: String,
}

/// Three organizations: A and B carry every value, C carries none.
async fn orgs(c: &mut PgConnection, user: &AuthUser) -> Orgs {
    let tag = format!("Srch {}", hex8());
    let number = attr(c, "organization", "number").await;
    let date = attr(c, "organization", "date").await;
    let checkbox = attr(c, "organization", "checkbox").await;
    let text = attr(c, "organization", "text").await;
    let recent = (Utc::now().date_naive() - Duration::days(2)).to_string();
    let rows = [
        json!({"name": format!("{tag} 50% off"), "employee_count": 10,
               "custom": {number.clone(): 1.5, date.clone(): "2020-01-10", checkbox.clone(): true, text.clone(): "a_b"}}),
        json!({"name": format!("{tag} 500 units"), "employee_count": 50,
               "custom": {number.clone(): 20, date.clone(): recent, checkbox.clone(): false, text.clone(): "axb"}}),
        json!({"name": format!("{tag} plain")}),
    ];
    let mut out = vec![];
    for fields in rows {
        let r = apply_change(
            c,
            user,
            "organization",
            "create",
            &fields,
            None,
            "ai_confirmed",
        )
        .await
        .unwrap();
        out.push(r["id"].as_str().unwrap().to_string());
    }
    Orgs {
        tag,
        number,
        date,
        checkbox,
        text,
        a: out[0].clone(),
        b: out[1].clone(),
        c: out[2].clone(),
        recent,
    }
}

#[tokio::test]
async fn test_builtin_text_operators() {
    let d = db(TAG).await;
    let c = &mut *d.pool.acquire().await.unwrap();
    let user = make_person(c, true, &[]).await;
    let o = orgs(c, &user).await;
    let (a, b, cc, tag) = (&o.a, &o.b, &o.c, o.tag.as_str());
    macro_rules! q {
        ($($f:expr),*) => { ids(c, &user, "organization", "name", tag, &[$($f),*]).await };
    }
    // "50%" is a literal percent sign, not a wildcard: "500 units" must not match.
    assert_eq!(q!(f("name", "contains", json!("50%"))), set(&[a]));
    assert_eq!(
        q!(f("name", "starts_with", json!(format!("{tag} 50%")))),
        set(&[a])
    );
    assert_eq!(
        q!(f("name", "eq", json!(format!("{tag} PLAIN")))),
        set(&[cc])
    );
    assert_eq!(
        q!(f("name", "ne", json!(format!("{tag} plain")))),
        set(&[a, b])
    );
    assert_eq!(
        q!(f("name", "in", json!([format!("{tag} plain")]))),
        set(&[cc])
    );
    assert_eq!(q!(f("name", "is_empty", Value::Null)), set(&[]));
    assert_eq!(q!(f("name", "is_not_empty", Value::Null)), set(&[a, b, cc]));
}

#[tokio::test]
async fn test_builtin_number_operators() {
    let d = db(TAG).await;
    let c = &mut *d.pool.acquire().await.unwrap();
    let user = make_person(c, true, &[]).await;
    let o = orgs(c, &user).await;
    let (a, b, cc, tag) = (&o.a, &o.b, &o.c, o.tag.as_str());
    macro_rules! q {
        ($($f:expr),*) => { ids(c, &user, "organization", "name", tag, &[$($f),*]).await };
    }
    assert_eq!(q!(f("employee_count", "eq", json!(10))), set(&[a]));
    assert_eq!(q!(f("employee_count", "ne", json!(10))), set(&[b]));
    assert_eq!(q!(f("employee_count", "gt", json!(10))), set(&[b]));
    assert_eq!(q!(f("employee_count", "gte", json!(10))), set(&[a, b]));
    assert_eq!(q!(f("employee_count", "lt", json!(50))), set(&[a]));
    assert_eq!(q!(f("employee_count", "lte", json!("50"))), set(&[a, b]));
    assert_eq!(q!(f("employee_count", "is_empty", Value::Null)), set(&[cc]));
    assert_eq!(
        q!(f("employee_count", "is_not_empty", Value::Null)),
        set(&[a, b])
    );
}

#[tokio::test]
async fn test_builtin_datetime_operators() {
    let d = db(TAG).await;
    let c = &mut *d.pool.acquire().await.unwrap();
    let user = make_person(c, true, &[]).await;
    let o = orgs(c, &user).await;
    let tag = o.tag.as_str();
    let everyone = set(&[&o.a, &o.b, &o.c]);
    let none = HashSet::new();
    let today = Utc::now().date_naive();
    let later =
        granary_compat::pyjson::PyDateTime::from_chrono_utc(&(Utc::now() + Duration::hours(1)))
            .isoformat();
    macro_rules! q {
        ($($f:expr),*) => { ids(c, &user, "organization", "name", tag, &[$($f),*]).await };
    }
    // A bare date is the whole day, so "lte today" includes rows created today.
    assert_eq!(
        q!(f("created_at", "gte", json!(today.to_string()))),
        everyone
    );
    assert_eq!(
        q!(f("created_at", "lte", json!(today.to_string()))),
        everyone
    );
    assert_eq!(q!(f("created_at", "gt", json!(today.to_string()))), none);
    assert_eq!(q!(f("created_at", "lt", json!(today.to_string()))), none);
    assert_eq!(
        q!(f(
            "created_at",
            "gt",
            json!((today - Duration::days(1)).to_string())
        )),
        everyone
    );
    assert_eq!(q!(f("created_at", "lt", json!(later))), everyone);
    assert_eq!(q!(f("created_at", "gt", json!(later))), none);
    assert_eq!(q!(f("created_at", "after_days_ago", json!(1))), everyone);
    assert_eq!(q!(f("created_at", "before_days_ago", json!(1))), none);
}

#[tokio::test]
async fn test_builtin_datetime_rejects_garbage() {
    let d = db(TAG).await;
    let c = &mut *d.pool.acquire().await.unwrap();
    let user = make_person(c, true, &[]).await;
    let r = run_filters(
        c,
        &user,
        "organization",
        &[f("created_at", "gt", json!("last tuesday"))],
        50,
        0,
        None,
    )
    .await;
    // SearchError: the router's 400
    assert!(matches!(r, Err(SearchFail::Search(_))), "{r:?}");
}

/// Python creates the deals through /crm/ai/apply; a deal create reaches wave-3 code, so they are inserted
/// directly with the same fields.
#[tokio::test]
async fn test_builtin_date_operators() {
    let d = db(TAG).await;
    let c = &mut *d.pool.acquire().await.unwrap();
    let user = make_person(c, true, &[]).await;
    let tag = format!("SrchDeal {}", hex8());
    let future = (Utc::now().date_naive() + Duration::days(30)).to_string();
    let mut ids_by = vec![];
    for (label, close) in [
        ("D1", Some("2020-01-10")),
        ("D2", Some(future.as_str())),
        ("D3", None),
    ] {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO deals (id, title, stage, expected_close_date, is_deleted, custom) \
             VALUES ($1, $2, 'new', $3::date, false, '{}')",
        )
        .bind(id)
        .bind(format!("{tag} {label}"))
        .bind(close)
        .execute(&mut *c)
        .await
        .unwrap();
        ids_by.push(id.to_string());
    }
    let (d1, d2, d3) = (&ids_by[0], &ids_by[1], &ids_by[2]);
    macro_rules! q {
        ($($f:expr),*) => { ids(c, &user, "deal", "title", &tag, &[$($f),*]).await };
    }
    let k = "expected_close_date";
    assert_eq!(q!(f(k, "eq", json!("2020-01-10"))), set(&[d1]));
    assert_eq!(q!(f(k, "gt", json!("2020-01-10"))), set(&[d2]));
    assert_eq!(q!(f(k, "gte", json!("2020-01-10"))), set(&[d1, d2]));
    assert_eq!(q!(f(k, "lt", json!(future))), set(&[d1]));
    assert_eq!(q!(f(k, "lte", json!(future))), set(&[d1, d2]));
    assert_eq!(q!(f(k, "before_days_ago", json!(1))), set(&[d1]));
    assert_eq!(q!(f(k, "after_days_ago", json!(1))), set(&[d2]));
    assert_eq!(q!(f(k, "is_empty", Value::Null)), set(&[d3]));
    assert_eq!(q!(f(k, "is_not_empty", Value::Null)), set(&[d1, d2]));
}

#[tokio::test]
async fn test_custom_text_operators() {
    let d = db(TAG).await;
    let c = &mut *d.pool.acquire().await.unwrap();
    let user = make_person(c, true, &[]).await;
    let o = orgs(c, &user).await;
    let (a, b, cc, tag, k) = (&o.a, &o.b, &o.c, o.tag.as_str(), o.text.as_str());
    macro_rules! q {
        ($($f:expr),*) => { ids(c, &user, "organization", "name", tag, &[$($f),*]).await };
    }
    // "_" is a literal underscore, not a single-character wildcard: "axb" must not match.
    assert_eq!(q!(f(k, "contains", json!("a_b"))), set(&[a]));
    assert_eq!(q!(f(k, "starts_with", json!("a_"))), set(&[a]));
    assert_eq!(q!(f(k, "eq", json!("A_B"))), set(&[a]));
    assert_eq!(q!(f(k, "ne", json!("a_b"))), set(&[b]));
    assert_eq!(q!(f(k, "in", json!(["axb"]))), set(&[b]));
    assert_eq!(q!(f(k, "is_empty", Value::Null)), set(&[cc]));
    assert_eq!(q!(f(k, "is_not_empty", Value::Null)), set(&[a, b]));
}

#[tokio::test]
async fn test_custom_number_operators() {
    let d = db(TAG).await;
    let c = &mut *d.pool.acquire().await.unwrap();
    let user = make_person(c, true, &[]).await;
    let o = orgs(c, &user).await;
    let (a, b, cc, tag, k) = (&o.a, &o.b, &o.c, o.tag.as_str(), o.number.as_str());
    macro_rules! q {
        ($($f:expr),*) => { ids(c, &user, "organization", "name", tag, &[$($f),*]).await };
    }
    assert_eq!(q!(f(k, "eq", json!(1.5))), set(&[a]));
    assert_eq!(q!(f(k, "ne", json!(1.5))), set(&[b]));
    assert_eq!(q!(f(k, "gt", json!(1.5))), set(&[b]));
    assert_eq!(q!(f(k, "gte", json!(1.5))), set(&[a, b]));
    assert_eq!(q!(f(k, "lt", json!(20))), set(&[a]));
    assert_eq!(q!(f(k, "lte", json!(20))), set(&[a, b]));
    assert_eq!(q!(f(k, "is_empty", Value::Null)), set(&[cc]));
    assert_eq!(q!(f(k, "is_not_empty", Value::Null)), set(&[a, b]));
}

#[tokio::test]
async fn test_custom_date_operators() {
    let d = db(TAG).await;
    let c = &mut *d.pool.acquire().await.unwrap();
    let user = make_person(c, true, &[]).await;
    let o = orgs(c, &user).await;
    let (a, b, cc, tag, k) = (&o.a, &o.b, &o.c, o.tag.as_str(), o.date.as_str());
    let recent = o.recent.clone();
    macro_rules! q {
        ($($f:expr),*) => { ids(c, &user, "organization", "name", tag, &[$($f),*]).await };
    }
    assert_eq!(q!(f(k, "eq", json!("2020-01-10"))), set(&[a]));
    assert_eq!(q!(f(k, "gt", json!("2020-01-10"))), set(&[b]));
    assert_eq!(q!(f(k, "gte", json!("2020-01-10"))), set(&[a, b]));
    assert_eq!(q!(f(k, "lt", json!(recent))), set(&[a]));
    assert_eq!(q!(f(k, "lte", json!(recent))), set(&[a, b]));
    assert_eq!(q!(f(k, "before_days_ago", json!(10))), set(&[a]));
    assert_eq!(q!(f(k, "after_days_ago", json!(10))), set(&[b]));
    assert_eq!(q!(f(k, "is_empty", Value::Null)), set(&[cc]));
    assert_eq!(q!(f(k, "is_not_empty", Value::Null)), set(&[a, b]));
}

#[tokio::test]
async fn test_custom_checkbox_operators() {
    let d = db(TAG).await;
    let c = &mut *d.pool.acquire().await.unwrap();
    let user = make_person(c, true, &[]).await;
    let o = orgs(c, &user).await;
    let (a, b, cc, tag, k) = (&o.a, &o.b, &o.c, o.tag.as_str(), o.checkbox.as_str());
    macro_rules! q {
        ($($f:expr),*) => { ids(c, &user, "organization", "name", tag, &[$($f),*]).await };
    }
    assert_eq!(q!(f(k, "eq", json!(true))), set(&[a]));
    assert_eq!(q!(f(k, "eq", json!("yes"))), set(&[a]));
    // An unset checkbox counts as unchecked.
    assert_eq!(q!(f(k, "eq", json!(false))), set(&[b, cc]));
}
