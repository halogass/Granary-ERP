//! crm/writes.py against a database: the service side of backend/tests/test_crm_ai_api.py (Phase 2 preview
//! and apply; the routers wrap WriteError as a 400), test_nary_cards.py, test_nary_p0_stages.py,
//! test_nary_p0_promote.py and test_audit_gaps_c.py.
//!
//! A deal stage move on a deal with a partner registration reaches routers.partner (wave 4): that test stays
//! ignored until then.

mod crm_s16_common;

use crm_s16_common::{db, make_person, u};
use granary_services::crm::writes::{
    self, apply_change, build_diff, check_refs, resolve_refs, validate_change, WriteError,
};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

const TAG: &str = "s16_writes";

async fn org(c: &mut PgConnection, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, name, industry, country, city, type, tags, revenue_currency, is_deleted, custom) \
         VALUES ($1, $2, 'Manufacturing', 'Japan', 'Osaka', '{prospect}', '{}', 'USD', false, '{}')",
    )
    .bind(id)
    .bind(name)
    .execute(&mut *c)
    .await
    .unwrap();
    id
}

async fn lead(c: &mut PgConnection, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO leads (id, title, source, status, is_deleted, custom) VALUES ($1, $2, 'email', 'new', false, '{}')")
        .bind(id)
        .bind(title)
        .execute(&mut *c)
        .await
        .unwrap();
    id
}

async fn attr_key(c: &mut PgConnection) -> String {
    let key = format!("tier_{}", &Uuid::new_v4().simple().to_string()[..8]);
    sqlx::query(
        "INSERT INTO crm_attribute_defs (id, object_type, key, label, field_type, options, is_ai, sort_order, is_active) \
         VALUES ($1, 'organization', $2, 'Tier', 'select', '{\"choices\": [\"Tier 1\", \"Tier 2\"]}', false, 0, true)",
    )
    .bind(Uuid::new_v4())
    .bind(&key)
    .execute(&mut *c)
    .await
    .unwrap();
    key
}

fn invalid<T: std::fmt::Debug>(r: Result<T, WriteError>) -> String {
    match r {
        Err(WriteError::Invalid(m)) => m,
        other => panic!("expected a WriteError, got {other:?}"),
    }
}

// ─── test_crm_ai_api.py ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_preview_shows_before_and_after() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let o = org(&mut c, &u("PyTest Org")).await;
    let body = build_diff(
        &mut c,
        "organization",
        "update",
        &json!({"city": "Tokyo"}),
        Some(o),
        false,
    )
    .await
    .unwrap();
    assert_eq!(body["before"]["city"], "Osaka");
    assert_eq!(body["after"]["city"], "Tokyo");
    assert_eq!(body["record_id"], o.to_string());
}

#[tokio::test]
async fn test_apply_writes_and_audits_with_ai_origin() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let o = org(&mut c, &u("PyTest Org")).await;
    let out = apply_change(
        &mut c,
        &user,
        "organization",
        "update",
        &json!({"city": "Kyoto"}),
        Some(o),
        "ai_confirmed",
    )
    .await
    .unwrap();
    assert_eq!(out["origin"], "ai_confirmed");
    let city: Option<String> = sqlx::query_scalar("SELECT city FROM organizations WHERE id = $1")
        .bind(o)
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!(city.as_deref(), Some("Kyoto"));
    let (changed, old, new): (Option<Vec<String>>, String, String) = sqlx::query_as(
        "SELECT changed_fields, old_values::text, new_values::text FROM audit_logs \
         WHERE entity_id = $1 AND origin = 'ai_confirmed' ORDER BY created_at DESC LIMIT 1",
    )
    .bind(o.to_string())
    .fetch_one(&mut *c)
    .await
    .unwrap();
    assert_eq!(changed, Some(vec!["city".to_string()]));
    assert_eq!(old, r#"{"city": "Osaka"}"#);
    assert_eq!(new, r#"{"city": "Kyoto"}"#);
}

#[tokio::test]
async fn test_apply_can_create_a_record() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let out = apply_change(
        &mut c,
        &user,
        "lead",
        "create",
        &json!({"source": "website_form", "title": u("AI Lead")}),
        None,
        "ai_confirmed",
    )
    .await
    .unwrap();
    assert_eq!(out["action"], "create");
}

