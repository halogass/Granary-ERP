//! Passive liveness pieces of the sidecar (Release 2 of the face clock plan, 6 Nov 2026).
//!
//! - The MiniFASNet anti-spoof score (models/antispoof/README.md): two models on 80 x 80 crops of the face box
//!   at 2.7x and 4.0x, BGR 0..255 CHW; softmax per model; real = mean of the two "real" (index 1) probabilities.
//! - The skin-region mean colour of a frame for the screen-flash check: the rectangle between landmark 3 and
//!   landmark 13 across, landmark 28 (nose bridge) and landmark 33 (nose bottom) down; edges rounded to the
//!   nearest pixel (Math.round), inclusive, clipped to the image (the kiosk's TypeScript does the same).

use std::path::Path;
use std::sync::Mutex;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;

use crate::decode::Rgb;
use crate::pipeline::{BoxF, EngineError, Pt};

pub const MODEL_V2: &str = "minifasnet_v2_2.7.onnx";
pub const MODEL_V1SE: &str = "minifasnet_v1se_4.0.onnx";
pub const MODELS: [&str; 2] = [MODEL_V2, MODEL_V1SE];
const SIZE: usize = 80;

/// The two anti-spoof networks. `Session::run` takes `&mut self`, so each sits behind a mutex.
pub struct AntiSpoof {
    v2: Mutex<Session>,
    v1se: Mutex<Session>,
}

impl AntiSpoof {
    /// Load both models from `dir` (models/antispoof, or FACE_ANTISPOOF_DIR in the image).
    pub fn load(dir: &Path) -> Result<AntiSpoof, EngineError> {
        let env = crate::pipeline::ort_env()?;
        let open = |name: &str| -> Result<Mutex<Session>, EngineError> {
            let path = dir.join(name);
            if !path.is_file() {
                return Err(EngineError(format!(
                    "model file not found: {}",
                    path.display()
                )));
            }
            let s = Session::builder(env)?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(ort::Error::from)?
                .commit_from_file(&path)?;
            Ok(Mutex::new(s))
        };
        Ok(AntiSpoof {
            v2: open(MODEL_V2)?,
            v1se: open(MODEL_V1SE)?,
        })
    }

    fn real_prob(session: &Mutex<Session>, input: Vec<f32>) -> Result<f64, EngineError> {
        let tensor = Tensor::from_array(([1usize, 3, SIZE, SIZE], input))?;
        let mut s = session
            .lock()
            .map_err(|_| EngineError("model session poisoned".into()))?;
        let outputs = s.run(ort::inputs![tensor])?;
        let Some((_, out)) = outputs.iter().next() else {
            return Err(EngineError("anti-spoof model gave no output".into()));
        };
        let (_, logits) = out.try_extract_tensor::<f32>()?;
        if logits.len() != 3 {
            return Err(EngineError(format!(
                "anti-spoof model gave {} logits, expected 3",
                logits.len()
            )));
        }
        Ok(softmax(logits)[1])
    }

    /// The real-face score (0..1) of the face `b` in `img`.
    pub fn score(&self, img: &Rgb, b: BoxF) -> Result<f64, EngineError> {
        let p2 = Self::real_prob(&self.v2, crop_input(img, b, 2.7))?;
        let p4 = Self::real_prob(&self.v1se, crop_input(img, b, 4.0))?;
        Ok((p2 + p4) / 2.0)
    }
}

/// Softmax in float64.
pub fn softmax(logits: &[f32]) -> Vec<f64> {
    let max = logits
        .iter()
        .fold(f64::NEG_INFINITY, |m, &v| m.max(v as f64));
    let e: Vec<f64> = logits.iter().map(|&v| (v as f64 - max).exp()).collect();
    let sum: f64 = e.iter().sum();
    e.iter().map(|v| v / sum).collect()
}

