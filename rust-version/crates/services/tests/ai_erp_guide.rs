//! services/ai/erp_guide.rs: the guide's sections and its database sync against the golden
//! compat-tests/golden/ai_a4/erp_guide.json (first recorded from the retired Python module; re-record with
//! GOLDEN_BLESS=1 after an intended change to the guide, then review the diff), plus checks of the guide files.
#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::cell::RefCell;
use std::rc::Rc;

use granary_services::ai::erp_guide::{self, Section};
use serde_json::Value;
use sqlx::PgConnection;

const GOLDEN: &str = "ai_a4/erp_guide.json";

fn golden() -> Value {
    bless::read(GOLDEN)
}

/// Exact text of a JSON value (key order included).
fn txt(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

fn sec_json(s: &Section) -> Value {
    serde_json::json!({"key": s.key, "title": s.title, "summary": s.summary, "content": s.content,
                       "digest": s.digest()})
}

/// Compares `got` with the golden's `key`, or records it there when blessing.
fn check(blessed: &mut Vec<(String, Value)>, g: &Value, key: &str, got: Value) {
    if bless::blessing() {
        blessed.push((key.to_string(), got));
    } else {
        assert_eq!(txt(&got), txt(&g[key]), "{key}");
    }
}

fn write_blessed(blessed: Vec<(String, Value)>) {
    if bless::blessing() {
        bless::update(GOLDEN, |v| {
            for (k, x) in blessed {
                v[k] = x;
            }
        });
    }
}

#[test]
fn static_sections_match_the_golden() {
    let g = golden();
    let mut b = vec![];
    let narrative: Vec<Value> = erp_guide::narrative().iter().map(sec_json).collect();
    check(&mut b, &g, "narrative", Value::Array(narrative));
    check(&mut b, &g, "roles", sec_json(&erp_guide::roles_section()));
    check(&mut b, &g, "fields", sec_json(&erp_guide::fields_section()));
    check(&mut b, &g, "api", sec_json(&erp_guide::api_section()));
    check(
        &mut b,
        &g,
        "prompt_map",
        Value::String(erp_guide::prompt_map()),
    );
    write_blessed(b);
}

/// The guide's files: one Markdown file per narrative section plus the API map, each a real text, none with
/// an em dash in the Markdown (the AI reads and repeats them; the API map is generated from the routes).
#[test]
fn the_guide_files_are_complete() {
    let here = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ai/erp_guide");
    let mut names: Vec<String> = std::fs::read_dir(here)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let mut want: Vec<String> = erp_guide::narrative()
        .into_iter()
        .map(|s| format!("{}.md", s.key))
        .collect();
    want.push("api_map.txt".into());
    want.sort();
    assert_eq!(names, want);
    assert_eq!(names.len(), 15);
    for n in names {
        let text = std::fs::read_to_string(format!("{here}/{n}")).unwrap();
        assert!(text.chars().count() > 300, "{n} is too short");
        assert!(
            !n.ends_with(".md") || !text.contains('\u{2014}'),
            "{n}: no em dashes"
        );
    }
}

/// test_every_permission_module_has_a_narrative_section
#[test]
fn test_every_permission_module_has_a_narrative_section() {
    let seed: Value =
        serde_json::from_str(include_str!("../../db/src/seed/seed_data.json")).unwrap();
    let have: Vec<String> = erp_guide::narrative().into_iter().map(|s| s.key).collect();
    let mut missing: Vec<String> = seed["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            p[0].as_str()
                .unwrap()
                .split(':')
                .next()
                .unwrap()
                .to_string()
        })
        .filter(|m| !erp_guide::module_section(m).is_some_and(|s| have.iter().any(|h| h == s)))
        .collect();
    missing.sort();
    missing.dedup();
    assert!(missing.is_empty(), "no ERP guide section for: {missing:?}");
    for s in erp_guide::narrative() {
        assert!(
            !s.title.is_empty()
                && s.summary.chars().count() > 20
                && s.content.chars().count() > 300,
            "{}",
            s.key
        );
        assert!(
            !s.content.contains('\u{2014}'),
            "{}: no em dashes in what the AI reads and repeats",
            s.key
        );
    }
}

