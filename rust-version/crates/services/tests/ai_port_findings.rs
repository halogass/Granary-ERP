//! Port of backend/tests/test_ai_port_findings.py: bugs the Rust port found in the AI services (a record brief
//! and an AI field run that crashed after doing their work, and drafting lookups that treated the user's text
//! as a LIKE pattern and picked an arbitrary row).
//!
//! The model is stubbed through the registry seam (`fake_use_ai`); Python also replaced `get_ai_config` and
//! `feature_enabled`: here the seam's config is the real one with AI and the features switched on. A fresh
//! database per test.
mod support;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use granary_core::auth::AuthUser;
use granary_db::rows::crm_attributes::CRMAttributeDef;
use granary_services::ai::base::{AiError, ChatRequest, ChatResult, LLMProvider};
use granary_services::ai::registry::{self, seam};
use granary_services::ai::{attributes, drafting, records};
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

async fn pool(name: &str) -> PgPool {
    support::fresh_db(&format!("aipf_{name}")).await
}

fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..10].to_string()
}

struct StubProvider {
    answer: Value,
    model_used: Option<&'static str>,
}

#[async_trait]
impl LLMProvider for StubProvider {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat(&self, _model: &str, _req: &ChatRequest) -> Result<ChatResult, AiError> {
        let mut r = ChatResult::text(self.answer.to_string());
        r.prompt_tokens = 7;
        r.completion_tokens = 9;
        r.model_used = self.model_used.unwrap_or("").to_string();
        Ok(r)
    }
}

/// `_stub_ai(monkeypatch, module, answer, model_used=None)`.
async fn stub_ai(
    db: &mut PgConnection,
    answer: Value,
    model_used: Option<&'static str>,
) -> seam::Seam {
    let stub = seam::fake_use_ai(Arc::new(StubProvider { answer, model_used }), "stub-model");
    let s = stub.install().await;
    let mut cfg = registry::get_ai_config(db).await.unwrap();
    for (k, v) in [
        ("ai.api_key", "k"),
        ("ai.provider", "openai"),
        ("ai.enabled", "true"),
        ("ai.feature.scoring", "true"),
        ("ai.feature.crm_ai_fields", "true"),
    ] {
        cfg.insert(k.into(), v.into());
    }
    s.get_ai_config(move || Ok(cfg.clone()));
    s.month_spend(|_| Ok(rust_decimal::Decimal::ZERO));
    s.record_usage(|_| None);
    s
}

/// `make_person(perms=())`.
async fn user(db: &mut PgConnection) -> AuthUser {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, $2, 'Pytest person', 'x', true, false, false, 'internal')",
    )
    .bind(id)
    .bind(format!("{id}@pytest.test"))
    .execute(&mut *db)
    .await
    .unwrap();
    AuthUser {
        id,
        email: format!("{id}@pytest.test"),
        name: "Pytest person".into(),
        phone: None,
        avatar_url: None,
        department: None,
        position: None,
        is_active: true,
        is_superadmin: false,
        is_system_admin: false,
        portal_type: "internal".into(),
        partner_account_id: None,
        vendor_id: None,
        last_login_at: None,
        must_change_password: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        perms: Default::default(),
    }
}

async fn org(db: &mut PgConnection, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO organizations (id, name, is_deleted, custom) VALUES (gen_random_uuid(), $1, false, '{}') RETURNING id",
    )
    .bind(name)
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

async fn contact(db: &mut PgConnection, first: &str, last: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO contacts (id, first_name, last_name, is_deleted, custom) VALUES (gen_random_uuid(), $1, $2, false, '{}') \
         RETURNING id",
    )
    .bind(first)
    .bind(last)
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

async fn product(db: &mut PgConnection, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO product_catalog (id, sku, name, product_type, unit, is_active, is_orderable, pricing_mode, \
         sale_model, price_metric, modules, addon_for, is_whitelabel, is_deleted) \
         VALUES (gen_random_uuid(), $1, $2, 'physical', 'Unit', true, true, 'list', 'one_time', 'unit', '{}', '{}', \
         false, false) RETURNING id",
    )
    .bind(format!("PYT-{}", tag()))
    .bind(name)
    .fetch_one(&mut *db)
    .await
    .unwrap()
}

// ─── record brief ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_a_brief_with_an_answer_is_returned_with_the_model_used() {
    let pool = pool("brief_model").await;
    let mut c = pool.acquire().await.unwrap();
    let _s = stub_ai(
        &mut c,
        json!({"summary": "Warm lead.", "next_best_action": "Call them.", "signals": ["replied"], "risks": []}),
        Some("stub-model-2"),
    )
    .await;
    let o = org(&mut c, &format!("Brief Co {}", tag())).await;
    let u = user(&mut c).await;

    let out = records::build_brief(&mut c, &u, "organization", o)
        .await
        .unwrap();

    assert!(out.get("error").is_none(), "{out}");
    assert_eq!(out["summary"], "Warm lead.");
    assert_eq!(
        out["model"], "stub-model-2",
        "the model the provider says it used"
    );
}

