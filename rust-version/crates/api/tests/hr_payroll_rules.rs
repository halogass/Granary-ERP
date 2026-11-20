//! The pure payroll helpers: configured contributions (caps, taxable and deductible shares), progressive
//! income tax with allowances and the expense deduction, the cumulative monthly withholding that settles
//! the year, proration, overtime tiers, and the settings checks.

mod support;

use chrono::NaiveDate;
use granary_api::routers::hr::payroll::{
    contributions, contributions_cfg, deductibles, employee_total, employer_total,
    employment_in_month, income_tax_cfg, income_tax_for_month, overtime_multiple, payroll_settings,
    period_tax, taxable_gross, ContributionDef, IncomeTaxCfg, PayrollCfg, Ytd,
    PAYROLL_SETTINGS_WHERE,
};
use granary_core::money::quantize_money;
use granary_core::validation::PyObj;
use rust_decimal::Decimal;

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn json_obj(s: &str) -> PyObj {
    PyObj::from_json(&serde_json::from_str(s).unwrap())
}

/// Two contributions: health (capped at 10,000, employee share deductible) and pension (employer share
/// taxable).
fn cfg() -> PayrollCfg {
    let mut c = PayrollCfg::with_rates(&[("overtime_hours_divisor", "173")]);
    c.contributions = vec![
        ContributionDef {
            key: "health".into(),
            label: "Health".into(),
            employee_rate: d("1"),
            employer_rate: d("4"),
            base_cap: Some(d("10000")),
            employer_taxable: false,
            employee_deductible: true,
        },
        ContributionDef {
            key: "pension".into(),
            label: "Pension".into(),
            employee_rate: d("2"),
            employer_rate: d("3"),
            base_cap: None,
            employer_taxable: true,
            employee_deductible: false,
        },
    ];
    c.income_tax = IncomeTaxCfg {
        progressive: true,
        allowances: [
            ("single".to_string(), d("12000")),
            ("family".to_string(), d("24000")),
        ]
        .into_iter()
        .collect(),
        default_status: "single".into(),
        expense_deduction_rate: d("5"),
        expense_deduction_monthly_cap: Some(d("100")),
        taxable_rounding: d("1000"),
        brackets: vec![(Some(d("50000")), d("10")), (None, d("20"))],
    };
    c
}

#[test]
fn contributions_follow_the_settings_and_the_cap() {
    let c = contributions(d("20000"), &cfg(), 2);
    assert_eq!(c["health"], (d("100.00"), d("400.00"))); // capped at 10,000
    assert_eq!(c["pension"], (d("400.00"), d("600.00")));
    assert_eq!(employee_total(&c), d("500.00"));
    assert_eq!(employer_total(&c), d("1000.00"));
    // the employer pension counts as income; the employee health share is deductible
    assert_eq!(taxable_gross(d("20000"), &c, &cfg()), d("20600.00"));
    assert_eq!(deductibles(&c, &cfg()), d("100.00"));
    // no contributions configured: nothing
    let mut none = cfg();
    none.contributions.clear();
    assert!(contributions(d("20000"), &none, 2).is_empty());
}

#[test]
fn the_year_tax_is_progressive_after_the_allowance_and_deductions() {
    let c = cfg().income_tax;
    // 120,000 a year: expense 5% capped at 100 x 12 = 1,200; 120,000 - 1,200 - 1,200 deductible - 12,000
    // allowance = 105,600 -> 105,000 (rounded down to 1,000): 50,000 x 10% + 55,000 x 20% = 16,000
    assert_eq!(period_tax(d("120000"), d("1200"), 12, None, &c), d("16000"));
    // a bigger allowance for another status lowers it; an unknown status uses the default
    assert!(period_tax(d("120000"), d("1200"), 12, Some("family"), &c) < d("16000"));
    assert_eq!(
        period_tax(d("120000"), d("1200"), 12, Some("nope"), &c),
        d("16000")
    );
    // method none: no tax
    assert_eq!(
        period_tax(d("120000"), d("0"), 12, None, &IncomeTaxCfg::default()),
        d("0")
    );
}

#[test]
fn twelve_equal_months_withhold_exactly_the_year_tax() {
    let c = cfg().income_tax;
    let mut ytd = Ytd::default();
    for _ in 0..12 {
        let t = income_tax_for_month(d("10000"), d("100"), None, &c, 2, &ytd);
        ytd.taxable_gross += d("10000");
        ytd.deductibles += d("100");
        ytd.withheld += t;
        ytd.months += 1;
    }
    assert_eq!(ytd.withheld, quantize_money(d("16000"), 2));
}

#[test]
fn a_raise_late_in_the_year_is_caught_up_in_the_following_months() {
    let c = cfg().income_tax;
    let mut ytd = Ytd::default();
    for m in 0..12 {
        let pay = if m < 6 { d("5000") } else { d("15000") };
        let t = income_tax_for_month(pay, d("0"), None, &c, 2, &ytd);
        ytd.taxable_gross += pay;
        ytd.withheld += t;
        ytd.months += 1;
    }
    let annual = period_tax(d("120000"), d("0"), 12, None, &c);
    assert_eq!(ytd.withheld, quantize_money(annual, 2));
}