/// The same create path for a record that reaches no wave-3 code (not in Python's suite).
#[tokio::test]
async fn test_apply_can_create_an_organization() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, false, &["crm:create_contact"]).await;
    let name = u("AI Org");
    let out = apply_change(
        &mut c,
        &user,
        "organization",
        "create",
        &json!({"name": name, "employee_count": 12}),
        None,
        "mcp",
    )
    .await
    .unwrap();
    assert_eq!(out["action"], "create");
    assert_eq!(out["label"], name.as_str());
    let id = Uuid::parse_str(out["id"].as_str().unwrap()).unwrap();
    let row: (Option<Vec<String>>, Option<String>, Option<Uuid>, Option<Uuid>, Option<i32>, Value) = sqlx::query_as(
        "SELECT type, revenue_currency, created_by_id, assigned_to_id, employee_count, custom FROM organizations WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *c)
    .await
    .unwrap();
    assert_eq!(row.0, Some(vec!["prospect".to_string()]));
    assert_eq!(row.1.as_deref(), Some("USD"));
    assert_eq!((row.2, row.3), (Some(user.id), Some(user.id)));
    assert_eq!(row.4, Some(12));
    assert_eq!(row.5, json!({}));
    let old: String =
        sqlx::query_scalar("SELECT old_values::text FROM audit_logs WHERE entity_id = $1")
            .bind(id.to_string())
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(old, "null"); // Python's None in a JSON column
                             // without the permission
    let viewer = make_person(&mut c, false, &["crm:view_all"]).await;
    assert_eq!(
        invalid(
            apply_change(
                &mut c,
                &viewer,
                "organization",
                "create",
                &json!({"name": "x"}),
                None,
                "mcp"
            )
            .await
        ),
        "Permission required: crm:create_contact"
    );
}

#[tokio::test]
async fn test_apply_rejects_fields_outside_the_allow_list() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let o = org(&mut c, &u("PyTest Org")).await;
    let msg = invalid(
        apply_change(
            &mut c,
            &user,
            "organization",
            "update",
            &json!({"created_by_id": Uuid::new_v4().to_string()}),
            Some(o),
            "ai_confirmed",
        )
        .await,
    );
    assert!(msg.contains("not writable"));
}

#[tokio::test]
async fn test_apply_update_requires_record_id() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let msg = invalid(
        apply_change(
            &mut c,
            &user,
            "organization",
            "update",
            &json!({"city": "X"}),
            None,
            "ai_confirmed",
        )
        .await,
    );
    assert_eq!(msg, "record_id is required for an update");
}

#[tokio::test]
async fn test_apply_validates_custom_values_too() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let o = org(&mut c, &u("PyTest Org")).await;
    let key = attr_key(&mut c).await;
    invalid(
        apply_change(
            &mut c,
            &user,
            "organization",
            "update",
            &json!({"custom": {&key: "Tier 42"}}),
            Some(o),
            "ai_confirmed",
        )
        .await,
    );
    // and a valid value lands in custom
    apply_change(
        &mut c,
        &user,
        "organization",
        "update",
        &json!({"custom": {&key: "Tier 1"}}),
        Some(o),
        "ai_confirmed",
    )
    .await
    .unwrap();
    let custom: Value = sqlx::query_scalar("SELECT custom FROM organizations WHERE id = $1")
        .bind(o)
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!(custom[&key], "Tier 1");
}

/// Owner scoping on update (writes.py; exercised by the MCP tests in Python).
#[tokio::test]
async fn test_a_view_own_user_cannot_edit_someone_elses_record() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let me = make_person(&mut c, false, &["crm:edit_contact", "crm:view_own"]).await;
    let other = make_person(&mut c, false, &[]).await;
    let o = org(&mut c, &u("Theirs")).await;
    sqlx::query("UPDATE organizations SET assigned_to_id = $1 WHERE id = $2")
        .bind(other.id)
        .bind(o)
        .execute(&mut *c)
        .await
        .unwrap();
    assert_eq!(
        invalid(
            apply_change(
                &mut c,
                &me,
                "organization",
                "update",
                &json!({"city": "X"}),
                Some(o),
                "mcp"
            )
            .await
        ),
        "This record is assigned to someone else"
    );
}

