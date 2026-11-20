//! Passive liveness settings and helpers (migration 0166, Release 2 of the face clock plan, 6 Nov 2026; defaults
//! flash_ms 400, settle_ms 340, flash_min_delta 1.5 chromaticity units since 0167).
//!
//! Phones: the server picks the flash colours with the challenge (kept in the Redis challenge payload), the
//! sidecar's `POST /passive` measures the skin-region colour of each frame and the anti-spoof score of the
//! neutral frame, and `flash::judge` + `flash::verdict` decide. Not passed: the active challenge follows.
//! Kiosk tablets run the same maths in the browser and report the result.

use std::collections::HashMap;

use granary_compat::crypto::tokens::RandomSource;
use granary_core::settings_store::{require_from, values, SettingError, StoreResult};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgConnection;

pub use crate::face::flash::{judge, verdict, Judgement, Outcome, Verdict, NO_FACE};

pub const ENABLED_KEY: &str = "hr.face.passive.enabled";
pub const FLASH_COUNT_KEY: &str = "hr.face.passive.flash_count";
pub const FLASH_MS_KEY: &str = "hr.face.passive.flash_ms";
pub const SETTLE_MS_KEY: &str = "hr.face.passive.settle_ms";
pub const PALETTE_KEY: &str = "hr.face.passive.palette";
pub const MIN_DELTA_KEY: &str = "hr.face.passive.flash_min_delta";
pub const MIN_CORR_KEY: &str = "hr.face.passive.flash_min_corr";
pub const SPOOF_MIN_KEY: &str = "hr.face.passive.spoof_min_real";
pub const KEYS: [&str; 8] = [
    ENABLED_KEY,
    FLASH_COUNT_KEY,
    FLASH_MS_KEY,
    SETTLE_MS_KEY,
    PALETTE_KEY,
    MIN_DELTA_KEY,
    MIN_CORR_KEY,
    SPOOF_MIN_KEY,
];
const WHERE: &str = "People & payroll \u{2192} Attendance";

/// The passive check's settings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PassiveSettings {
    pub enabled: bool,
    pub flash_count: u32,
    pub flash_ms: u32,
    pub settle_ms: u32,
    pub palette: Vec<[u8; 3]>,
    pub flash_min_delta: f64,
    pub flash_min_corr: f64,
    pub spoof_min_real: f64,
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_int(raw: &str) -> Option<i64> {
    raw.trim().parse::<i64>().ok()
}

