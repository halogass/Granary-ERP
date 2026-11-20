//! Nary fix batch 1, agent N2: what Nary writes and the AI layer.
//!
//! - B3: the writing rules, the style guide and the approved skills in every customer prompt; the post-write
//!   check before a card (one rewrite, else flagged and held); the linter over a sample of sent messages
//!   (tests/nary_fix1_data/sent_messages.json, invented messages in the shape of a chat export); the
//!   acknowledgement's time greeting and template.
//! - B6: one closing (body vs signature, case- and punctuation-insensitive), one "Re: " (subjects), the stored
//!   subjects fixed by migration 0107.
//! - C1: every structured-output schema Nary uses passes the Anthropic rules; a 400 invalid_request_error
//!   is not retried and raises one alert naming the step.
//! - C2: search_records with gte/lte on a timestamp column (dates and timestamps in every form).
//! - C3: the whole inbound pipeline on a new WhatsApp lead, every step on, without a failed event.
//! - C15: the quote-defaults skill is proposed, never approved by itself.

mod nary_n2_common;

use chrono::Utc;
use granary_services::ai::anthropic::AnthropicProvider;
use granary_services::ai::base::{AiError, ChatRequest, ProviderError};
use granary_services::ai::registry::seam::{Answer, Brain};
use granary_services::ai::schema;
use granary_services::nary::writing::{self, LintInput};
use granary_services::nary::{self, channel_actions, executor, media};
use nary_n2_common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const TAG: &str = "n2_fix1_writing";

fn lint(text: &str, channel: &str, language: Option<&str>) -> Vec<&'static str> {
    writing::lint(LintInput {
        text,
        channel,
        language,
        document_confirmed: false,
    })
    .into_iter()
    .map(|f| f.code)
    .collect()
}

// ─── B3: the linter ──────────────────────────────────────────────────────────

#[test]
fn the_linter_catches_each_rule() {
    let good = "Good afternoon Mr Smith, thank you for your message. How many laptops do you need?";
    assert!(
        lint(good, "whatsapp", Some("en")).is_empty(),
        "{:?}",
        lint(good, "whatsapp", Some("en"))
    );
    // no time greeting is required, on WhatsApp or by email
    assert!(lint(
        "Hello Mr Smith, how many laptops do you need?",
        "whatsapp",
        Some("en")
    )
    .is_empty());
    assert!(lint(
        "Hello Mr Smith, how many laptops do you need?",
        "email",
        Some("en")
    )
    .is_empty());
    assert_eq!(
        lint(
            "Hi John, shall we have a quick Zoom call tomorrow?",
            "whatsapp",
            Some("en")
        ),
        vec!["call_tool"]
    );
    assert_eq!(
        lint(
            "Good afternoon Mr Smith, may we give you a call tomorrow morning?",
            "whatsapp",
            Some("en")
        ),
        vec!["call_tool"]
    );
    // asking for a phone number is not offering a call
    assert!(lint(
        "Good afternoon Mr Smith, may I have your office phone number?",
        "whatsapp",
        Some("en")
    )
    .is_empty());
    assert!(lint(
        "Good afternoon Mr Smith, shall we continue on Google Meet tomorrow?",
        "whatsapp",
        Some("en")
    )
    .is_empty());
    assert_eq!(
        lint(
            "Hi John, thanks \u{2014} how many laptops?",
            "whatsapp",
            Some("en")
        ),
        vec!["em_dash"]
    );
    assert_eq!(
        lint(
            "Good afternoon Mr Smith, the quotation will be sent tomorrow morning.",
            "whatsapp",
            Some("en")
        ),
        vec!["document_promise"]
    );
    assert_eq!(
        lint(
            "Hi John, we will send you the quotation tomorrow.",
            "email",
            Some("en")
        ),
        vec!["document_promise"]
    );
    assert!(writing::lint(LintInput {
        text: "Hi John, we will send you the quotation tomorrow.",
        channel: "email",
        language: Some("en"),
        document_confirmed: true,
    })
    .is_empty());
    // messages are written in English; the greeting placeholder of the send-time re-greeting is fine
    assert_eq!(
        writing::guess_language("Thank you Mr Smith, how many laptops do you need?"),
        "en"
    );
    assert!(lint(
        "{greeting} Mr Smith, how many laptops do you need?",
        "whatsapp",
        None
    )
    .is_empty());
}

