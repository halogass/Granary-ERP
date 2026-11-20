//! Face check telemetry (migration 0165, Release 1 of the face clock plan): every liveness check a clock runs
//! (kiosk tablet or a personal device) reports how it ended, so HR > Attendance > Attempts shows whether the
//! timeouts went away. Rows carry no image and no face data.
//!
//! Since 0167 a row may also carry `note` (what went wrong on the device: a stalled tick, a lost WebGL context,
//! the engine moved to the CPU, the camera resumed) and `details` (numbers only: first tick and tick count of an
//! active check; reason, corr, delta, lag, spoof score, faces per frame, frame size of a passive one).

use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

pub const MODES: [&str; 2] = ["kiosk", "personal"];
pub const OUTCOMES: [&str; 5] = [
    "done",
    "timeout",
    "aborted",
    "passive_inconclusive",
    "passive_failed",
];
/// The tracker's steps (frontend/src/lib/faceLiveness.ts `Step`, without the end states but with "done").
pub const STEPS: [&str; 5] = ["find_face", "hold_still", "do_action", "return", "done"];
pub const BACKENDS: [&str; 3] = ["webgl", "cpu", "unknown"];
pub const USER_AGENT_MAX: usize = 200;
/// Characters of `note` kept.
pub const NOTE_MAX: usize = 200;
/// Why a passive check did not pass (details.reason).
pub const PASSIVE_REASONS: [&str; 4] = ["flash_failed", "spoof", "flash_inconclusive", "no_face"];

/// One attempt as a clock reports it.
#[derive(Debug, Clone)]
pub struct Attempt {
    pub mode: &'static str,
    pub kiosk_device_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub employee_id: Option<Uuid>,
    pub action: String,
    pub outcome: String,
    pub step_reached: String,
    pub duration_ms: i32,
    pub avg_tick_ms: Option<i32>,
    pub backend: String,
    pub user_agent: Option<String>,
    /// 0167: what went wrong on the device (cut to NOTE_MAX characters).
    pub note: Option<String>,
    /// 0167: the numbers behind the outcome (a JSON object, never images or face data).
    pub details: Option<Value>,
}

/// The user agent cut to the column (by characters).
pub fn truncate_ua(ua: &str) -> String {
    ua.chars().take(USER_AGENT_MAX).collect()
}

/// Insert one attempt; returns its id.
pub async fn record(db: &mut PgConnection, a: &Attempt) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO face_check_attempts (id, mode, kiosk_device_id, user_id, employee_id, action, outcome, \
         step_reached, duration_ms, avg_tick_ms, backend, user_agent, note, details) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(id)
    .bind(a.mode)
    .bind(a.kiosk_device_id)
    .bind(a.user_id)
    .bind(a.employee_id)
    .bind(&a.action)
    .bind(&a.outcome)
    .bind(&a.step_reached)
    .bind(a.duration_ms)
    .bind(a.avg_tick_ms)
    .bind(&a.backend)
    .bind(a.user_agent.as_deref().map(truncate_ua))
    .bind(
        a.note
            .as_deref()
            .map(|n| n.chars().take(NOTE_MAX).collect::<String>()),
    )
    .bind(a.details.clone().filter(Value::is_object))
    .execute(db)
    .await?;
    Ok(id)
}

async fn counts(db: &mut PgConnection, column: &str, days: i64) -> Result<Value, sqlx::Error> {
    // column is one of a fixed set chosen below, never user input
    let rows: Vec<(String, i64)> = sqlx::query_as(&format!(
        "SELECT {column}, count(*) FROM face_check_attempts \
         WHERE created_at >= now() - make_interval(days => $1::int) GROUP BY {column} ORDER BY {column}"
    ))
    .bind(days as i32)
    .fetch_all(&mut *db)
    .await?;
    Ok(Value::Object(
        rows.into_iter().map(|(k, n)| (k, json!(n))).collect(),
    ))
}

