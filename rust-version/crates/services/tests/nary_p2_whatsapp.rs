//! Port of backend/tests/test_nary_p2_whatsapp.py: persistence, privacy, takeover, the inbound pipeline
//! (wa_ingest, privacy, channels). Personal chats never reach the AI: there is no model call at all for a
//! held or excluded chat, and an excluded chat's messages are never stored.

mod nary_n2_common;

use granary_services::ai::registry::seam::{Answer, Brain};
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use granary_services::nary::{channels, media, privacy, wa_ingest};
use granary_services::storage::{set_store, ObjectStore, StoreError};
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::ResponseTemplate;

const TAG: &str = "n2_p2_whatsapp";

fn p(cid: &str, body: &str) -> Value {
    payload(cid, body, false, None, None, json!({}))
}

async fn contact(pool: &PgPool, ph: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, whatsapp, custom, is_primary, tags, is_deleted) \
         VALUES ($1, 'Bob', 'Sanders', $2, '{}', false, '{}', false)",
    )
    .bind(id)
    .bind(format!("+{ph}"))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn ack_on(pool: &PgPool) -> Vec<(String, Option<String>)> {
    set(pool, &[("nary.ack.enabled", "true")]).await
}

// ─── Persistence ─────────────────────────────────────────────────────────────

#[test]
fn test_both_directions_are_stored_once_by_message_key() {
    run(TAG, |pool| async move {
        let cid = chat_id();
        let p_in = payload(
            &cid,
            "Hello, what is the price of your laptops?",
            false,
            Some("KEYIN1"),
            None,
            json!({}),
        );
        assert_eq!(ingest(&pool, &p_in, "live").await["status"], "stored");
        assert_eq!(ingest(&pool, &p_in, "live").await["status"], "duplicate");
        let mut lid = p_in.clone();
        lid["messageId"] = json!(p_in["messageId"]
            .as_str()
            .unwrap()
            .replace(&cid, "12345@lid"));
        assert_eq!(ingest(&pool, &lid, "live").await["status"], "duplicate");
        let out = payload(
            &cid,
            "Sure Mr, happy to help",
            true,
            Some("KEYOUT1"),
            None,
            json!({"sentByApi": true}),
        );
        assert_eq!(ingest(&pool, &out, "live").await["status"], "stored");
        let rows = messages(&pool, &cid).await;
        let got: Vec<(String, bool, bool)> = rows
            .iter()
            .map(|m| (m.direction.clone(), m.from_me, m.sent_by_erp))
            .collect();
        assert_eq!(
            got,
            vec![
                ("inbound".into(), false, false),
                ("outbound".into(), true, true)
            ]
        );
        let types: Vec<String> =
            sqlx::query_scalar("SELECT type FROM domain_events ORDER BY occurred_at")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(types, vec!["message.inbound", "message.outbound"]);
        let chat = get_chat(&pool, &cid).await.unwrap();
        assert_eq!(
            chat.phone,
            Some(format!("+{}", cid.split('@').next().unwrap()))
        );
        assert_eq!(chat.customer_started, Some(true));
        assert_eq!(chat.status, "active");
    });
}

#[test]
fn test_unknown_numbers_are_kept_and_known_ones_get_an_activity() {
    run(TAG, |pool| async move {
        let unknown = chat_id();
        ingest(&pool, &p(&unknown, DEFAULT_BODY), "live").await;
        assert_eq!(messages(&pool, &unknown).await.len(), 1);
        let ph = phone();
        let c = contact(&pool, &ph).await;
        let known = format!("{ph}@c.us");
        ingest(
            &pool,
            &p(&known, "Mr, I have paid this month's invoice"),
            "live",
        )
        .await;
        assert_eq!(get_chat(&pool, &known).await.unwrap().contact_id, Some(c));
        let acts: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT activity_type, direction FROM activities WHERE contact_id = $1")
                .bind(c)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            acts,
            vec![("whatsapp".to_string(), Some("inbound".to_string()))]
        );
    });
}

#[test]
fn test_groups_and_broadcasts_are_never_stored() {
    run(TAG, |pool| async move {
        for cid in ["12036304@g.us", "status@broadcast", "1203@newsletter"] {
            assert_eq!(
                ingest(&pool, &p(cid, DEFAULT_BODY), "live").await["status"],
                "skipped"
            );
        }
        let g = payload(
            &chat_id(),
            DEFAULT_BODY,
            false,
            None,
            None,
            json!({"isGroup": true}),
        );
        assert_eq!(ingest(&pool, &g, "live").await["status"], "skipped");
        let n: i64 = sqlx::query_scalar("SELECT count(id) FROM wa_messages")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    });
}

