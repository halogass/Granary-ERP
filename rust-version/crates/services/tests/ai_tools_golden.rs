//! Golden tests for ai/tools/catalog.rs against fixtures first recorded from the retired Python catalog
//! (re-record with GOLDEN_BLESS=1 after an intended change and review the diff):
//! - toolspecs.json: the tool list the model sees (name, description, JSON schema), per permission set,
//!   compared byte for byte (compact JSON, key order kept).
//! - calls.json: every call run through `run_tool` on the scenario of ai_tools/scenario.sql, output compared
//!   byte for byte after the same normalisation (scenario timestamps -> tokens, unknown UUIDs -> "<uuid>").
//!   A handler still waiting for another wave-3 module (its body is `todo!("waiting for ...")`) is counted
//!   and skipped, never silently passed.

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;

use chrono::{Duration, Timelike, Utc};
use futures::FutureExt;
use granary_core::auth::AuthUser;
use granary_services::ai::tools::catalog::{self, run_tool, tools_for_user};
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../compat-tests/golden/ai_tools"
);

fn read(name: &str) -> String {
    std::fs::read_to_string(format!("{GOLDEN}/{name}")).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn user(id: Uuid, superadmin: bool, perms: &[String]) -> AuthUser {
    let now = Utc::now();
    AuthUser {
        id,
        email: "golden@example.com".into(),
        name: "Golden".into(),
        phone: None,
        avatar_url: None,
        department: None,
        position: None,
        is_active: true,
        is_superadmin: superadmin,
        is_system_admin: false,
        portal_type: "internal".into(),
        partner_account_id: None,
        vendor_id: None,
        last_login_at: None,
        must_change_password: None,
        created_at: now,
        updated_at: now,
        perms: perms.iter().cloned().collect::<HashSet<_>>(),
    }
}

#[test]
fn toolspecs_per_permission_set() {
    let mut cases = bless::read("ai_tools/toolspecs.json");
    assert!(cases.as_array().unwrap().len() > 20);
    for case in cases.as_array_mut().unwrap() {
        let perms: Vec<String> = case["perms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_string())
            .collect();
        let u = user(Uuid::nil(), case["superadmin"].as_bool().unwrap(), &perms);
        let tools = tools_for_user(&u);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        let label = case["name"].to_string();
        bless::eq(
            &mut case["names"],
            json!(names),
            format!("names for {label}"),
        );
        let specs: Vec<Value> = tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "parameters": t.parameters}))
            .collect();
        bless::eq(
            &mut case["specs"],
            serde_json::to_string(&specs).unwrap(),
            format!("specs for {label}"),
        );
    }
    bless::save("ai_tools/toolspecs.json", &cases);
}

/// Every tool of the catalog is in the superadmin's list, in catalog order.
#[test]
fn all_tools_is_the_superadmin_list() {
    let cases = bless::read("ai_tools/toolspecs.json");
    let all = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "superadmin")
        .unwrap();
    let ported: Vec<&str> = catalog::ALL_TOOLS.iter().map(|t| t.name.as_str()).collect();
    let want: Vec<&str> = all["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert_eq!(ported, want);
}

fn uuid_re() -> regex::Regex {
    regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap()
}

/// token -> timestamp (Python's `isoformat()` of `base - n`), as gen_ai_tools_calls.tokens.
fn tokens(sql: &str, base: chrono::DateTime<Utc>) -> Vec<(String, String)> {
    let re = regex::Regex::new(r"\{\{NOW-(\d+)([dh])\}\}").unwrap();
    let mut out: Vec<(String, String)> = Vec::new();
    for m in re.captures_iter(sql) {
        let n: i64 = m[1].parse().unwrap();
        let t = if &m[2] == "d" {
            base - Duration::days(n)
        } else {
            base - Duration::hours(n)
        };
        let iso = granary_compat::pyjson::PyDateTime::from_chrono_utc(&t).isoformat();
        if !out.iter().any(|(k, _)| k == &m[0]) {
            out.push((m[0].to_string(), iso));
        }
    }
    out
}

fn normalise(s: &str, toks: &[(String, String)], known: &HashSet<String>) -> String {
    let mut s = s.to_string();
    for (tok, iso) in toks {
        s = s.replace(iso.as_str(), tok);
    }
    uuid_re()
        .replace_all(&s, |c: &regex::Captures| {
            if known.contains(&c[0]) {
                c[0].to_string()
            } else {
                "<uuid>".to_string()
            }
        })
        .to_string()
}

/// The day calls.json was recorded ("today" in its outputs reads as this date).
const RECORDED_ON: &str = "2026-10-15";

