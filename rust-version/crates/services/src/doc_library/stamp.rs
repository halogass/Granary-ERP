//! doclib A2: the watermark and the hidden fingerprint on every copy of a library document that leaves the
//! system.
//!
//! - PDF: one line of small grey text centered at the bottom of EVERY page (the page rotation is honoured),
//!   the fingerprint as invisible text (render mode 3) on every page, and in the Info dictionary
//!   (`/GranaryFingerprint` and `/Keywords`).
//! - PNG / JPEG / WebP: a small grey stamp in the bottom right corner, the fingerprint in the file's own
//!   metadata (PNG tEXt, JPEG COM, WebP XMP chunk).
//! - DOCX / XLSX / PPTX (source downloads only): no visible change, the fingerprint in docProps/custom.xml
//!   (and the keywords of docProps/core.xml).
//!
//! `read_fingerprints` finds those marks again (the trace of a leaked copy). Honest limit: the visible footer
//! survives printing and photos; the hidden id survives forwarding, not retyping or screenshots.

use std::io::{Cursor, Read};

use granary_core::errors::{ApiError, ApiResult};
use image::{DynamicImage, ImageDecoder, ImageReader, Rgba, RgbaImage};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::pdftools::{self, Document, Point, SaveOptions, TextOptions};

/// What goes on a copy: the visible footer line and the hidden fingerprint (`ID-` + 10 characters).
#[derive(Debug, Clone, PartialEq)]
pub struct Stamp {
    pub footer_text: String,
    pub fingerprint: String,
    /// Footer size in points (`documents.watermark.font_size`); images scale it to their size.
    pub font_size: f64,
}

impl Stamp {
    pub fn new(footer_text: impl Into<String>, fingerprint: impl Into<String>) -> Stamp {
        Stamp {
            footer_text: footer_text.into(),
            fingerprint: fingerprint.into(),
            font_size: 7.0,
        }
    }

    pub fn with_font_size(mut self, size: f64) -> Stamp {
        if size.is_finite() && size > 0.0 {
            self.font_size = size;
        }
        self
    }
}

/// Fill gray of the footer (0 black, 1 white): a mid grey readable on white, quiet on the page.
pub const FOOTER_GRAY: f64 = 0.45;
/// Name of the PDF Info entry, PNG tEXt keyword, JPEG comment prefix and OOXML custom property.
pub const MARK_KEY: &str = "GranaryFingerprint";

/// A fingerprint id: `ID-` + 10 Crockford base32 characters.
pub static FINGERPRINT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"ID-[0-9A-HJKMNP-TV-Z]{10}").expect("fingerprint regex"));

fn bad(detail: impl Into<String>) -> ApiError {
    ApiError::http(400, detail.into())
}

/// The kinds of file a stamp knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Pdf,
    Png,
    Jpeg,
    Webp,
    Gif,
    Ooxml,
}

pub const OOXML_MIMES: [&str; 3] = [
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
];

/// The kind from the mime type (falling back to the bytes when the mime is generic).
pub fn file_kind(bytes: &[u8], mime: &str) -> Option<FileKind> {
    let m = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match m.as_str() {
        "application/pdf" => return Some(FileKind::Pdf),
        "image/png" => return Some(FileKind::Png),
        "image/jpeg" | "image/jpg" | "image/pjpeg" => return Some(FileKind::Jpeg),
        "image/webp" => return Some(FileKind::Webp),
        "image/gif" => return Some(FileKind::Gif),
        _ if OOXML_MIMES.contains(&m.as_str()) => return Some(FileKind::Ooxml),
        _ => {}
    }
    sniff(bytes)
}

