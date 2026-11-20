//! Partner training certificates: `GET /partner/portal/training/certifications/{id}/pdf` renders the caller's
//! own certificate with the PDF service (a fake here). One test: the fake's address must be in the environment
//! before the app starts.

mod support;

use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{u, UserSpec};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const API: &str = "/api/v1/partner";

async fn partner_account(db: &mut PgConnection, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO partner_accounts (id, company_name, partner_type, tier, status, territory_countries, \
           territory_verticals, authorized_products, authorized_services, is_exclusive, commitment_currency, \
           certified_staff_count, certification_details, cumulative_units, cumulative_revenue, current_year_units, \
           current_year_revenue, is_deleted) \
         VALUES ($1, $2, 'reseller', 'authorized', 'active', '{}', '{}', '{}', '{}', false, 'USD', 0, '[]', \
           0, 0, 0, 0, false)",
    )
    .bind(id)
    .bind(name)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn certificate_pdf_is_the_callers_own() {
    let pdf = MockServer::start().await;
    std::env::set_var("PDF_SERVICE_URL", pdf.uri());
    let app = support::app("partner_certificate_pdf");
    Mock::given(method("POST"))
        .and(path("/render"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-cert".to_vec()))
        .mount(&pdf)
        .await;

    let mut db = app.db().await;
    let company = u("Kanda Logistics");
    let pa = partner_account(&mut db, &company).await;
    let holder = app
        .make_user(UserSpec::new().portal("partner").name("Pyt Holder"))
        .await;
    let other = app.make_user(UserSpec::new().portal("partner")).await;
    for uid in [holder.id, other.id] {
        sqlx::query("UPDATE users SET partner_account_id = $2 WHERE id = $1")
            .bind(uid)
            .bind(pa)
            .execute(&mut db)
            .await
            .unwrap();
    }
    let staff = app.make_user(UserSpec::new()).await;
    let (mid, cert) = (Uuid::new_v4(), Uuid::new_v4());
    let title = u("Product fundamentals");
    sqlx::query(
        "INSERT INTO training_modules (id, title, min_tier, partner_types, is_published, pass_score, is_deleted) \
         VALUES ($1, $2, 'authorized', '{}', true, 70, false)",
    )
    .bind(mid)
    .bind(&title)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO partner_certifications (id, partner_user_id, module_id, issued_at, badge_slug, created_at, \
           updated_at) VALUES ($1, $2, $3, '2026-11-20T03:00:00Z', 'pf', now(), now())",
    )
    .bind(cert)
    .bind(holder.id)
    .bind(mid)
    .execute(&mut db)
    .await
    .unwrap();

    let url = format!("{API}/portal/training/certifications/{cert}/pdf");
    let res = app.get(&holder.token, &url).await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(res.body, b"%PDF-cert");
    assert_eq!(res.header("content-type").unwrap(), "application/pdf");
    let number = format!("CERT-{}", cert.simple().to_string()[..8].to_uppercase());
    assert_eq!(
        res.header("content-disposition").unwrap(),
        format!("attachment; filename=\"{number}.pdf\"")
    );
    let renders = pdf.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&renders.last().unwrap().body).unwrap();
    assert_eq!(sent["template"], json!("partner_certificate"));
    let data = &sent["data"];
    assert_eq!(data["holder_name"], json!("Pyt Holder"));
    assert_eq!(data["partner_company"], json!(company));
    assert_eq!(data["module_title"], json!(title));
    assert_eq!(data["certificate_number"], json!(number));

    // Someone else's certificate (even a colleague's) is not found; staff are not partner users.
    assert_eq!(app.get(&other.token, &url).await.status, 404);
    assert_eq!(app.get(&staff.token, &url).await.status, 403);
}
