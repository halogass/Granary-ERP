//! Ports of backend/tests/test_assistant_loop.py, test_assistant_persona.py (service parts),
//! test_ai_catalog_digest.py, test_ai_knowledge.py (service parts) and test_suggestions_history.py,
//! against a fresh database. The model is a scripted provider behind the registry seam.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use granary_core::auth::AuthUser;
use granary_db::rows::ai::AIConversation;
use granary_services::ai::assistant::{self, AssistantOptions};
use granary_services::ai::base::{
    AiError, ChatRequest, ChatResult, LLMProvider, ProviderError, ToolCall,
};
use granary_services::ai::registry::seam::{fake_use_ai, Seam};
use granary_services::ai::registry::AiConfig;
use granary_services::ai::tools::catalog::{handler, set_tool_for_tests, tool_by_name, Tool};
use granary_services::ai::{catalog_digest, knowledge, mcp_access, suggestions};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use tokio::sync::OnceCell;
use uuid::Uuid;

static DB_URL: OnceCell<String> = OnceCell::const_new();

/// The file's database is created once; each test (its own tokio runtime) gets its own pool on it: a pool made
/// in an earlier test's runtime holds connections whose I/O driver is gone (PoolTimedOut).
async fn pool() -> PgPool {
    let url = DB_URL
        .get_or_init(|| async {
            let pool = support::fresh_db("a2_assistant").await;
            pool.close().await;
            std::env::var("DATABASE_URL").expect("fresh_db sets DATABASE_URL")
        })
        .await;
    PgPool::connect(url).await.expect("connect test db")
}

async fn superadmin(db: &mut PgConnection) -> AuthUser {
    let id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    granary_core::auth::load_active_user(db, id)
        .await
        .unwrap()
        .unwrap()
}

async fn conversation(
    db: &mut PgConnection,
    user: &AuthUser,
    title: Option<&str>,
) -> AIConversation {
    sqlx::query_as(
        "INSERT INTO ai_conversations (id, user_id, title, created_at, updated_at) VALUES ($1, $2, $3, now(), now()) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(title)
    .fetch_one(db)
    .await
    .unwrap()
}

fn cfg(extra: &[(&str, &str)]) -> AiConfig {
    let mut c: AiConfig = HashMap::new();
    c.insert("ai.api_key".into(), "k".into());
    c.insert(
        "ai.routing".into(),
        r#"{"default": {"provider": "ollama", "model": "m"}, "embedding": {"provider": "ollama", "model": "e"}}"#.into(),
    );
    for (k, v) in extra {
        c.insert(k.to_string(), v.to_string());
    }
    c
}

/// Embeds everything to the same vector (no SOPs or knowledge in a fresh database anyway).
struct Embedder;

#[async_trait]
impl LLMProvider for Embedder {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _m: &str, _r: &ChatRequest) -> Result<ChatResult, AiError> {
        Err(AiError::Other("not a chat provider".into()))
    }
    async fn embed(&self, _m: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, AiError> {
        Ok(texts.iter().map(|_| vec![0.1; 768]).collect())
    }
}

/// `_setup(monkeypatch, provider, tool_result)`: the stub model, the config, and every tool the provider
/// may call answering from `tool_result`.
async fn setup(
    provider: Arc<dyn LLMProvider>,
    extra_cfg: &[(&str, &str)],
) -> (Seam, granary_services::ai::registry::seam::StubAi) {
    let stub = fake_use_ai(provider, "stub-model");
    let seam = stub.install().await;
    let c = cfg(extra_cfg);
    seam.get_ai_config(move || Ok(c.clone()));
    seam.get_provider(|_, _| Ok(Arc::new(Embedder) as Arc<dyn LLMProvider>));
    (seam, stub)
}

type ToolFn = Arc<dyn Fn(&str, &Value) -> Value + Send + Sync>;

/// `monkeypatch.setattr(assistant_module, "run_tool", run_tool)` for the named tools; the originals are
/// put back by the guard.
struct ToolPatch(Vec<(String, Option<Tool>)>);

impl ToolPatch {
    fn new(names: &[&str], f: ToolFn) -> ToolPatch {
        let mut saved = Vec::new();
        for name in names {
            saved.push((name.to_string(), tool_by_name(name)));
            let f = f.clone();
            let n = name.to_string();
            let base = tool_by_name(name);
            let h = handler(move |_db, _user, args| {
                let out = f(&n, &args);
                Box::pin(async move { Ok(out) })
            });
            let tool = Tool::new(
                name,
                base.as_ref().map(|t| t.description.as_str()).unwrap_or("x"),
                base.as_ref()
                    .map(|t| t.parameters.clone())
                    .unwrap_or(json!({"type": "object", "properties": {}})),
                h,
                &[],
            );
            set_tool_for_tests(name, Some(tool));
        }
        ToolPatch(saved)
    }
}

impl Drop for ToolPatch {
    fn drop(&mut self) {
        for (name, tool) in self.0.drain(..) {
            set_tool_for_tests(&name, tool);
        }
    }
}

async fn run(
    db: &mut PgConnection,
    user: &AuthUser,
    conv: &mut AIConversation,
    message: &str,
    opts: AssistantOptions,
) -> Vec<Value> {
    let mut s = assistant::run_assistant(db, user, conv, message, opts);
    let mut out = Vec::new();
    while let Some(ev) = s.next().await {
        out.push(ev.expect("no exception"));
    }
    out
}

fn text(events: &[Value]) -> String {
    events
        .iter()
        .filter(|e| e["type"] == "text")
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect()
}

async fn stored(db: &mut PgConnection, conv: Uuid) -> Vec<(String, Option<String>, Option<Value>)> {
    sqlx::query_as(&format!(
        "SELECT role, content, tool_calls FROM ai_messages WHERE conversation_id = $1 ORDER BY created_at, {}",
        assistant::QUESTION_FIRST
    ))
    .bind(conv)
    .fetch_all(db)
    .await
    .unwrap()
}

// ─── test_assistant_loop.py ──────────────────────────────────────────────────

/// Asks for a lookup on every call that offers tools; answers only when tools are withheld.
struct AlwaysTools(Mutex<usize>);

#[async_trait]
impl LLMProvider for AlwaysTools {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _m: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        if req.tools.is_some() {
            return Ok(ChatResult {
                tool_calls: vec![ToolCall {
                    id: format!("c{n}"),
                    name: "search_contacts".into(),
                    args: json!({"query": n.to_string()}),
                }],
                ..Default::default()
            });
        }
        Ok(ChatResult::text("Here is what I found."))
    }
}

