//! 1 Nov 2026 (Owner decisions, migration 0145): videos your AI client (MCP) or Nary upload and send to customers.
//!
//! - sniff: MP4 (isom / mp42 brands), MOV (`qt  `), WebM; HEIC, AVIF and Matroska refused. The length is read
//!   from the file (MP4 mvhd version 0 and 1, WebM Info/Duration as a 4 or 8 byte float with its scale).
//! - upload_file: a video is a CRM attachment (file_type video) with its duration stored and returned; videos
//!   have their own cap (nary.upload.video_max_mb), images and documents keep nary.upload.max_mb.
//! - WhatsApp: an MP4 up to nary.wa.video_max_mb goes as a playable video (no asDocument; wa_messages type
//!   video, outbound log kind video); a larger one goes as a document (asDocument); videos do not count toward
//!   nary.attach.max_total_mb; a video over the upload cap is refused. The card says size, length and how.
//! - Email: a stored file over nary.attach.max_total_mb goes as a signed download link valid nary.email.link_days,
//!   one line in the body; a small file is still attached.
//! - MCP: upload_file over MCP marks the file via MCP; at full power the parked final version names the
//!   video with its size and length. The chat path and the confirm are the same tool and the same send.
//!
//! Fixtures are tiny synthetic MP4 / WebM byte strings built here, not real media.

mod nary_n2_common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine as _;
use granary_core::auth::AuthUser;
use granary_services::ai::mcp_access;
use granary_services::ai::tools::catalog;
use granary_services::email::{self as email_service, Seams, SendCall, SmtpConfig};
use granary_services::nary::{executor, uploads};
use granary_services::storage::{self, ObjectStore, StoreError};
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::ResponseTemplate;

const TAG: &str = "nary_video_files";
const MB: usize = 1024 * 1024;

// ─── synthetic media ─────────────────────────────────────────────────────────

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// An ISO media file: ftyp (major brand + compatible brands), moov/mvhd (version 0 or 1) with an H.264 (or
/// another codec's) sample entry tag, then an mdat padding it to `total` bytes.
fn iso(
    major: &[u8; 4],
    compat: &[&[u8; 4]],
    v1: bool,
    timescale: u32,
    duration: u64,
    codec: &[u8; 4],
    total: usize,
) -> Vec<u8> {
    let mut ftyp = major.to_vec();
    ftyp.extend_from_slice(&[0, 0, 2, 0]);
    for c in compat {
        ftyp.extend_from_slice(*c);
    }
    let mut out = boxed(b"ftyp", &ftyp);
    let mut mvhd = Vec::new();
    if v1 {
        mvhd.extend_from_slice(&[1, 0, 0, 0]);
        mvhd.extend_from_slice(&[0; 16]); // creation, modification
        mvhd.extend_from_slice(&timescale.to_be_bytes());
        mvhd.extend_from_slice(&duration.to_be_bytes());
    } else {
        mvhd.extend_from_slice(&[0, 0, 0, 0]);
        mvhd.extend_from_slice(&[0; 8]);
        mvhd.extend_from_slice(&timescale.to_be_bytes());
        mvhd.extend_from_slice(&(duration as u32).to_be_bytes());
    }
    mvhd.resize(mvhd.len() + 80, 0);
    let mut moov = boxed(b"mvhd", &mvhd);
    // trak/mdia/minf/stbl/stsd with one sample entry of `codec`
    let stsd = boxed(b"stsd", &[&[0u8; 8][..], &boxed(codec, &[0; 16])].concat());
    moov.extend(boxed(
        b"trak",
        &boxed(b"mdia", &boxed(b"minf", &boxed(b"stbl", &stsd))),
    ));
    out.extend(boxed(b"moov", &moov));
    let pad = total.saturating_sub(out.len() + 8);
    out.extend(boxed(b"mdat", &vec![0x11; pad]));
    out
}

/// An MP4 (isom, H.264) of `seconds` and `total` bytes.
fn mp4(seconds: u32, total: usize) -> Vec<u8> {
    iso(
        b"isom",
        &[b"isom", b"iso2", b"avc1", b"mp41"],
        false,
        1000,
        u64::from(seconds) * 1000,
        b"avc1",
        total,
    )
}