/// The kind from the first bytes.
pub fn sniff(bytes: &[u8]) -> Option<FileKind> {
    if bytes.starts_with(b"%PDF")
        || bytes
            .get(..1024)
            .is_some_and(|h| find(h, b"%PDF-").is_some())
    {
        Some(FileKind::Pdf)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(FileKind::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(FileKind::Jpeg)
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(FileKind::Webp)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(FileKind::Gif)
    } else if bytes.starts_with(b"PK\x03\x04") {
        Some(FileKind::Ooxml)
    } else {
        None
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `stamp_copy(bytes, mime, stamp)`: the stamped copy (see the module doc).
pub fn stamp_copy(bytes: &[u8], mime: &str, stamp: &Stamp) -> ApiResult<Vec<u8>> {
    if !FINGERPRINT_RE.is_match(&stamp.fingerprint) {
        return Err(ApiError::internal(anyhow::anyhow!(
            "invalid fingerprint {:?}",
            stamp.fingerprint
        )));
    }
    match file_kind(bytes, mime) {
        Some(FileKind::Pdf) => stamp_pdf(bytes, stamp),
        Some(FileKind::Png) | Some(FileKind::Jpeg) | Some(FileKind::Webp) | Some(FileKind::Gif) => {
            stamp_image(bytes, mime, stamp)
        }
        Some(FileKind::Ooxml) => stamp_ooxml(bytes, &stamp.fingerprint),
        None => Err(bad(format!(
            "This file type ({mime}) can't be watermarked; only PDF, PNG, JPEG, WebP, GIF and DOCX/XLSX/PPTX sources"
        ))),
    }
}

// ─── PDF ──────────────────────────────────────────────────────────────────────

fn pdf_err(e: pdftools::PdfError) -> ApiError {
    bad(format!("The PDF could not be read: {e}"))
}

/// The hidden text line: the key and the id, so a text search for either finds it.
fn hidden_line(fp: &str) -> String {
    format!("{MARK_KEY} {fp}")
}

/// Keywords with any earlier fingerprint removed and ours appended.
fn merged_keywords(existing: Option<&str>, fp: &str) -> String {
    let mut parts: Vec<String> = existing
        .unwrap_or("")
        .split([';', ','])
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.starts_with(MARK_KEY))
        .map(str::to_string)
        .collect();
    parts.push(hidden_line(fp));
    parts.join("; ")
}

pub fn stamp_pdf(bytes: &[u8], stamp: &Stamp) -> ApiResult<Vec<u8>> {
    let mut doc = Document::open(bytes).map_err(pdf_err)?;
    if doc.is_encrypted() && !doc.authenticate("") {
        return Err(bad(
            "The PDF is password protected; upload a copy without a password",
        ));
    }
    let n = doc.page_count();
    if n == 0 {
        return Err(bad("The PDF has no pages"));
    }
    let font = doc.add_stamp_font().map_err(pdf_err)?;
    let hidden = hidden_line(&stamp.fingerprint);
    for p in 0..n {
        doc.stamp_footer(
            p,
            font,
            &stamp.footer_text,
            stamp.font_size,
            FOOTER_GRAY,
            Some(&hidden),
        )
        .map_err(pdf_err)?;
    }
    doc.set_info(MARK_KEY, &stamp.fingerprint)
        .map_err(pdf_err)?;
    let kw = doc.get_info("Keywords").map_err(pdf_err)?;
    doc.set_info(
        "Keywords",
        &merged_keywords(kw.as_deref(), &stamp.fingerprint),
    )
    .map_err(pdf_err)?;
    doc.tobytes(SaveOptions {
        garbage: 1,
        deflate: true,
        clean: false,
    })
    .map_err(pdf_err)
}

/// Fingerprints in a PDF: Info `/GranaryFingerprint`, Info keywords, then the text of the pages (the
/// invisible line, or the staff footer), newest marks first.
fn pdf_fingerprints(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(mut doc) = Document::open(bytes) else {
        return out;
    };
    if doc.is_encrypted() && !doc.authenticate("") {
        return out;
    }
    if let Ok(Some(v)) = doc.get_info(MARK_KEY) {
        push_matches(&mut out, &v);
    }
    if let Ok(Some(v)) = doc.get_info("Keywords") {
        push_matches(&mut out, &v);
    }
    // the stamp is on every page: a few pages are enough (a leaked excerpt may start anywhere, so the
    // first pages of what was leaked)
    for p in 0..doc.page_count().min(5) {
        if let Ok(t) = doc.get_text(p) {
            let mut found: Vec<String> = FINGERPRINT_RE
                .find_iter(&t)
                .map(|m| m.as_str().to_string())
                .collect();
            found.reverse();
            for f in found {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
    }
    out
}

fn push_matches(out: &mut Vec<String>, text: &str) {
    for m in FINGERPRINT_RE.find_iter(text) {
        let s = m.as_str().to_string();
        if !out.contains(&s) {
            out.push(s);
        }
    }
}

// ─── images ───────────────────────────────────────────────────────────────────

/// Coverage mask (0..255) of `text` drawn in Helvetica, `px` pixels high, through MuPDF.
fn text_mask(text: &str, px: u32) -> ApiResult<(u32, u32, Vec<u8>)> {
    let text = pdftools::latin1_safe(text);
    let fs = 10.0;
    let tw = Document::helvetica_width(&text, fs).map_err(pdf_err)?;
    let (w_pt, h_pt) = (tw + 2.0, fs * 1.3);
    let mut d = Document::new().map_err(pdf_err)?;
    d.new_page(w_pt, h_pt).map_err(pdf_err)?;
    d.insert_text(
        0,
        Point::new(1.0, fs * 1.0),
        &text,
        &TextOptions {
            fontsize: fs,
            fontname: "helv".into(),
            color: Some(vec![0.0, 0.0, 0.0]),
            align: 0,
        },
    )
    .map_err(pdf_err)?;
    let zoom = px as f64 / h_pt;
    let img = d.render_page(0, zoom).map_err(pdf_err)?;
    let mask = img
        .samples
        .chunks(3)
        .map(|c| 255 - ((c[0] as u16 + c[1] as u16 + c[2] as u16) / 3) as u8)
        .collect();
    Ok((img.width, img.height, mask))
}

fn blend(dst: &mut Rgba<u8>, rgb: [u8; 3], alpha: f64) {
    let a = alpha.clamp(0.0, 1.0);
    for (d, &c) in dst.0.iter_mut().zip(rgb.iter()) {
        *d = (*d as f64 * (1.0 - a) + c as f64 * a).round() as u8;
    }
    // a transparent pixel under the stamp becomes as opaque as the stamp
    let da = dst.0[3] as f64 / 255.0;
    dst.0[3] = ((da + a * (1.0 - da)) * 255.0).round() as u8;
}

/// Draws the corner stamp: a light box with the footer text in grey, bottom right.
fn draw_corner(img: &mut RgbaImage, stamp: &Stamp) -> ApiResult<()> {
    let (w, h) = img.dimensions();
    if w < 16 || h < 16 {
        return Ok(());
    }
    // Text height: the footer size relative to an A4 width (595 pt), at least 9 px.
    let mut px = ((w as f64) * stamp.font_size * 1.3 / 595.0)
        .round()
        .max(9.0) as u32;
    px = px.min(h / 4).max(6);
    let (mut mw, mut mh, mut mask) = text_mask(&stamp.footer_text, px)?;
    let max_w = (w as f64 * 0.96) as u32;
    if mw > max_w {
        let px2 = ((px as f64) * max_w as f64 / mw as f64).floor().max(5.0) as u32;
        (mw, mh, mask) = text_mask(&stamp.footer_text, px2)?;
    }
    let pad = (mh / 4).max(2);
    let bw = (mw + 2 * pad).min(w);
    let bh = (mh + 2 * pad).min(h);
    let bx = w.saturating_sub(bw + pad);
    let by = h.saturating_sub(bh + pad);
    for y in 0..bh {
        for x in 0..bw {
            blend(img.get_pixel_mut(bx + x, by + y), [255, 255, 255], 0.55);
        }
    }
    for y in 0..mh.min(bh.saturating_sub(pad)) {
        for x in 0..mw.min(bw.saturating_sub(pad)) {
            let cov = mask[(y * mw + x) as usize];
            if cov > 0 {
                blend(
                    img.get_pixel_mut(bx + pad + x, by + pad + y),
                    [115, 115, 115],
                    cov as f64 / 255.0,
                );
            }
        }
    }
    Ok(())
}

fn decode_oriented(bytes: &[u8]) -> ApiResult<DynamicImage> {
    let unreadable = |_| bad("The image could not be read");
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| bad("The image could not be read"))?;
    let mut dec = reader.into_decoder().map_err(unreadable)?;
    let orientation = dec.orientation().ok();
    let mut img = DynamicImage::from_decoder(dec).map_err(unreadable)?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok(img)
}

pub fn stamp_image(bytes: &[u8], mime: &str, stamp: &Stamp) -> ApiResult<Vec<u8>> {
    let kind = file_kind(bytes, mime).ok_or_else(|| bad("Not an image"))?;
    let img = decode_oriented(bytes)?;
    let mut rgba = img.to_rgba8();
    draw_corner(&mut rgba, stamp)?;
    let fp = &stamp.fingerprint;
    match kind {
        FileKind::Png => {
            let mut out = Vec::new();
            DynamicImage::ImageRgba8(rgba)
                .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
                .map_err(|e| ApiError::internal(anyhow::anyhow!("png encode: {e}")))?;
            Ok(png_add_text(&out, MARK_KEY, fp))
        }
        FileKind::Jpeg => {
            let rgb = DynamicImage::ImageRgba8(rgba).to_rgb8();
            let mut out = Vec::new();
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92);
            rgb.write_with_encoder(enc)
                .map_err(|e| ApiError::internal(anyhow::anyhow!("jpeg encode: {e}")))?;
            Ok(jpeg_add_comment(&out, &format!("{MARK_KEY} {fp}")))
        }
        FileKind::Webp => {
            let mut out = Vec::new();
            let enc = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
            rgba.write_with_encoder(enc)
                .map_err(|e| ApiError::internal(anyhow::anyhow!("webp encode: {e}")))?;
            Ok(webp_add_xmp(&out, &xmp_packet(fp)))
        }
        FileKind::Gif => {
            // the first frame only (a library image is a still)
            let mut out = Vec::new();
            {
                let mut enc = image::codecs::gif::GifEncoder::new_with_speed(&mut out, 10);
                enc.encode_frame(image::Frame::new(rgba))
                    .map_err(|e| ApiError::internal(anyhow::anyhow!("gif encode: {e}")))?;
            }
            Ok(gif_add_comment(&out, &format!("{MARK_KEY} {fp}")))
        }
        _ => Err(bad("Not an image")),
    }
}

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = flate2::Crc::new();
    for p in parts {
        c.update(p);
    }
    c.sum()
}

/// A tEXt chunk inserted right before IEND.
pub fn png_add_text(png: &[u8], key: &str, value: &str) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(key.as_bytes());
    data.push(0);
    data.extend_from_slice(value.as_bytes());
    let mut chunk = Vec::new();
    chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
    chunk.extend_from_slice(b"tEXt");
    chunk.extend_from_slice(&data);
    chunk.extend_from_slice(&crc32(&[b"tEXt", &data]).to_be_bytes());
    // walk the chunks to find IEND
    let mut at = 8;
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]) as usize;
        if &png[at + 4..at + 8] == b"IEND" {
            let mut out = png[..at].to_vec();
            out.extend_from_slice(&chunk);
            out.extend_from_slice(&png[at..]);
            return out;
        }
        at += 12 + len;
    }
    png.to_vec()
}