#[tokio::test]
async fn test_running_out_of_tool_rounds_still_ends_in_an_answer() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let (_seam, stub) = setup(Arc::new(AlwaysTools(Mutex::new(0))), &[]).await;
    let _tools = ToolPatch::new(&["search_contacts"], Arc::new(|_, _| json!({"ok": true})));
    let user = superadmin(&mut db).await;
    let mut conv = conversation(&mut db, &user, None).await;
    let events = run(
        &mut db,
        &user,
        &mut conv,
        "draft an SOP",
        AssistantOptions::default(),
    )
    .await;

    assert_eq!(text(&events).trim(), "Here is what I found.");
    assert!(!text(&events).contains("wasn't able"));
    let calls = stub.calls();
    assert_eq!(calls.len(), assistant::MAX_TOOL_ROUNDS + 1);
    let last = calls.last().unwrap().chat().unwrap();
    assert!(last.tools.is_none());
    assert!(last.system.contains("TOOLS ARE FINISHED"));
    assert_eq!(events.last().unwrap()["type"], "done");
    assert_eq!(conv.title.as_deref(), Some("draft an SOP"));
}

/// Looks up, proposes two different changes in one round, repeats one, then answers.
struct ProposeTwiceThenAnswer(Mutex<usize>);

#[async_trait]
impl LLMProvider for ProposeTwiceThenAnswer {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _m: &str, _req: &ChatRequest) -> Result<ChatResult, AiError> {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        let a = json!({"object_type": "deal", "action": "update", "record_id": "d1", "fields": "{\"stage\": \"won\"}"});
        let b = json!({"object_type": "activity", "action": "create", "fields": "{\"activity_type\": \"call\"}"});
        let tc = |id: &str, name: &str, args: &Value| ToolCall {
            id: id.into(),
            name: name.into(),
            args: args.clone(),
        };
        Ok(match *n {
            1 => ChatResult {
                tool_calls: vec![tc("c1", "list_deals", &json!({}))],
                ..Default::default()
            },
            2 => ChatResult {
                tool_calls: vec![
                    tc("c2", "propose_record_change", &a),
                    tc("c3", "propose_record_change", &b),
                ],
                ..Default::default()
            },
            3 => ChatResult {
                tool_calls: vec![tc("c4", "propose_record_change", &a)],
                ..Default::default()
            },
            _ => ChatResult::text("Two changes are ready for you to confirm."),
        })
    }
}

fn proposal_result(name: &str, args: &Value) -> Value {
    if name == "propose_record_change" {
        return json!({"proposal": {"kind": "record_change", "object_type": args["object_type"],
                                   "action": args["action"], "label": args["object_type"],
                                   "before": {}, "after": {}}});
    }
    json!({"deals": []})
}

