//! The R&D backlog of 26 Oct 2026: double sends (one follow-up per customer per day, trigger keys, kept
//! drafts, the 12 h guard for automatic messages), duplicate deals, proposal review cards that end with the
//! proposal, and email Cc / subject on a thread.

mod nary_n10_common;
mod support;

use chrono::{Duration, Utc};
use granary_db::rows::documents::SalesDocument;
use granary_db::rows::nary::{AgentPendingAction, NaryCadence};
use granary_db::rows::sales::Quotation;
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::crm::deal_duplicates;
use granary_services::nary::{
    cadence, channel_actions, outbound_log, owner_channel, proposal_lifecycle,
};
use nary_n10_common::*;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

static SERIAL: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

async fn db(tag: &str) -> (sqlx::PgPool, sqlx::pool::PoolConnection<sqlx::Postgres>) {
    let pool = support::fresh_db(tag).await;
    granary_services::nary::set_session_pool(pool.clone());
    let conn = pool.acquire().await.unwrap();
    (pool, conn)
}

/// A sent quotation of `deal_id` with its follow-up cadence started `days_ago`.
async fn sent_quotation(db: &mut PgConnection, deal_id: Uuid, cust: &Cust, days_ago: i64) -> Uuid {
    let id = Uuid::new_v4();
    let today = Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO quotations (id, number, revision, currency, status, deal_id, customer_contact_id, customer_org_id, \
         issue_date, valid_until, subtotal, total, is_deleted) \
         VALUES ($1, $2, 1, 'USD', 'sent', $3, $4, $5, $6, $7, 5000, 5000, false)",
    )
    .bind(id)
    .bind(u("RND-Q"))
    .bind(deal_id)
    .bind(cust.contact_id)
    .bind(cust.org_id)
    .bind(today)
    .bind(today + Duration::days(30))
    .execute(&mut *db)
    .await
    .unwrap();
    let q: Quotation = sqlx::query_as("SELECT * FROM quotations WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    cadence::start(
        db,
        &q,
        Some(Utc::now() - Duration::days(days_ago) - Duration::minutes(5)),
    )
    .await
    .unwrap();
    id
}

async fn day_one(db: &mut PgConnection, qid: Uuid) -> NaryCadence {
    sqlx::query_as("SELECT * FROM nary_cadences WHERE quotation_id = $1 AND day = 1")
        .bind(qid)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

fn draft_calls(brain: &Brain) -> Vec<String> {
    brain
        .calls()
        .into_iter()
        .filter(|(job, _)| job == "nary.followup_draft")
        .map(|(_, c)| format!("{c:?}"))
        .collect()
}

// ─── One follow-up per customer per day ────────────────────────────────────────

#[tokio::test]
async fn two_quotations_of_one_deal_get_one_follow_up() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_cad_one").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner(db).await;
    let cust = customer(db, Some("franklin@northwind.test"), true, "en").await;
    let deal = deal_for(db, &cust, "proposal").await;
    // the pilot and the rollout, sent together (23 Oct)
    let pilot = sent_quotation(db, deal, &cust, 1).await;
    let rollout = sent_quotation(db, deal, &cust, 1).await;
    brain.set(
        "nary.followup_draft",
        Answer::json(
            json!({"text": "Good day Franklin, following up on both quotations.", "subject": null}),
        ),
    );
    let out = cadence::tick(db).await.unwrap();
    assert_eq!(out["drafted"], 1, "{out}");
    let (a, b) = (day_one(db, pilot).await, day_one(db, rollout).await);
    assert_eq!(
        (a.status.as_str(), b.status.as_str()),
        ("drafted", "drafted")
    );
    assert!(
        a.card_id.is_some() && a.card_id == b.card_id,
        "one card for both"
    );
    assert_eq!(
        [a.merged_into, b.merged_into]
            .iter()
            .filter(|m| m.is_some())
            .count(),
        1
    );
    assert_eq!(cards(db, "cadence.followup.send").await.len(), 1);
    let calls = draft_calls(&brain);
    assert_eq!(calls.len(), 1, "one draft for the customer");
    let numbers: Vec<String> =
        sqlx::query_scalar("SELECT number FROM quotations WHERE id = ANY($1)")
            .bind(vec![pilot, rollout])
            .fetch_all(&mut *db)
            .await
            .unwrap();
    for n in numbers {
        assert!(calls[0].contains(&n), "the draft names {n}");
    }
}