/// A WebM: EBML header (DocType webm), a Segment of unknown size, Info with TimecodeScale and Duration.
fn webm(
    scale: u32,
    duration_f64: Option<f64>,
    duration_f32: Option<f32>,
    doctype: &[u8],
) -> Vec<u8> {
    let mut header_body = vec![0x42, 0x82, 0x80 | doctype.len() as u8];
    header_body.extend_from_slice(doctype);
    let mut v = vec![0x1A, 0x45, 0xDF, 0xA3, 0x80 | header_body.len() as u8];
    v.extend(header_body);
    let mut info = vec![0x2A, 0xD7, 0xB1, 0x84];
    info.extend_from_slice(&scale.to_be_bytes());
    if let Some(d) = duration_f64 {
        info.extend_from_slice(&[0x44, 0x89, 0x88]);
        info.extend_from_slice(&d.to_be_bytes());
    }
    if let Some(d) = duration_f32 {
        info.extend_from_slice(&[0x44, 0x89, 0x84]);
        info.extend_from_slice(&d.to_be_bytes());
    }
    // Segment, unknown size
    v.extend_from_slice(&[
        0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ]);
    // a SeekHead first (skipped), then Info
    v.extend_from_slice(&[0x11, 0x4D, 0x9B, 0x74, 0x82, 0xEC, 0x80]);
    v.extend_from_slice(&[0x15, 0x49, 0xA9, 0x66, 0x80 | info.len() as u8]);
    v.extend(info);
    // a Cluster of padding
    v.extend_from_slice(&[0x1F, 0x43, 0xB6, 0x75, 0x84, 0, 0, 0, 0]);
    v
}

// ─── fixtures ────────────────────────────────────────────────────────────────

#[derive(Default)]
struct MemStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait]
impl ObjectStore for MemStore {
    async fn bucket_exists(&self, _b: &str) -> Result<bool, StoreError> {
        Ok(true)
    }
    async fn make_bucket(&self, _b: &str) -> Result<(), StoreError> {
        Ok(())
    }
    async fn put_object(
        &self,
        _b: &str,
        key: &str,
        data: &[u8],
        _ct: &str,
    ) -> Result<(), StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .insert(key.to_string(), data.to_vec());
        Ok(())
    }
    async fn get_object(&self, _b: &str, key: &str) -> Result<Vec<u8>, StoreError> {
        self.blobs
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(StoreError {
                kind: "S3Error".into(),
                message: key.into(),
            })
    }
}

fn store() -> Arc<MemStore> {
    let s = Arc::new(MemStore::default());
    storage::set_store(Some(s.clone()));
    s
}

fn cleanup() {
    storage::set_store(None);
    email_service::set_global_seams(None);
}

fn b64(d: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(d)
}

fn media_route() -> Arc<Mutex<Vec<Value>>> {
    let got: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(vec![]));
    let rec = got.clone();
    route("POST", "/send-media", move |req| {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let to = body["to"].as_str().unwrap_or("").to_string();
        rec.lock().unwrap().push(body);
        ResponseTemplate::new(200).set_body_json(json!({"success": true, "chatId": to,
            "messageId": format!("true_{to}_{}", Uuid::new_v4().simple().to_string()[..16].to_uppercase())}))
    });
    got
}

type Mails = Arc<Mutex<Vec<SendCall>>>;

fn smtp() -> Mails {
    let sent: Mails = Arc::new(Mutex::new(vec![]));
    let rec = sent.clone();
    email_service::set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(move |call: SendCall| {
            rec.lock().unwrap().push(call);
            Box::pin(async { Ok((true, None)) })
        })),
        load_system_smtp: None,
        load_crm_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    user: "u".into(),
                    password: "p".into(),
                    from_name: "Your Company Ltd".into(),
                    from_email: "hello@example.com".into(),
                    tls: true,
                })
            })
        })),
    }));
    sent
}

