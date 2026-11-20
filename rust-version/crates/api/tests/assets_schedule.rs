//! routers/assets.py `_calculate_schedule`: the backend/tests/test_audit_medium_w1.py schedule tests
//! (test_schedule_ends_exactly_at_salvage x6, test_double_declining_switches_to_straight_line) and the
//! golden cases recorded from the real Python function (compat-tests/golden/gen_assets.py).

use std::str::FromStr;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde_json::Value;

use granary_api::routers::assets::{calculate_schedule, ScheduleAsset, ScheduleCategory};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn asset(method: &str, life: i32, price: &str, salvage: &str) -> ScheduleAsset {
    ScheduleAsset {
        depreciation_method: Some(method.into()),
        useful_life_months: Some(life),
        purchase_price: d(price),
        salvage_value: Some(d(salvage)),
        purchase_date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
    }
}

#[test]
fn test_schedule_ends_exactly_at_salvage() {
    for method in ["declining_balance", "straight_line"] {
        for (places, price, salvage) in [
            (0, "100000000", "10000000"),
            (2, "12345.67", "1000.01"),
            (0, "7000001", "0"),
        ] {
            let sched = calculate_schedule(&asset(method, 48, price, salvage), None, places);
            assert_eq!(sched.len(), 48, "{method} {price}");
            assert_eq!(sched.last().unwrap().book_value, d(salvage));
            let total: Decimal = sched.iter().map(|l| l.depreciation_amount).sum();
            assert_eq!(total, d(price) - d(salvage));
            assert!(sched.iter().all(|l| l.depreciation_amount > Decimal::ZERO));
            assert!(sched
                .iter()
                .all(|l| l.depreciation_amount == l.depreciation_amount.round_dp(places)));
        }
    }
}

#[test]
fn test_double_declining_switches_to_straight_line() {
    let sched = calculate_schedule(&asset("declining_balance", 48, "48000000", "0"), None, 0);
    let amounts: Vec<Decimal> = sched.iter().map(|l| l.depreciation_amount).collect();
    assert_eq!(amounts[0], d("2000000")); // 2/48 of cost
    assert!(amounts[0] > amounts[20]); // declining at first ...
    let tail = &amounts[amounts.len() - 10..];
    let (mx, mn) = (tail.iter().max().unwrap(), tail.iter().min().unwrap());
    assert!(*mx - *mn <= Decimal::ONE); // ... then level (straight-line)
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

#[test]
fn golden_schedules_match_python() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../compat-tests/golden/assets/schedule.json"
    );
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(cases.len() > 150);
    for c in &cases {
        let a = &c["asset"];
        let sa = ScheduleAsset {
            depreciation_method: opt_str(&a["method"]),
            useful_life_months: a["life"].as_i64().map(|n| n as i32),
            purchase_price: d(a["price"].as_str().unwrap()),
            salvage_value: a["salvage"].as_str().map(d),
            purchase_date: NaiveDate::parse_from_str(a["date"].as_str().unwrap(), "%Y-%m-%d")
                .unwrap(),
        };
        let sc = c["category"].as_object().map(|cat| ScheduleCategory {
            depreciation_method: opt_str(&cat["method"]),
            useful_life_months: cat["life"].as_i64().map(|n| n as i32),
        });
        let places = c["places"].as_u64().unwrap() as u32;
        let got = calculate_schedule(&sa, sc.as_ref(), places);
        let want = c["lines"].as_array().unwrap();
        assert_eq!(got.len(), want.len(), "case {c}");
        for (g, w) in got.iter().zip(want) {
            assert_eq!(g.period_number, w["period_number"].as_i64().unwrap());
            assert_eq!(
                g.period_date.to_string(),
                w["period_date"].as_str().unwrap()
            );
            assert_eq!(g.period_label, w["period_label"].as_str().unwrap());
            // str(Decimal) text, scale included
            assert_eq!(
                g.depreciation_amount.to_string(),
                w["depreciation_amount"].as_str().unwrap(),
                "case {c}"
            );
            assert_eq!(
                g.accumulated_depreciation.to_string(),
                w["accumulated_depreciation"].as_str().unwrap()
            );
            assert_eq!(g.book_value.to_string(), w["book_value"].as_str().unwrap());
        }
    }
}