/// tEXt chunks of a PNG: (keyword, text).
pub fn png_texts(png: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !png.starts_with(b"\x89PNG") {
        return out;
    }
    let mut at = 8;
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]) as usize;
        let typ = &png[at + 4..at + 8];
        let Some(data) = png.get(at + 8..at + 8 + len) else {
            break;
        };
        if typ == b"tEXt" || typ == b"iTXt" {
            if let Some(z) = data.iter().position(|b| *b == 0) {
                out.push((
                    String::from_utf8_lossy(&data[..z]).into_owned(),
                    String::from_utf8_lossy(&data[z + 1..]).into_owned(),
                ));
            }
        }
        if typ == b"IEND" {
            break;
        }
        at += 12 + len;
    }
    out
}

/// A COM segment right after SOI.
pub fn jpeg_add_comment(jpg: &[u8], comment: &str) -> Vec<u8> {
    if jpg.len() < 2 {
        return jpg.to_vec();
    }
    let c = comment.as_bytes();
    let len = (c.len() + 2).min(0xFFFF) as u16;
    let mut out = Vec::with_capacity(jpg.len() + c.len() + 4);
    out.extend_from_slice(&jpg[..2]);
    out.extend_from_slice(&[0xFF, 0xFE]);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&c[..len as usize - 2]);
    out.extend_from_slice(&jpg[2..]);
    out
}