#[tokio::test]
async fn test_every_proposal_gets_its_own_card_and_is_saved() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let (_seam, _stub) = setup(Arc::new(ProposeTwiceThenAnswer(Mutex::new(0))), &[]).await;
    let _tools = ToolPatch::new(
        &["list_deals", "propose_record_change"],
        Arc::new(proposal_result),
    );
    let user = superadmin(&mut db).await;
    let mut conv = conversation(&mut db, &user, None).await;
    let events = run(
        &mut db,
        &user,
        &mut conv,
        "log the call and mark the deal won",
        AssistantOptions::default(),
    )
    .await;

    let cards: Vec<&Value> = events.iter().filter(|e| e["type"] == "proposal").collect();
    assert_eq!(
        cards
            .iter()
            .map(|c| c["object_type"].clone())
            .collect::<Vec<_>>(),
        vec![json!("deal"), json!("activity")]
    );
    assert!(cards.iter().all(|c| c["proposal_id"]
        .as_str()
        .map(|s| !s.is_empty())
        .unwrap_or(false)
        && c["status"] == "pending"));
    assert_ne!(cards[0]["proposal_id"], cards[1]["proposal_id"]);
    assert_eq!(
        text(&events).trim(),
        "Two changes are ready for you to confirm."
    );

    let saved: Vec<_> = stored(&mut db, conv.id)
        .await
        .into_iter()
        .filter(|r| r.0 == "assistant")
        .collect();
    assert_eq!(saved.len(), 1);
    let ids: Vec<Value> = saved[0].2.as_ref().unwrap()["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["proposal_id"].clone())
        .collect();
    assert_eq!(
        ids,
        cards
            .iter()
            .map(|c| c["proposal_id"].clone())
            .collect::<Vec<_>>()
    );
}

struct Fails;

#[async_trait]
impl LLMProvider for Fails {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _m: &str, _r: &ChatRequest) -> Result<ChatResult, AiError> {
        Err(AiError::Provider(Box::new(ProviderError {
            status: Some(401),
            kind: "Exception".into(),
            message: "Error code: 401 - {'error': {'message': 'Incorrect API key'}}".into(),
            body: None,
            display: None,
        })))
    }
}

#[tokio::test]
async fn test_a_provider_failure_is_readable_and_keeps_the_question() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let (_seam, _stub) = setup(Arc::new(Fails), &[]).await;
    let user = superadmin(&mut db).await;
    let mut conv = conversation(&mut db, &user, None).await;
    let events = run(
        &mut db,
        &user,
        &mut conv,
        "how is the pipeline?",
        AssistantOptions::default(),
    )
    .await;
    let errors: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "error")
        .map(|e| &e["text"])
        .collect();
    assert_eq!(
        errors,
        vec!["The AI provider rejected the API key. Check it in Settings → Nary & AI → Providers & keys."]
    );
    assert_eq!(
        *events.last().unwrap(),
        json!({"type": "done", "conversation_id": conv.id.to_string()})
    );
    let roles: Vec<String> = stored(&mut db, conv.id)
        .await
        .into_iter()
        .map(|r| r.0)
        .collect();
    assert_eq!(roles, vec!["user"]);
}

#[tokio::test]
async fn test_history_skips_placeholders_and_notes_cards_and_their_outcome() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let user = superadmin(&mut db).await;
    let conv = conversation(&mut db, &user, Some("x")).await;
    let rows = [
        ("user", "list deals", Value::Null),
        (
            "assistant",
            "Here are 3 deals.",
            json!({
            "cards": [{"object_type": "deal", "filters": [{"field": "stage", "op": "eq", "value": "won"}],
                       "offset": 0, "limit": 25, "count": 73}],
            "proposals": [{"kind": "record_change", "action": "update", "object_type": "deal",
                           "label": "X Ltd", "status": "applied"},
                          {"kind": "sop", "title": "Warranty claims", "status": "pending"}]}),
        ),
        ("user", "try again", Value::Null),
        (
            "assistant",
            "I wasn't able to complete that request.",
            Value::Null,
        ),
    ];
    // Saved in one transaction before the fix: same created_at; the question still comes first.
    for (i, (role, content, tc)) in rows.iter().enumerate() {
        sqlx::query("INSERT INTO ai_messages (id, conversation_id, role, content, tool_calls, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6), now())")
            .bind(Uuid::new_v4()).bind(conv.id).bind(role).bind(content)
            .bind(if tc.is_null() { None } else { Some(tc.clone()) })
            .bind((i / 2) as f64)
            .execute(&mut *db).await.unwrap();
    }
    let history = assistant::load_history(&mut db, conv.id).await.unwrap();
    assert_eq!(
        history
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["user", "assistant", "user"]
    );
    let note = history[1]["content"].as_str().unwrap();
    assert!(
        note.contains("[Table shown: search_records object_type=deal") && note.contains("count=73")
    );
    assert!(note.contains("update deal (X Ltd), confirmed and saved by the user"));
    assert!(note.contains("sop (Warranty claims), not confirmed yet"));
}