#[tokio::test]
async fn a_rerun_reuses_the_kept_draft_and_the_card() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_cad_rerun").await;
    let db: &mut PgConnection = &mut conn;
    let brain = Brain::p2();
    let _seam = brain.install().await;
    working_hours(db, true).await;
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    let deal = deal_for(db, &cust, "proposal").await;
    let q = sent_quotation(db, deal, &cust, 1).await;
    brain.set(
        "nary.followup_draft",
        Answer::json(json!({"text": "Following up on the quotation.", "subject": null})),
    );
    cadence::tick(db).await.unwrap();
    let step = day_one(db, q).await;
    assert_eq!(
        step.draft_text.as_deref(),
        Some("Following up on the quotation.")
    );
    // a run that died after the card was made: the step is due again
    sqlx::query("UPDATE nary_cadences SET status = 'scheduled', card_id = NULL WHERE id = $1")
        .bind(step.id)
        .execute(&mut *db)
        .await
        .unwrap();
    cadence::tick(db).await.unwrap();
    assert_eq!(draft_calls(&brain).len(), 1, "never drafted again");
    assert_eq!(
        cards(db, "cadence.followup.send").await.len(),
        1,
        "never a second card"
    );
    assert_eq!(day_one(db, q).await.card_id, step.card_id);
}

// ─── The guard for automatic messages ──────────────────────────────────────────

