//! Port of backend/app/services/face/matching.py: 1:N face search over enrolled descriptors, the server's
//! own identity decision for phones and kiosks alike.
//!
//! numpy maths reproduced bit for bit: `np.linalg.norm(matrix - q, axis=1)` is
//! `sqrt(np.add.reduce(d * d, axis=1))` on float64, and numpy's contiguous add-reduce is its pairwise sum
//! (8 accumulators per block of up to 128 items, combined as ((r0+r1)+(r2+r3))+((r4+r5)+(r6+r7)), the
//! remainder added in order, recursive halving above 128), started from the identity 0.0. Golden-tested
//! in tests/face_golden.rs.

use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::face::liveness::py_round;

pub const DESCRIPTOR_LEN: usize = 128;
/// Sample check: same person 0.34-0.48, different people 0.68-0.77.
pub const DEFAULT_MAX_DISTANCE: f64 = 0.50;
/// The runner-up employee must be at least this much further away.
pub const AMBIGUITY_MARGIN: f64 = 0.08;

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub employee_id: Option<String>,
    pub distance: Option<f64>,
    pub runner_up_id: Option<String>,
    pub runner_up_distance: Option<f64>,
}

impl Match {
    pub fn accepted(&self, max_distance: f64) -> bool {
        let Some(d) = self.distance else { return false };
        if self.employee_id.is_none() || d > max_distance {
            return false;
        }
        match self.runner_up_distance {
            None => true,
            Some(r) => r - d >= AMBIGUITY_MARGIN,
        }
    }

    pub fn ambiguous(&self, max_distance: f64) -> bool {
        match (self.distance, self.runner_up_distance) {
            (Some(d), Some(r)) => d <= max_distance && r - d < AMBIGUITY_MARGIN,
            _ => false,
        }
    }
}

/// `valid_descriptor(value)` for a list of floats (the kiosk body's `list[float]`).
pub fn valid_descriptor(value: &[f64]) -> bool {
    value.len() == DESCRIPTOR_LEN && value.iter().all(|x| x.is_finite())
}

/// `valid_descriptor(value)` for arbitrary JSON: `np.asarray(value, dtype=float)` must succeed with shape
/// (128,) and every item finite. Numbers and booleans convert, strings convert as `float(str)` does
/// (approximated with Rust's float parser on the stripped text, plus Python's inf/nan spellings, which are
/// not finite anyway), anything else (null, objects, nested lists) is refused.
/// Not identical: an int too large for a float makes numpy raise OverflowError, which Python does not
/// catch (a 500); here it is simply invalid.
pub fn valid_descriptor_value(value: &Value) -> bool {
    let Some(items) = value.as_array() else {
        return false;
    };
    if items.len() != DESCRIPTOR_LEN {
        return false;
    }
    items.iter().all(|v| match v {
        Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
        Value::Bool(_) => true,
        Value::String(s) => {
            let t = granary_compat::text::pystr::strip(s).replace('_', "");
            t.parse::<f64>().is_ok_and(f64::is_finite)
        }
        _ => false,
    })
}

