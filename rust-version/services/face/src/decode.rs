//! `tf.node.decodeImage(buffer, 3)` of @tensorflow/tfjs-node 4.22.0, pixel for pixel.
//!
//! - The type comes from the first bytes (tfjs-node `getImageType`); anything else throws.
//! - JPEG: TensorFlow's DecodeJpeg with its defaults. Measured on a reference data set: TensorFlow's output is
//!   bit-identical to libjpeg-turbo with the fast integer IDCT (JDCT_IFAST) and fancy upsampling, so this
//!   module decodes with libjpeg-turbo's TurboJPEG API and `TJFLAG_FASTDCT` (the system libturbojpeg.so.0,
//!   Debian package libturbojpeg0). EXIF orientation is ignored, as TensorFlow does.
//! - PNG: 8 bits per channel (16-bit samples keep their high byte, png_set_strip_16), palette and low-bit
//!   gray expanded, gray replicated to RGB, alpha dropped.
//! - GIF: `decodeImage` leaves `expandAnimations` at true, so a GIF is a 4-D tensor `[frames, h, w, 3]`:
//!   the service then reports `height` = frame count and `width` = the GIF's height, and face-api refuses a
//!   batch of more than one frame (the request answers 422). Both quirks are kept.
//! - BMP: decoded with the `image` crate (TensorFlow's own BMP reader accepts fewer variants).

use std::ffi::{c_char, c_int, c_uchar, c_ulong, c_void, CStr};

/// A decoded image: height x width x 3 RGB bytes, row-major.
#[derive(Debug, Clone)]
pub struct Rgb {
    pub h: usize,
    pub w: usize,
    pub data: Vec<u8>,
}

/// What the service reports as `width` / `height` (`const [height, width] = tensor.shape`).
#[derive(Debug, Clone)]
pub struct Decoded {
    pub image: Rgb,
    pub report_width: usize,
    pub report_height: usize,
    /// The tensor's batch size (GIF frame count, else 1); face-api refuses anything but 1.
    pub batch: usize,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Kind {
    Jpeg,
    Gif,
    Png,
    Bmp,
}

/// tfjs-node `getImageType`.
pub fn image_type(b: &[u8]) -> Option<Kind> {
    if b.len() > 3 && b[0] == 255 && b[1] == 216 && b[2] == 255 {
        Some(Kind::Jpeg)
    } else if b.len() > 4 && b[..4] == [71, 73, 70, 56] {
        Some(Kind::Gif)
    } else if b.len() > 8 && b[..8] == [137, 80, 78, 71, 13, 10, 26, 10] {
        Some(Kind::Png)
    } else if b.len() > 3 && b[0] == 66 && b[1] == 77 {
        Some(Kind::Bmp)
    } else {
        None
    }
}

pub fn decode(b: &[u8]) -> Result<Decoded, String> {
    let kind = image_type(b).ok_or_else(|| {
        "Expected image (BMP, JPEG, PNG, or GIF), but got unsupported image type".to_string()
    })?;
    let single = |image: Rgb| Decoded {
        report_width: image.w,
        report_height: image.h,
        batch: 1,
        image,
    };
    match kind {
        Kind::Jpeg => decode_jpeg(b).map(single),
        Kind::Png => decode_png(b).map(single),
        Kind::Bmp => decode_bmp(b).map(single),
        Kind::Gif => {
            let (image, frames) = decode_gif(b)?;
            Ok(Decoded {
                report_width: image.h,
                report_height: frames,
                batch: frames,
                image,
            })
        }
    }
}

// ─── JPEG: TurboJPEG (libjpeg-turbo) ─────────────────────────────────────────

#[link(name = "libturbojpeg.so.0", kind = "dylib", modifiers = "+verbatim")]
extern "C" {
    fn tjInitDecompress() -> *mut c_void;
    fn tjDecompressHeader3(
        handle: *mut c_void,
        jpeg_buf: *const c_uchar,
        jpeg_size: c_ulong,
        width: *mut c_int,
        height: *mut c_int,
        jpeg_subsamp: *mut c_int,
        jpeg_colorspace: *mut c_int,
    ) -> c_int;
    fn tjDecompress2(
        handle: *mut c_void,
        jpeg_buf: *const c_uchar,
        jpeg_size: c_ulong,
        dst_buf: *mut c_uchar,
        width: c_int,
        pitch: c_int,
        height: c_int,
        pixel_format: c_int,
        flags: c_int,
    ) -> c_int;
    fn tjGetErrorStr2(handle: *mut c_void) -> *const c_char;
    fn tjGetErrorCode(handle: *mut c_void) -> c_int;
    fn tjDestroy(handle: *mut c_void) -> c_int;
}

const TJPF_RGB: c_int = 0;
const TJFLAG_FASTDCT: c_int = 2048;
const TJERR_WARNING: c_int = 0;

struct Tj(*mut c_void);

impl Drop for Tj {
    fn drop(&mut self) {
        // SAFETY: the handle came from tjInitDecompress and is destroyed once.
        unsafe {
            tjDestroy(self.0);
        }
    }
}

impl Tj {
    fn error(&self) -> String {
        // SAFETY: tjGetErrorStr2 returns a NUL-terminated string owned by the handle.
        unsafe {
            let p = tjGetErrorStr2(self.0);
            if p.is_null() {
                "JPEG decode failed".into()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }
}

pub fn decode_jpeg(b: &[u8]) -> Result<Rgb, String> {
    // SAFETY: plain C calls on a handle we own; buffers are sized from the header the library returns.
    unsafe {
        let h = tjInitDecompress();
        if h.is_null() {
            return Err("tjInitDecompress failed".into());
        }
        let tj = Tj(h);
        let (mut w, mut ht, mut ss, mut cs) = (0, 0, 0, 0);
        if tjDecompressHeader3(
            tj.0,
            b.as_ptr(),
            b.len() as c_ulong,
            &mut w,
            &mut ht,
            &mut ss,
            &mut cs,
        ) != 0
        {
            return Err(tj.error());
        }
        if w <= 0 || ht <= 0 {
            return Err("Invalid JPEG data or crop window".into());
        }
        let (w, ht) = (w as usize, ht as usize);
        let mut data = vec![0u8; w * ht * 3];
        let r = tjDecompress2(
            tj.0,
            b.as_ptr(),
            b.len() as c_ulong,
            data.as_mut_ptr(),
            w as c_int,
            0,
            ht as c_int,
            TJPF_RGB,
            TJFLAG_FASTDCT,
        );
        if r != 0 {
            // A warning still gives the image (libjpeg's own recovery); TensorFlow refuses a truncated
            // file (try_recover_truncated=false), everything else it decodes.
            let msg = tj.error();
            if tjGetErrorCode(tj.0) != TJERR_WARNING || msg.contains("Premature end") {
                return Err(msg);
            }
        }
        Ok(Rgb { h: ht, w, data })
    }
}

// ─── PNG ──────────────────────────────────────────────────────────────────────

pub fn decode_png(b: &[u8]) -> Result<Rgb, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(b));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| "PNG too large".to_string())?;
    let mut buf = vec![0u8; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("unexpanded palette".into()),
    };
    let mut data = Vec::with_capacity(w * h * 3);
    for row in 0..h {
        let line = &buf[row * info.line_size..row * info.line_size + w * channels];
        for px in line.chunks_exact(channels) {
            if channels < 3 {
                data.extend_from_slice(&[px[0], px[0], px[0]]);
            } else {
                data.extend_from_slice(&px[..3]);
            }
        }
    }
    Ok(Rgb { h, w, data })
}

