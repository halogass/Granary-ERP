//! routers/partner.py through the real app. Ports of the backend tests that seed rows directly
//! (test_audit_medium_w2.py M43/M49, test_audit_high_b1.py H26/H27, test_audit_low_w1.py L64,
//! test_audit_gaps_c.py manual close and the AI write path), plus the routes whose effects need a fake
//! (the invite email, the MDF proof upload) or a direct read of the rows. The HTTP-only behaviour of every
//! route is pinned on both stacks by compat-tests/http-suite/tests/test_partner_*.py.

#![allow(clippy::type_complexity)]

mod support;

use std::sync::{Arc, Mutex};

use chrono::{Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::PgConnection;
use support::{app, u, UserSpec};
use uuid::Uuid;

const TAG: &str = "partner";
const API: &str = "/api/v1/partner";

fn dec(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// `PartnerAccount(company_name=_u("Partner"), partner_type="reseller", ...)`.
async fn partner(
    db: &mut PgConnection,
    commitment: Option<&str>,
    cumulative: Option<&str>,
    current: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO partner_accounts (id, company_name, partner_type, tier, status, territory_countries, \
           territory_verticals, authorized_products, authorized_services, is_exclusive, commitment_currency, \
           certified_staff_count, certification_details, cumulative_units, cumulative_revenue, current_year_units, \
           current_year_revenue, is_deleted) \
         VALUES ($1, $2, 'reseller', 'authorized', 'pending_agreement', '{}', '{}', '{}', '{}', false, $3, 0, '[]', \
           0, $4, 0, $5, false)",
    )
    .bind(id)
    .bind(u("Partner"))
    .bind(commitment.unwrap_or("USD"))
    .bind(cumulative.map(dec).unwrap_or_default())
    .bind(current.map(dec).unwrap_or_default())
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

/// `PartnerDealRegistration(partner_id=, title="Deal", status=, currency=, estimated_value=, registered_at=now)`.
async fn registration(
    db: &mut PgConnection,
    partner_id: Uuid,
    status: &str,
    value: Option<&str>,
    currency: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO partner_deal_registrations (id, partner_id, title, status, currency, estimated_value, \
           registered_at, protection_days, products_services) VALUES ($1, $2, 'Deal', $3, $4, $5, now(), 90, '{}')",
    )
    .bind(id)
    .bind(partner_id)
    .bind(status)
    .bind(currency)
    .bind(value.map(dec))
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn rate(db: &mut PgConnection, from: &str, to: &str, rate: &str, on: NaiveDate) {
    sqlx::query(
        "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, effective_date, source, is_deleted) \
         VALUES ($1, $2, $3, $4, $5, 'manual', false)",
    )
    .bind(Uuid::new_v4())
    .bind(from)
    .bind(to)
    .bind(dec(rate))
    .bind(on)
    .execute(&mut *db)
    .await
    .unwrap();
}

async fn revenue(db: &mut PgConnection, partner_id: Uuid) -> (Decimal, Decimal) {
    let (a, b): (Option<Decimal>, Option<Decimal>) = sqlx::query_as(
        "SELECT cumulative_revenue, current_year_revenue FROM partner_accounts WHERE id = $1",
    )
    .bind(partner_id)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    (a.unwrap_or_default(), b.unwrap_or_default())
}

async fn reg_status(db: &mut PgConnection, reg_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM partner_deal_registrations WHERE id = $1")
        .bind(reg_id)
        .fetch_one(&mut *db)
        .await
        .unwrap()
}

async fn portal_user(partner_id: Uuid) -> support::TestUser {
    let app = app(TAG);
    let user = app.make_user(UserSpec::new().portal("partner")).await;
    let mut db = app.db().await;
    sqlx::query("UPDATE users SET partner_account_id = $2 WHERE id = $1")
        .bind(user.id)
        .bind(partner_id)
        .execute(&mut db)
        .await
        .unwrap();
    user
}

// ─── test_audit_medium_w2.py (M43, M49) ──────────────────────────────────────

#[tokio::test]
async fn test_a_won_deal_adds_its_converted_value_to_partner_revenue() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    sqlx::query("DELETE FROM exchange_rates WHERE from_currency = 'SGD' AND to_currency = 'EUR'")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM exchange_rates WHERE from_currency = 'EUR' AND to_currency = 'SGD'")
        .execute(&mut db)
        .await
        .unwrap();
    let p = partner(&mut db, Some("EUR"), Some("0"), Some("0")).await;
    let deal = registration(&mut db, p, "approved", Some("1000"), "SGD").await;
    let url = format!("{API}/deal-registrations/{deal}/close");
    let missing = app.patch(&sa.token, &url, json!({"outcome": "won"})).await;
    assert_eq!(missing.status, 400);
    assert!(missing.json()["detail"]
        .as_str()
        .unwrap()
        .contains("SGD/EUR"));
    // `date.today()`: two days back, so the company time zone cannot put it after the closing date.
    let recent = Utc::now().date_naive() - chrono::Duration::days(2);
    rate(&mut db, "SGD", "EUR", "0.6789", recent).await;
    let res = app
        .patch(
            &sa.token,
            &url,
            json!({"outcome": "won", "final_value": 1200.5}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(res.json()["revenue_added"], json!(815.02)); // 1200.5 x 0.6789 = 815.019...
    assert_eq!(revenue(&mut db, p).await, (dec("815.02"), dec("815.02")));
    assert_eq!(reg_status(&mut db, deal).await, "closed_won");
    assert_eq!(
        app.patch(&sa.token, &url, json!({"outcome": "won"}))
            .await
            .status,
        400
    ); // closes once

    let lost = registration(&mut db, p, "approved", Some("1000"), "SGD").await;
    let res = app
        .patch(
            &sa.token,
            &format!("{API}/deal-registrations/{lost}/close"),
            json!({"outcome": "lost"}),
        )
        .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.json()["revenue_added"], Value::Null);
    let pending = registration(&mut db, p, "pending", Some("1000"), "SGD").await;
    let res = app
        .patch(
            &sa.token,
            &format!("{API}/deal-registrations/{pending}/close"),
            json!({"outcome": "won"}),
        )
        .await;
    assert_eq!(res.status, 400);
    assert_eq!(
        res.json(),
        json!({"detail": "A pending registration can't be closed (allowed: approved, extended)"})
    );
    assert_eq!(revenue(&mut db, p).await.0, dec("815.02"));
}

#[tokio::test]
async fn test_commission_amount_is_computed_and_paying_it_is_not_revenue() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let p = partner(&mut db, None, Some("0"), Some("0")).await;
    let deal = registration(&mut db, p, "approved", Some("2000"), "USD").await;
    let year = Utc::now().year();
    let res = app
        .post(
            &sa.token,
            &format!("{API}/commissions"),
            json!({"partner_id": p.to_string(), "deal_registration_id": deal.to_string(), "rate": 0.125,
                   "amount": 999999, "period_year": year}),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let body = res.json();
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["id", "status", "amount", "currency"]
    );
    assert_eq!(body["amount"], json!(250.0));
    let comm_id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let (base, amount, currency): (Decimal, Decimal, Option<String>) = sqlx::query_as(
        "SELECT base_amount, amount, currency FROM partner_commissions WHERE id = $1",
    )
    .bind(comm_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (base.to_string(), amount.to_string(), currency.as_deref()),
        ("2000.00".to_string(), "250.00".to_string(), Some("USD"))
    );
    let res = app
        .post(
            &sa.token,
            &format!("{API}/commissions"),
            json!({"partner_id": p.to_string(), "base_amount": 100, "rate": 10}),
        )
        .await;
    assert_eq!(res.status, 422); // rate is a fraction
    assert_eq!(
        res.json()["errors"],
        json!([{"field": "rate", "message": "Input should be less than or equal to 1"}])
    );

    let url = format!("{API}/commissions/{comm_id}/status");
    let approver = app.user_with(&["partner_mgmt:approve_commission"]).await;
    let payer = app.user_with(&["partner_mgmt:pay_commission"]).await;
    let r = app
        .patch(&payer.token, &url, json!({"status": "approved"}))
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        r.json(),
        json!({"detail": "Permission required: partner_mgmt:approve_commission"})
    );
    assert_eq!(
        app.patch(&approver.token, &url, json!({"status": "approved"}))
            .await
            .status,
        200
    );
    assert_eq!(
        app.patch(&approver.token, &url, json!({"status": "paid"}))
            .await
            .status,
        403
    );
    assert_eq!(
        app.patch(&payer.token, &url, json!({"status": "paid"}))
            .await
            .status,
        200
    );
    assert_eq!(revenue(&mut db, p).await, (dec("0"), dec("0")));
    let (approved_by, paid_at): (Option<Uuid>, Option<chrono::DateTime<Utc>>) =
        sqlx::query_as("SELECT approved_by_id, paid_at FROM partner_commissions WHERE id = $1")
            .bind(comm_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(approved_by, Some(approver.id));
    assert!(paid_at.is_some());
}

async fn commission(
    db: &mut PgConnection,
    partner_id: Uuid,
    amount: &str,
    currency: &str,
    status: &str,
    paid: bool,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO partner_commissions (id, partner_id, commission_type, base_amount, rate, amount, currency, \
           status, paid_at) VALUES ($1, $2, 'finder_fee', 1000, 0.1, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(partner_id)
    .bind(dec(amount))
    .bind(currency)
    .bind(status)
    .bind(if paid { Some(Utc::now()) } else { None })
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[allow(clippy::too_many_arguments)]
async fn mdf_request(
    db: &mut PgConnection,
    partner_id: Uuid,
    title: &str,
    activity: &str,
    requested: &str,
    approved: Option<&str>,
    currency: &str,
    status: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO mdf_requests (id, partner_id, title, activity_type, requested_amount, approved_amount, \
           currency, status, is_deleted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false)",
    )
    .bind(id)
    .bind(partner_id)
    .bind(title)
    .bind(activity)
    .bind(dec(requested))
    .bind(approved.map(dec))
    .bind(currency)
    .bind(status)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

async fn mdf_claim(
    db: &mut PgConnection,
    mdf_id: Uuid,
    by: Option<Uuid>,
    amount: &str,
    status: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO mdf_claims (id, mdf_request_id, submitted_by_id, claimed_amount, status) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(mdf_id)
    .bind(by)
    .bind(dec(amount))
    .bind(status)
    .execute(&mut *db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn test_partner_portal_stats_are_per_currency() {
    let app = app(TAG);
    let mut db = app.db().await;
    let p = partner(&mut db, None, Some("0"), Some("0")).await;
    let user = portal_user(p).await;
    commission(&mut db, p, "100", "USD", "paid", true).await;
    commission(&mut db, p, "500000", "JPY", "paid", true).await;
    commission(&mut db, p, "7", "USD", "pending", false).await;
    let usd = mdf_request(
        &mut db,
        p,
        "Expo",
        "event",
        "1000",
        Some("600"),
        "USD",
        "approved",
    )
    .await;
    let jpy = mdf_request(
        &mut db,
        p,
        "Ads",
        "advertising",
        "9000000",
        Some("8000000"),
        "JPY",
        "approved",
    )
    .await;
    mdf_claim(&mut db, usd, Some(user.id), "100", "submitted").await;
    mdf_claim(&mut db, jpy, Some(user.id), "2000000", "submitted").await;
    let comm = app
        .get(&user.token, &format!("{API}/portal/commissions/stats"))
        .await
        .json();
    let mdf = app
        .get(&user.token, &format!("{API}/portal/mdf/stats"))
        .await
        .json();
    // Python compares dicts (key order free); every row of one statement shares created_at, so the
    // first-seen order is the database's.
    let mut comm_ytd: Vec<(String, Value)> = comm["ytd"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    comm_ytd.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        comm_ytd,
        vec![
            ("JPY".to_string(), json!(500000.0)),
            ("USD".to_string(), json!(100.0))
        ]
    );
    assert_eq!(comm["pending"], json!({"USD": 7.0}));
    assert_eq!(comm["currency"], Value::Null);
    assert_eq!(mdf["approved"].as_object().unwrap().len(), 2, "{mdf}");
    assert_eq!(mdf["approved"]["USD"], json!(600.0));
    assert_eq!(mdf["approved"]["JPY"], json!(8000000.0));
    assert_eq!(mdf["claimed"]["USD"], json!(100.0));
    assert_eq!(mdf["claimed"]["JPY"], json!(2000000.0));
    assert_eq!(mdf["currency"], Value::Null);
}

// ─── test_audit_high_b1.py (H26, H27) ────────────────────────────────────────

#[tokio::test]
async fn test_commission_status_transitions_and_paid_counts_once() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let p = partner(&mut db, None, Some("0"), Some("0")).await;
    let comm = commission(&mut db, p, "100.50", "USD", "pending", false).await;
    let url = format!("{API}/commissions/{comm}/status");
    let r = app.patch(&sa.token, &url, json!({"status": "paid"})).await;
    assert_eq!(r.status, 400); // must be approved first
    assert_eq!(
        r.json(),
        json!({"detail": "Cannot move a pending commission to 'paid' (allowed: approved, cancelled)"})
    );
    let r = app.patch(&sa.token, &url, json!({"status": "bogus"})).await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "status must be one of: pending, approved, paid, cancelled"})
    );
    assert_eq!(
        app.patch(&sa.token, &url, json!({"status": "approved"}))
            .await
            .status,
        200
    );
    let r = app.patch(&sa.token, &url, json!({"status": "paid"})).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), json!({"id": comm.to_string(), "status": "paid"}));
    let r = app.patch(&sa.token, &url, json!({"status": "paid"})).await;
    assert_eq!(r.status, 400); // no double count
    assert_eq!(
        r.json(),
        json!({"detail": "Cannot move a paid commission to 'paid'"})
    );
    assert_eq!(
        app.patch(&sa.token, &url, json!({"status": "cancelled"}))
            .await
            .status,
        400
    );
    // M43: a partner's revenue grows when its deal is won, not when a commission is paid.
    assert_eq!(revenue(&mut db, p).await, (dec("0"), dec("0")));
}

