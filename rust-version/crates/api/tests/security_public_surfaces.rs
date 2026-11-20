//! Security stage 8, group P (public surfaces): regression tests for the branding images (M1), the FX key in
//! error text (L1) and the unauthenticated writes (L4). The signing (M2, L2), partner training (L3) and vendor
//! upload (L6) tests live with their routers' tests (signing_routes.rs, partner_routes.rs, vendor_routes.rs).
//!
//! Public routes count per address: each test sends its own X-Real-IP so the files' tests never share a bucket.

mod support;

use serde_json::{json, Value};
use support::{app, u, TestApp};
use uuid::Uuid;

const TAG: &str = "security_p";

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"alert(document.cookie)\"/>";

fn b64(data: &[u8]) -> String {
    granary_compat::crypto::b64_std(data)
}

/// A fresh address for the per-address limits.
fn new_ip() -> String {
    let n = Uuid::new_v4().as_u128();
    format!("10.{}.{}.{}", (n >> 16) as u8, (n >> 8) as u8, n as u8)
}

async fn upload(
    app: &TestApp,
    token: &str,
    what: &str,
    name: &str,
    mime: &str,
    data: &[u8],
) -> support::Resp {
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(name.to_string())
            .mime_str(mime)
            .unwrap(),
    );
    app.send(
        app.client()
            .post(app.url(&format!("/api/v1/system/branding/{what}")))
            .bearer_auth(token)
            .multipart(form),
    )
    .await
}

async fn post_from(app: &TestApp, ip: &str, path: &str, body: Value) -> support::Resp {
    app.send(
        app.client()
            .post(app.url(path))
            .header("x-real-ip", ip)
            .json(&body),
    )
    .await
}

// ─── M1: favicon and logo ─────────────────────────────────────────────────────

#[tokio::test]
async fn branding_images_refuse_svg_and_bytes_that_do_not_match() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    // SVG is no longer accepted, declared as such
    for what in ["favicon", "logo"] {
        let r = upload(app, &sa.token, what, "x.svg", "image/svg+xml", SVG).await;
        assert_eq!(r.status, 400, "{what}: {}", r.text());
    }
    // ... nor claimed to be a PNG: the bytes decide
    let r = upload(app, &sa.token, "favicon", "x.png", "image/png", SVG).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"detail": "The file is not a PNG, JPEG or ICO image. Choose another file."})
    );
    let r = upload(app, &sa.token, "logo", "x.png", "image/png", SVG).await;
    assert_eq!(r.status, 400, "{}", r.text());
    // a real PNG is stored under its sniffed type
    let r = upload(app, &sa.token, "favicon", "x.ico", "image/x-icon", PNG).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.json()["data_url"],
        json!(format!("data:image/png;base64,{}", b64(PNG)))
    );
    let ico = b"\x00\x00\x01\x00\x01\x00\x10\x10";
    let r = upload(app, &sa.token, "favicon", "x.ico", "image/x-icon", ico).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json()["data_url"]
        .as_str()
        .unwrap()
        .starts_with("data:image/x-icon;base64,"));
}

#[tokio::test]
async fn the_generic_settings_patch_only_takes_checked_branding_images() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    app.set_setting("branding.favicon_data", "").await;
    app.set_setting("branding.logo_image_data", "").await;
    let patch = |key: &'static str, value: String| {
        let token = sa.token.clone();
        async move {
            app.patch(
                &token,
                &format!("/api/v1/system/settings/{key}"),
                json!({"value": value}),
            )
            .await
        }
    };
    for (key, value) in [
        (
            "branding.favicon_data",
            format!("data:image/svg+xml;base64,{}", b64(SVG)),
        ),
        (
            "branding.favicon_data",
            format!("data:text/html;base64,{}", b64(b"<script>1</script>")),
        ),
        // a PNG type with SVG bytes
        (
            "branding.favicon_data",
            format!("data:image/png;base64,{}", b64(SVG)),
        ),
        ("branding.favicon_data", "javascript:alert(1)".to_string()),
        (
            "branding.logo_image_data",
            format!("data:image/svg+xml;base64,{}", b64(SVG)),
        ),
    ] {
        let r = patch(key, value.clone()).await;
        assert_eq!(r.status, 400, "{key} = {value}: {}", r.text());
    }
    let png = format!("data:image/png;base64,{}", b64(PNG));
    let r = patch("branding.favicon_data", png.clone()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["value"], json!(png));
    let r = patch("branding.favicon_data", String::new()).await;
    assert_eq!(r.status, 200, "{}", r.text());
}

