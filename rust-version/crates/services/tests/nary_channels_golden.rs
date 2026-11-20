//! nary/{wa_ingest, wa_send, channels} pure functions against compat-tests/golden/nary_channels/*.json, recorded
//! from the REAL backend functions by compat-tests/golden/gen_nary_channels.py (agent N2).

use granary_db::rows::nary::WaChat;
use granary_services::nary::{channels, wa_ingest, wa_send};
use serde_json::{json, Value};

fn load(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../compat-tests/golden/nary_channels/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap()
}

fn chat(over: Value) -> WaChat {
    let mut base = json!({
        "chat_id": "819012@c.us", "phone": null, "display_name": null, "is_my_contact": false, "status": "active",
        "customer_started": null, "contact_id": null, "lead_id": null, "deal_id": null, "staff_user_id": null,
        "human_until": null, "checkin_sent_at": null, "acked_at": null, "ack_pending_since": null,
        "first_inbound_at": null, "last_inbound_at": null, "last_outbound_at": null, "business_profile": null,
        "profile_fetched_at": null, "triage": null, "language": null, "attribution_asked": false,
        "excluded_at": null, "excluded_by_id": null, "exclude_reason": null, "released_at": null,
        "id": "00000000-0000-0000-0000-000000000001", "created_at": "2026-10-15T00:00:00Z",
        "updated_at": "2026-10-15T00:00:00Z"});
    for (k, v) in over.as_object().unwrap() {
        base[k] = v.clone();
    }
    serde_json::from_value(base).unwrap()
}

#[test]
fn golden_is_group() {
    for c in load("is_group") {
        assert_eq!(
            wa_ingest::is_group(&c["payload"]),
            c["is_group"].as_bool().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn golden_ts() {
    for c in load("ts") {
        let got = wa_ingest::_ts(Some(&c["value"]))
            .map(|t| t.format("%Y-%m-%dT%H:%M:%S+00:00").to_string());
        assert_eq!(got.as_deref(), c["ts"].as_str(), "{c}");
    }
}

#[test]
fn golden_should_hold() {
    let id = json!("00000000-0000-0000-0000-0000000000aa");
    for c in load("should_hold") {
        let ch = chat(json!({
            "status": c["status"], "is_my_contact": c["is_my_contact"],
            "contact_id": if c["contact"] == true { id.clone() } else { Value::Null },
            "staff_user_id": if c["staff"] == true { id.clone() } else { Value::Null },
            "released_at": if c["released"] == true { json!("2026-10-15T00:00:00Z") } else { Value::Null }}));
        assert_eq!(
            wa_ingest::should_hold(&ch),
            c["hold"].as_bool().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn golden_strip_signoff() {
    // The golden inputs sign off as the test company; a line with just its name is a sign-off too.
    for c in load("strip_signoff") {
        assert_eq!(
            wa_send::strip_signoff(c["text"].as_str().unwrap(), "Acme Ltd"),
            c["out"].as_str().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn golden_listing_to_payload() {
    for c in load("listing_to_payload") {
        let ch = chat(json!({"phone": c["phone"]}));
        assert_eq!(
            channels::listing_to_payload(&ch, &c["item"]),
            c["payload"],
            "{c}"
        );
    }
}