/// `_mdf(status, requested, approved)`: a partner, its portal user and one MDF request.
async fn mdf_setup(
    status: &str,
    requested: &str,
    approved: Option<&str>,
) -> (Uuid, support::TestUser, Uuid) {
    let app = app(TAG);
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await;
    let user = portal_user(p).await;
    let mdf = mdf_request(
        &mut db, p, "Expo", "event", requested, approved, "USD", status,
    )
    .await;
    (p, user, mdf)
}

fn form(fields: &[(&str, &str)]) -> reqwest::multipart::Form {
    let mut f = reqwest::multipart::Form::new();
    for (k, v) in fields {
        f = f.text(k.to_string(), v.to_string());
    }
    f
}

#[tokio::test]
async fn test_mdf_claim_takes_form_data_and_stays_within_the_approved_amount() {
    let app = app(TAG);
    let (_, user, mdf) = mdf_setup("approved", "1000", Some("600")).await;
    let url = format!("{API}/portal/mdf/{mdf}/claims");
    let post = |fields: Vec<(&'static str, &'static str)>| {
        let rb = app
            .client()
            .post(app.url(&url))
            .bearer_auth(&user.token)
            .multipart(form(&fields));
        app.send(rb)
    };
    let res = post(vec![("claimed_amount", "400"), ("description", "Booth")]).await;
    assert_eq!(res.status, 201, "{}", res.text());
    let claim = res.json();
    assert_eq!(
        claim.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "id",
            "mdf_request_id",
            "mdf_request_title",
            "claimed_amount",
            "description",
            "status",
            "created_at",
            "paid_at"
        ]
    );
    assert_eq!(claim["claimed_amount"], json!(400.0));
    assert_eq!(claim["description"], "Booth");
    assert_eq!(claim["mdf_request_id"], mdf.to_string());
    assert_eq!(claim["mdf_request_title"], "Expo");
    assert_eq!(claim["status"], "submitted");
    assert!(claim["created_at"].as_str().is_some());
    let over = post(vec![("claimed_amount", "250")]).await; // 400 + 250 > 600
    assert_eq!(over.status, 400);
    assert_eq!(
        over.json(),
        json!({"detail": "Claims would total 650.00 USD, above the approved 600.00 USD (200.00 USD left to claim)"})
    );
    assert_eq!(post(vec![("claimed_amount", "0")]).await.status, 400);
    let missing = post(vec![("description", "x")]).await;
    assert_eq!(missing.status, 422);
    assert_eq!(
        missing.json()["errors"],
        json!([{"field": "claimed_amount", "message": "Field required"}])
    );
    let bad = post(vec![("claimed_amount", "abc")]).await;
    assert_eq!(bad.status, 422);
    assert_eq!(
        bad.json()["errors"],
        json!([{"field": "claimed_amount", "message": "Input should be a valid decimal"}])
    );
    assert_eq!(post(vec![("claimed_amount", "200")]).await.status, 201); // exactly 600
}