// ─── L1: the FX API key never shows in an error ───────────────────────────────

#[tokio::test]
async fn the_fx_key_test_never_shows_the_key() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let key = format!("SECRETFX{}", Uuid::new_v4().simple());
    app.set_setting("currency.api_key", &key).await;
    granary_services::fx::set_api_base_for_tests(Some("http://127.0.0.1:9"));
    let r = app
        .post(
            &sa.token,
            "/api/v1/system/currencies/test-api-key",
            json!({}),
        )
        .await;
    granary_services::fx::set_api_base_for_tests(None);
    app.set_setting("currency.api_key", "").await;
    assert_eq!(r.status, 502, "{}", r.text());
    assert!(
        r.text().starts_with("{\"detail\":\"API request failed: "),
        "{}",
        r.text()
    );
    assert!(!r.text().contains(&key), "the key leaked: {}", r.text());
}

// ─── L4: unauthenticated writes ───────────────────────────────────────────────

#[tokio::test]
async fn a_public_ticket_takes_only_public_priorities_and_capped_lengths() {
    let app = app(TAG);
    let _g = app.serial().await;
    let ip = new_ip();
    let base = || json!({"customer_name": "John Doe", "customer_email": "john@example.com", "subject": "Help"});
    let url = "/api/v1/helpdesk/public/tickets";
    let mut body = base();
    body["priority"] = json!("critical");
    let r = post_from(app, &ip, url, body).await;
    assert_eq!(r.status, 422, "{}", r.text());
    let mut body = base();
    body["source"] = json!("email");
    let r = post_from(app, &ip, url, body).await;
    assert_eq!(r.status, 422, "{}", r.text());
    for (field, n) in [
        ("subject", 501),
        ("customer_name", 256),
        ("description", 10_001),
    ] {
        let mut body = base();
        body[field] = json!("x".repeat(n));
        let r = post_from(app, &ip, url, body).await;
        assert_eq!(r.status, 422, "{field}: {}", r.text());
    }
    let mut body = base();
    body["priority"] = json!("high");
    let r = post_from(app, &ip, url, body).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let token = r.json()["public_token"].as_str().unwrap().to_string();
    let mut db = app.db().await;
    let (priority, source): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT priority, source FROM helpdesk_tickets WHERE public_token = $1::uuid",
    )
    .bind(&token)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (priority.as_deref(), source.as_deref()),
        (Some("high"), Some("portal"))
    );
}

#[tokio::test]
async fn portal_registrations_cap_their_lengths() {
    let app = app(TAG);
    let _g = app.serial().await;
    let ip = new_ip();
    for kind in ["vendor", "partner"] {
        let url = format!("/api/v1/portal/register/{kind}");
        let ok = json!({"company_name": u("Acme Ltd"), "contact_name": "Rachel Green",
                        "email": format!("{}@example.com", u("rachel"))});
        for (field, n) in [
            ("notes", 5001),
            ("company_name", 256),
            ("website", 501),
            ("phone", 51),
        ] {
            let mut body = ok.clone();
            body[field] = json!("x".repeat(n));
            let r = post_from(app, &ip, &url, body).await;
            assert_eq!(r.status, 422, "{kind} {field}: {}", r.text());
        }
        let r = post_from(app, &ip, &url, ok).await;
        assert_eq!(r.status, 201, "{kind}: {}", r.text());
    }
}

