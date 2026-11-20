//! Golden cases for nary::{margins, costing, rates, proposals}, first recorded from the retired Python (inputs
//! now neutral; re-record outputs with GOLDEN_BLESS=1 after an intended change and review the diff).

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::str::FromStr;

use granary_core::errors::ApiError;
use granary_db::rows::documents::NaryRateCard;
use granary_services::nary::costing::{self, RateTable};
use granary_services::nary::margins::{self, s, Band};
use granary_services::nary::{proposals, rates};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use uuid::Uuid;

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/nary_costing/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).unwrap()
}

fn d(v: &Value) -> Decimal {
    Decimal::from_str(v.as_str().unwrap()).unwrap()
}

/// The error a case recorded, as the Rust error should show it.
fn check_err(e: &ApiError, case: &Value, ctx: &str) {
    if let Some(detail) = case.get("setting_error") {
        assert_eq!(e.status().as_u16(), 400, "{ctx}");
        let body = format!("{e:?}");
        assert!(
            body.contains(detail.as_str().unwrap()),
            "{ctx}: {body} vs {detail}"
        );
    } else if case.get("error").is_some() {
        assert_eq!(e.status().as_u16(), 500, "{ctx}: {e:?}");
    } else {
        panic!("{ctx}: unexpected error {e:?}");
    }
}

#[test]
fn golden_price_at() {
    for c in load("price_at") {
        let got = margins::price_at(d(&c["cost"]), d(&c["pct"]));
        match c.get("out") {
            Some(out) => assert_eq!(s(got.unwrap()), out.as_str().unwrap(), "{c}"),
            None => check_err(&got.unwrap_err(), &c, &c.to_string()),
        }
    }
}

#[test]
fn golden_margin_pct() {
    for c in load("margin_pct") {
        let got = margins::margin_pct(d(&c["price"]), d(&c["cost"])).unwrap();
        assert_eq!(got.map(s), c["out"].as_str().map(str::to_string), "{c}");
    }
}

