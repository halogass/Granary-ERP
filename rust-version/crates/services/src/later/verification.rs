//! Helpers of backend/app/routers/signing.py that services/verification/signing_flow.py calls (wave 2 stubs,
//! now real), plus the routers/signing.py helpers that need this crate's dependencies (MuPDF, image, qrcode,
//! flate2), which the api crate does not carry. The router (crates/api/src/routers/signing.rs) calls these.
//!
//! - [`routers_signing_upload_to_minio`] is `_upload_to_minio`;
//! - [`routers_signing_send_signing_invitation`] is `_send_signing_invitation` (own DB session, errors swallowed);
//! - [`stamp_signed_pdf`] is the flattening half of `_generate_signed_pdf` (`_draw_signature_stamp`,
//!   `_insert_fitted`, `_img_fit_rect`, `_fit_caption_fontsize`, the field loop and `tobytes`);
//! - [`qr_png`] is `qrcode.QRCode(box_size=, border=)` PNGs (`_generate_qr_base64`, `_make_qr_png`);
//! - [`zip_deflated`] is `zipfile.ZipFile(buf, "w", ZIP_DEFLATED)` + `writestr`.

use std::io::{Cursor, Write};

use chrono::{DateTime, Utc};
use granary_compat::pyfmt::{html_escape, strftime};
use granary_compat::pyjson::PyDateTime;
use granary_core::config::settings;
use granary_core::errors::{ApiError, ApiResult};
use granary_db::rows::signing::{SignDocument, SignSigner};
use uuid::Uuid;

use crate::pdftools::{self, Point, Rect, SaveOptions, TextOptions};

/// routers/signing.py `_upload_to_minio(data, object_name, content_type="application/pdf") -> str`: puts the
/// bytes in MINIO_BUCKET_DOCUMENTS (creating the bucket if missing) and returns `"signing/{object_name}"`.
/// A storage failure is the exception minio-py raises (unhandled where Python does not catch it).
pub async fn routers_signing_upload_to_minio(
    data: Vec<u8>,
    object_name: String,
    content_type: String,
) -> ApiResult<String> {
    let store = crate::storage::minio();
    let bucket = settings().MINIO_BUCKET_DOCUMENTS.clone();
    crate::storage::ensure_bucket(&*store, &bucket)
        .await
        .map_err(ApiError::internal)?;
    store
        .put_object(&bucket, &object_name, &data, &content_type)
        .await
        .map_err(ApiError::internal)?;
    Ok(format!("signing/{object_name}"))
}

/// routers/signing.py `_send_signing_invitation(doc_id, signer_id)`: emails a signer the signing link from
/// its own DB session (`AsyncSessionLocal`); every error is swallowed (`except Exception: pass`).
pub async fn routers_signing_send_signing_invitation(
    doc_id: String,
    signer_id: String,
) -> ApiResult<()> {
    let _ = send_signing_invitation_inner(&doc_id, &signer_id).await;
    Ok(())
}

async fn send_signing_invitation_inner(doc_id: &str, signer_id: &str) -> anyhow::Result<()> {
    // `_uuid.UUID(doc_id)`: a malformed id raises (swallowed).
    let doc_uuid = Uuid::parse_str(doc_id)?;
    let signer_uuid = Uuid::parse_str(signer_id)?;
    let pool = crate::nary::session_pool().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut conn = pool.acquire().await?;
    let doc: Option<SignDocument> = sqlx::query_as("SELECT * FROM sign_documents WHERE id = $1")
        .bind(doc_uuid)
        .fetch_optional(&mut *conn)
        .await?;
    let signer: Option<SignSigner> = sqlx::query_as("SELECT * FROM sign_signers WHERE id = $1")
        .bind(signer_uuid)
        .fetch_optional(&mut *conn)
        .await?;
    let (Some(doc), Some(signer)) = (doc, signer) else {
        return Ok(());
    };
    let cfg = crate::email::load_system_smtp(&mut conn).await?;
    let s = settings();
    let sign_url = format!("{}/sign/{}", s.APP_URL, signer.token);
    // settings.APP_NAME.split()[0] (IndexError on an empty name: swallowed).
    let brand = s
        .APP_NAME
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("list index out of range"))?
        .to_string();
    let message = match doc.message.as_deref() {
        Some(m) if !m.is_empty() => format!("<p style=\"color:#555\">{}</p>", html_escape(m, true)),
        _ => String::new(),
    };
    let html = format!(
        r##"
            <html><body style="font-family:Arial,sans-serif;color:#111;max-width:560px;margin:40px auto;padding:0 20px">
              <div style="border-bottom:3px solid #1f4e79;padding-bottom:16px;margin-bottom:24px">
                <span style="font-size:24px;font-weight:900;color:#1f4e79;letter-spacing:4px">{brand}</span>
              </div>
              <h2 style="font-size:20px;color:#111">You have a document to sign</h2>
              <p>Hello <strong>{name}</strong>,</p>
              <p><strong>{title}</strong> requires your electronic signature.</p>
              {message}
              <div style="margin:32px 0;text-align:center">
                <a href="{sign_url}" style="background:#1f4e79;color:white;padding:14px 32px;border-radius:10px;font-weight:700;font-size:15px;text-decoration:none;display:inline-block">
                  Review &amp; Sign Document
                </a>
              </div>
              <p style="font-size:12px;color:#888">
                Or copy this link: <a href="{sign_url}" style="color:#1f4e79">{sign_url}</a>
              </p>
              <hr style="border:none;border-top:1px solid #eee;margin:24px 0">
              <p style="font-size:11px;color:#aaa">
                By signing, you consent to the use of electronic signatures for this document.
              </p>
            </body></html>
            "##,
        name = html_escape(&signer.name, true),
        title = html_escape(&doc.title, true),
    );
    crate::email::_send_via_config(
        &cfg,
        std::slice::from_ref(&signer.email),
        &format!("Signature Required: {}", doc.title),
        &html,
        None,
        None,
        None,
        None,
        None,
    )
    .await?;
    Ok(())
}

