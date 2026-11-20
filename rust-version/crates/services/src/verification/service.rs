//! Port of backend/app/services/verification/service.py.
//!
//! Issue, evaluate and revoke QR verifications for outbound documents.
//!
//! A verification is issued only for an approved document whose policy enables QR. The PDF is rendered
//! at issue time with a QR on every page pointing at /v/<token>. Validity is re-evaluated on every
//! scan, so cancelling the document, superseding its approval or changing it after approval invalidates
//! the paper immediately: no token has to be touched for that to take effect.
//!
//! Where Python mutates the ORM object and lets the session flush it at commit (`revoke`, the
//! active -> expired step of `evaluate`, the late fields of `issue`), this port writes the row at once
//! and updates the struct it was given; the database ends the transaction in the same state.
//!
//! The QR (`qr_data_uri`) is python-qrcode 8.0's `qrcode.make(url, border=1, box_size=10)`: the same
//! segments (`optimal_data_chunks(data, minimum=20)`), version (`best_fit`), error correction (M) and mask
//! (`best_mask_pattern`, python-qrcode's own `lost_point`, scored on the matrix with blank format bits as
//! Python does), drawn with the `qrcode` crate's canvas, and the same image (modules of 10 px, a 1-module
//! border). The PNG is 8-bit grayscale where Pillow writes 1-bit, so the bytes differ; the pixels do not.

use std::io::Cursor;

use base64::Engine as _;
use chrono::{DateTime, Duration, NaiveDate, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use granary_compat::crypto::tokens::{new_short_code as compat_short_code, OsRandom};
use granary_compat::pyjson::PyDateTime;
use granary_core::auth::AuthUser;
use granary_core::config::settings;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, SettingError};
use granary_db::rows::approval::ApprovalRequest;
use granary_db::rows::verification::DocumentVerification;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::adapters::{get_adapter, DocumentAdapter, Entity};
use crate::approvals::engine;
use crate::verification::render;

pub const VOID_STATUSES: [&str; 4] = ["cancelled", "void", "voided", "terminated"];
/// `_CODE_ALPHABET` (the generator lives in granary_compat::crypto::tokens).
pub const CODE_ALPHABET: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn now() -> DateTime<Utc> {
    Utc::now()
}

/// `verify_url(token)`.
pub fn verify_url(token: impl std::fmt::Display) -> String {
    format!("{}/v/{token}", settings().APP_URL.trim_end_matches('/'))
}

/// `new_short_code()`: 10 characters of `_CODE_ALPHABET` from the OS CSPRNG.
pub fn new_short_code() -> String {
    compat_short_code(&mut OsRandom)
}

// ─── QR ───────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QrMode {
    Number,
    AlphaNum,
    Byte,
}

const ALPHA_NUM: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

/// `_optimal_split(data, pattern)` with `pattern` = a byte class repeated `{minimum,}` (or `^class+$` for
/// short data).
fn optimal_split(
    data: &[u8],
    is_class: impl Fn(u8) -> bool,
    minimum: usize,
    anchored: bool,
) -> Vec<(bool, &[u8])> {
    let mut out = vec![];
    let mut rest = data;
    while !rest.is_empty() {
        let found = if anchored {
            // ^class+$ ($ also matches before a final newline)
            let run = rest.iter().take_while(|b| is_class(**b)).count();
            if run > 0 && (run == rest.len() || (run == rest.len() - 1 && rest[run] == b'\n')) {
                Some((0, run))
            } else {
                None
            }
        } else {
            let mut hit = None;
            let mut i = 0;
            while i < rest.len() {
                let run = rest[i..].iter().take_while(|b| is_class(**b)).count();
                if run >= minimum {
                    hit = Some((i, i + run));
                    break;
                }
                i += run.max(1);
            }
            hit
        };
        let Some((start, end)) = found else { break };
        if start > 0 {
            out.push((false, &rest[..start]));
        }
        out.push((true, &rest[start..end]));
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        out.push((false, rest));
    }
    out
}

