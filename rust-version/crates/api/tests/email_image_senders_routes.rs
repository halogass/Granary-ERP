//! Security triage 18 (27 Oct 2026): "Always show images from <sender>" is kept per user on the server.

mod support;

use serde_json::json;
use support::{app, UserSpec};

const TAG: &str = "email_image_senders_routes";
const P: &str = "/api/v1/email/image-senders";

#[tokio::test]
async fn senders_are_kept_per_user_normalized_and_removable() {
    let app = app(TAG);
    let me = app.user_with(&[]).await;
    let other = app.user_with(&[]).await;

    let r = app.get(&me.token, P).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), json!({"senders": []}));

    let r = app
        .post(&me.token, P, json!({"email": "  News@Shop.Example "}))
        .await;
    assert_eq!(r.status, 201);
    assert_eq!(r.json(), json!({"email": "news@shop.example"}));
    // Adding again is not an error and keeps one row.
    let r = app
        .post(&me.token, P, json!({"email": "news@shop.example"}))
        .await;
    assert_eq!(r.status, 201);
    app.post(&me.token, P, json!({"email": "a@b.test"})).await;

    let r = app.get(&me.token, P).await;
    assert_eq!(
        r.json(),
        json!({"senders": ["a@b.test", "news@shop.example"]})
    );
    // Another user does not see them.
    assert_eq!(
        app.get(&other.token, P).await.json(),
        json!({"senders": []})
    );
    // Another user's delete does not touch mine.
    let r = app
        .delete(&other.token, &format!("{P}?email=a%40b.test"))
        .await;
    assert_eq!(r.status, 204);
    assert_eq!(app.get(&me.token, P).await.json()["senders"][0], "a@b.test");

    let r = app
        .delete(&me.token, &format!("{P}?email=News%40Shop.Example"))
        .await;
    assert_eq!(r.status, 204);
    assert_eq!(
        app.get(&me.token, P).await.json(),
        json!({"senders": ["a@b.test"]})
    );
}

#[tokio::test]
async fn bad_addresses_anonymous_and_portal_users_are_refused() {
    let app = app(TAG);
    let me = app.user_with(&[]).await;
    for bad in [
        "",
        "nobody",
        "a b@c.test",
        "<x@y.test>",
        "x@",
        "@y.test",
        "a@b@c",
    ] {
        let r = app.post(&me.token, P, json!({ "email": bad })).await;
        assert_eq!(r.status, 422, "{bad:?} should be refused");
    }
    assert_eq!(app.get("", P).await.status, 401);
    let portal = app.make_user(UserSpec::new().portal("partner")).await;
    assert_eq!(app.get(&portal.token, P).await.status, 403);
}