#[tokio::test]
async fn test_mdf_claim_review_transitions_keep_paid_at() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let (_, _, mdf) = mdf_setup("approved", "1000", Some("600")).await;
    let mut db = app.db().await;
    let claim = mdf_claim(&mut db, mdf, None, "100", "submitted").await;
    let url = format!("{API}/mdf/claims/{claim}/review");
    let r = app.patch(&sa.token, &url, json!({"status": "paid"})).await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "Cannot move a submitted claim to 'paid' (allowed: approved, rejected)"})
    );
    // the admin page's shape
    assert_eq!(
        app.patch(&sa.token, &url, json!({"action": "approve"}))
            .await
            .status,
        200
    );
    let r = app
        .patch(
            &sa.token,
            &url,
            json!({"status": "paid", "payment_reference": "TRX-1"}),
        )
        .await;
    assert_eq!(r.status, 200);
    let paid_at = || async {
        let mut db = app.db().await;
        let (p, r): (Option<chrono::DateTime<Utc>>, Option<String>) =
            sqlx::query_as("SELECT paid_at, payment_reference FROM mdf_claims WHERE id = $1")
                .bind(claim)
                .fetch_one(&mut db)
                .await
                .unwrap();
        (p, r)
    };
    let (first_paid, reference) = paid_at().await;
    assert!(first_paid.is_some_and(|p| p <= Utc::now()));
    assert_eq!(reference.as_deref(), Some("TRX-1"));
    let r = app.patch(&sa.token, &url, json!({"status": "paid"})).await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "Cannot move a paid claim to 'paid'"})
    );
    assert_eq!(paid_at().await.0, first_paid);
    // No status and no action: `None` is not a move.
    let r = app.patch(&sa.token, &url, json!({})).await;
    assert_eq!(
        r.json(),
        json!({"detail": "Cannot move a paid claim to None"})
    );
}

#[tokio::test]
async fn test_mdf_review_validates_status_and_approved_amount() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let (_, _, mdf) = mdf_setup("submitted", "1000", None).await;
    let url = format!("{API}/mdf/{mdf}/review");
    let r = app.patch(&sa.token, &url, json!({"status": "bogus"})).await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "status must be one of: draft, submitted, under_review, approved, rejected, completed, cancelled"})
    );
    let r = app
        .patch(&sa.token, &url, json!({"status": "approved"}))
        .await;
    assert_eq!(r.status, 400); // no amount
    assert_eq!(
        r.json(),
        json!({"detail": "An approved MDF request needs an approved_amount above zero"})
    );
    assert_eq!(
        app.patch(
            &sa.token,
            &url,
            json!({"status": "approved", "approved_amount": 0})
        )
        .await
        .status,
        400
    );
    let r = app
        .patch(
            &sa.token,
            &url,
            json!({"status": "approved", "approved_amount": 1500}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "approved_amount can't exceed the requested 1000.00 USD"})
    );
    let r = app
        .patch(
            &sa.token,
            &url,
            json!({"status": "approved", "approved_amount": "lots"}),
        )
        .await;
    assert_eq!(
        r.json(),
        json!({"detail": "approved_amount must be a number"})
    );
    let res = app
        .patch(
            &sa.token,
            &url,
            json!({"action": "approve", "approved_amount": 800}),
        )
        .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.json()["status"], "approved");
    let mut db = app.db().await;
    let (amount, reviewer): (Option<Decimal>, Option<Uuid>) =
        sqlx::query_as("SELECT approved_amount, reviewed_by_id FROM mdf_requests WHERE id = $1")
            .bind(mdf)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(amount, Some(dec("800")));
    assert_eq!(reviewer, Some(sa.id));
    // Below what is already claimed.
    mdf_claim(&mut db, mdf, None, "500", "submitted").await;
    let r = app
        .patch(&sa.token, &url, json!({"approved_amount": "400"}))
        .await;
    assert_eq!(
        r.json(),
        json!({"detail": "approved_amount can't be below the 500.00 USD already claimed"})
    );
}

// ─── test_audit_low_w1.py (L64) ──────────────────────────────────────────────

#[tokio::test]
async fn test_partner_portal_commissions_return_created_at_and_rate_fraction() {
    let app = app(TAG);
    let mut db = app.db().await;
    let p = partner(&mut db, None, Some("0"), Some("0")).await;
    let user = portal_user(p).await;
    commission(&mut db, p, "100", "USD", "pending", false).await;
    let rows = app
        .get(&user.token, &format!("{API}/portal/commissions"))
        .await
        .json();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["rate"], json!(0.1));
    assert!(rows[0]["created_at"].as_str().is_some());
    assert_eq!(
        rows[0].as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "id",
            "commission_type",
            "base_amount",
            "rate",
            "amount",
            "currency",
            "status",
            "period_month",
            "period_year",
            "deal_registration_id",
            "paid_at",
            "created_at"
        ]
    );
}

// ─── test_audit_gaps_c.py ────────────────────────────────────────────────────

