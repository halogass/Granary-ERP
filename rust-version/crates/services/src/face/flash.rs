//! The screen-flash judgement of the passive liveness check (Release 2 of the face clock plan, 6 Nov 2026;
//! lag + chromaticity revision the same evening, migration 0167).
//!
//! Pure std on purpose: the face sidecar's contract test includes this file by path, and the kiosk's
//! TypeScript port (frontend) is checked against the same vectors
//! (compat-tests/golden/face/passive_flash.json).
//!
//! Inputs: the neutral frame's skin-region mean N (r, g, b), the flash frames' means F_i and the screen colours
//! C_i (i = 0..k-1).
//! 0. Chromaticity: every N, F_i and C_i becomes x / (r + g + b) * 255 per channel (0 when the sum is 0), so an
//!    auto-exposure change in brightness does not swamp the colour signal.
//! 1. d_i = F_i - N, e_i = C_i (per channel, chromaticity units).
//! 2. Centre over i per channel: d'_i = d_i - mean_i(d), e'_i = e_i - mean_i(e).
//! 3. delta = mean over i and channel of |d'_i|; corr = Pearson(concat(d'), concat(e')) over the 3k values
//!    (0 when either side's sum of squares is below ZERO_VARIANCE).
//! 4. Lag: steps 1-3 at lag 0 (F_i with C_i) and, with 3 or more flashes, at lag 1 (F_i with C_{i-1}, the first
//!    frame dropped: Android front cameras deliver frames 100-250 ms late); the higher corr wins (ties: lag 0).
//! 5. delta < min_delta -> inconclusive; else corr >= min_corr -> pass; else fail.
//!
//! A live face reflects the screen's colour, so its change follows the colour sequence; a printed photo or a
//! replaying screen responds weakly or uniformly.

/// A sum of squares below this counts as zero variance (corr 0).
pub const ZERO_VARIANCE: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Inconclusive,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Inconclusive => "inconclusive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Judgement {
    pub outcome: Outcome,
    pub corr: f64,
    pub delta: f64,
    /// 0: flash frame i judged against colour i; 1: against colour i-1 (a late camera).
    pub lag: u8,
}

/// Pearson correlation of two equally long series; 0 when either has (near) zero variance.
pub fn pearson(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len().min(y.len());
    if n == 0 {
        return 0.0;
    }
    let mx = x[..n].iter().sum::<f64>() / n as f64;
    let my = y[..n].iter().sum::<f64>() / n as f64;
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (a, b) = (x[i] - mx, y[i] - my);
        sxy += a * b;
        sxx += a * a;
        syy += b * b;
    }
    if sxx < ZERO_VARIANCE || syy < ZERO_VARIANCE {
        return 0.0;
    }
    sxy / (sxx * syy).sqrt()
}

/// Per channel, each value minus the channel's mean over the frames; flattened frame by frame (r, g, b, r, ...).
fn centred(v: &[[f64; 3]]) -> Vec<f64> {
    let k = v.len() as f64;
    let mut mean = [0.0; 3];
    for x in v {
        for c in 0..3 {
            mean[c] += x[c];
        }
    }
    for m in mean.iter_mut() {
        *m /= k;
    }
    v.iter()
        .flat_map(|x| [x[0] - mean[0], x[1] - mean[1], x[2] - mean[2]])
        .collect()
}

/// x / (r + g + b) * 255 per channel; 0 when the sum is 0.
pub fn chroma(m: [f64; 3]) -> [f64; 3] {
    let s = m[0] + m[1] + m[2];
    if s <= 0.0 {
        return [0.0; 3];
    }
    [m[0] / s * 255.0, m[1] / s * 255.0, m[2] / s * 255.0]
}

/// (corr, delta) of flash frames against colours, pair by pair, in chromaticity units.
fn series(neutral: [f64; 3], flashes: &[[f64; 3]], colors: &[[f64; 3]]) -> (f64, f64) {
    let n = chroma(neutral);
    let d: Vec<[f64; 3]> = flashes
        .iter()
        .map(|f| {
            let f = chroma(*f);
            [f[0] - n[0], f[1] - n[1], f[2] - n[2]]
        })
        .collect();
    let e: Vec<[f64; 3]> = colors.iter().map(|c| chroma(*c)).collect();
    let dc = centred(&d);
    let ec = centred(&e);
    let delta = dc.iter().map(|v| v.abs()).sum::<f64>() / dc.len() as f64;
    (pearson(&dc, &ec), delta)
}

/// The flash judgement. `flashes` and `colors` must be equally long and non-empty, else inconclusive.
pub fn judge(
    neutral: [f64; 3],
    flashes: &[[f64; 3]],
    colors: &[[f64; 3]],
    min_delta: f64,
    min_corr: f64,
) -> Judgement {
    let k = flashes.len();
    if k == 0 || k != colors.len() {
        return Judgement {
            outcome: Outcome::Inconclusive,
            corr: 0.0,
            delta: 0.0,
            lag: 0,
        };
    }
    let (mut corr, mut delta) = series(neutral, flashes, colors);
    let mut lag = 0;
    if k >= 3 {
        let (c1, d1) = series(neutral, &flashes[1..], &colors[..k - 1]);
        if c1 > corr {
            (corr, delta, lag) = (c1, d1, 1);
        }
    }
    let outcome = if delta < min_delta {
        Outcome::Inconclusive
    } else if corr >= min_corr {
        Outcome::Pass
    } else {
        Outcome::Fail
    };
    Judgement {
        outcome,
        corr,
        delta,
        lag,
    }
}

/// The passive verdict: flash pass AND spoof real >= spoof_min_real -> passed; flash inconclusive ->
/// inconclusive; otherwise failed. `reason` is what the phone is told when it must do the active check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Passed,
    Inconclusive { reason: &'static str },
    Failed { reason: &'static str },
}

impl Verdict {
    pub fn passed(&self) -> bool {
        matches!(self, Verdict::Passed)
    }
    /// "flash_inconclusive" | "flash_failed" | "spoof" | "no_face" (None when passed).
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Verdict::Passed => None,
            Verdict::Inconclusive { reason } | Verdict::Failed { reason } => Some(reason),
        }
    }
    /// The face_check_attempts outcome: passive_inconclusive | passive_failed (None when passed).
    pub fn attempt_outcome(&self) -> Option<&'static str> {
        match self {
            Verdict::Passed => None,
            Verdict::Inconclusive { .. } => Some("passive_inconclusive"),
            Verdict::Failed { .. } => Some("passive_failed"),
        }
    }
}

/// Combine the flash judgement with the anti-spoof score.
pub fn verdict(flash: Outcome, spoof_real: f64, spoof_min_real: f64) -> Verdict {
    match flash {
        Outcome::Pass if spoof_real >= spoof_min_real => Verdict::Passed,
        Outcome::Pass => Verdict::Failed { reason: "spoof" },
        Outcome::Inconclusive => Verdict::Inconclusive {
            reason: "flash_inconclusive",
        },
        Outcome::Fail => Verdict::Failed {
            reason: "flash_failed",
        },
    }
}

/// A face missing from a frame: inconclusive (no evidence either way), the active check follows.
pub const NO_FACE: Verdict = Verdict::Inconclusive { reason: "no_face" };
