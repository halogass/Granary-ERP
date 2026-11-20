//! Golden fixtures recorded from backend/app/services/ai/{assistant,style,suggestions,knowledge,embeddings,
//! catalog_digest}.py; the cases whose texts the product has since changed on purpose are re-recorded with
//! GOLDEN_BLESS=1 (see golden/bless.rs).
#[path = "golden/bless.rs"]
mod bless;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use granary_db::rows::sales::{CatalogVolumeTier, ProductCatalog};
use granary_services::ai::{assistant, catalog_digest, embeddings, knowledge, style, suggestions};
use rust_decimal::Decimal;
use serde_json::{json, Value};

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/ai_assistant/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json")
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

#[test]
fn strip_em_dashes() {
    for c in load("strip_em_dashes") {
        assert_eq!(
            style::strip_em_dashes(s(&c["in"])),
            s(&c["out"]),
            "{:?}",
            c["in"]
        );
    }
}

#[test]
fn clean() {
    for c in load("clean") {
        assert_eq!(style::clean(&c["in"]), c["out"], "{:?}", c["in"]);
    }
}

#[test]
fn style_enabled() {
    for c in load("style_enabled") {
        let cfg: HashMap<String, String> = c["in"]
            .as_object()
            .unwrap()
            .iter()
            // A None value reads as missing (`cfg.get(SETTING) or "true"`).
            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
            .collect();
        assert_eq!(
            style::enabled(&cfg),
            c["out"].as_bool().unwrap(),
            "{:?}",
            c["in"]
        );
    }
}

#[test]
fn constants_are_byte_identical() {
    let mut all = bless::read("ai_assistant/constants.json");
    let c = &mut all[0];
    bless::eq(
        &mut c["identity"],
        assistant::IDENTITY.to_string(),
        "identity",
    );
    bless::eq(&mut c["rules"], assistant::RULES, "rules");
    bless::eq(&mut c["wrap_up"], assistant::WRAP_UP, "wrap_up");
    bless::eq(&mut c["system"], assistant::SYSTEM.to_string(), "system");
    bless::eq(
        &mut c["max_tool_rounds"],
        assistant::MAX_TOOL_ROUNDS as u64,
        "max_tool_rounds",
    );
    bless::eq(&mut c["no_answer"], assistant::NO_ANSWER, "no_answer");
    bless::eq(
        &mut c["fallbacks"],
        json!(assistant::FALLBACKS.to_vec()),
        "fallbacks",
    );
    let keys: Vec<String> = c["status_labels"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    for k in keys {
        bless::eq(&mut c["status_labels"][&k], assistant::status_label(&k), &k);
    }
    assert_eq!(
        assistant::status_label("list_invoices"),
        "Running list_invoices…"
    );
    bless::save("ai_assistant/constants.json", &all);
}

#[test]
fn build_system_prompt() {
    let mut cases = bless::read("ai_assistant/build_system_prompt.json");
    for c in cases.as_array_mut().unwrap() {
        let mut cfg = HashMap::new();
        if let Some(p) = c["persona"].as_str() {
            cfg.insert("ai.assistant.system_prompt".to_string(), p.to_string());
        }
        let msg = format!("{:?}", c["persona"]);
        bless::eq(&mut c["out"], assistant::build_system_prompt(&cfg), msg);
    }
    bless::save("ai_assistant/build_system_prompt.json", &cases);
}

#[test]
fn history_notes() {
    for c in load("history_notes") {
        assert_eq!(
            assistant::turn_notes(&c["tool_calls"]),
            s(&c["turn_notes"]),
            "{c}"
        );
        assert_eq!(
            assistant::parts_note(s(&c["text"]), &c["tool_calls"]),
            s(&c["parts_note"]),
            "{c}"
        );
    }
}

#[test]
fn load_history_assembly() {
    for c in load("load_history") {
        let rows: Vec<assistant::HistoryRow> = c["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| assistant::HistoryRow {
                role: s(&r["role"]).to_string(),
                content: r["content"].as_str().map(str::to_string),
                tool_calls: if r["tool_calls"].is_null() {
                    None
                } else {
                    Some(r["tool_calls"].clone())
                },
            })
            .collect();
        assert_eq!(
            Value::Array(assistant::history_messages(&rows)),
            c["out"],
            "{c}"
        );
    }
}