#[test]
fn test_a_reply_typed_on_the_phone_starts_a_takeover_but_our_own_echo_does_not() {
    run(TAG, |pool| async move {
        let cid = chat_id();
        ingest(&pool, &p(&cid, DEFAULT_BODY), "live").await;
        ingest(
            &pool,
            &payload(&cid, "Sure", true, None, None, json!({"sentByApi": true})),
            "live",
        )
        .await;
        assert!(get_chat(&pool, &cid).await.unwrap().human_until.is_none());
        let out = ingest(
            &pool,
            &payload(
                &cid,
                "Sure Mr, I will call you",
                true,
                None,
                None,
                json!({}),
            ),
            "live",
        )
        .await;
        assert_eq!(out["takeover"], true);
        let chat = get_chat(&pool, &cid).await.unwrap();
        let secs = (chat.human_until.unwrap() - Utc::now()).num_seconds();
        assert!((secs - 24 * 3600).abs() <= 120, "{secs}");
        let late = (Utc::now() - Duration::hours(30)).timestamp();
        let out = ingest(
            &pool,
            &payload(&cid, "ok", true, None, Some(late), json!({})),
            "gapfill",
        )
        .await;
        assert_eq!(out["takeover"], false);
    });
}

#[test]
#[ignore = "covered by crates/api/tests/whatsapp_routes.rs::test_the_webhook_needs_the_service_token_and_stores (wave 4)"]
fn test_the_webhook_needs_the_service_token_and_stores() {}

// ─── Privacy: personal chats never reach the AI ──────────────────────────────

#[test]
fn test_an_excluded_chat_is_never_stored_never_read_never_answered() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        let cid = chat_id();
        ingest(
            &pool,
            &p(&cid, "Honey, where shall we eat tonight?"),
            "live",
        )
        .await;
        {
            let mut tx = pool.begin().await.unwrap();
            let su = superadmin(&mut tx).await;
            let user = auth_user(&mut tx, su).await;
            wa_ingest::exclude_chat(&mut tx, &cid, Some(&user), "manual")
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        assert!(messages(&pool, &cid).await.is_empty());
        let chat = get_chat(&pool, &cid).await.unwrap();
        assert!(chat.status == "excluded" && chat.display_name.is_none());
        assert_eq!(
            ingest(&pool, &p(&cid, "Don't forget to buy milk"), "live").await["status"],
            "excluded"
        );
        assert!(messages(&pool, &cid).await.is_empty());
        let old = ack_on(&pool).await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        assert!(brain.calls().is_empty());
        assert!(sent().is_empty());
        let mut c = pool.acquire().await.unwrap();
        let err = privacy::assert_ai_allowed(&mut c, &cid).await.unwrap_err();
        assert!(matches!(err, privacy::PrivacyError::Private(_)));
    });
}

#[test]
fn test_a_saved_number_the_crm_does_not_know_is_held_until_the_owner_decides() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        let cid = chat_id();
        let pl = payload(
            &cid,
            "Mr, are we still meeting at home tomorrow?",
            false,
            None,
            None,
            json!({"isMyContact": true}),
        );
        ingest(&pool, &pl, "live").await;
        assert_eq!(get_chat(&pool, &cid).await.unwrap().status, "held");
        let old = ack_on(&pool).await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        assert!(brain.calls().is_empty() && sent().is_empty());
        let note: Option<Option<String>> =
            sqlx::query_scalar("SELECT body FROM notifications WHERE type = 'nary_held' LIMIT 1")
                .fetch_optional(&pool)
                .await
                .unwrap();
        let note = note.expect("a held notification");
        assert!(!note.unwrap_or_default().contains("tomorrow"));
        let mut tx = pool.begin().await.unwrap();
        let su = superadmin(&mut tx).await;
        let user = auth_user(&mut tx, su).await;
        channels::decide_held(&mut tx, &cid, false, Some(&user))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(get_chat(&pool, &cid).await.unwrap().status, "excluded");
        assert!(messages(&pool, &cid).await.is_empty());
        assert!(brain.calls().is_empty());
    });
}

