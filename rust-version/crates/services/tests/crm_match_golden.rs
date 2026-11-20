//! crm_match against compat-tests/golden/crm_match/cases.json (recorded from the REAL backend module by
//! compat-tests/golden/gen_crm_match.py), plus the port of tests/test_phone.py::test_phone_digit_forms.

mod support;

use std::collections::{BTreeSet, HashMap};

use granary_services::crm_match::{
    phone_digit_forms, resolve_contact_for_phone, resolve_many, Candidate,
};
use serde_json::Value;
use uuid::Uuid;

/// The test company is in Japan: a national number (0...) is read with country code 81. Every test of this
/// binary sets it before the first phone lookup (the code is read once per process).
fn japan() {
    std::env::set_var("PHONE_DEFAULT_COUNTRY_CODE", "81");
}

fn golden() -> Value {
    japan();
    serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compat-tests/golden/crm_match/cases.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn test_phone_digit_forms() {
    japan();
    let forms: BTreeSet<String> = phone_digit_forms("819012345678").into_iter().collect();
    let want: BTreeSet<String> = ["819012345678", "09012345678", "9012345678"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(forms, want);
}

#[test]
fn phone_digit_forms_golden() {
    let g = golden();
    for case in g["forms"].as_array().unwrap() {
        let mut got = phone_digit_forms(case["in"].as_str().unwrap());
        got.sort();
        let want: Vec<String> = case["out"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(got, want, "{case}");
    }
}

#[tokio::test]
async fn resolve_golden() {
    let g = golden();
    let pool = support::fresh_db("s7_crm_match").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut sqlx::PgConnection = &mut conn;
    sqlx::query("DELETE FROM wa_contact_links")
        .execute(&mut *db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM contacts")
        .execute(&mut *db)
        .await
        .unwrap();
    let mut ids: HashMap<String, Uuid> = HashMap::new();
    for c in g["contacts"].as_array().unwrap() {
        let id = Uuid::new_v4();
        let label = c[0].as_str().unwrap().to_string();
        sqlx::query("INSERT INTO contacts (id, first_name, whatsapp, phone, is_deleted) VALUES ($1, $2, $3, $4, $5)")
            .bind(id)
            .bind(&label)
            .bind(c[1].as_str())
            .bind(c[2].as_str())
            .bind(c[3].as_bool().unwrap())
            .execute(&mut *db)
            .await
            .unwrap();
        ids.insert(label, id);
    }
    for l in g["links"].as_array().unwrap() {
        sqlx::query("INSERT INTO wa_contact_links (id, phone, contact_id) VALUES ($1, $2, $3)")
            .bind(Uuid::new_v4())
            .bind(l[0].as_str().unwrap())
            .bind(ids[l[1].as_str().unwrap()])
            .execute(&mut *db)
            .await
            .unwrap();
    }

    for case in g["single"].as_array().unwrap() {
        let got = resolve_contact_for_phone(db, case["in"].as_str())
            .await
            .map(|c| c.first_name);
        assert_eq!(got.as_deref(), case["out"].as_str(), "{case}");
    }

    let mut wanted: Vec<(String, Vec<Candidate>)> = Vec::new();
    for (key, cands) in g["many_in"].as_object().unwrap() {
        let list = cands
            .as_array()
            .unwrap()
            .iter()
            .map(|c| match c.get("id") {
                Some(Value::String(label)) => Candidate::Id(ids[label.as_str()]),
                Some(_) => Candidate::Id(Uuid::new_v4()),
                None => Candidate::Number(c["n"].as_str().unwrap().to_string()),
            })
            .collect();
        wanted.push((key.clone(), list));
    }
    let got = resolve_many(db, &wanted).await;
    for (key, want) in g["many_out"].as_object().unwrap() {
        assert_eq!(
            got.get(key).map(|c| c.first_name.as_str()),
            want.as_str(),
            "{key}"
        );
    }
    assert_eq!(
        got.len(),
        g["many_out"]
            .as_object()
            .unwrap()
            .values()
            .filter(|v| !v.is_null())
            .count()
    );
}

/// Inside a transaction a failed lookup does not poison the caller: the session is rolled back and a new
/// transaction begun, as Python's `_safe_rollback`.
#[tokio::test]
async fn a_failed_lookup_answers_none_and_leaves_a_usable_transaction() {
    japan();
    let pool = support::fresh_db("s7_crm_match_tx").await;
    let mut tx = pool.begin().await.unwrap();
    // Break the transaction on purpose, then ask: the answer is None and the transaction works again.
    let _ = sqlx::query("SELECT 1/0").execute(&mut *tx).await;
    assert!(resolve_contact_for_phone(&mut tx, Some("819012345678"))
        .await
        .is_none());
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(one, 1);
    assert!(resolve_many(
        &mut tx,
        &[("k".into(), vec![Candidate::Number("819012345678".into())])]
    )
    .await
    .is_empty());
    tx.commit().await.unwrap();
}
