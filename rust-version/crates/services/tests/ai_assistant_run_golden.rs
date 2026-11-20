//! `run_assistant` against recorded scenarios (first recorded from the retired Python tool loop): the same
//! scenarios, in the same order, on a fresh database; every request sent to the model (system prompt byte for
//! byte, messages, tool names, temperature, tool_choice), the events and the rows written must match.
//! Re-record with GOLDEN_BLESS=1 after an intended change and review the diff.

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use granary_core::auth::AuthUser;
use granary_db::rows::ai::AIConversation;
use granary_services::ai::assistant::{self, AssistantOptions};
use granary_services::ai::base::{
    AiError, ChatRequest, ChatResult, LLMProvider, ProviderError, ToolCall,
};
use granary_services::ai::registry::seam::{fake_use_ai, Seam};
use granary_services::ai::tools::catalog::{handler, set_tool_for_tests, tool_by_name, Tool};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

fn scenarios() -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/ai_assistant/run_scenarios.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json")
}

struct ConstEmbedder;

#[async_trait]
impl LLMProvider for ConstEmbedder {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _m: &str, _r: &ChatRequest) -> Result<ChatResult, AiError> {
        Err(AiError::Other("no chat".into()))
    }
    async fn embed(&self, _m: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, AiError> {
        Ok(texts
            .iter()
            .map(|_| {
                let mut v = vec![0.0; 768];
                v[0] = 1.0;
                v
            })
            .collect())
    }
}

struct Scripted {
    name: String,
    script: Mutex<Vec<Value>>,
    calls: Mutex<Vec<Value>>,
}

#[async_trait]
impl LLMProvider for Scripted {
    fn name(&self) -> &str {
        &self.name
    }
    async fn chat(&self, _m: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
        self.calls.lock().unwrap().push(json!({
            "system": req.system, "messages": req.messages,
            "tools": req.tools.as_ref().map(|t| t.iter().map(|s| s.name.clone()).collect::<Vec<_>>()),
            "temperature": req.temperature, "allow_tool_calls": req.allow_tool_calls}));
        let step = self.script.lock().unwrap().remove(0);
        if let Some(r) = step.get("raise") {
            return Err(AiError::Provider(Box::new(ProviderError {
                status: r["status"].as_u64().map(|s| s as u16),
                kind: "Exception".into(),
                message: r["message"].as_str().unwrap().into(),
                body: None,
                display: None,
            })));
        }
        let s = |k: &str| {
            step.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        Ok(ChatResult {
            text: s("text"),
            stop_reason: s("stop_reason"),
            refusal_category: step
                .get("refusal_category")
                .and_then(Value::as_str)
                .map(str::to_string),
            reasoning: s("reasoning"),
            raw_content: step.get("raw_content").filter(|v| !v.is_null()).cloned(),
            tool_calls: step
                .get("tool_calls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|c| ToolCall {
                    id: c["id"].as_str().unwrap().into(),
                    name: c["name"].as_str().unwrap().into(),
                    args: c["args"].clone(),
                })
                .collect(),
            ..Default::default()
        })
    }
}

fn norm(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| {
                    let pid =
                        k == "proposal_id" && x.as_str().map(|s| s.len() == 32).unwrap_or(false);
                    (k.clone(), if pid { json!("<pid>") } else { norm(x) })
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        o => o.clone(),
    }
}