/// `util.optimal_data_chunks(data, minimum)`.
fn optimal_data_chunks(data: &[u8], minimum: usize) -> Vec<(QrMode, Vec<u8>)> {
    let anchored = data.len() <= minimum;
    let mut out = vec![];
    for (is_num, chunk) in optimal_split(data, |b| b.is_ascii_digit(), minimum, anchored) {
        if is_num {
            out.push((QrMode::Number, chunk.to_vec()));
        } else {
            for (is_alpha, sub) in
                optimal_split(chunk, |b| ALPHA_NUM.contains(&b), minimum, anchored)
            {
                out.push((
                    if is_alpha {
                        QrMode::AlphaNum
                    } else {
                        QrMode::Byte
                    },
                    sub.to_vec(),
                ));
            }
        }
    }
    out
}

/// `util.lost_point(modules)` (python-qrcode's penalty, including its skip shortcuts).
fn lost_point(m: &[Vec<bool>]) -> i64 {
    let n = m.len();
    // level 1
    let mut container = vec![0i64; n + 1];
    for row in m {
        let mut prev = row[0];
        let mut length = 0usize;
        for &c in row {
            if c == prev {
                length += 1;
            } else {
                if length >= 5 {
                    container[length] += 1;
                }
                length = 1;
                prev = c;
            }
        }
        if length >= 5 {
            container[length] += 1;
        }
    }
    for col in 0..n {
        let mut prev = m[0][col];
        let mut length = 0usize;
        for row in m {
            if row[col] == prev {
                length += 1;
            } else {
                if length >= 5 {
                    container[length] += 1;
                }
                length = 1;
                prev = row[col];
            }
        }
        if length >= 5 {
            container[length] += 1;
        }
    }
    let mut lost: i64 = (5..=n).map(|l| container[l] * (l as i64 - 2)).sum();
    // level 2
    for r in 0..n - 1 {
        let (this, next) = (&m[r], &m[r + 1]);
        let mut col = 0;
        while col < n - 1 {
            let top_right = this[col + 1];
            if top_right != next[col + 1] {
                col += 1; // next(iter): skip the next block
            } else if top_right != this[col] || top_right != next[col] {
            } else {
                lost += 3;
            }
            col += 1;
        }
    }
    // level 3
    let finder = |g: &dyn Fn(usize) -> bool| -> bool {
        !g(1)
            && g(4)
            && !g(5)
            && g(6)
            && !g(9)
            && ((g(0) && g(2) && g(3) && !g(7) && !g(8) && !g(10))
                || (!g(0) && !g(2) && !g(3) && g(7) && g(8) && g(10)))
    };
    if n > 10 {
        for row in m {
            let mut col = 0;
            while col < n - 10 {
                if finder(&|k| row[col + k]) {
                    lost += 40;
                }
                if row[col + 10] {
                    col += 1;
                }
                col += 1;
            }
        }
        #[allow(clippy::needless_range_loop)] // the column scan of the square module matrix
        for col in 0..n {
            let mut row = 0;
            while row < n - 10 {
                if finder(&|k| m[row + k][col]) {
                    lost += 40;
                }
                if m[row + 10][col] {
                    row += 1;
                }
                row += 1;
            }
        }
    }
    // level 4
    let dark: usize = m.iter().map(|r| r.iter().filter(|c| **c).count()).sum();
    let percent = dark as f64 / (n * n) as f64;
    let rating = ((percent * 100.0 - 50.0).abs() / 5.0) as i64;
    lost + rating * 10
}