/// Calls every confirm-card tool once, then answers.
struct CallsEachCardTool(Vec<String>, Mutex<usize>);

#[async_trait]
impl LLMProvider for CallsEachCardTool {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _m: &str, _r: &ChatRequest) -> Result<ChatResult, AiError> {
        let mut n = self.1.lock().unwrap();
        *n += 1;
        if *n == 1 {
            return Ok(ChatResult {
                tool_calls: self
                    .0
                    .iter()
                    .enumerate()
                    .map(|(i, name)| ToolCall {
                        id: format!("c{i}"),
                        name: name.clone(),
                        args: json!({"i": i}),
                    })
                    .collect(),
                ..Default::default()
            });
        }
        Ok(ChatResult::text("Cards are below."))
    }
}

#[tokio::test]
async fn test_every_card_tool_shows_its_card_in_the_chat() {
    let names: Vec<String> = mcp_access::PROPOSAL_TOOLS
        .iter()
        .map(|s| s.to_string())
        .collect();
    for n in [
        "propose_contact_merge",
        "propose_meeting_guests",
        "propose_meeting_cancel",
    ] {
        assert!(names.iter().any(|x| x == n));
    }
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let (_seam, _stub) = setup(
        Arc::new(CallsEachCardTool(names.clone(), Mutex::new(0))),
        &[],
    )
    .await;
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let _tools = ToolPatch::new(
        &refs,
        Arc::new(
            |name, args| json!({"proposal": {"kind": "nary_action", "action": name, "summary": name, "i": args["i"]}}),
        ),
    );
    let user = superadmin(&mut db).await;
    let mut conv = conversation(&mut db, &user, None).await;
    let events = run(
        &mut db,
        &user,
        &mut conv,
        "merge the Diana contacts",
        AssistantOptions::default(),
    )
    .await;
    let mut got: Vec<String> = events
        .iter()
        .filter(|e| e["type"] == "proposal")
        .map(|e| e["action"].as_str().unwrap().to_string())
        .collect();
    let mut want = names.clone();
    got.sort();
    want.sort();
    assert_eq!(got, want);
}

// ─── test_assistant_persona.py ───────────────────────────────────────────────

const PERSONA: &str = "You are Nary, the advanced AI business strategist.";

#[test]
fn test_the_chat_is_nary_and_a_persona_never_replaces_the_rules() {
    use granary_services::nary::persona;
    let prompt = assistant::build_system_prompt_from(Some(PERSONA));
    assert!(prompt.starts_with(persona::IDENTITY) && prompt.contains(persona::FIXED_RULES));
    let i = |s: &str| prompt.find(s).unwrap();
    assert!(i(persona::STAFF_REGISTER) < i(PERSONA) && i(PERSONA) < i("OPERATING RULES"));
    assert!(prompt.contains("takes precedence over the register"));
    assert!(prompt.contains("COMPANY DATA") && prompt.contains("NEVER invent"));
    assert!(
        !prompt.contains("Granary Assistant")
            && !prompt.contains(persona::CUSTOMER_REGISTER.split(':').next().unwrap())
    );
}

#[test]
fn test_without_a_persona_nary_answers_alone() {
    for p in [None, Some("   ")] {
        let prompt = assistant::build_system_prompt_from(p);
        assert!(
            prompt.starts_with(assistant::IDENTITY.as_str())
                && !prompt.contains("HOW YOU THINK")
                && prompt.contains("OPERATING RULES")
        );
    }
}

#[test]
fn test_the_fixed_rules_carry_no_em_dash_for_the_model_to_copy() {
    assert!(!assistant::RULES.contains('—'));
}

struct Says(&'static str);

#[async_trait]
impl LLMProvider for Says {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _m: &str, _r: &ChatRequest) -> Result<ChatResult, AiError> {
        Ok(ChatResult::text(self.0))
    }
}