/// numpy `pairwise_sum_DOUBLE` (contiguous, stride 1).
pub fn np_pairwise_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        let mut res = 0.0;
        for &x in a {
            res += x;
        }
        res
    } else if n <= 128 {
        let mut r = [0.0f64; 8];
        r.copy_from_slice(&a[..8]);
        let mut i = 8;
        while i < n - (n % 8) {
            for j in 0..8 {
                r[j] += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        np_pairwise_sum(&a[..n2]) + np_pairwise_sum(&a[n2..])
    }
}

/// One row of `np.linalg.norm(matrix - q, axis=1)`.
pub fn np_row_distance(row: &[f64], q: &[f64]) -> f64 {
    let sq: Vec<f64> = row
        .iter()
        .zip(q)
        .map(|(a, b)| {
            let d = a - b;
            d * d
        })
        .collect();
    (0.0 + np_pairwise_sum(&sq)).sqrt()
}

/// `gallery(db, employee_ids)`: active descriptors, (employee id per row, N x 128 matrix), in the
/// database's row order (the Python query has no ORDER BY either).
pub async fn gallery(
    db: &mut PgConnection,
    employee_ids: Option<&[Uuid]>,
) -> Result<(Vec<String>, Vec<Vec<f64>>), sqlx::Error> {
    let base = "SELECT face_descriptors.employee_id, face_descriptors.descriptor \
                FROM face_descriptors JOIN face_enrollments \
                ON face_enrollments.employee_id = face_descriptors.employee_id \
                WHERE face_enrollments.is_active = true";
    let rows: Vec<(Uuid, Vec<f64>)> = match employee_ids {
        None => sqlx::query_as(base).fetch_all(&mut *db).await?,
        Some(ids) => {
            sqlx::query_as(&format!(
                "{base} AND face_descriptors.employee_id = ANY($1)"
            ))
            .bind(ids)
            .fetch_all(&mut *db)
            .await?
        }
    };
    Ok(rows.into_iter().map(|(id, d)| (id.to_string(), d)).unzip())
}

/// `search_in(ids, matrix, *queries)`: best employee for any of the query descriptors (closest descriptor
/// wins) and the best *other* employee.
pub fn search_in(ids: &[String], matrix: &[Vec<f64>], queries: &[&[f64]]) -> Match {
    if ids.is_empty() {
        return Match {
            employee_id: None,
            distance: None,
            runner_up_id: None,
            runner_up_distance: None,
        };
    }
    // dict insertion order: first time an employee gets a finite distance
    let mut best: Vec<(String, f64)> = Vec::new();
    for q in queries {
        for (emp, row) in ids.iter().zip(matrix) {
            let d = np_row_distance(row, q);
            match best.iter_mut().find(|(e, _)| e == emp) {
                Some(slot) => {
                    if d < slot.1 {
                        slot.1 = d;
                    }
                }
                None => {
                    if d < f64::INFINITY {
                        best.push((emp.clone(), d));
                    }
                }
            }
        }
    }
    // sorted(..., key=distance): stable
    best.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    // Python: ranked[0] raises IndexError when no distance was finite (NaN rows); not reachable with
    // valid descriptors, so treated as "no match" here.
    let Some(first) = best.first() else {
        return Match {
            employee_id: None,
            distance: None,
            runner_up_id: None,
            runner_up_distance: None,
        };
    };
    let second = best.get(1);
    Match {
        employee_id: Some(first.0.clone()),
        distance: Some(py_round(first.1, 4)),
        runner_up_id: second.map(|s| s.0.clone()),
        runner_up_distance: second.map(|s| py_round(s.1, 4)),
    }
}

/// `search(db, *queries)`.
pub async fn search(db: &mut PgConnection, queries: &[&[f64]]) -> Result<Match, sqlx::Error> {
    let (ids, matrix) = gallery(db, None).await?;
    Ok(search_in(&ids, &matrix, queries))
}

/// `max_distance(db)`: the `hr.face_match_distance` setting clamped to 0.30-0.65, the default when unset
/// or not a number.
pub async fn max_distance(db: &mut PgConnection) -> Result<f64, sqlx::Error> {
    let row: Option<Option<String>> = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'hr.face_match_distance'",
    )
    .fetch_optional(&mut *db)
    .await?;
    let value = match row.flatten() {
        Some(v) if !v.is_empty() => py_float_str(&v).unwrap_or(DEFAULT_MAX_DISTANCE),
        _ => DEFAULT_MAX_DISTANCE,
    };
    // min(max(value, 0.30), 0.65)
    let lo = if 0.30 > value { 0.30 } else { value };
    Ok(if 0.65 < lo { 0.65 } else { lo })
}

/// Python `float(str)`: stripped text, underscores between digits, inf/nan spellings.
fn py_float_str(s: &str) -> Option<f64> {
    let t = granary_compat::text::pystr::strip(s);
    if t.is_empty() || t.starts_with('_') || t.ends_with('_') || t.contains("__") {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    let body = lower.trim_start_matches(['+', '-']);
    let neg = lower.starts_with('-');
    if lower.len() - body.len() > 1 {
        return None;
    }
    match body {
        "inf" | "infinity" => {
            return Some(if neg {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            })
        }
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    // underscores only between digits
    let b = t.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c == b'_'
            && !(i > 0 && b[i - 1].is_ascii_digit() && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            return None;
        }
    }
    let cleaned = t.replace('_', "");
    if !cleaned
        .bytes()
        .all(|c| c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return None;
    }
    cleaned.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_parse() {
        assert_eq!(py_float_str(" 0.45 "), Some(0.45));
        assert_eq!(py_float_str("1_0.5"), Some(10.5));
        assert_eq!(py_float_str("abc"), None);
        assert_eq!(py_float_str("_1"), None);
        assert!(py_float_str("nan").unwrap().is_nan());
    }
}