async fn apply_settings(db: &mut PgConnection, settings: &Value) {
    for (k, v) in settings.as_object().unwrap() {
        sqlx::query("INSERT INTO system_settings (id, key, value, value_type, category, created_at, updated_at) \
                     VALUES (gen_random_uuid(), $1, $2, 'string', 'ai', now(), now()) \
                     ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
            .bind(k).bind(v.as_str().unwrap()).execute(&mut *db).await.unwrap();
    }
}

#[tokio::test]
async fn run_assistant_matches_python() {
    let pool = support::fresh_db("a2_assistant_run").await;
    bless::neutral_owner(&pool).await;
    let mut db = pool.acquire().await.unwrap();
    let uid: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE is_superadmin ORDER BY created_at LIMIT 1")
            .fetch_one(&mut *db)
            .await
            .unwrap();
    let user: AuthUser = granary_core::auth::load_active_user(&mut db, uid)
        .await
        .unwrap()
        .unwrap();
    let now: DateTime<Utc> = "2026-10-13T04:35:00Z".parse().unwrap();
    // propose_tool_idea is offered only once a backlog project is named; the golden tool lists include it.
    apply_settings(
        &mut db,
        &json!({"nary.skills.tool_ideas_project": "Nary tools"}),
    )
    .await;

    let mut all = scenarios();
    for sc in all.iter_mut() {
        let name = sc["name"].as_str().unwrap().to_string();
        apply_settings(&mut db, &sc["settings"]).await;
        for stmt in sc["setup"].as_array().unwrap() {
            sqlx::query(stmt.as_str().unwrap())
                .execute(&mut *db)
                .await
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let provider = Arc::new(Scripted {
            name: sc
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("stub")
                .into(),
            script: Mutex::new(sc["script"].as_array().unwrap().clone()),
            calls: Mutex::new(Vec::new()),
        });
        let seam = match sc.get("use_ai_error").and_then(Value::as_str) {
            Some(msg) => {
                let s = Seam::install().await;
                let msg = msg.to_string();
                s.use_ai(move |_, _| Err(AiError::ai(msg.clone())));
                s
            }
            None => fake_use_ai(provider.clone(), "stub-model").install().await,
        };
        seam.get_provider(|_, _| Ok(Arc::new(ConstEmbedder) as Arc<dyn LLMProvider>));
        assistant::set_now_for_tests(Some(now));

        // `monkeypatch(assistant, "run_tool")`: every tool the script calls answers from the scenario.
        let tools = sc.get("tools").cloned().unwrap_or(json!({}));
        let counters: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
        let mut names: Vec<String> = Vec::new();
        for step in sc["script"].as_array().unwrap() {
            for c in step
                .get("tool_calls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let n = c["name"].as_str().unwrap().to_string();
                if n != "withdraw_cards" && !names.contains(&n) {
                    names.push(n);
                }
            }
        }
        let mut saved = Vec::new();
        for n in &names {
            saved.push((n.clone(), tool_by_name(n)));
            let outs = tools
                .get(n)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| vec![json!({"ok": true})]);
            let (counters, key) = (counters.clone(), n.clone());
            let h = handler(move |_db, _u, _args| {
                let mut c = counters.lock().unwrap();
                let i = c.entry(key.clone()).or_insert(0);
                let out = outs[(*i).min(outs.len() - 1)].clone();
                *i += 1;
                Box::pin(async move { Ok(out) })
            });
            let base = tool_by_name(n);
            set_tool_for_tests(
                n,
                Some(Tool::new(
                    n,
                    base.as_ref().map(|t| t.description.as_str()).unwrap_or("x"),
                    base.as_ref()
                        .map(|t| t.parameters.clone())
                        .unwrap_or(json!({"type": "object", "properties": {}})),
                    h,
                    &[],
                )),
            );
        }
        let extra: Vec<Tool> = sc
            .get("extra_tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|n| {
                let n = n.as_str().unwrap();
                Tool::new(
                    n,
                    &format!("Extra tool {n}."),
                    json!({"type": "object", "properties": {}}),
                    tool_by_name(n).unwrap().handler,
                    &[],
                )
            })
            .collect();

        let mut conv: AIConversation =
            sqlx::query_as("SELECT * FROM ai_conversations WHERE id = $1")
                .bind(Uuid::parse_str(sc["conv"].as_str().unwrap()).unwrap())
                .fetch_one(&mut *db)
                .await
                .unwrap();
        let opts = AssistantOptions {
            context: sc.get("context").and_then(Value::as_object).cloned(),
            job: "assistant".into(),
            extra_tools: if extra.is_empty() { None } else { Some(extra) },
            extra_context: sc
                .get("extra_context")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
        };
        let events: Vec<Value> = {
            let mut s = assistant::run_assistant(
                &mut db,
                &user,
                &mut conv,
                sc["message"].as_str().unwrap(),
                opts,
            );
            let mut out = Vec::new();
            while let Some(ev) = s.next().await {
                out.push(norm(&ev.unwrap_or_else(|e| panic!("{name}: raised {e:?}"))));
            }
            out
        };
        assistant::set_now_for_tests(None);
        for (n, t) in saved {
            set_tool_for_tests(&n, t);
        }
        drop(seam);

        let calls = provider.calls.lock().unwrap().clone();
        if bless::blessing() {
            sc["calls"] = Value::Array(calls.clone());
            sc["events"] = Value::Array(events.clone());
        }
        let want_calls = sc["calls"].as_array().unwrap();
        assert_eq!(
            calls.len(),
            want_calls.len(),
            "{name}: number of model calls"
        );
        for (i, (got, want)) in calls.iter().zip(want_calls).enumerate() {
            assert_eq!(
                got["system"].as_str().unwrap(),
                want["system"].as_str().unwrap(),
                "{name}: call {i} system prompt"
            );
            assert_eq!(
                got["messages"], want["messages"],
                "{name}: call {i} messages"
            );
            assert_eq!(got["tools"], want["tools"], "{name}: call {i} tools");
            assert_eq!(
                got["temperature"], want["temperature"],
                "{name}: call {i} temperature"
            );
            assert_eq!(
                got["allow_tool_calls"], want["allow_tool_calls"],
                "{name}: call {i} tool_choice"
            );
        }
        assert_eq!(Value::Array(events), sc["events"], "{name}: events");

        let rows: Vec<(String, Option<String>, Option<Value>, Option<Value>, String)> = sqlx::query_as(
            "SELECT role, content, tool_calls, citations, to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.US') \
             FROM ai_messages WHERE conversation_id = $1 ORDER BY created_at, CASE WHEN role = 'user' THEN 0 ELSE 1 END, content")
            .bind(conv.id).fetch_all(&mut *db).await.unwrap();
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|(r, c, t, ci, at)| {
                norm(&json!([
                    r,
                    c,
                    t,
                    ci,
                    if at.as_str() >= "2026-10-13" {
                        at
                    } else {
                        "old".into()
                    }
                ]))
            })
            .collect();
        if bless::blessing() {
            sc["rows"] = Value::Array(rows.clone());
        }
        assert_eq!(Value::Array(rows), sc["rows"], "{name}: rows");
        let title: Option<String> =
            sqlx::query_scalar("SELECT title FROM ai_conversations WHERE id = $1")
                .bind(conv.id)
                .fetch_one(&mut *db)
                .await
                .unwrap();
        if bless::blessing() {
            sc["title"] = json!(title);
        }
        assert_eq!(json!(title), sc["title"], "{name}: title");
        assert_eq!(
            conv.title, title,
            "{name}: the conversation object follows its row"
        );
        let uses: i64 =
            sqlx::query_scalar("SELECT coalesce(sum(use_count), 0)::bigint FROM ai_knowledge_docs")
                .fetch_one(&mut *db)
                .await
                .unwrap();
        if bless::blessing() {
            sc["knowledge_uses"] = json!(uses);
        }
        assert_eq!(
            json!(uses),
            sc["knowledge_uses"],
            "{name}: knowledge use counter"
        );
    }
    bless::save("ai_assistant/run_scenarios.json", &Value::Array(all));
}
