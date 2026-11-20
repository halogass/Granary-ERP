//! Batch E: the call size (`calls.max_participants`) and the STUN servers (`calls.stun_urls`) are settings.
//! Calls no longer add Google's STUN servers: by default browsers get the installation's own TURN/STUN only.
//! Its own file (own database): the tests change settings every call route reads.

mod support;

use granary_api::routers::calls::own_stun_urls;
use serde_json::{json, Value};
use support::{app, u};

const TAG: &str = "calls_settings";

fn stun_urls(body: &Value) -> Vec<String> {
    body["iceServers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["urls"].as_str())
        .filter(|u| u.starts_with("stun:") || u.starts_with("stuns:"))
        .map(str::to_string)
        .collect()
}

#[test]
fn own_stun_comes_from_the_plain_turn_uris() {
    let uris = vec![
        "turn:turn.example.com:3478?transport=udp".to_string(),
        "turn:turn.example.com:3478?transport=tcp".to_string(),
        "turns:turn.example.com:5349".to_string(),
    ];
    assert_eq!(own_stun_urls(&uris), vec!["stun:turn.example.com:3478"]);
    assert!(own_stun_urls(&[]).is_empty());
}

#[tokio::test]
async fn call_size_and_stun_servers_follow_the_settings() {
    let app = app(TAG);
    let host = app.user_with(&[]).await;

    // defaults: 10 people, no outside STUN server
    let r = app
        .post(
            &host.token,
            "/api/v1/calls/rooms",
            json!({"name": u("Size")}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let body = r.json();
    assert_eq!(body["max_participants"], 10);
    let stun = stun_urls(&body);
    assert!(
        stun.iter().all(|s| !s.contains("google")),
        "no outside STUN by default: {stun:?}"
    );

    // the setting sets the size; a bigger request is capped, a smaller one kept
    app.set_setting("calls.max_participants", "3").await;
    let r = app
        .post(
            &host.token,
            "/api/v1/calls/rooms",
            json!({"name": u("Size")}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["max_participants"], 3);
    let room_id = r.json()["room_id"].as_str().unwrap().to_string();
    let r = app
        .post(
            &host.token,
            "/api/v1/calls/rooms",
            json!({"name": u("Size"), "max_participants": 50}),
        )
        .await;
    assert_eq!(r.json()["max_participants"], 3, "{}", r.text());
    let r = app
        .post(
            &host.token,
            "/api/v1/calls/rooms",
            json!({"name": u("Size"), "max_participants": 2}),
        )
        .await;
    assert_eq!(r.json()["max_participants"], 2, "{}", r.text());
    let r = app
        .post(
            &host.token,
            "/api/v1/calls/rooms",
            json!({"name": u("Size"), "max_participants": 1}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());

    // three people fill the room of three; the fourth is refused
    for _ in 0..3 {
        let who = app.user_with(&[]).await;
        let r = app
            .post(
                &who.token,
                &format!("/api/v1/calls/rooms/{room_id}/join"),
                json!({}),
            )
            .await;
        assert_eq!(r.status, 200, "{}", r.text());
    }
    let late = app.user_with(&[]).await;
    let r = app
        .post(
            &late.token,
            &format!("/api/v1/calls/rooms/{room_id}/join"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, 409, "{}", r.text());

    // an extra STUN server from the settings reaches every route that hands out ICE servers
    app.set_setting("calls.stun_urls", r#"["stun:stun.example.org:3478"]"#)
        .await;
    let r = app
        .get(&host.token, &format!("/api/v1/calls/rooms/{room_id}"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(
        stun_urls(&r.json()).contains(&"stun:stun.example.org:3478".to_string()),
        "{}",
        r.text()
    );

    // a malformed list is a clear settings error, not a silent fallback to someone else's server
    app.set_setting("calls.stun_urls", r#"["http://not-stun"]"#)
        .await;
    let r = app
        .get(&host.token, &format!("/api/v1/calls/rooms/{room_id}"))
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("calls.stun_urls"), "{}", r.text());

    app.set_setting("calls.stun_urls", "[]").await;
    app.set_setting("calls.max_participants", "10").await;
}