// ─── QR codes ─────────────────────────────────────────────────────────────────

/// The PNG of `qrcode.QRCode(box_size=box_size, border=border)` for `url` (best-fit version, ECC M, black
/// on white). `_make_qr_png` is `qr_png(url, 10, 1)`; `_generate_qr_base64` is `qr_png(url, 8, 2)`.
pub fn qr_png(url: &str, box_size: usize, border: usize) -> ApiResult<Vec<u8>> {
    let (_, _, rows) = crate::verification::service::qr_matrix(url.as_bytes())?;
    let size = (rows.len() + 2 * border) * box_size;
    let mut img = image::GrayImage::from_pixel(size as u32, size as u32, image::Luma([255u8]));
    for (r, row) in rows.iter().enumerate() {
        for (c, dark) in row.iter().enumerate() {
            if *dark {
                for y in 0..box_size {
                    for x in 0..box_size {
                        img.put_pixel(
                            ((c + border) * box_size + x) as u32,
                            ((r + border) * box_size + y) as u32,
                            image::Luma([0]),
                        );
                    }
                }
            }
        }
    }
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(ApiError::internal)?;
    Ok(buf.into_inner())
}

// ─── Signed PDF (flattening) ──────────────────────────────────────────────────

/// One `sign_fields` row with its signer (`selectinload(SignField.signer)`), as `_generate_signed_pdf` reads it.
pub struct StampField {
    pub field_type: String,
    pub page_number: Option<i32>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub value: Option<String>,
    pub signer: Option<SignSigner>,
}

/// `_decode_data_url(data)`: a base64 data URL (or bare base64) to bytes; None when empty or undecodable.
pub fn decode_data_url(data: Option<&str>) -> Option<Vec<u8>> {
    let data = data.filter(|d| !d.is_empty())?;
    let b64 = match data.split_once(',') {
        Some((_, rest)) => rest,
        None => data,
    };
    granary_compat::crypto::pyb64::b64decode_str(b64).ok()
}