// ─── GIF ──────────────────────────────────────────────────────────────────────

/// The first frame on a zeroed screen-sized canvas (transparent pixels stay 0) and the frame count.
pub fn decode_gif(b: &[u8]) -> Result<(Rgb, usize), String> {
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::Indexed);
    let mut dec = opts.read_info(b).map_err(|e| e.to_string())?;
    let (w, h) = (dec.width() as usize, dec.height() as usize);
    let global = dec.global_palette().map(|p| p.to_vec());
    let mut data = vec![0u8; w * h * 3];
    let mut frames = 0usize;
    while let Some(frame) = dec.read_next_frame().map_err(|e| e.to_string())? {
        if frames == 0 {
            let palette = frame
                .palette
                .clone()
                .or_else(|| global.clone())
                .ok_or_else(|| "GIF without a color map".to_string())?;
            let (fl, ft) = (frame.left as usize, frame.top as usize);
            let fw = frame.width as usize;
            for (i, &idx) in frame.buffer.iter().enumerate() {
                let (x, y) = (fl + i % fw.max(1), ft + i / fw.max(1));
                if x >= w || y >= h || Some(idx) == frame.transparent {
                    continue;
                }
                let p = idx as usize * 3;
                if p + 2 < palette.len() {
                    data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&palette[p..p + 3]);
                }
            }
        }
        frames += 1;
    }
    if frames == 0 {
        return Err("GIF without frames".into());
    }
    Ok((Rgb { h, w, data }, frames))
}

// ─── BMP ──────────────────────────────────────────────────────────────────────

pub fn decode_bmp(b: &[u8]) -> Result<Rgb, String> {
    let img = image::load_from_memory_with_format(b, image::ImageFormat::Bmp)
        .map_err(|e| e.to_string())?
        .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    Ok(Rgb {
        h,
        w,
        data: img.into_raw(),
    })
}
