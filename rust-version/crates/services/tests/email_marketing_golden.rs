//! Golden fixtures recorded from backend/app/services/email_marketing.py
//! (compat-tests/golden/gen_email_marketing.py). The rendered outputs re-record with `GOLDEN_BLESS=1`
//! (tests/golden/bless.rs): the Rust renderer is now the reference.

#[path = "golden/bless.rs"]
mod bless;

use granary_services::email_marketing::{
    _footer_html, _render_block, _safe_color, _safe_url, apply_tokens, render_campaign_html_with,
    Ctx,
};
use serde_json::Value;

fn load(name: &str) -> Vec<Value> {
    serde_json::from_value(bless::read(&rel(name))).expect("json")
}

fn rel(name: &str) -> String {
    format!("email_marketing/{name}.json")
}

/// Checks (or, when blessing, records) `out` of every case, then writes the file back when blessing.
fn golden_outs(name: &str, mut got: impl FnMut(&Value) -> String) {
    let mut cases = load(name);
    for c in cases.iter_mut() {
        let out = got(c);
        let msg = c.to_string();
        bless::eq(&mut c["out"], out, msg);
    }
    bless::save(&rel(name), &Value::Array(cases));
}

fn ctx(v: &Value) -> Ctx {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p[0].as_str().unwrap().to_string(),
                p[1].as_str().map(str::to_string),
            )
        })
        .collect()
}

#[test]
fn golden_apply_tokens() {
    for c in load("apply_tokens") {
        assert_eq!(
            apply_tokens(c["text"].as_str().unwrap(), &ctx(&c["ctx"])),
            c["out"].as_str().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn golden_safe_url() {
    for c in load("safe_url") {
        assert_eq!(
            _safe_url(c["in"].as_str()),
            c["out"].as_str().unwrap(),
            "{c}"
        );
    }
}

/// `_safe_color(color, _RED)`: CSS colours pass, anything else falls back.
#[test]
fn golden_safe_color() {
    let cases = load("safe_color");
    assert!(cases.len() > 40);
    for c in cases {
        assert_eq!(
            _safe_color(Some(&c["in"]), "#1f4e79"),
            c["out"].as_str().unwrap(),
            "{c}"
        );
    }
    assert_eq!(_safe_color(None, "#1f4e79"), "#1f4e79");
}

#[test]
fn golden_render_block() {
    assert!(load("render_block").len() > 100);
    golden_outs("render_block", |c| {
        _render_block(&c["block"], &ctx(&c["ctx"])).unwrap()
    });
}

#[test]
fn golden_render_block_errors() {
    let ctx0: Ctx = vec![
        ("first_name".into(), Some("Ann".into())),
        ("email".into(), Some("a@x".into())),
    ];
    for c in load("render_block_errors") {
        let got = _render_block(&c["block"], &ctx0);
        match c.get("error") {
            Some(e) => assert_eq!(got.unwrap_err().0, e.as_str().unwrap(), "{c}"),
            None => assert_eq!(got.unwrap(), c["ok"].as_str().unwrap(), "{c}"),
        }
    }
}

#[test]
fn golden_footer() {
    golden_outs("footer", |c| {
        _footer_html(
            &ctx(&c["ctx"]),
            c["sig"].as_str().unwrap(),
            c["preview"].as_bool().unwrap(),
        )
    });
}

#[test]
fn golden_render_campaign_html() {
    golden_outs("render_campaign_html", |c| {
        render_campaign_html_with(
            c["api_url"].as_str().unwrap(),
            c["blocks"].as_array().unwrap(),
            &ctx(&c["ctx"]),
            c["tracking_id"].as_str().unwrap(),
            c["sig"].as_str().unwrap(),
            c["preview"].as_bool().unwrap(),
        )
        .unwrap()
    });
}
