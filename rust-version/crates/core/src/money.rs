//! backend/app/core/money.py: Decimal-only money arithmetic, rounded to the currency's minor unit with
//! ROUND_HALF_UP (halves away from zero). Pure functions: the number of places comes from
//! `settings_store::currency_places`.

#![allow(non_snake_case)]

use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;

use crate::validation::py_float_repr;

/// `HUNDRED`: percent -> fraction.
pub const HUNDRED: Decimal = Decimal::ONE_HUNDRED;

/// Something `D()` accepts.
pub trait ToDec {
    fn to_dec(&self) -> Decimal;
}

impl ToDec for Decimal {
    fn to_dec(&self) -> Decimal {
        *self
    }
}

impl ToDec for f64 {
    /// `Decimal(str(x))`: floats go through repr so 0.1 stays 0.1.
    fn to_dec(&self) -> Decimal {
        let r = py_float_repr(*self);
        r.parse::<Decimal>()
            .or_else(|_| Decimal::from_scientific(&r))
            .unwrap_or(Decimal::ZERO)
    }
}

impl ToDec for i64 {
    fn to_dec(&self) -> Decimal {
        Decimal::from(*self)
    }
}

impl ToDec for i32 {
    fn to_dec(&self) -> Decimal {
        Decimal::from(*self)
    }
}

impl ToDec for &str {
    fn to_dec(&self) -> Decimal {
        crate::validation::types::PyDec::parse(self)
            .and_then(|d| d.to_decimal())
            .unwrap_or(Decimal::ZERO)
    }
}

impl<T: ToDec> ToDec for Option<T> {
    /// `D(None)` is 0.
    fn to_dec(&self) -> Decimal {
        match self {
            Some(x) => x.to_dec(),
            None => Decimal::ZERO,
        }
    }
}

impl<T: ToDec> ToDec for &T {
    fn to_dec(&self) -> Decimal {
        (*self).to_dec()
    }
}

/// `D(x)`.
pub fn D(x: impl ToDec) -> Decimal {
    x.to_dec()
}

/// `quantize_money(x, places)`: `D(x).quantize(Decimal(1).scaleb(-places), ROUND_HALF_UP)`. The result
/// has exactly `places` decimals (1.5 at 2 places is 1.50), as Python's quantize gives.
pub fn quantize_money(x: impl ToDec, places: u32) -> Decimal {
    let mut d = x
        .to_dec()
        .round_dp_with_strategy(places, RoundingStrategy::MidpointAwayFromZero);
    d.rescale(places);
    d
}

/// `pct_of(base, pct, places)`: `pct` percent of `base`, rounded.
pub fn pct_of(base: impl ToDec, pct: impl ToDec, places: u32) -> Decimal {
    quantize_money(base.to_dec() * pct.to_dec() / HUNDRED, places)
}

/// `LineInput`.
#[derive(Clone, Debug, Default)]
pub struct LineInput {
    pub quantity: Decimal,
    pub unit_price: Decimal,
    pub discount_pct: Decimal,
    pub tax_pct: Decimal,
    pub tax_code: Option<String>,
    pub tax_name: Option<String>,
}

/// `LineResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct LineResult {
    /// qty x unit price
    pub gross: Decimal,
    pub discount_amount: Decimal,
    /// net of discount, before tax: what InvoiceItem.amount stores
    pub amount: Decimal,
    pub tax_amount: Decimal,
}

/// One `tax_lines` entry: `{code, name, rate, base, amount}` (JSONB). The numbers are kept exact and written
/// the way Python's json writes a float (`11.0`, `91000.0`, `19.99`), see `dec_json` (13 Nov 2026: they went
/// through f64 before).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TaxLine {
    pub code: Option<String>,
    pub name: Option<String>,
    #[serde(serialize_with = "ser_dec_json")]
    pub rate: Decimal,
    #[serde(serialize_with = "ser_dec_json")]
    pub base: Decimal,
    #[serde(serialize_with = "ser_dec_json")]
    pub amount: Decimal,
}

/// A money amount or rate as an exact JSON number in the shape Python's json gives a float: the Decimal's
/// digits without trailing zeros, and `.0` when it is whole (`11` -> `11.0`, `19.990` -> `19.99`). Needs
/// serde_json's arbitrary_precision (workspace Cargo.toml), so no digit is lost.
pub fn dec_json(d: Decimal) -> serde_json::Value {
    let mut s = d.normalize().to_string();
    if !s.contains('.') {
        s.push_str(".0");
    }
    serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)
}

fn ser_dec_json<S: serde::Serializer>(d: &Decimal, s: S) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(&dec_json(*d), s)
}

/// `Totals`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Totals {
    pub lines: Vec<LineResult>,
    /// sum of line net amounts
    pub subtotal: Decimal,
    pub discount_total: Decimal,
    pub tax_total: Decimal,
    pub tax_lines: Vec<TaxLine>,
    pub total: Decimal,
}

/// `float(Decimal)`: correctly rounded, like parsing the decimal text.
pub fn dec_to_f64(d: Decimal) -> f64 {
    d.to_string().parse().unwrap_or(0.0)
}