/// The QR of `data` as python-qrcode builds it: (version, mask pattern, modules by row).
pub fn qr_matrix(data: &[u8]) -> Result<(i16, u8, Vec<Vec<bool>>), ApiError> {
    use qrcode::bits::Bits;
    use qrcode::canvas::{Canvas, MaskPattern};
    use qrcode::types::{Color, EcLevel, Version};
    let err = |e: qrcode::types::QrError| ApiError::internal(anyhow::anyhow!("qrcode: {e:?}"));
    let chunks = optimal_data_chunks(data, 20);
    let ec = EcLevel::M;
    // best_fit: the smallest version whose data capacity holds the segments
    let mut chosen = None;
    for v in 1..=40i16 {
        let mut bits = Bits::new(Version::Normal(v));
        let mut ok = true;
        for (mode, chunk) in &chunks {
            let r = match mode {
                QrMode::Number => bits.push_numeric_data(chunk),
                QrMode::AlphaNum => bits.push_alphanumeric_data(chunk),
                QrMode::Byte => bits.push_byte_data(chunk),
            };
            if r.is_err() {
                ok = false;
                break;
            }
        }
        if ok && bits.len() <= bits.max_len(ec).map_err(err)? {
            chosen = Some((v, bits));
            break;
        }
    }
    let Some((v, mut bits)) = chosen else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "qrcode: DataOverflowError"
        )));
    };
    bits.push_terminator(ec).map_err(err)?;
    let version = Version::Normal(v);
    let (data_cw, ec_cw) =
        qrcode::ec::construct_codewords(&bits.into_bytes(), version, ec).map_err(err)?;
    let mut base = Canvas::new(version, ec);
    base.draw_all_functional_patterns();
    base.draw_data(&data_cw, &ec_cw);
    let n = (v * 4 + 17) as usize;
    let to_rows = |colors: Vec<Color>| -> Vec<Vec<bool>> {
        colors
            .chunks(n)
            .map(|r| r.iter().map(|c| *c == Color::Dark).collect())
            .collect()
    };
    let masks = [
        MaskPattern::Checkerboard,
        MaskPattern::HorizontalLines,
        MaskPattern::VerticalLines,
        MaskPattern::DiagonalLines,
        MaskPattern::LargeCheckerboard,
        MaskPattern::Fields,
        MaskPattern::Diamonds,
        MaskPattern::Meadow,
    ];
    // best_mask_pattern: makeImpl(test=True) leaves the format bits, the fixed dark module and the
    // version bits light; score that matrix.
    let mut best: Option<(i64, usize)> = None;
    for (i, mask) in masks.iter().enumerate() {
        let mut c = base.clone();
        c.apply_mask(*mask);
        let mut rows = to_rows(c.into_colors());
        for k in 0..15 {
            let r = if k < 6 {
                k
            } else if k < 8 {
                k + 1
            } else {
                n - 15 + k
            };
            rows[r][8] = false;
            let col = if k < 8 {
                n - k - 1
            } else if k < 9 {
                15 - k
            } else {
                15 - k - 1
            };
            rows[8][col] = false;
        }
        rows[n - 8][8] = false;
        if v >= 7 {
            for k in 0..18 {
                rows[k / 3][k % 3 + n - 11] = false;
                rows[k % 3 + n - 11][k / 3] = false;
            }
        }
        let lp = lost_point(&rows);
        if best.is_none() || best.unwrap().0 > lp {
            best = Some((lp, i));
        }
    }
    let mask = best.unwrap().1;
    let mut c = base;
    c.apply_mask(masks[mask]);
    Ok((v, mask as u8, to_rows(c.into_colors())))
}

