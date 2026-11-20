//! Golden fixtures first recorded from the retired Python helpers of nary/{staff_chat,commands,persona,
//! skills,context}: WhatsApp formatting, bubbles, card descriptions and endpoints, number parsing, prompt
//! texts, the customer context wall. Re-record with GOLDEN_BLESS=1 after an intended change and review the
//! diff.
#[path = "golden/bless.rs"]
mod bless;

use std::path::PathBuf;

use serde_json::{json, Value};

use granary_services::nary::{commands, context, persona, skills, staff_chat};

fn load(name: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../compat-tests/golden/nary_staff_chat")
        .join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?}: {e}")))
        .unwrap()
}

fn rel(name: &str) -> String {
    format!("nary_staff_chat/{name}.json")
}

fn cases(name: &str) -> Vec<Value> {
    load(name).as_array().unwrap().clone()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn opt(v: &Value) -> Option<&str> {
    v.as_str()
}

#[test]
fn to_whatsapp_matches_python() {
    for c in cases("to_whatsapp") {
        assert_eq!(
            staff_chat::to_whatsapp(s(&c["in"])),
            s(&c["out"]),
            "in: {:?}",
            c["in"]
        );
    }
}

#[test]
fn bubbles_match_python() {
    for c in cases("bubbles") {
        let out = staff_chat::bubbles(
            s(&c["text"]),
            c["chars"].as_i64().unwrap(),
            c["max_n"].as_i64().unwrap(),
            s(&c["link"]),
        );
        assert_eq!(json!(out), c["out"], "case {c}");
    }
}

#[test]
fn describe_done_text_and_listing_match_python() {
    for c in cases("describe") {
        assert_eq!(
            json!(staff_chat::describe_opt(&c["p"])),
            c["out"],
            "p: {}",
            c["p"]
        );
    }
    for c in cases("done_text") {
        assert_eq!(
            staff_chat::done_text(&c["p"], s(&c["label"])),
            s(&c["out"]),
            "p: {}",
            c["p"]
        );
    }
    for c in cases("proposals_text") {
        let props = c["props"].as_array().unwrap();
        assert_eq!(staff_chat::proposals_text(props), s(&c["out"]));
    }
}

#[test]
fn request_for_matches_python() {
    let mut all = cases("request_for");
    for c in all.iter_mut() {
        match staff_chat::_request_for(&c["p"]) {
            Ok((m, p, b)) => {
                let msg = format!("p: {}", c["p"]);
                // compared as JSON text: the body's key order is the dict's
                bless::eq(&mut c["out"], json!([m, p, b]), msg);
            }
            Err(e) => {
                assert_eq!(c["err"], "ValueError", "{c}");
                assert_eq!(e.message, s(&c["msg"]));
            }
        }
    }
    bless::save(&rel("request_for"), &Value::Array(all));
}

#[test]
fn numbers_and_parts_match_python() {
    for c in cases("is_answer") {
        assert_eq!(
            staff_chat::is_answer(opt(&c["in"])),
            c["out"].as_bool().unwrap(),
            "{c}"
        );
    }
    for c in cases("split_parts") {
        assert_eq!(json!(staff_chat::split_parts(s(&c["in"]))), c["out"], "{c}");
    }
    for c in cases("commands_parse") {
        let prompt = if c["prompt"].is_null() {
            None
        } else {
            Some(&c["prompt"])
        };
        let got = commands::parse(opt(&c["text"]), prompt)
            .map(|cmd| json!({"kind": cmd.kind, "numbers": cmd.numbers}));
        assert_eq!(json!(got), c["out"], "{c}");
    }
}

#[test]
fn constants_and_prompt_texts_match_python() {
    let mut k = load("constants");
    let sorted = |a: &[&str]| {
        let mut v: Vec<&str> = a.to_vec();
        v.sort();
        json!(v)
    };
    {
        let sc = &mut k["staff_chat"];
        bless::eq(&mut sc["WHERE"], staff_chat::WHERE, "WHERE");
        bless::eq(&mut sc["TITLE"], staff_chat::TITLE, "TITLE");
        bless::eq(
            &mut sc["NEW_WORDS"],
            sorted(&staff_chat::NEW_WORDS),
            "NEW_WORDS",
        );
        bless::eq(
            &mut sc["NEXT_WORDS"],
            sorted(&staff_chat::NEXT_WORDS),
            "NEXT_WORDS",
        );
        bless::eq(
            &mut sc["DONE_WORDS"],
            sorted(&staff_chat::DONE_WORDS),
            "DONE_WORDS",
        );
        bless::eq(
            &mut sc["SKIP_WORDS"],
            sorted(&staff_chat::SKIP_WORDS),
            "SKIP_WORDS",
        );
        let lang = "en";
        let p = staff_chat::queue_phrases(lang).unwrap();
        bless::eq(
            &mut sc["QUEUE_PHRASES"][lang],
            json!({"more": p.more, "before": p.before, "none": p.none, "done": p.done}),
            lang,
        );
        for (n, key) in [(6, "pacing_6"), (3, "pacing_3")] {
            bless::eq(&mut sc[key], staff_chat::pacing_rule_text(n), key);
        }
    }
    assert!(staff_chat::queue_phrases("fr").is_none());
    assert!(staff_chat::queue_phrases("id").is_none());
    {
        let c = &mut k["commands"];
        bless::eq(&mut c["PROMPT_KEY"], commands::PROMPT_KEY, "PROMPT_KEY");
        bless::eq(&mut c["CARD_FOOTER"], commands::CARD_FOOTER, "CARD_FOOTER");
        bless::eq(&mut c["HELD_FOOTER"], commands::HELD_FOOTER, "HELD_FOOTER");
    }
    {
        let p = &mut k["persona"];
        bless::eq(&mut p["NAME"], persona::NAME, "NAME");
        bless::eq(&mut p["IDENTITY"], persona::IDENTITY, "IDENTITY");
        bless::eq(&mut p["FIXED_RULES"], persona::FIXED_RULES, "FIXED_RULES");
        bless::eq(
            &mut p["CUSTOMER_REGISTER"],
            persona::CUSTOMER_REGISTER,
            "CUSTOMER_REGISTER",
        );
        bless::eq(
            &mut p["STAFF_REGISTER"],
            persona::STAFF_REGISTER,
            "STAFF_REGISTER",
        );
        bless::eq(
            &mut p["STYLE_GUIDE_TITLE"],
            persona::STYLE_GUIDE_TITLE,
            "STYLE_GUIDE_TITLE",
        );
    }
    {
        let sk = &mut k["skills"];
        bless::eq(&mut sk["WHERE"], skills::WHERE, "skills WHERE");
        bless::eq(&mut sk["SKILL"], skills::SKILL, "SKILL");
        bless::eq(&mut sk["IDEA_LABEL"], skills::IDEA_LABEL, "IDEA_LABEL");
    }
    {
        let cx = &mut k["context"];
        bless::eq(&mut cx["INTERNAL"], context::INTERNAL, "INTERNAL");
        bless::eq(
            &mut cx["CUSTOMER_TOOL_NAMES"],
            json!(context::CUSTOMER_TOOL_NAMES),
            "CUSTOMER_TOOL_NAMES",
        );
        bless::eq(
            &mut cx["FORBIDDEN_KEYS"],
            sorted(&context::FORBIDDEN_KEYS),
            "FORBIDDEN_KEYS",
        );
    }
    bless::save(&rel("constants"), &k);

    let mut all = cases("system_prompt");
    for c in all.iter_mut() {
        let got = persona::system_prompt(
            s(&c["register"]),
            s(&c["extra"]),
            s(&c["style"]),
            s(&c["context"]),
        );
        let msg = c.to_string();
        bless::eq(&mut c["out"], got, msg);
    }
    bless::save(&rel("system_prompt"), &Value::Array(all));
    let mut all = cases("style_section");
    for c in all.iter_mut() {
        let got = persona::style_section(opt(&c["text"]).unwrap_or(""), opt(&c["channel"]));
        let msg = c.to_string();
        bless::eq(&mut c["out"], got, msg);
    }
    bless::save(&rel("style_section"), &Value::Array(all));
    let mut all = cases("skills_content");
    for c in all.iter_mut() {
        let got = skills::_content(
            s(&c["when"]),
            s(&c["steps"]),
            opt(&c["watch_out"]),
            opt(&c["learnt_from"]),
        );
        let msg = c.to_string();
        bless::eq(&mut c["out"], got, msg);
    }
    bless::save(&rel("skills_content"), &Value::Array(all));
    for c in cases("skills_when") {
        match skills::_when(s(&c["in"])) {
            Ok(w) => assert_eq!(w, s(&c["ok"]), "{c}"),
            Err(_) => assert_eq!(c["err"], "IndexError", "{c}"),
        }
    }
}

/// pacing_rule needs the setting: the text is checked through a fake value (the Python call recorded with
/// max_lines 6 and 3).
#[test]
fn pacing_rule_text_matches_python() {
    let k = load("constants");
    for (n, key) in [(6, "pacing_6"), (3, "pacing_3")] {
        let want = s(&k["staff_chat"][key]);
        let expect_prefix = format!("in at most {n} short lines");
        assert!(want.contains(&expect_prefix));
        assert_eq!(staff_chat::pacing_rule_text(n), want);
    }
}

#[test]
fn context_wall_matches_python() {
    for c in cases("context_parse") {
        match context::parse(s(&c["in"])) {
            Ok((kind, id)) => {
                let ok = c["ok"]
                    .as_array()
                    .unwrap_or_else(|| panic!("python raised for {c}"));
                assert_eq!(kind, s(&ok[0]));
                assert_eq!(
                    id.map(|u| u.to_string()),
                    ok[1].as_str().map(str::to_string)
                );
            }
            Err(m) => {
                assert_eq!(c["err"], "ValueError", "{c}");
                assert_eq!(m, s(&c["msg"]), "{c}");
            }
        }
    }
    for c in cases("context_scrub") {
        let got = context::scrub(&c["in"]);
        assert_eq!(got, c["out"]);
        assert_eq!(
            serde_json::to_string(&got).unwrap(),
            serde_json::to_string(&c["out"]).unwrap()
        );
    }
    let tools = context::_customer_tools(uuid::Uuid::new_v4());
    let got: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({"name": t.name, "description": t.description, "parameters": t.parameters,
                   "required_any": t.required_any})
        })
        .collect();
    let mut want = load("context_tools");
    bless::eq(&mut want, Value::Array(got), "context_tools");
    bless::save(&rel("context_tools"), &want);
    assert_eq!(context::customer_context("abc"), "customer:abc");
}