#[test]
fn test_a_held_chat_released_as_business_is_processed() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        let cid = chat_id();
        let pl = payload(
            &cid,
            "Good afternoon, we need laptops for our warehouse",
            false,
            None,
            None,
            json!({"isMyContact": true}),
        );
        ingest(&pool, &pl, "live").await;
        dispatch(&pool).await;
        assert!(brain.calls().is_empty());
        let mut tx = pool.begin().await.unwrap();
        let su = superadmin(&mut tx).await;
        let user = auth_user(&mut tx, su).await;
        channels::decide_held(&mut tx, &cid, true, Some(&user))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        dispatch(&pool).await;
        assert!(brain.jobs().contains(&"nary.triage".to_string()));
        assert_eq!(
            get_chat(&pool, &cid).await.unwrap().triage.as_deref(),
            Some("prospect")
        );
    });
}

#[test]
fn test_a_personal_verdict_holds_the_chat_and_stops_the_ai() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        brain.set(
            "nary.triage",
            Answer::json(json!({"verdict": "personal", "language": "en", "reason": "family chat"})),
        );
        let cid = chat_id();
        ingest(&pool, &p(&cid, "Mum, I will be home late"), "live").await;
        let old = ack_on(&pool).await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        assert_eq!(get_chat(&pool, &cid).await.unwrap().status, "held");
        assert!(sent().is_empty() && !brain.jobs().contains(&"nary.reply_draft".to_string()));
        let n = brain.calls().len();
        ingest(&pool, &p(&cid, "Have you eaten yet?"), "live").await;
        dispatch(&pool).await;
        assert_eq!(brain.calls().len(), n);
    });
}

// ─── The inbound pipeline ────────────────────────────────────────────────────

#[test]
fn test_a_new_prospect_is_triaged_acknowledged_once_and_becomes_an_intake() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        brain.set(
            "nary.triage",
            Answer::json(json!({"verdict": "prospect", "language": "en", "reason": "pricing"})),
        );
        let cid = chat_id();
        ingest(
            &pool,
            &p(
                &cid,
                "Good morning, I am Bob from Acme Ltd. What is the price of laptops for 10 staff?",
            ),
            "live",
        )
        .await;
        let old = ack_on(&pool).await;
        let (_, failed) = dispatch(&pool).await;
        assert_eq!(failed, 0);
        let chat = get_chat(&pool, &cid).await.unwrap();
        assert_eq!(chat.triage.as_deref(), Some("prospect"));
        assert_eq!(chat.language.as_deref(), Some("en"));
        assert!(chat.acked_at.is_some());
        assert_eq!(sent().len(), 1);
        let text = sent()[0]["message"].as_str().unwrap().to_string();
        // the English template with the customer's first name, signed by Nary
        assert!(
            text.starts_with("Hi Bob, thank you for contacting us."),
            "{text}"
        );
        assert!(
            text.contains("2 working hours") && text.trim_end().ends_with("_\u{2014} Nary_"),
            "{text}"
        );
        let lead: (String, Option<String>) =
            sqlx::query_as("SELECT source, status FROM leads WHERE id = $1")
                .bind(chat.lead_id.unwrap())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            lead,
            (
                "whatsapp_direct".to_string(),
                Some("qualifying".to_string())
            )
        );
        let promote: Vec<String> = cards(&pool, Some("lead.promote"))
            .await
            .into_iter()
            .map(|c| c.status)
            .collect();
        assert_eq!(promote, vec!["pending"]);
        let draft = cards(&pool, Some("whatsapp.reply.send")).await;
        assert!(draft.len() == 1 && draft[0].status == "shadow");
        ingest(&pool, &p(&cid, "Oh, and the site is in Osaka"), "live").await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        assert_eq!(sent().len(), 1);
        let nary: Vec<_> = messages(&pool, &cid)
            .await
            .into_iter()
            .filter(|m| m.sent_by_nary)
            .collect();
        assert!(nary.len() == 1 && nary[0].from_me);
    });
}

#[test]
fn test_after_hours_the_acknowledgement_promises_the_next_working_day() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        owner(&pool).await;
        working_hours(false);
        let cid = chat_id();
        ingest(
            &pool,
            &p(
                &cid,
                "Hello, we need laptops for 30 stores. Can you share pricing?",
            ),
            "live",
        )
        .await;
        brain.set(
            "nary.triage",
            Answer::json(json!({"verdict": "prospect", "language": "en", "reason": "pricing"})),
        );
        let old = ack_on(&pool).await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        assert_eq!(sent().len(), 1);
        let m = sent()[0]["message"].as_str().unwrap().to_string();
        assert!(m.contains("will reply on the next working day."), "{m}");
        assert!(m.starts_with("Hi Bob,"), "{m}");
    });
}