async fn card_row(db: &mut PgConnection, action: &str, payload: Value) -> AgentPendingAction {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_pending_actions (id, action, origin, status, level, customer_facing, payload) \
         VALUES ($1, $2, 'nary', 'applying', 2, true, $3)",
    )
    .bind(id)
    .bind(action)
    .bind(&payload)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn sent_automatic(db: &mut PgConnection, contact: Uuid, hours_ago: i64) {
    sqlx::query(
        "INSERT INTO nary_outbound_log (id, created_at, channel, contact_id, action, automatic, kind) \
         VALUES ($1, $2, 'whatsapp', $3, 'cadence.followup.send', true, 'reply')",
    )
    .bind(Uuid::new_v4())
    .bind(Utc::now() - Duration::hours(hours_ago))
    .bind(contact)
    .execute(&mut *db)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_second_automatic_message_within_the_gap_waits_for_the_owner() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_guard").await;
    let db: &mut PgConnection = &mut conn;
    let cust = customer(db, Some("adams@globex.test"), true, "en").await;
    let payload = json!({"channel": "whatsapp", "chat_id": cust.chat_id, "text": "Hello again",
                         "contact_id": cust.contact_id.to_string()});
    let card = card_row(db, "cadence.followup.send", payload.clone()).await;
    assert_eq!(
        outbound_log::autosend_gate(db, &card, &payload)
            .await
            .unwrap(),
        None
    );
    sent_automatic(db, cust.contact_id, 2).await;
    let held = outbound_log::autosend_gate(db, &card, &payload)
        .await
        .unwrap();
    assert!(
        held.as_deref().is_some_and(|r| r.contains("12 h")),
        "{held:?}"
    );
    // the Owner's own decision is never held
    sqlx::query("UPDATE agent_pending_actions SET decided_by_id = $2 WHERE id = $1")
        .bind(card.id)
        .bind(owner(db).await)
        .execute(&mut *db)
        .await
        .unwrap();
    let decided: AgentPendingAction =
        sqlx::query_as("SELECT * FROM agent_pending_actions WHERE id = $1")
            .bind(card.id)
            .fetch_one(&mut *db)
            .await
            .unwrap();
    assert_eq!(
        outbound_log::autosend_gate(db, &decided, &payload)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn an_automatic_message_outside_the_gap_goes_and_bcc_waits() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_guard_old").await;
    let db: &mut PgConnection = &mut conn;
    let cust = customer(db, Some("buyer@acme.test"), true, "en").await;
    sent_automatic(db, cust.contact_id, 13).await;
    let payload = json!({"channel": "whatsapp", "chat_id": cust.chat_id, "text": "Hello",
                         "contact_id": cust.contact_id.to_string()});
    let card = card_row(db, "cadence.followup.send", payload.clone()).await;
    assert_eq!(
        outbound_log::autosend_gate(db, &card, &payload)
            .await
            .unwrap(),
        None
    );
    // an automatic email with a blind copy waits for the Owner's yes
    let email = json!({"channel": "email", "to": "buyer@acme.test", "text": "Hello", "bcc": ["boss@granary.test"]});
    let card = card_row(db, "email.reply.send", email.clone()).await;
    let held = outbound_log::autosend_gate(db, &card, &email)
        .await
        .unwrap();
    assert!(
        held.as_deref().is_some_and(|r| r.contains("Bcc")),
        "{held:?}"
    );
}

// ─── Proposal review cards end with the proposal ───────────────────────────────

async fn proposal(db: &mut PgConnection, status: &str) -> SalesDocument {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sales_documents (id, doc_type, number, revision, title, language, status, validity_days, cover, \
         is_deleted) VALUES ($1, 'solution_proposal', $2, 1, 'Laptop rollout', 'en', $3, 30, '{}', false)",
    )
    .bind(id)
    .bind(u("PRP"))
    .bind(status)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query_as("SELECT * FROM sales_documents WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn topic(
    db: &mut PgConnection,
    kind: &str,
    status: &str,
    doc: Uuid,
    key: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    let (ref_type, ref_id) = if key.is_some() {
        (None, None)
    } else {
        (Some("sales_document"), Some(doc))
    };
    sqlx::query(
        "INSERT INTO nary_owner_topics (id, kind, tag, text, ref_type, ref_id, status, facts) \
         VALUES ($1, $2, 'proposal', 'Proposal drafted, tell me what to change', $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(kind)
    .bind(ref_type)
    .bind(ref_id)
    .bind(status)
    .bind(key.map(|k| json!({"key": k})))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn topic_status(db: &mut PgConnection, id: Uuid) -> (String, Option<Value>) {
    sqlx::query_as("SELECT status, facts FROM nary_owner_topics WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

#[tokio::test]
async fn rejecting_a_proposal_closes_its_review_topics_and_cards() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_prop_reject").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    let doc = proposal(db, "review").await;
    let waiting = topic(db, "proposal_review", "waiting", doc.id, None).await;
    let queued = topic(db, "proposal_review", "queued", doc.id, None).await;
    let edit = card_row(
        db,
        "proposal.edit",
        json!({"document_id": doc.id.to_string()}),
    )
    .await;
    let quote = card_row(
        db,
        "quotation.create_draft",
        json!({"proposal_id": doc.id.to_string()}),
    )
    .await;
    for c in [edit.id, quote.id] {
        sqlx::query("UPDATE agent_pending_actions SET status = 'pending' WHERE id = $1")
            .bind(c)
            .execute(&mut *db)
            .await
            .unwrap();
    }
    proposal_lifecycle::reject(db, &doc, &user, "will remake it", None)
        .await
        .unwrap();
    for t in [waiting, queued] {
        let (status, facts) = topic_status(db, t).await;
        assert_eq!(status, "closed");
        assert!(facts.unwrap()["closed_reason"]
            .as_str()
            .unwrap()
            .contains("rejected"));
    }
    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM agent_pending_actions WHERE id = ANY($1)")
            .bind(vec![edit.id, quote.id])
            .fetch_all(&mut *db)
            .await
            .unwrap();
    assert!(statuses.iter().all(|s| s == "cancelled"), "{statuses:?}");
}

#[tokio::test]
async fn a_sent_proposal_never_reaches_the_owner_again() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_prop_sent").await;
    let db: &mut PgConnection = &mut conn;
    let sent = proposal(db, "sent").await;
    let draft = proposal(db, "draft").await;
    let stale = topic(db, "proposal_review", "queued", sent.id, None).await;
    let figure_key = format!("proposal_figure:{}:architecture", sent.id);
    let figure = topic(db, "alert", "queued", sent.id, Some(&figure_key)).await;
    let live = topic(db, "proposal_review", "queued", draft.id, None).await;
    let closed = owner_channel::close_stale_proposal_topics(db)
        .await
        .unwrap();
    assert_eq!(closed, 2);
    assert_eq!(topic_status(db, stale).await.0, "closed");
    assert_eq!(topic_status(db, figure).await.0, "closed");
    assert_eq!(
        topic_status(db, live).await.0,
        "queued",
        "a draft still asks"
    );
}

// ─── Duplicate deals ───────────────────────────────────────────────────────────

#[tokio::test]
async fn two_open_deals_of_one_customer_get_one_merge_card() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_dup_deals").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    let cust = customer(db, Some("adams@globex.test"), true, "en").await;
    let older = deal_for(db, &cust, "proposal").await;
    sqlx::query("UPDATE deals SET created_at = now() - interval '3 days' WHERE id = $1")
        .bind(older)
        .execute(&mut *db)
        .await
        .unwrap();
    let newer = deal_for(db, &cust, "new").await;
    let quote = sent_quotation(db, newer, &cust, 0).await;
    assert_eq!(deal_duplicates::scan(db, None).await.unwrap(), 1);
    assert!(deal_duplicates::on_hold(db, newer).await.unwrap());
    assert!(!deal_duplicates::on_hold(db, older).await.unwrap());
    assert_eq!(
        deal_duplicates::scan(db, None).await.unwrap(),
        0,
        "one card per pair"
    );
    let card = cards(db, "deal.merge").await.pop().unwrap();
    assert_eq!(card.payload["keep_id"], json!(older.to_string()));
    // no rule reminders on the held deal
    let made = granary_services::crm::followups::create(
        db,
        granary_services::crm::followups::NewReminder {
            owner_user_id: me,
            source: "stale_deal".into(),
            entity_type: "deal".into(),
            entity_id: Some(newer),
            title: "Deal is stuck".into(),
            rule_key: Some(u("stale")),
            ..granary_services::crm::followups::NewReminder::new(me, "deal", "x")
        },
    )
    .await
    .unwrap();
    assert!(made.is_none());
    // the merge moves everything to the older deal and closes the newer one
    deal_duplicates::merge(db, &user, older, newer)
        .await
        .unwrap();
    let moved: Uuid = sqlx::query_scalar("SELECT deal_id FROM quotations WHERE id = $1")
        .bind(quote)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(moved, older);
    let stage: String = sqlx::query_scalar("SELECT stage FROM deals WHERE id = $1")
        .bind(newer)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(stage, "disqualified");
}

#[tokio::test]
async fn not_a_duplicate_lifts_the_hold_for_good() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_dup_dismiss").await;
    let db: &mut PgConnection = &mut conn;
    owner(db).await;
    let cust = customer(db, None, true, "en").await;
    let a = deal_for(db, &cust, "proposal").await;
    let b = deal_for(db, &cust, "proposal").await;
    deal_duplicates::scan(db, None).await.unwrap();
    let (keep, merge) = {
        let c = cards(db, "deal.merge").await.pop().unwrap();
        (
            Uuid::parse_str(c.payload["keep_id"].as_str().unwrap()).unwrap(),
            Uuid::parse_str(c.payload["merge_id"].as_str().unwrap()).unwrap(),
        )
    };
    assert!([a, b].contains(&keep) && [a, b].contains(&merge));
    deal_duplicates::dismiss(db, keep, merge).await.unwrap();
    assert!(!deal_duplicates::on_hold(db, merge).await.unwrap());
    sqlx::query("DELETE FROM agent_pending_actions WHERE action = 'deal.merge'")
        .execute(&mut *db)
        .await
        .unwrap();
    assert_eq!(
        deal_duplicates::scan(db, None).await.unwrap(),
        0,
        "never asked again"
    );
}

#[tokio::test]
async fn the_merge_list_covers_every_column_that_points_at_a_deal() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_dup_schema").await;
    let db: &mut PgConnection = &mut conn;
    let fks: Vec<(String, String)> = sqlx::query_as(
        "SELECT tc.table_name::text, kcu.column_name::text FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu ON kcu.constraint_name = tc.constraint_name \
         JOIN information_schema.constraint_column_usage ccu ON ccu.constraint_name = tc.constraint_name \
         WHERE tc.constraint_type = 'FOREIGN KEY' AND ccu.table_name = 'deals' AND ccu.column_name = 'id'",
    )
    .fetch_all(&mut *db)
    .await
    .unwrap();
    for (table, column) in fks {
        let listed = deal_duplicates::RELATED
            .iter()
            .any(|(t, c, _)| *t == table && *c == column)
            || deal_duplicates::KEPT_WITH_MERGED
                .iter()
                .any(|(t, c)| *t == table && *c == column)
            || table == "nary_outbound_log";
        assert!(
            listed,
            "{table}.{column} points at a deal but the merge doesn't know it"
        );
    }
}