fn parse_float(raw: &str) -> Option<f64> {
    raw.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The palette: a JSON list of at least 4 [r, g, b] triples, each 0..255.
pub fn parse_palette(raw: &str) -> Result<Vec<[u8; 3]>, String> {
    let v: Value = serde_json::from_str(raw)
        .map_err(|_| "must be a JSON list of [r, g, b] colours".to_string())?;
    let list = v
        .as_array()
        .ok_or("must be a JSON list of [r, g, b] colours")?;
    let mut out = Vec::new();
    for c in list {
        let t = c
            .as_array()
            .filter(|t| t.len() == 3)
            .ok_or("needs each colour as [r, g, b]")?;
        let mut rgb = [0u8; 3];
        for (i, x) in t.iter().enumerate() {
            rgb[i] = x
                .as_u64()
                .filter(|n| *n <= 255)
                .ok_or("needs each colour value a whole number 0 to 255")?
                as u8;
        }
        out.push(rgb);
    }
    if out.len() < 4 {
        return Err("needs at least 4 colours".into());
    }
    let mut distinct = out.clone();
    distinct.sort();
    distinct.dedup();
    if distinct.len() < 2 {
        return Err("needs at least 2 different colours".into());
    }
    Ok(out)
}

/// One value checked on its own: Err(problem) in the words of SettingError ("must be ...").
fn check_one(key: &str, raw: &str) -> Result<(), String> {
    let int_in = |lo: i64, hi: i64, what: &str| match parse_int(raw) {
        Some(n) if (lo..=hi).contains(&n) => Ok(()),
        _ => Err(format!("must be a whole number {what}")),
    };
    let float_in = |lo: f64, hi: f64, what: &str| match parse_float(raw) {
        Some(n) if n >= lo && n <= hi => Ok(()),
        _ => Err(format!("must be a number {what}")),
    };
    match key {
        ENABLED_KEY => parse_bool(raw)
            .map(|_| ())
            .ok_or_else(|| "must be true or false".to_string()),
        FLASH_COUNT_KEY => int_in(4, 6, "from 4 to 6"),
        // at least 250 ms per colour: under 3 flashes per second (WCAG 2.3.1)
        FLASH_MS_KEY => int_in(250, 2000, "of milliseconds from 250 to 2000"),
        SETTLE_MS_KEY => int_in(0, 1999, "of milliseconds from 0 to 1999"),
        PALETTE_KEY => parse_palette(raw).map(|_| ()),
        MIN_DELTA_KEY => float_in(0.5, 60.0, "from 0.5 to 60"),
        MIN_CORR_KEY => float_in(0.0, 1.0, "from 0 to 1"),
        SPOOF_MIN_KEY => float_in(0.0, 1.0, "from 0 to 1"),
        _ => Ok(()),
    }
}

/// Settings > save: None for keys not of this module; the settle time must stay below the flash time
/// (`current` holds the other values as saved).
pub fn validate_setting(
    key: &str,
    raw: &str,
    current: &HashMap<String, Option<String>>,
) -> Option<Result<(), String>> {
    if !KEYS.contains(&key) {
        return None;
    }
    if let Err(e) = check_one(key, raw) {
        return Some(Err(format!("{key} {e}")));
    }
    let other = |k: &str| {
        current
            .get(k)
            .cloned()
            .flatten()
            .and_then(|v| parse_int(&v))
    };
    let (flash, settle) = match key {
        FLASH_MS_KEY => (parse_int(raw), other(SETTLE_MS_KEY)),
        SETTLE_MS_KEY => (other(FLASH_MS_KEY), parse_int(raw)),
        _ => (None, None),
    };
    if let (Some(f), Some(s)) = (flash, settle) {
        if s >= f {
            return Some(Err(format!(
                "{SETTLE_MS_KEY} ({s} ms) must be shorter than {FLASH_MS_KEY} ({f} ms)"
            )));
        }
    }
    Some(Ok(()))
}

/// Parse all eight values; the first missing or malformed one names its key.
pub fn parse(vals: &HashMap<String, Option<String>>) -> Result<PassiveSettings, SettingError> {
    for k in KEYS {
        let raw = require_from(vals, k, WHERE)?;
        check_one(k, &raw).map_err(|e| SettingError::new(k, &e, WHERE))?;
    }
    let get = |k: &str| require_from(vals, k, WHERE).unwrap_or_default();
    let s = PassiveSettings {
        enabled: parse_bool(&get(ENABLED_KEY)).unwrap_or(false),
        flash_count: parse_int(&get(FLASH_COUNT_KEY)).unwrap_or(0) as u32,
        flash_ms: parse_int(&get(FLASH_MS_KEY)).unwrap_or(0) as u32,
        settle_ms: parse_int(&get(SETTLE_MS_KEY)).unwrap_or(0) as u32,
        palette: parse_palette(&get(PALETTE_KEY)).unwrap_or_default(),
        flash_min_delta: parse_float(&get(MIN_DELTA_KEY)).unwrap_or(0.0),
        flash_min_corr: parse_float(&get(MIN_CORR_KEY)).unwrap_or(0.0),
        spoof_min_real: parse_float(&get(SPOOF_MIN_KEY)).unwrap_or(0.0),
    };
    if s.settle_ms >= s.flash_ms {
        return Err(SettingError::new(
            SETTLE_MS_KEY,
            &format!("must be shorter than {FLASH_MS_KEY}"),
            WHERE,
        ));
    }
    Ok(s)
}

/// The settings from the database.
pub async fn load(db: &mut PgConnection) -> StoreResult<PassiveSettings> {
    let vals = values(db, &KEYS).await?;
    Ok(parse(&vals)?)
}

/// `flash_count` colours from the palette, never the same colour twice in a row.
pub fn pick_colors(rng: &mut impl RandomSource, palette: &[[u8; 3]], count: usize) -> Vec<[u8; 3]> {
    let mut out: Vec<[u8; 3]> = Vec::with_capacity(count);
    while out.len() < count {
        let c = palette[rng.below(palette.len())];
        if out.last() != Some(&c) {
            out.push(c);
        }
    }
    out
}

/// The `passive` object of a phone's challenge (`colors` set) or of the kiosk session (all settings).
pub fn challenge_json(s: &PassiveSettings, colors: &[[u8; 3]]) -> Value {
    json!({"enabled": s.enabled, "colors": colors, "flash_ms": s.flash_ms, "settle_ms": s.settle_ms})
}

pub fn kiosk_json(s: &PassiveSettings) -> Value {
    json!({
        "enabled": s.enabled, "flash_count": s.flash_count, "flash_ms": s.flash_ms, "settle_ms": s.settle_ms,
        "palette": s.palette, "flash_min_delta": s.flash_min_delta, "flash_min_corr": s.flash_min_corr,
        "spoof_min_real": s.spoof_min_real,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> HashMap<String, Option<String>> {
        [
            (ENABLED_KEY, "true"),
            (FLASH_COUNT_KEY, "5"),
            (FLASH_MS_KEY, "400"),
            (SETTLE_MS_KEY, "340"),
            (
                PALETTE_KEY,
                "[[255,120,120],[120,255,140],[120,150,255],[255,230,120],[220,130,255],[120,240,240]]",
            ),
            (MIN_DELTA_KEY, "1.5"),
            (MIN_CORR_KEY, "0.6"),
            (SPOOF_MIN_KEY, "0.7"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), Some(v.to_string())))
        .collect()
    }

    struct Seq(Vec<usize>, usize);
    impl RandomSource for Seq {
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            vec![0; n]
        }
        fn below(&mut self, n: usize) -> usize {
            let v = self.0[self.1 % self.0.len()] % n;
            self.1 += 1;
            v
        }
    }

    #[test]
    fn parses_the_defaults_and_names_bad_keys() {
        let s = parse(&defaults()).unwrap();
        assert!(s.enabled);
        assert_eq!((s.flash_count, s.flash_ms, s.settle_ms), (5, 400, 340));
        assert_eq!(s.palette.len(), 6);
        let mut bad = defaults();
        bad.insert(FLASH_COUNT_KEY.into(), Some("9".into()));
        assert!(parse(&bad).unwrap_err().detail.contains(FLASH_COUNT_KEY));
        let mut bad = defaults();
        bad.remove(PALETTE_KEY);
        assert!(parse(&bad).unwrap_err().detail.contains("is not set"));
        let mut bad = defaults();
        bad.insert(SETTLE_MS_KEY.into(), Some("400".into()));
        assert!(parse(&bad).unwrap_err().detail.contains(SETTLE_MS_KEY));
    }

    #[test]
    fn validates_single_values_and_the_settle_time() {
        let cur = defaults();
        assert_eq!(validate_setting("hr.other", "x", &cur), None);
        assert_eq!(validate_setting(FLASH_MS_KEY, "350", &cur), Some(Ok(())));
        // shorter than the saved settle time (340)
        assert!(validate_setting(FLASH_MS_KEY, "300", &cur)
            .unwrap()
            .is_err());
        assert!(validate_setting(FLASH_MS_KEY, "200", &cur)
            .unwrap()
            .is_err());
        assert!(validate_setting(FLASH_MS_KEY, "150", &cur)
            .unwrap()
            .is_err());
        assert!(validate_setting(SETTLE_MS_KEY, "400", &cur)
            .unwrap()
            .is_err());
        assert_eq!(validate_setting(SETTLE_MS_KEY, "300", &cur), Some(Ok(())));
        assert!(validate_setting(PALETTE_KEY, "[[1,2,3]]", &cur)
            .unwrap()
            .is_err());
        assert!(
            validate_setting(PALETTE_KEY, "[[1,2,3],[1,2,3],[1,2,3],[1,2,300]]", &cur)
                .unwrap()
                .is_err()
        );
        assert!(validate_setting(MIN_CORR_KEY, "1.5", &cur)
            .unwrap()
            .is_err());
        assert!(validate_setting(ENABLED_KEY, "maybe", &cur)
            .unwrap()
            .is_err());
        assert_eq!(validate_setting(ENABLED_KEY, "false", &cur), Some(Ok(())));
    }

    #[test]
    fn colours_never_repeat_back_to_back() {
        let pal = [[1, 1, 1], [2, 2, 2], [3, 3, 3], [4, 4, 4]];
        let c = pick_colors(&mut Seq(vec![0, 0, 0, 1, 1, 2], 0), &pal, 4);
        assert_eq!(c, vec![[1, 1, 1], [2, 2, 2], [3, 3, 3], [1, 1, 1]]);
        assert!(c.windows(2).all(|w| w[0] != w[1]));
    }
}