#[tokio::test]
async fn test_the_assistant_speaks_as_the_persona() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let (_seam, stub) = setup(
        Arc::new(Says("Diagnosis: the pipeline is thin.")),
        &[("ai.assistant.system_prompt", PERSONA)],
    )
    .await;
    let user = superadmin(&mut db).await;
    let mut conv = conversation(&mut db, &user, Some("x")).await;
    let events = run(
        &mut db,
        &user,
        &mut conv,
        "How do we grow in Europe?",
        AssistantOptions::default(),
    )
    .await;
    let system = stub.calls()[0].chat().unwrap().system.clone();
    assert!(
        system.starts_with(granary_services::nary::persona::IDENTITY)
            && system.contains(PERSONA)
            && system.contains("OPERATING RULES")
    );
    assert!(text(&events).starts_with("Diagnosis: the pipeline is thin."));
    assert_eq!(conv.title.as_deref(), Some("x"));
}

// ─── More of the loop: withdraw, supersede, disabled, empty answer ───────────

#[tokio::test]
async fn test_withdraw_and_supersede_cards_in_the_conversation() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let user = superadmin(&mut db).await;
    let conv = conversation(&mut db, &user, Some("x")).await;
    let tc = json!({"proposals": [
        {"kind": "customer_message", "channel": "whatsapp", "chat_id": "81901@c.us", "summary": "Reply to Bob", "status": "pending"},
        {"kind": "customer_message", "channel": "email", "to": "A@x.com", "summary": "Email to A Ltd", "status": "pending"},
        {"kind": "sop", "title": "Warranty", "status": "applied"}]});
    sqlx::query("INSERT INTO ai_messages (id, conversation_id, role, content, tool_calls, created_at, updated_at) \
                 VALUES ($1, $2, 'assistant', 'cards', $3, now(), now())")
        .bind(Uuid::new_v4()).bind(conv.id).bind(&tc).execute(&mut *db).await.unwrap();
    let replaced = assistant::supersede_messages(
        &mut db,
        conv.id,
        &json!({"kind": "customer_message", "channel": "email", "to": "a@X.com"}),
    )
    .await
    .unwrap();
    assert_eq!(replaced, vec!["Email to A Ltd"]);
    let listed = assistant::withdraw_cards(&mut db, conv.id, &json!({}))
        .await
        .unwrap();
    // batch 4 D3: the waiting cards are listed with their id, kind, customer and time
    assert_eq!(listed["pending_cards"].as_array().unwrap().len(), 1);
    assert_eq!(listed["pending_cards"][0]["summary"], json!("Reply to Bob"));
    assert_eq!(
        listed["pending_cards"][0]["kind"],
        json!("customer_message")
    );
    let out = assistant::withdraw_cards(&mut db, conv.id, &json!({"match": "BOB"}))
        .await
        .unwrap();
    assert_eq!(out["withdrawn"], json!(["Reply to Bob"]));
    assert_eq!(out["still_pending"], json!([]));
    let rows = stored(&mut db, conv.id).await;
    let statuses: Vec<Value> = rows[0].2.as_ref().unwrap()["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["status"].clone())
        .collect();
    assert_eq!(
        statuses,
        vec![json!("withdrawn"), json!("superseded"), json!("applied")]
    );
}

#[tokio::test]
async fn test_disabled_assistant_and_empty_answers() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let user = superadmin(&mut db).await;
    {
        let (_seam, _stub) = setup(Arc::new(Says("x")), &[("ai.feature.assistant", "false")]).await;
        let mut conv = conversation(&mut db, &user, None).await;
        let events = run(&mut db, &user, &mut conv, "hi", AssistantOptions::default()).await;
        assert_eq!(
            events,
            vec![
                json!({"type": "error", "text": "The AI assistant is disabled. Enable it in Settings → Nary & AI → AI features."})
            ]
        );
    }
    let (_seam, _stub) = setup(Arc::new(Says("  ")), &[]).await;
    let mut conv = conversation(&mut db, &user, None).await;
    let events = run(&mut db, &user, &mut conv, "  ", AssistantOptions::default()).await;
    assert_eq!(text(&events), assistant::NO_ANSWER);
    // the placeholder is not stored; the question is, and the title falls back
    assert_eq!(
        stored(&mut db, conv.id)
            .await
            .into_iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        vec!["user"]
    );
    assert_eq!(conv.title.as_deref(), Some("New chat"));
}

// ─── test_ai_catalog_digest.py ───────────────────────────────────────────────