/// test_crm_ai_api.py::test_apply_updates_a_value_that_is_already_set. estimated_value / value are Numeric:
/// the old value is a Decimal, and the audit row must still serialise it instead of failing the whole write.
/// Python creates both records through /crm/ai/apply; a lead create reaches wave-3 code (nary.events), so
/// the records are inserted directly here with the same values.
#[tokio::test]
async fn test_apply_updates_a_value_that_is_already_set() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let l = lead(&mut c, &u("Valued Lead")).await;
    sqlx::query("UPDATE leads SET source = 'website_form', estimated_value = 1000 WHERE id = $1")
        .bind(l)
        .execute(&mut *c)
        .await
        .unwrap();
    apply_change(
        &mut c,
        &user,
        "lead",
        "update",
        &json!({"estimated_value": 2500}),
        Some(l),
        "ai_confirmed",
    )
    .await
    .unwrap();

    let deal = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO deals (id, title, stage, value, expected_close_date, is_deleted, custom) \
         VALUES ($1, $2, 'new', 5000, '2026-12-01', false, '{}')",
    )
    .bind(deal)
    .bind(u("Valued Deal"))
    .execute(&mut *c)
    .await
    .unwrap();
    apply_change(
        &mut c,
        &user,
        "deal",
        "update",
        &json!({"value": 7500, "expected_close_date": "2027-01-15"}),
        Some(deal),
        "ai_confirmed",
    )
    .await
    .unwrap();

    let old = |id: Uuid| {
        sqlx::query_scalar::<_, Value>(
            "SELECT old_values FROM audit_logs WHERE entity_id = $1 AND action = 'update'",
        )
        .bind(id.to_string())
    };
    let lead_old = old(l).fetch_one(&mut *c).await.unwrap();
    let deal_old = old(deal).fetch_one(&mut *c).await.unwrap();
    let f = |v: &Value| v.as_str().unwrap().parse::<f64>().unwrap();
    assert_eq!(f(&lead_old["estimated_value"]), 1000.0);
    assert_eq!(f(&deal_old["value"]), 5000.0);
    assert_eq!(deal_old["expected_close_date"], "2026-12-01");
    // str(Decimal) keeps the column's scale
    assert_eq!(lead_old["estimated_value"], "1000.00");
    let value: rust_decimal::Decimal = sqlx::query_scalar("SELECT value FROM deals WHERE id = $1")
        .bind(deal)
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!(value, rust_decimal::Decimal::from(7500));
    // and the preview shows the Decimal as FastAPI does (a float)
    let diff = build_diff(
        &mut c,
        "lead",
        "update",
        &json!({"estimated_value": 3000}),
        Some(l),
        false,
    )
    .await
    .unwrap();
    assert_eq!(diff["before"]["estimated_value"].to_string(), "2500.0");
}

// ─── test_nary_cards.py (the proposal side of _h_propose_record_change) ──────

/// `_h_propose_record_change`'s card: build_diff with refs allowed, plus kind and ref.
async fn propose(
    c: &mut PgConnection,
    object_type: &str,
    fields: Value,
    r#ref: Option<&str>,
) -> Result<Map<String, Value>, String> {
    match build_diff(c, object_type, "create", &fields, None, true).await {
        Ok(Value::Object(diff)) => {
            let mut card = Map::new();
            card.insert("kind".into(), json!("record_change"));
            card.extend(diff);
            if let Some(r) = r#ref {
                card.insert("ref".into(), json!(r));
            }
            Ok(card)
        }
        Ok(_) => unreachable!(),
        Err(WriteError::Invalid(m)) => Err(format!(
            "{m}. Fix the card (crm_field_schema lists the allowed values) and propose it again."
        )),
        Err(WriteError::Api(e)) => panic!("{e}"),
    }
}

#[tokio::test]
async fn test_a_free_text_lead_source_is_refused_when_proposed_with_the_allowed_keys() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let err = propose(
        &mut c,
        "lead",
        json!({"title": "Globex", "source": "Email, hello@"}),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("Unknown lead source 'Email, hello@'") && err.contains("email"),
        "{err}"
    );
    assert!(err.contains("crm_field_schema"));
}