/// generate_patches.py `_get_new_box`: the face box scaled about its centre (scale capped so the box fits),
/// shifted inside the image; inclusive integer corners (left, top, right, bottom).
pub fn new_box(src_w: usize, src_h: usize, b: BoxF, scale: f64) -> (usize, usize, usize, usize) {
    let (sw, sh) = (src_w as f64, src_h as f64);
    let (bw, bh) = (b.width.max(1.0), b.height.max(1.0));
    let scale = scale.min((sh - 1.0) / bh).min((sw - 1.0) / bw);
    let (nw, nh) = (bw * scale, bh * scale);
    let (cx, cy) = (b.x + bw / 2.0, b.y + bh / 2.0);
    let (mut l, mut t) = (cx - nw / 2.0, cy - nh / 2.0);
    let (mut r, mut btm) = (cx + nw / 2.0, cy + nh / 2.0);
    if l < 0.0 {
        r -= l;
        l = 0.0;
    }
    if t < 0.0 {
        btm -= t;
        t = 0.0;
    }
    if r > sw - 1.0 {
        l -= r - sw + 1.0;
        r = sw - 1.0;
    }
    if btm > sh - 1.0 {
        t -= btm - sh + 1.0;
        btm = sh - 1.0;
    }
    // Python int(): truncation toward zero; clamp what float error might push outside
    let clamp = |v: f64, max: usize| (v.trunc().max(0.0) as usize).min(max);
    (
        clamp(l, src_w - 1),
        clamp(t, src_h - 1),
        clamp(r, src_w - 1),
        clamp(btm, src_h - 1),
    )
}

/// The model input for one scale: the inclusive crop resized to 80 x 80 with OpenCV's INTER_LINEAR
/// (half-pixel centres, edge clamped), BGR, 0..255, CHW.
pub fn crop_input(img: &Rgb, b: BoxF, scale: f64) -> Vec<f32> {
    let (l, t, r, btm) = new_box(img.w, img.h, b, scale);
    let (cw, ch) = (r - l + 1, btm - t + 1);
    let px = |y: usize, x: usize, c: usize| img.data[((t + y) * img.w + (l + x)) * 3 + c] as f64;
    let axis = |out: usize, inp: usize| -> Vec<(usize, usize, f64)> {
        let s = inp as f64 / out as f64;
        (0..out)
            .map(|i| {
                let f = ((i as f64 + 0.5) * s - 0.5).max(0.0);
                let i0 = (f.floor() as usize).min(inp - 1);
                let i1 = (i0 + 1).min(inp - 1);
                (i0, i1, f - i0 as f64)
            })
            .collect()
    };
    let xs = axis(SIZE, cw);
    let ys = axis(SIZE, ch);
    let n = SIZE * SIZE;
    let mut out = vec![0f32; 3 * n];
    for (oy, &(y0, y1, fy)) in ys.iter().enumerate() {
        for (ox, &(x0, x1, fx)) in xs.iter().enumerate() {
            for c in 0..3 {
                let top = px(y0, x0, c) * (1.0 - fx) + px(y0, x1, c) * fx;
                let bottom = px(y1, x0, c) * (1.0 - fx) + px(y1, x1, c) * fx;
                let v = top * (1.0 - fy) + bottom * fy;
                // RGB in, BGR out: channel 0 of the model is blue
                out[(2 - c) * n + oy * SIZE + ox] = v as f32;
            }
        }
    }
    out
}