#[tokio::test]
async fn public_writes_are_limited_per_address() {
    let app = app(TAG);
    let _g = app.serial().await;
    app.set_setting("public.forms.requests_per_minute", "2")
        .await;
    let minute = || chrono::Utc::now().format("%Y%m%d%H%M").to_string();
    let ticket =
        json!({"customer_name": "John Doe", "customer_email": "john@example.com", "subject": "Hi"});
    let reg = || {
        json!({"company_name": u("Acme"), "contact_name": "Rachel Green",
               "email": format!("{}@example.com", u("r"))})
    };
    let mut checked = 0;
    for (url, body) in [
        ("/api/v1/helpdesk/public/tickets", ticket.clone()),
        ("/api/v1/portal/register/vendor", reg()),
    ] {
        // A burst that crosses a minute boundary starts a new window: try again then.
        for _ in 0..3 {
            let (ip, other_ip) = (new_ip(), new_ip());
            let before = minute();
            let mut codes = Vec::new();
            for _ in 0..3 {
                let mut b = body.clone();
                if b.get("company_name").is_some() {
                    b = reg();
                }
                codes.push(post_from(app, &ip, url, b).await.status);
            }
            let other = post_from(app, &other_ip, url, body.clone()).await.status;
            if minute() != before {
                continue;
            }
            assert_eq!(codes, [201, 201, 429], "{url}");
            assert_eq!(other, 201, "{url}: another address is not limited");
            checked += 1;
            break;
        }
    }
    app.set_setting("public.forms.requests_per_minute", "10")
        .await;
    assert_eq!(checked, 2);
}

/// A live video room hosted by a fresh user; returns its room_id and the host's token.
async fn room(app: &TestApp) -> (String, String) {
    let host = app.user_with(&[]).await;
    let room_id = u("room");
    let mut db = app.db().await;
    sqlx::query(
        "INSERT INTO video_rooms (id, room_id, name, created_by_id, is_active, max_participants, participants, \
         waiting_participants, created_at, updated_at) VALUES ($1, $2, 'Weekly', $3, true, 10, '[]', '[]', now(), now())",
    )
    .bind(Uuid::new_v4())
    .bind(&room_id)
    .bind(host.id)
    .execute(&mut db)
    .await
    .unwrap();
    (room_id, host.token)
}

async fn waiting_entries(app: &TestApp, room_id: &str) -> Vec<Value> {
    let mut db = app.db().await;
    let v: Option<Value> =
        sqlx::query_scalar("SELECT waiting_participants FROM video_rooms WHERE room_id = $1")
            .bind(room_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    v.and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

#[tokio::test]
async fn the_waiting_room_caps_names_and_guests_and_keeps_every_join() {
    let app = app(TAG);
    let _g = app.serial().await;
    app.set_setting("public.forms.requests_per_minute", "1000")
        .await;
    let (room_id, _) = room(app).await;
    let url = format!("/api/v1/calls/rooms/{room_id}/guest-wait");
    let r = post_from(app, &new_ip(), &url, json!({"name": "x".repeat(101)})).await;
    assert_eq!(r.status, 422, "{}", r.text());
    // Ten guests at once: with the row locked, no join overwrites another.
    let joins = (0..10).map(|i| {
        let url = url.clone();
        async move {
            post_from(app, &new_ip(), &url, json!({"name": format!("Guest {i}")}))
                .await
                .status
        }
    });
    let codes = futures::future::join_all(joins).await;
    assert!(codes.iter().all(|c| *c == 200), "{codes:?}");
    assert_eq!(waiting_entries(app, &room_id).await.len(), 10);
    // The cap: calls.guest_wait.max_waiting guests waiting at once.
    app.set_setting("calls.guest_wait.max_waiting", "10").await;
    let r = post_from(app, &new_ip(), &url, json!({"name": "One too many"})).await;
    app.set_setting("calls.guest_wait.max_waiting", "50").await;
    app.set_setting("public.forms.requests_per_minute", "10")
        .await;
    assert_eq!(r.status, 429, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"detail": "The waiting room is full. Try again in a few minutes."})
    );
    assert_eq!(waiting_entries(app, &room_id).await.len(), 10);
}
