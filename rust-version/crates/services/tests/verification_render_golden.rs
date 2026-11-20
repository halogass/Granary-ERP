//! Golden tests for verification/render.rs and the QR of verification/service.rs against fixtures first
//! recorded from the retired Python code (neutral data since; re-record with GOLDEN_BLESS=1 after an intended
//! change and review the diff).

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use granary_db::rows::approval::ApprovalRequest;
use granary_services::verification::{render, service};
use rust_decimal::Decimal;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../../compat-tests/golden/verification_render/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json")
}

fn dec(v: &Value) -> Option<Decimal> {
    v.as_str().map(|s| {
        Decimal::from_str(s)
            .or_else(|_| Decimal::from_scientific(s))
            .expect("decimal")
    })
}

fn opt_str(v: &Value) -> Option<&str> {
    v.as_str()
}

fn opt_i64(v: &Value) -> Option<i64> {
    v.as_i64()
}

/// Equal values with the same key order in every map (the sidecar receives Python's dict order).
fn assert_same(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            let ak: Vec<&String> = a.keys().collect();
            let ek: Vec<&String> = e.keys().collect();
            assert_eq!(ak, ek, "keys at {path}");
            for (k, v) in e {
                assert_same(&a[k], v, &format!("{path}.{k}"));
            }
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "length at {path}: {actual} vs {expected}");
            for (i, (x, y)) in a.iter().zip(e).enumerate() {
                assert_same(x, y, &format!("{path}[{i}]"));
            }
        }
        (Value::Number(a), Value::Number(e)) => {
            assert_eq!(a.as_f64(), e.as_f64(), "number at {path}")
        }
        _ => assert_eq!(actual, expected, "value at {path}"),
    }
}

/// When blessing, stores `cases` as `key` of pure.json.
fn save_pure(key: &str, cases: Vec<Value>) {
    bless::update("verification_render/pure.json", |v| {
        v[key] = Value::Array(cases)
    });
}

#[test]
fn sanitize_html_matches_python() {
    let f = fixture("pure.json");
    let mut blessed = f["sanitize"].as_array().unwrap().clone();
    let cases = f["sanitize"].as_array().unwrap();
    let mut bad = vec![];
    for (i, c) in cases.iter().enumerate() {
        let got = render::sanitize_html(opt_str(&c[0]));
        if bless::blessing() {
            blessed[i][1] = Value::String(got);
            continue;
        }
        if got != c[1].as_str().unwrap() {
            bad.push(format!("{:?}\n  got  {:?}\n  want {:?}", c[0], got, c[1]));
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {} differ:\n{}",
        bad.len(),
        cases.len(),
        bad[..bad.len().min(10)].join("\n")
    );
    assert!(cases.len() > 1500);
    save_pure("sanitize", blessed);
}

#[test]
fn unescape_matches_python() {
    let f = fixture("pure.json");
    for c in f["unescape"].as_array().unwrap() {
        assert_eq!(
            render::unescape(c[0].as_str().unwrap()),
            c[1].as_str().unwrap(),
            "{:?}",
            c[0]
        );
    }
}

#[test]
fn numbers_match_python() {
    let f = fixture("pure.json");
    let mut money = f["fmt_money"].as_array().unwrap().clone();
    for c in money.iter_mut() {
        let got = render::fmt_money(dec(&c[0]), opt_str(&c[1]), c[2].as_u64().unwrap() as u32);
        let msg = c.to_string();
        bless::eq(&mut c[3], got, msg);
    }
    save_pure("fmt_money", money);
    for c in f["fmt_qty"].as_array().unwrap() {
        assert_eq!(render::fmt_qty(dec(&c[0])), c[1].as_str().unwrap(), "{c}");
    }
    for c in f["pct"].as_array().unwrap() {
        assert_eq!(render::pct(dec(&c[0])), c[1].as_str().unwrap(), "{c}");
    }
}

#[test]
fn dates_match_python() {
    let f = fixture("pure.json");
    for c in f["dates"].as_array().unwrap() {
        let arg: render::DateArg = match c[0].as_str().unwrap() {
            "str" => {
                let s = c[1].as_str().unwrap();
                let want = c[2]
                    .as_str()
                    .map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap());
                assert_eq!(
                    render::py_datetime_fromisoformat_date(s),
                    want,
                    "fromisoformat {s:?}"
                );
                render::DateArg::Str(s.into())
            }
            "date" => render::DateArg::Date(
                NaiveDate::parse_from_str(c[1].as_str().unwrap(), "%Y-%m-%d").unwrap(),
            ),
            "datetime" => DateTime::parse_from_rfc3339(c[1].as_str().unwrap())
                .unwrap()
                .with_timezone(&Utc)
                .into(),
            "none" => render::DateArg::None,
            _ => render::DateArg::Other,
        };
        assert_eq!(render::fmt_date(arg.clone()), c[3].as_str().unwrap(), "{c}");
        assert_eq!(render::fmt_date_short(arg), c[4].as_str().unwrap(), "{c}");
    }
}

