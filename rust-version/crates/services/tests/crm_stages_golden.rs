//! services/crm/stages.py, promote.py, deal_hooks.py and access.py: pure goldens, settings-backed goldens on
//! the scenario database, and the recorded effects of change_deal_stage / promote_lead (rows written).
#[path = "golden/bless.rs"]
mod bless;
mod crm_s15_common;
mod support;

use chrono::NaiveDate;
use granary_core::errors::ApiError;
use granary_services::crm::access::assert_can_view;
use granary_services::crm::stages::{self as st, StageFail};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

fn arg(v: &Value) -> Option<&str> {
    v.as_str()
}

#[test]
fn pure_golden() {
    for c in crm_s15_common::load("crm_stages/pure.json")
        .as_array()
        .unwrap()
    {
        let f = c["fn"].as_str().unwrap();
        let a = c["args"].as_array().unwrap();
        let got: Result<Value, String> = match f {
            "validate_deal_stage" => st::validate_deal_stage(arg(&a[0]))
                .map(Value::from)
                .map_err(|e| e.0),
            "validate_lead_status" => st::validate_lead_status(arg(&a[0]), false)
                .map(Value::from)
                .map_err(|e| e.0),
            "validate_lead_status_create" => st::validate_lead_status(arg(&a[0]), true)
                .map(Value::from)
                .map_err(|e| e.0),
            "is_won" => Ok(json!(st::is_won(arg(&a[0])))),
            "is_closed" => Ok(json!(st::is_closed(arg(&a[0])))),
            "deal_stage" => Ok(st::deal_stage(arg(&a[0]))
                .map(|s| json!(s.key))
                .unwrap_or(Value::Null)),
            "validate_lead_transition" => {
                st::validate_lead_transition(arg(&a[0]), a[1].as_str().unwrap())
                    .map(Value::from)
                    .map_err(|e| e.0)
            }
            "consts" => {
                let tr: serde_json::Map<String, Value> = st::LEAD_TRANSITIONS
                    .iter()
                    .map(|(k, v)| (k.to_string(), json!(v)))
                    .collect();
                Ok(json!({
                    "WON_STAGES": st::WON_STAGES, "CLOSED_STAGES": st::CLOSED_STAGES,
                    "ACTIVE_STAGES": st::ACTIVE_STAGES, "ACTIVE_LEAD_STATUSES": st::ACTIVE_LEAD_STATUSES,
                    "CLOSED_LEAD_STATUSES": st::CLOSED_LEAD_STATUSES, "LEAD_TRANSITIONS": tr,
                }))
            }
            other => panic!("{other}"),
        };
        match (got, c.get("ok"), c.get("err")) {
            (Ok(v), Some(exp), _) => assert_eq!(&v, exp, "{c}"),
            (Err(m), _, Some(exp)) => assert_eq!(m, exp.as_str().unwrap(), "{c}"),
            (g, ..) => panic!("{c}: {g:?}"),
        }
    }
}

fn stage_outcome(r: Result<Value, StageFail>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(StageFail::Stage(e)) => json!({"err": e.0}),
        Err(StageFail::Api(e @ ApiError::Http { .. })) => {
            let (s, d) = crm_s15_common::http_of(&e);
            json!({"http": [s, d]})
        }
        Err(StageFail::Api(_)) => json!({"exc": true}),
    }
}

async fn db_case(db: &mut PgConnection, name: &str) -> Result<Value, StageFail> {
    let (f, rest) = name.split_once(':').unwrap_or((name, ""));
    let to_v = |r: Result<Vec<Value>, StageFail>| r.map(Value::Array);
    match f {
        "registry" | "registry_bad_probs" | "registry_empty_probs" => st::registry(db).await,
        "probability_for" => st::probability_for(db, rest).await.map(Value::from),
        "probability_for_value"
        | "probability_for_list"
        | "probability_for_unset"
        | "probability_for_badjson" => st::probability_for(db, "new").await.map(Value::from),
        "validate_reasons" => {
            let (_, r) = rest.split_once(':').unwrap();
            let r: Value = serde_json::from_str(r).unwrap();
            to_v(st::validate_reasons(db, "lost", Some(&r)).await)
        }
        "validate_disqualify_reason" => {
            st::validate_disqualify_reason(db, Some(&json!(["spam", "duplicate"]))).await
        }
        "reason_picklist_bad" | "reason_picklist_obj" => {
            to_v(st::reason_picklist(db, "crm.funnel.loss_reasons").await)
        }
        "validate_lead_source" => {
            let s = if rest == "None" { None } else { Some(rest) };
            st::validate_lead_source(db, s).await.map(Value::from)
        }
        "validate_lead_source_bad" => st::validate_lead_source(db, Some("x"))
            .await
            .map(Value::from),
        "validate_lead_priority" => {
            let s = if rest == "None" { None } else { Some(rest) };
            st::validate_lead_priority(db, s).await.map(Value::from)
        }
        "validate_activity" => {
            let fields: Value = serde_json::from_str(rest).unwrap();
            st::validate_activity(db, fields.as_object().unwrap())
                .await
                .map(|_| Value::Null)
        }
        "default_lead_owner_id" => st::default_lead_owner_id(db)
            .await
            .map(|u| json!(u.to_string())),
        other => panic!("{other}"),
    }
}

