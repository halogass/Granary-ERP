//! services/crm/search.py: _coerce goldens, field_schema / count_filters / run_filters replayed on the
//! scenario database (same rows, same order, same refusals as Python), plus the search tests of
//! backend/tests/test_crm_ai_unit.py and test_crm_ai_api.py (Phase 4, service level).
#[path = "golden/bless.rs"]
mod bless;
mod crm_s15_common;
mod support;

use granary_services::crm::search::{
    _coerce, count_filters, field_schema, run_filters, Coerced, SearchFail,
};
use serde_json::{json, Value};

fn typed(c: &Coerced) -> Value {
    match c {
        Coerced::None => json!(["none"]),
        Coerced::Bool(b) => json!(["bool", b]),
        Coerced::Int(i) => json!(["int", i.to_string()]),
        Coerced::BigInt(s) => json!(["int", s]),
        Coerced::Float(f) => json!(["float", granary_compat::pyjson::py_float_repr(*f)]),
        Coerced::Uuid(u) => json!(["uuid", u.to_string()]),
        Coerced::Date(d) => json!(["date", d.format("%Y-%m-%d").to_string()]),
        Coerced::Ts(t) => json!([
            "datetime",
            granary_compat::pyjson::PyDateTime::from_chrono_utc(t).isoformat()
        ]),
        Coerced::List(l) => json!(["list", l]),
        Coerced::Str(s) => json!(["str", s]),
    }
}