/// `_img_fit_rect(box, img_bytes)`: the image normalised to PNG (EXIF orientation applied, odd modes to
/// RGBA) and the aspect-preserving rect centred inside `box`.
fn img_fit_rect(bx: Rect, img_bytes: &[u8]) -> Result<(Vec<u8>, Rect), String> {
    use image::ImageDecoder;
    let reader = image::ImageReader::new(Cursor::new(img_bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut im = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        im.apply_orientation(o);
    }
    let im = match im {
        image::DynamicImage::ImageRgb8(_) | image::DynamicImage::ImageRgba8(_) => im,
        image::DynamicImage::ImageRgb16(_) | image::DynamicImage::ImageRgb32F(_) => {
            image::DynamicImage::ImageRgb8(im.to_rgb8())
        }
        other => image::DynamicImage::ImageRgba8(other.to_rgba8()),
    };
    let mut buf = Cursor::new(Vec::new());
    im.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let (iw, ih) = (im.width() as f64, im.height() as f64);
    if iw <= 0.0 || ih <= 0.0 {
        return Ok((buf.into_inner(), bx));
    }
    let scale = (bx.width() / iw).min(bx.height() / ih);
    let (w, h) = (iw * scale, ih * scale);
    let (cx, cy) = ((bx.x0 + bx.x1) / 2.0, (bx.y0 + bx.y1) / 2.0);
    Ok((
        buf.into_inner(),
        Rect::new(cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0),
    ))
}

/// `_insert_fitted(page, box, img_bytes)`: never fails.
fn insert_fitted(pdf: &mut pdftools::Document, pno: usize, bx: Rect, img: Option<&[u8]>) {
    let Some(img) = img.filter(|i| !i.is_empty()) else {
        return;
    };
    if let Ok((png, fr)) = img_fit_rect(bx, img) {
        let _ = pdf.insert_image(pno, fr, &png);
    }
}

/// `_fit_caption_fontsize(lines, w, h)` (fontname "helv", maxsize 6.5, minsize 2.6).
fn fit_caption_fontsize(lines: &[&str], w: f64, h: f64) -> f64 {
    let (maxsize, minsize) = (6.5f64, 2.6f64);
    let n = lines.len().max(1) as f64;
    let mut fs = maxsize.min(h / (n * 1.18));
    // max(lines, key=len): the first longest line (len counts characters).
    let mut longest = "";
    for l in lines {
        if l.chars().count() > longest.chars().count() {
            longest = l;
        }
    }
    if !longest.is_empty() {
        if let Ok(wlen) = pdftools::get_text_length(longest, "helv", fs) {
            if wlen > w && w > 0.0 {
                fs *= w / wlen;
            }
        }
    }
    minsize.max(fs.min(maxsize))
}

/// `_draw_signature_stamp(page, rect, signer, doc, tz)`.
fn draw_signature_stamp(
    pdf: &mut pdftools::Document,
    pno: usize,
    rect: Rect,
    signer: Option<&SignSigner>,
    verify_url: &str,
    tz: chrono_tz::Tz,
) -> Result<(), String> {
    let Some(signer) = signer else { return Ok(()) };
    let Some(sig) = decode_data_url(signer.signature_data.as_deref()) else {
        return Ok(());
    };
    let signed_at = signer.signed_at.unwrap_or_else(Utc::now);
    let when = PyDateTime::from_utc_in_zone(signed_at.naive_utc(), tz);
    let caption = format!(
        "ID: {}\nReason: {}\n<{}>\n{}",
        signer.id,
        signer
            .sign_reason
            .as_deref()
            .filter(|r| !r.is_empty())
            .unwrap_or("Authorized by"),
        signer.email,
        strftime(&when, "%B %d, %Y %I:%M %p %Z"),
    );
    let lines: Vec<&str> = caption.split('\n').collect();
    let (bx0, by0, bw, bh) = (rect.x0, rect.y0, rect.width(), rect.height());
    let gap = 2.0;
    let qr_side = 0f64.max(bh.min(bw * 0.30).min(46.0));
    let content_w = 10f64.max(bw - qr_side - gap);
    let img_h = bh * 0.50;
    insert_fitted(
        pdf,
        pno,
        Rect::new(bx0, by0, bx0 + content_w, by0 + img_h),
        Some(&sig),
    );
    pdf.draw_line(
        pno,
        Point::new(bx0, by0 + img_h),
        Point::new(bx0 + content_w, by0 + img_h),
        Some(&[0.75, 0.75, 0.75]),
        0.3,
    )
    .map_err(|e| e.to_string())?;
    let cap_box = Rect::new(bx0, by0 + img_h + 0.5, bx0 + content_w, by0 + bh);
    let fs = fit_caption_fontsize(&lines, content_w - 1.0, 1f64.max(bh - img_h - 0.5));
    pdf.insert_textbox(
        pno,
        cap_box,
        &caption,
        &TextOptions {
            fontsize: fs,
            fontname: "helv".into(),
            color: Some(vec![0.2, 0.2, 0.2]),
            align: 0,
        },
    )
    .map_err(|e| e.to_string())?;
    if let Ok(qr) = qr_png(verify_url, 10, 1) {
        if qr_side > 4.0 {
            let qy = by0 + (bh - qr_side) / 2.0;
            insert_fitted(
                pdf,
                pno,
                Rect::new(
                    bx0 + content_w + gap,
                    qy,
                    bx0 + content_w + gap + qr_side,
                    qy + qr_side,
                ),
                Some(&qr),
            );
        }
    }
    Ok(())
}

/// The flattening part of `_generate_signed_pdf`: stamps every field onto `original` and returns
/// `pdf.tobytes(garbage=4, deflate=True, clean=True)`. `verify_url` is `f"{APP_URL}/verify/{doc.id}"`,
/// `tz` the company time zone. One bad field never aborts the document.
pub fn stamp_signed_pdf(
    original: &[u8],
    fields: &[StampField],
    verify_url: &str,
    tz: chrono_tz::Tz,
) -> Result<Vec<u8>, String> {
    let mut pdf = pdftools::Document::open(original).map_err(|e| e.to_string())?;
    let black = Some(vec![0.0, 0.0, 0.0]);
    for f in fields {
        let page_idx = f.page_number.filter(|p| *p != 0).unwrap_or(1) as i64 - 1;
        if page_idx < 0 || page_idx >= pdf.page_count() as i64 {
            continue;
        }
        let pno = page_idx as usize;
        let Ok(pr) = pdf.page_rect(pno) else { continue };
        let (pw, ph) = (pr.width(), pr.height());
        let x = f.x.unwrap_or(0.0) / 100.0 * pw;
        let y = f.y.unwrap_or(0.0) / 100.0 * ph;
        let w = f.width.unwrap_or(0.0) / 100.0 * pw;
        let h = f.height.unwrap_or(0.0) / 100.0 * ph;
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let rect = Rect::new(x, y, x + w, y + h);
        let _ = match f.field_type.as_str() {
            "signature" => {
                draw_signature_stamp(&mut pdf, pno, rect, f.signer.as_ref(), verify_url, tz)
            }
            "initials" => {
                let data = f.signer.as_ref().and_then(|s| {
                    s.initials_data
                        .as_deref()
                        .filter(|d| !d.is_empty())
                        .or(s.signature_data.as_deref())
                });
                insert_fitted(&mut pdf, pno, rect, decode_data_url(data).as_deref());
                Ok(())
            }
            "image" => {
                insert_fitted(
                    &mut pdf,
                    pno,
                    rect,
                    decode_data_url(f.value.as_deref()).as_deref(),
                );
                Ok(())
            }
            "checkbox" => {
                let v = f.value.as_deref().unwrap_or("").trim().to_lowercase();
                if matches!(v.as_str(), "true" | "1" | "yes" | "on") {
                    pdf.insert_textbox(
                        pno,
                        rect,
                        "X",
                        &TextOptions {
                            fontsize: h.min(12.0),
                            fontname: "helv".into(),
                            color: black.clone(),
                            align: 1,
                        },
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                } else {
                    Ok(())
                }
            }
            _ => {
                let text = f.value.as_deref().unwrap_or("").trim();
                if text.is_empty() {
                    Ok(())
                } else {
                    pdf.insert_textbox(
                        pno,
                        rect,
                        text,
                        &TextOptions {
                            fontsize: 10.0,
                            fontname: "helv".into(),
                            color: black.clone(),
                            align: 0,
                        },
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                }
            }
        };
    }
    pdf.tobytes(SaveOptions {
        garbage: 4,
        deflate: true,
        clean: true,
    })
    .map_err(|e| e.to_string())
}

/// `fitz.open(stream=out).tobytes(garbage=4, deflate=True, clean=True)`: the second clean re-serialisation
/// `_generate_signed_pdf` retries the seal with.
pub fn reserialize_pdf(pdf_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let pdf = pdftools::Document::open(pdf_bytes).map_err(|e| e.to_string())?;
    pdf.tobytes(SaveOptions {
        garbage: 4,
        deflate: true,
        clean: true,
    })
    .map_err(|e| e.to_string())
}

// ─── ZIP ──────────────────────────────────────────────────────────────────────

/// `zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED)` with one `writestr(name, data)` per entry (entries
/// stamped with the current local time, as zipfile does).
pub fn zip_deflated(entries: &[(String, Vec<u8>)], now: DateTime<chrono::Local>) -> Vec<u8> {
    use chrono::{Datelike, Timelike};
    let dos_time =
        ((now.hour() as u16) << 11) | ((now.minute() as u16) << 5) | ((now.second() / 2) as u16);
    let dos_date = (((now.year() - 1980).clamp(0, 127) as u16) << 9)
        | ((now.month() as u16) << 5)
        | now.day() as u16;
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in entries {
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
        let _ = enc.write_all(data);
        let compressed = enc.finish().unwrap_or_default();
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let crc = crc.sum();
        let name_bytes = name.as_bytes();
        let flags: u16 = if name.is_ascii() { 0 } else { 0x800 };
        let offset = out.len() as u32;
        // local file header
        out.extend_from_slice(&0x04034b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&dos_time.to_le_bytes());
        out.extend_from_slice(&dos_date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(&compressed);
        // central directory entry
        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&flags.to_le_bytes());
        central.extend_from_slice(&8u16.to_le_bytes());
        central.extend_from_slice(&dos_time.to_le_bytes());
        central.extend_from_slice(&dos_date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&(0o600u32 << 16).to_le_bytes()); // external attrs (writestr default)
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}