#[test]
fn chunk_cited_recipient() {
    for c in load("chunk") {
        assert_eq!(json!(assistant::chunk(s(&c["in"]))), c["out"]);
    }
    for c in load("cited_in") {
        let cands = c["candidates"].as_array().unwrap();
        assert_eq!(
            Value::Array(assistant::cited_in(s(&c["answer"]), cands)),
            c["out"],
            "{:?}",
            c["answer"]
        );
    }
    for c in load("same_recipient") {
        assert_eq!(
            assistant::same_recipient(&c["a"], &c["b"]),
            c["out"].as_bool().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn now_line() {
    for c in load("now_line") {
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339(s(&c["now"]))
            .unwrap()
            .with_timezone(&Utc);
        let tz = c["tz"]
            .as_str()
            .map(|t| t.parse::<chrono_tz::Tz>().unwrap());
        assert_eq!(assistant::now_line_at(now, tz), s(&c["out"]), "{c}");
    }
}

#[test]
fn suggestions_helpers() {
    for c in load("wa_media_note") {
        assert_eq!(
            suggestions::wa_media_note(&c["in"]),
            s(&c["out"]),
            "{:?}",
            c["in"]
        );
    }
    for c in load("strip_html") {
        assert_eq!(
            suggestions::strip_html(c["in"].as_str()),
            s(&c["out"]),
            "{:?}",
            c["in"]
        );
    }
}

#[test]
fn embeddings_text() {
    for c in load("strip_md") {
        assert_eq!(
            embeddings::strip_md(c["in"].as_str()),
            s(&c["out"]),
            "{:?}",
            c["in"]
        );
    }
    for c in load("article_text") {
        let a = &c["in"];
        let got = embeddings::article_text_of(
            a["title"].as_str(),
            a["sop_code"].as_str(),
            a["summary"].as_str(),
            a["content"].as_str(),
            if a["sop_steps"].is_null() {
                None
            } else {
                Some(&a["sop_steps"])
            },
        );
        assert_eq!(got, s(&c["out"]), "{a}");
    }
}

#[test]
fn knowledge_block() {
    for c in load("knowledge_block") {
        let hits: Vec<knowledge::KnowledgeHit> = c["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| knowledge::KnowledgeHit {
                doc_id: s(&h["doc_id"]).into(),
                title: s(&h["title"]).into(),
                content: s(&h["content"]).into(),
                distance: h["distance"].as_f64().unwrap(),
            })
            .collect();
        assert_eq!(knowledge::format_block(&hits), s(&c["out"]));
    }
}

fn dec(v: &Value) -> Option<Decimal> {
    v.as_str().map(|x| x.parse().unwrap())
}

fn product(p: &Value) -> (ProductCatalog, Vec<CatalogVolumeTier>) {
    let id = uuid::Uuid::new_v4();
    let mut row = json!({
        "sku": "SKU", "name": "Name", "product_type": "physical", "pricing_mode": "list", "is_whitelabel": false,
        "id": id, "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z", "is_deleted": false,
    });
    for (k, v) in p.as_object().unwrap() {
        if k != "volume_tiers" {
            row[k] = v.clone();
        }
    }
    let prod: ProductCatalog = serde_json::from_value(row).expect("product row");
    let tiers = p["volume_tiers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| CatalogVolumeTier {
            product_id: id,
            min_qty: dec(&t["min_qty"]).unwrap(),
            max_qty: dec(&t["max_qty"]),
            discount_pct: dec(&t["discount_pct"]).unwrap(),
            id: uuid::Uuid::new_v4(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .collect();
    (prod, tiers)
}

#[test]
fn catalog_lines() {
    let mut cases = bless::read("ai_assistant/catalog_lines.json");
    for c in cases.as_array_mut().unwrap() {
        let (p, tiers) = product(&c["product"]);
        let msg = c["product"].to_string();
        bless::eq(&mut c["sold"], catalog_digest::sold(&p), &msg);
        let amounts: Vec<String> = c["full"].as_object().unwrap().keys().cloned().collect();
        for amount in amounts {
            let a = if amount == "None" {
                None
            } else {
                Some(amount.parse::<Decimal>().unwrap())
            };
            bless::eq(
                &mut c["full"][&amount],
                catalog_digest::full(&p, &tiers, "x", a),
                format!("{msg} {amount}"),
            );
        }
    }
    bless::save("ai_assistant/catalog_lines.json", &cases);
    for c in load("catalog_money") {
        let d: Decimal = s(&c["in"])
            .parse()
            .unwrap_or_else(|_| Decimal::from_scientific(s(&c["in"])).unwrap());
        assert_eq!(
            catalog_digest::money(d, "JPY"),
            s(&c["out"]),
            "{:?}",
            c["in"]
        );
    }
}
