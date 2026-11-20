//! face-api.js 1.7.15's `detectAllFaces(tensor, SsdMobilenetv1Options({minConfidence: 0.5, maxResults: 5}))
//! .withFaceLandmarks().withFaceDescriptors()` on ONNX Runtime, step for step.
//!
//! The three networks are ONNX graphs converted from face-api's own weights (models/convert.py); what
//! face-api does around them is reproduced here with the same arithmetic: tensor ops (padToSquare, the
//! legacy `resizeBilinear`, normalisation, landmark post-processing) in float32 as TensorFlow does them,
//! box and point geometry (Box, Point, FaceLandmarks, alignDlib) in float64 as JavaScript does it.

use std::path::Path;
use std::sync::Mutex;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;

use crate::decode::Rgb;
use crate::jsnum::math_round;

/// A float32 image tensor, height x width x 3, row-major (a TensorFlow HWC tensor).
#[derive(Debug, Clone)]
pub struct Img {
    pub h: usize,
    pub w: usize,
    pub data: Vec<f32>,
}

/// `tf.slice3d(img, [y, x, 0], [h, w, 3])` of the decoded image, as float32.
pub fn crop(img: &Rgb, x: usize, y: usize, w: usize, h: usize) -> Img {
    let mut data = Vec::with_capacity(w * h * 3);
    for row in y..y + h {
        let start = (row * img.w + x) * 3;
        data.extend(img.data[start..start + w * 3].iter().map(|&v| v as f32));
    }
    Img { h, w, data }
}

/// ops/padToSquare geometry: the padded size and the image's offset in it. Zero padding of the shorter
/// side; `center` splits it (the larger half, `Math.round(diff * 0.5)`, after the image), else all of it
/// goes after (bottom / right).
fn pad_geometry(h: usize, w: usize, center: bool) -> (usize, usize, usize) {
    if h == w {
        return (h, 0, 0);
    }
    let diff = h.abs_diff(w);
    let after = math_round(diff as f64 * if center { 0.5 } else { 1.0 }) as usize;
    let before = if center { diff - after } else { 0 };
    if h > w {
        (h, 0, before) // padding on the width axis
    } else {
        (w, before, 0)
    }
}

/// TensorFlow's ResizeBilinear interpolation weights with align_corners=false, half_pixel_centers=false
/// (the legacy scaler), float32, as resize_bilinear_op.cc computes them.
struct Interp {
    lower: usize,
    upper: usize,
    lerp: f32,
}

fn interp_weights(out: usize, inp: usize) -> Vec<Interp> {
    let scale = inp as f32 / out as f32;
    (0..out)
        .map(|i| {
            let v = i as f32 * scale;
            let f = v.floor();
            Interp {
                lower: (f as i64).max(0) as usize,
                upper: (v.ceil() as i64).min(inp as i64 - 1) as usize,
                lerp: v - f,
            }
        })
        .collect()
}

/// NetInput.toBatchTensor(size, center) for one h x w image read through `get(y, x, c)`: padToSquare
/// (zeros), then `tf.image.resizeBilinear(.., [size, size], false, false)` unless already `size`. The
/// padded image is never materialised (a 6528 px photo would need 500 MB); the values are the same.
pub fn to_batch(
    h: usize,
    w: usize,
    get: impl Fn(usize, usize, usize) -> f32,
    size: usize,
    center: bool,
) -> Img {
    let (sq, by, bx) = pad_geometry(h, w, center);
    let padded = |y: usize, x: usize, c: usize| -> f32 {
        if y < by || x < bx || y - by >= h || x - bx >= w {
            0.0
        } else {
            get(y - by, x - bx, c)
        }
    };
    let mut data = vec![0f32; size * size * 3];
    if sq == size {
        for y in 0..size {
            for x in 0..size {
                for c in 0..3 {
                    data[(y * size + x) * 3 + c] = padded(y, x, c);
                }
            }
        }
    } else {
        let ys = interp_weights(size, sq);
        let xs = interp_weights(size, sq);
        for (oy, yi) in ys.iter().enumerate() {
            for (ox, xi) in xs.iter().enumerate() {
                for c in 0..3 {
                    let tl = padded(yi.lower, xi.lower, c);
                    let tr = padded(yi.lower, xi.upper, c);
                    let bl = padded(yi.upper, xi.lower, c);
                    let br = padded(yi.upper, xi.upper, c);
                    let top = tl + (tr - tl) * xi.lerp;
                    let bottom = bl + (br - bl) * xi.lerp;
                    data[(oy * size + ox) * 3 + c] = top + (bottom - top) * yi.lerp;
                }
            }
        }
    }
    Img {
        h: size,
        w: size,
        data,
    }
}