/// `_partner_and_deal(stage="proposal", **reg_kw)`: an SGD partner already carrying revenue, a USD deal and
/// one registration on it (SGD).
async fn partner_and_deal(db: &mut PgConnection, stage: &str) -> (Uuid, Uuid, Uuid) {
    let p = partner(db, Some("SGD"), Some("500.00"), Some("200.00")).await;
    let deal = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, value, currency, probability, product_interests, product_line_ids, \
           custom, is_deleted) VALUES ($1, $2, $3, 1000, 'USD', 20, '{}', '{}', '{}', false)",
    )
    .bind(deal)
    .bind(u("Deal"))
    .bind(stage)
    .execute(&mut *db)
    .await
    .unwrap();
    let reg = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO partner_deal_registrations (id, partner_id, title, currency, estimated_value, deal_id, \
           registered_at, status, protection_days, products_services) \
         VALUES ($1, $2, 'Reg', 'SGD', 1, $3, now(), 'approved', 90, '{}')",
    )
    .bind(reg)
    .bind(p)
    .bind(deal)
    .execute(&mut *db)
    .await
    .unwrap();
    (p, deal, reg)
}

#[tokio::test]
async fn test_manual_close_records_the_credit() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (_, _, reg) = partner_and_deal(&mut db, "proposal").await;
    let res = app
        .patch(
            &sa.token,
            &format!("{API}/deal-registrations/{reg}/close"),
            json!({"outcome": "won", "final_value": "250"}),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let (credited, cur, before): (Option<Decimal>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT revenue_credited, revenue_credited_currency, status_before_close \
         FROM partner_deal_registrations WHERE id = $1",
        )
        .bind(reg)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(
        (
            credited.map(|d| d.to_string()),
            cur.as_deref(),
            before.as_deref()
        ),
        (Some("250.00".to_string()), Some("SGD"), Some("approved"))
    );
}

/// The services test crm_writes_db.rs `test_deal_unwon_through_the_ai_write_path` (ignored there until the
/// partner functions existed): a deal moved to won and back through crm.writes.apply_change.
#[tokio::test]
async fn test_deal_unwon_through_the_ai_write_path() {
    use sqlx::Connection;
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let (p, deal, reg) = partner_and_deal(&mut db, "proposal").await;
    let user = granary_core::auth::load_active_user(&mut db, sa.id)
        .await
        .unwrap()
        .unwrap();

    async fn stage(
        db: &mut PgConnection,
        user: &granary_core::auth::AuthUser,
        deal: Uuid,
        value: &str,
    ) -> Result<Value, String> {
        // One AsyncSessionLocal() per call: a transaction that the service commits (or that is rolled back
        // when it raises).
        let mut tx = db.begin().await.unwrap();
        let out = granary_services::crm::writes::apply_change(
            &mut tx,
            user,
            "deal",
            "update",
            &json!({"stage": value}),
            Some(deal),
            "mcp",
        )
        .await;
        match out {
            Ok(v) => {
                tx.commit().await.unwrap();
                Ok(v)
            }
            Err(e) => {
                tx.rollback().await.unwrap();
                Err(e.to_string())
            }
        }
    }

    // No USD/SGD rate yet: a clean error and nothing written.
    sqlx::query("DELETE FROM exchange_rates WHERE from_currency = 'USD' AND to_currency = 'SGD'")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM exchange_rates WHERE from_currency = 'SGD' AND to_currency = 'USD'")
        .execute(&mut db)
        .await
        .unwrap();
    let err = stage(&mut db, &user, deal, "won").await.unwrap_err();
    assert!(err.contains("USD/SGD"), "{err}");
    let s: String = sqlx::query_scalar("SELECT stage FROM deals WHERE id = $1")
        .bind(deal)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(s, "proposal");

    let today = Utc::now().date_naive();
    rate(
        &mut db,
        "USD",
        "SGD",
        "1.3456",
        NaiveDate::from_ymd_opt(today.year() - 2, 1, 1).unwrap(),
    )
    .await;
    assert_eq!(
        stage(&mut db, &user, deal, "won").await.unwrap()["ok"],
        true
    );
    let close: Option<NaiveDate> =
        sqlx::query_scalar("SELECT actual_close_date FROM deals WHERE id = $1")
            .bind(deal)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(close.is_some());
    let (status, credited): (String, Option<Decimal>) = sqlx::query_as(
        "SELECT status, revenue_credited FROM partner_deal_registrations WHERE id = $1",
    )
    .bind(reg)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (status.as_str(), credited),
        ("closed_won", Some(dec("1345.60")))
    );
    assert_eq!(revenue(&mut db, p).await, (dec("1845.60"), dec("1545.60")));

    assert_eq!(
        stage(&mut db, &user, deal, "proposal").await.unwrap()["ok"],
        true
    );
    assert_eq!(reg_status(&mut db, reg).await, "approved");
    assert_eq!(revenue(&mut db, p).await, (dec("500.00"), dec("200.00")));
    sqlx::query("DELETE FROM exchange_rates WHERE from_currency = 'USD' AND to_currency = 'SGD'")
        .execute(&mut db)
        .await
        .unwrap();
}

// ─── Routes whose effects need a fake or a row read ──────────────────────────

#[tokio::test]
async fn approving_a_registration_creates_its_deal_and_links_both_sides() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await;
    let reg = registration(&mut db, p, "pending", Some("1234.5"), "EUR").await;
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/deal-registrations/{reg}/review"),
            json!({"action": "approve"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    let deal_id: Uuid = body["deal_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(body["status"], "approved");
    let (title, stage, value, currency, notes, link, owner): (
        String,
        String,
        Option<Decimal>,
        Option<String>,
        Option<String>,
        Option<Uuid>,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT title, stage, value, currency, notes, deal_registration_id, assigned_to_id FROM deals WHERE id = $1",
    )
    .bind(deal_id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (title.as_str(), stage.as_str(), currency.as_deref()),
        ("Deal", "new", Some("EUR"))
    );
    assert_eq!(value, Some(dec("1234.50")));
    assert_eq!(
        notes,
        Some(format!("From partner deal registration {reg}."))
    );
    assert_eq!((link, owner), (Some(reg), Some(sa.id)));
    let (rdeal, approved_by, approved_at): (
        Option<Uuid>,
        Option<Uuid>,
        Option<chrono::DateTime<Utc>>,
    ) = sqlx::query_as(
        "SELECT deal_id, approved_by_id, approved_at FROM partner_deal_registrations WHERE id = $1",
    )
    .bind(reg)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((rdeal, approved_by), (Some(deal_id), Some(sa.id)));
    assert!(approved_at.is_some());
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM deal_stage_events WHERE deal_id = $1")
            .bind(deal_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(events, 1);

    // Linking an existing deal; one already linked elsewhere is refused.
    let reg2 = registration(&mut db, p, "pending", None, "USD").await;
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/deal-registrations/{reg2}/review"),
            json!({"decision": "approved", "deal_id": deal_id.to_string()}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"detail": "That deal is already linked to another partner registration"})
    );
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/deal-registrations/{reg2}/review"),
            json!({"decision": "approved", "deal_id": "nope"}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()),
        (400, json!({"detail": "Invalid deal_id format"}))
    );
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/deal-registrations/{reg2}/review"),
            json!({"decision": "approved", "deal_id": Uuid::new_v4().to_string()}),
        )
        .await;
    assert_eq!(
        (r.status, r.json()),
        (400, json!({"detail": "deal_id: no such deal"}))
    );
    // A rejection keeps the reason and links nothing.
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/deal-registrations/{reg2}/review"),
            json!({"decision": "reject", "reason": "Conflict"}),
        )
        .await;
    assert_eq!(
        r.json(),
        json!({"id": reg2.to_string(), "status": "rejected", "deal_id": null})
    );
    let (reason, rdeal): (Option<String>, Option<Uuid>) = sqlx::query_as(
        "SELECT rejection_reason, deal_id FROM partner_deal_registrations WHERE id = $1",
    )
    .bind(reg2)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((reason.as_deref(), rdeal), (Some("Conflict"), None));
}