// ─── Email: Cc and subject on a thread ─────────────────────────────────────────

#[tokio::test]
async fn a_reply_goes_to_everyone_on_the_thread_with_its_real_subject() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_email_cc").await;
    let db: &mut PgConnection = &mut conn;
    let ours: String = sqlx::query_scalar("SELECT address FROM crm_mailboxes WHERE is_default")
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let tid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO crm_email_threads (id, subject, status, is_read, message_count) \
         VALUES ($1, 'New Lead [web3forms]: Globex', 'open', true, 3)",
    )
    .bind(tid)
    .execute(&mut *db)
    .await
    .unwrap();
    let msg = |dir: &str,
               from: &str,
               to: Vec<&str>,
               cc: Vec<&str>,
               subject: &str,
               auth: Value,
               mins: i64| {
        (
            dir.to_string(),
            from.to_string(),
            to.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            cc.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            subject.to_string(),
            auth,
            mins,
        )
    };
    for (dir, from, to, cc, subject, auth, mins) in [
        msg(
            "inbound",
            "notify@web3forms.com",
            vec![&ours],
            vec![],
            "New Lead [web3forms]: Globex",
            json!({"web3forms": {"ok": true}}),
            300,
        ),
        msg(
            "outbound",
            &ours,
            vec!["rachel@globex.test"],
            vec!["ali@globex.test"],
            "Tender documents",
            json!({}),
            200,
        ),
        msg(
            "inbound",
            "rachel@globex.test",
            vec![&ours],
            vec!["richard@globex.test", &ours],
            "Re: Tender documents",
            json!({}),
            100,
        ),
    ] {
        sqlx::query(
            "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, to_emails, \
             cc_emails, subject, is_read, received_at, auth) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, true, $9, $10)",
        )
        .bind(Uuid::new_v4())
        .bind(tid)
        .bind(format!("<{}@test>", Uuid::new_v4()))
        .bind(dir)
        .bind(from)
        .bind(to)
        .bind(cc)
        .bind(subject)
        .bind(Utc::now() - Duration::minutes(mins))
        .bind(auth)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    let cc = granary_services::crm_inbox::thread_participants(db, tid, "rachel@globex.test")
        .await
        .unwrap();
    assert_eq!(cc, vec!["richard@globex.test", "ali@globex.test"]);
    let subject = granary_services::crm_inbox::thread_subject(db, tid)
        .await
        .unwrap();
    assert_eq!(subject, "Tender documents");
    // Nary's own reply card says it all before it goes
    let mut payload = json!({"channel": "email", "thread_id": tid.to_string(), "text": "Dear Rachel,",
                             "to": "rachel@globex.test"});
    channel_actions::email_card_defaults(db, "email.reply.send", &mut payload)
        .await
        .unwrap();
    assert_eq!(
        payload["cc"],
        json!(["richard@globex.test", "ali@globex.test"])
    );
    assert_eq!(payload["subject"], json!("Re: Tender documents"));
    assert_eq!(payload["from_inbox"], json!(ours));
    assert!(payload.get("bcc").is_none(), "Nary never adds Bcc");
}