async fn the_owner(pool: &PgPool) -> AuthUser {
    let id = owner(pool).await;
    let mut c = pool.acquire().await.unwrap();
    auth_user(&mut c, id).await
}

async fn tool(pool: &PgPool, user: &AuthUser, name: &str, args: Value) -> Value {
    let mut c = pool.acquire().await.unwrap();
    catalog::run_tool(&mut c, user, name, args).await
}

async fn deal(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, value, currency, probability, is_deleted) \
         VALUES ($1, $2, 'discovery', 0, 'JPY', 10, false)",
    )
    .bind(id)
    .bind(u("Video deal"))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn customer_chat(pool: &PgPool) -> String {
    let cid = chat_id();
    ingest(
        pool,
        &payload(
            &cid,
            "Hello, is there a tutorial video?",
            false,
            None,
            None,
            json!({}),
        ),
        "live",
    )
    .await;
    cid
}

async fn upload(pool: &PgPool, user: &AuthUser, d: Uuid, name: &str, data: &[u8]) -> Value {
    tool(
        pool,
        user,
        "upload_file",
        json!({"entity_type": "deal", "entity_id": d.to_string(), "file_name": name, "content_base64": b64(data)}),
    )
    .await
}

fn error_of(out: &Value) -> String {
    out["error"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error: {out}"))
        .to_string()
}

// ─── sniff and length (no database) ──────────────────────────────────────────

#[test]
fn test_video_sniff_accepts_mp4_mov_webm_and_refuses_heic_avif_mkv() {
    let m = uploads::sniff(&mp4(60, 4096)).unwrap();
    assert_eq!((m.mime, m.ext, m.label), ("video/mp4", "mp4", "MP4 video"));
    // a phone recording's mp42 major brand
    let m = uploads::sniff(&iso(
        b"mp42",
        &[b"mp42", b"isom"],
        false,
        600,
        600,
        b"avc1",
        512,
    ))
    .unwrap();
    assert_eq!(m.mime, "video/mp4");
    let mov = uploads::sniff(&iso(b"qt  ", &[b"qt  "], true, 600, 600 * 90, b"avc1", 512)).unwrap();
    assert_eq!((mov.mime, mov.ext), ("video/quicktime", "mov"));
    let w = uploads::sniff(&webm(1_000_000, Some(42_500.0), None, b"webm")).unwrap();
    assert_eq!(
        (w.mime, w.ext, w.label),
        ("video/webm", "webm", "WebM video")
    );
    let heic = uploads::sniff(&iso(
        b"heic",
        &[b"mif1", b"heic"],
        false,
        1,
        1,
        b"hvc1",
        256,
    ))
    .unwrap_err();
    assert!(heic.contains("HEIC or AVIF"), "{heic}");
    assert!(uploads::sniff(&iso(
        b"avif",
        &[b"avif", b"mif1"],
        false,
        1,
        1,
        b"av01",
        256
    ))
    .unwrap_err()
    .contains("HEIC or AVIF"));
    assert!(uploads::sniff(&iso(b"mif1", &[b"mif1"], false, 1, 1, b"hvc1", 256)).is_err());
    // an M4A (audio) is no video
    assert!(uploads::sniff(&iso(b"M4A ", &[b"M4A "], false, 1, 1, b"mp4a", 256)).is_err());
    assert!(
        uploads::sniff(&webm(1_000_000, Some(1.0), None, b"matroska"))
            .unwrap_err()
            .contains("Matroska")
    );
    // the earlier types are still what they were
    assert_eq!(
        uploads::sniff(b"%PDF-1.7\n").unwrap().mime,
        "application/pdf"
    );
    assert!(uploads::ALLOWED_TEXT.contains("MP4, MOV or WebM videos"));
    let named = uploads::clean_name("Tutorial Acme", &m);
    assert_eq!(named, "Tutorial Acme.mp4");
    assert_eq!(uploads::type_label("video/mp4"), "MP4 video");
    assert!(uploads::is_video("video/quicktime") && !uploads::is_video("image/png"));
}

#[test]
fn test_video_duration_from_mp4_mvhd_and_webm_info() {
    assert_eq!(uploads::video_duration(&mp4(60, 4096)), Some(60.0));
    // version 1 mvhd (64-bit duration), QuickTime's 600 timescale
    let mov = iso(
        b"qt  ",
        &[b"qt  "],
        true,
        600,
        600 * 90 + 300,
        b"avc1",
        1024,
    );
    assert_eq!(uploads::video_duration(&mov), Some(90.5));
    // the moov after a large mdat (the usual phone recording layout)
    let mut tail = boxed(b"ftyp", b"isom\0\0\x02\0isomavc1");
    tail.extend(boxed(b"mdat", &vec![0; 100_000]));
    let moov_src = mp4(7, 0);
    let moov_at = moov_src.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let moov_len = u32::from_be_bytes(moov_src[moov_at..moov_at + 4].try_into().unwrap()) as usize;
    tail.extend_from_slice(&moov_src[moov_at..moov_at + moov_len]);
    assert_eq!(uploads::video_duration(&tail), Some(7.0));
    // WebM: 8-byte float in ms (default scale), 4-byte float with a 10 ms scale
    assert_eq!(
        uploads::video_duration(&webm(1_000_000, Some(42_500.0), None, b"webm")),
        Some(42.5)
    );
    let d = uploads::video_duration(&webm(10_000_000, None, Some(6_000.0), b"webm")).unwrap();
    assert!((d - 60.0).abs() < 1e-6, "{d}");
    // no moov, no Info, a truncated box: no length, never a panic
    assert_eq!(
        uploads::video_duration(&boxed(b"ftyp", b"isom\0\0\x02\0")),
        None
    );
    assert_eq!(
        uploads::video_duration(&webm(1_000_000, None, None, b"webm")),
        None
    );
    let cut = &mp4(5, 2048)[..60];
    assert_eq!(uploads::video_duration(cut), None);
    assert_eq!(uploads::video_duration(b"%PDF-1.7"), None);
    assert_eq!(uploads::duration_label(60.2), "1:00");
    assert_eq!(uploads::duration_label(42.5), "0:43");
    assert_eq!(uploads::duration_label(3723.0), "1:02:03");
    // WhatsApp plays H.264; an HEVC-only MP4 goes as a document
    assert!(uploads::mp4_plays_on_whatsapp(&mp4(5, 2048)));
    assert!(!uploads::mp4_plays_on_whatsapp(&iso(
        b"isom",
        &[b"isom"],
        false,
        1000,
        5000,
        b"hvc1",
        2048
    )));
}

// ─── upload_file ─────────────────────────────────────────────────────────────

#[test]
fn test_upload_file_stores_a_video_with_its_length_under_its_own_cap() {
    run(TAG, |pool| async move {
        let s = store();
        let owner = the_owner(&pool).await;
        let d = deal(&pool).await;
        // images and documents keep the 1 MB cap; a 2 MB video passes under the video cap
        let old = set(&pool, &[("nary.upload.max_mb", "1")]).await;
        let out = upload(&pool, &owner, d, "Tutorial Acme.mov", &mp4(60, 2 * MB)).await;
        let fid = out["file_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{out}"))
            .to_string();
        assert_eq!(
            out["file_name"], "Tutorial Acme.mp4",
            "the extension follows the content"
        );
        assert_eq!(out["mime_type"], "video/mp4");
        assert_eq!(out["type"], "MP4 video");
        assert_eq!(out["duration_seconds"], json!(60.0));
        assert_eq!(out["duration_label"], "1:00");
        assert!(
            out["note"]
                .as_str()
                .unwrap()
                .contains("nary.wa.video_max_mb"),
            "{out}"
        );
        let (ftype, dur, size): (String, Option<f64>, i32) = sqlx::query_as(
            "SELECT file_type, duration_seconds, file_size FROM attachments WHERE id = $1",
        )
        .bind(Uuid::parse_str(&fid).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (ftype.as_str(), dur, size as usize),
            ("video", Some(60.0), 2 * MB)
        );
        let png = {
            let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            v.resize(MB + 10, 7);
            v
        };
        let e = error_of(&upload(&pool, &owner, d, "big.png", &png).await);
        assert!(
            e.contains("nary.upload.max_mb") && !e.contains("video_max"),
            "{e}"
        );
        restore(&pool, old).await;
        // the video cap
        let old = set(&pool, &[("nary.upload.video_max_mb", "1")]).await;
        let e = error_of(&upload(&pool, &owner, d, "long.mp4", &mp4(90, 2 * MB)).await);
        assert!(
            e.contains("The video is 2.0 MB, over the 1.0 MB")
                && e.contains("nary.upload.video_max_mb"),
            "{e}"
        );
        restore(&pool, old).await;
        // a WebM and a MOV are stored as what they are
        let w = upload(
            &pool,
            &owner,
            d,
            "clip",
            &webm(1_000_000, Some(42_500.0), None, b"webm"),
        )
        .await;
        assert_eq!(
            (w["file_name"].as_str(), w["duration_seconds"].as_f64()),
            (Some("clip.webm"), Some(42.5))
        );
        let heic = upload(
            &pool,
            &owner,
            d,
            "photo.heic",
            &iso(b"heic", &[b"mif1"], false, 1, 1, b"hvc1", 300),
        )
        .await;
        assert!(error_of(&heic).contains("HEIC"), "{heic}");
        // no video setting: the error names the key
        sqlx::query("DELETE FROM system_settings WHERE key = 'nary.upload.video_max_mb'")
            .execute(&pool)
            .await
            .unwrap();
        let e = error_of(&upload(&pool, &owner, d, "a.mp4", &mp4(3, 2048)).await);
        assert!(e.contains("nary.upload.video_max_mb"), "{e}");
        sqlx::query(
            "INSERT INTO system_settings (id, created_at, updated_at, key, value, value_type, category, label, \
             is_public, is_encrypted) VALUES (gen_random_uuid(), now(), now(), 'nary.upload.video_max_mb', '100', \
             'int', 'nary', 'x', false, false)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // refused files were not stored: the MP4 and the WebM only
        assert_eq!(s.blobs.lock().unwrap().len(), 2);
        cleanup();
    });
}

// ─── WhatsApp ────────────────────────────────────────────────────────────────

#[test]
fn test_whatsapp_sends_a_small_mp4_as_a_video_and_a_large_one_as_a_document() {
    run(TAG, |pool| async move {
        let _s = store();
        let media = media_route();
        working_hours(true);
        let owner = the_owner(&pool).await;
        let cid = customer_chat(&pool).await;
        let d = deal(&pool).await;
        // 16 MB in production; 1 MB here. Videos don't count toward the 1 MB message total either.
        let old = set(
            &pool,
            &[
                ("nary.wa.video_max_mb", "1"),
                ("nary.attach.max_total_mb", "1"),
            ],
        )
        .await;
        // over MCP: stored via MCP
        let args = mcp_access::mcp_args(
            "upload_file",
            json!({"entity_type": "deal", "entity_id": d.to_string(), "file_name": "Short.mp4",
                   "content_base64": b64(&mp4(20, 512 * 1024)), "_via": "nary"}),
        );
        let small = tool(&pool, &owner, "upload_file", args).await;
        let small_id = small["file_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{small}"))
            .to_string();
        let via: Option<String> =
            sqlx::query_scalar("SELECT uploaded_via FROM attachments WHERE id = $1")
                .bind(Uuid::parse_str(&small_id).unwrap())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(via.as_deref(), Some("mcp"));
        let big = upload(&pool, &owner, d, "Tutorial.mp4", &mp4(60, 3 * MB)).await;
        let big_id = big["file_id"].as_str().unwrap().to_string();
        let ask = json!({"channel": "whatsapp", "chat_id": cid, "text": "Here is the tutorial video, Mr.",
                         "attach": [{"type": "file", "id": small_id}, {"type": "file", "id": big_id}]});
        let out = tool(&pool, &owner, "propose_customer_message", ask.clone()).await;
        let p = out["proposal"].clone();
        assert!(p.is_object(), "{out}");
        assert_eq!(
            p["attachments"],
            json!([
                "Short.mp4 (512 KB, 0:20, MP4 video, goes as a playable video)",
                "Tutorial.mp4 (3.0 MB, 1:00, MP4 video, goes as a document: over the 1.0 MB WhatsApp plays as a video)"
            ])
        );
        let files = p["files"].as_array().unwrap();
        assert_eq!(files[0]["video"], json!(true));
        assert_eq!(files[0]["send_as"], "video");
        assert_eq!(files[0]["duration_seconds"], json!(20.0));
        assert_eq!(files[0]["duration_label"], "0:20");
        assert_eq!(files[1]["send_as"], "document");
        assert!(files[1]["thumbnail_url"].is_null());
        assert_eq!(p["attach"][0]["send_as"], "video");

        // MCP full power: the parked final version names both with size and length
        let full: Vec<String> = ["act", "owner", "full"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut c = pool.acquire().await.unwrap();
        let mut proposal = p.clone();
        let parked = mcp_access::handle_proposal(
            &mut c,
            &owner,
            &full,
            "propose_customer_message",
            &ask,
            &mut proposal,
            None,
        )
        .await
        .unwrap();
        drop(c);
        assert_eq!(parked["applied"], false, "{parked}");
        let final_ = parked["final_version"].as_str().unwrap_or_default();
        assert!(
            final_.contains("Tutorial.mp4 (3.0 MB, 1:00, MP4 video, goes as a document"),
            "{parked}"
        );
        assert!(
            media.lock().unwrap().is_empty(),
            "nothing goes at the proposal"
        );

        // the confirm (the card's Send, the WhatsApp "1", nary_apply all reach /nary/send)
        let mut c = pool.acquire().await.unwrap();
        let row = executor::send_on_instruction(
            &mut c,
            &owner,
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": p["text"], "attach": p["attach"]}),
            None,
        )
        .await
        .unwrap();
        drop(c);
        assert_eq!(row.status, "applied", "{:?}", row.result);
        let got = media.lock().unwrap().clone();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["mimetype"], "video/mp4");
        assert!(
            got[0].get("asDocument").is_none(),
            "a small MP4 plays: {}",
            got[0]["filename"]
        );
        assert_eq!(got[1]["filename"], "Tutorial.mp4");
        assert_eq!(got[1]["asDocument"], json!(true));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(got[1]["data"].as_str().unwrap())
                .unwrap()
                .len(),
            3 * MB
        );
        let kinds: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT type, media_name FROM wa_messages WHERE chat_id = $1 AND sent_by_nary AND has_media \
             ORDER BY media_name",
        )
        .bind(&cid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            kinds,
            vec![
                ("video".to_string(), Some("Short.mp4".to_string())),
                ("document".to_string(), Some("Tutorial.mp4".to_string())),
            ]
        );
        let logged: Vec<String> = sqlx::query_scalar(
            "SELECT kind FROM nary_outbound_log WHERE chat_id = $1 AND kind IN ('video', 'document') ORDER BY created_at",
        )
        .bind(&cid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(logged, vec!["video", "document"]);
        restore(&pool, old).await;

        // over the video upload cap: refused on the card, naming the key
        let old = set(&pool, &[("nary.upload.video_max_mb", "2")]).await;
        let out = tool(
            &pool,
            &owner,
            "propose_customer_message",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "The video, Mr.",
                   "attach": [{"type": "file", "id": big_id}]}),
        )
        .await;
        assert!(error_of(&out).contains("nary.upload.video_max_mb"), "{out}");
        restore(&pool, old).await;
        unroute("POST", "/send-media");
        cleanup();
    });
}