#[derive(Clone, Default)]
struct Sent(Arc<Mutex<Vec<granary_services::email::SendCall>>>);

#[tokio::test]
async fn invite_creates_the_portal_user_and_emails_the_temporary_password() {
    use granary_services::email::{set_global_seams, Capture, Seams, SmtpConfig};
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let sent = Sent::default();
    let s2 = sent.clone();
    set_global_seams(Some(Seams {
        send_via_config: Some(Arc::new(move |call| {
            s2.0.lock().unwrap().push(call);
            Box::pin(async { Ok::<(bool, Option<Capture>), _>((true, None)) })
        })),
        load_system_smtp: Some(Arc::new(|| {
            Box::pin(async {
                Ok(SmtpConfig {
                    host: "smtp.test".into(),
                    port: 587,
                    user: "u".into(),
                    password: "p".into(),
                    from_name: "ACME".into(),
                    from_email: "noreply@granary.test".into(),
                    tls: true,
                })
            })
        })),
        load_crm_smtp: None,
    }));
    let email = format!("{}@example.com", u("partner.contact"));
    let created = app
        .post(
            &sa.token,
            &format!("{API}/accounts"),
            json!({"company_name": u("Invite Co"), "contact_name": "Rachel", "contact_email": email}),
        )
        .await;
    assert_eq!(created.status, 201);
    let pid = created.json()["id"].as_str().unwrap().to_string();
    let url = format!("{API}/accounts/{pid}/invite");
    let r = app.post(&sa.token, &url, json!(null)).await;
    let calls = std::mem::take(&mut *sent.0.lock().unwrap());
    set_global_seams(None);
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"email": email, "message": "Invite sent successfully"})
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].to, vec![email.clone()]);
    assert!(calls[0]
        .subject
        .starts_with("Your Partner Portal Access - "));
    assert!(calls[0].html_body.contains("noreply@granary.test"));
    let mut db = app.db().await;
    let (portal, active, must_change, name): (String, bool, Option<bool>, String) = sqlx::query_as(
        "SELECT portal_type, is_active, must_change_password, name FROM users WHERE email = $1",
    )
    .bind(&email)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(
        (portal.as_str(), active, must_change, name.as_str()),
        ("partner", true, Some(true), "Rachel")
    );
    // A second invite while access is live is refused; after a revoke it reactivates the same user.
    let r = app.post(&sa.token, &url, json!(null)).await;
    assert_eq!(
        (r.status, r.json()),
        (
            409,
            json!({"detail": "This partner already has portal access"})
        )
    );
    let r = app
        .delete(&sa.token, &format!("{API}/accounts/{pid}/access"))
        .await;
    assert_eq!(r.status, 204);
    assert!(r.body.is_empty());
    let active: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE email = $1")
        .bind(&email)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert!(!active);
    let r = app.post(&sa.token, &url, json!(null)).await;
    assert_eq!(r.status, 201);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE partner_account_id = $1::uuid AND is_active",
    )
    .bind(&pid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 1);
    // Deleting the partner deactivates its portal users too.
    assert_eq!(
        app.delete(&sa.token, &format!("{API}/accounts/{pid}"))
            .await
            .status,
        204
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE partner_account_id = $1::uuid AND is_active",
    )
    .bind(&pid)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 0);
}

struct FakeStore(Arc<Mutex<Vec<(String, String, Vec<u8>, String)>>>);

type StoreFut<'a, T> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<T, granary_services::storage::StoreError>>
            + Send
            + 'a,
    >,
>;

/// The `#[async_trait]` methods written out (this crate does not depend on async-trait).
impl granary_services::storage::ObjectStore for FakeStore {
    fn bucket_exists<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> StoreFut<'c, bool>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(true) })
    }
    fn make_bucket<'a, 'b, 'c>(&'a self, _bucket: &'b str) -> StoreFut<'c, ()>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async { Ok(()) })
    }
    fn put_object<'a, 'b, 'd, 'e, 'f, 'c>(
        &'a self,
        bucket: &'b str,
        key: &'d str,
        data: &'e [u8],
        content_type: &'f str,
    ) -> StoreFut<'c, ()>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        'e: 'c,
        'f: 'c,
        Self: 'c,
    {
        self.0.lock().unwrap().push((
            bucket.into(),
            key.into(),
            data.to_vec(),
            content_type.into(),
        ));
        Box::pin(async { Ok(()) })
    }
    fn get_object<'a, 'b, 'd, 'c>(&'a self, bucket: &'b str, key: &'d str) -> StoreFut<'c, Vec<u8>>
    where
        'a: 'c,
        'b: 'c,
        'd: 'c,
        Self: 'c,
    {
        let found = self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|(b, k, _, _)| b == bucket && k == key)
            .map(|(_, _, d, _)| d.clone());
        Box::pin(async move {
            found.ok_or_else(|| granary_services::storage::StoreError {
                kind: "NoSuchKey".into(),
                message: "not found".into(),
            })
        })
    }
}

#[tokio::test]
async fn an_mdf_claim_proof_goes_to_minio_under_a_safe_name() {
    let app = app(TAG);
    let _g = app.serial().await;
    let (_, user, mdf) = mdf_setup("approved", "1000", Some("600")).await;
    let puts = Arc::new(Mutex::new(Vec::new()));
    granary_services::storage::set_store(Some(Arc::new(FakeStore(puts.clone()))));
    let part = reqwest::multipart::Part::bytes(b"%PDF-1.4".to_vec())
        .file_name("../my receipt (final).pdf")
        .mime_str("application/pdf")
        .unwrap();
    let f = reqwest::multipart::Form::new()
        .text("claimed_amount", "12.345")
        .part("proof_file", part);
    let rb = app
        .client()
        .post(app.url(&format!("{API}/portal/mdf/{mdf}/claims")))
        .bearer_auth(&user.token)
        .multipart(f);
    let res = app.send(rb).await;
    granary_services::storage::set_store(None);
    assert_eq!(res.status, 201, "{}", res.text());
    // The response is the refreshed row: the stored (rounded) amount.
    assert_eq!(res.json()["claimed_amount"], json!(12.35));
    let puts = puts.lock().unwrap().clone();
    assert_eq!(puts.len(), 1);
    let (bucket, key, data, ctype) = &puts[0];
    assert_eq!(
        bucket,
        &granary_core::config::settings().MINIO_BUCKET_DOCUMENTS
    );
    let prefix = format!("mdf-claims/{mdf}/");
    assert!(key.starts_with(&prefix), "{key}");
    let rest = &key[prefix.len()..];
    assert_eq!(rest.len(), 9 + "my_receipt_final_.pdf".len(), "{rest}");
    assert!(rest.ends_with("-my_receipt_final_.pdf"), "{rest}");
    assert_eq!(
        (data.as_slice(), ctype.as_str()),
        (&b"%PDF-1.4"[..], "application/pdf")
    );
    let mut db = app.db().await;
    let url: Option<String> =
        sqlx::query_scalar("SELECT proof_file_url FROM mdf_claims WHERE mdf_request_id = $1")
            .bind(mdf)
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert_eq!(url.as_deref(), Some(key.as_str()));
}