#[test]
fn test_no_acknowledgement_for_anything_but_prospects_and_customers() {
    for verdict in [
        "spam",
        "scam",
        "wrong_number",
        "vendor",
        "job_seeker",
        "partner",
        "unclear",
    ] {
        run(TAG, |pool| async move {
            let brain = Brain::p2();
            let _seam = brain.install().await;
            working_hours(true);
            owner(&pool).await;
            brain.set(
                "nary.triage",
                Answer::json(json!({"verdict": verdict, "language": "en", "reason": "x"})),
            );
            brain.set(
                "nary.cascade_review",
                Answer::json(json!({"verdict": verdict, "language": "en", "reason": "x"})),
            );
            let cid = chat_id();
            ingest(&pool, &p(&cid, "Special promo: cheap backlinks!"), "live").await;
            let old = ack_on(&pool).await;
            dispatch(&pool).await;
            restore(&pool, old).await;
            assert!(sent().is_empty(), "{verdict}");
            let v: String = sqlx::query_scalar(
                "SELECT verdict FROM nary_triage_verdicts WHERE subject_ref = $1",
            )
            .bind(&cid)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(v, verdict);
        });
    }
}

#[test]
fn test_no_acknowledgement_to_staff_or_when_switched_off_or_in_an_ongoing_chat() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        let me = owner(&pool).await;
        let cid = chat_id();
        ingest(&pool, &p(&cid, DEFAULT_BODY), "live").await;
        dispatch(&pool).await; // nary.ack.enabled is seeded false
        assert!(sent().is_empty());
        let other = chat_id();
        let old_ts = (Utc::now() - Duration::days(3)).timestamp();
        ingest(
            &pool,
            &payload(
                &other,
                "Mr, I received yesterday's quotation",
                false,
                None,
                Some(old_ts),
                json!({}),
            ),
            "history",
        )
        .await;
        ingest(
            &pool,
            &p(&other, "I have a question about the payment terms"),
            "live",
        )
        .await;
        let old = ack_on(&pool).await;
        dispatch(&pool).await;
        assert!(sent().is_empty());
        let staff_chat = chat_id();
        link_staff(&pool, me, &staff_chat, "test").await;
        let n = brain.calls().len();
        ingest(
            &pool,
            &p(&staff_chat, "Boss, I have sent today's recap"),
            "live",
        )
        .await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        assert!(sent().is_empty() && brain.calls().len() == n);
    });
}

#[test]
fn test_when_whatsapp_is_down_the_acknowledgement_is_owed_and_sent_later() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        route("POST", "/send", |_| {
            ResponseTemplate::new(503).set_body_json(json!({"error": "not ready"}))
        });
        let cid = chat_id();
        ingest(
            &pool,
            &p(&cid, "I would like to ask about support plans"),
            "live",
        )
        .await;
        let old = ack_on(&pool).await;
        dispatch(&pool).await;
        let chat = get_chat(&pool, &cid).await.unwrap();
        assert!(chat.acked_at.is_none() && chat.ack_pending_since.is_some());
        unroute("POST", "/send");
        let mut tx = pool.begin().await.unwrap();
        assert_eq!(channels::owed_acks(&mut tx).await.unwrap(), 1);
        tx.commit().await.unwrap();
        restore(&pool, old).await;
        assert_eq!(sent().len(), 1);
        assert!(get_chat(&pool, &cid).await.unwrap().acked_at.is_some());
    });
}

#[test]
fn test_the_pipeline_and_the_owed_ack_tick_never_both_send() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        route("POST", "/send", |_| {
            ResponseTemplate::new(503).set_body_json(json!({"error": "not ready"}))
        });
        let cid = chat_id();
        ingest(
            &pool,
            &p(&cid, "I would like to ask about support plans"),
            "live",
        )
        .await;
        let old = ack_on(&pool).await;
        dispatch(&pool).await;
        unroute("POST", "/send");
        let chat = get_chat(&pool, &cid).await.unwrap();
        let since = chat.ack_pending_since.unwrap();
        let pipeline = |pool: PgPool, cid: String| async move {
            let mut c = pool.acquire().await.unwrap();
            let chat: granary_db::rows::nary::WaChat =
                sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
                    .bind(&cid)
                    .fetch_one(&mut *c)
                    .await
                    .unwrap();
            granary_services::nary::ack::send_for_chat(&mut c, &chat, since, None)
                .await
                .ok();
        };
        let tick = |pool: PgPool| async move {
            let mut c = pool.acquire().await.unwrap();
            channels::owed_acks(&mut c).await.ok();
        };
        let a = tokio::spawn(Box::pin(pipeline(pool.clone(), cid.clone())));
        let b = tokio::spawn(Box::pin(tick(pool.clone())));
        let c = tokio::spawn(Box::pin(pipeline(pool.clone(), cid.clone())));
        let _ = tokio::join!(a, b, c);
        restore(&pool, old).await;
        assert_eq!(sent().len(), 1);
        let chat = get_chat(&pool, &cid).await.unwrap();
        assert!(chat.acked_at.is_some() && chat.ack_pending_since.is_none());
    });
}