/// The PNG of `qrcode.make(url, border=1, box_size=10)` (black modules on white).
pub fn qr_png(url: &str) -> Result<Vec<u8>, ApiError> {
    let (_, _, rows) = qr_matrix(url.as_bytes())?;
    let (border, boxs) = (1usize, 10usize);
    let size = (rows.len() + 2 * border) * boxs;
    let mut img = image::GrayImage::from_pixel(size as u32, size as u32, image::Luma([255u8]));
    for (r, row) in rows.iter().enumerate() {
        for (c, dark) in row.iter().enumerate() {
            if *dark {
                for y in 0..boxs {
                    for x in 0..boxs {
                        img.put_pixel(
                            ((c + border) * boxs + x) as u32,
                            ((r + border) * boxs + y) as u32,
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

/// `qr_data_uri(url)`.
pub fn qr_data_uri(url: &str) -> Result<String, ApiError> {
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(qr_png(url)?)
    ))
}

// ─── Storage (MinIO, documents bucket) ────────────────────────────────────────
// Python uses the `minio` package; here the shared `crate::storage` client (path-style SigV4, region
// us-east-1). Any S3 failure is an error the caller maps (issue: a 502).

/// `put_pdf(key, data)`: creates the documents bucket when missing, then stores the PDF.
pub async fn put_pdf(key: &str, data: &[u8]) -> anyhow::Result<()> {
    let bucket = settings().MINIO_BUCKET_DOCUMENTS.clone();
    let store = crate::storage::minio();
    crate::storage::ensure_bucket(&*store, &bucket).await?;
    store
        .put_object(&bucket, key, data, "application/pdf")
        .await?;
    Ok(())
}

/// `get_pdf(key)`.
pub async fn get_pdf(key: &str) -> anyhow::Result<Vec<u8>> {
    let bucket = settings().MINIO_BUCKET_DOCUMENTS.clone();
    Ok(crate::storage::minio().get_object(&bucket, key).await?)
}

// ─── Lifecycle ────────────────────────────────────────────────────────────────

/// `link_expiry(adapter, entity, policy, now, tz)`: a document with its own end date (quotation
/// valid-until, contract end, offer deadline) keeps a live link through the end of the following day,
/// company time (`tz`); anything else follows the policy TTL (`policy.token_ttl_days`).
pub fn link_expiry(
    adapter: &DocumentAdapter,
    entity: &Entity,
    token_ttl_days: Option<i32>,
    now: DateTime<Utc>,
    tz: Tz,
) -> Option<DateTime<Utc>> {
    link_expiry_from(adapter.expiry_date(entity), token_ttl_days, now, tz)
}

/// `link_expiry` given `adapter.expiry_date(entity)`.
pub fn link_expiry_from(
    ends: Option<NaiveDate>,
    token_ttl_days: Option<i32>,
    now: DateTime<Utc>,
    tz: Tz,
) -> Option<DateTime<Utc>> {
    if let Some(ends) = ends {
        let last_day = ends + Duration::days(1);
        let local = last_day.and_hms_opt(23, 59, 59).unwrap();
        // datetime(..., tzinfo=ZoneInfo): fold=0, the earlier of two ambiguous times
        // (a wall time in a DST gap, which the company zones never have, is read with the offset in force
        // a day earlier, as ZoneInfo's fold=0 uses the offset before the transition)
        let at = tz
            .from_local_datetime(&local)
            .earliest()
            .unwrap_or_else(|| {
                let before = tz
                    .offset_from_utc_datetime(&(local - Duration::days(1)))
                    .fix()
                    .local_minus_utc();
                tz.from_utc_datetime(&(local - Duration::seconds(before as i64)))
            });
        return Some(at.with_timezone(&Utc));
    }
    match token_ttl_days {
        Some(d) if d != 0 => Some(now + Duration::days(d as i64)),
        _ => None,
    }
}

fn py_slice(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

async fn fetch(db: &mut PgConnection, id: Uuid) -> Result<DocumentVerification, ApiError> {
    Ok(
        sqlx::query_as("SELECT * FROM document_verifications WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *db)
            .await?,
    )
}

/// `issue(db, entity_type=, entity_id=, user=)`.
pub async fn issue(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: &str,
    user: &AuthUser,
) -> Result<DocumentVerification, ApiError> {
    let adapter = get_adapter(entity_type)?;
    if !render::supports(entity_type) {
        return Err(ApiError::http(
            400,
            format!(
                "{} documents are not issued with a verification QR",
                adapter.label
            ),
        ));
    }
    let Some(entity) = render::load(db, entity_type, entity_id).await? else {
        return Err(ApiError::http(404, format!("{} not found", adapter.label)));
    };
    let lower = adapter.label.to_lowercase();
    let policy = match engine::get_policy(db, entity_type).await? {
        Some(p) if p.row.qr_enabled => p,
        _ => {
            return Err(ApiError::http(
                400,
                format!("QR verification is not enabled for {lower}s"),
            ))
        }
    };
    let status = entity.status().unwrap_or("").to_string();
    if VOID_STATUSES.contains(&status.as_str()) || status == "superseded" {
        return Err(ApiError::http(
            400,
            format!("A {status} {lower} cannot be issued"),
        ));
    }
    let request =
        engine::ensure_approved(db, entity_type, &entity, "issued with a verification QR").await?;

    let t = now();
    let tz = granary_core::clock::local_tz(db).await?;
    let id = Uuid::new_v4();
    let token = Uuid::new_v4();
    let view_scope = Some(policy.row.view_scope.clone())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "metadata".into());
    sqlx::query(
        "INSERT INTO document_verifications (id, entity_type, entity_id, revision, document_number, document_title, \
         token, short_code, status, issued_at, expires_at, view_scope, allow_download, requires_login, facts, \
         approval_request_id, issued_by_id, view_count) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'active', $9, $10, $11, $12, $13, '{}'::jsonb, $14, $15, 0)",
    )
    .bind(id)
    .bind(entity_type)
    .bind(entity.id())
    .bind(adapter.revision(&entity))
    .bind(py_slice(&adapter.number(&entity), 100))
    .bind(py_slice(&adapter.document_label(&entity), 255))
    .bind(token)
    .bind(new_short_code())
    .bind(t)
    .bind(link_expiry(adapter, &entity, policy.row.token_ttl_days, t, tz))
    .bind(&view_scope)
    .bind(policy.row.allow_download)
    .bind(policy.row.requires_login)
    .bind(request.as_ref().map(|r| r.id))
    .bind(user.id)
    .execute(&mut *db)
    .await?;
    let verification = fetch(db, id).await?;

    let (template, mut data, facts) =
        render::build(db, entity_type, &entity, request.as_ref()).await?;
    let footer = json!({
        "qr_data_uri": qr_data_uri(&verify_url(verification.token))?,
        "title": format!("{} {}", adapter.label, adapter.number(&entity)),
        "line": "Scan the QR code to confirm this document is authentic and current.",
        "code": format!("Verification code {}", verification.short_code),
    });
    // header = {"text": data.pop("pdf_header")} if data.get("pdf_header") else None
    let header = if render_truthy(data.get("pdf_header")) {
        let text = data
            .as_object_mut()
            .and_then(|m| m.shift_remove("pdf_header"))
            .unwrap_or(Value::Null);
        Some(json!({"text": text}))
    } else {
        None
    };
    let pdf = match crate::pdf::generate_pdf(&template, &data, Some(&footer), header.as_ref()).await
    {
        Ok(pdf) => pdf,
        Err(exc) => {
            tracing::error!(entity_type, entity_id = %entity.id(), error = %exc, "verification_render_failed");
            // 10 Nov 2026: 503, not 502 (Cloudflare replaces an origin 502 with its own page), and the reason
            let why = match &exc {
                crate::whatsapp::HttpxError::Timeout(_) => {
                    "the PDF service did not answer in time".to_string()
                }
                crate::whatsapp::HttpxError::Status { status, body, .. } => {
                    serde_json::from_slice::<Value>(body)
                        .ok()
                        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_else(|| format!("the PDF service answered with error {status}"))
                }
                other => other.to_string(),
            };
            return Err(ApiError::http(
                503,
                format!("The document could not be rendered: {why}. Try again shortly; if it keeps failing, tell the admin."),
            ));
        }
    };
    let document_hash = hex::encode(Sha256::digest(&pdf));
    let key = format!("verification/{entity_type}/{id}.pdf");
    if let Err(exc) = put_pdf(&key, &pdf).await {
        tracing::error!(key = %key, error = %exc, "verification_store_failed");
        return Err(ApiError::http(
            502,
            "The rendered document could not be stored: try again shortly",
        ));
    }
    sqlx::query(
        "UPDATE document_verifications SET facts = $2, document_hash = $3, pdf_object_key = $4, rendered_at = $5, \
         updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(json!({"items": facts}))
    .bind(&document_hash)
    .bind(&key)
    .bind(now())
    .execute(&mut *db)
    .await?;

    sqlx::query(
        "UPDATE document_verifications SET status = 'superseded', superseded_by_id = $3, updated_at = now() \
         WHERE entity_type = $1 AND entity_id = $2 AND status = 'active' AND id != $3",
    )
    .bind(entity_type)
    .bind(entity.id())
    .bind(id)
    .execute(&mut *db)
    .await?;
    // the outbound copy is sealed now, at approval time (a failure alerts the Owner and is retried on send)
    let mut issued = fetch(db, id).await?;
    let _ = outbound_pdf(db, &mut issued).await?;
    Ok(issued)
}

// ─── Outbound seal ────────────────────────────────────────────────────────────

/// `documents.seal.outbound`: seal the copies that leave the ERP.
pub const SEAL_SETTING: &str = "documents.seal.outbound";

async fn seal_enabled(db: &mut PgConnection) -> ApiResult<bool> {
    match get_optional(db, SEAL_SETTING).await? {
        Some(v) => Ok(v.trim().eq_ignore_ascii_case("true")),
        None => Err(SettingError::new(
            SEAL_SETTING,
            "is not set",
            "Approvals & signing \u{2192} E-signing",
        )
        .into()),
    }
}

/// The key of a verification's sealed outbound copy, next to its master.
pub fn sealed_key(v: &DocumentVerification) -> String {
    format!("verification/{}/{}.sealed.pdf", v.entity_type, v.id)
}

type SealRow = (
    Option<String>,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<String>,
);

/// The PDF that leaves the ERP for a verification (email, WhatsApp, the public view and download, staff
/// download), and whether it is sealed. `pdf_object_key` stays the unsealed master (e-sign and the MCP client's
/// page renderer read it); the sealed copy (PAdES certification, DocMDP P=2: a customer can still fill and
/// sign it) is stored next to it the first time it is needed. Setting off: the master. Sealing never
/// blocks sending: on a failure the master goes out (the QR still protects it) and the Owner is alerted once.
pub async fn outbound_pdf(
    db: &mut PgConnection,
    v: &mut DocumentVerification,
) -> ApiResult<(Vec<u8>, bool)> {
    let Some(master_key) = v.pdf_object_key.clone().filter(|k| !k.is_empty()) else {
        return Err(ApiError::http(
            404,
            "No rendered document for this verification",
        ));
    };
    if !seal_enabled(db).await? {
        let master = get_pdf(&master_key).await.map_err(ApiError::internal)?;
        return Ok((master, false));
    }
    if let Some(k) = v.sealed_object_key.clone().filter(|k| !k.is_empty()) {
        return Ok((get_pdf(&k).await.map_err(ApiError::internal)?, true));
    }
    let master = get_pdf(&master_key).await.map_err(ApiError::internal)?;
    if v.status != "active" {
        // superseded, revoked, expired or void copies are never sent: only active ones are sealed
        return Ok((master, false));
    }
    // one seal per verification: a concurrent send waits here, then uses the copy the first one stored
    let row: Option<SealRow> = sqlx::query_as(
        "SELECT sealed_object_key, sealed_hash, sealed_at, seal_level FROM document_verifications \
         WHERE id = $1 FOR UPDATE",
    )
    .bind(v.id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((Some(k), hash, at, level)) = row {
        if !k.is_empty() {
            v.sealed_object_key = Some(k.clone());
            v.sealed_hash = hash;
            v.sealed_at = at;
            v.seal_level = level;
            return Ok((get_pdf(&k).await.map_err(ApiError::internal)?, true));
        }
    }
    let key = sealed_key(v);
    let sealed =
        match crate::pdf_sign::sign_pdf_with(&master, crate::pdf_sign::DocMdp::FillAndSign).await {
            Ok((bytes, level)) => match put_pdf(&key, &bytes).await {
                Ok(()) => Ok((bytes, level)),
                Err(e) => Err(format!("the sealed copy could not be stored: {e}")),
            },
            Err(e) => Err(e.to_string()),
        };
    let (bytes, level) = match sealed {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(verification_id = %v.id, error = %e, "outbound_seal_failed");
            alert_seal_failure(db, v, &e).await;
            return Ok((master, false));
        }
    };
    let hash = hex::encode(Sha256::digest(&bytes));
    let at = now();
    sqlx::query(
        "UPDATE document_verifications SET sealed_object_key = $2, sealed_hash = $3, sealed_at = $4, \
         seal_level = $5, updated_at = now() WHERE id = $1 AND sealed_object_key IS NULL",
    )
    .bind(v.id)
    .bind(&key)
    .bind(&hash)
    .bind(at)
    .bind(&level)
    .execute(&mut *db)
    .await?;
    v.sealed_object_key = Some(key);
    v.sealed_hash = Some(hash);
    v.sealed_at = Some(at);
    v.seal_level = Some(level);
    Ok((bytes, true))
}

/// One Owner alert per verification that went out without its seal (best effort: never fails the send).
async fn alert_seal_failure(db: &mut PgConnection, v: &DocumentVerification, error: &str) {
    let key = format!("documents.seal:{}", v.id);
    let seen: Result<Option<(i32,)>, sqlx::Error> = sqlx::query_as(
        "SELECT 1 FROM notifications WHERE type = 'nary_error' AND entity_id = $1 LIMIT 1",
    )
    .bind(&key)
    .fetch_optional(&mut *db)
    .await;
    if !matches!(seen, Ok(None)) {
        return;
    }
    let adapter = get_adapter(&v.entity_type).ok();
    let label = adapter
        .map(|a| a.label.to_string())
        .unwrap_or_else(|| v.entity_type.clone());
    let link = adapter
        .map(|a| a.link_for_id(&v.entity_id.to_string()))
        .unwrap_or_default();
    let body = format!(
        "{label} {} was sent or downloaded without the company's digital seal, because sealing failed: {}. \
         Its QR code still verifies it. The ERP tries the seal again on the next send.",
        v.document_number.as_deref().unwrap_or_default(),
        py_slice(error, 300)
    );
    if let Err(e) = crate::nary::governance::error_card(
        db,
        &key,
        "A document went out without its digital seal",
        &body,
        &link,
    )
    .await
    {
        tracing::warn!(verification_id = %v.id, error = %e, "outbound_seal_alert_failed");
    }
}

fn render_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `revoke(db, verification, user, reason)`.
pub async fn revoke(
    db: &mut PgConnection,
    verification: &mut DocumentVerification,
    user: &AuthUser,
    reason: &str,
) -> Result<(), ApiError> {
    if verification.status != "active" {
        return Err(ApiError::http(
            409,
            format!("This verification is already {}", verification.status),
        ));
    }
    let t = now();
    sqlx::query(
        "UPDATE document_verifications SET status = 'revoked', revoked_at = $2, revoked_by_id = $3, \
         revoke_reason = $4, updated_at = now() WHERE id = $1",
    )
    .bind(verification.id)
    .bind(t)
    .bind(user.id)
    .bind(reason)
    .execute(&mut *db)
    .await?;
    verification.status = "revoked".into();
    verification.revoked_at = Some(t);
    verification.revoked_by_id = Some(user.id);
    verification.revoke_reason = Some(reason.to_string());
    Ok(())
}

/// `supersede_for_entity(db, entity_type, entity_id)`: a revision / cancellation of the document retires
/// every active code printed for it.
pub async fn supersede_for_entity(
    db: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE document_verifications SET status = 'superseded', updated_at = now() \
         WHERE entity_type = $1 AND entity_id = $2 AND status = 'active'",
    )
    .bind(entity_type)
    .bind(entity_id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// `evaluate(db, verification)`: valid | revoked | superseded | expired | void | not_approved.
pub async fn evaluate(
    db: &mut PgConnection,
    verification: &mut DocumentVerification,
) -> Result<String, ApiError> {
    if verification.status == "revoked" || verification.status == "superseded" {
        return Ok(verification.status.clone());
    }
    if verification.status == "expired" || verification.expires_at.is_some_and(|e| e < now()) {
        if verification.status == "active" {
            sqlx::query("UPDATE document_verifications SET status = 'expired', updated_at = now() WHERE id = $1")
                .bind(verification.id)
                .execute(&mut *db)
                .await?;
            verification.status = "expired".into();
        }
        return Ok("expired".into());
    }
    let entity = get_adapter(&verification.entity_type)?
        .load(db, &verification.entity_id.to_string())
        .await?;
    let status = entity
        .as_ref()
        .and_then(|e| e.status())
        .unwrap_or("")
        .to_string();
    let Some(entity) = entity.filter(|_| !VOID_STATUSES.contains(&status.as_str())) else {
        return Ok("void".into());
    };
    if status == "superseded" {
        return Ok("superseded".into());
    }
    if engine::get_policy(db, &verification.entity_type)
        .await?
        .is_some()
    {
        match engine::ensure_approved(db, &verification.entity_type, &entity, "verified").await {
            Ok(_) => {}
            Err(ApiError::Http { .. }) => return Ok("not_approved".into()),
            Err(e) => return Err(e),
        }
    }
    Ok("valid".into())
}

// ─── Serializers ──────────────────────────────────────────────────────────────

/// `_iso(value)`: `value.isoformat()` of an aware UTC datetime, or None.
fn iso(value: Option<DateTime<Utc>>) -> Value {
    match value {
        Some(v) => Value::String(PyDateTime::from_chrono_utc(&v).isoformat()),
        None => Value::Null,
    }
}

/// `internal_dict(v, include_token=, live_state=)`.
pub fn internal_dict(
    v: &DocumentVerification,
    include_token: bool,
    live_state: Option<&str>,
) -> Value {
    json!({
        "id": v.id.to_string(), "entity_type": v.entity_type, "entity_id": v.entity_id.to_string(),
        "revision": v.revision, "document_number": v.document_number, "document_title": v.document_title,
        "status": v.status, "live_state": live_state, "short_code": v.short_code,
        "token": if include_token { Some(v.token.to_string()) } else { None },
        "url": if include_token { Some(verify_url(v.token)) } else { None },
        "issued_at": iso(Some(v.issued_at)), "expires_at": iso(v.expires_at),
        "revoked_at": iso(v.revoked_at), "revoke_reason": v.revoke_reason,
        "view_count": v.view_count, "last_viewed_at": iso(v.last_viewed_at),
        "document_hash": v.document_hash, "view_scope": v.view_scope,
        "allow_download": v.allow_download, "requires_login": v.requires_login,
        "has_pdf": v.pdf_object_key.as_deref().is_some_and(|k| !k.is_empty()),
    })
}

/// `public_payload(db, v, state)`.
pub async fn public_payload(
    db: &mut PgConnection,
    v: &DocumentVerification,
    state: &str,
) -> Result<Value, ApiError> {
    let adapter = get_adapter(&v.entity_type)?;
    let request: Option<ApprovalRequest> = match v.approval_request_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM approval_requests WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let mut signature = Value::Null;
    if let Some(sid) = v.sign_document_id {
        let doc: Option<(String, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>)> =
            sqlx::query_as("SELECT status, completed_at, sealed_at, signature_level FROM sign_documents WHERE id = $1")
                .bind(sid)
                .fetch_optional(&mut *db)
                .await?;
        if let Some((status, completed_at, sealed_at, level)) = doc {
            signature = json!({"status": status, "completed_at": iso(completed_at),
                               "digitally_sealed": sealed_at.is_some(),
                               "signature_level": level.filter(|l| !l.is_empty()).unwrap_or_else(|| "ses".into())});
        }
    }
    let valid = state == "valid";
    let has_pdf = v.pdf_object_key.as_deref().is_some_and(|k| !k.is_empty());
    let facts = v.facts.get("items").cloned().unwrap_or_else(|| json!([]));
    Ok(json!({
        "state": state, "valid": valid,
        "issuer": render::company_name(db).await?,
        "entity_type": v.entity_type, "entity_label": adapter.label,
        "document_number": v.document_number, "document_title": v.document_title, "revision": v.revision,
        "issued_at": iso(Some(v.issued_at)), "expires_at": iso(v.expires_at), "revoked_at": iso(v.revoked_at),
        "facts": facts,
        "approvals": render::approvals_block(db, request.as_ref()).await?,
        "signature": signature,
        // the outbound copy's seal (3 Nov 2026): the PDF sent and downloaded carries the company's certification
        "digitally_sealed": v.sealed_at.is_some(),
        "seal_level": v.seal_level.clone().filter(|l| !l.is_empty()),
        "sealed_hash": v.sealed_hash,
        "document_hash": v.document_hash, "hash_algorithm": "SHA-256",
        "can_view_document": valid && v.view_scope == "metadata_document" && has_pdf,
        "allow_download": valid && v.allow_download && has_pdf,
    }))
}
