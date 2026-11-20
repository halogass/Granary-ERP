//! Port of backend/app/services/face/challenge.py: single-use liveness challenges for personal devices
//! (the server picks the action, the client must perform it) and the fixed-window rate limiter.

use granary_compat::crypto::tokens::{token_urlsafe, OsRandom, RandomSource};
use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
use granary_core::errors::ApiError;
use granary_core::redis_client::get_redis;
use redis::AsyncCommands;
use serde::Serialize;
use sqlx::PgConnection;

use granary_core::settings_store::{require_int, require_json, SettingError, StoreResult};

use crate::face::liveness::ISSUED_ACTIONS;

/// The Python default, kept for `issue` (the services tests); the API issues with `ttl_seconds` (the setting).
pub const TTL_SECONDS: u64 = 45;

/// Budget per step of the device's liveness check, `{"find", "action", "return"}` in milliseconds.
pub const STEP_TIMEOUTS_KEY: &str = "hr.face.step_timeouts_ms";
/// How long a phone's challenge nonce lives (seconds).
pub const TTL_KEY: &str = "hr.face.challenge_ttl_seconds";
const WHERE: &str = "People & payroll \u{2192} Attendance";
/// Time left for the upload after the last step: the nonce never expires before the steps can end.
pub const UPLOAD_MARGIN_SECONDS: u64 = 15;

/// The device's per-step budgets (milliseconds), each started when its step begins.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct StepTimeouts {
    pub find: u64,
    pub action: u64,
    #[serde(rename = "return")]
    pub return_: u64,
}

impl StepTimeouts {
    pub fn total_ms(&self) -> u64 {
        self.find + self.action + self.return_
    }
}

/// `hr.face.step_timeouts_ms`; a missing key or a step outside 1000-60000 ms names the setting.
pub async fn step_timeouts(db: &mut PgConnection) -> StoreResult<StepTimeouts> {
    let v = require_json(db, STEP_TIMEOUTS_KEY, WHERE).await?;
    let step = |name: &str| -> Result<u64, SettingError> {
        match v.get(name).and_then(serde_json::Value::as_u64) {
            Some(ms) if (1000..=60_000).contains(&ms) => Ok(ms),
            _ => Err(SettingError::new(
                STEP_TIMEOUTS_KEY,
                &format!("needs \"{name}\" in milliseconds, 1000 to 60000"),
                WHERE,
            )),
        }
    };
    Ok(StepTimeouts {
        find: step("find")?,
        action: step("action")?,
        return_: step("return")?,
    })
}

/// `hr.face.challenge_ttl_seconds` (10-600), raised to the steps' total plus the upload margin.
pub async fn ttl_seconds(db: &mut PgConnection, steps: &StepTimeouts) -> StoreResult<u64> {
    let ttl = require_int(db, TTL_KEY, WHERE).await?;
    if !(10..=600).contains(&ttl) {
        return Err(SettingError::new(TTL_KEY, "must be 10 to 600 seconds", WHERE).into());
    }
    Ok((ttl as u64).max(steps.total_ms().div_ceil(1000) + UPLOAD_MARGIN_SECONDS))
}

fn key(nonce: &str) -> String {
    format!("face:challenge:{nonce}")
}

fn unavailable() -> ApiError {
    ApiError::http(
        503,
        "Clock-in is temporarily unavailable: try again shortly",
    )
}

fn expired() -> ApiError {
    ApiError::http(400, "The check expired: start again")
}

/// `{"nonce", "action", "expires_in"}` in Python's key order.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Issued {
    pub nonce: String,
    pub action: String,
    pub expires_in: u64,
}

/// `issue(event_type, user_id)`.
pub async fn issue(event_type: &str, user_id: &str) -> Result<Issued, ApiError> {
    issue_with(&mut OsRandom, event_type, user_id).await
}

/// `issue` with the nonce living `ttl` seconds (the API passes `ttl_seconds`).
pub async fn issue_for(event_type: &str, user_id: &str, ttl: u64) -> Result<Issued, ApiError> {
    issue_inner(&mut OsRandom, event_type, user_id, ttl, &Extra::default()).await
}

/// What a challenge may carry besides its action (migration 0166, passive liveness): the screen-flash
/// colours of a passive check, or why a passive check fell back to this active one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extra {
    pub colors: Option<Vec<[u8; 3]>>,
    pub fallback_reason: Option<String>,
    /// The failed passive check's numbers ({corr, delta, spoof_real}), logged with the active clock.
    pub passive_metrics: Option<serde_json::Value>,
}

/// `issue_for` with the passive colours or the fallback reason stored next to the action.
pub async fn issue_extra(
    event_type: &str,
    user_id: &str,
    ttl: u64,
    extra: &Extra,
) -> Result<Issued, ApiError> {
    issue_inner(&mut OsRandom, event_type, user_id, ttl, extra).await
}

/// `issue` with an explicit randomness source (token_urlsafe(24) drawn before choice(ISSUED_ACTIONS); only head turns since 6 Nov 2026).
pub async fn issue_with(
    rng: &mut impl RandomSource,
    event_type: &str,
    user_id: &str,
) -> Result<Issued, ApiError> {
    issue_inner(rng, event_type, user_id, TTL_SECONDS, &Extra::default()).await
}