#[tokio::test]
async fn test_every_picklist_and_id_is_checked_before_the_card_is_shown() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let id = Uuid::new_v4().to_string();
    for (ot, fields, needle) in [
        (
            "lead",
            json!({"title": "x", "source": "email", "priority": "super"}),
            "Unknown lead priority",
        ),
        (
            "activity",
            json!({"entity_type": "lead", "entity_id": id, "activity_type": "chat"}),
            "Unknown activity type",
        ),
        (
            "activity",
            json!({"entity_type": "invoice", "entity_id": id, "activity_type": "call"}),
            "Unknown entity_type",
        ),
        (
            "lead",
            json!({"title": "x", "source": "email", "organization_id": id}),
            "no organization with id",
        ),
        (
            "deal",
            json!({"title": "x", "expected_close_date": "next week"}),
            "must be a date",
        ),
    ] {
        let err = propose(&mut c, ot, fields, None).await.unwrap_err();
        assert!(err.contains(needle), "{err}");
    }
}

#[tokio::test]
async fn test_the_card_shows_names_not_ids() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let name = u("Globex Group");
    let o = org(&mut c, &name).await;
    let title = u("Globex: SDK");
    let card = propose(&mut c, "lead", json!({"title": title, "source": "email", "priority": "high", "organization_id": o.to_string()}), None)
        .await
        .unwrap();
    assert_eq!(card["display"]["organization_id"], name.as_str());
    assert_eq!(card["label"], title.as_str()); // a create card has a label too
}

async fn batch(c: &mut PgConnection, tag: &str) -> Vec<Map<String, Value>> {
    let org = propose(
        c,
        "organization",
        json!({"name": format!("Globex Group {tag}")}),
        Some("org"),
    )
    .await
    .unwrap();
    let contact = propose(
        c,
        "contact",
        json!({"first_name": "Adams", "last_name": format!("Turner {tag}"), "organization_id": "@org"}),
        Some("adams"),
    )
    .await
    .unwrap();
    let lead = propose(
        c,
        "lead",
        json!({"title": format!("Globex: SDK {tag}"), "source": "email", "organization_id": "@org", "contact_id": "@adams"}),
        None,
    )
    .await
    .unwrap();
    let mut props: Vec<Map<String, Value>> = vec![];
    for mut p in [org, contact, lead] {
        assert_eq!(check_refs(&mut p, &props), None);
        p.insert(
            "proposal_id".into(),
            json!(Uuid::new_v4().simple().to_string()),
        );
        p.insert("status".into(), json!("pending"));
        props.push(p);
    }
    props
}

#[tokio::test]
async fn test_a_ref_must_point_to_an_earlier_create_card_of_the_right_kind() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let props = batch(&mut c, &u("cards")).await;
    let mut stray = propose(
        &mut c,
        "lead",
        json!({"title": "x", "source": "email", "organization_id": "@nope"}),
        None,
    )
    .await
    .unwrap();
    assert!(check_refs(&mut stray, &props)
        .unwrap()
        .contains("no earlier 'create' card"));
    let mut wrong = propose(
        &mut c,
        "lead",
        json!({"title": "x", "source": "email", "organization_id": "@adams"}),
        None,
    )
    .await
    .unwrap();
    assert!(check_refs(&mut wrong, &props)
        .unwrap()
        .contains("creates a contact, not a organization"));
    assert!(props[2]["display"]["organization_id"]
        .as_str()
        .unwrap()
        .contains("(new, from the card above)"));
    let after = props[2]["after"].as_object().unwrap().clone();
    assert!(invalid(resolve_refs(&after, &props)).contains("needs 'Create organization"));
}

