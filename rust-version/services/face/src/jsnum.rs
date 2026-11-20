//! JavaScript number semantics the Node service gets for free: `Math.round`, the service's
//! `round(n, d) = Math.round(n * 10 ** d) / 10 ** d`, and how `JSON.stringify` prints a number.

/// `Math.round`: the nearest integer, ties toward +Infinity (exact: `x - floor(x)` is exact for doubles).
pub fn math_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    let r = if x - f >= 0.5 { f + 1.0 } else { f };
    // Math.round keeps the sign of zero for -0.5 <= x < 0 (prints as "0" anyway)
    if r == 0.0 && x.is_sign_negative() {
        -0.0
    } else {
        r
    }
}

/// `round(n, d)` of services/face/src/index.js.
pub fn round(n: f64, d: i32) -> f64 {
    let p = 10f64.powi(d); // 10 ** d is exact for d <= 22
    math_round(n * p) / p
}

/// `JSON.stringify(x)` for a number: Number.prototype.toString, `null` for NaN and the infinities.
pub fn json(x: f64) -> String {
    if !x.is_finite() {
        return "null".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x < 0.0 {
        return format!("-{}", json(-x));
    }
    // Shortest round-trip digits: Rust's `{:e}` is shortest, like V8's.
    let sci = format!("{x:e}");
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let k = digits.len() as i64;
    let n = exp.parse::<i64>().unwrap_or(0) + 1; // value = 0.d1d2... x 10^n
    if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign = if e < 0 { '-' } else { '+' };
        if k == 1 {
            format!("{digits}e{sign}{}", e.abs())
        } else {
            format!("{}.{}e{sign}{}", &digits[..1], &digits[1..], e.abs())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_round_matches_javascript() {
        assert_eq!(math_round(2.5), 3.0);
        assert_eq!(math_round(-2.5), -2.0);
        assert_eq!(math_round(0.49999999999999994), 0.0);
        assert_eq!(math_round(-0.4).to_bits(), (-0.0f64).to_bits());
        assert_eq!(math_round(1.5), 2.0);
    }

    #[test]
    fn json_numbers_match_javascript() {
        assert_eq!(json(100.0), "100");
        assert_eq!(json(-0.0), "0");
        assert_eq!(json(0.000001), "0.000001");
        assert_eq!(json(0.0000001), "1e-7");
        assert_eq!(json(123.45), "123.45");
        assert_eq!(json(-0.123456), "-0.123456");
        assert_eq!(json(1e21), "1e+21");
        assert_eq!(json(f64::NAN), "null");
        assert_eq!(json(round(0.123456789, 6)), "0.123457");
        assert_eq!(json(round(1.005, 2)), "1"); // 1.005 * 100 = 100.49999999999999
    }
}