#[tokio::test]
async fn an_acknowledgement_never_holds_the_real_answer() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_guard_ack").await;
    let db: &mut PgConnection = &mut conn;
    let cust = customer(db, None, true, "en").await;
    sqlx::query(
        "INSERT INTO nary_outbound_log (id, channel, chat_id, contact_id, action, automatic, kind) \
         VALUES ($1, 'whatsapp', $2, $3, 'whatsapp.ack.send', true, 'ack')",
    )
    .bind(Uuid::new_v4())
    .bind(&cust.chat_id)
    .bind(cust.contact_id)
    .execute(&mut *db)
    .await
    .unwrap();
    let payload = json!({"channel": "whatsapp", "chat_id": cust.chat_id, "text": "Sure Mr"});
    let card = card_row(db, "whatsapp.reply.send", payload.clone()).await;
    assert_eq!(
        outbound_log::autosend_gate(db, &card, &payload)
            .await
            .unwrap(),
        None
    );
}

// ─── Second batch: deleted activities, linked threads, the thread card, inbox switches ──────────

async fn activity(
    db: &mut PgConnection,
    lead: Uuid,
    by: Uuid,
    subject: &str,
    channel: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, lead_id, activity_type, direction, subject, content, \
         is_completed, completed_at, channel_message_id, created_by_id, next_action, next_action_due) \
         VALUES ($1, 'lead', $2, $2, 'email', 'inbound', $3, 'Hello', true, now(), $4, $5, 'Send the deck', \
         current_date)",
    )
    .bind(id)
    .bind(lead)
    .bind(subject)
    .bind(channel)
    .bind(by)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_deleted_activity_is_kept_restorable_and_its_follow_up_goes() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_act_void").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    let cust = customer(db, Some("eduard@dmci.test"), true, "en").await;
    let lead = cust.lead_id.unwrap();
    let id = activity(db, lead, me, "Request for presentation", None).await;
    sqlx::query(
        "INSERT INTO crm_reminders (id, owner_user_id, source, entity_type, entity_id, title, due_at, original_due_at, \
         status, snooze_count, rule_key) VALUES ($1, $2, 'next_action', 'lead', $3, 'Next action due', now(), now(), \
         'open', 0, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(me)
    .bind(lead)
    .bind(format!("next_action:{id}:2026-10-26"))
    .execute(&mut *db)
    .await
    .unwrap();
    let out = granary_services::crm::activity_void::void(
        db,
        &user,
        id,
        "duplicate of the linked thread",
        false,
    )
    .await
    .unwrap();
    assert_eq!(out["reminders_cancelled"], 1);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM activities WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(left, 0);
    let voided = Uuid::parse_str(out["voided_id"].as_str().unwrap()).unwrap();
    granary_services::crm::activity_void::restore(db, &user, voided)
        .await
        .unwrap();
    let back: String = sqlx::query_scalar("SELECT subject FROM activities WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(back, "Request for presentation");
    // a synced email is not Nary's to delete
    let synced = activity(db, lead, me, "Synced", Some("<abc@x>")).await;
    let refused = granary_services::crm::activity_void::_h_propose_activity_delete(
        db,
        &user,
        json!({"activity_id": synced.to_string(), "reason": "test"}),
    )
    .await
    .unwrap();
    assert!(
        refused["error"].as_str().unwrap().contains("synced"),
        "{refused}"
    );
}

async fn thread_with_mail(db: &mut PgConnection, customer_email: &str) -> Uuid {
    let tid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO crm_email_threads (id, subject, status, is_read, message_count, last_sender_email, \
         last_sender_name, last_message_at) VALUES ($1, 'Request for presentation', 'open', true, 2, $2, 'Robert', now())",
    )
    .bind(tid)
    .bind(customer_email)
    .execute(&mut *db)
    .await
    .unwrap();
    for (dir, from, to, subject, mins) in [
        (
            "inbound",
            customer_email,
            "hello@example.com",
            "Request for presentation",
            30,
        ),
        (
            "outbound",
            "hello@example.com",
            customer_email,
            "Re: Request for presentation",
            10,
        ),
    ] {
        sqlx::query(
            "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, from_name, \
             to_emails, cc_emails, subject, body_text, is_read, received_at) \
             VALUES ($1, $2, $3, $4, $5, $5, ARRAY[$6], '{}', $7, 'Body', true, now() - make_interval(mins => $8))",
        )
        .bind(Uuid::new_v4())
        .bind(tid)
        .bind(format!("<{}@test>", Uuid::new_v4()))
        .bind(dir)
        .bind(from)
        .bind(to)
        .bind(subject)
        .bind(mins)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    tid
}

#[tokio::test]
async fn a_contact_made_after_the_email_gets_the_thread_and_its_history() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_relink").await;
    let db: &mut PgConnection = &mut conn;
    owner(db).await;
    let tid = thread_with_mail(db, "eduard@dmci.test").await;
    // the contact and its lead come after the emails
    let cust = customer(db, Some("eduard@dmci.test"), true, "en").await;
    let n = granary_services::crm_inbox::relink_sweep(db, 180)
        .await
        .unwrap();
    assert_eq!(n, 1);
    let (contact, lead): (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT linked_contact_id, linked_lead_id FROM crm_email_threads WHERE id = $1",
    )
    .bind(tid)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!((contact, lead), (Some(cust.contact_id), cust.lead_id));
    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM activities WHERE lead_id = $1 AND activity_type = 'email' AND channel_message_id IS NOT NULL",
    )
    .bind(cust.lead_id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(logged, 2, "both emails on the lead's history");
    assert_eq!(
        granary_services::crm_inbox::relink_sweep(db, 180)
            .await
            .unwrap(),
        0,
        "once"
    );
}