impl Img {
    fn batch(&self, size: usize, center: bool) -> Img {
        to_batch(
            self.h,
            self.w,
            |y, x, c| self.data[(y * self.w + x) * 3 + c],
            size,
            center,
        )
    }
}

/// HWC values -> NCHW after a per-value transform (the normalisation), for the ONNX input.
fn nchw(img: &Img, f: impl Fn(f32, usize) -> f32) -> Vec<f32> {
    let n = img.h * img.w;
    let mut out = vec![0f32; n * 3];
    for i in 0..n {
        for c in 0..3 {
            out[c * n + i] = f(img.data[i * 3 + c], c);
        }
    }
    out
}

const MEAN_RGB: [f32; 3] = [122.782, 117.001, 104.298];

/// ops/normalize then `.div(255)`: `(x - mean) / 255` in float32.
fn normalize_mean(img: &Img) -> Vec<f32> {
    nchw(img, |v, c| (v - MEAN_RGB[c]) / 255.0)
}

// ─── Geometry (JavaScript doubles) ────────────────────────────────────────────

/// classes/Box: x, y, width, height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxF {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BoxF {
    fn rescale(&self, sx: f64, sy: f64) -> BoxF {
        BoxF {
            x: self.x * sx,
            y: self.y * sy,
            width: self.width * sx,
            height: self.height * sy,
        }
    }

    /// Box.clipAtImageBorders(imgWidth, imgHeight): clip, then floor all four values.
    fn clip_at_image_borders(&self, img_w: f64, img_h: f64) -> BoxF {
        let cx = self.x.max(0.0);
        let cy = self.y.max(0.0);
        let new_w = (self.x + self.width) - cx;
        let new_h = (self.y + self.height) - cy;
        let cw = new_w.min(img_w - cx);
        let ch = new_h.min(img_h - cy);
        BoxF {
            x: cx.floor(),
            y: cy.floor(),
            width: cw.floor(),
            height: ch.floor(),
        }
    }
}

/// classes/Point arithmetic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    fn add(self, p: Pt) -> Pt {
        Pt {
            x: self.x + p.x,
            y: self.y + p.y,
        }
    }
    fn sub(self, p: Pt) -> Pt {
        Pt {
            x: self.x - p.x,
            y: self.y - p.y,
        }
    }
    fn mul(self, p: Pt) -> Pt {
        Pt {
            x: self.x * p.x,
            y: self.y * p.y,
        }
    }
    fn div(self, p: Pt) -> Pt {
        Pt {
            x: self.x / p.x,
            y: self.y / p.y,
        }
    }
    fn magnitude(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

/// utils getCenterPoint: the running sum from (0, 0), divided by (n, n).
fn center_point(pts: &[Pt]) -> Pt {
    let n = pts.len() as f64;
    pts.iter()
        .fold(Pt { x: 0.0, y: 0.0 }, |s, p| s.add(*p))
        .div(Pt { x: n, y: n })
}

/// One face: the detection (score, box in image pixels), its 68 landmarks and the 128-d descriptor.
#[derive(Debug, Clone)]
pub struct Face {
    pub score: f32,
    pub detection: BoxF,
    pub landmarks: Vec<Pt>,
    pub descriptor: Vec<f32>,
}

/// A request-fatal error (face-api throws; the service answers 422).
#[derive(Debug)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<ort::Error> for EngineError {
    fn from(e: ort::Error) -> Self {
        EngineError(e.to_string())
    }
}

// ─── Engine ───────────────────────────────────────────────────────────────────

pub const SSD_MODEL: &str = "ssd_mobilenetv1.onnx";
pub const LANDMARK_MODEL: &str = "face_landmark_68.onnx";
pub const RECOGNITION_MODEL: &str = "face_recognition.onnx";

/// ONNX Runtime allows one environment per process, ever: the face networks and the anti-spoof models share
/// this one (built on first use, kept for the life of the process).
pub fn ort_env() -> Result<&'static ort::environment::Environment, EngineError> {
    static ENV: std::sync::OnceLock<ort::environment::Environment> = std::sync::OnceLock::new();
    static BUILD: Mutex<()> = Mutex::new(());
    if let Some(e) = ENV.get() {
        return Ok(e);
    }
    let _guard = BUILD
        .lock()
        .map_err(|_| EngineError("ort environment lock poisoned".into()))?;
    if let Some(e) = ENV.get() {
        return Ok(e);
    }
    let env = ort::init()
        .with_name("granary-face")
        .build()
        .map_err(EngineError::from)?;
    Ok(ENV.get_or_init(|| env))
}