/// COM segments (and any APP segment text) of a JPEG before the scan.
pub fn jpeg_comments(jpg: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    if !jpg.starts_with(&[0xFF, 0xD8]) {
        return out;
    }
    let mut at = 2;
    while at + 4 <= jpg.len() && jpg[at] == 0xFF {
        let marker = jpg[at + 1];
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 || marker == 0xFF {
            at += if marker == 0xFF { 1 } else { 2 };
            continue;
        }
        let len = u16::from_be_bytes([jpg[at + 2], jpg[at + 3]]) as usize;
        let Some(seg) = jpg.get(at + 4..at + 2 + len) else {
            break;
        };
        if marker == 0xFE || (0xE0..=0xEF).contains(&marker) {
            out.push(String::from_utf8_lossy(seg).into_owned());
        }
        at += 2 + len;
    }
    out
}

/// A Comment Extension right after the header, logical screen descriptor and global color table.
pub fn gif_add_comment(gif: &[u8], comment: &str) -> Vec<u8> {
    if gif.len() < 13 {
        return gif.to_vec();
    }
    let packed = gif[10];
    let gct = if packed & 0x80 != 0 {
        3 * (1usize << ((packed & 0x07) + 1))
    } else {
        0
    };
    let at = 13 + gct;
    if at > gif.len() {
        return gif.to_vec();
    }
    let mut ext = vec![0x21, 0xFE];
    for chunk in comment.as_bytes().chunks(255) {
        ext.push(chunk.len() as u8);
        ext.extend_from_slice(chunk);
    }
    ext.push(0);
    let mut out = gif[..at].to_vec();
    out.extend_from_slice(&ext);
    out.extend_from_slice(&gif[at..]);
    out
}

