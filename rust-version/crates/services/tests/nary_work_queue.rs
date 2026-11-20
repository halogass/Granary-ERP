//! 3 Nov 2026 (Owner decisions, Stream A, migration 0154): the work queue while internal Nary is paused.
//!
//! - Every kind appears for its record (nary.paused = true) and disappears once its work is done.
//! - The nudge: at nary.queue.nudge_times on working days, at once for a new urgent item within working hours
//!   but at most once per nary.queue.urgent_gap_minutes, nothing on a weekend or a holiday, nothing when off;
//!   the bell + WhatsApp when the Owner channel is off, a topic in the Owner's chat when it is on.

mod support;

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use granary_services::nary::work_queue::{self, Filter, Item};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn set(db: &mut PgConnection, k: &str, v: &str) {
    let n = sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(k)
        .bind(v)
        .execute(&mut *db)
        .await
        .unwrap();
    assert_eq!(n.rows_affected(), 1, "setting not seeded: {k}");
}

async fn exec(db: &mut PgConnection, sql: &str) {
    sqlx::query(sql).execute(&mut *db).await.unwrap();
}

async fn setup(tag: &str) -> PgPool {
    let pool = support::fresh_db(tag).await;
    let mut db = pool.acquire().await.unwrap();
    set(&mut db, "nary.paused", "true").await;
    let owner: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    sqlx::query(
        "UPDATE nary_role_assignments SET is_nary = false, user_id = $1 WHERE sop_role = 'Owner'",
    )
    .bind(owner)
    .execute(&mut *db)
    .await
    .unwrap();
    pool
}

async fn owner_id(db: &mut PgConnection) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

fn phone() -> String {
    format!("8190{}", &Uuid::new_v4().as_u128().to_string()[..8])
}

async fn org(db: &mut PgConnection, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES ($1, $2, false, '{}')",
    )
    .bind(id)
    .bind(name)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn contact(db: &mut PgConnection, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, is_primary, tags, is_deleted, custom) \
         VALUES ($1, $2, false, '{}', false, '{}')",
    )
    .bind(id)
    .bind(name)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// A chat (linked to `contact` or not) where the customer wrote at `at`: its chat id.
async fn chat(db: &mut PgConnection, contact: Option<Uuid>, at: DateTime<Utc>) -> String {
    let p = phone();
    let chat_id = format!("{p}@c.us");
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, \
         contact_id, triage, language, attribution_asked, last_inbound_at) \
         VALUES ($1, $2, $3, 'Bob', false, 'active', true, $4, 'prospect', 'id', false, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(&chat_id)
    .bind(format!("+{p}"))
    .bind(contact)
    .bind(at)
    .execute(&mut *db)
    .await
    .unwrap();
    message(
        db,
        &chat_id,
        false,
        "Hello, what is the price of your laptops?",
        at,
    )
    .await;
    chat_id
}

async fn message(
    db: &mut PgConnection,
    chat_id: &str,
    from_me: bool,
    body: &str,
    at: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body, wa_ts) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().simple().to_string()[..20].to_uppercase())
    .bind(chat_id)
    .bind(if from_me { "outbound" } else { "inbound" })
    .bind(from_me)
    .bind(body)
    .bind(at)
    .execute(&mut *db)
    .await
    .unwrap();
}

async fn queue(db: &mut PgConnection) -> Vec<Item> {
    work_queue::compute(db, Utc::now()).await.unwrap()
}

fn has(items: &[Item], id: &str) -> Option<Item> {
    items.iter().find(|i| i.id == id).cloned()
}