#[tokio::test]
async fn test_a_brief_falls_back_to_the_configured_model() {
    let pool = pool("brief_fallback").await;
    let mut c = pool.acquire().await.unwrap();
    let _s = stub_ai(
        &mut c,
        json!({"summary": "Quiet.", "next_best_action": "Wait."}),
        None,
    )
    .await;
    let o = org(&mut c, &format!("Brief Co {}", tag())).await;
    let u = user(&mut c).await;

    let out = records::build_brief(&mut c, &u, "organization", o)
        .await
        .unwrap();

    assert_eq!(out["model"], "stub-model");
}

// ─── AI field run ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_an_ai_field_run_returns_normally_after_saving() {
    let pool = pool("field_run").await;
    let mut c = pool.acquire().await.unwrap();
    let _s = stub_ai(
        &mut c,
        json!({"value": "Logistics buyer.", "confidence": 77, "reasoning": "notes"}),
        None,
    )
    .await;
    let o = org(&mut c, &format!("Field Co {}", tag())).await;
    let u = user(&mut c).await;
    let key = format!("pyt_{}", &tag()[..8]);
    let defn: CRMAttributeDef = sqlx::query_as(
        "INSERT INTO crm_attribute_defs (id, object_type, key, label, field_type, options, is_ai, ai_kind, ai_config, \
         sort_order, is_active) VALUES (gen_random_uuid(), 'organization', $1, 'Pytest summary', 'long_text', '{}', \
         true, 'summary', '{}', 0, true) RETURNING *",
    )
    .bind(&key)
    .fetch_one(&mut *c)
    .await
    .unwrap();

    let out = attributes::run_attribute(&mut c, &u, &defn, o)
        .await
        .unwrap_or_else(|e| panic!("{}", e.py_str()));

    assert!(out["value"] == "Logistics buyer." && out["confidence"].as_f64() == Some(77.0));
    assert!(
        out["model"] == "stub-model" && out["provider"] == "stub",
        "{out}"
    );

    let custom: Value = sqlx::query_scalar("SELECT custom FROM organizations WHERE id = $1")
        .bind(o)
        .fetch_one(&mut *c)
        .await
        .unwrap();
    let errors: Vec<Option<String>> =
        sqlx::query_scalar("SELECT error FROM crm_ai_field_runs WHERE record_id = $1")
            .bind(o)
            .fetch_all(&mut *c)
            .await
            .unwrap();
    assert_eq!(custom[&key], "Logistics buyer.");
    assert!(errors.len() == 1 && errors[0].is_none());
}

// ─── drafting lookups ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_customer_lookup_treats_wildcards_as_text() {
    let pool = pool("wild").await;
    let mut c = pool.acquire().await.unwrap();
    let t = tag();
    org(&mut c, &format!("Wild{t}X")).await;
    let out = drafting::_resolve_customer(&mut c, &format!("Wild{t}_"))
        .await
        .unwrap();
    assert!(
        out.customer_org_id.is_none(),
        "an underscore is not a one-character wildcard"
    );
    assert!(out.warning.is_some());
}

#[tokio::test]
async fn test_customer_lookup_prefers_the_exact_name() {
    let pool = pool("exact_org").await;
    let mut c = pool.acquire().await.unwrap();
    let t = tag();
    org(&mut c, &format!("Acme {t} Holdings")).await;
    let exact = org(&mut c, &format!("Acme {t}")).await;
    let out = drafting::_resolve_customer(&mut c, &format!("acme {t}"))
        .await
        .unwrap();
    assert_eq!(out.customer_org_id, Some(exact.to_string()));
}

#[tokio::test]
async fn test_contact_lookup_prefers_the_exact_name() {
    let pool = pool("exact_contact").await;
    let mut c = pool.acquire().await.unwrap();
    let t = tag();
    contact(&mut c, &format!("Bob{t}"), Some("Sanders")).await;
    let exact = contact(&mut c, &format!("Bob{t}"), None).await;
    let out = drafting::_resolve_customer(&mut c, &format!("Bob{t}"))
        .await
        .unwrap();
    assert_eq!(out.customer_contact_id, Some(exact.to_string()));
}

#[tokio::test]
async fn test_product_lookup_escapes_and_prefers_the_exact_name() {
    let pool = pool("product").await;
    let mut c = pool.acquire().await.unwrap();
    let t = tag();
    let name = |n: String| -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("name".into(), json!(n));
        m
    };
    product(&mut c, &format!("Cam{t}X")).await;
    assert!(drafting::_find_product(&mut c, &name(format!("Cam{t}_")))
        .await
        .unwrap()
        .is_none());

    product(&mut c, &format!("Box {t} Pro")).await;
    let exact = product(&mut c, &format!("Box {t}")).await;
    let found = drafting::_find_product(&mut c, &name(format!("Box {t}")))
        .await
        .unwrap();
    assert!(found.is_some_and(|p| p.id == exact));
}