fn xmp_packet(fp: &str) -> String {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:granary=\"https://example.com/ns/doc/1.0/\" granary:{MARK_KEY}=\"{fp}\"/></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
    )
}

fn riff_chunks(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 12;
    while at + 8 <= data.len() {
        let mut id = [0u8; 4];
        id.copy_from_slice(&data[at..at + 4]);
        let len =
            u32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]) as usize;
        let Some(body) = data.get(at + 8..at + 8 + len) else {
            break;
        };
        out.push((id, body));
        at += 8 + len + (len & 1);
    }
    out
}

fn riff_chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() & 1 == 1 {
        out.push(0);
    }
}

/// A simple (VP8L/VP8) WebP turned into the extended format with an `XMP ` chunk.
pub fn webp_add_xmp(webp: &[u8], xmp: &str) -> Vec<u8> {
    let chunks = riff_chunks(webp);
    let Some((first_id, first)) = chunks.first() else {
        return webp.to_vec();
    };
    let (w, h) = match first_id {
        b"VP8L" if first.len() >= 5 => {
            let b = &first[1..5];
            let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1)
        }
        b"VP8 " if first.len() >= 10 => (
            (u16::from_le_bytes([first[6], first[7]]) & 0x3FFF) as u32,
            (u16::from_le_bytes([first[8], first[9]]) & 0x3FFF) as u32,
        ),
        _ => return webp.to_vec(),
    };
    let alpha = matches!(first_id, b"VP8L") && first.len() >= 5 && (first[4] >> 4) & 1 == 1;
    let mut flags = 0x04u8; // XMP
    if alpha {
        flags |= 0x10;
    }
    let mut vp8x = vec![flags, 0, 0, 0];
    vp8x.extend_from_slice(&(w - 1).to_le_bytes()[..3]);
    vp8x.extend_from_slice(&(h - 1).to_le_bytes()[..3]);
    let mut body = b"WEBP".to_vec();
    riff_chunk(&mut body, b"VP8X", &vp8x);
    for (id, b) in &chunks {
        riff_chunk(&mut body, id, b);
    }
    riff_chunk(&mut body, b"XMP ", xmp.as_bytes());
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn webp_texts(webp: &[u8]) -> Vec<String> {
    if !(webp.len() > 12 && &webp[..4] == b"RIFF" && &webp[8..12] == b"WEBP") {
        return vec![];
    }
    riff_chunks(webp)
        .into_iter()
        .filter(|(id, _)| id == b"XMP " || id == b"EXIF")
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
        .collect()
}