#[test]
fn recurring_terms_match_python() {
    let f = fixture("pure.json");
    let mut cases = f["recurring"].as_array().unwrap().clone();
    for c in cases.iter_mut() {
        let got = render::recurring_terms_text(
            opt_str(&c[0]),
            opt_i64(&c[1]),
            opt_i64(&c[2]),
            opt_i64(&c[3]),
        );
        let msg = c.to_string();
        bless::eq(&mut c[4], got, msg);
    }
    save_pure("recurring", cases);
}

#[test]
fn tax_totals_match_python() {
    let f = fixture("pure.json");
    let mut cases = f["tax_totals"].as_array().unwrap().clone();
    for c in cases.iter_mut() {
        let tl = if c[0].is_null() {
            None
        } else {
            Some(c[0].clone())
        };
        let got = render::tax_totals(
            tl.as_ref(),
            dec(&c[1]),
            opt_str(&c[2]),
            c[3].as_u64().unwrap() as u32,
        );
        if bless::blessing() {
            c[4] = Value::Array(got);
        } else {
            assert_same(&Value::Array(got), &c[4], &c.to_string());
        }
    }
    save_pure("tax_totals", cases);
}

fn decode_png(bytes: &[u8]) -> image::GrayImage {
    image::load_from_memory(bytes).expect("png").to_luma8()
}