/// The web half of test_on_the_web_each_card_links_to_the_one_confirmed_before_it: routers/crm_records.py
/// resolves the refs against the sibling cards and records each applied card's result on its card.
#[tokio::test]
async fn test_on_the_web_each_card_links_to_the_one_confirmed_before_it_up_to_the_lead() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let tag = u("cards");
    let mut props = batch(&mut c, &tag).await;
    let early = props[1]["after"].as_object().unwrap().clone();
    assert!(invalid(resolve_refs(&early, &props)).contains("needs 'Create organization"));
    for i in 0..2 {
        let after = props[i]["after"].as_object().unwrap().clone();
        let fields = resolve_refs(&after, &props).unwrap();
        let ot = props[i]["object_type"].as_str().unwrap().to_string();
        let out = apply_change(
            &mut c,
            &user,
            &ot,
            "create",
            &Value::Object(fields),
            None,
            "ai_confirmed",
        )
        .await
        .unwrap();
        props[i].insert("status".into(), json!("applied"));
        props[i].insert(
            "result".into(),
            json!({"id": out["id"], "label": out["label"]}),
        );
    }
    let org_id: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE name = $1")
        .bind(format!("Globex Group {tag}"))
        .fetch_one(&mut *c)
        .await
        .unwrap();
    let contact_org: Option<Uuid> =
        sqlx::query_scalar("SELECT organization_id FROM contacts WHERE last_name = $1")
            .bind(format!("Turner {tag}"))
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(contact_org, Some(org_id));
    // the lead card now resolves to both ids (applying it is wave 3: it emits lead.created)
    let lead_after = props[2]["after"].as_object().unwrap().clone();
    let resolved = resolve_refs(&lead_after, &props).unwrap();
    assert_eq!(resolved["organization_id"], org_id.to_string());
}

/// test_on_the_web_each_card_links_to_the_one_confirmed_before_it, service side: what routers/crm_records.py
/// `/crm/ai/apply` does per card (resolve the refs against the sibling cards, apply_change, record the result
/// on the card), for all three cards including the lead (whose create emits lead.created).
#[tokio::test]
async fn test_on_the_web_each_card_links_to_the_one_confirmed_before_it() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let tag = u("cards");
    let mut props = batch(&mut c, &tag).await;
    let early = props[1]["after"].as_object().unwrap().clone();
    assert!(invalid(resolve_refs(&early, &props)).contains("needs 'Create organization"));
    for i in 0..3 {
        let after = props[i]["after"].as_object().unwrap().clone();
        let fields = resolve_refs(&after, &props).unwrap();
        let ot = props[i]["object_type"].as_str().unwrap().to_string();
        let out = apply_change(
            &mut c,
            &user,
            &ot,
            "create",
            &Value::Object(fields),
            None,
            "ai_confirmed",
        )
        .await
        .unwrap();
        props[i].insert("status".into(), json!("applied"));
        props[i].insert(
            "result".into(),
            json!({"id": out["id"], "label": out["label"]}),
        );
    }
    let org_id: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE name = $1")
        .bind(format!("Globex Group {tag}"))
        .fetch_one(&mut *c)
        .await
        .unwrap();
    let (lead_org, lead_contact): (Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT organization_id, contact_id FROM leads WHERE title = $1")
            .bind(format!("Globex: SDK {tag}"))
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(lead_org, Some(org_id));
    assert!(lead_contact.is_some());
}