// ─── OOXML ────────────────────────────────────────────────────────────────────

/// Every entry of a zip, inflated: (name, data). Stored and deflated entries only.
pub fn unzip_all(data: &[u8]) -> ApiResult<Vec<(String, Vec<u8>)>> {
    let badzip = || bad("The file is not a valid DOCX/XLSX/PPTX (zip) file");
    let u16le = |at: usize| -> ApiResult<usize> {
        data.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
            .ok_or_else(badzip)
    };
    let u32le = |at: usize| -> ApiResult<usize> {
        data.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(badzip)
    };
    if data.len() < 22 {
        return Err(badzip());
    }
    let lo = data.len().saturating_sub(22 + 65535);
    let eocd = (lo..=data.len() - 22)
        .rev()
        .find(|&i| data[i..i + 4] == *b"PK\x05\x06")
        .ok_or_else(badzip)?;
    let count = u16le(eocd + 10)?;
    let mut at = u32le(eocd + 16)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if data.get(at..at + 4) != Some(b"PK\x01\x02") {
            return Err(badzip());
        }
        let method = u16le(at + 10)?;
        let csize = u32le(at + 20)?;
        let nlen = u16le(at + 28)?;
        let elen = u16le(at + 30)?;
        let clen = u16le(at + 32)?;
        let local = u32le(at + 42)?;
        let name = String::from_utf8_lossy(data.get(at + 46..at + 46 + nlen).ok_or_else(badzip)?)
            .into_owned();
        at += 46 + nlen + elen + clen;
        if data.get(local..local + 4) != Some(b"PK\x03\x04") {
            return Err(badzip());
        }
        let start = local + 30 + u16le(local + 26)? + u16le(local + 28)?;
        let raw = data.get(start..start + csize).ok_or_else(badzip)?;
        let body = match method {
            0 => raw.to_vec(),
            8 => {
                let mut v = Vec::new();
                flate2::read::DeflateDecoder::new(raw)
                    .read_to_end(&mut v)
                    .map_err(|_| badzip())?;
                v
            }
            _ => return Err(bad("The file uses a zip compression that can't be read")),
        };
        out.push((name, body));
    }
    Ok(out)
}

const CUSTOM_PART: &str = "docProps/custom.xml";
const CUSTOM_CT: &str = "application/vnd.openxmlformats-officedocument.custom-properties+xml";
const CUSTOM_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";
const CORE_NS: &str = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";

static PROP_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r#"(?s)<(?:\w+:)?property\b[^>]*\bname="{MARK_KEY}"[^>]*>.*?</(?:\w+:)?property>"#
    ))
    .expect("prop regex")
});
static PID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\bpid="(\d+)""#).expect("pid regex"));

