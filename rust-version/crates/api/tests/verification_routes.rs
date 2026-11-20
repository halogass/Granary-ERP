//! Port of backend/tests/test_verification.py through the routes (routers/verification.py). Python fakes
//! `service.generate_pdf` / `put_pdf` / `get_pdf`; here the PDF service and MinIO are one wiremock server
//! (verification_common). `PUBLIC_RATE_LIMIT` is raised as the Python fixture does. Approvals: Python submits
//! through /approvals; here the Owner's prepare-send (routers/delivery.py) approves on the spot, which is the
//! same engine call.
//!
//! `test_offer_letter_code_requires_an_hr_login` needs the /hr/offer-letters routes (group 10); its access
//! rules (anonymous 401, outsider 403, HR 200) are pinned by compat-tests/http-suite/tests/
//! test_verification_routes.py `test_hr_documents_need_a_company_login` on both stacks instead.

mod support;
mod verification_common;

use std::sync::atomic::Ordering;

use serde_json::json;
use uuid::Uuid;
use verification_common as c;

const TAG: &str = "verification";

fn app() -> &'static support::TestApp {
    let app = c::app(TAG);
    // Raised once: a test starting while test_public_view_is_rate_limited_per_ip runs must not undo its limit.
    static RAISED: std::sync::Once = std::sync::Once::new();
    RAISED.call_once(|| {
        granary_api::routers::verification::PUBLIC_RATE_LIMIT.store(10_000, Ordering::SeqCst)
    });
    app
}

async fn issuer(app: &support::TestApp) -> support::TestUser {
    c::make_person(
        app,
        &["sales_bizdev"],
        &[
            "approvals:view",
            "verification:issue",
            "verification:revoke",
        ],
    )
    .await
}

async fn approved_quotation(app: &support::TestApp) -> Uuid {
    let mut db = app.db().await;
    let product = c::make_product(&mut db).await;
    let (qid, _) = c::make_quotation(&mut db, Some(product), false).await;
    c::approve_as_owner(app, "quotation", qid).await;
    qid
}

async fn issue(app: &support::TestApp, token: &str, entity_type: &str, id: Uuid) -> support::Resp {
    app.post(
        token,
        "/api/v1/verify/issue",
        json!({"entity_type": entity_type, "entity_id": id.to_string()}),
    )
    .await
}