async fn wipe(db: &mut PgConnection) {
    sqlx::query("DELETE FROM ai_knowledge_chunks WHERE doc_id IN (SELECT id FROM ai_knowledge_docs WHERE system_key IS NOT NULL)")
        .execute(&mut *db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ai_knowledge_docs WHERE system_key IS NOT NULL")
        .execute(&mut *db)
        .await
        .unwrap();
}

fn hook() -> Rc<RefCell<Vec<String>>> {
    let calls: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let c = calls.clone();
    erp_guide::set_process_hook(Some(Box::new(move |id| {
        c.borrow_mut().push(id.to_string())
    })));
    calls
}

async fn system_key(db: &mut PgConnection, id: &str) -> String {
    sqlx::query_scalar::<_, String>("SELECT system_key FROM ai_knowledge_docs WHERE id = $1::uuid")
        .bind(id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

/// The database goldens, then test_sync_stores_each_section_once_and_reindexes_only_what_changed and
/// test_the_tool_reads_a_section_by_name_and_lists_them, on one database (they run in order).
/// The database sections and lookups against the golden, then
/// test_sync_stores_each_section_once_and_reindexes_only_what_changed and
/// test_the_tool_reads_a_section_by_name_and_lists_them, on one database (they run in order).
#[tokio::test(flavor = "current_thread")]
async fn database_goldens_and_sync() {
    let pool = support::fresh_db("a4_erp_guide").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;
    let g = golden();
    let mut b = vec![];
    wipe(db).await;

    let vocab = sec_json(&erp_guide::crm_vocab_section(db).await.unwrap());
    check(&mut b, &g, "crm_vocab", vocab);
    let settings = sec_json(&erp_guide::settings_section(db).await.unwrap());
    if !bless::blessing() {
        let (got_c, want_c) = (
            settings["content"].as_str().unwrap().to_string(),
            g["settings"]["content"].as_str().unwrap_or("").to_string(),
        );
        for (a, w) in got_c.lines().zip(want_c.lines()) {
            assert_eq!(a, w, "first differing settings line");
        }
    }
    check(&mut b, &g, "settings", settings);
    let mut unsynced = vec![];
    for c in g["section_unsynced"].as_array().unwrap() {
        let topic = c["topic"].as_str().unwrap();
        let got = erp_guide::section(db, topic).await.unwrap();
        unsynced.push(serde_json::json!({"topic": topic, "out": got}));
    }
    check(&mut b, &g, "section_unsynced", Value::Array(unsynced));

    let calls = hook();
    let first = erp_guide::sync(db).await.unwrap();
    check(&mut b, &g, "sync_first", first.clone());
    let n_sections = first["sections"].as_u64().unwrap() as usize;
    assert!(
        first["updated"] == first["sections"] && n_sections >= erp_guide::narrative().len() + 4
    );
    let docs: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT system_key, title, customer_safe FROM ai_knowledge_docs WHERE system_key LIKE 'erp_guide:%'",
    )
    .fetch_all(&mut *db)
    .await
    .unwrap();
    for k in [
        "erp_guide:crm-picklists",
        "erp_guide:permissions",
        "erp_guide:sales",
    ] {
        assert!(docs.iter().any(|d| d.0 == k), "{k}");
    }
    assert!(docs.iter().all(|d| d.1.starts_with("ERP guide: ") && !d.2));
    let n = calls.borrow().len();
    let again = erp_guide::sync(db).await.unwrap();
    check(&mut b, &g, "sync_again", again.clone());
    assert!(again["updated"] == 0 && calls.borrow().len() == n); // nothing changed, nothing reindexed

    let mut synced = vec![];
    for c in g["section_synced"].as_array().unwrap() {
        let topic = c["topic"].as_str().unwrap();
        let got = erp_guide::section(db, topic).await.unwrap();
        synced.push(serde_json::json!({"topic": topic, "out": got}));
    }
    check(&mut b, &g, "section_synced", Value::Array(synced));

    // test_the_tool_reads_a_section_by_name_and_lists_them (the section() half; the tool wrapper is
    // catalog's _h_erp_guide)
    let listing = erp_guide::section(db, "").await.unwrap();
    assert!(listing["topics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["topic"] == "sales"));
    let picklists = erp_guide::section(db, "crm-picklists").await.unwrap();
    let text = picklists["text"].as_str().unwrap();
    assert!(text.contains("`email`") && text.contains("Lead sources"));
    let fuzzy = erp_guide::section(db, "how do invoices and payments work")
        .await
        .unwrap();
    assert_eq!(fuzzy["topic"], "finance");
    let api = erp_guide::section(db, "api:nary").await.unwrap();
    let api_text = api["text"].as_str().unwrap();
    assert!(api_text.to_lowercase().starts_with("## nary") && api_text.contains("/nary/send"));
    let nothing = erp_guide::section(db, "zzzz qqqq").await.unwrap();
    assert!(nothing.get("error").is_some() && !nothing["topics"].as_array().unwrap().is_empty());

    // settings_ctx(crm__funnel__lead_priorities=...): one section changes, one reindex.
    let old: String = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE key = 'crm.funnel.lead_priorities'",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap();
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'crm.funnel.lead_priorities'")
        .bind(r#"[{"key": "low", "label": "Low"}, {"key": "vip", "label": "VIP"}]"#)
        .execute(&mut *db)
        .await
        .unwrap();
    let changed = erp_guide::sync(db).await.unwrap();
    sqlx::query("UPDATE system_settings SET value = $1 WHERE key = 'crm.funnel.lead_priorities'")
        .bind(&old)
        .execute(&mut *db)
        .await
        .unwrap();
    assert_eq!(changed["updated"], 1);
    let last = calls.borrow().last().cloned().unwrap();
    assert_eq!(system_key(db, &last).await, "erp_guide:crm-picklists");
    erp_guide::set_process_hook(None);
    wipe(db).await;
    write_blessed(b);
}

#[test]
#[ignore = "covered by crates/api/tests/ai_knowledge_routes.rs::test_guide_documents_are_read_only_in_settings (wave 4)"]
fn test_guide_documents_are_read_only_in_settings() {
    // Python (backend/tests/test_ai_erp_guide.py):
    //   await erp_guide.sync(db); doc = the "erp_guide:sales" document
    //   listed = GET /api/v1/ai/knowledge -> items; the item with doc.id has "system" is True
    //   PATCH /api/v1/ai/knowledge/{doc.id} {"content": "x"} -> 400
    //   DELETE /api/v1/ai/knowledge/{doc.id} -> 400
    //   PATCH /api/v1/ai/knowledge/{doc.id} {"is_active": False} -> 200 and json()["is_active"] is False
    todo!("wave 4")
}

/// test_the_map_is_in_the_cached_prompt: stable_prompt appends prompt_map() verbatim (assistant.rs), so the
/// map itself is checked here.
#[test]
fn test_the_map_is_in_the_cached_prompt() {
    let text = erp_guide::prompt_map();
    assert!(
        text.contains("HOW THE ERP WORKS")
            && text.contains("erp_guide(topic)")
            && text.contains("- sales:")
    );
}
