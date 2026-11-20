//! The router half of backend/tests/test_staff_whatsapp_links.py::
//! test_without_inbox_access_only_a_complete_number_is_confirmed: GET /nary/staff-chats/search decides
//! `full` from the caller's permissions, and someone without inbox access (crm:view_whatsapp) or nary:manage
//! must not browse the company WhatsApp directory. The service half (`identity::search_chats` with
//! `full` given) is crates/services/tests/nary_identity_search.rs. The WhatsApp service is a wiremock fake
//! answering /directory the way the Python `directory(wa, rows)` handler does.

mod support;

use std::sync::OnceLock;

use serde_json::{json, Value};
use support::{app, TestApp};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TAG: &str = "nary_access";
const API: &str = "/api/v1/nary/staff-chats";

/// The one number the fake directory knows (`number()` in Python: 62812 + 8 digits).
fn mine() -> &'static str {
    static N: OnceLock<String> = OnceLock::new();
    N.get_or_init(|| {
        format!(
            "8190{}{:07}",
            1 + uuid::Uuid::new_v4().as_u128() % 9,
            uuid::Uuid::new_v4().as_u128() % 10_000_000
        )
    })
}

/// The fake WhatsApp service: lives on its own runtime for the whole process (set before the app starts).
fn wa() -> &'static MockServer {
    static WA: OnceLock<MockServer> = OnceLock::new();
    WA.get_or_init(|| {
        let mine = mine().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let server = MockServer::start().await;
                let rows = [
                    json!({"id": format!("{mine}@c.us"), "name": "Andy (office phone)",
                                       "number": mine, "isGroup": false, "isMyContact": true,
                                       "hasChat": true}),
                ];
                Mock::given(method("GET"))
                    .and(path("/directory"))
                    .respond_with(move |req: &Request| {
                        let param = |k: &str| {
                            req.url
                                .query_pairs()
                                .find(|(n, _)| n == k)
                                .map(|(_, v)| v.to_string())
                                .unwrap_or_default()
                        };
                        let exact = param("exact") == "1";
                        let q = param("q").to_lowercase();
                        let digits = param("digits");
                        let hits: Vec<Value> = rows
                            .iter()
                            .filter(|r| {
                                let number = r["number"].as_str().unwrap_or("");
                                if exact {
                                    number == digits
                                } else {
                                    (q.chars().count() >= 2
                                        && r["name"].as_str().unwrap().to_lowercase().contains(&q))
                                        || (digits.chars().count() >= 3 && number.contains(&digits))
                                }
                            })
                            .cloned()
                            .collect();
                        ResponseTemplate::new(200).set_body_json(Value::Array(hits))
                    })
                    .mount(&server)
                    .await;
                tx.send(server).unwrap();
                std::future::pending::<()>().await;
            });
        });
        let server = rx.recv().unwrap();
        std::env::set_var("WHATSAPP_SERVICE_URL", server.uri());
        server
    })
}

fn the_app() -> &'static TestApp {
    wa();
    app(TAG)
}