#[test]
fn golden_max_discount() {
    for c in load("max_discount") {
        let got = margins::max_discount(
            d(&c["cost"]),
            d(&c["floor"]),
            d(&c["rate"]),
            d(&c["list_price"]),
        )
        .unwrap();
        assert_eq!(s(got), c["out"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn golden_man_day() {
    for c in load("man_day") {
        let got = rates::man_day_from(
            d(&c["monthly"]),
            d(&c["load"]),
            c["days"].as_i64().unwrap(),
            d(&c["floor"]),
        )
        .unwrap();
        assert_eq!(s(got), c["out"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn golden_bands() {
    for c in load("bands") {
        let raw = &c["raw"];
        let table = margins::table_from(raw);
        match c.get("table") {
            Some(t) => {
                let got: Vec<Value> = table
                    .as_ref()
                    .unwrap()
                    .0
                    .iter()
                    .map(|b| json!([b.key, b.label, s(b.target), s(b.floor)]))
                    .collect();
                assert_eq!(&Value::Array(got), t, "{raw}");
            }
            None => check_err(
                table.as_ref().unwrap_err(),
                &c["table_error"],
                &raw.to_string(),
            ),
        }
        for (fam, want) in c["category_for"].as_object().unwrap() {
            let family = if fam == "None" {
                None
            } else {
                Some(fam.as_str())
            };
            let got = match &table {
                Ok(t) => margins::category_from(raw, t, family),
                Err(_) => margins::table_from(raw).map(|_| None),
            };
            match want {
                Value::Null => assert!(got.unwrap().is_none(), "{raw} {fam}"),
                Value::String(k) => assert_eq!(&got.unwrap().unwrap().key, k, "{raw} {fam}"),
                other => check_err(&got.unwrap_err(), other, &format!("{raw} {fam}")),
            }
        }
    }
}

const ROLES: [(&str, &str, &str); 4] = [
    ("Software Engineer", "mid", "650000.00"),
    ("Field Technician", "junior", "300000.00"),
    ("Field Technician", "mid", "412345.67"),
    ("Project Manager", "senior", "1250000.00"),
];

fn rate_table() -> RateTable {
    RateTable(
        ROLES
            .iter()
            .map(|(r, sn, c)| NaryRateCard {
                role: r.to_string(),
                seniority: sn.to_string(),
                cost_base: Decimal::from_str(c).unwrap(),
                source: None,
                valid_from: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                status: "active".into(),
                proposed_by_nary: false,
                approved_by_id: None,
                id: Uuid::new_v4(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
            .collect(),
    )
}

#[test]
fn golden_labour() {
    let table = rate_table();
    let mut cases = load("labour");
    for c in cases.iter_mut() {
        let ws: Vec<Value> = c["workstreams"].as_array().unwrap().clone();
        let msg = c["workstreams"].to_string();
        bless::eq(&mut c["out"], costing::labour(&ws, &table).unwrap(), msg);
    }
    bless::save("nary_costing/labour.json", &Value::Array(cases));
}

#[test]
fn golden_price_lines() {
    for c in load("price_lines") {
        let band = c["band"].as_array().map(|b| Band {
            key: b[0].as_str().unwrap().into(),
            label: b[1].clone(),
            target: d(&b[2]),
            floor: d(&b[3]),
        });
        assert_eq!(
            costing::price_lines(d(&c["cost"]), band.as_ref()).unwrap(),
            c["out"],
            "{c}"
        );
    }
}

#[test]
fn golden_floor_flags() {
    for c in load("floor_flags") {
        let prices: Vec<(String, String)> = c["prices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into()))
            .collect();
        let got = costing::floor_flags(&c["sheet"], &prices).unwrap();
        assert_eq!(json!(got), c["out"], "{c}");
    }
}

#[test]
fn golden_strip_accuracy() {
    let mut cases = load("strip_accuracy");
    for c in cases.iter_mut() {
        let (text, removed) = proposals::strip_accuracy(c["in"].as_str().unwrap());
        let msg = c["in"].to_string();
        bless::eq(&mut c["text"], text, &msg);
        bless::eq(&mut c["removed"], json!(removed), &msg);
    }
    bless::save("nary_costing/strip_accuracy.json", &Value::Array(cases));
}

#[test]
fn golden_coverage() {
    for c in load("coverage") {
        let fields = c["fields"].as_array().unwrap().clone();
        assert_eq!(
            proposals::coverage(&c["content"], &fields),
            c["out"].as_i64().unwrap(),
            "{c}"
        );
    }
}

/// compute() against a database holding the generator's settings, rate card and catalog.
#[tokio::test]
async fn golden_compute() {
    let pool = support::fresh_db("nary_costing_golden").await;
    let mut db = pool.acquire().await.unwrap();
    sqlx::query("DELETE FROM nary_rate_card")
        .execute(&mut *db)
        .await
        .unwrap();
    for (r, sn, c) in ROLES {
        sqlx::query(
            "INSERT INTO nary_rate_card (id, role, seniority, cost_base, valid_from, status, proposed_by_nary) \
             VALUES ($1, $2, $3, $4, '2026-10-01', 'active', false)",
        )
        .bind(Uuid::new_v4())
        .bind(r)
        .bind(sn)
        .bind(Decimal::from_str(c).unwrap())
        .execute(&mut *db)
        .await
        .unwrap();
    }
    for (sku, cost, family) in [
        ("LAP-1", Some("9000000.00"), Some("laptop")),
        ("MON-2", Some("1234567.89"), Some("monitor")),
        ("SRV-3", Some("350000.00"), Some("services")),
        ("NOC-4", None, None),
    ] {
        sqlx::query(
            "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, family, is_active, sale_model, \
             price_metric, modules, pricing_mode, addon_for, is_whitelabel, is_orderable, is_deleted) VALUES \
             ($1, $2, $2, 'physical', 'Unit', $3, $4, true, 'one_time', 'unit', '{}', 'list', '{}', false, true, false)",
        )
        .bind(Uuid::new_v4())
        .bind(sku)
        .bind(cost.map(|c| Decimal::from_str(c).unwrap()))
        .bind(family)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    let margins_raw = r#"{"hardware": {"label": "Hardware", "target": 25, "floor": 15, "families": ["laptop", "monitor"]}, "project_services": {"label": "Project services", "target": "45", "floor": "35", "families": ["services"]}, "custom_ai": {"target": 55.5, "floor": 40, "families": "custom_ai,analytics_kit"}}"#;
    set(&mut db, "sales.margins", margins_raw).await;
    let mut n = 0;
    let mut cases = load("compute");
    for c in cases.iter_mut() {
        set(
            &mut db,
            "proposal.cost.contingency_pct",
            c["contingency"].as_str().unwrap(),
        )
        .await;
        let got = costing::compute(&mut db, &c["estimate"]).await;
        if bless::blessing() {
            match &got {
                Ok(v) => c["out"] = v.clone(),
                Err(_) => {
                    c.as_object_mut().unwrap().shift_remove("out");
                }
            }
        }
        match c.get("out") {
            Some(out) => assert_eq!(&got.unwrap(), out, "{}", c["estimate"]),
            None => assert!(got.is_err(), "{}", c["estimate"]),
        }
        n += 1;
    }
    bless::save("nary_costing/compute.json", &Value::Array(cases));
    assert_eq!(n, 150);
}

async fn set(db: &mut sqlx::PgConnection, key: &str, value: &str) {
    let n = sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(key)
        .bind(value)
        .execute(&mut *db)
        .await
        .unwrap()
        .rows_affected();
    if n == 0 {
        sqlx::query("INSERT INTO system_settings (id, key, value, category, label) VALUES ($1, $2, $3, 'test', $2)")
            .bind(Uuid::new_v4())
            .bind(key)
            .bind(value)
            .execute(&mut *db)
            .await
            .unwrap();
    }
}

/// The fixed instructions and schemas of every model call, byte for byte (json.dumps of the schema, so the
/// key order of the request body is Python's too).
#[test]
fn golden_prompts_and_schemas() {
    use granary_services::nary::{negotiation, proposal_edits, quotes};
    let mut p = bless::read("nary_costing/prompts.json");
    let dumps = |v: &Value| {
        granary_compat::pyjson::dumps(
            &granary_compat::pyjson::PyValue::from(v),
            &Default::default(),
        )
        .unwrap()
    };
    let ex = &mut p["extras"];
    bless::eq(&mut ex["quote_lines"], quotes::LINES_RULES, "quote_lines");
    bless::eq(
        &mut ex["cost_estimate"],
        costing::ESTIMATE_RULES,
        "cost_estimate",
    );
    bless::eq(
        &mut ex["negotiation"],
        negotiation::HINT_RULES,
        "negotiation",
    );
    bless::eq(
        &mut ex["rate_research"],
        rates::RESEARCH_RULES,
        "rate_research",
    );
    bless::eq(
        &mut ex["fit_check_sheet"],
        proposals::UPDATE_RULES,
        "fit_check_sheet",
    );
    bless::eq(
        &mut ex["proposal_draft_en"],
        proposals::draft_rules("en"),
        "draft en",
    );
    bless::eq(
        &mut ex["proposal_draft_id"],
        proposals::draft_rules("id"),
        "draft id",
    );
    bless::eq(
        &mut ex["proposal_edit"],
        proposal_edits::EDIT_RULES,
        "proposal_edit",
    );
    let sc = &mut p["schemas"];
    bless::eq(
        &mut sc["quote_lines"],
        dumps(&quotes::LINES_SCHEMA),
        "s quote_lines",
    );
    bless::eq(
        &mut sc["cost_estimate"],
        dumps(&costing::SCHEMA),
        "s cost_estimate",
    );
    bless::eq(
        &mut sc["negotiation"],
        dumps(&negotiation::SCHEMA),
        "s negotiation",
    );
    bless::eq(
        &mut sc["rate_research"],
        dumps(&rates::SCHEMA),
        "s rate_research",
    );
    bless::eq(
        &mut sc["fit_check_sheet"],
        dumps(&proposals::UPDATE_SCHEMA),
        "s fit_check_sheet",
    );
    bless::eq(
        &mut sc["proposal_draft"],
        dumps(&proposals::DRAFT_SCHEMA),
        "s proposal_draft",
    );
    bless::eq(
        &mut sc["proposal_edit"],
        dumps(&proposal_edits::SCHEMA),
        "s proposal_edit",
    );
    bless::eq(&mut p["nary_note"], quotes::NARY_NOTE, "nary_note");
    bless::save("nary_costing/prompts.json", &p);
}