/// The mean R, G, B of the cheeks-and-nose rectangle (landmarks 3, 13, 28, 33); None when it is empty.
pub fn region_mean(img: &Rgb, lm: &[Pt]) -> Option<[f64; 3]> {
    if lm.len() < 68 {
        return None;
    }
    let (xa, xb) = (lm[3].x.min(lm[13].x), lm[3].x.max(lm[13].x));
    let (ya, yb) = (lm[28].y.min(lm[33].y), lm[28].y.max(lm[33].y));
    // Math.round, then inclusive edges clipped to the image: columns x0..x1, rows y0..y1 (half-open here)
    let round = |v: f64| (v + 0.5).floor();
    let span = |a: f64, b: f64, size: usize| -> Option<(usize, usize)> {
        let lo = round(a).max(0.0);
        let hi = (round(b) + 1.0).min(size as f64);
        (lo < hi).then_some((lo as usize, hi as usize))
    };
    let (x0, x1) = span(xa, xb, img.w)?;
    let (y0, y1) = span(ya, yb, img.h)?;
    let mut sum = [0f64; 3];
    for y in y0..y1 {
        let row = &img.data[(y * img.w + x0) * 3..(y * img.w + x1) * 3];
        for p in row.as_chunks::<3>().0 {
            for c in 0..3 {
                sum[c] += p[c] as f64;
            }
        }
    }
    let n = ((x1 - x0) * (y1 - y0)) as f64;
    Some([sum[0] / n, sum[1] / n, sum[2] / n])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(x: f64, y: f64, w: f64, h: f64) -> BoxF {
        BoxF {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn new_box_scales_caps_and_shifts_inside() {
        // centred box, room to grow: 2x about the centre
        assert_eq!(
            new_box(200, 200, bx(80.0, 80.0, 40.0, 40.0), 2.0),
            (60, 60, 140, 140)
        );
        // at the left/top edge: shifted right/down
        assert_eq!(
            new_box(200, 200, bx(0.0, 0.0, 40.0, 40.0), 2.0),
            (0, 0, 80, 80)
        );
        // at the right/bottom edge: shifted left/up
        assert_eq!(
            new_box(200, 200, bx(160.0, 160.0, 40.0, 40.0), 2.0),
            (119, 119, 199, 199)
        );
        // scale capped by the image: the whole image
        assert_eq!(
            new_box(100, 100, bx(30.0, 30.0, 40.0, 40.0), 4.0),
            (0, 0, 99, 99)
        );
    }

    #[test]
    fn crop_is_bgr_chw_and_bilinear() {
        // a 4x4 image whose red channel is x*10, green y*10, blue 7
        let mut data = Vec::new();
        for y in 0..4u8 {
            for x in 0..4u8 {
                data.extend([x * 10, y * 10, 7]);
            }
        }
        let img = Rgb { h: 4, w: 4, data };
        let out = crop_input(&img, bx(0.0, 0.0, 4.0, 4.0), 1.0);
        let n = SIZE * SIZE;
        // blue first, constant
        assert!(out[..n].iter().all(|&v| v == 7.0));
        // red last: increases along x, clamped at the edges (0 at the left, 30 at the right)
        let red_row: Vec<f32> = out[2 * n..2 * n + SIZE].to_vec();
        assert_eq!(red_row[0], 0.0);
        assert_eq!(red_row[SIZE - 1], 30.0);
        assert!(red_row.windows(2).all(|w| w[0] <= w[1]));
        // green (middle) increases along y
        assert_eq!(out[n], 0.0);
        assert_eq!(out[n + (SIZE - 1) * SIZE], 30.0);
    }

    #[test]
    fn region_mean_of_the_cheek_rectangle() {
        let mut data = vec![0u8; 10 * 10 * 3];
        for y in 2..6 {
            for x in 3..7 {
                let i = (y * 10 + x) * 3;
                data[i..i + 3].copy_from_slice(&[200, 100, 50]);
            }
        }
        let img = Rgb { h: 10, w: 10, data };
        let mut lm = vec![Pt { x: 0.0, y: 0.0 }; 68];
        lm[3] = Pt { x: 3.2, y: 4.0 };
        lm[13] = Pt { x: 6.4, y: 4.0 };
        lm[28] = Pt { x: 5.0, y: 2.0 };
        lm[33] = Pt { x: 5.0, y: 5.4 };
        // rounded, inclusive: columns 3..=6, rows 2..=5, exactly the coloured block
        assert_eq!(region_mean(&img, &lm), Some([200.0, 100.0, 50.0]));
        // one column is still a region; outside the image is none
        lm[13] = Pt { x: 3.0, y: 4.0 };
        lm[3] = Pt { x: 3.0, y: 4.0 };
        assert_eq!(region_mean(&img, &lm), Some([200.0, 100.0, 50.0]));
        lm[13] = Pt { x: -9.0, y: 4.0 };
        lm[3] = Pt { x: -7.0, y: 4.0 };
        assert_eq!(region_mean(&img, &lm), None);
    }

    #[test]
    fn softmax_sums_to_one() {
        let p = softmax(&[1.0, 2.0, 0.5]);
        assert!((p.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(p[1] > p[0] && p[0] > p[2]);
    }
}