/// The three networks. `Session::run` takes `&mut self`, so each sits behind a mutex.
pub struct Engine {
    ssd: Mutex<Session>,
    landmark: Mutex<Session>,
    recognition: Mutex<Session>,
}

const MIN_CONFIDENCE: f64 = 0.5;
const MAX_RESULTS: usize = 5;
const IOU_THRESHOLD: f64 = 0.5;

impl Engine {
    /// Load the ONNX models from `dir` (ssd_mobilenetv1.onnx, face_landmark_68.onnx, face_recognition.onnx).
    pub fn load(dir: &Path) -> Result<Engine, EngineError> {
        let env = ort_env()?;
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
        Ok(Engine {
            ssd: open(SSD_MODEL)?,
            landmark: open(LANDMARK_MODEL)?,
            recognition: open(RECOGNITION_MODEL)?,
        })
    }

    fn run(
        session: &Mutex<Session>,
        size: usize,
        input: Vec<f32>,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        let tensor = Tensor::from_array(([1usize, 3, size, size], input))?;
        let mut s = session
            .lock()
            .map_err(|_| EngineError("model session poisoned".into()))?;
        let outputs = s.run(ort::inputs![tensor])?;
        let mut res = Vec::new();
        for (_, v) in outputs.iter() {
            let (_, data) = v.try_extract_tensor::<f32>()?;
            res.push(data.to_vec());
        }
        Ok(res)
    }

    /// SsdMobilenetv1.locateFaces: (score, box in image pixels) per kept detection.
    pub fn locate_faces(&self, img: &Rgb) -> Result<Vec<(f32, BoxF)>, EngineError> {
        let (img_h, img_w) = (img.h as f64, img.w as f64);
        let batch = to_batch(
            img.h,
            img.w,
            |y, x, c| img.data[(y * img.w + x) * 3 + c] as f32,
            512,
            false,
        );
        let input = nchw(&batch, |v, _| v / 127.5 - 1.0);
        let out = Self::run(&self.ssd, 512, input)?;
        let (boxes, scores) = (&out[0], &out[1]);
        let n = scores.len();
        let b = |i: usize, k: usize| boxes[i * 4 + k] as f64;

        // ssdMobilenetv1/nonMaxSuppression
        let iou = |i: usize, j: usize| -> f64 {
            let (ymin_i, xmin_i) = (b(i, 0).min(b(i, 2)), b(i, 1).min(b(i, 3)));
            let (ymax_i, xmax_i) = (b(i, 0).max(b(i, 2)), b(i, 1).max(b(i, 3)));
            let (ymin_j, xmin_j) = (b(j, 0).min(b(j, 2)), b(j, 1).min(b(j, 3)));
            let (ymax_j, xmax_j) = (b(j, 0).max(b(j, 2)), b(j, 1).max(b(j, 3)));
            let area_i = (ymax_i - ymin_i) * (xmax_i - xmin_i);
            let area_j = (ymax_j - ymin_j) * (xmax_j - xmin_j);
            if area_i <= 0.0 || area_j <= 0.0 {
                return 0.0;
            }
            let iy0 = ymin_i.max(ymin_j);
            let ix0 = xmin_i.max(xmin_j);
            let iy1 = ymax_i.min(ymax_j);
            let ix1 = xmax_i.min(xmax_j);
            let inter = (iy1 - iy0).max(0.0) * (ix1 - ix0).max(0.0);
            inter / (area_i + area_j - inter)
        };
        let output_size = MAX_RESULTS.min(n);
        let mut candidates: Vec<(f64, usize)> = (0..n)
            .map(|i| (scores[i] as f64, i))
            .filter(|c| c.0 > MIN_CONFIDENCE)
            .collect();
        // Array.prototype.sort is stable
        candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut selected: Vec<usize> = Vec::new();
        for (score, idx) in candidates {
            if selected.len() >= output_size {
                continue;
            }
            let mut s = score;
            for &j in selected.iter().rev() {
                let v = iou(idx, j);
                if v == 0.0 {
                    continue;
                }
                s *= if v <= IOU_THRESHOLD { 1.0 } else { 0.0 };
                if s <= MIN_CONFIDENCE {
                    break;
                }
            }
            if s == score {
                selected.push(idx);
            }
        }

        // computeReshapedDimensions and the padding factors
        let scale = 512.0 / img_h.max(img_w);
        let rw = math_round(img_w * scale);
        let rh = math_round(img_h * scale);
        let pad_x = 512.0 / rw;
        let pad_y = 512.0 / rh;
        let mut res = Vec::new();
        for idx in selected {
            let top = b(idx, 0).max(0.0) * pad_y;
            let bottom = b(idx, 2).min(1.0) * pad_y;
            let left = b(idx, 1).max(0.0) * pad_x;
            let right = b(idx, 3).min(1.0) * pad_x;
            let (w, h) = (right - left, bottom - top);
            if w < 0.0 || h < 0.0 {
                // new Rect(...) refuses negative dimensions
                return Err(EngineError(format!(
                    "Box.constructor - width ({w}) and height ({h}) must be positive numbers"
                )));
            }
            let rel = BoxF {
                x: left,
                y: top,
                width: w,
                height: h,
            };
            res.push((scores[idx], rel.rescale(img_w, img_h)));
        }
        Ok(res)
    }