/// `_conversation(props)`: an AI conversation whose last assistant message carries the cards.
async fn conversation(
    c: &mut PgConnection,
    user: &granary_core::auth::AuthUser,
    props: &[Map<String, Value>],
    tag: &str,
) -> granary_db::rows::ai::AIConversation {
    let conv: granary_db::rows::ai::AIConversation = sqlx::query_as(
        "INSERT INTO ai_conversations (id, user_id, title) VALUES ($1, $2, $3) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(format!("cards {tag}"))
    .fetch_one(&mut *c)
    .await
    .unwrap();
    let cards: Vec<Value> = props.iter().map(|p| Value::Object(p.clone())).collect();
    sqlx::query(
        "INSERT INTO ai_messages (id, conversation_id, role, content, tool_calls) \
         VALUES ($1, $2, 'assistant', 'Three cards.', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(conv.id)
    .bind(json!({"proposals": cards}))
    .execute(&mut *c)
    .await
    .unwrap();
    conv
}

/// The ERP's own API as staff_chat calls it in process (`_as_user`, wave 4's router): only
/// `POST /crm/ai/apply` without a proposal_id, which is routers/crm_records.py `apply_ai_change` with no sibling
/// cards: `resolve_refs(fields, [])`, apply_change(origin="ai_confirmed"), a WriteError as a 400.
struct CrmAiApply {
    pool: sqlx::PgPool,
}

#[async_trait::async_trait]
impl granary_services::nary::staff_chat::seams::ErpApi for CrmAiApply {
    async fn request(
        &self,
        user: &granary_core::auth::AuthUser,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Vec<u8>), granary_services::nary::staff_chat::PyExc> {
        let reply = |status: u16, v: Value| Ok((status, serde_json::to_vec(&v).unwrap()));
        if method != "POST" || path != "/crm/ai/apply" {
            return reply(404, json!({"detail": "Not Found"}));
        }
        let b = body.unwrap_or(Value::Null);
        let mut c = self.pool.acquire().await.unwrap();
        let fields = match resolve_refs(&b["fields"].as_object().cloned().unwrap_or_default(), &[])
        {
            Ok(f) => f,
            Err(WriteError::Invalid(m)) => return reply(400, json!({"detail": m})),
            Err(WriteError::Api(e)) => panic!("{e}"),
        };
        let record_id = b["record_id"].as_str().map(|r| Uuid::parse_str(r).unwrap());
        match apply_change(
            &mut c,
            user,
            b["object_type"].as_str().unwrap_or(""),
            b["action"].as_str().unwrap_or(""),
            &Value::Object(fields),
            record_id,
            "ai_confirmed",
        )
        .await
        {
            Ok(out) => reply(200, out),
            Err(WriteError::Invalid(m)) => reply(400, json!({"detail": m})),
            Err(WriteError::Api(e)) => panic!("{e}"),
        }
    }
}

#[tokio::test]
async fn test_one_two_three_on_whatsapp_creates_all_three_linked() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let tag = u("cards");
    let props = batch(&mut c, &tag).await;
    let conv = conversation(&mut c, &user, &props, &tag).await;
    let api = std::sync::Arc::new(CrmAiApply {
        pool: d.pool.clone(),
    });
    let seams = granary_services::nary::staff_chat::seams::Seams {
        erp: Some(api),
        ..Default::default()
    };
    let out = granary_services::nary::staff_chat::seams::scope(
        seams,
        granary_services::nary::staff_chat::apply_answer(&mut c, &user, &conv, "1 2 3", None),
    )
    .await
    .unwrap();
    assert_eq!(out.matches("Done:").count(), 3, "{out}");
    let org_id: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE name = $1")
        .bind(format!("Globex Group {tag}"))
        .fetch_one(&mut *c)
        .await
        .unwrap();
    let (contact_id, contact_org): (Uuid, Option<Uuid>) =
        sqlx::query_as("SELECT id, organization_id FROM contacts WHERE last_name = $1")
            .bind(format!("Turner {tag}"))
            .fetch_one(&mut *c)
            .await
            .unwrap();
    let (lead_org, lead_contact): (Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT organization_id, contact_id FROM leads WHERE title = $1")
            .bind(format!("Globex: SDK {tag}"))
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(contact_org, Some(org_id));
    assert_eq!(lead_org, Some(org_id));
    assert_eq!(lead_contact, Some(contact_id));
}

#[tokio::test]
async fn test_skipping_the_organisation_leaves_the_cards_that_need_it_undone() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let tag = u("cards");
    let props = batch(&mut c, &tag).await;
    let conv = conversation(&mut c, &user, &props, &tag).await;
    let out = granary_services::nary::staff_chat::apply_answer(&mut c, &user, &conv, "2 3", None)
        .await
        .unwrap();
    assert!(
        out.contains("2. Not done") && out.contains("needs 'Create organization"),
        "{out}"
    );
    let lead: Option<Uuid> = sqlx::query_scalar("SELECT id FROM leads WHERE title = $1")
        .bind(format!("Globex: SDK {tag}"))
        .fetch_optional(&mut *c)
        .await
        .unwrap();
    assert!(lead.is_none());
}

// ─── test_nary_p0_stages.py / test_nary_p0_promote.py ────────────────────────

#[tokio::test]
async fn test_unknown_stages_and_statuses_are_refused_on_every_path() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    assert!(invalid(validate_change(
        "deal",
        "update",
        &json!({"stage": "closed_won"}),
        false
    ))
    .contains("Unknown deal stage"));
    let deal = Uuid::new_v4();
    sqlx::query("INSERT INTO deals (id, title, stage, is_deleted, custom) VALUES ($1, $2, 'proposal', false, '{}')")
        .bind(deal)
        .bind(u("Deal"))
        .execute(&mut *c)
        .await
        .unwrap();
    assert!(invalid(
        apply_change(
            &mut c,
            &user,
            "deal",
            "update",
            &json!({"stage": "closed_won"}),
            Some(deal),
            "mcp"
        )
        .await
    )
    .contains("Unknown deal stage"));
    let l = lead(&mut c, &u("AI lead")).await;
    assert!(invalid(
        apply_change(
            &mut c,
            &user,
            "lead",
            "update",
            &json!({"status": "promoted"}),
            Some(l),
            "mcp"
        )
        .await
    )
    .contains("Promote"));
    assert!(invalid(
        apply_change(
            &mut c,
            &user,
            "lead",
            "update",
            &json!({"status": "disqualified"}),
            Some(l),
            "mcp"
        )
        .await
    )
    .contains("needs at least one reason"));
    let out = apply_change(
        &mut c,
        &user,
        "lead",
        "update",
        &json!({"status": "disqualified", "disqualification_reason": "spam"}),
        Some(l),
        "mcp",
    )
    .await
    .unwrap();
    assert_eq!(out["ok"], true);
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM leads WHERE id = $1")
        .bind(l)
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!(status.as_deref(), Some("disqualified"));
}

