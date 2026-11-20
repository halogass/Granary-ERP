//! The optional segment of leads and deals (services::segment) through the real app: free text while
//! crm.segments is empty, only the listed values once it lists some; the list filters (a value, or "none"); a
//! PATCH sets or clears it; a new deal starts with its lead's segment; GET /crm/stages serves the allowed values.

mod support;

use serde_json::{json, Value};
use support::{app, u};

const TAG: &str = "segment_routes";
const API: &str = "/api/v1";

fn ids(v: &Value) -> Vec<String> {
    let rows = v.get("items").unwrap_or(v);
    rows.as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn leads_and_deals_carry_an_optional_segment() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    app.set_setting("crm.segments", "[]").await;

    // free text while no values are configured; too long is refused; none at all is fine
    let free = u("Seg free");
    let r = app
        .post(
            &sa.token,
            &format!("{API}/crm/leads"),
            json!({"title": free, "source": "referral", "segment": "Hospitality"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["segment"], "Hospitality");
    let r = app
        .post(
            &sa.token,
            &format!("{API}/crm/leads"),
            json!({"title": u("Seg long"), "source": "referral", "segment": "x".repeat(31)}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    let r = app
        .post(
            &sa.token,
            &format!("{API}/crm/leads"),
            json!({"title": u("Seg none"), "source": "referral"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let none_id = r.json()["id"].as_str().unwrap().to_string();
    let r = app.get(&sa.token, &format!("{API}/crm/stages")).await;
    assert_eq!(r.json()["segments"], json!([]));

    // a configured list: only its values
    app.set_setting("crm.segments", r#"["retail", "public_sector"]"#)
        .await;
    let r = app.get(&sa.token, &format!("{API}/crm/stages")).await;
    assert_eq!(r.json()["segments"], json!(["retail", "public_sector"]));
    let r = app
        .post(
            &sa.token,
            &format!("{API}/crm/leads"),
            json!({"title": u("Seg gold"), "source": "referral", "segment": "gold"}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    assert!(
        r.text().contains("retail"),
        "names the values: {}",
        r.text()
    );
    let r = app
        .post(
            &sa.token,
            &format!("{API}/crm/leads"),
            json!({"title": u("Seg retail"), "source": "referral", "segment": "retail"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let retail_id = r.json()["id"].as_str().unwrap().to_string();

    // the list filters: a value, and "none"
    let r = app
        .get(
            &sa.token,
            &format!("{API}/crm/leads?segment=retail&limit=500"),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let got = ids(&r.json());
    assert!(
        got.contains(&retail_id) && !got.contains(&none_id),
        "{got:?}"
    );
    let r = app
        .get(
            &sa.token,
            &format!("{API}/crm/leads?segment=none&limit=500"),
        )
        .await;
    let got = ids(&r.json());
    assert!(
        got.contains(&none_id) && !got.contains(&retail_id),
        "{got:?}"
    );

    // the detail carries it; a PATCH changes it and null clears it
    let r = app
        .get(&sa.token, &format!("{API}/crm/leads/{retail_id}"))
        .await;
    assert_eq!(r.json()["segment"], "retail");
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/crm/leads/{none_id}"),
            json!({"segment": "public_sector"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = app
        .get(&sa.token, &format!("{API}/crm/leads/{none_id}"))
        .await;
    assert_eq!(r.json()["segment"], "public_sector");
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/crm/leads/{none_id}"),
            json!({"segment": null}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = app
        .get(&sa.token, &format!("{API}/crm/leads/{none_id}"))
        .await;
    assert_eq!(r.json()["segment"], Value::Null);

    // a deal starts with its lead's segment; the deal list filters by it; a PATCH changes it
    let r = app
        .post(
            &sa.token,
            &format!("{API}/crm/deals"),
            json!({"title": u("Seg deal"), "lead_id": retail_id}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let deal = r.json()["id"].as_str().unwrap().to_string();
    let r = app.get(&sa.token, &format!("{API}/crm/deals/{deal}")).await;
    assert_eq!(r.json()["segment"], "retail");
    let r = app
        .get(
            &sa.token,
            &format!("{API}/crm/deals?segment=retail&limit=500"),
        )
        .await;
    assert!(ids(&r.json()).contains(&deal));
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/crm/deals/{deal}"),
            json!({"segment": "gold"}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/crm/deals/{deal}"),
            json!({"segment": "public_sector"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = app
        .get(
            &sa.token,
            &format!("{API}/crm/deals?segment=public_sector&limit=500"),
        )
        .await;
    assert!(ids(&r.json()).contains(&deal));
    app.set_setting("crm.segments", "[]").await;
}