#[tokio::test(flavor = "current_thread")]
async fn every_kind_appears_and_goes_when_handled_while_paused() {
    let _g = SERIAL.lock().await;
    let pool = setup("work_queue_items").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    assert!(granary_services::nary::governance::is_paused(db)
        .await
        .unwrap());
    let owner = owner_id(db).await;
    let now = Utc::now();
    let tz = chrono_tz::Asia::Tokyo;
    let today = granary_core::clock::today_in(tz);

    // whatsapp_reply: a known customer had the last word
    let bob = contact(db, "Bob").await;
    let known = chat(db, Some(bob), now - Duration::hours(2)).await;
    // unknown_chat: a number with no lead, contact or deal
    let stranger = chat(db, None, now - Duration::hours(1)).await;
    // email_reply
    let thread = Uuid::new_v4();
    exec(db, &format!(
        "INSERT INTO crm_email_threads (id, subject, status, last_message_at) VALUES ('{thread}', 'Laptop quotation', 'open', now())"
    )).await;
    exec(db, &format!(
        "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, from_name, body_text, received_at) \
         VALUES (gen_random_uuid(), '{thread}', '<a@x>', 'inbound', 'sarah@acme.example', 'Sarah', 'Could you send a quotation?', now() - interval '3 hours')"
    )).await;
    // new_lead
    let lead_org = org(db, "New Lead Ltd").await;
    let lead = Uuid::new_v4();
    exec(
        db,
        &format!(
            "INSERT INTO leads (id, organization_id, source, status, title, is_deleted, custom) \
         VALUES ('{lead}', '{lead_org}', 'website', 'new', 'Laptops for 12 staff', false, '{{}}')"
        ),
    )
    .await;
    // held_chat
    let held = format!("{}@c.us", phone());
    exec(db, &format!(
        "INSERT INTO wa_chats (id, chat_id, display_name, is_my_contact, status, attribution_asked) \
         VALUES (gen_random_uuid(), '{held}', 'Mama', true, 'held', false)"
    )).await;
    // followup_due
    let reminder = Uuid::new_v4();
    exec(db, &format!(
        "INSERT INTO crm_reminders (id, owner_user_id, source, entity_type, entity_id, title, due_at, status, snooze_count) \
         VALUES ('{reminder}', '{owner}', 'manual', 'lead', '{lead}', 'Call Mr Andy', now() - interval '1 hour', 'open', 0)"
    )).await;
    // quotations: one sent and expiring in 3 days, one expired yesterday
    let deal_org = org(db, "Vandelay Industries Ltd").await;
    let deal = Uuid::new_v4();
    exec(
        db,
        &format!(
            "INSERT INTO deals (id, organization_id, title, stage, is_deleted, custom) \
         VALUES ('{deal}', '{deal_org}', 'Vandelay laptop refresh', 'negotiation', false, '{{}}')"
        ),
    )
    .await;
    let q_soon = Uuid::new_v4();
    let q_old = Uuid::new_v4();
    for (id, no, status, valid) in [
        (q_soon, "Q-SOON", "sent", today + Duration::days(3)),
        (q_old, "Q-OLD", "expired", today - Duration::days(1)),
    ] {
        sqlx::query(
            "INSERT INTO quotations (id, number, status, deal_id, customer_org_id, issue_date, valid_until, is_deleted) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, false)",
        )
        .bind(id)
        .bind(no)
        .bind(status)
        .bind(if no == "Q-SOON" { Some(deal) } else { None })
        .bind(deal_org)
        .bind(today - Duration::days(20))
        .bind(valid)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    // a meeting in two hours with no reminder and no brief, a recording waiting
    let meeting = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO nary_meetings (id, deal_id, chat_id, title, status, starts_at, ends_at) \
         VALUES ($1, $2, $3, 'Your Company x Vandelay Industries', 'booked', $4, $5)",
    )
    .bind(meeting)
    .bind(deal)
    .bind(&known)
    .bind(now + Duration::hours(2))
    .bind(now + Duration::hours(3))
    .execute(&mut *db)
    .await
    .unwrap();
    let file = Uuid::new_v4();
    exec(db, &format!(
        "INSERT INTO nary_meeting_files (id, kind, meeting_id, filename, object_key, source, status) \
         VALUES ('{file}', 'recording', '{meeting}', 'call.m4a', 'k/call.m4a', 'upload', 'queued')"
    )).await;
    // duplicate deals (same organization, both open) and duplicate leads (same contact)
    let dup = Uuid::new_v4();
    exec(db, &format!(
        "INSERT INTO deals (id, organization_id, title, stage, is_deleted, custom, created_at) \
         VALUES ('{dup}', '{deal_org}', 'Vandelay Industries (website form)', 'new', false, '{{}}', now() + interval '1 second')"
    )).await;
    let andy = contact(db, "Andy").await;
    let (l1, l2) = (Uuid::new_v4(), Uuid::new_v4());
    for (id, secs) in [(l1, 0), (l2, 5)] {
        exec(db, &format!(
            "INSERT INTO leads (id, contact_id, source, status, title, is_deleted, custom, created_at) \
             VALUES ('{id}', '{andy}', 'email', 'qualifying', 'Andy parking', false, '{{}}', now() - interval '2 days' + interval '{secs} seconds')"
        )).await;
    }
    // cards: one fresh, one expiring within a day (nary.pending.expire_days 7)
    let (card, old_card) = (Uuid::new_v4(), Uuid::new_v4());
    exec(db, &format!(
        "INSERT INTO agent_pending_actions (id, action, origin, status, level, payload, rationale, created_at) VALUES \
         ('{card}', 'deal.stage', 'mcp', 'pending', 1, '{{}}', 'Move to proposal', now()), \
         ('{old_card}', 'deal.stage', 'nary', 'pending', 1, '{{}}', 'Old one', now() - interval '6 days 20 hours')"
    )).await;
    // an overdue approval
    let request = Uuid::new_v4();
    let due = (now - Duration::hours(5)).to_rfc3339();
    sqlx::query(
        "INSERT INTO approval_requests (id, entity_type, entity_id, status, document_revision, document_label, \
         submitted_at, current_step_seq, snapshot, flags, steps) \
         VALUES ($1, 'quotation', $2, 'pending', 1, 'Quotation Q-SOON', now() - interval '2 days', 1, '{}', '{}', $3)",
    )
    .bind(request)
    .bind(q_soon)
    .bind(json!([{"seq": 1, "name": "Owner approval", "due_at": due, "candidate_user_ids": [owner.to_string()]}]))
    .execute(&mut *db)
    .await
    .unwrap();

    let items = queue(db).await;
    let want = [
        (
            format!("whatsapp_reply:{known}"),
            "whatsapp_reply",
            "urgent",
            Some("get_whatsapp_messages"),
        ),
        (
            format!("unknown_chat:{stranger}"),
            "unknown_chat",
            "urgent",
            Some("propose_whatsapp_lead"),
        ),
        (
            format!("email_reply:{thread}"),
            "email_reply",
            "urgent",
            Some("get_email_thread"),
        ),
        (
            format!("new_lead:{lead}"),
            "new_lead",
            "urgent",
            Some("score_lead"),
        ),
        (
            format!("held_chat:{held}"),
            "held_chat",
            "normal",
            Some("propose_held_chat_decision"),
        ),
        (
            format!("followup_due:{reminder}"),
            "followup_due",
            "today",
            Some("propose_reminder_change"),
        ),
        (
            format!("quotation_expiring:{q_soon}"),
            "quotation_expiring",
            "today",
            Some("propose_customer_message"),
        ),
        (
            format!("quotation_expired:{q_old}"),
            "quotation_expired",
            "normal",
            Some("propose_quotation_revision"),
        ),
        (
            format!("meeting_reminder:{meeting}"),
            "meeting_reminder",
            "today",
            Some("propose_customer_message"),
        ),
        (
            format!("meeting_brief:{meeting}"),
            "meeting_brief",
            "today",
            Some("get_activity_history"),
        ),
        (
            format!("meeting_file:{file}"),
            "meeting_file",
            "normal",
            Some("transcribe_meeting_file"),
        ),
        (
            format!("duplicate_deal:{deal}:{dup}"),
            "duplicate_deal",
            "normal",
            Some("propose_deal_merge"),
        ),
        (
            format!("duplicate_lead:{l1}:{l2}"),
            "duplicate_lead",
            "normal",
            Some("propose_lead_merge"),
        ),
        (
            format!("card:{card}"),
            "card_pending",
            "normal",
            Some("nary_decide_card"),
        ),
        (
            format!("card:{old_card}"),
            "card_expiring",
            "today",
            Some("nary_decide_card"),
        ),
        (
            format!("approval_overdue:{request}"),
            "approval_overdue",
            "today",
            None,
        ),
    ];
    for (id, kind, urgency, tool) in &want {
        let it = has(&items, id).unwrap_or_else(|| {
            panic!(
                "{id} missing: {:?}",
                items.iter().map(|i| &i.id).collect::<Vec<_>>()
            )
        });
        assert_eq!(
            (it.kind, it.urgency, it.suggested_tool),
            (*kind, *urgency, *tool),
            "{id}"
        );
        assert!(!it.title.contains('\u{2014}'), "{}", it.title);
        assert!(!it.link.is_empty(), "{id}");
    }
    let wa = has(&items, &format!("whatsapp_reply:{known}")).unwrap();
    assert_eq!(wa.suggested_args, json!({"chat_id": known}));
    assert_eq!(wa.subject["contact_id"], json!(bob.to_string()));
    // urgent first, then today, then normal
    let ranks: Vec<usize> = items
        .iter()
        .map(|i| {
            ["urgent", "today", "normal"]
                .iter()
                .position(|u| *u == i.urgency)
                .unwrap()
        })
        .collect();
    assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "{ranks:?}");

    // Nary's chat: the Owner has the tool and gets the same items; a staff member without Nary's pages does not
    let owner_user = granary_services::nary::roles::load_actor(db, owner)
        .await
        .unwrap()
        .unwrap();
    use granary_services::ai::tools::catalog::{run_tool, tools_for_user};
    assert!(tools_for_user(&owner_user)
        .iter()
        .any(|t| t.name == "nary_work_queue"));
    let out = run_tool(
        db,
        &owner_user,
        "nary_work_queue",
        json!({"kinds": ["new_lead"]}),
    )
    .await;
    assert_eq!(
        out["items"][0]["id"],
        json!(format!("new_lead:{lead}")),
        "{out}"
    );
    let mut staff = owner_user.clone();
    staff.id = Uuid::new_v4();
    staff.is_superadmin = false;
    staff.is_system_admin = false;
    staff.perms = ["crm:view_all".to_string()].into_iter().collect();
    assert!(!tools_for_user(&staff)
        .iter()
        .any(|t| t.name == "nary_work_queue"));
    staff.perms.insert("nary:view".into());
    let out = run_tool(db, &staff, "nary_work_queue", json!({})).await;
    assert_eq!(
        out["error"],
        json!("The work queue is for the Owner and system admins."),
        "{out}"
    );
    let out = run_tool(
        db,
        &owner_user,
        "nary_work_queue",
        json!({"kinds": ["nope"]}),
    )
    .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .starts_with("Unknown kind 'nope'"),
        "{out}"
    );

    // the filters
    let out = work_queue::list(
        db,
        &Filter::from_args(&json!({"urgency": "urgent"})).unwrap(),
    )
    .await
    .unwrap();
    assert!(out["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["urgency"] == "urgent"));
    assert_eq!(out["nary_paused"], json!(true));
    let out = work_queue::list(
        db,
        &Filter::from_args(&json!({"kinds": ["held_chat"], "limit": 1})).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(out["shown"], json!(1));
    assert_eq!(out["items"][0]["kind"], json!("held_chat"));

    // close-out: the expiring quotation is accepted -> no contract yet
    exec(
        db,
        &format!("UPDATE quotations SET status = 'accepted' WHERE id = '{q_soon}'"),
    )
    .await;
    let items = queue(db).await;
    assert!(has(&items, &format!("quotation_expiring:{q_soon}")).is_none());
    assert!(has(&items, &format!("accepted_no_contract:{q_soon}")).is_some());
    // a contract draft -> no proforma yet
    let contract = Uuid::new_v4();
    exec(db, &format!(
        "INSERT INTO contracts (id, number, quotation_id, deal_id, customer_org_id, status, is_deleted) \
         VALUES ('{contract}', 'C-1', '{q_soon}', '{deal}', '{deal_org}', 'draft', false)"
    )).await;
    let items = queue(db).await;
    assert!(has(&items, &format!("accepted_no_contract:{q_soon}")).is_none());
    assert!(has(&items, &format!("contract_no_proforma:{contract}")).is_some());
    // the proforma is raised, the contract signed -> ready to win
    exec(db, &format!(
        "INSERT INTO invoices (id, number, invoice_type, quotation_id, issue_date, status, amount_paid, is_deleted) \
         VALUES (gen_random_uuid(), 'PF-1', 'proforma', '{q_soon}', now()::date, 'sent', 0, false)"
    )).await;
    exec(
        db,
        &format!("UPDATE contracts SET status = 'signed' WHERE id = '{contract}'"),
    )
    .await;
    let items = queue(db).await;
    assert!(has(&items, &format!("contract_no_proforma:{contract}")).is_none());
    let win = has(&items, &format!("ready_to_win:{deal}")).expect("ready to win");
    assert_eq!(win.suggested_tool, Some("propose_deal_won"));

    // the work is done, one by one
    message(
        db,
        &known,
        true,
        "Sure, we will send the prices",
        now - Duration::minutes(30),
    )
    .await;
    exec(db, &format!(
        "INSERT INTO agent_pending_actions (id, action, origin, status, level, payload) \
         VALUES (gen_random_uuid(), 'whatsapp.reply.send', 'mcp', 'pending', 1, '{{\"chat_id\": \"{stranger}\"}}')"
    )).await;
    exec(db, &format!(
        "INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, body_text, received_at) \
         VALUES (gen_random_uuid(), '{thread}', '<b@x>', 'outbound', 'hello@example.com', 'Attached', now())"
    )).await;
    exec(
        db,
        &format!("UPDATE leads SET status = 'qualifying' WHERE id = '{lead}'"),
    )
    .await;
    exec(
        db,
        &format!("UPDATE wa_chats SET status = 'excluded' WHERE chat_id = '{held}'"),
    )
    .await;
    exec(
        db,
        &format!("UPDATE crm_reminders SET status = 'done' WHERE id = '{reminder}'"),
    )
    .await;
    exec(db, &format!(
        "INSERT INTO quotations (id, number, status, previous_version_id, issue_date, valid_until, is_deleted) \
         VALUES (gen_random_uuid(), 'Q-OLD-2', 'draft', '{q_old}', now()::date, now()::date + 14, false)"
    )).await;
    exec(
        db,
        &format!(
        "UPDATE nary_meetings SET day_reminder_at = now(), brief_at = now() WHERE id = '{meeting}'"
    ),
    )
    .await;
    exec(
        db,
        &format!("UPDATE nary_meeting_files SET status = 'done' WHERE id = '{file}'"),
    )
    .await;
    exec(
        db,
        &format!("UPDATE deals SET stage = 'lost' WHERE id = '{dup}'"),
    )
    .await;
    exec(
        db,
        &format!("UPDATE leads SET status = 'disqualified' WHERE id = '{l2}'"),
    )
    .await;
    exec(
        db,
        &format!(
        "UPDATE agent_pending_actions SET status = 'rejected' WHERE id IN ('{card}', '{old_card}')"
    ),
    )
    .await;
    exec(
        db,
        &format!("UPDATE approval_requests SET status = 'approved' WHERE id = '{request}'"),
    )
    .await;
    exec(
        db,
        &format!("UPDATE deals SET stage = 'won' WHERE id = '{deal}'"),
    )
    .await;
    let items = queue(db).await;
    for id in [
        format!("whatsapp_reply:{known}"),
        format!("unknown_chat:{stranger}"),
        format!("email_reply:{thread}"),
        format!("new_lead:{lead}"),
        format!("held_chat:{held}"),
        format!("followup_due:{reminder}"),
        format!("quotation_expired:{q_old}"),
        format!("meeting_reminder:{meeting}"),
        format!("meeting_brief:{meeting}"),
        format!("meeting_file:{file}"),
        format!("duplicate_deal:{deal}:{dup}"),
        format!("duplicate_lead:{l1}:{l2}"),
        format!("card:{card}"),
        format!("card:{old_card}"),
        format!("approval_overdue:{request}"),
        format!("ready_to_win:{deal}"),
    ] {
        assert!(has(&items, &id).is_none(), "{id} still listed");
    }
    // the stranger's reply card is now the item
    assert!(items
        .iter()
        .any(|i| i.kind == "card_pending" && i.subject["action"] == json!("whatsapp.reply.send")));
}

// ─── The nudge ─────────────────────────────────────────────────────────────────

/// Company time (Asia/Tokyo, UTC+9) on a date, as UTC.
fn company_at(date: (i32, u32, u32), h: u32, m: u32) -> DateTime<Utc> {
    chrono_tz::Asia::Tokyo
        .from_local_datetime(
            &NaiveDate::from_ymd_opt(date.0, date.1, date.2)
                .unwrap()
                .and_hms_opt(h, m, 0)
                .unwrap(),
        )
        .unwrap()
        .with_timezone(&Utc)
}

async fn bells(db: &mut PgConnection) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT title, body FROM notifications WHERE type = 'nary_work_queue' ORDER BY created_at",
    )
    .fetch_all(&mut *db)
    .await
    .unwrap()
}

