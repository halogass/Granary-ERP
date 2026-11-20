//! Port of backend/tests/test_nary_p2_send.py: where and when Nary may write on WhatsApp (wa_send), the
//! shadow-mode exemption for the acknowledgement only, human takeover, the channel tick (channels) and the
//! WhatsApp Business labels (labels). The Owner's WhatsApp command tests are commands.py's (its own port) and
//! the webhook ones wave 4's.

mod nary_n2_common;

use chrono::{Duration, Utc};
use granary_services::nary::wa_send::{self, SendError};
use granary_services::nary::{autonomy, channels, executor, labels};
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use wiremock::ResponseTemplate;

const TAG: &str = "n2_p2_send";

fn p(cid: &str, body: &str) -> Value {
    payload(cid, body, false, None, None, json!({}))
}

async fn _send(
    pool: &PgPool,
    cid: &str,
    text: &str,
    kind: &str,
) -> Result<wa_send::Sent, SendError> {
    let mut c = pool.acquire().await.unwrap();
    wa_send::send_text(&mut c, cid, text, kind, None, false).await
}

fn refused_with(r: Result<wa_send::Sent, SendError>, needle: &str) -> SendError {
    let e = r.expect_err("refused");
    assert!(e.is_send_refused(), "{e}");
    assert!(e.detail().contains(needle), "{} / {needle}", e.detail());
    e
}

// ─── Send rules ──────────────────────────────────────────────────────────────

#[test]
fn test_nary_only_writes_into_chats_the_customer_started() {
    run(TAG, |pool| async move {
        working_hours(true);
        refused_with(_send(&pool, &chat_id(), "Sure Mr", "reply").await, "knows");
        let cid = chat_id();
        ingest(
            &pool,
            &payload(
                &cid,
                "Hello, is this Your Company?",
                true,
                None,
                None,
                json!({"sentByApi": true}),
            ),
            "live",
        )
        .await;
        refused_with(
            _send(&pool, &cid, "Sure Mr", "reply").await,
            "customer has written",
        );
        assert!(sent().is_empty());
        ingest(
            &pool,
            &p(
                &cid,
                "Yes, that's right. I would like to ask about the price.",
            ),
            "live",
        )
        .await;
        _send(&pool, &cid, "Sure Mr", "reply").await.unwrap();
        assert_eq!(sent().len(), 1);
    });
}

#[test]
fn test_a_send_is_signed_stored_and_its_echo_is_not_a_takeover() {
    run(TAG, |pool| async move {
        working_hours(true);
        let cid = chat_id();
        ingest(&pool, &p(&cid, DEFAULT_BODY), "live").await;
        let out = _send(&pool, &cid, "Thank you Mr, let us check first.", "reply")
            .await
            .unwrap();
        assert!(out.body.ends_with("\n\n_\u{2014} Nary_"), "{}", out.body);
        assert_eq!(sent()[0]["to"], json!(cid));
        let stored: Vec<_> = messages(&pool, &cid)
            .await
            .into_iter()
            .filter(|m| m.from_me)
            .collect();
        assert!(stored.len() == 1 && stored[0].sent_by_nary && stored[0].sent_by_erp);
        let key = out
            .message_id
            .as_deref()
            .unwrap()
            .rsplit('_')
            .next()
            .unwrap()
            .to_string();
        let echo = payload(&cid, &out.body, true, Some(&key), None, json!({}));
        assert_eq!(ingest(&pool, &echo, "live").await["status"], "duplicate");
        assert!(get_chat(&pool, &cid).await.unwrap().human_until.is_none());
    });
}

#[test]
fn test_takeover_quiet_hours_and_pacing() {
    run(TAG, |pool| async move {
        working_hours(true);
        let cid = chat_id();
        ingest(&pool, &p(&cid, DEFAULT_BODY), "live").await;
        ingest(
            &pool,
            &payload(&cid, "I will call you, Mr", true, None, None, json!({})),
            "live",
        )
        .await;
        refused_with(
            _send(&pool, &cid, "Sure Mr", "reply").await,
            "from the phone",
        );
        sqlx::query("UPDATE wa_chats SET human_until = $1 WHERE chat_id = $2")
            .bind(Utc::now() - Duration::minutes(1))
            .bind(&cid)
            .execute(&pool)
            .await
            .unwrap();
        working_hours(false);
        let e = refused_with(
            _send(&pool, &cid, "Sure Mr", "reply").await,
            "working hours",
        );
        assert!(e.is_quiet_hours());
        _send(
            &pool,
            &cid,
            "Thank you, we have received your message.",
            "ack",
        )
        .await
        .unwrap();
        working_hours(true);
        let old = set(&pool, &[("nary.wa.max_per_hour", "1")]).await;
        let e = _send(&pool, &cid, "Sure Mr", "reply")
            .await
            .expect_err("paced");
        restore(&pool, old).await;
        assert!(e.is_send_refused() && e.status() == 429);
        assert_eq!(sent().len(), 1);
    });
}

