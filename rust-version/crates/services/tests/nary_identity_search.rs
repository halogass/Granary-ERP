//! Port of tests/test_staff_whatsapp_links.py's search tests on `identity::search_chats` (the handler of
//! GET /nary/staff-chats/search is wave 4: `full` is what the router decides from inbox access). The WhatsApp
//! sidecar's /directory is a wiremock filtering the way the service does. One test function: the service URL
//! setting is process-wide.

mod support;

use granary_services::nary::identity;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

struct Directory(Vec<Value>);

impl Respond for Directory {
    fn respond(&self, req: &Request) -> ResponseTemplate {
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
        let hits: Vec<Value> = self
            .0
            .iter()
            .filter(|r| {
                let number = r["number"].as_str().unwrap_or("");
                if exact {
                    number == digits
                } else {
                    (q.chars().count() >= 2
                        && r["name"].as_str().unwrap().to_lowercase().contains(&q))
                        || (digits.len() >= 3 && number.contains(&digits))
                }
            })
            .cloned()
            .collect();
        ResponseTemplate::new(200).set_body_json(hits)
    }
}

async fn user(db: &mut PgConnection, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, $3, 'x', true, false, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{id}@granary.test"))
    .bind(name)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn staff_chat_search() {
    // The test company is in Japan: a national number (0...) is read with country code 81.
    std::env::set_var("PHONE_DEFAULT_COUNTRY_CODE", "81");
    let server = MockServer::builder().start().await;
    std::env::set_var("WHATSAPP_SERVICE_URL", server.uri());
    let pool = support::fresh_db("n8_identity_search").await;
    let mut conn = pool.acquire().await.unwrap();
    let db: &mut PgConnection = &mut conn;

    // test_people_with_inbox_access_search_names_numbers_and_groups
    let (bob, sarah, personal) = ("8181211110001", "8181211110002", "8181211110003");
    let group = "120363012345678@g.us";
    Mock::given(method("GET"))
        .and(path("/directory"))
        .respond_with(Directory(vec![
            json!({"id": format!("{bob}@c.us"), "name": "Bob Technician", "number": bob, "isGroup": false, "isMyContact": true, "hasChat": true}),
            json!({"id": format!("{sarah}@c.us"), "name": "Sarah Admin", "number": sarah, "isGroup": false, "isMyContact": true, "hasChat": false}),
            json!({"id": group, "name": "Technician ACME group", "number": null, "isGroup": true, "isMyContact": false, "hasChat": true}),
            json!({"id": format!("{personal}@c.us"), "name": "Technician next door", "number": personal, "isGroup": false, "isMyContact": true, "hasChat": true}),
        ]))
        .mount(&server)
        .await;
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, status, is_my_contact, attribution_asked) VALUES ($1, $2, $3, 'excluded', false, false)",
    )
    .bind(Uuid::new_v4())
    .bind(format!("{personal}@c.us"))
    .bind(format!("+{personal}"))
    .execute(&mut *db)
    .await
    .unwrap(); // a personal chat
    let other = user(db, "Other Person").await;
    identity::link_chat(db, other, Some(bob), None, Some(other))
        .await
        .unwrap(); // Bob is already someone's
    let res = identity::search_chats(db, Some("technician"), true, 20)
        .await
        .unwrap();
    let ids: std::collections::BTreeSet<String> = res
        .iter()
        .map(|r| r["chat_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        ids,
        [format!("{bob}@c.us"), group.to_string()]
            .into_iter()
            .collect()
    ); // the personal chat never shows
    let by_id = |id: &str| res.iter().find(|r| r["chat_id"] == id).unwrap().clone();
    assert_eq!(by_id(group)["is_group"], true);
    assert_eq!(by_id(&format!("{bob}@c.us"))["linked_to"], "Other Person");
    // The row is the Python dict, keys in order.
    let keys: Vec<&String> = res[0].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "chat_id",
            "name",
            "number",
            "is_group",
            "saved_contact",
            "has_chat",
            "linked_to"
        ]
    );
    // Not full: the link shows as "someone".
    let res = identity::search_chats(db, Some(&format!("0{}", &bob[2..])), false, 20)
        .await
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0]["linked_to"], "someone");

    // test_without_inbox_access_only_a_complete_number_is_confirmed
    server.reset().await;
    let mine = "8181299990001";
    Mock::given(method("GET"))
        .and(path("/directory"))
        .respond_with(Directory(vec![json!({"id": format!("{mine}@c.us"), "name": "Andy (office phone)", "number": mine,
                                            "isGroup": false, "isMyContact": true, "hasChat": true})]))
        .mount(&server)
        .await;
    assert!(identity::search_chats(db, Some("Andy"), false, 20)
        .await
        .unwrap()
        .is_empty());
    assert!(identity::search_chats(db, Some(&mine[..7]), false, 20)
        .await
        .unwrap()
        .is_empty());
    let exact = identity::search_chats(db, Some(&format!("0{}", &mine[2..])), false, 20)
        .await
        .unwrap(); // typed the local way
    assert_eq!(
        exact.iter().map(|r| r["name"].clone()).collect::<Vec<_>>(),
        vec![json!("Andy (office phone)")]
    );
    let asked: Vec<Request> = server.received_requests().await.unwrap();
    assert!(
        !asked.is_empty()
            && asked
                .iter()
                .all(|r| r.url.query_pairs().any(|(k, v)| k == "exact" && v == "1"))
    );
    // A full search needs two characters.
    assert!(identity::search_chats(db, Some("a"), true, 20)
        .await
        .unwrap()
        .is_empty());

    // test_the_stored_chats_answer_while_whatsapp_is_down
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/directory"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({"error": "not ready"})))
        .mount(&server)
        .await;
    let n = "8181277776543";
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, phone, display_name, status, is_my_contact, attribution_asked) \
         VALUES ($1, $2, $3, $4, 'active', true, false)",
    )
    .bind(Uuid::new_v4())
    .bind(format!("{n}@c.us"))
    .bind(format!("+{n}"))
    .bind(format!("Debbie Warehouse {}", &n[n.len() - 6..]))
    .execute(&mut *db)
    .await
    .unwrap();
    let res = identity::search_chats(
        db,
        Some(&format!("warehouse {}", &n[n.len() - 6..])),
        true,
        20,
    )
    .await
    .unwrap();
    assert_eq!(
        res.iter().map(|r| r["chat_id"].clone()).collect::<Vec<_>>(),
        vec![json!(format!("{n}@c.us"))]
    );
    assert_eq!(res[0]["number"], n);
    assert_eq!(res[0]["saved_contact"], true);
    assert_eq!(res[0]["linked_to"], Value::Null);
    // By part of the number too.
    let res = identity::search_chats(db, Some(&n[4..]), true, 20)
        .await
        .unwrap();
    assert_eq!(res.len(), 1);
}
