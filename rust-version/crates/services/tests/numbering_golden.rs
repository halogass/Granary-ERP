//! services/numbering.rs: golden cases first recorded from the retired Python `next_document_number`
//! (re-record with GOLDEN_BLESS=1 after an intended change), the ported test_audit_low test, and concurrency.

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde_json::Value;
use sqlx::Row;

use granary_services::numbering::{
    format_number, next_document_number, next_document_number_at, zfill,
};

fn cases() -> Vec<Value> {
    serde_json::from_value(bless::read("numbering/cases.json")).unwrap()
}

#[test]
fn zfill_and_format() {
    assert_eq!(zfill("7", 5), "00007");
    assert_eq!(zfill("-4", 5), "-0004");
    assert_eq!(zfill("123456", 3), "123456");
    assert_eq!(zfill("1", 0), "1");
    assert_eq!(zfill("1", -3), "1");
    assert_eq!(format_number("QUO/X", 2025, 5, 1, 5), "QUO/X-2025-05-00001");
}

#[tokio::test]
async fn golden_cases() {
    let pool = support::fresh_db("numbering_golden").await;
    let mut cases = cases();
    assert!(cases.len() >= 16);
    for case in cases.iter_mut() {
        let name = case["name"].as_str().unwrap().to_string();
        let name = name.as_str();
        let tz: Tz = case["tz"].as_str().unwrap().parse().unwrap();
        let now: DateTime<Utc> = case["now"].as_str().unwrap().parse().unwrap();
        let now = now.with_timezone(&tz);
        let ty = case["type"].as_str().unwrap();
        let mut tx = pool.begin().await.unwrap();
        // The generator's scratch database holds only the case rows; drop the seeded rows of the type.
        sqlx::query("DELETE FROM document_sequences WHERE sequence_type = $1")
            .bind(ty)
            .execute(&mut *tx)
            .await
            .unwrap();
        for r in case["rows"].as_array().unwrap() {
            sqlx::query(
                "INSERT INTO document_sequences (id, sequence_type, prefix, suffix_separator, year, month, \
                 current_sequence, sequence_padding, last_generated) VALUES ($1,$2,$3,'-',$4,$5,$6,$7,$8)",
            )
            .bind(uuid::Uuid::new_v4())
            .bind(r["sequence_type"].as_str())
            .bind(r["prefix"].as_str())
            .bind(r["year"].as_i64().map(|v| v as i32))
            .bind(r["month"].as_i64().map(|v| v as i32))
            .bind(r["current_sequence"].as_i64().map(|v| v as i32))
            .bind(r["sequence_padding"].as_i64().map(|v| v as i32))
            .bind(r["last_generated"].as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        let mut numbers = vec![];
        let mut error = None;
        for _ in 0..case["calls"].as_i64().unwrap() {
            match next_document_number_at(&mut tx, ty, now).await {
                Ok(n) => numbers.push(Value::String(n)),
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        bless::eq(&mut case["numbers"], Value::Array(numbers), name);
        if !bless::blessing() {
            assert_eq!(
                error.is_some(),
                !case["error"].is_null(),
                "{name}: {error:?}"
            );
        }
        if error.is_none() {
            let types: Vec<String> = case["rows_after"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["sequence_type"].as_str().unwrap().to_string())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            let rows = sqlx::query(
                "SELECT sequence_type, prefix, suffix_separator, year, month, current_sequence, sequence_padding, \
                 last_generated FROM document_sequences WHERE sequence_type = ANY($1) \
                 ORDER BY sequence_type, year, month",
            )
            .bind(&types)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
            let got: Vec<Value> = rows
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "sequence_type": r.get::<String, _>(0),
                        "prefix": r.get::<String, _>(1),
                        "suffix_separator": r.get::<Option<String>, _>(2),
                        "year": r.get::<i32, _>(3),
                        "month": r.get::<i32, _>(4),
                        "current_sequence": r.get::<i32, _>(5),
                        "sequence_padding": r.get::<Option<i32>, _>(6),
                        "last_generated": r.get::<Option<String>, _>(7),
                    })
                })
                .collect();
            bless::eq(&mut case["rows_after"], Value::Array(got), name);
        }
        tx.rollback().await.unwrap();
    }
    bless::save("numbering/cases.json", &Value::Array(cases));
}

/// test_audit_low.py::test_document_numbers_follow_the_local_month (Python replaces clock.now_in).
#[tokio::test]
async fn test_document_numbers_follow_the_local_month() {
    let pool = support::fresh_db("numbering_clock").await;
    let mut tx = pool.begin().await.unwrap();
    // 18:00 UTC on 31 Oct is 03:00 on 1 Nov in Tokyo (the company's `timezone` setting).
    sqlx::query("UPDATE system_settings SET value = 'Asia/Tokyo' WHERE key = 'timezone'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let tz = granary_core::clock::local_tz(&mut tx).await.unwrap();
    let now: DateTime<Utc> = "2031-10-31T18:00:00Z".parse().unwrap();
    let number = next_document_number_at(&mut tx, "pytest_clock", now.with_timezone(&tz))
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(number.contains("-2031-11-"), "{number}");
}

/// Concurrent callers (each in its own transaction, also racing to create the period row) never get the
/// same number, and the numbers are contiguous.
#[tokio::test]
async fn concurrent_calls_never_share_a_number() {
    let pool = support::fresh_db("numbering_conc").await;
    let mut handles = vec![];
    for _ in 0..24 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let mut tx = pool.begin().await.unwrap();
            let n = next_document_number(&mut tx, "pytest_concurrent")
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(5)).await; // hold the row lock a while
            tx.commit().await.unwrap();
            n
        }));
    }
    let mut numbers = vec![];
    for h in handles {
        numbers.push(h.await.unwrap());
    }
    let unique: HashSet<_> = numbers.iter().cloned().collect();
    assert_eq!(unique.len(), numbers.len(), "{numbers:?}");
    let mut seqs: Vec<i64> = numbers
        .iter()
        .map(|n| n.rsplit('-').next().unwrap().parse().unwrap())
        .collect();
    seqs.sort();
    assert_eq!(seqs, (1..=24).collect::<Vec<_>>());
    assert!(numbers.iter().all(|n| n.starts_with("PYTEST_CONCURRENT-")));
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_sequences WHERE sequence_type = 'pytest_concurrent'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1);
}
