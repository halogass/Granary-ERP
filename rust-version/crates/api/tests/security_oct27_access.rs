//! Security triage #20 (27 Oct 2026): the permission gaps, through the real app.
//!
//! - GET /partner/resources: staff see every resource; a partner portal account sees its own tier and lower
//!   tiers (for every partner type or its own); a vendor portal account is refused.
//! - GET /hr/job-openings: anonymous callers and staff without a recruiting permission see published
//!   (`open`) openings only, whatever `status` says; recruiting staff see every status.
//! - GET /system/dashboard-kpis: portal accounts are refused (company counts are for staff).
//! - Message reactions: reacting to, or listing the reactions of, a message in a private channel the caller
//!   is not in is a 404, as reading the channel is; a member may.

mod support;

use serde_json::{json, Value};
use support::{app, u, UserSpec};
use uuid::Uuid;

const TAG: &str = "sec27";

fn titles(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn partner_resources_follow_the_partners_tier_and_type() {
    let app = app(TAG);
    let sa = app.superadmin().await;
    let mut db = app.db().await;
    let partner = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO partner_accounts (id, company_name, partner_type, tier, status, is_deleted) \
         VALUES ($1, $2, 'reseller', 'silver', 'active', false)",
    )
    .bind(partner)
    .bind(u("Partner"))
    .execute(&mut db)
    .await
    .unwrap();
    let tag = u("res");
    let rows = [
        ("authorized", "{}", "low"),
        ("silver", "{}", "same"),
        ("gold", "{}", "higher"),
        ("platinum", "{}", "top"),
        ("silver", "{reseller}", "mytype"),
        ("authorized", "{distributor}", "othertype"),
    ];
    for (tier, types, name) in rows {
        sqlx::query(
            "INSERT INTO partner_resources (id, title, resource_type, min_tier, partner_types, is_active, \
               download_count) VALUES ($1, $2, 'document', $3, $4::text[], true, 0)",
        )
        .bind(Uuid::new_v4())
        .bind(format!("{tag}-{name}"))
        .bind(tier)
        .bind(types)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let portal = app.make_user(UserSpec::new().portal("partner")).await;
    sqlx::query("UPDATE users SET partner_account_id = $2 WHERE id = $1")
        .bind(portal.id)
        .bind(partner)
        .execute(&mut db)
        .await
        .unwrap();

    let mine = |v: &Value| {
        let mut t: Vec<String> = titles(v)
            .into_iter()
            .filter(|t| t.starts_with(&tag))
            .collect();
        t.sort();
        t
    };
    let r = app.get(&portal.token, "/api/v1/partner/resources").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        mine(&r.json()),
        vec![
            format!("{tag}-low"),
            format!("{tag}-mytype"),
            format!("{tag}-same")
        ]
    );
    // asking for a higher tier does not widen it
    let r = app
        .get(&portal.token, "/api/v1/partner/resources?tier=gold")
        .await;
    assert_eq!(mine(&r.json()), Vec::<String>::new());

    let r = app.get(&sa.token, "/api/v1/partner/resources").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(mine(&r.json()).len(), 6);

    let vendor = app.make_user(UserSpec::new().portal("vendor")).await;
    let r = app.get(&vendor.token, "/api/v1/partner/resources").await;
    assert_eq!(r.status, 403, "{}", r.text());
}

#[tokio::test]
async fn only_published_job_openings_are_public() {
    let app = app(TAG);
    let mut db = app.db().await;
    let tag = u("job");
    for status in ["open", "draft", "paused", "filled", "cancelled"] {
        sqlx::query(
            "INSERT INTO job_openings (id, title, status, is_deleted, show_salary, is_remote) \
             VALUES ($1, $2, $3, false, false, false)",
        )
        .bind(Uuid::new_v4())
        .bind(format!("{tag}-{status}"))
        .bind(status)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let mine = |v: &Value| {
        let mut t: Vec<String> = titles(v)
            .into_iter()
            .filter(|t| t.starts_with(&tag))
            .collect();
        t.sort();
        t
    };
    let staff = app.user_with(&[]).await;
    let recruiter = app.user_with(&["hr:manage_job_openings"]).await;
    for (token, status) in [
        ("", "all"),
        ("", "draft"),
        ("", ""),
        (staff.token.as_str(), "all"),
    ] {
        let r = app
            .get(token, &format!("/api/v1/hr/job-openings?status={status}"))
            .await;
        assert_eq!(r.status, 200, "{}", r.text());
        assert_eq!(mine(&r.json()), vec![format!("{tag}-open")], "{status}");
    }
    let r = app
        .get(&recruiter.token, "/api/v1/hr/job-openings?status=all")
        .await;
    assert_eq!(mine(&r.json()).len(), 5);
    let r = app
        .get(&recruiter.token, "/api/v1/hr/job-openings?status=draft")
        .await;
    assert_eq!(mine(&r.json()), vec![format!("{tag}-draft")]);
}

#[tokio::test]
async fn portal_accounts_get_no_dashboard_counts() {
    let app = app(TAG);
    for portal in ["partner", "vendor"] {
        let user = app.make_user(UserSpec::new().portal(portal)).await;
        let r = app.get(&user.token, "/api/v1/system/dashboard-kpis").await;
        assert_eq!(r.status, 403, "{portal}: {}", r.text());
    }
    let staff = app.user_with(&[]).await;
    let r = app.get(&staff.token, "/api/v1/system/dashboard-kpis").await;
    assert_eq!(r.status, 200, "{}", r.text());
}

#[tokio::test]
async fn reactions_need_the_channel_membership() {
    let app = app(TAG);
    let member = app.user_with(&[]).await;
    let outsider = app.user_with(&[]).await;
    let mut db = app.db().await;
    let channel = Uuid::new_v4();
    let slug = u("priv");
    sqlx::query(
        "INSERT INTO channels (id, name, slug, type, created_by_id) VALUES ($1, $2, $2, 'private', $3)",
    )
    .bind(channel)
    .bind(&slug)
    .bind(member.id)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO channel_memberships (id, channel_id, user_id) VALUES ($1, $2, $3)")
        .bind(Uuid::new_v4())
        .bind(channel)
        .bind(member.id)
        .execute(&mut db)
        .await
        .unwrap();
    let msg = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channel_messages (id, channel_id, sender_id, content, is_deleted) \
         VALUES ($1, $2, $3, 'secret plans', false)",
    )
    .bind(msg)
    .bind(channel)
    .bind(member.id)
    .execute(&mut db)
    .await
    .unwrap();

    let react = format!("/api/v1/messaging/messages/{msg}/react?source=channel");
    let list = format!("/api/v1/messaging/messages/{msg}/reactions?source=channel");
    let r = app
        .post(&outsider.token, &react, json!({"emoji": "\u{1F44D}"}))
        .await;
    assert_eq!(r.status, 404, "{}", r.text());
    let r = app.get(&outsider.token, &list).await;
    assert_eq!(r.status, 404, "{}", r.text());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM message_reactions WHERE message_id = $1")
        .bind(msg.to_string())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(n, 0);

    let r = app
        .post(&member.token, &react, json!({"emoji": "\u{1F44D}"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = app.get(&member.token, &list).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()[0]["count"], json!(1));
}