#[tokio::test]
async fn quiz_scores_issue_one_certificate_and_track_progress() {
    let app = app(TAG);
    // partner.training.max_attempts is global: the attempts test changes it under the same lock.
    let _g = app.serial().await;
    app.set_setting("partner.training.max_attempts", "100")
        .await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await;
    sqlx::query("UPDATE partner_accounts SET tier = 'gold' WHERE id = $1")
        .bind(p)
        .execute(&mut db)
        .await
        .unwrap();
    let user = portal_user(p).await;
    let title = format!("Edge AI {}", u("Basics"));
    let m = app
        .post(
            &sa.token,
            &format!("{API}/training/modules"),
            json!({"title": title, "pass_score": 60}),
        )
        .await;
    assert_eq!(m.status, 201);
    let mid = m.json()["id"].as_str().unwrap().to_string();
    let ch = app
        .post(
            &sa.token,
            &format!("{API}/training/modules/{mid}/chapters"),
            json!({"title": "One", "content": "c", "questions": [
                {"question": "Q1", "options": ["a", "b"], "correct_index": 1},
                {"question": "Q2", "options": ["a", "b"], "correct_index": 0, "order": 2},
                {"question": "Q3", "options": null, "correct_index": 0, "order": 3},
            ]}),
        )
        .await;
    assert_eq!(ch.status, 201, "{}", ch.text());
    assert_eq!(ch.json()["order"], 1);
    let qids: Vec<(Uuid, i32)> = sqlx::query_as(
        "SELECT q.id, q.\"order\" FROM training_quiz_questions q JOIN training_chapters c ON c.id = q.chapter_id \
         WHERE c.module_id = $1::uuid ORDER BY q.\"order\"",
    )
    .bind(&mid)
    .fetch_all(&mut db)
    .await
    .unwrap();
    assert_eq!(qids.len(), 3);
    // Only a published module can be taken (security stage 8, L3).
    sqlx::query("UPDATE training_modules SET is_published = true WHERE id = $1::uuid")
        .bind(&mid)
        .execute(&mut db)
        .await
        .unwrap();
    let quiz = format!("{API}/portal/training/{mid}/submit-quiz");
    // 1 of 3 right: round(33.33) = 33 < 60. A failed attempt shows neither the score nor the number right.
    let r = app
        .post(
            &user.token,
            &quiz,
            json!({"answers": [{"question_id": qids[0].0.to_string(), "selected_index": 1}]}),
        )
        .await;
    assert_eq!(
        r.json(),
        json!({"passed": false, "pass_score": 60, "attempts_left": 99, "certification_issued": false})
    );
    // true and 0.0 are not integers (port findings batch 3): nothing right.
    let r = app
        .post(
            &user.token,
            &quiz,
            json!({"answers": [
                {"question_id": qids[0].0.to_string(), "selected_index": true},
                {"question_id": qids[1].0.to_string(), "selected_index": 0.0},
                {"question_id": 5, "selected_index": 0},
            ]}),
        )
        .await;
    assert_eq!(
        r.json(),
        json!({"passed": false, "pass_score": 60, "attempts_left": 98, "certification_issued": false})
    );
    // 2 of 3, a repeated answer counted once: round(66.67) = 67 >= 60.
    let r = app
        .post(
            &user.token,
            &quiz,
            json!({"answers": [
                {"question_id": qids[0].0.to_string(), "selected_index": 1},
                {"question_id": qids[0].0.to_string(), "selected_index": 1},
                {"question_id": qids[1].0.to_string(), "selected_index": 0},
                {"question_id": 5, "selected_index": 0},
            ]}),
        )
        .await;
    let mut body = r.json();
    // The issued certificate's id and the API path of its PDF (portal_certificate_pdf).
    let cert_id = body["certification_id"].as_str().unwrap().to_string();
    assert_eq!(
        body["cert_url"],
        json!(format!(
            "/api/v1/partner/portal/training/certifications/{cert_id}/pdf"
        ))
    );
    body.as_object_mut().unwrap().remove("certification_id");
    body.as_object_mut().unwrap().remove("cert_url");
    assert_eq!(
        body,
        json!({"score": 67, "passed": true, "correct": 2, "total": 3, "certification_issued": true,
               "attempts_left": 97})
    );
    // Passing again does not issue a second certificate.
    app.post(
        &user.token,
        &quiz,
        json!({"answers": [{"question_id": qids[0].0.to_string(), "selected_index": 1},
                           {"question_id": qids[1].0.to_string(), "selected_index": 0}]}),
    )
    .await;
    let (n, slug): (i64, Option<String>) = sqlx::query_as(
        "SELECT count(*), max(badge_slug) FROM partner_certifications WHERE partner_user_id = $1",
    )
    .bind(user.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 1);
    let expected: String = title
        .to_lowercase()
        .replace(' ', "-")
        .chars()
        .take(50)
        .collect();
    assert_eq!(slug, Some(expected));
    let (status, score, n): (Option<String>, Option<i32>, i64) = sqlx::query_as(
        "SELECT max(status), max(score), count(*) FROM partner_module_progress WHERE partner_user_id = $1",
    )
    .bind(user.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!((status.as_deref(), score, n), (Some("passed"), Some(67), 1));
    // Bad answer shapes are skipped (port findings batch 3); a non-list is a 400.
    let r = app
        .post(&user.token, &quiz, json!({"answers": ["x"]}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["passed"], json!(false));
    let r = app.post(&user.token, &quiz, json!({"answers": "x"})).await;
    assert_eq!(
        (r.status, r.json()),
        (400, json!({"detail": "answers must be a list"}))
    );
    app.set_setting("partner.training.max_attempts", "5").await;
}

/// A module with one question (right answer: 1); returns (module id, question id).
async fn quiz_module(
    db: &mut PgConnection,
    min_tier: &str,
    published: bool,
    deleted: bool,
) -> (Uuid, Uuid) {
    let (mid, cid, qid) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    sqlx::query(
        "INSERT INTO training_modules (id, title, min_tier, partner_types, is_published, pass_score, is_deleted) \
         VALUES ($1, $2, $3, '{}', $4, 100, $5)",
    )
    .bind(mid)
    .bind(u("Module"))
    .bind(min_tier)
    .bind(published)
    .bind(deleted)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO training_chapters (id, module_id, title, content, \"order\") VALUES ($1, $2, 'One', 'c', 1)",
    )
    .bind(cid)
    .bind(mid)
    .execute(&mut *db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO training_quiz_questions (id, chapter_id, question, options, correct_index, \"order\") \
         VALUES ($1, $2, 'Q', '[\"a\", \"b\"]', 1, 1)",
    )
    .bind(qid)
    .bind(cid)
    .execute(&mut *db)
    .await
    .unwrap();
    (mid, qid)
}

/// Security stage 8, L3: get, start and submit enforce the tier and publication rules the list applies.
#[tokio::test]
async fn training_routes_refuse_modules_the_list_hides() {
    let app = app(TAG);
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await; // tier "authorized"
    let user = portal_user(p).await;
    let (gold, _) = quiz_module(&mut db, "gold", true, false).await;
    let (draft, _) = quiz_module(&mut db, "authorized", false, false).await;
    let (gone, _) = quiz_module(&mut db, "authorized", true, true).await;
    let (open, _) = quiz_module(&mut db, "authorized", true, false).await;
    let listed: Vec<String> = app
        .get(&user.token, &format!("{API}/portal/training"))
        .await
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert!(listed.contains(&open.to_string()));
    for m in [gold, draft, gone] {
        assert!(!listed.contains(&m.to_string()));
        let r = app
            .get(&user.token, &format!("{API}/portal/training/{m}"))
            .await;
        assert_eq!(r.status, 404, "get {m}: {}", r.text());
        let r = app
            .post(
                &user.token,
                &format!("{API}/portal/training/{m}/start"),
                json!({}),
            )
            .await;
        assert_eq!(r.status, 404, "start {m}: {}", r.text());
        let r = app
            .post(
                &user.token,
                &format!("{API}/portal/training/{m}/submit-quiz"),
                json!({"answers": []}),
            )
            .await;
        assert_eq!(r.status, 404, "submit {m}: {}", r.text());
    }
    let r = app
        .get(&user.token, &format!("{API}/portal/training/{open}"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM partner_module_progress WHERE partner_user_id = $1",
    )
    .bind(user.id)
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(n, 0, "no progress row on a hidden module");
}

/// Security stage 8, L3: a failed attempt reveals no score anywhere, and attempts are capped by
/// partner.training.max_attempts.
#[tokio::test]
async fn quiz_attempts_are_capped_and_a_fail_hides_the_score() {
    let app = app(TAG);
    let _g = app.serial().await;
    app.set_setting("partner.training.max_attempts", "2").await;
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await;
    let user = portal_user(p).await;
    let (mid, qid) = quiz_module(&mut db, "authorized", true, false).await;
    let quiz = format!("{API}/portal/training/{mid}/submit-quiz");
    let wrong = json!({"answers": [{"question_id": qid.to_string(), "selected_index": 0}]});
    let r = app.post(&user.token, &quiz, wrong.clone()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let body = r.json();
    assert_eq!(body["passed"], json!(false));
    assert_eq!(body["attempts_left"], json!(1));
    for k in ["score", "correct", "total"] {
        assert!(body.get(k).is_none(), "{k} leaked: {body}");
    }
    let detail = app
        .get(&user.token, &format!("{API}/portal/training/{mid}"))
        .await
        .json();
    assert_eq!(detail["progress"]["status"], json!("failed"));
    assert_eq!(detail["progress"]["score"], Value::Null);
    let listed = app
        .get(&user.token, &format!("{API}/portal/training"))
        .await
        .json();
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(mid.to_string()))
        .unwrap()
        .clone();
    assert_eq!(row["progress_score"], Value::Null);
    let r = app.post(&user.token, &quiz, wrong.clone()).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["attempts_left"], json!(0));
    // The third attempt is refused, even with the right answer.
    let right = json!({"answers": [{"question_id": qid.to_string(), "selected_index": 1}]});
    let r = app.post(&user.token, &quiz, right).await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"detail": "You have used all 2 attempts at this quiz. Contact your partner manager to try again."})
    );
    app.set_setting("partner.training.max_attempts", "5").await;
}