#[tokio::test]
async fn calls_match_python() {
    let mut fixture = bless::read("ai_tools/calls.json");
    let profiles: HashMap<String, (Uuid, Vec<String>)> = fixture["profiles"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            let id = Uuid::parse_str(v[0].as_str().unwrap()).unwrap();
            let perms = v[1]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().to_string())
                .collect();
            (k.clone(), (id, perms))
        })
        .collect();

    let pool = support::fresh_db("ai_tools_gold").await;
    let base = Utc::now()
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();
    let raw_sql = read("scenario.sql");
    let toks = tokens(&raw_sql, base);
    let mut sql = raw_sql.clone();
    for (tok, iso) in &toks {
        sql = sql.replace(tok.as_str(), iso);
    }
    let known: HashSet<String> = uuid_re()
        .find_iter(&sql)
        .map(|m| m.as_str().to_string())
        .collect();
    sqlx::raw_sql(&sql).execute(&pool).await.expect("scenario");
    let url = std::env::var("DATABASE_URL").unwrap();

    // The outputs carry the day the fixtures were recorded ("today" in the company time zone, in the
    // pricing notes).
    let today = {
        let mut c = pool.acquire().await.unwrap();
        granary_core::clock::today_local(&mut c)
            .await
            .unwrap()
            .to_string()
    };
    let (mut ok, mut pending, mut failed) = (0, Vec::new(), Vec::new());
    granary_services::nary::set_session_pool(pool.clone());
    for (i, call) in fixture["calls"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        if let Some(sql) = call.get("sql").and_then(|s| s.as_str()) {
            sqlx::query(sql).execute(&pool).await.expect("setup sql");
            continue;
        }
        let (uid, perms) = &profiles[call["profile"].as_str().unwrap()];
        let mut u: AuthUser = sqlx::query_as(
            "SELECT id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, \
             is_system_admin, portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, \
             created_at, updated_at FROM users WHERE id = $1",
        )
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
        u.perms = perms.iter().cloned().collect();
        let mut conn = PgConnection::connect(&url).await.unwrap();
        let mut tx = conn.begin().await.unwrap();
        let tool = call["tool"].as_str().unwrap().to_string();
        let tool = tool.as_str();
        let fut = run_tool(&mut tx, &u, tool, call["args"].clone());
        let res = AssertUnwindSafe(fut).catch_unwind().await;
        let _ = tx.rollback().await;
        match res {
            Ok(out) => {
                let got = normalise(&serde_json::to_string(&out).unwrap(), &toks, &known);
                let rows_ok = match call.get("rows").and_then(|r| r.as_str()).map(String::from) {
                    None => true,
                    Some(q) => {
                        let rows: String = sqlx::query_scalar(&format!(
                            "SELECT coalesce(json_agg(t)::text, '[]') FROM ({q}) t"
                        ))
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                        let rows = normalise(&rows, &toks, &known);
                        if bless::blessing() {
                            call["rows_out"] = json!(rows);
                        }
                        if rows != call["rows_out"].as_str().unwrap() {
                            failed.push(format!(
                                "#{i} {tool} rows\n  want {}\n  got  {rows}",
                                call["rows_out"].as_str().unwrap()
                            ));
                        }
                        rows == call["rows_out"].as_str().unwrap()
                    }
                };
                if bless::blessing() {
                    call["out"] = json!(got.replace(&today, RECORDED_ON));
                }
                let want = call["out"].as_str().unwrap().replace(RECORDED_ON, &today);
                if got == want {
                    if rows_ok {
                        ok += 1;
                    }
                } else {
                    failed.push(format!(
                        "#{i} {tool} {} {}\n  want {}\n  got  {}",
                        call["profile"], call["args"], want, got
                    ));
                }
            }
            Err(p) => {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                if msg.contains("waiting for") || msg.contains("wave 4") {
                    pending.push(format!("#{i} {tool}: {msg}"));
                } else {
                    failed.push(format!("#{i} {tool} panicked: {msg}"));
                }
            }
        }
    }
    bless::save("ai_tools/calls.json", &fixture);
    eprintln!(
        "ai_tools calls: {ok} identical, {} pending on other modules",
        pending.len()
    );
    for p in &pending {
        eprintln!("  pending {p}");
    }
    assert!(
        failed.is_empty(),
        "{} differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

/// Issue #3: every propose tool that makes an approval card offers an optional `reason`; tools that already had
/// their own `reason` keep it (and keep it required where it was).
#[test]
fn propose_tools_take_an_optional_reason() {
    let no_card = ["propose_skill", "propose_tool_idea"];
    let mut checked = 0;
    for tool in catalog::ALL_TOOLS.iter() {
        if !tool.name.starts_with("propose_") || no_card.contains(&tool.name.as_str()) {
            continue;
        }
        let reason = &tool.parameters["properties"]["reason"];
        assert_eq!(reason["type"], "string", "{} has no reason argument", tool.name);
        checked += 1;
    }
    assert!(checked >= 40, "only {checked} propose tools found");
    for name in ["propose_quotation_cancel", "propose_activity_delete", "propose_proposal_reject"] {
        let tool = catalog::ALL_TOOLS.iter().find(|t| t.name == name).unwrap();
        let required = tool.parameters["required"].as_array().cloned().unwrap_or_default();
        assert!(required.contains(&json!("reason")), "{name} must still require its reason");
        assert_ne!(
            tool.parameters["properties"]["reason"]["description"],
            json!("Optional: why you propose this, in one or two sentences. Shown on the approval card."),
            "{name} keeps its own reason description"
        );
    }
    for name in no_card {
        let tool = catalog::ALL_TOOLS.iter().find(|t| t.name == name).unwrap();
        assert!(tool.parameters["properties"].get("reason").is_none(), "{name} makes no card");
    }
}