#[test]
fn qr_matches_python_qrcode() {
    use base64::Engine as _;
    if bless::blessing() {
        let mut all = fixture("qr.json");
        for c in all.as_array_mut().unwrap() {
            let url = c["url"].as_str().unwrap().to_string();
            let (version, mask, rows) = service::qr_matrix(url.as_bytes()).unwrap();
            let modules: Vec<String> = rows
                .iter()
                .map(|r| r.iter().map(|d| if *d { '1' } else { '0' }).collect())
                .collect();
            let uri = service::qr_data_uri(&url).unwrap();
            let b64 = uri
                .strip_prefix("data:image/png;base64,")
                .unwrap()
                .to_string();
            let img = decode_png(
                &base64::engine::general_purpose::STANDARD
                    .decode(&b64)
                    .unwrap(),
            );
            c["version"] = serde_json::json!(version);
            c["mask"] = serde_json::json!(mask);
            c["modules"] = serde_json::json!(modules);
            c["size"] = serde_json::json!([img.width(), img.height()]);
            c["png_b64"] = serde_json::json!(b64);
        }
        bless::write("verification_render/qr.json", &all);
        return;
    }
    let cases = fixture("qr.json");
    let cases = cases.as_array().unwrap();
    assert!(
        cases.iter().any(|c| c["version"].as_i64().unwrap() >= 7),
        "a case with version bits"
    );
    for c in cases {
        let url = c["url"].as_str().unwrap();
        let (version, mask, rows) = service::qr_matrix(url.as_bytes()).unwrap();
        assert_eq!(
            version as i64,
            c["version"].as_i64().unwrap(),
            "version {url}"
        );
        assert_eq!(mask as i64, c["mask"].as_i64().unwrap(), "mask {url}");
        let modules: Vec<String> = rows
            .iter()
            .map(|r| r.iter().map(|d| if *d { '1' } else { '0' }).collect())
            .collect();
        let want: Vec<&str> = c["modules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap())
            .collect();
        assert_eq!(modules, want, "modules {url}");

        // the image: same size and same pixels as Pillow's
        let uri = service::qr_data_uri(url).unwrap();
        let ours = decode_png(
            &base64::engine::general_purpose::STANDARD
                .decode(uri.strip_prefix("data:image/png;base64,").unwrap())
                .unwrap(),
        );
        let theirs = decode_png(
            &base64::engine::general_purpose::STANDARD
                .decode(c["png_b64"].as_str().unwrap())
                .unwrap(),
        );
        let size: Vec<u32> = c["size"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        assert_eq!(
            [ours.width(), ours.height()],
            [size[0], size[1]],
            "size {url}"
        );
        assert_eq!(ours.as_raw(), theirs.as_raw(), "pixels {url}");

        // and it decodes back to the URL
        let mut prepared = rqrr::PreparedImage::prepare(ours);
        let grids = prepared.detect_grids();
        assert_eq!(grids.len(), 1, "one code in {url}");
        let (meta, content) = grids[0].decode().expect("decodes");
        assert_eq!(content, url);
        assert_eq!(meta.version.0 as i64, c["version"].as_i64().unwrap());
        assert_eq!(meta.ecc_level, 0, "error correction M");
        assert_eq!(meta.mask as i64, c["mask"].as_i64().unwrap());
    }
}

async fn insert_row(
    conn: &mut sqlx::PgConnection,
    table: &str,
    row: &serde_json::Map<String, Value>,
) {
    let cols = row
        .keys()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("INSERT INTO {table} ({cols}) SELECT {cols} FROM json_populate_record(NULL::{table}, $1::json)");
    sqlx::query(&sql)
        .bind(Value::Object(row.clone()))
        .execute(&mut *conn)
        .await
        .unwrap_or_else(|e| panic!("{table}: {e}"));
}

#[tokio::test]
async fn documents_build_like_python() {
    let pool = support::fresh_db("s11_render").await;
    bless::neutral_owner(&pool).await;
    let mut all = fixture("scenarios.json");
    let scenarios = all.as_array_mut().unwrap();
    let n_scenarios = scenarios.len();
    let mut failures = vec![];
    for sc in scenarios.iter_mut() {
        let name = sc["name"].as_str().unwrap().to_string();
        let name = name.as_str();
        let mut tx = pool.begin().await.unwrap();
        for sql in sc["sql"].as_array().unwrap() {
            sqlx::query(sql.as_str().unwrap())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        for r in sc["rows"].as_array().unwrap() {
            insert_row(&mut tx, r[0].as_str().unwrap(), r[1].as_object().unwrap()).await;
        }
        let entity_type = sc["entity_type"].as_str().unwrap().to_string();
        let entity_type = entity_type.as_str();
        let entity = render::load(&mut tx, entity_type, sc["entity_id"].as_str().unwrap())
            .await
            .unwrap()
            .expect("entity");
        let rid = uuid::Uuid::parse_str(sc["request_id"].as_str().unwrap()).unwrap();
        let request: Option<ApprovalRequest> =
            sqlx::query_as("SELECT * FROM approval_requests WHERE id = $1")
                .bind(rid)
                .fetch_optional(&mut *tx)
                .await
                .unwrap();
        let result = std::panic::AssertUnwindSafe(async {
            let (template, data, facts) =
                render::build(&mut tx, entity_type, &entity, request.as_ref())
                    .await
                    .unwrap();
            let got = serde_json::json!({"template": template, "data": data, "facts": facts});
            let extra = serde_json::json!({
                "approvals_block": render::approvals_block(&mut tx, request.as_ref()).await.unwrap(),
                "approval_stamp": render::approval_stamp(&mut tx, request.as_ref()).await.unwrap(),
                "company_name": render::company_name(&mut tx).await.unwrap(),
            });
            if bless::blessing() {
                sc["result"] = got;
                sc["extra"] = extra;
                return;
            }
            assert_same(&got, &sc["result"], name);
            assert_same(&extra, &sc["extra"], &format!("{name} extra"));
        });
        if let Err(e) = futures::FutureExt::catch_unwind(result).await {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
            failures.push(format!("{name}: {}", msg.unwrap_or_default()));
        }
        tx.rollback().await.unwrap();
    }
    assert!(
        failures.is_empty(),
        "{} of {} scenarios differ:\n{}",
        failures.len(),
        n_scenarios,
        failures.join("\n")
    );
    bless::save("verification_render/scenarios.json", &all);
}