async fn product(db: &mut PgConnection, sku: &str, jpy: Option<&str>, fields: Value) -> Uuid {
    let id = Uuid::new_v4();
    let f = |k: &str| fields.get(k).cloned().unwrap_or(Value::Null);
    sqlx::query(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, family, sale_model, price_metric, \
         pricing_mode, is_whitelabel, is_active, is_orderable, is_deleted, included_support_months, \
         rate_period_months, billing_interval_months, min_term_months, created_at, updated_at) \
         VALUES ($1, $2, $3, 'physical', 'Unit', $4, $5, $6, $7, $8, $9, true, false, $10, $11, $12, $13, now(), now())",
    )
    .bind(id)
    .bind(sku)
    .bind(f("name").as_str().unwrap_or("Pytest").to_string())
    .bind(f("family").as_str().unwrap_or("laptop_mini").to_string())
    .bind(f("sale_model").as_str().unwrap_or("one_time").to_string())
    .bind(f("price_metric").as_str().unwrap_or("unit").to_string())
    .bind(f("pricing_mode").as_str().unwrap_or("list").to_string())
    .bind(f("is_whitelabel").as_bool().unwrap_or(false))
    .bind(f("is_active").as_bool().unwrap_or(true))
    .bind(f("included_support_months").as_i64().map(|v| v as i32))
    .bind(f("rate_period_months").as_i64().map(|v| v as i32))
    .bind(f("billing_interval_months").as_i64().map(|v| v as i32))
    .bind(f("min_term_months").as_i64().map(|v| v as i32))
    .execute(&mut *db)
    .await
    .unwrap();
    if let Some(a) = jpy {
        sqlx::query("INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from, created_at, updated_at) \
                     VALUES ($1, $2, 'JPY', $3::numeric, '2000-01-01', now(), now())")
            .bind(Uuid::new_v4()).bind(id).bind(a).execute(&mut *db).await.unwrap();
    }
    id
}

fn line_for<'t>(text: &'t str, sku: &str) -> &'t str {
    text.lines()
        .find(|l| l.starts_with(&format!("{sku} ")))
        .unwrap_or_else(|| panic!("no line for {sku}"))
}

async fn set_setting(db: &mut PgConnection, key: &str, value: &str) {
    sqlx::query("UPDATE system_settings SET value = $2 WHERE key = $1")
        .bind(key)
        .bind(value)
        .execute(db)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_catalog_digest_lists_sells_prices_and_follows_the_cap() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let tag = format!(
        "RSDG{}",
        Uuid::new_v4().simple().to_string()[..5].to_uppercase()
    );
    let mini = product(
        &mut db,
        &format!("{tag}-MINI"),
        Some("70000000"),
        json!({"name": "Edge Gateway", "included_support_months": 12}),
    )
    .await;
    sqlx::query("INSERT INTO catalog_volume_tiers (id, product_id, min_qty, max_qty, discount_pct, created_at, updated_at) \
                 VALUES ($1, $2, 21, NULL, 17.5, now(), now())").bind(Uuid::new_v4()).bind(mini).execute(&mut *db).await.unwrap();
    product(&mut db, &format!("{tag}-SUB"), Some("450000"), json!({"family": "software_licence", "name": "Retail Counter licence",
        "sale_model": "recurring", "price_metric": "seat", "rate_period_months": 1, "billing_interval_months": 12,
        "min_term_months": 12})).await;
    product(
        &mut db,
        &format!("{tag}-WL"),
        None,
        json!({"name": "Laptop Mini white-label", "is_whitelabel": true}),
    )
    .await;
    product(
        &mut db,
        &format!("{tag}-QUOTE"),
        None,
        json!({"family": "custom_service", "pricing_mode": "quote_only"}),
    )
    .await;
    product(
        &mut db,
        &format!("{tag}-OLD"),
        Some("1"),
        json!({"is_active": false}),
    )
    .await;
    set_setting(&mut db, "ai.assistant.catalog_digest_max_items", "100000").await;

    let text = catalog_digest::digest(&mut db, "list").await.unwrap();
    let l = line_for(&text, &format!("{tag}-MINI"));
    assert!(
        l.contains("Edge Gateway")
            && l.contains("12 mo support included")
            && l.contains("JPY 70,000,000")
    );
    assert!(
        !l.contains("17.5")
            && !text
                .to_lowercase()
                .split("price in")
                .next()
                .unwrap()
                .contains("tier")
    );
    assert!(
        line_for(&text, &format!("{tag}-SUB")).contains("per seat")
            && line_for(&text, &format!("{tag}-SUB")).contains("min term 12 mo")
    );
    assert!(line_for(&text, &format!("{tag}-WL")).contains("white-label"));
    assert!(line_for(&text, &format!("{tag}-QUOTE")).contains("quote only"));
    assert!(!text.contains(&format!("{tag}-OLD")));
    let full = catalog_digest::digest(&mut db, "full").await.unwrap();
    assert!(line_for(&full, &format!("{tag}-MINI")).contains("tiers 21+: -17.5%"));

    // a price change shows on the next message
    sqlx::query(
        "UPDATE catalog_prices SET amount = 75000000, updated_at = now() WHERE product_id = $1",
    )
    .bind(mini)
    .execute(&mut *db)
    .await
    .unwrap();
    assert!(line_for(
        &catalog_digest::digest(&mut db, "list").await.unwrap(),
        &format!("{tag}-MINI")
    )
    .contains("JPY 75,000,000"));

    // the chat prompt carries it in the cached part
    let c = cfg(&[]);
    assert!(assistant::stable_prompt(&mut db, &c, true, "list")
        .await
        .contains(&format!("{tag}-MINI")));
    assert!(!assistant::stable_prompt(&mut db, &c, false, "list")
        .await
        .contains(&format!("{tag}-MINI")));

    // over the cap it lists families and points to search
    set_setting(&mut db, "ai.assistant.catalog_digest_max_items", "1").await;
    // Python quirk kept: the cap is not part of the cache key, so (as in the Python test, which adds
    // products) the catalog has to change for a new cap to show.
    product(
        &mut db,
        &format!("{tag}-B"),
        Some("1000"),
        json!({"family": "sdk"}),
    )
    .await;
    let text = catalog_digest::digest(&mut db, "list").await.unwrap();
    assert!(
        text.contains("too many to list")
            && text.contains("sdk")
            && text.contains("search_catalog")
    );
    set_setting(&mut db, "ai.assistant.catalog_digest_max_items", "100000").await;
}