#[test]
fn test_held_excluded_and_staff_chats_are_refused() {
    run(TAG, |pool| async move {
        working_hours(true);
        let cid = chat_id();
        ingest(
            &pool,
            &payload(
                &cid,
                DEFAULT_BODY,
                false,
                None,
                None,
                json!({"isMyContact": true}),
            ),
            "live",
        )
        .await;
        assert_eq!(get_chat(&pool, &cid).await.unwrap().status, "held");
        refused_with(_send(&pool, &cid, "Sure Mr", "reply").await, "held");
        let staff = chat_id();
        let mut c = pool.acquire().await.unwrap();
        let su = superadmin(&mut c).await;
        drop(c);
        link_staff(&pool, su, &staff, "t").await;
        ingest(&pool, &p(&staff, "Bos"), "live").await;
        refused_with(_send(&pool, &staff, "Sure Mr", "reply").await, "staff");
        assert!(sent().is_empty());
    });
}

#[test]
fn test_in_shadow_mode_only_the_acknowledgement_is_exempt() {
    run(TAG, |pool| async move {
        working_hours(true);
        let cid = chat_id();
        ingest(&pool, &p(&cid, DEFAULT_BODY), "live").await;
        let mut c = pool.acquire().await.unwrap();
        let su = superadmin(&mut c).await;
        let actor = auth_user(&mut c, su).await;
        drop(c);
        let levels = json!({"whatsapp.reply.send": 3, "whatsapp.ack.send": 3}).to_string();
        let old = set(&pool, &[("nary.autonomy.levels", &levels)]).await;
        let propose = |pool: PgPool,
                       action: &'static str,
                       payload: Value,
                       actor: granary_core::auth::AuthUser| async move {
            let mut c = pool.acquire().await.unwrap();
            executor::propose(
                &mut c,
                action,
                payload,
                &actor,
                executor::ProposeOpts::default(),
            )
            .await
            .unwrap()
        };
        let reply = propose(
            pool.clone(),
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Laptop price..."}),
            actor.clone(),
        )
        .await;
        let ack = propose(
            pool.clone(),
            "whatsapp.ack.send",
            json!({"chat_id": cid, "text": "Thank you"}),
            actor.clone(),
        )
        .await;
        assert_eq!(
            (reply.status.as_str(), ack.status.as_str()),
            ("shadow", "applied")
        );
        assert!(
            sent().len() == 1
                && sent()[0]["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("Thank you")
        );
        let old2 = set(&pool, &[("nary.shadow_mode", "false")]).await;
        let reply = propose(
            pool.clone(),
            "whatsapp.reply.send",
            json!({"channel": "whatsapp", "chat_id": cid, "text": "Sure Mr"}),
            actor.clone(),
        )
        .await;
        restore(&pool, old2).await;
        assert!(reply.status == "applied" && sent().len() == 2);
        restore(&pool, old).await;
        let mut c = pool.acquire().await.unwrap();
        assert_eq!(
            autonomy::level_for(&mut c, "reengagement.send")
                .await
                .unwrap(),
            1
        );
    });
}

#[test]
fn test_the_takeover_check_in_reaches_the_owner_once() {
    run(TAG, |pool| async move {
        owner(&pool).await;
        let cid = chat_id();
        ingest(&pool, &p(&cid, DEFAULT_BODY), "live").await;
        ingest(
            &pool,
            &payload(
                &cid,
                "I will reply to this one",
                true,
                None,
                None,
                json!({}),
            ),
            "live",
        )
        .await;
        sqlx::query("UPDATE wa_chats SET human_until = $1 WHERE chat_id = $2")
            .bind(Utc::now() - Duration::seconds(5))
            .bind(&cid)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert_eq!(channels::takeover_checkins(&mut tx).await.unwrap(), 1);
        tx.commit().await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert_eq!(channels::takeover_checkins(&mut tx).await.unwrap(), 0);
        tx.commit().await.unwrap();
        let notes: Vec<Option<String>> =
            sqlx::query_scalar("SELECT body FROM notifications WHERE type = 'nary_checkin'")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(notes.len() == 1 && notes[0].as_deref().unwrap().contains("stayed silent"));
    });
}

// ─── The Owner's WhatsApp commands (commands.py; the webhook is wave 4) ──
// test_command_parsing is ported in nary_staff_chat_flows.rs (test_command_parsing).

#[test]
#[ignore = "covered by crates/api/tests/whatsapp_routes.rs::test_pause_and_resume_from_the_ceos_whatsapp_work_while_paused (wave 4)"]
fn test_pause_and_resume_from_the_ceos_whatsapp_work_while_paused() {}

#[test]
#[ignore = "covered by crates/api/tests/whatsapp_routes.rs::test_numbered_replies_apply_the_cards_from_the_brief (wave 4)"]
fn test_numbered_replies_apply_the_cards_from_the_brief() {}

#[test]
fn test_a_single_card_notice_offers_1_apply_2_view_3_skip() {
    run(TAG, |pool| async move {
        working_hours(true);
        granary_services::nary::owner_channel::set_enabled_override(Some(false)); // owner_channel_off
        owner(&pool).await;
        let lead_id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO leads (id, title, source, status, custom, is_deleted) VALUES ($1, 'Card lead', 'whatsapp_direct', 'new', '{}', false)",
        )
        .bind(lead_id)
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let su = superadmin(&mut tx).await;
        let actor = auth_user(&mut tx, su).await;
        let card = executor::propose(
            &mut tx,
            "lead.promote",
            json!({"lead_id": lead_id.to_string()}),
            &actor,
            executor::ProposeOpts::default(),
        )
        .await
        .unwrap();
        let prompt = granary_services::nary::commands::get_prompt(&mut tx)
            .await
            .unwrap()
            .expect("prompt");
        tx.commit().await.unwrap();
        let body: Option<String> = sqlx::query_scalar(
            "SELECT body FROM notifications WHERE entity_id = $1 ORDER BY created_at LIMIT 1",
        )
        .bind(card.id.to_string())
        .fetch_optional(&pool)
        .await
        .unwrap()
        .flatten();
        assert!(body
            .expect("notification")
            .contains(granary_services::nary::commands::CARD_FOOTER));
        let mut p = prompt.as_object().unwrap().clone();
        p.remove("at");
        assert_eq!(
            Value::Object(p),
            json!({"kind": "card", "card_id": card.id.to_string()})
        );
    });
}