/// `compute_lines(lines, places)`: per-line discount and tax, each rounded; totals are sums of rounded
/// lines; tax grouped by (tax_code, rate) in first-seen order.
#[allow(clippy::type_complexity)]
pub fn compute_lines<'a>(lines: impl IntoIterator<Item = &'a LineInput>, places: u32) -> Totals {
    let mut out = Totals::default();
    // (code, pct) -> (name, base, amount)
    let mut groups: Vec<((Option<String>, Decimal), Option<String>, Decimal, Decimal)> = Vec::new();
    for ln in lines {
        let gross = quantize_money(ln.quantity * ln.unit_price, places);
        let discount = pct_of(gross, ln.discount_pct, places);
        let net = gross - discount;
        let tax = pct_of(net, ln.tax_pct, places);
        out.lines.push(LineResult {
            gross,
            discount_amount: discount,
            amount: net,
            tax_amount: tax,
        });
        out.subtotal += net;
        out.discount_total += discount;
        out.tax_total += tax;
        if !ln.tax_pct.is_zero() {
            let key = (ln.tax_code.clone(), ln.tax_pct);
            match groups.iter_mut().find(|g| g.0 == key) {
                Some(g) => {
                    g.2 += net;
                    g.3 += tax;
                }
                None => {
                    // `ln.tax_name or ln.tax_code`: an empty name falls back too.
                    let name = match &ln.tax_name {
                        Some(n) if !n.is_empty() => Some(n.clone()),
                        _ => ln.tax_code.clone(),
                    };
                    groups.push((key, name, net, tax));
                }
            }
        }
    }
    out.total = out.subtotal + out.tax_total;
    out.tax_lines = groups
        .into_iter()
        .map(|((code, pct), name, base, amount)| TaxLine {
            code,
            name,
            rate: pct,
            base,
            amount,
        })
        .collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn d_and_quantize() {
        assert_eq!(D(0.1f64).to_string(), "0.1");
        assert_eq!(D(None::<Decimal>), Decimal::ZERO);
        assert_eq!(quantize_money(d("1234.5"), 0).to_string(), "1235");
        assert_eq!(quantize_money(d("-1234.5"), 0).to_string(), "-1235");
        assert_eq!(quantize_money(d("1.005"), 2).to_string(), "1.01");
        assert_eq!(quantize_money(d("1.5"), 2).to_string(), "1.50");
        assert_eq!(quantize_money(2.675f64, 2).to_string(), "2.68"); // str(2.675) is "2.675"
        assert_eq!(pct_of(d("1000"), d("11"), 0).to_string(), "110");
        assert_eq!(pct_of(d("99.99"), d("11"), 2).to_string(), "11.00");
    }

    #[test]
    fn compute() {
        let lines = vec![
            LineInput {
                quantity: d("3"),
                unit_price: d("33333.33"),
                discount_pct: d("10"),
                tax_pct: d("11"),
                tax_code: Some("VAT".into()),
                tax_name: Some("VAT 11%".into()),
            },
            LineInput {
                quantity: d("1"),
                unit_price: d("1000"),
                discount_pct: d("0"),
                tax_pct: d("11.00"),
                tax_code: Some("VAT".into()),
                tax_name: None,
            },
            LineInput {
                quantity: d("2"),
                unit_price: d("50"),
                ..Default::default()
            },
        ];
        let t = compute_lines(&lines, 0);
        assert_eq!(t.lines[0].gross.to_string(), "100000");
        assert_eq!(t.lines[0].discount_amount.to_string(), "10000");
        assert_eq!(t.lines[0].tax_amount.to_string(), "9900");
        assert_eq!(t.subtotal.to_string(), "91100");
        assert_eq!(t.tax_total.to_string(), "10010");
        assert_eq!(t.total.to_string(), "101110");
        assert_eq!(t.tax_lines.len(), 1);
        assert_eq!(t.tax_lines[0].name.as_deref(), Some("VAT 11%"));
        assert_eq!(t.tax_lines[0].rate.to_string(), "11");
        assert_eq!(t.tax_lines[0].base.to_string(), "91000");
        let j = serde_json::to_value(&t.tax_lines[0]).unwrap();
        assert_eq!(j["rate"].to_string(), "11.0");
        assert_eq!(j["base"].to_string(), "91000.0");
        assert_eq!(j["amount"].to_string(), "10010.0");
    }

    #[test]
    fn dec_json_is_exact_and_python_float_shaped() {
        assert_eq!(dec_json(d("19.990")).to_string(), "19.99");
        assert_eq!(dec_json(d("0")).to_string(), "0.0");
        assert_eq!(dec_json(d("-2.50")).to_string(), "-2.5");
        // past f64's 15-17 significant digits: kept digit for digit
        assert_eq!(
            dec_json(d("123456789012345678.91")).to_string(),
            "123456789012345678.91"
        );
        assert_eq!(dec_json(d("0.1")).as_f64(), Some(0.1));
    }
}