/// The MDF review page (partner-mgmt/mdf) reads `partner_name` and each request's `claims` from the admin
/// list: both are on every row (the Partner column, the Review subtitle and the Claims tab were empty).
#[tokio::test]
async fn admin_mdf_list_carries_partner_name_and_claims() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await;
    let name: String =
        sqlx::query_scalar("SELECT company_name FROM partner_accounts WHERE id = $1")
            .bind(p)
            .fetch_one(&mut db)
            .await
            .unwrap();
    let with = mdf_request(
        &mut db,
        p,
        "Webinar",
        "webinar",
        "1000",
        Some("800"),
        "USD",
        "approved",
    )
    .await;
    let without = mdf_request(&mut db, p, "Expo", "event", "500", None, "USD", "submitted").await;
    let claim = mdf_claim(&mut db, with, None, "300", "submitted").await;
    let res = app
        .get(&sa.token, &format!("{API}/mdf?partner_id={p}&limit=200"))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let rows = res.json();
    let row = |id: Uuid| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(id.to_string()))
            .cloned()
            .unwrap()
    };
    let r = row(with);
    assert_eq!(r["partner_name"], json!(name));
    assert_eq!(r["partner_id"], json!(p.to_string()));
    let claims = r["claims"].as_array().unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0]["id"], json!(claim.to_string()));
    assert_eq!(claims[0]["claimed_amount"].as_f64(), Some(300.0));
    assert_eq!(claims[0]["status"], json!("submitted"));
    assert_eq!(claims[0]["proof_file_url"], Value::Null);
    let r = row(without);
    assert_eq!(r["partner_name"], json!(name));
    assert_eq!(r["claims"], json!([]));
}

// ─── Batch E (20 Nov 2026) ───────────────────────────────────────────────────

/// Batch E: the MDF review list links a claim's proof to the authorised download route (never a storage
/// link); the route serves the bytes to MDF reviewers only, as a download.
#[tokio::test]
async fn mdf_claim_proof_is_served_through_an_authorised_route() {
    let app = app(TAG);
    let _g = app.serial().await;
    let sa = app.superadmin().await;
    let (_, user, mdf) = mdf_setup("approved", "1000", Some("600")).await;
    let puts = Arc::new(Mutex::new(Vec::new()));
    granary_services::storage::set_store(Some(Arc::new(FakeStore(puts.clone()))));
    let part = reqwest::multipart::Part::bytes(b"%PDF-1.4 proof".to_vec())
        .file_name("receipt.pdf")
        .mime_str("application/pdf")
        .unwrap();
    let f = reqwest::multipart::Form::new()
        .text("claimed_amount", "10")
        .part("proof_file", part);
    let rb = app
        .client()
        .post(app.url(&format!("{API}/portal/mdf/{mdf}/claims")))
        .bearer_auth(&user.token)
        .multipart(f);
    let res = app.send(rb).await;
    assert_eq!(res.status, 201, "{}", res.text());
    let claim = res.json()["id"].as_str().unwrap().to_string();
    let rows = app
        .get(&sa.token, &format!("{API}/mdf?status=approved&limit=200"))
        .await
        .json();
    let link = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(mdf.to_string()))
        .unwrap()["claims"][0]["proof_file_url"]
        .clone();
    let want = format!("{API}/mdf/claims/{claim}/proof");
    assert_eq!(link, json!(want));
    let got = app.get(&sa.token, &want).await;
    // A partner (or staff without partner_mgmt:manage) cannot fetch it.
    let refused = app.get(&user.token, &want).await;
    let viewer = app
        .make_user(UserSpec::new().perms(&["partner_mgmt:view"]))
        .await;
    let refused_viewer = app.get(&viewer.token, &want).await;
    granary_services::storage::set_store(None);
    assert_eq!(got.status, 200, "{}", got.text());
    assert_eq!(got.text(), "%PDF-1.4 proof");
    let cd = got.header("content-disposition").unwrap_or_default();
    assert!(cd.starts_with("attachment"), "{cd}");
    assert!(cd.contains("receipt.pdf"), "{cd}");
    assert_eq!(refused.status, 403, "{}", refused.text());
    assert_eq!(refused_viewer.status, 403, "{}", refused_viewer.text());
}