#[tokio::test]
async fn settings_golden_and_access() {
    let pool = support::fresh_db("s15_stages").await;
    crm_s15_common::scenario_db(&pool).await;
    let mut bad = Vec::new();
    let mut cases = crm_s15_common::load("crm_stages/db.json");
    for c in cases.as_array_mut().unwrap() {
        let mut tx = pool.begin().await.unwrap();
        for sql in c["setup"].as_array().unwrap() {
            sqlx::query(sql.as_str().unwrap())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        let name = c["name"].as_str().unwrap().to_string();
        let name = name.as_str();
        let got = stage_outcome(db_case(&mut tx, name).await);
        if bless::blessing() && name != "default_lead_owner_id:seed" {
            let o = c.as_object_mut().unwrap();
            o.retain(|k, _| k == "name" || k == "setup");
            for (k, v) in got.as_object().unwrap() {
                o.insert(k.clone(), v.clone());
            }
            continue;
        }
        let mut exp = c.clone();
        let exp = exp.as_object_mut().unwrap();
        exp.remove("name");
        exp.remove("setup");
        let mut exp = Value::Object(exp.clone());
        if name == "default_lead_owner_id:seed" {
            // the seed's superadmin id is random: compare with this database's row (the user the
            // crm.funnel.default_lead_owner_email setting names)
            let id: Uuid = sqlx::query_scalar(
                "select id from users where email = (select value from system_settings \
                 where key = 'crm.funnel.default_lead_owner_email')",
            )
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            exp = json!({"ok": id.to_string()});
        }
        let same = if exp.get("exc").is_some() {
            got.get("exc").is_some()
        } else {
            exp == got
        };
        if !same {
            bad.push(format!("{name}\n exp {exp}\n got {got}"));
        }
    }
    bless::save("crm_stages/db.json", &cases);
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad.join("\n")
    );

    // access.assert_can_view
    let a1 = Uuid::parse_str("00000000-0000-0000-0000-0000000000a1").unwrap();
    let a2 = Uuid::parse_str("00000000-0000-0000-0000-0000000000a2").unwrap();
    let own = crm_s15_common::named_user("own_a1");
    assert!(assert_can_view(&own, Some(a1)).is_ok());
    assert!(assert_can_view(&own, None).is_ok());
    let e = assert_can_view(&own, Some(a2)).unwrap_err();
    assert_eq!(
        crm_s15_common::http_of(&e),
        (403, json!("This record is assigned to someone else"))
    );
    let e = assert_can_view(&crm_s15_common::named_user("none_a2"), Some(a2)).unwrap_err();
    assert_eq!(
        crm_s15_common::http_of(&e),
        (403, json!("Permission required: crm:view_own"))
    );
    assert!(assert_can_view(&crm_s15_common::named_user("all_a2"), Some(a1)).is_ok());
    assert!(assert_can_view(&crm_s15_common::named_user("super"), Some(a2)).is_ok());
}

