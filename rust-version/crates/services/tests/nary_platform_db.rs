//! Port of backend/tests/test_nary_p1_platform.py (Nary Phase 1, groups B and C): domain events, runs, the
//! action registry, the executor with approvals and autonomy, shadow drafts, the pause switch.
//!
//! One tokio test runs the scenarios in order on one fresh database (Python wipes the tables between tests;
//! the scenarios here do the same), because the event dispatcher sees every event in the database.
//! The HTTP tests (CRM changes emit events, MCP writes, deciding a card on the web) are routers: wave 4.
mod support;

use std::sync::{Arc, Mutex};

use granary_core::auth::AuthUser;
use granary_services::ai::base::ChatResult;
use granary_services::nary::{
    self, actions, autonomy, events, executor, governance, roles, runs, wa_send,
};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn u(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

async fn set(pool: &PgPool, key: &str, value: &str) -> Option<String> {
    let old: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM system_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(pool)
            .await
            .unwrap();
    sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(key)
        .bind(value)
        .execute(pool)
        .await
        .unwrap();
    old.and_then(|o| o.0)
}

async fn restore(pool: &PgPool, key: &str, old: Option<String>) {
    sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(key)
        .bind(old)
        .execute(pool)
        .await
        .unwrap();
}

async fn wipe(pool: &PgPool) {
    for q in [
        "DELETE FROM agent_pending_actions",
        "DELETE FROM agent_steps",
        "DELETE FROM agent_runs",
        "DELETE FROM domain_events",
        "DELETE FROM notifications WHERE type LIKE 'nary_%'",
        "UPDATE nary_autonomy SET streak = 0, candidate_since = NULL",
    ] {
        sqlx::query(q).execute(pool).await.unwrap();
    }
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

async fn make_user(pool: &PgPool, superadmin: bool, perms: &[&str]) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         email_signature_enabled, must_change_password) VALUES ($1, $2, $3, 'x', true, $4, false, 'internal', true, false)",
    )
    .bind(id)
    .bind(format!("{}@granary.test", u("pytest")))
    .bind(u("Pytest Person"))
    .bind(superadmin)
    .execute(pool)
    .await
    .unwrap();
    for p in perms {
        sqlx::query("INSERT INTO user_permissions (user_id, permission_id, is_deny) SELECT $1, id, false FROM permissions WHERE code = $2")
            .bind(id)
            .bind(p)
            .execute(pool)
            .await
            .unwrap();
    }
    let mut conn = pool.acquire().await.unwrap();
    roles::load_actor(&mut conn, id).await.unwrap().unwrap()
}

