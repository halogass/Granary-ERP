//! 10 Nov 2026: list pages stopped at the API's default page (50) and several search boxes sent a parameter
//! nothing read. The lists page with limit/offset, and the searches reach the server.

mod support;

use serde_json::{json, Value};
use support::{app, u};

const TAG: &str = "list_search";

fn names(v: &Value, key: &str) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| r[key].as_str().unwrap_or("").to_string())
        .collect()
}

#[tokio::test]
async fn organizations_page_search_every_column_and_carry_created_at() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let tag = u("Orgsrch").replace(' ', "");
    for i in 0..5 {
        let r = app
            .post(
                &sa.token,
                "/api/v1/crm/organizations",
                json!({"name": format!("{tag} {i}"), "city": if i == 4 { "Katowice" } else { "Tokyo" }}),
            )
            .await;
        assert_eq!(r.status, 201, "{}", r.text());
    }
    // the page used to send `search`; both names work now
    for p in ["q", "search"] {
        let r = app
            .get(
                &sa.token,
                &format!("/api/v1/crm/organizations?{p}={tag}&limit=2&offset=0"),
            )
            .await;
        assert_eq!(r.status, 200, "{}", r.text());
        assert_eq!(
            names(&r.json(), "name"),
            vec![format!("{tag} 0"), format!("{tag} 1")]
        );
        assert!(
            r.json()[0]["created_at"]
                .as_str()
                .is_some_and(|t| !t.is_empty()),
            "{}",
            r.text()
        );
    }
    let r = app
        .get(
            &sa.token,
            &format!("/api/v1/crm/organizations?q={tag}&limit=2&offset=4"),
        )
        .await;
    assert_eq!(names(&r.json(), "name"), vec![format!("{tag} 4")]);
    // a column other than the name: the city
    let r = app
        .get(
            &sa.token,
            &format!("/api/v1/crm/organizations?q={tag}&limit=50"),
        )
        .await;
    assert_eq!(r.json().as_array().unwrap().len(), 5);
    let r = app
        .get(&sa.token, "/api/v1/crm/organizations?q=katowice&limit=500")
        .await;
    assert!(
        names(&r.json(), "name").contains(&format!("{tag} 4")),
        "{}",
        r.text()
    );
}

#[tokio::test]
async fn leads_and_deals_search_title_and_organization() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let tag = u("Ldsrch").replace(' ', "");
    let org = app
        .post(
            &sa.token,
            "/api/v1/crm/organizations",
            json!({"name": format!("{tag} Holdings")}),
        )
        .await;
    let org_id = org.json()["id"].as_str().unwrap().to_string();
    let r = app
        .post(
            &sa.token,
            "/api/v1/crm/leads",
            json!({"title": "Plain lead", "source": "email", "organization_id": org_id}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let r = app
        .post(
            &sa.token,
            "/api/v1/crm/leads",
            json!({"title": format!("{tag} titled"), "source": "email"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    for p in ["q", "search"] {
        let r = app
            .get(&sa.token, &format!("/api/v1/crm/leads?{p}={tag}"))
            .await;
        assert_eq!(r.status, 200, "{}", r.text());
        let mut got = names(&r.json(), "title");
        got.sort();
        assert_eq!(got, vec![format!("{tag} titled"), "Plain lead".to_string()]);
    }
    let r = app
        .post(
            &sa.token,
            "/api/v1/crm/deals",
            json!({"title": format!("{tag} deal"), "stage": "new"}),
        )
        .await;
    assert!(r.status == 200 || r.status == 201, "{}", r.text());
    let r = app
        .get(&sa.token, &format!("/api/v1/crm/deals?q={tag}"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(names(&r.json(), "title"), vec![format!("{tag} deal")]);
    let r = app
        .get(&sa.token, "/api/v1/crm/deals?q=zz-no-such-deal-zz")
        .await;
    assert_eq!(r.json(), json!([]));
}

#[tokio::test]
async fn quotations_search_and_users_portal_type_reach_the_query() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let r = app
        .get(
            &sa.token,
            "/api/v1/sales/quotations?search=zz-no-such-quotation-zz",
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!([]));
    let r = app
        .get(
            &sa.token,
            "/api/v1/sales/quotations?q=zz-no-such-quotation-zz&status=draft",
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!([]));
    let r = app
        .get(
            &sa.token,
            "/api/v1/users?portal_type=vendor,partner&limit=500",
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    for row in r.json().as_array().unwrap() {
        let t = row["portal_type"].as_str().unwrap_or("");
        assert!(t == "vendor" || t == "partner", "{row}");
    }
    let r = app
        .get(&sa.token, "/api/v1/sales/documents?limit=1&offset=0")
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json().as_array().unwrap().len() <= 1);
}