fn custom_xml(existing: Option<&str>, fp: &str) -> String {
    let fresh = || {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/custom-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\"><property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"2\" name=\"{MARK_KEY}\"><vt:lpwstr>{fp}</vt:lpwstr></property></Properties>"
        )
    };
    let Some(xml) = existing else {
        return fresh();
    };
    let xml = PROP_RE.replace_all(xml, "").into_owned();
    let pid = PID_RE
        .captures_iter(&xml)
        .filter_map(|c| c[1].parse::<u32>().ok())
        .max()
        .unwrap_or(1)
        + 1;
    // the vt prefix the file already uses for docPropsVTypes (usually vt)
    let vt = Regex::new(
        r#"xmlns:(\w+)="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes""#,
    )
    .ok()
    .and_then(|r| r.captures(&xml).map(|c| c[1].to_string()));
    let Some(close) = xml.rfind("</") else {
        return fresh();
    };
    let root_prefix = {
        let tag = &xml[close + 2..];
        let name = tag.split('>').next().unwrap_or("");
        name.split_once(':')
            .map(|(p, _)| format!("{p}:"))
            .unwrap_or_default()
    };
    let (vt_prefix, vt_decl) = match vt {
        Some(p) => (p, String::new()),
        None => (
            "granaryvt".to_string(),
            " xmlns:granaryvt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\""
                .to_string(),
        ),
    };
    let prop = format!(
        "<{root_prefix}property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"{pid}\" name=\"{MARK_KEY}\"{vt_decl}><{vt_prefix}:lpwstr>{fp}</{vt_prefix}:lpwstr></{root_prefix}property>"
    );
    format!("{}{}{}", &xml[..close], prop, &xml[close..])
}