// ─── Media (media._put/_get -> storage::set_store, stt_client.transcribe / media.download -> media::seams) ──

#[derive(Default)]
struct MemStore {
    blobs: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

#[async_trait::async_trait]
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
        // `stored[key]`: a KeyError for a key never put
        self.blobs
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(StoreError {
                kind: "KeyError".into(),
                message: key.into(),
            })
    }
}

/// Restores the media seams and the object store when a media test ends (also on a failed assert).
struct MediaSeams;

impl MediaSeams {
    fn install() -> MediaSeams {
        set_store(Some(Arc::new(MemStore::default())));
        MediaSeams
    }
}

impl Drop for MediaSeams {
    fn drop(&mut self) {
        set_store(None);
        media::seams::set_download(None);
        media::seams::set_transcribe(None);
    }
}

#[test]
fn test_a_voice_note_is_transcribed_locally_and_triaged_from_the_text() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        let _media = MediaSeams::install();
        route("GET", "/media", |_| {
            ResponseTemplate::new(200).set_body_raw(b"OggS-voice".to_vec(), "audio/ogg")
        });
        let heard: Arc<Mutex<Option<media::seams::SttCall>>> = Arc::new(Mutex::new(None));
        let h = heard.clone();
        media::seams::set_transcribe(Some(Arc::new(move |call| {
            *h.lock().unwrap() = Some(call);
            Box::pin(async {
                Ok(
                    json!({"text": "hello, I want to buy laptops for the office", "language": "en", "duration": 6.0}),
                )
            })
        })));
        let cid = chat_id();
        let pl = payload(
            &cid,
            "",
            false,
            None,
            None,
            json!({"type": "ptt", "hasMedia": true,
            "mediaMeta": {"mimetype": "audio/ogg; codecs=opus", "size": 9000}}),
        );
        ingest(&pool, &pl, "live").await;
        dispatch(&pool).await;
        let msg = messages(&pool, &cid).await.remove(0);
        assert_eq!(msg.media_status, "stored");
        assert!(msg.media_key.as_deref().unwrap().starts_with("whatsapp/"));
        assert_eq!(
            msg.transcript.as_deref(),
            Some("hello, I want to buy laptops for the office")
        );
        let heard = heard
            .lock()
            .unwrap()
            .clone()
            .expect("transcribe was called");
        assert_eq!(heard.audio, b"OggS-voice".to_vec());
        assert_eq!(heard.max_seconds, Some(600));
        let calls = brain.calls();
        let triage = calls
            .iter()
            .find(|(job, _)| job == "nary.triage")
            .expect("a triage call");
        let content = triage.1.chat().unwrap().messages[0]["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            content.contains("[voice note] hello, I want to buy laptops"),
            "{content}"
        );
    });
}

#[test]
fn test_a_photo_is_described_by_the_vision_job() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        let _media = MediaSeams::install();
        route("GET", "/media", |_| {
            ResponseTemplate::new(200).set_body_raw(b"\xff\xd8jpeg".to_vec(), "image/jpeg")
        });
        let cid = chat_id();
        let pl = payload(
            &cid,
            "Here is our delivery note",
            false,
            None,
            None,
            json!({"type": "image", "hasMedia": true, "mediaMeta": {"mimetype": "image/jpeg"}}),
        );
        ingest(&pool, &pl, "live").await;
        dispatch(&pool).await;
        let msg = messages(&pool, &cid).await.remove(0);
        assert_eq!(
            msg.media_description.as_deref(),
            Some("A photo of a signed delivery note.")
        );
        let calls = brain.calls();
        let vision = calls
            .iter()
            .find(|(job, _)| job == "nary.vision")
            .expect("a vision call");
        let file = vision.1.file().unwrap();
        assert!(file.prompt.contains("for the sales team"));
        assert_eq!(file.mime_type, "image/jpeg");
    });
}