async fn lead(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO leads (id, title, source, status, custom, is_deleted) VALUES ($1, $2, 'email', 'new', '{}', false)")
        .bind(id)
        .bind(u("P1 lead"))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn lead_field(pool: &PgPool, id: Uuid, field: &str) -> Option<String> {
    sqlx::query_scalar(&format!("SELECT {field}::text FROM leads WHERE id = $1"))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn change(lead_id: Uuid, fields: Value) -> Value {
    json!({"object_type": "lead", "action": "update", "record_id": lead_id.to_string(), "fields": fields})
}

fn detail(e: &granary_core::errors::ApiError) -> String {
    governance::http_detail(e).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nary_p1_platform() {
    let pool = support::fresh_db("n1_nary_platform").await;
    nary::set_session_pool(pool.clone());
    granary_services::approvals::notify::set_external_delivery_enabled(false);
    let admin = make_user(&pool, true, &[]).await;

    // ─── test_events_commit_with_the_change_and_dispatch_once ─────────────────
    wipe(&pool).await;
    {
        let mut tx = pool.begin().await.unwrap();
        events::emit(
            &mut tx,
            "lead.created",
            Some("lead"),
            Some(Uuid::new_v4()),
            Some(json!({"x": 1})),
            None,
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap(); // the change failed: no event
    }
    assert_eq!(count(&pool, "SELECT count(*) FROM domain_events").await, 0);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let s2 = seen.clone();
    let id = events::subscribe(
        &["deal.staged"],
        events::subscriber(move |_db, ev| {
            let s = s2.clone();
            let id = ev.id.to_string();
            Box::pin(async move {
                s.lock().unwrap().push(id);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                Ok(())
            })
        }),
    );
    {
        let mut tx = pool.begin().await.unwrap();
        for _ in 0..5 {
            events::emit(
                &mut tx,
                "deal.staged",
                Some("deal"),
                Some(Uuid::new_v4()),
                Some(json!({"to": "proposal"})),
                None,
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
    }
    let (a, b) = tokio::join!(
        events::dispatch_pending(None),
        events::dispatch_pending(None)
    ); // two workers
    events::unsubscribe(id);
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(
        a["dispatched"].as_i64().unwrap() + b["dispatched"].as_i64().unwrap(),
        5,
        "{a} {b}"
    );
    {
        let seen = seen.lock().unwrap();
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert!(seen.len() == 5 && unique.len() == 5); // each event exactly once
    }
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM domain_events WHERE dispatched_at IS NULL"
        )
        .await,
        0
    );
    // The Phase 1 subscriber recorded a run per event
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent_runs WHERE trigger_type = 'event'"
        )
        .await,
        5
    );

    // ─── test_a_failing_subscriber_retries_then_parks_with_an_error_card ──────
    wipe(&pool).await;
    let id = events::subscribe(
        &["payment.recorded"],
        events::subscriber(|_db, _ev| {
            Box::pin(async {
                Err(granary_core::errors::ApiError::internal(
                    governance::PyException::new("RuntimeError", "subscriber blew up"),
                ))
            })
        }),
    );
    {
        let mut tx = pool.begin().await.unwrap();
        events::emit(
            &mut tx,
            "payment.recorded",
            Some("payment"),
            Some(Uuid::new_v4()),
            None,
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let old = set(&pool, "nary.events.max_attempts", "2").await;
    for _ in 0..3 {
        events::dispatch_pending(None).await.unwrap(); // the third: parked, not tried again
    }
    restore(&pool, "nary.events.max_attempts", old).await;
    events::unsubscribe(id);
    let (attempts, dispatched, last_error): (i32, Option<chrono::DateTime<chrono::Utc>>, Option<String>) =
        sqlx::query_as("SELECT attempts, dispatched_at, last_error FROM domain_events WHERE type = 'payment.recorded'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((attempts, dispatched), (2, None));
    assert!(last_error.unwrap().contains("blew up"));
    assert!(
        count(
            &pool,
            "SELECT count(*) FROM notifications WHERE type = 'nary_error'"
        )
        .await
            >= 1
    );
    {
        let mut conn = pool.acquire().await.unwrap(); // the same problem again: deduplicated
        assert!(!governance::error_card(
            &mut conn,
            "event:payment.recorded",
            "again",
            "x",
            "/nary/runs"
        )
        .await
        .unwrap());
    }

    // ─── test_pause_holds_events_until_resumed ────────────────────────────────
    wipe(&pool).await;
    {
        let mut tx = pool.begin().await.unwrap();
        events::emit(
            &mut tx,
            "lead.created",
            Some("lead"),
            Some(Uuid::new_v4()),
            None,
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let old = set(&pool, "nary.paused", "true").await;
    // 29 Oct 2026 (Owner decision): while paused the quiet subscribers run (lead.created: the WhatsApp labels) and
    // the event waits for Nary's own ones
    assert_eq!(
        events::dispatch_pending(None).await.unwrap(),
        json!({"dispatched": 0, "quiet": 1, "failed": 0, "paused": true})
    );
    restore(&pool, "nary.paused", old).await;
    let r = events::dispatch_pending(None).await.unwrap();
    assert_eq!(r["dispatched"], 1, "{r}");

    // ─── test_runs_and_steps_add_up ───────────────────────────────────────────
    wipe(&pool).await;
    {
        let mut tx = pool.begin().await.unwrap();
        let mut run = runs::start_run(
            &mut tx,
            "manual",
            "nary.card",
            None,
            "internal",
            Some("SOP-001"),
        )
        .await
        .unwrap();
        let mut res = ChatResult::text("");
        res.prompt_tokens = 100;
        res.completion_tokens = 10;
        res.cache_read_tokens = 900;
        res.model_used = "claude-opus-5-5".into();
        runs::add_step(
            &mut tx,
            &mut run,
            "llm",
            Some("draft"),
            runs::StepOpts {
                result: Some(&res),
                cost: Some("0.0015".parse().unwrap()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let (i, o) = (json!({"q": "x"}), json!({"n": 1}));
        runs::add_step(
            &mut tx,
            &mut run,
            "tool",
            Some("lookup"),
            runs::StepOpts {
                input: Some(&i),
                output: Some(&o),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        runs::finish_run(&mut tx, &mut run, "done", Some("ok"), None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let row: (i32, i32, Option<String>, Decimal, String) = sqlx::query_as(
            "SELECT prompt_tokens, cache_read_tokens, model, est_cost, status FROM agent_runs WHERE id = $1",
        )
        .bind(run.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                100,
                900,
                Some("claude-opus-5-5".into()),
                "0.001500".parse().unwrap(),
                "done".into()
            )
        );
        assert_eq!(row.3.to_string(), "0.001500");
        let seqs: Vec<i32> =
            sqlx::query_scalar("SELECT seq FROM agent_steps WHERE run_id = $1 ORDER BY seq")
                .bind(run.id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(seqs, vec![1, 2]);
    }

    // ─── test_ask_level_makes_a_card_that_applies_as_the_approver ─────────────
    wipe(&pool).await;
    let lead1 = lead(&pool).await;
    let card = {
        let mut tx = pool.begin().await.unwrap();
        let c = executor::propose(
            &mut tx,
            "crm.record_change",
            change(lead1, json!({"title": "Renamed by Nary"})),
            &admin,
            executor::ProposeOpts {
                rationale: Some("Customer gave the project name"),
                sop_ref: Some("SOP-002"),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        c
    };
    assert_eq!((card.status.as_str(), card.level), ("pending", 1));
    assert_eq!(
        card.diff.as_ref().unwrap()["after"]["title"],
        "Renamed by Nary"
    );
    assert_ne!(
        lead_field(&pool, lead1, "title").await.as_deref(),
        Some("Renamed by Nary")
    ); // nothing changed yet
    let note_type: String =
        sqlx::query_scalar("SELECT type FROM notifications WHERE entity_id = $1 LIMIT 1")
            .bind(card.id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(note_type, "nary_card");
    // Someone without crm:edit_lead can't apply it: the decider's permissions apply.
    let weak = make_user(&pool, false, &["nary:approve"]).await;
    {
        let mut tx = pool.begin().await.unwrap();
        let err = executor::decide(&mut tx, card.id, &weak, "apply", None, None)
            .await
            .unwrap_err();
        assert!(
            governance::is_http_exception(&err)
                || matches!(err, granary_core::errors::ApiError::App { .. }),
            "{err:?}"
        );
        let _ = tx.rollback().await;
    }
    assert_eq!(
        count(
            &pool,
            &format!(
                "SELECT count(*) FROM agent_pending_actions WHERE id = '{}' AND status = 'failed'",
                card.id
            )
        )
        .await,
        1
    );
    assert_ne!(
        lead_field(&pool, lead1, "title").await.as_deref(),
        Some("Renamed by Nary")
    );
    let lead2 = lead(&pool).await;
    let card2 = {
        let mut tx = pool.begin().await.unwrap();
        let c = executor::propose(
            &mut tx,
            "crm.record_change",
            change(lead2, json!({"title": "Better"})),
            &admin,
            executor::ProposeOpts::default(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        c
    };
    let done = {
        let mut tx = pool.begin().await.unwrap();
        let d = executor::decide(
            &mut tx,
            card2.id,
            &admin,
            "apply",
            Some(change(lead2, json!({"title": "Better title"}))),
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        d
    };
    assert!(
        done.status == "applied" && done.edit_ratio.unwrap() > Decimal::ZERO,
        "{done:?}"
    );
    assert_eq!(
        lead_field(&pool, lead2, "title").await.as_deref(),
        Some("Better title")
    );
    let origin: String = sqlx::query_scalar(
        "SELECT origin FROM audit_logs WHERE entity_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(lead2.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(origin, "nary");

    // ─── test_a_reject_needs_a_reason ─────────────────────────────────────────
    wipe(&pool).await;
    let lead3 = lead(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let card = executor::propose(
        &mut conn,
        "crm.record_change",
        change(lead3, json!({"title": "x"})),
        &admin,
        executor::ProposeOpts::default(),
    )
    .await
    .unwrap();
    let err = executor::decide(&mut conn, card.id, &admin, "reject", None, None)
        .await
        .unwrap_err();
    assert!(detail(&err).contains("why"));
    let done = executor::decide(
        &mut conn,
        card.id,
        &admin,
        "reject",
        None,
        Some("Wrong lead"),
    )
    .await
    .unwrap();
    assert_eq!(
        (done.status.as_str(), done.reject_reason.as_deref()),
        ("rejected", Some("Wrong lead"))
    );
    let err = executor::decide(&mut conn, card.id, &admin, "apply", None, None)
        .await
        .unwrap_err();
    assert!(detail(&err).contains("already rejected"));

    // ─── test_customer_facing_actions_never_send_in_shadow_mode_whatever_the_level
    wipe(&pool).await;
    const { assert!(granary_services::nary::actions::CUSTOMER_SENDING_ENABLED) };
    let old_levels = set(
        &pool,
        "nary.autonomy.levels",
        r#"{"whatsapp.reply.send": 3}"#,
    )
    .await;
    let old_shadow = set(&pool, "nary.shadow_mode", "true").await;
    let reply =
        json!({"channel": "whatsapp", "chat_id": "81901234@c.us", "text": "Hello, thank you!"});
    let card = executor::propose(
        &mut conn,
        "whatsapp.reply.send",
        reply,
        &admin,
        executor::ProposeOpts::default(),
    )
    .await
    .unwrap();
    restore(&pool, "nary.shadow_mode", old_shadow).await;
    restore(&pool, "nary.autonomy.levels", old_levels.clone()).await;
    assert_eq!(
        (card.status.as_str(), card.level, card.customer_facing),
        ("shadow", 0, true)
    );
    let ratio = executor::compare(&mut conn, card.id, Some("Hello, thank you very much!"))
        .await
        .unwrap();
    assert!(
        Decimal::ZERO < ratio && ratio < Decimal::from(50),
        "{ratio}"
    );

    // ─── test_act_and_tell_applies_and_tells_and_pause_stops_nary_only ────────
    wipe(&pool).await;
    let lead4 = lead(&pool).await;
    set(
        &pool,
        "nary.autonomy.levels",
        r#"{"crm.record_change": 2, "mcp.crm_write": 2}"#,
    )
    .await;
    let card = executor::propose(
        &mut conn,
        "crm.record_change",
        change(lead4, json!({"priority": "high"})),
        &admin,
        executor::ProposeOpts::default(),
    )
    .await
    .unwrap();
    assert_eq!(card.status, "applied");
    assert_eq!(
        lead_field(&pool, lead4, "priority").await.as_deref(),
        Some("high")
    );
    let note_type: String =
        sqlx::query_scalar("SELECT type FROM notifications WHERE entity_id = $1 LIMIT 1")
            .bind(card.id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(note_type, "nary_done");
    let old_paused = set(&pool, "nary.paused", "true").await;
    let err = executor::propose(
        &mut conn,
        "crm.record_change",
        change(lead4, json!({"priority": "low"})),
        &admin,
        executor::ProposeOpts::default(),
    )
    .await
    .unwrap_err();
    assert!(executor::is_nary_paused(&err));
    // an MCP client is a person's tool, not Nary; mcp.crm_write stays on never-promote, so a card
    let card = executor::propose(
        &mut conn,
        "mcp.crm_write",
        change(lead4, json!({"priority": "urgent"})),
        &admin,
        executor::ProposeOpts {
            origin: "mcp",
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(card.status == "pending" && card.origin == "mcp", "{card:?}");
    restore(&pool, "nary.paused", old_paused).await;
    restore(&pool, "nary.autonomy.levels", old_levels.clone()).await;

    // ─── test_clean_approvals_make_a_candidate_and_edits_demote ───────────────
    wipe(&pool).await;
    let old_count = set(&pool, "nary.autonomy.promotion_count", "3").await;
    set(
        &pool,
        "nary.autonomy.levels",
        r#"{"reminder.add": 1, "quotation.create_draft": 1}"#,
    )
    .await;
    {
        let mut tx = pool.begin().await.unwrap();
        let db = &mut *tx;
        assert_eq!(
            autonomy::record_decision(db, "reminder.add", "approved", Decimal::ZERO)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            autonomy::record_decision(db, "reminder.add", "approved", Decimal::from(5))
                .await
                .unwrap(),
            None
        ); // small edit
        assert_eq!(
            autonomy::record_decision(db, "reminder.add", "approved", Decimal::ZERO)
                .await
                .unwrap(),
            Some("candidate")
        );
        let c = autonomy::candidates(db).await.unwrap();
        assert_eq!(
            c.iter()
                .map(|c| c["action"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["reminder.add"]
        );
        assert_eq!(
            autonomy::promote(db, "reminder.add", Some(&admin))
                .await
                .unwrap(),
            2
        );
        assert_eq!(autonomy::level_for(db, "reminder.add").await.unwrap(), 2);
        // A meaningful edit demotes one level and resets the streak
        assert_eq!(
            autonomy::record_decision(db, "reminder.add", "approved", Decimal::from(40))
                .await
                .unwrap(),
            Some("demoted")
        );
        assert_eq!(autonomy::level_for(db, "reminder.add").await.unwrap(), 1);
        // At the floor, a reject doesn't go lower
        assert_eq!(
            autonomy::record_decision(db, "reminder.add", "rejected", Decimal::ZERO)
                .await
                .unwrap(),
            None
        );
        // Never-promote actions never become candidates, and can't be raised by hand
        for _ in 0..3 {
            assert_eq!(
                autonomy::record_decision(db, "quotation.create_draft", "approved", Decimal::ZERO)
                    .await
                    .unwrap(),
                None
            );
        }
        let e = autonomy::set_level(db, "quotation.create_draft", 2, Some(&admin))
            .await
            .unwrap_err();
        assert!(detail(&e).contains("never-promote"));
        let e = autonomy::set_never_promote(db, &[], &admin)
            .await
            .unwrap_err();
        assert!(detail(&e).contains("locked"));
        tx.rollback().await.unwrap();
    }
    restore(&pool, "nary.autonomy.promotion_count", old_count).await;
    restore(&pool, "nary.autonomy.levels", old_levels.clone()).await;
    wipe(&pool).await;

    // ─── Quiet hours / pacing / failures at the executor (test_nary_quiet_hours.py, without WhatsApp) ──
    quiet_hours_and_failures(&pool, &admin, old_levels).await;
}

fn fake(name: &str, kind: &'static str) -> actions::Action {
    use granary_services::nary::actions::{apply_fn, Action, Field, Schema, Ty};
    Action::new(
        name,
        &format!("Test {kind}"),
        Schema::strict("Fake", vec![Field::req("text", Ty::Str)]),
        apply_fn(move |_db, _a, p| {
            let text = p["text"].clone();
            Box::pin(async move {
                if wa_send::owner_decision() {
                    return Ok(json!({"sent": text}));
                }
                Err(match kind {
                    "quiet" => wa_send::SendError::QuietHours {
                        until: chrono::Utc::now(),
                        local_label: "Thu 08:00".into(),
                    }
                    .into(),
                    "paced" => wa_send::SendError::PacingLimit {
                        reason: "Nary's hourly WhatsApp limit is reached (nary.wa.max_per_hour)."
                            .into(),
                    }
                    .into(),
                    _ => granary_core::errors::ApiError::internal(governance::PyException::new(
                        "RuntimeError",
                        "boom",
                    )),
                })
            })
        }),
    )
}

async fn quiet_hours_and_failures(pool: &PgPool, admin: &AuthUser, old_levels: Option<String>) {
    for (n, k) in [
        ("test.quiet", "quiet"),
        ("test.paced", "paced"),
        ("test.boom", "boom"),
    ] {
        actions::register(fake(n, k));
    }
    set(
        pool,
        "nary.autonomy.levels",
        r#"{"test.quiet": 2, "test.paced": 2, "test.boom": 3}"#,
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();
    // Nary's own message at night: scheduled, not failed; the Owner's "1" sends it at once.
    let card = executor::propose(
        &mut conn,
        "test.quiet",
        json!({"text": "hi"}),
        admin,
        executor::ProposeOpts::default(),
    )
    .await
    .unwrap();
    assert_eq!(card.status, "scheduled", "{card:?}");
    assert_eq!(card.result.as_ref().unwrap()["when"], "Thu 08:00");
    let done = executor::decide(&mut conn, card.id, admin, "apply", None, None)
        .await
        .unwrap();
    assert_eq!(done.status, "applied");
    assert_eq!(done.result.unwrap(), json!({"sent": "hi"}));
    // A pacing limit keeps it (and the rest) scheduled.
    let card = executor::propose(
        &mut conn,
        "test.paced",
        json!({"text": "x"}),
        admin,
        executor::ProposeOpts::default(),
    )
    .await
    .unwrap();
    assert_eq!(card.status, "scheduled");
    assert_eq!(card.result.as_ref().unwrap()["pacing"], true);
    let out = executor::release_scheduled(&mut conn).await.unwrap();
    assert_eq!(out, executor::Released::default());
    assert_eq!(
        executor::get_card(&mut conn, card.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "scheduled"
    );
    // A failure marks the card failed and raises an error card keyed by the action and its error.
    let err = executor::propose(
        &mut conn,
        "test.boom",
        json!({"text": "x"}),
        admin,
        executor::ProposeOpts::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(governance::plain_error(&err), "RuntimeError: boom");
    let key = format!(
        "action:test.boom:{}",
        executor::_fingerprint(&["RuntimeError: boom"])
    );
    assert!(count(pool, &format!("SELECT count(*) FROM notifications WHERE type = 'nary_error' AND entity_id = '{key}'")).await >= 1);
    for n in ["test.quiet", "test.paced", "test.boom"] {
        actions::unregister(n);
    }
    restore(pool, "nary.autonomy.levels", old_levels).await;
    wipe(pool).await;
}

#[test]
fn test_edit_ratio() {
    let a = json!({"text": "Hello Mr Bob, thank you for your message.", "channel": "whatsapp"});
    assert_eq!(autonomy::edit_ratio(&a, Some(&a.clone())), Decimal::ZERO);
    let mut b = a.clone();
    b["text"] = json!("Hello Mr Bob, thank you for your message!");
    let r = autonomy::edit_ratio(&a, Some(&b));
    assert!(Decimal::ZERO < r && r < Decimal::from(10));
    let mut c = a.clone();
    c["channel"] = json!("email");
    assert_eq!(autonomy::edit_ratio(&a, Some(&c)), Decimal::from(100)); // a non-text field changed
}