fn core_xml(xml: &str, fp: &str) -> String {
    let prefix = Regex::new(&format!(r#"xmlns:(\w+)="{}""#, regex::escape(CORE_NS)))
        .ok()
        .and_then(|r| r.captures(xml).map(|c| c[1].to_string()));
    let Some(p) = prefix else {
        return xml.to_string();
    };
    let full = Regex::new(&format!(
        r"(?s)<{p}:keywords\b[^>]*?(?:/>|>(.*?)</{p}:keywords>)"
    ))
    .expect("keywords regex");
    if let Some(c) = full.captures(xml) {
        let existing = c.get(1).map(|m| m.as_str());
        let kw = merged_keywords(existing, fp);
        let m = c.get(0).expect("whole match");
        return format!(
            "{}<{p}:keywords>{kw}</{p}:keywords>{}",
            &xml[..m.start()],
            &xml[m.end()..]
        );
    }
    let Some(close) = xml.rfind("</") else {
        return xml.to_string();
    };
    format!(
        "{}<{p}:keywords>{}</{p}:keywords>{}",
        &xml[..close],
        hidden_line(fp),
        &xml[close..]
    )
}

fn content_types(xml: &str) -> String {
    if xml.contains("/docProps/custom.xml") {
        return xml.to_string();
    }
    let Some(close) = xml.rfind("</") else {
        return xml.to_string();
    };
    format!(
        "{}<Override PartName=\"/{CUSTOM_PART}\" ContentType=\"{CUSTOM_CT}\"/>{}",
        &xml[..close],
        &xml[close..]
    )
}

fn root_rels(xml: &str) -> String {
    if xml.contains(CUSTOM_REL) {
        return xml.to_string();
    }
    let Some(close) = xml.rfind("</") else {
        return xml.to_string();
    };
    format!(
        "{}<Relationship Id=\"rIdGranaryFingerprint\" Type=\"{CUSTOM_REL}\" Target=\"{CUSTOM_PART}\"/>{}",
        &xml[..close],
        &xml[close..]
    )
}

pub fn stamp_ooxml(bytes: &[u8], fp: &str) -> ApiResult<Vec<u8>> {
    let mut entries = unzip_all(bytes)?;
    if !entries.iter().any(|(n, _)| n == "[Content_Types].xml") {
        return Err(bad("The file is not a valid DOCX/XLSX/PPTX file"));
    }
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let mut has_custom = false;
    let mut has_rels = false;
    for (name, body) in entries.iter_mut() {
        match name.as_str() {
            CUSTOM_PART => {
                has_custom = true;
                *body = custom_xml(Some(&text(body)), fp).into_bytes();
            }
            "docProps/core.xml" => *body = core_xml(&text(body), fp).into_bytes(),
            "[Content_Types].xml" => *body = content_types(&text(body)).into_bytes(),
            "_rels/.rels" => {
                has_rels = true;
                *body = root_rels(&text(body)).into_bytes();
            }
            _ => {}
        }
    }
    if !has_custom {
        entries.push((CUSTOM_PART.to_string(), custom_xml(None, fp).into_bytes()));
    }
    if !has_rels {
        entries.push((
            "_rels/.rels".to_string(),
            root_rels("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>").into_bytes(),
        ));
    }
    Ok(crate::later::verification::zip_deflated(
        &entries,
        chrono::Local::now(),
    ))
}

fn ooxml_texts(bytes: &[u8]) -> Vec<String> {
    let Ok(entries) = unzip_all(bytes) else {
        return vec![];
    };
    entries
        .into_iter()
        .filter(|(n, _)| n == CUSTOM_PART || n == "docProps/core.xml")
        .map(|(_, b)| String::from_utf8_lossy(&b).into_owned())
        .collect()
}

// ─── reading the marks back ───────────────────────────────────────────────────

/// Every fingerprint id found in the file's marks, the most specific first (metadata before text).
pub fn read_fingerprints(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    match sniff(bytes) {
        Some(FileKind::Pdf) => return pdf_fingerprints(bytes),
        Some(FileKind::Png) => {
            for (k, v) in png_texts(bytes) {
                if k == MARK_KEY {
                    push_matches(&mut out, &v);
                }
            }
            for (_, v) in png_texts(bytes) {
                push_matches(&mut out, &v);
            }
        }
        Some(FileKind::Jpeg) => {
            for c in jpeg_comments(bytes) {
                push_matches(&mut out, &c);
            }
        }
        Some(FileKind::Webp) => {
            for t in webp_texts(bytes) {
                push_matches(&mut out, &t);
            }
        }
        Some(FileKind::Ooxml) => {
            for t in ooxml_texts(bytes) {
                push_matches(&mut out, &t);
            }
        }
        Some(FileKind::Gif) | None => {}
    }
    if out.is_empty() && sniff(bytes) != Some(FileKind::Ooxml) {
        // image metadata is stored uncompressed: a raw scan for the key finds it in any container
        let raw = String::from_utf8_lossy(bytes);
        let key_re = Regex::new(&format!(
            r"{MARK_KEY}\W{{0,3}}(ID-[0-9A-HJKMNP-TV-Z]{{10}})"
        ))
        .expect("raw key regex");
        for c in key_re.captures_iter(&raw) {
            let s = c[1].to_string();
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_replace_old_marks() {
        assert_eq!(
            merged_keywords(
                Some("brochure; GranaryFingerprint ID-0000000000"),
                "ID-ABCDEFGHJK"
            ),
            "brochure; GranaryFingerprint ID-ABCDEFGHJK"
        );
        assert_eq!(
            merged_keywords(None, "ID-ABCDEFGHJK"),
            "GranaryFingerprint ID-ABCDEFGHJK"
        );
    }

    #[test]
    fn jpeg_comment_round_trip() {
        let fake = [0xFF, 0xD8, 0xFF, 0xDA, 0, 2, 0xFF, 0xD9];
        let out = jpeg_add_comment(&fake, "GranaryFingerprint ID-ABCDEFGHJK");
        assert_eq!(
            jpeg_comments(&out),
            vec!["GranaryFingerprint ID-ABCDEFGHJK"]
        );
    }
}