#[test]
fn the_settings_are_parsed_and_checked() {
    let c = contributions_cfg(&json_obj(
        r#"[{"key": "health", "label": "Health", "employee_rate": 1, "employer_rate": "4", "base_cap": 10000,
             "employee_deductible": true}]"#,
    ))
    .unwrap();
    assert_eq!(c[0].employer_rate, d("4"));
    assert_eq!(c[0].base_cap, Some(d("10000")));
    assert!(c[0].employee_deductible && !c[0].employer_taxable);
    assert!(contributions_cfg(&json_obj(r#"[{"key": "Bad Key"}]"#)).is_err());
    assert!(contributions_cfg(&json_obj(r#"[{"key": "a"}, {"key": "a"}]"#)).is_err());
    assert!(contributions_cfg(&json_obj(r#"[{"key": "a", "employee_rate": -1}]"#)).is_err());
    assert!(
        !income_tax_cfg(&json_obj(r#"{"method": "none"}"#))
            .unwrap()
            .progressive
    );
    let t = income_tax_cfg(&json_obj(
        r#"{"method": "progressive", "brackets": [[50000, 10], [null, 20]],
            "allowances": {"single": 12000}, "default_status": "single"}"#,
    ))
    .unwrap();
    assert!(t.progressive && t.brackets.len() == 2);
    assert!(income_tax_cfg(&json_obj(r#"{"method": "flat"}"#)).is_err());
    assert!(income_tax_cfg(&json_obj(
        r#"{"method": "progressive", "brackets": [[50000, 10]]}"#
    ))
    .is_err());
    assert!(income_tax_cfg(&json_obj(
        r#"{"method": "progressive", "brackets": [[null, 10]], "allowances": {"a": 1}, "default_status": "b"}"#
    ))
    .is_err());
}

#[test]
fn test_employment_in_month_prorates_joiners_and_leavers() {
    let day = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd);
    assert_eq!(employment_in_month(None, None, 2031, 4), Some((30, 30)));
    assert_eq!(
        employment_in_month(day(2031, 4, 15), None, 2031, 4),
        Some((16, 30))
    );
    assert_eq!(
        employment_in_month(None, day(2031, 4, 10), 2031, 4),
        Some((10, 30))
    );
    assert_eq!(employment_in_month(day(2031, 5, 1), None, 2031, 4), None);
    assert_eq!(employment_in_month(None, day(2031, 3, 31), 2031, 4), None);
    assert_eq!(employment_in_month(None, None, 2032, 2), Some((29, 29)));
    assert_eq!(employment_in_month(None, None, 2031, 12), Some((31, 31)));
}

#[test]
fn test_overtime_multiples_follow_the_tiers() {
    let workday = vec![(Some(d("1")), d("1.5")), (None, d("2"))];
    let rest = vec![
        (Some(d("8")), d("2")),
        (Some(d("9")), d("3")),
        (None, d("4")),
    ];
    assert_eq!(overtime_multiple(d("3"), &workday), d("5.5"));
    assert_eq!(overtime_multiple(d("0.5"), &workday), d("0.75"));
    assert_eq!(overtime_multiple(d("10"), &rest), d("23"));
}

#[test]
fn test_other_where_strings_match_the_settings_page() {
    assert_eq!(PAYROLL_SETTINGS_WHERE, "People & payroll \u{2192} Payroll");
}

#[tokio::test]
async fn the_seeded_settings_withhold_nothing_until_configured() {
    let app = support::app("hr_payroll_rules");
    let mut db = app.db().await;
    let cfg = payroll_settings(&mut db)
        .await
        .expect("seeded payroll settings");
    assert!(cfg.contributions.is_empty());
    assert!(!cfg.income_tax.progressive);
}

#[tokio::test]
async fn a_missing_or_malformed_payroll_setting_is_a_400_naming_it() {
    let app = support::app("hr_payroll_rules");
    let _guard = app.serial().await;
    let mut db = app.db().await;
    let saved: Option<String> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = 'payroll.rest_weekdays'")
            .fetch_one(&mut db)
            .await
            .unwrap();
    for (value, problem) in [
        (
            "[7]",
            "must be a list of weekday numbers (Monday = 0 \u{2026} Sunday = 6)",
        ),
        ("[true, 5]", ""),
        ("", "is not set"),
    ] {
        app.set_setting("payroll.rest_weekdays", value).await;
        let res = payroll_settings(&mut db).await;
        if problem.is_empty() {
            // bool is an int in Python: True is Tuesday
            assert!(res.unwrap().rest_weekdays.contains(&1));
        } else {
            let err = res.unwrap_err();
            assert_eq!(
                err.body()["detail"],
                format!(
                    "Setting 'payroll.rest_weekdays' {problem}. Set it in Settings \u{2192} {PAYROLL_SETTINGS_WHERE}."
                )
            );
            assert_eq!(err.status().as_u16(), 400);
        }
    }
    app.set_setting("payroll.rest_weekdays", saved.as_deref().unwrap_or(""))
        .await;
}