#[tokio::test]
async fn test_without_inbox_access_only_a_complete_number_is_confirmed() {
    let app = the_app();
    let _g = app.serial().await;
    let mine = mine();
    let staff = app.user_with(&["approvals:view"]).await;
    let search = |q: String| {
        let token = staff.token.clone();
        async move {
            let r = app
                .send(
                    app.client()
                        .get(app.url(&format!("{API}/search")))
                        .bearer_auth(token)
                        .query(&[("q", q)]),
                )
                .await;
            assert_eq!(r.status, 200, "{}", r.text());
            r.json()
        }
    };
    let browse = search("Andy".into()).await;
    let partial = search(mine[..7].to_string()).await;
    let exact = search(format!("+{mine}")).await; // the full international number
    assert_eq!(browse, json!({"full": false, "results": []}));
    assert_eq!(partial["results"], json!([]));
    let names: Vec<Value> = exact["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].clone())
        .collect();
    assert_eq!(names, vec![json!("Andy (office phone)")]);
    let asked: Vec<Request> = wa()
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/directory")
        .collect();
    assert!(
        !asked.is_empty()
            && asked
                .iter()
                .all(|r| r.url.query_pairs().any(|(k, v)| k == "exact" && v == "1")),
        "a caller without inbox access only ever asks for an exact number"
    );
}

/// Stage 8: PUT /nary/roles/{sop_role}: only the superadmin hands out the approver role (Owner, whose holder decides
/// Nary's cards and lends Nary its permissions); any role goes only to an active staff account; each change is
/// audit-logged.
#[tokio::test]
async fn test_only_the_superadmin_assigns_the_approver_role() {
    let app = the_app();
    let _g = app.serial().await;
    let roles = "/api/v1/nary/roles";
    let mut db = app.db().await;
    let before: Option<(bool, Option<uuid::Uuid>)> = sqlx::query_as(
        "SELECT is_nary, user_id FROM nary_role_assignments WHERE sop_role = 'Owner'",
    )
    .fetch_optional(&mut db)
    .await
    .unwrap();
    let manager = app.user_with(&["nary:manage", "nary:view"]).await;
    let staff = app.user_with(&["crm:view_own"]).await;
    let portal = app
        .make_user(support::UserSpec::new().portal("partner"))
        .await;
    let closed = app.user_with(&["crm:view_own"]).await;
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(closed.id)
        .execute(&mut db)
        .await
        .unwrap();

    // a nary:manage holder can't make anyone (themselves included) the approver, in any spelling
    for role in ["Owner", "owner", " Owner "] {
        let r = app
            .put(
                &manager.token,
                &format!("{roles}/{}", role.replace(' ', "%20")),
                json!({"user_id": manager.id.to_string()}),
            )
            .await;
        assert_eq!(r.status, 403, "{role}: {}", r.text());
    }
    let r = app
        .put(
            &manager.token,
            &format!("{roles}/Owner"),
            json!({"is_nary": true}),
        )
        .await;
    assert_eq!(r.status, 403);
    // other roles: only an active staff account
    let bdr = format!("BDR-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    for who in [portal.id, closed.id, uuid::Uuid::new_v4()] {
        let r = app
            .put(
                &manager.token,
                &format!("{roles}/{bdr}"),
                json!({"user_id": who.to_string()}),
            )
            .await;
        assert_eq!(r.status, 400, "{}", r.text());
    }
    let r = app
        .put(
            &manager.token,
            &format!("{roles}/{bdr}"),
            json!({"user_id": staff.id.to_string()}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let logged: (Option<uuid::Uuid>, Value) = sqlx::query_as(
        "SELECT user_id, new_values::jsonb FROM audit_logs WHERE action = 'nary_role_assign' \
         AND entity_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&bdr)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(logged.0, Some(manager.id));
    assert_eq!(logged.1["user_id"], json!(staff.id.to_string()));

    // the superadmin can
    let sa = app.superadmin().await;
    let r = app
        .put(
            &sa.token,
            &format!("{roles}/Owner"),
            json!({"user_id": staff.id.to_string()}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE action = 'nary_role_assign' AND entity_id = 'Owner' \
         AND user_id = $1",
    )
    .bind(sa.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert!(n >= 1);
    // put the approver back as it was
    match before {
        Some((is_nary, user_id)) => {
            sqlx::query(
                "UPDATE nary_role_assignments SET is_nary = $1, user_id = $2 WHERE sop_role = 'Owner'",
            )
            .bind(is_nary)
            .bind(user_id)
            .execute(&mut db)
            .await
            .unwrap();
        }
        None => {
            sqlx::query("DELETE FROM nary_role_assignments WHERE sop_role = 'Owner'")
                .execute(&mut db)
                .await
                .unwrap();
        }
    }
}