// ─── test_ai_knowledge.py (service level) ────────────────────────────────────

/// The Python test's toy bag-of-words embedding (md5 of each word, 768 buckets, normalised).
struct ToyEmbedder;

fn md5_bucket(word: &str) -> usize {
    // Python uses md5; any stable hash serves the toy embedding.
    use sha2::Digest;
    let d = sha2::Sha256::digest(word.as_bytes());
    (u64::from_be_bytes(d[..8].try_into().unwrap()) % 768) as usize
}

fn toy_vector(text: &str) -> Vec<f64> {
    let mut v = vec![0.0; 768];
    let lower = text.to_lowercase();
    for w in lower
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        .filter(|w| w.len() > 2)
    {
        v[md5_bucket(w)] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm = if norm == 0.0 { 1.0 } else { norm };
    v.iter().map(|x| x / norm).collect()
}

#[async_trait]
impl LLMProvider for ToyEmbedder {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _m: &str, _r: &ChatRequest) -> Result<ChatResult, AiError> {
        Err(AiError::Other("no chat".into()))
    }
    async fn embed(&self, _m: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, AiError> {
        Ok(texts.iter().map(|t| toy_vector(t)).collect())
    }
}

const PRICING: &str = "# Discount rules\nDistributors get 12% off list price on IoT gateways. Discounts above 20% need Owner approval.\nNever quote the Granary Server below JPY 45,000,000.";

#[tokio::test]
async fn test_knowledge_is_indexed_searchable_and_feeds_the_prompt() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let seam = Seam::install().await;
    let c = cfg(&[]);
    seam.get_ai_config(move || Ok(c.clone()));
    seam.get_provider(|_, _| Ok(Arc::new(ToyEmbedder) as Arc<dyn LLMProvider>));

    let mut doc: granary_db::rows::ai::AIKnowledgeDoc = sqlx::query_as(
        "INSERT INTO ai_knowledge_docs (id, title, source_type, content, status, is_active, customer_safe, chunk_count, \
         use_count, created_at, updated_at) VALUES ($1, 'PYTEST pricing', 'markdown', $2, 'processing', true, false, 0, 0, now(), now()) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(PRICING)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    knowledge::process(&mut db, &mut doc, None, None)
        .await
        .unwrap();
    assert_eq!(doc.status, "ready");
    assert!(doc.chunk_count >= 1);

    let hits = knowledge::search(
        &mut db,
        "what discount do distributors get on gateways",
        5,
        None,
        false,
    )
    .await
    .unwrap();
    assert_eq!(hits[0].title, "PYTEST pricing");
    let used: i32 = sqlx::query_scalar("SELECT use_count FROM ai_knowledge_docs WHERE id = $1")
        .bind(doc.id)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert!(used >= 1);

    let block = knowledge::context_block(
        &mut db,
        "Never quote the Granary Server below this price",
        4,
    )
    .await;
    assert!(block.contains("COMPANY KNOWLEDGE FOR AI") && block.contains("45,000,000"));
    assert_eq!(
        knowledge::context_block(&mut db, "annual leave carry over for employees", 4).await,
        ""
    );

    // no embedding provider means no context, not a failed answer
    seam.get_provider(|_, _| Err(AiError::Other("no embedding provider".into())));
    assert_eq!(knowledge::context_block(&mut db, "discount", 4).await, "");

    // an empty document fails with the admin's message
    let mut empty = doc.clone();
    empty.title = String::new();
    empty.content = None;
    empty.description = None;
    seam.get_provider(|_, _| Ok(Arc::new(ToyEmbedder) as Arc<dyn LLMProvider>));
    knowledge::process(&mut db, &mut empty, None, None)
        .await
        .unwrap();
    assert_eq!(
        (empty.status.as_str(), empty.error.as_deref()),
        ("ready", None),
        "'# ' is still text"
    );
    sqlx::query("DELETE FROM ai_knowledge_docs WHERE id = $1")
        .bind(doc.id)
        .execute(&mut *db)
        .await
        .unwrap();
}

// ─── test_suggestions_history.py ─────────────────────────────────────────────

#[test]
fn test_wa_media_placeholders() {
    assert_eq!(
        suggestions::wa_media_note(&json!({"type": "image"})),
        "[sent an image]"
    );
    assert_eq!(
        suggestions::wa_media_note(&json!({"type": "ptt"})),
        "[sent a voice message]"
    );
    assert_eq!(
        suggestions::wa_media_note(
            &json!({"type": "document", "mediaMeta": {"filename": "PO-2291.pdf"}})
        ),
        "[sent a document: PO-2291.pdf]"
    );
    assert_eq!(
        suggestions::wa_media_note(&json!({"type": "weird_new_type", "hasMedia": true})),
        "[sent an attachment]"
    );
    assert_eq!(
        suggestions::wa_media_note(&json!({"type": "chat", "body": "hello"})),
        ""
    );
}

#[tokio::test]
async fn test_the_wa_transcript_keeps_caption_less_media_and_both_sides() {
    use redis::AsyncCommands;
    let _ = pool().await;
    if granary_core::config::settings()
        .WHATSAPP_SERVICE_URL
        .is_empty()
    {
        return; // no WhatsApp service configured: _wa_history returns [] before the cache (as in Python)
    }
    let chat = format!("8190{}@c.us", &Uuid::new_v4().simple().to_string()[..8]);
    let msgs = json!([
        {"type": "chat", "body": "Good morning, could I have a quotation?", "fromMe": false},
        {"type": "chat", "body": "Of course, I will send it", "fromMe": true},
        {"type": "document", "body": "", "hasMedia": true, "mediaMeta": {"filename": "PO-2291.pdf"}, "fromMe": false}]);
    let mut conn = granary_core::redis_client::get_redis()
        .conn()
        .await
        .unwrap();
    let _: () = conn
        .set(format!("wa:messages:{chat}:20:0"), msgs.to_string())
        .await
        .unwrap();
    let lines = suggestions::wa_history(&chat, 20).await.unwrap();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("Customer:") && lines[1].starts_with("Us:"));
    assert_eq!(lines[2], "Customer: [sent a document: PO-2291.pdf]");
}