// ─── The channel tick: health, gap check ─────────────────────────────────────

#[test]
fn test_health_emails_the_owner_when_whatsapp_is_down() {
    run(TAG, |pool| async move {
        owner(&pool).await;
        route("GET", "/health", |_| {
            ResponseTemplate::new(200)
                .set_body_json(json!({"status": "disconnected", "outboxPending": 3}))
        });
        let mut c = pool.acquire().await.unwrap();
        let out = channels::health(&mut c).await.unwrap();
        let problems: Vec<String> = out["problems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_string())
            .collect();
        assert!(
            problems.iter().any(|p| p.contains("disconnected"))
                && problems.iter().any(|p| p.contains("3 WhatsApp"))
        );
        let prio: Option<Option<String>> = sqlx::query_scalar(
            "SELECT priority FROM notifications WHERE type = 'nary_error' AND entity_id = 'nary.health' LIMIT 1",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(prio, Some(Some("high".to_string())));
    });
}

#[test]
fn test_the_gap_check_stores_what_the_webhook_missed() {
    run(TAG, |pool| async move {
        let cid = chat_id();
        ingest(&pool, &p(&cid, "first message"), "live").await;
        let now = Utc::now().timestamp() - 600;
        let listing = json!([
            {"id": format!("false_{cid}_MISSED1"), "body": "the missed message", "fromMe": false, "timestamp": now, "type": "chat"},
            {"id": format!("false_{cid}_MISSED2"), "body": "and one more", "fromMe": false, "timestamp": now, "type": "chat"},
        ]);
        route("GET", &format!("/messages/{cid}"), move |_| {
            ResponseTemplate::new(200).set_body_json(listing.clone())
        });
        let mut tx = pool.begin().await.unwrap();
        assert_eq!(channels::gap_check(&mut tx).await.unwrap(), 2);
        assert_eq!(channels::gap_check(&mut tx).await.unwrap(), 0);
        tx.commit().await.unwrap();
        let bodies: Vec<String> = messages(&pool, &cid)
            .await
            .into_iter()
            .filter_map(|m| m.body)
            .collect();
        assert!(
            bodies.contains(&"the missed message".to_string())
                && bodies.contains(&"and one more".to_string())
        );
        let origins: Vec<Option<String>> =
            sqlx::query_scalar("SELECT payload ->> 'origin' FROM domain_events")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            origins
                .iter()
                .filter(|o| o.as_deref() == Some("gapfill"))
                .count(),
            2
        );
    });
}

#[test]
fn test_the_tick_runs_every_part_and_survives_one_failing() {
    run(TAG, |pool| async move {
        // Python monkeypatches channels.gap_check to raise; here the gap check fails on a broken setting.
        let old = set(&pool, &[("nary.wa.gapcheck_lookback_hours", "x")]).await;
        route("GET", "/health", |_| {
            ResponseTemplate::new(200).set_body_json(json!({"status": "ready", "outboxPending": 0}))
        });
        let mut tx = pool.begin().await.unwrap();
        let out = channels::tick(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        assert!(
            out["gapcheck"].as_str().unwrap().starts_with("failed"),
            "{out}"
        );
        assert_eq!(out["health"]["problems"], json!([]));
        assert_eq!(out["history"], json!("idle"));
        let mut tx = pool.begin().await.unwrap();
        let again = channels::tick(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        restore(&pool, old).await;
        assert!(again.get("health").is_none() && again.get("gapcheck").is_none());
    });
}

// ─── WhatsApp Business labels ────────────────────────────────────────────────

#[test]
fn test_labels_follow_the_deal_stage_and_skip_when_unsupported() {
    run(TAG, |pool| async move {
        owner(&pool).await;
        let cid = chat_id();
        ingest(&pool, &p(&cid, DEFAULT_BODY), "live").await;
        route("GET", "/labels", |_| {
            ResponseTemplate::new(200).set_body_json(json!([
            {"id": "1", "name": "New lead"}, {"id": "2", "name": "Proposal"}, {"id": "9", "name": "VIP"}]))
        });
        let posted = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, Value)>::new()));
        let rec = posted.clone();
        route("POST", "/labels/", move |req| {
            rec.lock().unwrap().push((
                req.url.path().to_string(),
                serde_json::from_slice(&req.body).unwrap(),
            ));
            ResponseTemplate::new(200).set_body_json(json!({"success": true}))
        });
        let mut c = pool.acquire().await.unwrap();
        assert_eq!(
            labels::sync(&mut c, "proposal", std::slice::from_ref(&cid))
                .await
                .unwrap()["synced"],
            0
        );
        let map = json!({"lead:new": "New lead", "proposal": "Proposal"}).to_string();
        let old = set(
            &pool,
            &[
                ("nary.wa.labels_enabled", "true"),
                ("nary.wa.label_map", &map),
            ],
        )
        .await;
        let out = labels::sync(&mut c, "proposal", std::slice::from_ref(&cid))
            .await
            .unwrap();
        assert_eq!(out, json!({"synced": 1, "add": ["2"], "remove": ["1"]}));
        assert_eq!(
            *posted.lock().unwrap(),
            vec![(
                format!("/labels/{cid}"),
                json!({"add": ["2"], "remove": ["1"]})
            )]
        );
        route("GET", "/labels", |_| {
            ResponseTemplate::new(501).set_body_json(json!({"error": "not supported"}))
        });
        let out = labels::sync(&mut c, "proposal", std::slice::from_ref(&cid))
            .await
            .unwrap();
        assert_eq!(out["error"], "labels not supported");
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notifications WHERE entity_id = 'nary.labels'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        restore(&pool, old).await;
        assert!(n >= 1);
    });
}