/// The passive checks of the last `days` days (0167): why the ones that fell back did not pass
/// (details.reason of passive_* attempts), how many passed (clock events whose details.passive.method is
/// "passive") and the median corr, delta and anti-spoof score over both.
pub async fn passive_summary(db: &mut PgConnection, days: i64) -> Result<Value, sqlx::Error> {
    let reasons: Vec<(String, i64)> = sqlx::query_as(
        "SELECT details->>'reason', count(*) FROM face_check_attempts \
         WHERE created_at >= now() - make_interval(days => $1::int) \
           AND outcome IN ('passive_inconclusive', 'passive_failed') AND details->>'reason' IS NOT NULL \
         GROUP BY 1 ORDER BY 1",
    )
    .bind(days as i32)
    .fetch_all(&mut *db)
    .await?;
    let num = |path: &str| {
        format!("CASE WHEN jsonb_typeof({path}) = 'number' THEN ({path})::text::float8 END")
    };
    // passes from the clock log, fallbacks from the attempts: each passive verdict counted once
    let sql = format!(
        "WITH v AS ( \
           SELECT {c1} AS corr, {d1} AS delta, {s1} AS spoof FROM face_check_attempts \
           WHERE created_at >= now() - make_interval(days => $1::int) \
             AND outcome IN ('passive_inconclusive', 'passive_failed') \
           UNION ALL \
           SELECT {c2}, {d2}, {s2} FROM clock_events \
           WHERE created_at >= now() - make_interval(days => $1::int) \
             AND details->'passive'->>'method' = 'passive') \
         SELECT (SELECT count(*) FROM clock_events WHERE created_at >= now() - make_interval(days => $1::int) \
                   AND details->'passive'->>'method' = 'passive'), \
                percentile_cont(0.5) WITHIN GROUP (ORDER BY corr), \
                percentile_cont(0.5) WITHIN GROUP (ORDER BY delta), \
                percentile_cont(0.5) WITHIN GROUP (ORDER BY spoof) FROM v",
        c1 = num("details->'corr'"),
        d1 = num("details->'delta'"),
        s1 = num("details->'spoof_real'"),
        c2 = num("details->'passive'->'corr'"),
        d2 = num("details->'passive'->'delta'"),
        s2 = num("details->'passive'->'spoof_real'"),
    );
    let (passed, corr, delta, spoof): (i64, Option<f64>, Option<f64>, Option<f64>) =
        sqlx::query_as(&sql)
            .bind(days as i32)
            .fetch_one(&mut *db)
            .await?;
    let r4 = |x: Option<f64>| x.map(|v| (v * 10_000.0).round() / 10_000.0);
    let not_passed: i64 = reasons.iter().map(|(_, n)| n).sum();
    Ok(json!({
        "passed": passed,
        "not_passed": not_passed,
        "reasons": Value::Object(reasons.into_iter().map(|(k, n)| (k, json!(n))).collect()),
        "median_corr": r4(corr),
        "median_delta": r4(delta),
        "median_spoof_real": r4(spoof),
    }))
}

/// The last `days` days: totals by outcome, by mode, by step reached, by mode and outcome, the timeout rate
/// (timeouts / all attempts), the mean tick time and the passive checks (`passive_summary`).
pub async fn summary(db: &mut PgConnection, days: i64) -> Result<Value, sqlx::Error> {
    let (total, timeouts, avg_tick): (i64, i64, Option<f64>) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE outcome = 'timeout'), avg(avg_tick_ms)::float8 \
         FROM face_check_attempts WHERE created_at >= now() - make_interval(days => $1::int)",
    )
    .bind(days as i32)
    .fetch_one(&mut *db)
    .await?;
    let by_mode_outcome: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT mode, outcome, count(*) FROM face_check_attempts \
         WHERE created_at >= now() - make_interval(days => $1::int) GROUP BY mode, outcome ORDER BY mode, outcome",
    )
    .bind(days as i32)
    .fetch_all(&mut *db)
    .await?;
    let mut mode_outcome = serde_json::Map::new();
    for (mode, outcome, n) in by_mode_outcome {
        let entry = mode_outcome
            .entry(mode)
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Value::Object(m) = entry {
            m.insert(outcome, json!(n));
        }
    }
    let rate = if total > 0 {
        (timeouts as f64 / total as f64 * 10_000.0).round() / 10_000.0
    } else {
        0.0
    };
    Ok(json!({
        "days": days,
        "total": total,
        "timeouts": timeouts,
        "timeout_rate": rate,
        "avg_tick_ms": avg_tick.map(|t| t.round() as i64),
        "by_outcome": counts(db, "outcome", days).await?,
        "by_mode": counts(db, "mode", days).await?,
        "by_step": counts(db, "step_reached", days).await?,
        "by_mode_outcome": Value::Object(mode_outcome),
        "passive": passive_summary(db, days).await?,
    }))
}