async fn tick(
    db: &mut PgConnection,
    at: DateTime<Utc>,
) -> (Value, Vec<granary_services::approvals::notify::OutboxItem>) {
    granary_services::approvals::notify::scope(async {
        let out = work_queue::nudge_tick_at(db, at).await.unwrap();
        (out, granary_services::approvals::notify::take_outbox())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn the_nudge_at_its_times_urgent_with_the_gap_and_never_off_days() {
    let _g = SERIAL.lock().await;
    let pool = setup("work_queue_nudge").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    set(db, "nary.owner.enabled", "false").await;
    // the company's public holidays (the baseline ships none)
    set(db, "nary.meetings.holidays", r#"["2026-12-25"]"#).await;
    // Wednesday 4 Nov 2026 (a working day); the seed's hours are 08:00-17:00, Monday to Friday
    let wed = (2026, 11, 4);
    let bob = contact(db, "Bob").await;
    chat(db, Some(bob), company_at(wed, 6, 0)).await;

    // 07:30: no nudge time yet, and the urgent item came outside working hours
    let (out, _) = tick(db, company_at(wed, 7, 30)).await;
    assert_eq!(out["sent"], json!(false), "{out}");
    assert!(bells(db).await.is_empty());
    // 08:00: the morning nudge, bell + WhatsApp to the Owner
    let (out, outbox) = tick(db, company_at(wed, 8, 0)).await;
    assert_eq!(out["sent"], json!(true), "{out}");
    let b = bells(db).await;
    assert_eq!(b.len(), 1);
    assert!(
        b[0].0.starts_with("1 item(s) waiting, 1 urgent"),
        "{:?}",
        b[0]
    );
    assert!(
        b[0].1
            .contains("1. [urgent] Bob is waiting for an answer on WhatsApp"),
        "{}",
        b[0].1
    );
    assert!(
        outbox.iter().any(|o| o.whatsapp && !o.email),
        "WhatsApp, no email"
    );
    assert!(!b[0].1.contains('\u{2014}'));
    // 08:05: nothing new
    let (out, _) = tick(db, company_at(wed, 8, 5)).await;
    assert_eq!(out["sent"], json!(false));
    // a new customer writes at 08:10: urgent, but within 30 minutes of the last nudge it waits
    let sarah = contact(db, "Sarah").await;
    chat(db, Some(sarah), company_at(wed, 8, 10)).await;
    let (out, _) = tick(db, company_at(wed, 8, 15)).await;
    assert_eq!(out["waiting"], json!("the urgent nudge gap"), "{out}");
    assert_eq!(bells(db).await.len(), 1);
    // 08:31: the urgent nudge
    let (out, _) = tick(db, company_at(wed, 8, 31)).await;
    assert_eq!(out["urgent"], json!(true), "{out}");
    let b = bells(db).await;
    assert_eq!(b.len(), 2);
    assert!(
        b[1].0.starts_with("Urgent: 2 item(s) waiting, 2 urgent"),
        "{:?}",
        b[1]
    );
    // told once: 08:40 nothing
    let (out, _) = tick(db, company_at(wed, 8, 40)).await;
    assert_eq!(out["sent"], json!(false));
    // another at 08:45, told at 09:01 (30 minutes after 08:31), not at 08:50
    let andy = contact(db, "Andy").await;
    chat(db, Some(andy), company_at(wed, 8, 45)).await;
    let (out, _) = tick(db, company_at(wed, 8, 50)).await;
    assert_eq!(out["sent"], json!(false));
    let (out, _) = tick(db, company_at(wed, 9, 1)).await;
    assert_eq!(out["sent"], json!(true), "{out}");
    assert_eq!(bells(db).await.len(), 3);
    // 13:00: the midday nudge
    let (out, _) = tick(db, company_at(wed, 13, 0)).await;
    assert_eq!(out["slot"], json!("13:00"), "{out}");
    assert_eq!(bells(db).await.len(), 4);
    // Saturday and a holiday (Friday 25 Dec 2026): nothing at all
    for day in [(2026, 11, 7), (2026, 12, 25)] {
        let (out, _) = tick(db, company_at(day, 8, 0)).await;
        assert_eq!(out["skipped"], json!("not a working day"), "{day:?}: {out}");
    }
    assert_eq!(bells(db).await.len(), 4);
    // off: nothing
    set(db, "nary.queue.nudge_enabled", "false").await;
    let (out, _) = tick(db, company_at(wed, 17, 0)).await;
    assert_eq!(out["skipped"], json!("off"));
    set(db, "nary.queue.nudge_enabled", "true").await;
    // with the Owner's chat with Nary on, the 17:00 nudge is a topic there (sent by the minute flush, paused or not)
    set(db, "nary.owner.enabled", "true").await;
    let (out, _) = tick(db, company_at(wed, 17, 0)).await;
    assert_eq!(out["sent"], json!(true), "{out}");
    let topics: Vec<(String, String)> =
        sqlx::query_as("SELECT kind, tag FROM nary_owner_topics WHERE ref_type = 'work_queue'")
            .fetch_all(&mut *db)
            .await
            .unwrap();
    assert_eq!(topics.len(), 1);
    assert_eq!(topics[0].0, "alert");
    assert!(
        topics[0].1.starts_with("3 item(s) waiting"),
        "{:?}",
        topics[0]
    );
    assert_eq!(bells(db).await.len(), 4);
    // Nary stayed paused the whole time
    assert!(granary_services::nary::governance::is_paused(db)
        .await
        .unwrap());
}