#[tokio::test]
async fn test_a_long_email_thread_includes_the_newest_message_oldest_first() {
    let pool = pool().await;
    let mut db = pool.acquire().await.unwrap();
    let tid = Uuid::new_v4();
    sqlx::query("INSERT INTO crm_email_threads (id, subject, message_count, last_message_at, created_at, updated_at) \
                 VALUES ($1, 'Quotation', 60, now(), now(), now())").bind(tid).execute(&mut *db).await.unwrap();
    for i in 0..60 {
        sqlx::query("INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, to_emails, \
                     subject, body_text, is_read, received_at, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, ARRAY['sales@example.com'], 'Quotation', $6, false, \
                     now() - make_interval(mins => $7), now(), now())")
            .bind(Uuid::new_v4()).bind(tid).bind(format!("<{}@test>", Uuid::new_v4().simple()))
            .bind(if i % 2 == 0 { "inbound" } else { "outbound" })
            .bind(if i % 2 == 0 { "buyer@example.com" } else { "sales@example.com" })
            .bind(format!("message number {i}")).bind(60 - i)
            .execute(&mut *db).await.unwrap();
    }
    let lines = suggestions::email_history(&mut db, tid, 15).await.unwrap();
    assert!(lines.last().unwrap().contains("message number 59"));
    assert!(!lines.iter().any(|l| l.contains("message number 0")));
    let nums: Vec<i64> = lines
        .iter()
        .map(|l| l.rsplit("message number ").next().unwrap().parse().unwrap())
        .collect();
    let mut sorted = nums.clone();
    sorted.sort();
    assert_eq!(nums, sorted);
    assert!(
        lines.iter().any(|l| l.starts_with("Us:")) && lines.iter().any(|l| !l.starts_with("Us:"))
    );
}