#[test]
fn test_the_phone_reply_is_compared_with_the_shadow_draft() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        let cid = chat_id();
        ingest(
            &pool,
            &p(&cid, "Morning, can we see a laptop demo next week?"),
            "live",
        )
        .await;
        dispatch(&pool).await;
        let draft = cards(&pool, Some("whatsapp.reply.send")).await;
        assert_eq!(draft[0].status, "shadow");
        let reply = payload(
            &cid,
            "Sure Mr, 10 laptops is fine. May I have your company name?",
            true,
            None,
            None,
            json!({}),
        );
        ingest(&pool, &reply, "live").await;
        dispatch(&pool).await;
        let draft = cards(&pool, Some("whatsapp.reply.send")).await;
        assert!(draft[0].compared_at.is_some());
        assert!(draft[0]
            .actual_text
            .as_deref()
            .unwrap()
            .starts_with("Sure Mr"));
        assert!(draft[0].diff_ratio.unwrap() < rust_decimal::Decimal::from(20));
    });
}

#[test]
fn test_a_saved_number_found_only_by_the_profile_lookup_is_still_held() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        route("GET", "/contact/", |_| {
            ResponseTemplate::new(200)
                .set_body_json(json!({"isMyContact": true, "isBusiness": false}))
        });
        let cid = chat_id();
        let pl = payload(
            &cid,
            "Uncle, the family dinner is at our house next week",
            false,
            None,
            None,
            json!({"isMyContact": null}),
        );
        ingest(&pool, &pl, "live").await;
        assert_eq!(get_chat(&pool, &cid).await.unwrap().status, "active");
        let old = ack_on(&pool).await;
        dispatch(&pool).await;
        restore(&pool, old).await;
        assert_eq!(get_chat(&pool, &cid).await.unwrap().status, "held");
        assert!(brain.calls().is_empty() && sent().is_empty());
    });
}

#[test]
fn test_once_the_owner_says_business_the_chat_is_never_held_again() {
    run(TAG, |pool| async move {
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        route("GET", "/contact/", |_| {
            ResponseTemplate::new(200).set_body_json(json!({"isMyContact": true}))
        });
        let cid = chat_id();
        let pl = payload(
            &cid,
            "Mr, this is Northwind Traders, we would like to order again",
            false,
            None,
            None,
            json!({"isMyContact": true}),
        );
        ingest(&pool, &pl, "live").await;
        let mut tx = pool.begin().await.unwrap();
        let su = superadmin(&mut tx).await;
        let user = auth_user(&mut tx, su).await;
        channels::decide_held(&mut tx, &cid, true, Some(&user))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        dispatch(&pool).await;
        let pl = payload(
            &cid,
            "Five units please, Mr",
            false,
            None,
            None,
            json!({"isMyContact": true}),
        );
        ingest(&pool, &pl, "live").await;
        dispatch(&pool).await;
        let chat = get_chat(&pool, &cid).await.unwrap();
        assert!(chat.status == "active" && chat.released_at.is_some());
        assert!(brain.jobs().contains(&"nary.triage".to_string()));
    });
}

#[test]
fn test_expired_media_is_marked_missing() {
    run(TAG, |pool| async move {
        let cid = chat_id();
        ingest(
            &pool,
            &payload(
                &cid,
                DEFAULT_BODY,
                false,
                None,
                None,
                json!({"type": "image", "hasMedia": true}),
            ),
            "live",
        )
        .await;
        let _media = MediaSeams::install();
        media::seams::set_download(Some(Arc::new(|_serialized| {
            Box::pin(async { ("missing".to_string(), None, None) })
        })));
        let mut tx = pool.begin().await.unwrap();
        let chat: granary_db::rows::nary::WaChat =
            sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
                .bind(&cid)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        let mut msg: granary_db::rows::nary::WaMessage =
            sqlx::query_as("SELECT * FROM wa_messages WHERE chat_id = $1")
                .bind(&cid)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert!(!media::store(&mut tx, &chat, &mut msg).await.unwrap());
        assert_eq!(msg.media_status, "missing");
        tx.commit().await.unwrap();
        assert_eq!(messages(&pool, &cid).await[0].media_status, "missing");
    });
}