// ─── email ───────────────────────────────────────────────────────────────────

#[test]
fn test_email_sends_a_file_over_the_limit_as_a_download_link_and_attaches_the_rest() {
    run(TAG, |pool| async move {
        let _s = store();
        let mails = smtp();
        let owner = the_owner(&pool).await;
        let d = deal(&pool).await;
        let old = set(&pool, &[("nary.attach.max_total_mb", "1")]).await;
        let pdf = {
            let mut v = b"%PDF-1.7\n".to_vec();
            v.resize(2000, b' ');
            v
        };
        let doc = upload(&pool, &owner, d, "Guide.pdf", &pdf).await["file_id"]
            .as_str()
            .unwrap()
            .to_string();
        let vid = upload(&pool, &owner, d, "Tutorial.mp4", &mp4(60, 2 * MB)).await["file_id"]
            .as_str()
            .unwrap()
            .to_string();
        let to = format!("{}@customer.example", u("bob"));
        let out = tool(
            &pool,
            &owner,
            "propose_customer_message",
            json!({"channel": "email", "to": to, "subject": "Video tutorial",
                   "text": "Dear Mr Bob,\n\nHere is the tutorial and the guide.\n\nBest regards,",
                   "attach": [{"type": "file", "id": doc}, {"type": "file", "id": vid}]}),
        )
        .await;
        let p = out["proposal"].clone();
        assert_eq!(
            p["attachments"],
            json!([
                "Guide.pdf (2 KB, PDF)",
                "Tutorial.mp4 (2.0 MB, 1:00, MP4 video, sent as a download link valid 7 days: over the 1.0 MB an email carries)"
            ]),
            "{out}"
        );
        assert_eq!(p["files"][1]["send_as"], "link");
        assert!(p["files"][0].get("send_as").is_none());
        let mut conn = pool.acquire().await.unwrap();
        let row = executor::send_on_instruction(
            &mut conn,
            &owner,
            "email.reply.send",
            json!({"channel": "email", "to": to, "subject": p["subject"], "text": p["text"], "attach": p["attach"]}),
            None,
        )
        .await
        .unwrap();
        drop(conn);
        assert_eq!(row.status, "applied", "{:?}", row.result);
        let mails = mails.lock().unwrap().clone();
        assert_eq!(mails.len(), 1);
        let files = mails[0].attachments.clone().unwrap_or_default();
        assert_eq!(files.len(), 1, "only the PDF is attached");
        assert_eq!(files[0].filename, "Guide.pdf");
        let html = &mails[0].html_body;
        assert!(
            html.contains("Download Tutorial.mp4 (2.0 MB, link valid for 7 days)"),
            "{html}"
        );
        assert!(
            html.contains("X-Amz-Expires=604800"),
            "a 7-day signed link: {html}"
        );
        assert!(html.contains("Tutorial.mp4"), "{html}");
        // the link days setting, clamped to the storage's 7-day limit
        let old2 = set(&pool, &[("nary.email.link_days", "30")]).await;
        let out = tool(
            &pool,
            &owner,
            "propose_customer_message",
            json!({"channel": "email", "to": to, "subject": "Again", "text": "Dear Mr Bob,\n\nThe video.",
                   "attach": [{"type": "file", "id": vid}]}),
        )
        .await;
        assert!(
            out["proposal"]["attachments"][0]
                .as_str()
                .unwrap()
                .contains("valid 7 days"),
            "{out}"
        );
        restore(&pool, old2).await;
        let old3 = set(&pool, &[("nary.email.link_days", "2")]).await;
        let out = tool(
            &pool,
            &owner,
            "propose_customer_message",
            json!({"channel": "email", "to": to, "subject": "Again", "text": "Dear Mr Bob,\n\nThe video.",
                   "attach": [{"type": "file", "id": vid}]}),
        )
        .await;
        assert!(
            out["proposal"]["attachments"][0]
                .as_str()
                .unwrap()
                .contains("valid 2 days"),
            "{out}"
        );
        restore(&pool, old3).await;
        restore(&pool, old).await;
        // within the limit the video is attached as before
        let out = tool(
            &pool,
            &owner,
            "propose_customer_message",
            json!({"channel": "email", "to": to, "subject": "Again", "text": "Dear Mr Bob,\n\nThe video.",
                   "attach": [{"type": "file", "id": vid}]}),
        )
        .await;
        assert_eq!(
            out["proposal"]["attachments"][0], "Tutorial.mp4 (2.0 MB, 1:00, MP4 video)",
            "{out}"
        );
        cleanup();
    });
}