/// Batch E: a partner sees the rejection reason of its own rejected registrations, and no other
/// registration carries one.
#[tokio::test]
async fn partner_sees_its_own_rejection_reason() {
    let app = app(TAG);
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await;
    let other = partner(&mut db, None, None, None).await;
    let user = portal_user(p).await;
    let rejected = registration(&mut db, p, "rejected", None, "USD").await;
    let approved = registration(&mut db, p, "approved", None, "USD").await;
    let theirs = registration(&mut db, other, "rejected", None, "USD").await;
    for (id, why) in [
        (rejected, "Already registered by another partner"),
        (approved, "stale reason"),
        (theirs, "not yours"),
    ] {
        sqlx::query("UPDATE partner_deal_registrations SET rejection_reason = $2, notes = 'internal' WHERE id = $1")
            .bind(id)
            .bind(why)
            .execute(&mut db)
            .await
            .unwrap();
    }
    let rows = app
        .get(&user.token, &format!("{API}/portal/deals"))
        .await
        .json();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let row = |id: Uuid| {
        rows.iter()
            .find(|r| r["id"] == json!(id.to_string()))
            .unwrap()
            .clone()
    };
    assert_eq!(
        row(rejected)["rejection_reason"],
        json!("Already registered by another partner")
    );
    assert_eq!(row(approved)["rejection_reason"], Value::Null);
    assert!(rows.iter().all(|r| r.get("notes").is_none()));
}

/// Batch E: one list of tiers, partner types and statuses; create and update refuse values outside it.
#[tokio::test]
async fn partner_options_are_one_list_and_are_enforced() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let opts = app.get(&sa.token, &format!("{API}/options")).await;
    assert_eq!(opts.status, 200, "{}", opts.text());
    let o = opts.json();
    let values = |k: &str| -> Vec<String> {
        o[k].as_array()
            .unwrap()
            .iter()
            .map(|v| v["value"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        values("tiers"),
        ["authorized", "silver", "gold", "platinum"]
    );
    assert!(values("partner_types").contains(&"reseller".to_string()));
    assert!(values("statuses").contains(&"suspended".to_string()));
    // The public registration form reads the same partner types.
    let public = app
        .client()
        .get(app.url("/api/v1/portal/register/partner-types"));
    let public = app.send(public).await;
    assert_eq!(public.status, 200);
    assert_eq!(public.json(), o["partner_types"]);
    // A partner portal account does not read the staff options.
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await;
    let pu = portal_user(p).await;
    assert_eq!(
        app.get(&pu.token, &format!("{API}/options")).await.status,
        403
    );
    let bad = app
        .post(
            &sa.token,
            &format!("{API}/accounts"),
            json!({"company_name": u("P"), "tier": "registered"}),
        )
        .await;
    assert_eq!(bad.status, 400, "{}", bad.text());
    let bad = app
        .post(
            &sa.token,
            "/api/v1/portal/register/partner",
            json!({"company_name": "X", "contact_name": "Y", "email": format!("{}@example.com", u("p")), "partner_type": "pirate"}),
        )
        .await;
    assert_eq!(bad.status, 400, "{}", bad.text());
}

/// Batch E: staff change a partner's tier and status; suspending it ends its portal logins.
#[tokio::test]
async fn partner_tier_and_status_change_and_suspension_revokes_access() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await;
    let pu = portal_user(p).await;
    let listed = app
        .get(&sa.token, &format!("{API}/accounts?limit=500"))
        .await
        .json();
    let mine = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(p.to_string()))
        .unwrap()
        .clone();
    assert_eq!(mine["portal_access"], json!(true));
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/accounts/{p}"),
            json!({"tier": "gold"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["tier"], json!("gold"));
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/accounts/{p}"),
            json!({"tier": "diamond"}),
        )
        .await;
    assert_eq!(r.status, 400);
    let viewer = app
        .make_user(UserSpec::new().perms(&["partner_mgmt:view"]))
        .await;
    let r = app
        .patch(
            &viewer.token,
            &format!("{API}/accounts/{p}"),
            json!({"tier": "platinum"}),
        )
        .await;
    assert_eq!(r.status, 403);
    let r = app
        .patch(
            &sa.token,
            &format!("{API}/accounts/{p}"),
            json!({"status": "suspended"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["status"], json!("suspended"));
    let active: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1")
        .bind(pu.id)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert!(!active);
}

/// Batch E: an approved registration can be extended to a later date or expired; a pending one cannot.
#[tokio::test]
async fn registration_protection_can_be_extended_or_expired() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await;
    let reg = registration(&mut db, p, "approved", None, "USD").await;
    let pending = registration(&mut db, p, "pending", None, "USD").await;
    let url = |id: Uuid| format!("{API}/deal-registrations/{id}/protection");
    let until = (Utc::now() + chrono::Duration::days(400))
        .date_naive()
        .to_string();
    let r = app
        .patch(
            &sa.token,
            &url(reg),
            json!({"action": "extend", "until": until}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["status"], json!("extended"));
    assert_eq!(r.json()["extended_until"], json!(until));
    // Not earlier than the current end.
    let earlier = (Utc::now() + chrono::Duration::days(10))
        .date_naive()
        .to_string();
    let r = app
        .patch(
            &sa.token,
            &url(reg),
            json!({"action": "extend", "until": earlier}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    let r = app
        .patch(&sa.token, &url(reg), json!({"action": "expire"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(reg_status(&mut db, reg).await, "expired");
    let r = app
        .patch(&sa.token, &url(pending), json!({"action": "expire"}))
        .await;
    assert_eq!(r.status, 400);
    let viewer = app
        .make_user(UserSpec::new().perms(&["partner_mgmt:view"]))
        .await;
    let r = app
        .patch(&viewer.token, &url(pending), json!({"action": "expire"}))
        .await;
    assert_eq!(r.status, 403);
}

/// Batch E: staff see each partner user's quiz attempts on a module and can give them back.
#[tokio::test]
async fn staff_can_reset_quiz_attempts() {
    let app = app(TAG);
    let _g = app.serial().await;
    app.set_setting("partner.training.max_attempts", "1").await;
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let p = partner(&mut db, None, None, None).await;
    let user = portal_user(p).await;
    let (mid, qid) = quiz_module(&mut db, "authorized", true, false).await;
    let quiz = format!("{API}/portal/training/{mid}/submit-quiz");
    let wrong = json!({"answers": [{"question_id": qid.to_string(), "selected_index": 0}]});
    assert_eq!(
        app.post(&user.token, &quiz, wrong.clone()).await.status,
        200
    );
    assert_eq!(
        app.post(&user.token, &quiz, wrong.clone()).await.status,
        403
    );
    let progress = app
        .get(&sa.token, &format!("{API}/training/modules/{mid}/progress"))
        .await;
    assert_eq!(progress.status, 200, "{}", progress.text());
    let rows = progress.json();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["attempts"], json!(1));
    assert_eq!(rows[0]["attempts_left"], json!(0));
    let pid = rows[0]["id"].as_str().unwrap().to_string();
    let viewer = app
        .make_user(UserSpec::new().perms(&["partner_mgmt:view"]))
        .await;
    let reset = format!("{API}/training/progress/{pid}/reset");
    assert_eq!(app.post(&viewer.token, &reset, json!({})).await.status, 403);
    let r = app.post(&sa.token, &reset, json!({})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let right = json!({"answers": [{"question_id": qid.to_string(), "selected_index": 1}]});
    let r = app.post(&user.token, &quiz, right).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["passed"], json!(true));
    // A passed module is not reset.
    let r = app.post(&sa.token, &reset, json!({})).await;
    assert_eq!(r.status, 400, "{}", r.text());
    app.set_setting("partner.training.max_attempts", "5").await;
}