async fn state(app: &support::TestApp, token: &str) -> String {
    let r = app.get("", &format!("/api/v1/verify/t/{token}")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()["state"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_unapproved_documents_cannot_be_issued() {
    let app = app();
    let _g = app.serial().await;
    let person = issuer(app).await;
    let mut db = app.db().await;
    let product = c::make_product(&mut db).await;
    let (qid, _) = c::make_quotation(&mut db, Some(product), false).await;
    let r = issue(app, &person.token, "quotation", qid).await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(r.json()["detail"]["code"], "approval_required");
}

#[tokio::test]
async fn test_issued_quotation_verifies_publicly_and_serves_the_pdf() {
    let app = app();
    let _g = app.serial().await;
    let person = issuer(app).await;
    let qid = approved_quotation(app).await;
    let r = issue(app, &person.token, "quotation", qid).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let v = r.json();
    assert_eq!(v["status"], "active");
    assert!(v["url"]
        .as_str()
        .unwrap()
        .ends_with(&format!("/v/{}", v["token"].as_str().unwrap())));
    assert_eq!(v["allow_download"], true);
    assert_eq!(v["live_state"], "valid");
    let token = v["token"].as_str().unwrap();

    let meta = app.get("", &format!("/api/v1/verify/t/{token}")).await;
    let pdf = app.get("", &format!("/api/v1/verify/t/{token}/pdf")).await;
    let dl = app
        .get("", &format!("/api/v1/verify/t/{token}/pdf?download=true"))
        .await;
    let unknown = app
        .get("", &format!("/api/v1/verify/t/{}", Uuid::new_v4()))
        .await;
    let junk = app.get("", "/api/v1/verify/t/not-a-token").await;
    assert_eq!(meta.status, 200, "{}", meta.text());
    let body = meta.json();
    assert_eq!(body["state"], "valid");
    assert_eq!(body["can_view_document"], true);
    let labels: Vec<&str> = body["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"Document") && labels.contains(&"Total"));
    assert!(!body["approvals"].is_null(), "the approval record is shown");
    assert_eq!(pdf.status, 200);
    assert!(pdf.body.starts_with(b"%PDF"));
    assert!(pdf
        .header("content-disposition")
        .unwrap()
        .starts_with("inline"));
    assert_eq!(
        pdf.header("content-type").as_deref(),
        Some("application/pdf")
    );
    assert_eq!(pdf.header("cache-control").as_deref(), Some("no-store"));
    assert_eq!(
        pdf.header("x-robots-tag").as_deref(),
        Some("noindex, nofollow")
    );
    assert_eq!(dl.status, 200);
    assert!(dl
        .header("content-disposition")
        .unwrap()
        .starts_with("attachment"));
    assert_eq!((unknown.status, junk.status), (404, 404));

    let listed = app
        .get(
            &person.token,
            &format!("/api/v1/verify/entity/quotation/{qid}"),
        )
        .await
        .json();
    assert!(listed["verifications"][0]["view_count"].as_i64().unwrap() >= 1);
    assert_eq!(listed["supported"], true);
    // the internal copy
    let id = v["id"].as_str().unwrap();
    let internal = app
        .get(&person.token, &format!("/api/v1/verify/{id}/pdf"))
        .await;
    assert_eq!(internal.status, 200);
    assert!(internal.body.starts_with(b"%PDF"));
    assert!(internal
        .header("content-disposition")
        .unwrap()
        .starts_with("attachment; filename=\""));
    let views = app
        .get(
            &app.superadmin().await.token,
            &format!("/api/v1/verify/{id}/views"),
        )
        .await
        .json();
    let outcomes: Vec<&str> = views
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["download", "pdf", "ok"]);
}

#[tokio::test]
async fn test_revoked_and_reissued_codes_stop_verifying() {
    let app = app();
    let _g = app.serial().await;
    let person = issuer(app).await;
    let qid = approved_quotation(app).await;
    let first = issue(app, &person.token, "quotation", qid).await.json();
    let second = issue(app, &person.token, "quotation", qid).await.json();
    let id = second["id"].as_str().unwrap();
    let too_short = app
        .post(
            &person.token,
            &format!("/api/v1/verify/{id}/revoke"),
            json!({"reason": ""}),
        )
        .await;
    let revoked = app
        .post(
            &person.token,
            &format!("/api/v1/verify/{id}/revoke"),
            json!({"reason": "Sent to the wrong customer"}),
        )
        .await;
    let again = app
        .post(
            &person.token,
            &format!("/api/v1/verify/{id}/revoke"),
            json!({"reason": "Twice"}),
        )
        .await;
    let listed = app
        .get(
            &person.token,
            &format!("/api/v1/verify/entity/quotation/{qid}"),
        )
        .await
        .json();
    assert_eq!(
        (too_short.status, revoked.status, again.status),
        (422, 200, 409)
    );
    assert_eq!(revoked.json()["live_state"], "revoked");
    assert_eq!(
        state(app, first["token"].as_str().unwrap()).await,
        "superseded"
    );
    assert_eq!(
        state(app, second["token"].as_str().unwrap()).await,
        "revoked"
    );
    let gone = app
        .get(
            "",
            &format!("/api/v1/verify/t/{}/pdf", second["token"].as_str().unwrap()),
        )
        .await;
    assert_eq!(gone.status, 410);
    let mut statuses: Vec<&str> = listed["verifications"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status"].as_str().unwrap())
        .collect();
    statuses.sort();
    statuses.dedup();
    assert_eq!(statuses, ["revoked", "superseded"]);
}

#[tokio::test]
async fn test_changing_the_document_after_issue_invalidates_the_code() {
    let app = app();
    let _g = app.serial().await;
    let person = issuer(app).await;
    let qid = approved_quotation(app).await;
    let v = issue(app, &person.token, "quotation", qid).await.json();
    let mut db = app.db().await;
    sqlx::query("UPDATE quotation_items SET unit_price = 1 WHERE quotation_id = $1")
        .bind(qid)
        .execute(&mut db)
        .await
        .unwrap();
    assert_eq!(
        state(app, v["token"].as_str().unwrap()).await,
        "not_approved"
    );
}

#[tokio::test]
async fn test_superseding_the_approval_supersedes_the_code() {
    let app = app();
    let _g = app.serial().await;
    let person = issuer(app).await;
    let qid = approved_quotation(app).await;
    let v = issue(app, &person.token, "quotation", qid).await.json();
    let token = v["token"].as_str().unwrap().to_string();
    app.on_server(async move {
        let mut db = app.db().await;
        sqlx::query("BEGIN").execute(&mut db).await.unwrap();
        granary_services::approvals::engine::supersede_for_entity(
            &mut db,
            "quotation",
            qid,
            "Revised",
        )
        .await
        .unwrap();
        sqlx::query("COMMIT").execute(&mut db).await.unwrap();
    })
    .await;
    assert_eq!(state(app, &token).await, "superseded");
}

#[tokio::test]
async fn test_public_view_is_rate_limited_per_ip() {
    let app = app();
    let _g = app.serial().await;
    granary_api::routers::verification::PUBLIC_RATE_LIMIT.store(2, Ordering::SeqCst);
    let ip = format!("198.51.100.{}", Uuid::new_v4().as_u128() % 250);
    let mut statuses = Vec::new();
    for _ in 0..3 {
        let r = app
            .send(
                app.client()
                    .get(app.url(&format!("/api/v1/verify/t/{}", Uuid::new_v4())))
                    .header("X-Real-IP", &ip),
            )
            .await;
        statuses.push(r.status);
        if r.status == 429 {
            assert_eq!(
                r.json(),
                json!({"detail": "Too many verification requests: try again in a minute"})
            );
        }
    }
    granary_api::routers::verification::PUBLIC_RATE_LIMIT.store(10_000, Ordering::SeqCst);
    assert_eq!(statuses, [404, 404, 429]);
}