#[tokio::test]
async fn linking_a_thread_removes_the_hand_logged_copies() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_link_copies").await;
    let db: &mut PgConnection = &mut conn;
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    let cust = customer(db, Some("buyer@dmci.test"), true, "en").await;
    let lead = cust.lead_id.unwrap();
    let tid = thread_with_mail(db, "someone.else@dmci.test").await;
    // Nary logged the inbound email by hand before the thread was linked
    let copy = activity(db, lead, me, "Request for presentation", None).await;
    let r = granary_services::crm_inbox::link_thread(db, tid, None, Some(lead), None, Some(&user))
        .await
        .unwrap();
    assert_eq!(r.duplicates_voided, vec![copy]);
    assert_eq!(r.logged, 2);
}

#[tokio::test]
async fn the_thread_card_names_the_newest_sender_even_when_it_is_us() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_last_from").await;
    let db: &mut PgConnection = &mut conn;
    let cust = customer(db, Some("eduard@dmci.test"), true, "en").await;
    let tid = thread_with_mail(db, "eduard@dmci.test").await;
    sqlx::query("UPDATE crm_email_threads SET linked_contact_id = $2 WHERE id = $1")
        .bind(tid)
        .bind(cust.contact_id)
        .execute(&mut *db)
        .await
        .unwrap();
    let me = owner(db).await;
    let user = auth_user(db, me).await;
    let items =
        granary_services::crm::timeline::build_timeline(db, &user, "contact", cust.contact_id, 50)
            .await
            .unwrap();
    let card = items
        .iter()
        .find(|i| i["kind"] == "email")
        .expect("the thread card");
    let preview = card["preview"].as_str().unwrap();
    assert!(preview.contains("last from us"), "{preview}");
    assert!(preview.contains("waiting on the customer"), "{preview}");
}

#[tokio::test]
async fn an_extra_inbox_with_empty_ssl_switches_uses_ssl() {
    let _s = SERIAL.lock().await;
    let (_pool, mut conn) = db("rnd_inbox_ssl").await;
    let db: &mut PgConnection = &mut conn;
    for k in ["crm_sales_imap_ssl", "crm_sales_smtp_tls"] {
        sqlx::query(
            "INSERT INTO system_settings (id, key, value, category) VALUES (gen_random_uuid(), $1, '', 'crm_email') \
             ON CONFLICT (key) DO UPDATE SET value = ''",
        )
        .bind(k)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    assert!(
        granary_services::email::load_imap_prefixed(db, "crm_sales")
            .await
            .unwrap()
            .ssl
    );
    assert!(
        granary_services::email::load_smtp_prefixed(db, "crm_sales")
            .await
            .unwrap()
            .tls
    );
}