    /// dom/extractFaceTensors for one box: the floored, clipped crop (None when empty).
    fn face_crop(img: &Rgb, b: BoxF) -> Option<Img> {
        let c = b.clip_at_image_borders(img.w as f64, img.h as f64);
        if c.width > 0.0 && c.height > 0.0 {
            Some(crop(
                img,
                c.x as usize,
                c.y as usize,
                c.width as usize,
                c.height as usize,
            ))
        } else {
            None
        }
    }

    /// FaceLandmark68Net.detectLandmarks on a face crop: positions relative to the crop, as float32.
    fn landmarks_rel(&self, crop: &Img) -> Result<Vec<(f32, f32)>, EngineError> {
        let batch = crop.batch(112, true);
        let out = Self::run(&self.landmark, 112, normalize_mean(&batch))?;
        let o = &out[0];
        // FaceLandmark68NetBase.postProcess
        let (cw, ch) = (crop.w as f64, crop.h as f64);
        let scale = 112.0 / ch.max(cw);
        let (width, height) = (cw * scale, ch * scale);
        let pad_x = if width < height {
            (width - height).abs() / 2.0
        } else {
            0.0
        };
        let pad_y = if height < width {
            (width - height).abs() / 2.0
        } else {
            0.0
        };
        let (px, py, wf, hf) = (pad_x as f32, pad_y as f32, width as f32, height as f32);
        Ok((0..68)
            .map(|i| {
                let x = (o[2 * i] * 112f32 - px) / wf;
                let y = (o[2 * i + 1] * 112f32 - py) / hf;
                (x, y)
            })
            .collect())
    }

    /// FaceRecognitionNet.computeFaceDescriptor on a face crop.
    fn descriptor(&self, crop: &Img) -> Result<Vec<f32>, EngineError> {
        let batch = crop.batch(150, true);
        let out = Self::run(&self.recognition, 150, normalize_mean(&batch))?;
        Ok(out[0].clone())
    }

    /// The whole chain for one decoded image (`batch` is the tensor's batch size: a GIF's frame count).
    pub fn analyze(&self, rgb: &Rgb, batch: usize) -> Result<Vec<Face>, EngineError> {
        self.analyze_opts(rgb, batch, true)
    }

    /// `analyze`, optionally without the descriptors (the passive check's flash frames need only the
    /// landmarks; `descriptor` is then empty).
    pub fn analyze_opts(
        &self,
        rgb: &Rgb,
        batch: usize,
        with_descriptors: bool,
    ) -> Result<Vec<Face>, EngineError> {
        if batch != 1 {
            return Err(EngineError(format!(
                "toNetInput - tf.Tensor4D with batchSize {batch} passed, but not supported in input array"
            )));
        }
        let img = rgb;
        let (img_w, img_h) = (img.w as f64, img.h as f64);
        let detections = self.locate_faces(img)?;

        // DetectAllFaceLandmarksTask: crops of det.forSize(w, h).box; an empty crop is dropped and the
        // results pair up by position (face-api's own filter).
        let mut lm_by_face = Vec::new();
        for (_, det) in &detections {
            let rel = det.rescale(1.0 / img_w, 1.0 / img_h);
            let sized = rel.rescale(img_w, img_h);
            if let Some(crop) = Self::face_crop(img, sized) {
                let rel_pts = self.landmarks_rel(&crop)?;
                lm_by_face.push((crop.w as f64, crop.h as f64, rel_pts));
            }
        }
        let mut with_landmarks = Vec::new();
        for (i, (cw, ch, rel_pts)) in lm_by_face.into_iter().enumerate() {
            let (score, det) = detections[i];
            let dims = Pt { x: cw, y: ch };
            // new FaceLandmarks68(rel, crop dims) then .shiftBy(det.box.x, det.box.y)
            let unshifted: Vec<Pt> = rel_pts
                .iter()
                .map(|&(x, y)| {
                    Pt {
                        x: x as f64,
                        y: y as f64,
                    }
                    .mul(dims)
                    .add(Pt { x: 0.0, y: 0.0 })
                })
                .collect();
            let shift = Pt { x: det.x, y: det.y };
            let landmarks: Vec<Pt> = unshifted
                .iter()
                .map(|p| p.sub(Pt { x: 0.0, y: 0.0 }).div(dims).mul(dims).add(shift))
                .collect();
            with_landmarks.push((score, det, landmarks, cw, ch));
        }

        if !with_descriptors {
            return Ok(with_landmarks
                .into_iter()
                .map(|(score, det, landmarks, _, _)| Face {
                    score,
                    detection: det,
                    landmarks,
                    descriptor: Vec::new(),
                })
                .collect());
        }
        // ComputeAllFaceDescriptorsTask: crops of landmarks.align(null, { useDlibAlignment: true })
        let mut descriptors = Vec::new();
        for (_, _, lm, cw, ch) in &with_landmarks {
            let rect = align_dlib(lm, *cw, *ch);
            if let Some(crop) = Self::face_crop(img, rect) {
                descriptors.push(self.descriptor(&crop)?);
            }
        }
        Ok(with_landmarks
            .into_iter()
            .zip(descriptors)
            .map(|((score, det, landmarks, _, _), descriptor)| Face {
                score,
                detection: det,
                landmarks,
                descriptor,
            })
            .collect())
    }
}