async fn issue_inner(
    rng: &mut impl RandomSource,
    event_type: &str,
    user_id: &str,
    ttl: u64,
    extra: &Extra,
) -> Result<Issued, ApiError> {
    let nonce = token_urlsafe(rng, 24);
    let action = ISSUED_ACTIONS[rng.below(ISSUED_ACTIONS.len())];
    let mut fields = vec![
        ("action".into(), PyValue::Str(action.into())),
        ("event_type".into(), PyValue::Str(event_type.into())),
        ("user_id".into(), PyValue::Str(user_id.into())),
    ];
    if let Some(colors) = &extra.colors {
        let list = colors
            .iter()
            .map(|c| PyValue::List(c.iter().map(|&v| PyValue::Int(v as i128)).collect()))
            .collect();
        fields.push(("colors".into(), PyValue::List(list)));
    }
    if let Some(reason) = &extra.fallback_reason {
        fields.push(("fallback_reason".into(), PyValue::Str(reason.clone())));
    }
    if let Some(m) = &extra.passive_metrics {
        fields.push(("passive_metrics".into(), PyValue::from(m)));
    }
    let payload = dumps(&PyValue::Dict(fields), &DumpOpts::default())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("{e:?}")))?;
    let mut conn = get_redis().conn().await.map_err(|_| unavailable())?;
    let _: () = conn
        .set_ex(key(&nonce), payload, ttl)
        .await
        .map_err(|_| unavailable())?;
    Ok(Issued {
        nonce,
        action: action.into(),
        expires_in: ttl,
    })
}

/// `consume(nonce, event_type, user_id)`: the challenge's action; the nonce is spent whether or not the
/// attempt succeeds.
pub async fn consume(nonce: &str, event_type: &str, user_id: &str) -> Result<String, ApiError> {
    Ok(consume_full(nonce, event_type, user_id).await?.0)
}

/// `consume` with what else the challenge carries (the passive colours, a fallback reason).
pub async fn consume_full(
    nonce: &str,
    event_type: &str,
    user_id: &str,
) -> Result<(String, Extra), ApiError> {
    // len() counts code points in Python
    if nonce.is_empty() || nonce.chars().count() > 64 {
        return Err(expired());
    }
    let k = key(nonce);
    let mut conn = get_redis().conn().await.map_err(|_| unavailable())?;
    let (raw, _): (Option<String>, i64) = redis::pipe()
        .atomic()
        .get(&k)
        .del(&k)
        .query_async(&mut conn)
        .await
        .map_err(|_| unavailable())?;
    let raw = match raw {
        Some(r) if !r.is_empty() => r,
        _ => return Err(expired()),
    };
    // json.loads / data.get / data["action"]: a malformed stored value is a 500 in Python too
    let data: serde_json::Value = serde_json::from_str(&raw).map_err(ApiError::internal)?;
    let obj = data.as_object().ok_or_else(|| {
        ApiError::internal(anyhow::anyhow!("AttributeError: challenge is not a dict"))
    })?;
    let same = |field: &str, want: &str| obj.get(field).and_then(|v| v.as_str()) == Some(want);
    if !same("event_type", event_type) || !same("user_id", user_id) {
        return Err(ApiError::http(
            400,
            "The check does not belong to this request: start again",
        ));
    }
    let action = match obj.get("action") {
        Some(serde_json::Value::String(a)) => a.clone(),
        _ => return Err(ApiError::internal(anyhow::anyhow!("KeyError: 'action'"))),
    };
    let colors = obj.get("colors").and_then(|v| v.as_array()).map(|list| {
        list.iter()
            .filter_map(|c| {
                let t = c.as_array()?;
                let ch = |i: usize| t.get(i).and_then(|x| x.as_u64()).map(|x| x.min(255) as u8);
                Some([ch(0)?, ch(1)?, ch(2)?])
            })
            .collect()
    });
    let fallback_reason = obj
        .get("fallback_reason")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let passive_metrics = obj
        .get("passive_metrics")
        .filter(|v| v.is_object())
        .cloned();
    Ok((
        action,
        Extra {
            colors,
            fallback_reason,
            passive_metrics,
        },
    ))
}

/// `rate_limit(key, limit, window=60)`: fixed-window counter; fails open if Redis is down (the face check
/// still protects the endpoint).
pub async fn rate_limit(key: &str, limit: i64, window: i64) -> Result<(), ApiError> {
    let bucket = format!("face:rl:{key}");
    let counted = async {
        let mut conn = get_redis().conn().await?;
        let count: i64 = conn.incr(&bucket, 1).await?;
        if count == 1 {
            let _: () = conn.expire(&bucket, window).await?;
        }
        Ok::<i64, redis::RedisError>(count)
    }
    .await;
    let Ok(count) = counted else {
        return Ok(());
    };
    if count > limit {
        return Err(ApiError::http(
            429,
            "Too many attempts: wait a minute and try again",
        ));
    }
    Ok(())
}