#[tokio::test]
async fn test_funnel_fields_are_writable_over_rest_and_ai() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let l = lead(&mut c, &u("Form lead")).await;
    let out = apply_change(
        &mut c,
        &user,
        "lead",
        "update",
        &json!({"interested_products": ["retail_counter"], "attribution": {"utm_source": "linkedin"}}),
        Some(l),
        "mcp",
    )
    .await
    .unwrap();
    assert_eq!(out["ok"], true);
    let (products, attribution): (Option<Vec<String>>, Option<Value>) =
        sqlx::query_as("SELECT interested_products, attribution FROM leads WHERE id = $1")
            .bind(l)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(products, Some(vec!["retail_counter".to_string()]));
    assert_eq!(attribution, Some(json!({"utm_source": "linkedin"})));
}

#[tokio::test]
#[ignore = "covered by crates/api/tests/partner_routes.rs::test_deal_unwon_through_the_ai_write_path (wave 4)"]
async fn test_deal_unwon_through_the_ai_write_path() {}

/// test_nary_chat_fixes.py: a hidden-id (@lid) chat whose display name is the number joins the contact when
/// the contact gets that number through the AI write path (chat_links.link_by_number).
#[tokio::test]
async fn test_a_chat_links_itself_when_the_contact_gets_its_number() {
    let d = db(TAG).await;
    let mut c = d.pool.acquire().await.unwrap();
    let user = make_person(&mut c, true, &[]).await;
    let n = Uuid::new_v4().as_u128();
    let number = format!("8190{:08}", n % 90_000_000 + 10_000_000);
    let lid = format!("{:015}@lid", (n >> 64) % 1_000_000_000_000_000);
    sqlx::query(
        "INSERT INTO wa_chats (id, chat_id, display_name, status, is_my_contact, attribution_asked) \
         VALUES ($1, $2, $3, 'active', false, false)",
    )
    .bind(Uuid::new_v4())
    .bind(&lid)
    .bind(format!("+{number}"))
    .execute(&mut *c)
    .await
    .unwrap();
    let contact = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO contacts (id, first_name, last_name, is_deleted, custom) VALUES ($1, 'Adam', $2, false, '{}')",
    )
    .bind(contact)
    .bind(u("Pytest"))
    .execute(&mut *c)
    .await
    .unwrap();
    apply_change(
        &mut c,
        &user,
        "contact",
        "update",
        &json!({"phone": format!("+{number}")}),
        Some(contact),
        "ai_confirmed",
    )
    .await
    .unwrap();
    let (cid, phone): (Option<Uuid>, Option<String>) =
        sqlx::query_as("SELECT contact_id, phone FROM wa_chats WHERE chat_id = $1")
            .bind(&lid)
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(cid, Some(contact));
    assert_eq!(phone, Some(format!("+{number}")));
}

#[test]
fn writable_lists_match_python() {
    assert!(writes::writable("deal").contains(&"product_line_ids"));
    assert!(!writes::writable("deal").contains(&"created_by_id"));
}