/// FaceLandmarks.alignDlib (relX 0.5, relY 0.43, relScale 0.45); `img_w` / `img_h` are the landmarks'
/// own image dimensions, i.e. the detection crop's.
pub fn align_dlib(lm: &[Pt], img_w: f64, img_h: f64) -> BoxF {
    let left_eye = center_point(&lm[36..42]);
    let right_eye = center_point(&lm[42..48]);
    let mouth = center_point(&lm[48..68]);
    let dist_to_mouth = |p: Pt| mouth.sub(p).magnitude();
    let eye_to_mouth = (dist_to_mouth(left_eye) + dist_to_mouth(right_eye)) / 2.0;
    let size = (eye_to_mouth / 0.45).floor();
    let r = center_point(&[left_eye, right_eye, mouth]);
    let x = (r.x - 0.5 * size).max(0.0).floor();
    let y = (r.y - 0.43 * size).max(0.0).floor();
    BoxF {
        x,
        y,
        width: size.min(img_w + x),
        height: size.min(img_h + y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(h: usize, w: usize) -> Img {
        Img {
            h,
            w,
            data: (0..h * w * 3).map(|v| (v % 251) as f32).collect(),
        }
    }

    fn get(i: &Img) -> impl Fn(usize, usize, usize) -> f32 + '_ {
        |y, x, c| i.data[(y * i.w + x) * 3 + c]
    }

    #[test]
    fn pad_to_square_centers_the_larger_half_after() {
        let src = img(2, 5);
        let p = to_batch(2, 5, get(&src), 5, true);
        assert_eq!((p.h, p.w), (5, 5));
        // diff 3: Math.round(1.5) = 2 rows after, 1 before
        assert!(p.data[..15].iter().all(|&v| v == 0.0));
        assert_eq!(p.data[15..45], src.data[..]);
        assert!(p.data[45..].iter().all(|&v| v == 0.0));
        let q = to_batch(2, 5, get(&src), 5, false);
        assert_eq!(q.data[..30], src.data[..]);
    }

    #[test]
    fn legacy_resize_samples_from_the_top_left() {
        let src = img(4, 4);
        let r = to_batch(4, 4, get(&src), 2, false);
        // scale 2: output (1,1) samples input (2,2) exactly
        assert_eq!(r.data[3 * 3], src.data[(2 * 4 + 2) * 3]);
        assert_eq!(r.data[0], 0.0);
        // scale 4/3: output 1 reads 4/3 of the way, lerp between 1 and 2
        let r = to_batch(4, 4, get(&src), 3, false);
        let v = 4f32 / 3f32;
        let (a, b) = (src.data[3], src.data[6]);
        assert_eq!(r.data[3], a + (b - a) * (v - 1.0));
    }

    #[test]
    fn clip_floors_after_clipping() {
        let b = BoxF {
            x: -3.5,
            y: 2.7,
            width: 20.2,
            height: 100.0,
        };
        let c = b.clip_at_image_borders(10.0, 50.0);
        assert_eq!(
            c,
            BoxF {
                x: 0.0,
                y: 2.0,
                width: 10.0,
                height: 47.0
            }
        );
    }
}