/// The generator's `snapshot()`.
async fn snapshot(
    db: &mut PgConnection,
    deal_ids: &[String],
    lead_ids: &[String],
    today: NaiveDate,
) -> Value {
    async fn q(db: &mut PgConnection, sql: &str, ids: &[String], lids: &[String]) -> Vec<Value> {
        let sql = format!("select coalesce(json_agg(t), '[]'::json) from ({sql}) t");
        let v: Value = sqlx::query_scalar(&sql)
            .bind(ids)
            .bind(lids)
            .fetch_one(db)
            .await
            .unwrap();
        v.as_array().unwrap().clone()
    }
    let deals = q(db, "select id::text, title, stage, value::text as value, currency, probability, actual_close_date, \
        loss_reason, loss_reasons::text as loss_reasons, loss_notes, product_interests, product_line_ids, notes, \
        lead_id::text, contact_id::text, organization_id::text, assigned_to_id::text, created_by_id::text, \
        custom::text, deal_number, expected_close_date from deals where id::text = any($1) or lead_id::text = any($2) \
        order by title", deal_ids, lead_ids).await;
    let events = q(db, "select e.deal_id::text, from_stage, to_stage, by_id::text, origin, reasons::text as reasons, \
        note from deal_stage_events e join deals d on d.id = e.deal_id where d.id::text = any($1) \
        or d.lead_id::text = any($2) order by d.title, e.to_stage", deal_ids, lead_ids).await;
    let dom = q(db, "select type, entity_type, entity_id::text, actor_id::text, payload::text from domain_events \
        where $1::text[] is not null and $2::text[] is not null order by type, payload::text", deal_ids, lead_ids).await;
    let rem = q(db, "select owner_user_id::text, created_by_id::text, source, entity_type, entity_id::text, title, \
        note, to_char(due_at at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"+00:00\"') as due_at, \
        to_char(original_due_at at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"+00:00\"') as original_due_at, \
        status, rule_key from crm_reminders where $1::text[] is not null and $2::text[] is not null order by title",
        deal_ids, lead_ids).await;
    let leads = q(db, "select id::text, status, converted_to_deal_id is not null as converted, \
        converted_at is not null as has_converted_at from leads where id::text = any($2) and $1::text[] is not null \
        order by id", deal_ids, lead_ids).await;
    let snap = json!({"deals": deals, "events": events, "domain_events": dom, "reminders": rem, "leads": leads});
    let mut txt = snap.to_string().replace(&today.to_string(), "{today}");
    for m in 1..25 {
        let d = granary_compat::text::relativedelta::add_months(today, m).unwrap();
        txt = txt.replace(&d.to_string(), &format!("{{today+{m}m}}"));
    }
    let re =
        regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap();
    let txt = re.replace_all(&txt, |c: &regex::Captures| {
        if c[0].starts_with("00000000-0000-0000-0000-") {
            c[0].to_string()
        } else {
            "{new}".to_string()
        }
    });
    serde_json::from_str(&txt).unwrap()
}

/// Python's json for the snapshot normalises a few column renderings the same way (value via str(Decimal),
/// JSONB as text); compare on parsed JSON so key order inside objects does not matter.
fn norm(v: &Value) -> Value {
    v.clone()
}

#[tokio::test]
async fn stage_changes_golden() {
    let pool = support::fresh_db("s15_changes").await;
    crm_s15_common::scenario_db(&pool).await;
    let mut bad = Vec::new();
    for c in crm_s15_common::load("crm_stages/changes.json")
        .as_array()
        .unwrap()
    {
        let mut tx = pool.begin().await.unwrap();
        let today = granary_core::clock::today_local(&mut tx).await.unwrap();
        let name = c["name"].as_str().unwrap();
        let (res, deal_ids, lead_ids) = if let Some(deal) = c.get("deal") {
            let id = Uuid::parse_str(deal.as_str().unwrap()).unwrap();
            let mut d: granary_db::rows::crm::Deal =
                sqlx::query_as("select * from deals where id = $1")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            let cd = c
                .get("close_date")
                .and_then(|v| v.as_str())
                .map(|s| s.parse::<NaiveDate>().unwrap());
            let by = c["by"].as_str().map(|s| Uuid::parse_str(s).unwrap());
            let r = st::change_deal_stage(
                &mut tx,
                &mut d,
                c["to"].as_str().unwrap(),
                by,
                c["origin"].as_str().unwrap(),
                c.get("reasons").filter(|v| !v.is_null()),
                c.get("note").and_then(|v| v.as_str()),
                cd,
            )
            .await;
            let res = match r {
                Ok(b) => json!({"ok": b}),
                Err(StageFail::Stage(e)) => json!({"err": e.0}),
                Err(StageFail::Api(e)) => json!({"api": e.to_string()}),
            };
            (res, vec![deal.as_str().unwrap().to_string()], vec![])
        } else {
            let lead = Uuid::parse_str(c["lead"].as_str().unwrap()).unwrap();
            let user = crm_s15_common::named_user(c["user"].as_str().unwrap());
            let origin = c
                .get("origin")
                .and_then(|v| v.as_str())
                .unwrap_or("promote");
            let mut r =
                granary_services::crm::promote::promote_lead(&mut tx, lead, &user, origin).await;
            if c.get("twice").is_some() && r.is_ok() {
                r = granary_services::crm::promote::promote_lead(&mut tx, lead, &user, "promote")
                    .await;
            }
            let res = match r {
                Ok(v) => json!({"ok": {"created": v["created"], "lead_id": v["lead_id"]}}),
                Err(e) => {
                    let (s, d) = crm_s15_common::http_of(&e);
                    json!({"http": [s, d]})
                }
            };
            (res, vec![], vec![lead.to_string()])
        };
        let snap = snapshot(&mut tx, &deal_ids, &lead_ids, today).await;
        let exp_res = ["ok", "err", "http"]
            .iter()
            .find_map(|k| c.get(*k).map(|v| json!({*k: v})))
            .unwrap();
        if exp_res != res || norm(&c["after"]) != norm(&snap) {
            bad.push(format!(
                "{name}\n exp {exp_res} {}\n got {res} {snap}",
                c["after"]
            ));
        }
        tx.rollback().await.unwrap();
    }
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