#[test]
fn coerce_golden() {
    let mut bad = Vec::new();
    for c in crm_s15_common::load("crm_search/coerce.json")
        .as_array()
        .unwrap()
    {
        let got = _coerce(
            c["kind"].as_str().unwrap(),
            c["op"].as_str().unwrap(),
            &c["value"],
        );
        let ok = match (&got, c.get("ok"), c.get("err")) {
            (Ok(v), Some(exp), _) => typed(v) == *exp,
            (Err(SearchFail::Search(e)), _, Some(exp)) => e.0 == exp.as_str().unwrap(),
            (Err(SearchFail::Api(_)), None, None) => c.get("exc").is_some(),
            _ => false,
        };
        if !ok {
            bad.push(format!("{c} -> {got:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad[..bad.len().min(30)].join("\n")
    );
}

// ─── test_crm_ai_unit.py ─────────────────────────────────────────────────────

fn search_msg(r: Result<Coerced, SearchFail>) -> String {
    match r {
        Err(SearchFail::Search(e)) => e.0,
        other => panic!("expected SearchError, got {other:?}"),
    }
}

#[test]
fn test_search_coerce_rejects_bad_number() {
    assert!(search_msg(_coerce("number", "gt", &json!("many"))).contains("not a number"));
}

#[test]
fn test_search_coerce_rejects_bad_date() {
    assert!(search_msg(_coerce("date", "lt", &json!("someday"))).contains("not a date"));
}

#[test]
fn test_search_coerce_days_ago_needs_int() {
    assert_eq!(
        _coerce("datetime", "before_days_ago", &json!("60")).unwrap(),
        Coerced::Int(60)
    );
    assert!(
        search_msg(_coerce("datetime", "before_days_ago", &json!("sixty")))
            .contains("number of days")
    );
}

#[test]
fn test_search_coerce_empty_ops_ignore_value() {
    assert_eq!(
        _coerce("text", "is_empty", &json!("whatever")).unwrap(),
        Coerced::None
    );
}

// ─── database goldens and test_crm_ai_api.py Phase 4 ─────────────────────────

fn outcome<T: serde::Serialize>(r: Result<T, SearchFail>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(SearchFail::Search(e)) => json!({"err": e.0}),
        Err(SearchFail::Api(_)) => json!({"exc": true}),
    }
}

fn same(exp: &Value, got: &Value) -> bool {
    match (exp.get("exc"), got.get("exc")) {
        (Some(_), Some(_)) => true,
        _ => exp == got,
    }
}

#[tokio::test]
async fn search_goldens_and_api_cases() {
    let pool = support::fresh_db("s15_search").await;
    crm_s15_common::scenario_db(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    // field_schema
    let mut schema = crm_s15_common::load("crm_search/schema.json");
    for (ot, exp) in schema.as_object_mut().unwrap() {
        let got = outcome(field_schema(&mut conn, ot).await);
        bless::eq(exp, got, format!("schema {ot}"));
    }
    bless::save("crm_search/schema.json", &schema);

    // count_filters / run_filters, each case in its own transaction (a refused query aborts it)
    let mut bad = Vec::new();
    let mut filter_cases = crm_s15_common::load("crm_search/filters.json");
    for c in filter_cases.as_array_mut().unwrap() {
        let ot = c["object_type"].as_str().unwrap();
        let filters: Vec<Value> = c["filters"].as_array().unwrap().clone();
        let user = crm_s15_common::named_user(c["user"].as_str().unwrap());
        let ot = ot.to_string();
        let ot = ot.as_str();
        if c.get("count").is_some() {
            let mut tx = pool.begin().await.unwrap();
            let got = outcome(count_filters(&mut tx, &user, ot, &filters).await);
            if bless::blessing() {
                c["count"] = got.clone();
            }
            if !same(&c["count"], &got) {
                bad.push(format!("count {c}\n  got {got}"));
            }
        }
        let limit = c.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
        let offset = c.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
        let sort = c.get("sort").filter(|s| !s.is_null());
        let mut tx = pool.begin().await.unwrap();
        let got = outcome(run_filters(&mut tx, &user, ot, &filters, limit, offset, sort).await);
        if bless::blessing() {
            c["rows"] = got.clone();
        }
        if !same(&c["rows"], &got) {
            bad.push(format!("rows {c}\n  got {got}"));
        }
    }
    bless::save("crm_search/filters.json", &filter_cases);
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad[..bad.len().min(20)].join("\n")
    );

    // test_search_schema_lists_builtin_and_custom_fields
    let fields = field_schema(&mut conn, "organization").await.unwrap();
    assert_eq!(fields["industry"]["custom"], json!(false));
    assert_eq!(fields["tier"]["custom"], json!(true));

    let su = crm_s15_common::named_user("super");
    let org = "00000000-0000-0000-0000-000000000101";
    let ids = |rows: &Vec<Value>| {
        rows.iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    // test_filter_search_on_builtin_field (the scenario's custom "city" attribute shadows the built-in city,
    // so this uses country)
    let r = run_filters(
        &mut conn,
        &su,
        "organization",
        &[json!({"field": "country", "op": "eq", "value": "Testland"})],
        50,
        0,
        None,
    )
    .await
    .unwrap();
    assert!(ids(&r).contains(&org.to_string()));
    // test_filter_search_on_custom_attribute
    let r = run_filters(
        &mut conn,
        &su,
        "organization",
        &[json!({"field": "tier", "op": "eq", "value": "Tier 1"})],
        50,
        0,
        None,
    )
    .await
    .unwrap();
    assert!(ids(&r).contains(&org.to_string()));
    // test_text_equality_is_case_insensitive
    let r = run_filters(
        &mut conn,
        &su,
        "organization",
        &[json!({"field": "industry", "op": "eq", "value": "manufacturing"})],
        50,
        0,
        None,
    )
    .await
    .unwrap();
    assert!(ids(&r).contains(&org.to_string()));
    let err = |r: Result<Vec<Value>, SearchFail>| match r {
        Err(SearchFail::Search(e)) => e.0,
        other => panic!("{other:?}"),
    };
    // test_filter_search_rejects_unknown_field
    let e = err(run_filters(
        &mut conn,
        &su,
        "organization",
        &[json!({"field": "'; DROP TABLE organizations;--", "op": "eq", "value": "x"})],
        50,
        0,
        None,
    )
    .await);
    assert!(e.contains("Unknown field"));
    // test_filter_search_rejects_operator_wrong_for_kind
    let e = err(run_filters(
        &mut conn,
        &su,
        "organization",
        &[json!({"field": "employee_count", "op": "contains", "value": "5"})],
        50,
        0,
        None,
    )
    .await);
    assert!(e.contains("not valid"));
    // test_filter_search_caps_filter_count
    let many: Vec<Value> = (0..20)
        .map(|i| json!({"field": "city", "op": "eq", "value": i.to_string()}))
        .collect();
    assert!(
        err(run_filters(&mut conn, &su, "organization", &many, 50, 0, None).await)
            .contains("Too many filters")
    );
}