#[test]
fn the_time_greeting_follows_the_hour() {
    // batch 2 T3: the hours come from nary.wa.greeting_hours (its seeded default here)
    let starts: Vec<(String, u32)> = [("morning", 4), ("afternoon", 12), ("evening", 18)]
        .iter()
        .map(|(w, h)| (w.to_string(), *h))
        .collect();
    assert_eq!(writing::time_greeting(7, "en", &starts), "Good morning");
    assert_eq!(writing::time_greeting(12, "en", &starts), "Good afternoon");
    assert_eq!(writing::time_greeting(15, "en", &starts), "Good afternoon");
    assert_eq!(writing::time_greeting(21, "en", &starts), "Good evening");
    // before the first start it is still the evening before
    assert_eq!(writing::time_greeting(2, "en", &starts), "Good evening");
}

/// The linter over a sample of sent messages (every message text a card showed, in the shape of a chat
/// export). The counts are the report the brief asks for.
#[test]
fn the_linter_report_over_the_sent_messages() {
    let path = format!(
        "{}/tests/nary_fix1_data/sent_messages.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let msgs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut flagged = 0;
    for m in &msgs {
        let text = m["text"].as_str().unwrap();
        let found = lint(text, m["channel"].as_str().unwrap(), None);
        if !found.is_empty() {
            flagged += 1;
        }
        for f in found {
            *counts.entry(f).or_default() += 1;
        }
    }
    println!(
        "writing check over {} sent messages: {flagged} flagged; per rule {counts:?}",
        msgs.len()
    );
    assert_eq!(msgs.len(), 42);
    // the emails promising a quotation or documents nobody confirmed are caught
    assert!(
        counts.get("document_promise").copied().unwrap_or(0) >= 2,
        "{counts:?}"
    );
    // none of them used an em dash in the body (the signature's is added later)
    assert_eq!(counts.get("em_dash"), None);
}

// ─── B6: closing and subjects ────────────────────────────────────────────────

#[test]
fn one_closing_when_the_signature_has_its_own() {
    let sig = "Best Regards,\n\nNary\nAcme Ltd";
    assert!(channel_actions::signature_has_closing(sig));
    for body in [
        "Dear Mr. Park,\n\nThank you.\n\nBest regards,",
        "Dear Mr. Park,\n\nThank you.\n\nBEST REGARDS",
        "Dear Mr. Park,\n\nThank you.\n\nBest regards, Nary",
        "Dear Mr. Park,\n\nThank you.\n\nKind regards!\nNary",
        "Dear Mr. Park,\n\nThank you.\n\nWarm regards ,\n",
    ] {
        assert_eq!(
            channel_actions::strip_signoff(body, true),
            "Dear Mr. Park,\n\nThank you.",
            "{body:?}"
        );
    }
    for body in [
        "Dear Ms Prince,\n\nThank you.\n\nYours sincerely,",
        "Dear Ms Prince,\n\nThank you.\n\nWarm wishes!",
    ] {
        assert_eq!(
            channel_actions::strip_signoff(body, true),
            "Dear Ms Prince,\n\nThank you."
        );
    }
    // without a closing in the signature the body keeps its own
    assert_eq!(
        channel_actions::strip_signoff("Thanks.\n\nBest regards,", false),
        "Thanks.\n\nBest regards,"
    );
    assert!(channel_actions::is_closing_line("  kind   Regards.. "));
    assert!(!channel_actions::is_closing_line(
        "Regards to your team, we will call"
    ));
}

#[test]
fn a_reply_subject_has_exactly_one_re() {
    use granary_services::crm_inbox::{_clean_subject, normalise_subject, reply_subject};
    for (given, want) in [
        (
            "Follow-Up on the Laptop Order",
            "Re: Follow-Up on the Laptop Order",
        ),
        ("Re: Follow-Up", "Re: Follow-Up"),
        ("Re: : Follow-Up", "Re: Follow-Up"),
        (": Follow-Up", "Re: Follow-Up"),
        ("RE: RE: Fwd: Follow-Up", "Re: Follow-Up"),
        ("FW: AW: Fwd: Follow-Up", "Re: Follow-Up"),
        ("Re[2]: Follow-Up", "Re: Follow-Up"),
        ("  re:Re :  : Follow-Up ", "Re: Follow-Up"),
        ("Report for May", "Re: Report for May"),
        ("", "Re: (No Subject)"),
        ("Re:", "Re: (No Subject)"),
    ] {
        assert_eq!(reply_subject(given), want, "{given:?}");
    }
    assert_eq!(
        normalise_subject("Quotation for Acme Ltd"),
        "Quotation for Acme Ltd"
    );
    assert_eq!(normalise_subject("Re: : Quotation"), "Re: Quotation");
    assert_eq!(_clean_subject("Re: : x"), "x");
    assert_eq!(_clean_subject("Fwd: x"), "x");
    assert_eq!(_clean_subject("Awards 2026"), "Awards 2026");
}

// ─── C1: schemas ─────────────────────────────────────────────────────────────

/// Every structured-output schema Nary sends (statics, builders and the inline ones made public for this).
fn every_schema() -> Vec<(&'static str, Value)> {
    use granary_services::nary::*;
    vec![
        ("replies.REPLY", replies::REPLY_SCHEMA.clone()),
        ("replies.FIT", replies::FIT_SCHEMA.clone()),
        ("replies.PRICE", replies::PRICE_SCHEMA.clone()),
        ("reply_alerts", reply_alerts::SCHEMA.clone()),
        ("costing", costing::SCHEMA.clone()),
        ("triage", triage::SCHEMA.clone()),
        // proposal_edits::SCHEMA is the Python-golden constant; what is sent is schema() (below)
        ("handoff.HANDOFF", handoff::HANDOFF_SCHEMA.clone()),
        ("handoff.LOST", handoff::LOST_SCHEMA.clone()),
        ("negotiation", negotiation::SCHEMA.clone()),
        ("meetings.pick", meetings::pick_schema()),
        ("meetings.extract", meetings::extract_schema()),
        ("meetings.summary_email", meetings::summary_email_schema()),
        ("sop_revisions", sop_revisions::schema()),
        ("profiling", profiling::SCHEMA.clone()),
        ("history.CLASSIFY", history::CLASSIFY_SCHEMA.clone()),
        ("history.READ", history::READ_SCHEMA.clone()),
        ("rates", rates::SCHEMA.clone()),
        ("web3forms.EXTRACT", web3forms::EXTRACT_SCHEMA.clone()),
        ("quotes.LINES", quotes::LINES_SCHEMA.clone()),
        (
            "identity_match.REVIEW",
            identity_match::REVIEW_SCHEMA.clone(),
        ),
        ("identity_match.HINTS", identity_match::HINTS_SCHEMA.clone()),
        ("proposals.UPDATE", proposals::UPDATE_SCHEMA.clone()),
        ("proposals.DRAFT", proposals::DRAFT_SCHEMA.clone()),
        ("cadence.draft", cadence::draft_schema()),
        (
            "scoring",
            scoring::schema(&["ACME Laptop Series".to_string(), "SDK".to_string()]),
        ),
        ("writing.fix", writing::fix_schema()),
        // visuals V2: the draft and edit schemas with figures, the figure spec job
        (
            "proposals.DRAFT+figures",
            proposal_visuals::extend_draft_schema(&proposals::DRAFT_SCHEMA),
        ),
        ("proposal_edits+figures", proposal_edits::schema()),
        (
            "proposal_visuals.spec",
            proposal_visuals::spec_answer_schema(),
        ),
        // batch 2 T1: the input schemas of the sales tools added in Rust
        tool_schema("propose_solution_proposal"),
        tool_schema("update_requirements"),
        tool_schema("list_quotations"),
        tool_schema("get_quotation"),
        // visuals V3: the proposal and figure tools
        tool_schema("list_proposals"),
        tool_schema("get_proposal"),
        tool_schema("preview_visual"),
        tool_schema("propose_proposal_visual"),
        tool_schema("propose_proposal_visual_change"),
        tool_schema("propose_proposal_visual_remove"),
        tool_schema("propose_proposal_edit"),
        // fix-longapply: revise instead of a duplicate, reject a draft
        tool_schema("propose_proposal_revision"),
        tool_schema("propose_proposal_reject"),
        tool_schema("propose_proposal_quotation"),
        // 23 Oct: a proposal written from the given sections
        tool_schema("propose_proposal_from_sections"),
        // 23 Oct: catalog changes by the Owner, one product in full
        tool_schema("propose_catalog_change"),
        tool_schema("get_catalog_item"),
        // visuals3 P3B: a picture into a proposal
        tool_schema("propose_proposal_image"),
        // 23 Oct: one activity in full, a document's PDF page as a picture
        tool_schema("get_activity"),
        tool_schema("render_document_page"),
        // 29 Oct 2026: a file Claude or Nary puts on a CRM record
        tool_schema("upload_file"),
        // 1 Nov 2026: direct proposal authoring
        tool_schema("proposal_get"),
        tool_schema("proposal_create"),
        tool_schema("proposal_set_section"),
        tool_schema("proposal_set_block"),
        tool_schema("proposal_delete_block"),
        tool_schema("proposal_set_requirements"),
        tool_schema("figure_place"),
        tool_schema("figure_replace"),
        tool_schema("figure_remove"),
        tool_schema("proposal_add_image"),
        tool_schema("proposal_check"),
    ]
}

/// batch 2: a catalog tool's input schema, by name.
fn tool_schema(name: &'static str) -> (&'static str, Value) {
    let t = granary_services::ai::tools::catalog::tool_by_name(name)
        .unwrap_or_else(|| panic!("{name} missing"));
    (name, t.parameters)
}

/// No schema is written with a list of types next to an enum (the 15 Oct 400), and every one passes the
/// Anthropic rules as it is sent (after `for_anthropic`).
#[test]
fn every_structured_output_schema_passes_the_anthropic_rules() {
    let mut problems = vec![];
    for (name, s) in every_schema() {
        for v in schema::violations(&s) {
            if v.contains("an enum with a list of types") {
                problems.push(format!("{name} (as written): {v}"));
            }
        }
        // tool input schemas are not strict here: a free-form object is fine there (structured outputs only)
        let is_tool = granary_services::ai::tools::catalog::tool_by_name(name).is_some();
        for v in schema::violations(&schema::for_anthropic(&s)) {
            if is_tool && v.contains("free-form object") {
                continue;
            }
            problems.push(format!("{name} (as sent): {v}"));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn a_nullable_enum_is_sent_as_any_of() {
    let old = json!({"type": "object", "properties": {
        "proposal_trigger": {"type": ["string", "null"], "enum": ["custom", "tender", null]}},
        "required": ["proposal_trigger"], "additionalProperties": false});
    assert!(schema::violations(&old)
        .iter()
        .any(|v| v.contains("an enum with a list of types")));
    let sent = schema::for_anthropic(&old);
    assert_eq!(
        sent["properties"]["proposal_trigger"],
        json!({"anyOf": [{"type": "string", "enum": ["custom", "tender"]}, {"type": "null"}]})
    );
    assert!(schema::violations(&sent).is_empty());
    // the request body carries the accepted form
    let p = AnthropicProvider::new(Some("k".into()), None);
    let mut req = ChatRequest::user("s", "x");
    req.response_schema = Some(old);
    let body = p.build_request_body("claude-sonnet-5-5", &req);
    assert_eq!(
        body["output_config"]["format"]["schema"]["properties"]["proposal_trigger"]["anyOf"][1],
        json!({"type": "null"})
    );
    // other rules: an object with properties gets additionalProperties false; constraints become words
    let loose = json!({"type": "object", "properties": {"n": {"type": "integer", "minimum": 1}}, "required": ["n"]});
    let fixed = schema::for_anthropic(&loose);
    assert_eq!(fixed["additionalProperties"], json!(false));
    assert!(fixed["properties"]["n"].get("minimum").is_none());
    assert!(schema::violations(&fixed).is_empty());
}

// ─── B4: messages read as what they were ─────────────────────────────────────

#[test]
fn media_and_failed_messages_are_never_empty_in_a_summary() {
    let base: granary_db::rows::nary::WaMessage = serde_json::from_value(json!({
        "message_key": "k", "serialized_id": null, "chat_id": "1@c.us", "direction": "in", "from_me": false,
        "sent_by_erp": false, "sent_by_nary": false, "type": "chat", "body": null, "quoted_key": null,
        "ack": 1, "wa_ts": null, "has_media": false, "media_key": null, "media_mime": null, "media_name": null,
        "media_size": null, "media_status": "none", "media_description": null, "transcript": null,
        "location": null, "contact_id": null, "lead_id": null, "deal_id": null, "activity_id": null,
        "origin": "live", "processed_at": null, "id": Uuid::new_v4(), "created_at": Utc::now(),
        "updated_at": Utc::now()}))
    .unwrap();
    let mut m = base.clone();
    m.type_ = "image".into();
    m.has_media = true;
    assert_eq!(media::text_or_label(&m), "[image, not readable]");
    let mut m = base.clone();
    m.ack = Some(-1);
    m.from_me = true;
    assert_eq!(media::text_or_label(&m), "[message not delivered]");
    m.body = Some("Good morning Mr".into());
    assert_eq!(media::text_or_label(&m), "[not delivered] Good morning Mr");
    let mut m = base.clone();
    m.type_ = "revoked".into();
    assert_eq!(media::text_or_label(&m), "[message deleted]");
    let mut m = base.clone();
    m.type_ = "sticker".into();
    assert_eq!(media::text_or_label(&m), "[sticker message, no text]");
    assert_eq!(media::text_or_label(&base), "[no text]");
    assert_eq!(
        granary_services::nary::history::company_key("The Northwind Traders Ltd."),
        Some("northwind traders".into())
    );
}

// ─── Database tests ──────────────────────────────────────────────────────────

async fn approve_skill(pool: &PgPool, title: &str, steps: &str) {
    sqlx::query(
        "INSERT INTO nary_memory (id, layer, subject_type, title, content, source_type, authored_by_nary, trusted, \
         status, approved_at) VALUES ($1, 'company', 'skill', $2, $3, 'nary', true, true, 'approved', now())",
    )
    .bind(Uuid::new_v4())
    .bind(title)
    .bind(format!("When to use: a customer writes on WhatsApp\n\nSteps:\n{steps}"))
    .execute(pool)
    .await
    .unwrap();
}

async fn approve_style(pool: &PgPool, text: &str) {
    sqlx::query(
        "INSERT INTO nary_memory (id, layer, title, content, source_type, authored_by_nary, trusted, status, \
         approved_at) VALUES ($1, 'company', 'Style guide', $2, 'nary', true, true, 'approved', now())",
    )
    .bind(Uuid::new_v4())
    .bind(text)
    .execute(pool)
    .await
    .unwrap();
}

async fn clear_learnt(pool: &PgPool) {
    sqlx::query("DELETE FROM nary_memory WHERE subject_type = 'skill' OR title = 'Style guide'")
        .execute(pool)
        .await
        .unwrap();
}

#[test]
fn the_customer_prompt_carries_rules_style_and_skills() {
    run(TAG, |pool| async move {
        clear_learnt(&pool).await;
        approve_skill(
            &pool,
            "Address with Mr/Ms",
            "1. Open with Good afternoon\n2. Never a first name alone",
        )
        .await;
        approve_style(
            &pool,
            "## On WhatsApp\n- Warm, short, emoji at the end\n\n## By email\n- Formal",
        )
        .await;
        let mut db = pool.acquire().await.unwrap();
        let system = writing::customer_prompt(&mut db, Some("whatsapp"), "EXTRA JOB RULES", "ctx")
            .await
            .unwrap();
        assert!(system.contains(writing::CUSTOMER_WRITING_RULES));
        assert!(system.contains("Warm, short, emoji at the end") && !system.contains("- Formal"));
        assert!(
            system.contains("### Address with Mr/Ms")
                && system.contains("2. Never a first name alone")
        );
        assert!(system.contains("EXTRA JOB RULES"));
        // the golden replays compare the rest byte for byte
        let bare = writing::without_writing_rules(&system);
        assert!(!bare.contains("WRITING RULES FOR EVERY CUSTOMER MESSAGE"));
        clear_learnt(&pool).await;
    });
}

#[test]
fn the_inbound_pipeline_runs_every_step_on_a_new_whatsapp_lead() {
    run(TAG, |pool| async move {
        clear_learnt(&pool).await;
        approve_skill(
            &pool,
            "Always ask the laptop count",
            "1. Ask how many laptops",
        )
        .await;
        let brain = Brain::p2();
        let _seam = brain.install().await;
        working_hours(true);
        owner(&pool).await;
        let old = set(
            &pool,
            &[
                ("nary.ack.enabled", "true"),
                ("nary.reply_alerts.enabled", "true"),
                ("nary.writing.lint_enabled", "true"),
            ],
        )
        .await;
        let cid = chat_id();
        ingest(
            &pool,
            &payload(
                &cid,
                "Good morning, I am Robert from Globex Corp. What is the price of laptops for 10 staff?",
                false,
                None,
                None,
                json!({}),
            ),
            "live",
        )
        .await;
        let (done, failed) = dispatch(&pool).await;
        assert_eq!(failed, 0);
        assert!(done >= 1);
        ingest(
            &pool,
            &payload(
                &cid,
                "We are in Springfield, 2 offices",
                false,
                None,
                None,
                json!({}),
            ),
            "live",
        )
        .await;
        let (_, failed) = dispatch(&pool).await;
        assert_eq!(failed, 0);
        let errors: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT type, last_error FROM domain_events WHERE last_error IS NOT NULL OR dispatched_at IS NULL",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let alerts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notifications WHERE type = 'nary_error'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(alerts, 0);
        let chat = get_chat(&pool, &cid).await.unwrap();
        assert!(chat.lead_id.is_some() && chat.acked_at.is_some());
        // the acknowledgement is the English template with the customer's first name
        let ack = sent()[0]["message"].as_str().unwrap().to_string();
        assert!(
            ack.starts_with("Hi ") && ack.contains(", thank you"),
            "{ack}"
        );
        // the reply was drafted with the writing rules and the approved skill, and checked before its card
        let drafts: Vec<String> = brain
            .calls()
            .iter()
            .filter(|(j, _)| j == "nary.reply_draft")
            .map(|(_, c)| c.system().to_string())
            .collect();
        assert!(!drafts.is_empty());
        assert!(drafts
            .iter()
            .all(|s| s.contains(writing::CUSTOMER_WRITING_RULES)
                && s.contains("Ask how many laptops")));
        let replies = cards(&pool, Some("whatsapp.reply.send")).await;
        assert!(!replies.is_empty());
        // Brain::p2's draft breaks no rule, so the card carries no writing-check note
        assert!(replies.iter().all(|c| !c
            .rationale
            .as_deref()
            .unwrap_or("")
            .contains("Writing check")));
        restore(&pool, old).await;
        clear_learnt(&pool).await;
    });
}

async fn propose_reply(pool: &PgPool, text: &str) -> granary_db::rows::nary::AgentPendingAction {
    let mut tx = pool.begin().await.unwrap();
    let actor = nary::roles::nary_actor(&mut tx).await.unwrap();
    let card = executor::propose(
        &mut tx,
        "whatsapp.reply.send",
        json!({"channel": "whatsapp", "chat_id": chat_id(), "text": text, "contact_id": null, "thread_id": null}),
        actor.as_ref(),
        executor::ProposeOpts {
            rationale: Some("Reply: answer"),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    card
}

#[test]
fn a_message_breaking_a_rule_is_rewritten_once_or_held_with_the_problems() {
    run(TAG, |pool| async move {
        owner(&pool).await;
        let brain = Brain::new();
        brain.set(
            "nary.style_fix",
            Answer::json(json!({"text": "Good afternoon Mr Smith, thank you. How many laptops do you need for the 2 offices?"})),
        );
        let _seam = brain.install().await;
        let card = propose_reply(
            &pool,
            "Good afternoon Mr Smith, thank you \u{2014} how many laptops do you need for the 2 offices?",
        )
        .await;
        assert_eq!(
            card.payload["text"],
            json!("Good afternoon Mr Smith, thank you. How many laptops do you need for the 2 offices?")
        );
        let r = card.rationale.clone().unwrap();
        assert!(
            r.starts_with("Reply: answer\nWriting check: rewritten once"),
            "{r}"
        );
        let fix = brain.calls();
        let (job, call) = fix.last().unwrap();
        assert_eq!(job, "nary.style_fix");
        assert!(call.system().contains(writing::CUSTOMER_WRITING_RULES));
        // a rewrite that changes a number is refused: the original stays, the card is held with the problems
        brain.set(
            "nary.style_fix",
            Answer::json(json!({"text": "Good afternoon Mr Smith, how many laptops do you need for the 3 offices?"})),
        );
        let text =
            "Good afternoon Mr Smith \u{2014} how many laptops do you need for the 2 offices?";
        let card = propose_reply(&pool, text).await;
        assert_eq!(card.payload["text"], json!(text));
        let r = card.rationale.clone().unwrap();
        assert!(
            r.contains("please look before sending") && r.contains("em dash"),
            "{r}"
        );
        assert!(
            card.status == "pending" || card.status == "shadow",
            "{}",
            card.status
        );
        // a clean message is left alone and costs no model call
        brain.clear();
        let clean = "Good afternoon Mr Smith, how many laptops do you need?";
        let card = propose_reply(&pool, clean).await;
        assert_eq!(card.payload["text"], json!(clean));
        assert_eq!(card.rationale.as_deref(), Some("Reply: answer"));
        assert!(brain.calls().is_empty());
        // switched off: no check at all
        let old = set(&pool, &[("nary.writing.lint_enabled", "false")]).await;
        let card = propose_reply(&pool, "Hello \u{2014} there").await;
        assert_eq!(card.rationale.as_deref(), Some("Reply: answer"));
        restore(&pool, old).await;
    });
}

#[test]
fn a_refused_request_is_not_retried_and_alerts_once() {
    run(TAG, |pool| async move {
        owner(&pool).await;
        sqlx::query("DELETE FROM notifications WHERE type = 'nary_error'")
            .execute(&pool)
            .await
            .unwrap();
        let brain = Brain::new();
        brain.set(
            "nary.fit_check",
            Answer::fail(|| {
                AiError::Provider(Box::new(ProviderError {
                    status: Some(400),
                    kind: "BadRequestError".into(),
                    message: "Error code: 400 - Enum value 'custom' does not match declared type ['string','null']".into(),
                    body: Some(json!({"type": "error", "error": {"type": "invalid_request_error",
                        "message": "Enum value 'custom' does not match declared type ['string','null']"}})),
                    display: None,
                }))
            }),
        );
        let _seam = brain.install().await;
        let mut db = pool.acquire().await.unwrap();
        for _ in 0..2 {
            let out = nary::llm::ask_json(
                &mut db,
                "nary.fit_check",
                "s",
                "c",
                &granary_services::nary::replies::FIT_SCHEMA,
                None,
                None,
                None,
            )
            .await
            .unwrap();
            assert!(out.is_none());
        }
        assert_eq!(brain.calls().len(), 2); // once per call, never retried inside
        let alerts: Vec<(String, String)> = sqlx::query_as(
            "SELECT entity_id, title FROM notifications WHERE type = 'nary_error' AND entity_id LIKE 'ai_refused:%'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        // one per approver, named after the step; the second failure is deduplicated
        assert!(!alerts.is_empty());
        assert!(alerts
            .iter()
            .all(|(k, t)| k == "ai_refused:nary.fit_check" && t.contains("nary.fit_check")));
        let distinct: std::collections::HashSet<_> = alerts.iter().collect();
        assert_eq!(distinct.len(), 1);
        // any other failure still propagates
        brain.set(
            "nary.fit_check",
            Answer::fail(|| AiError::Other("boom".into())),
        );
        assert!(nary::llm::ask_json(
            &mut db,
            "nary.fit_check",
            "s",
            "c",
            &json!({}),
            None,
            None,
            None
        )
        .await
        .is_err());
    });
}

#[test]
fn search_records_filters_timestamps_in_every_form() {
    run(TAG, |pool| async move {
        let mut db = pool.acquire().await.unwrap();
        let me = superadmin(&mut db).await;
        let user = auth_user(&mut db, me).await;
        let today = Utc::now().date_naive().to_string();
        for object_type in ["organization", "contact", "lead", "deal"] {
            for value in [
                today.clone(),
                format!("{today}T00:00:00Z"),
                format!("{today}T08:30:00+09:00"),
                format!("{today} 08:30:00"),
                format!("{today}T08:30:00.123456"),
            ] {
                for op in ["gte", "lte", "gt", "lt"] {
                    let filters = vec![json!({"field": "created_at", "op": op, "value": value})];
                    let out = granary_services::crm::search::run_filters(
                        &mut db,
                        &user,
                        object_type,
                        &filters,
                        5,
                        0,
                        None,
                    )
                    .await;
                    assert!(out.is_ok(), "{object_type} {op} {value}: {:?}", out.err());
                    let n = granary_services::crm::search::count_filters(
                        &mut db,
                        &user,
                        object_type,
                        &filters,
                    )
                    .await;
                    assert!(n.is_ok(), "{object_type} {op} {value}");
                }
            }
        }
        // through the tool the assistant calls
        let tool = granary_services::ai::tools::catalog::tool_by_name("search_records").unwrap();
        let out = (tool.handler)(
            &mut db,
            &user,
            json!({"object_type": "lead", "filters": [
                {"field": "created_at", "op": "gte", "value": "2026-10-01"},
                {"field": "created_at", "op": "lte", "value": "2026-10-15T23:59:59Z"}]}),
        )
        .await
        .unwrap();
        assert!(out.get("error").is_none(), "{out}");
    });
}

/// 22 Oct: Claude refused the proposal draft with "The compiled grammar is too large" once its structured
/// output reached 54 typed nodes; the largest proven on prod is 27 (history.READ). Every structured output stays
/// within 30 nodes and 2 unions: bigger data travels as JSON text (see proposal_visuals::json_text_field).
#[test]
fn every_structured_output_stays_within_the_grammar_budget() {
    fn count(v: &Value, n: &mut [usize; 2]) {
        match v {
            Value::Object(m) => {
                if m.contains_key("anyOf") {
                    n[0] += 1;
                }
                if m.get("type").is_some() {
                    n[1] += 1;
                }
                for x in m.values() {
                    count(x, n);
                }
            }
            Value::Array(a) => {
                for x in a {
                    count(x, n)
                }
            }
            _ => {}
        }
    }
    let mut over = vec![];
    for (name, s) in every_schema() {
        // tool input schemas are not structured outputs (not strict here)
        if granary_services::ai::tools::catalog::tool_by_name(name).is_some() {
            continue;
        }
        let mut n = [0usize; 2];
        count(&schema::for_anthropic(&s), &mut n);
        if n[1] > 30 || n[0] > 2 {
            over.push(format!("{name}: {} nodes, {} unions", n[1], n[0]));
        }
    }
    assert!(over.is_empty(), "{over:#?}");
}
