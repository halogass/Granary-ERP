//! Golden replay of the approval adapters (fixtures first recorded from the retired Python adapters, then
//! moved to the current schema with neutral data; re-record cases.json with GOLDEN_BLESS=1 after an intended
//! change and review the diff).
//!
//! The fixture database (every row of every table, seed included, same ids) is loaded into a fresh
//! database; then for every document the Rust adapter must give the same number, label, revision, expiry,
//! link, check_submittable result, document_facts and snapshot (compared as Python `json.dumps` text, so
//! key order, int/float and float reprs all count), the same engine fingerprint, and every hook must
//! change the same rows the same way.

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use granary_compat::pyjson::{self, DumpOpts, PyValue};
use granary_core::auth::AuthUser;
use granary_core::errors::ApiError;
use granary_services::approvals::adapters::{derive_terms_days, get_adapter, Facts, ADAPTERS};
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

const SKIP_TABLES: &[&str] = &[
    "alembic_version",
    "_sqlx_migrations",
    "rust_schema_migrations",
];

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../../compat-tests/golden/approvals_adapters/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap()
}

async fn tables(conn: &mut PgConnection) -> Vec<String> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables WHERE table_schema='public' \
         AND table_type='BASE TABLE' ORDER BY table_name",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    rows.into_iter()
        .filter(|t| !SKIP_TABLES.contains(&t.as_str()))
        .collect()
}

/// Replace the whole database content with the fixture's rows (FK triggers off while loading).
async fn load_fixture(pool: &PgPool) {
    let db = fixture("db.json");
    let mut conn = pool.acquire().await.unwrap();
    let all = tables(&mut conn).await;
    sqlx::query("SET session_replication_role = replica")
        .execute(&mut *conn)
        .await
        .unwrap();
    let list = all
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(", ");
    sqlx::query(&format!("TRUNCATE {list}"))
        .execute(&mut *conn)
        .await
        .unwrap();
    for entry in db["tables"].as_array().unwrap() {
        let table = entry[0].as_str().unwrap();
        let rows = entry[1].clone();
        let cols: Vec<String> = sqlx::query_scalar(
            "SELECT column_name::text FROM information_schema.columns WHERE table_schema='public' \
             AND table_name=$1 AND is_generated='NEVER' ORDER BY ordinal_position",
        )
        .bind(table)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        // Columns added after the fixture was frozen (e.g. organizations.quote_without_tax, 2 Nov) are
        // not in its rows: leave them to their database default instead of inserting NULL.
        let present = |c: &String| {
            rows.as_array()
                .is_some_and(|rs| rs.is_empty() || rs.iter().any(|r| r.get(c.as_str()).is_some()))
        };
        let cols = cols
            .iter()
            .filter(|c| present(c))
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        sqlx::query(&format!(
            "INSERT INTO \"{table}\" ({cols}) SELECT {cols} FROM jsonb_populate_recordset(NULL::\"{table}\", $1)"
        ))
        .bind(rows)
        .execute(&mut *conn)
        .await
        .unwrap_or_else(|e| panic!("load {table}: {e}"));
    }
    sqlx::query("SET session_replication_role = origin")
        .execute(&mut *conn)
        .await
        .unwrap();
    // The quotation snapshot reads the SKUs that always go to the Owner; the fixture's settings predate it.
    sqlx::query(
        "INSERT INTO system_settings (id, key, value, value_type, category, is_public, is_encrypted) \
         SELECT gen_random_uuid(), 'sales.pricing_rules.owner_approval_skus', '[]', 'json', \
         'sales', false, false WHERE NOT EXISTS (SELECT 1 FROM system_settings \
         WHERE key = 'sales.pricing_rules.owner_approval_skus')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
}

/// json.loads then json.dumps of a row_to_json value: numbers become Python ints / floats.
fn canon(v: &Value) -> Value {
    match v {
        Value::Number(n) => {
            let t = n.to_string();
            if t.contains(['.', 'e', 'E']) {
                let f: f64 = t.parse().unwrap();
                serde_json::from_str(&pyjson::py_float_repr(f)).unwrap()
            } else {
                v.clone()
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), canon(x))).collect()),
        other => other.clone(),
    }
}

async fn dump(conn: &mut PgConnection, tables: &[String]) -> HashMap<String, Vec<Value>> {
    let mut out = HashMap::new();
    for t in tables {
        let rows: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT row_to_json(s14_row)::text FROM \"{t}\" s14_row ORDER BY ctid"
        ))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        out.insert(
            t.clone(),
            rows.iter()
                .map(|r| canon(&serde_json::from_str(r).unwrap()))
                .collect(),
        );
    }
    out
}

fn row_key(row: &Value) -> String {
    match row.get("id") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => pyjson::dumps(&PyValue::from(row), &DumpOpts::sorted()).unwrap(),
    }
}

/// The generator's `diff` (same normalization).
fn diff(
    before: &HashMap<String, Vec<Value>>,
    after: &HashMap<String, Vec<Value>>,
    start: DateTime<Utc>,
    today: &str,
) -> Vec<Value> {
    let mut names: Vec<&String> = after.keys().collect();
    names.sort();
    let mut new_ids: HashMap<String, String> = HashMap::new();
    for t in &names {
        let old: HashSet<String> = before
            .get(*t)
            .map(|r| r.iter().map(row_key).collect())
            .unwrap_or_default();
        for r in &after[*t] {
            if !old.contains(&row_key(r)) {
                if let Some(Value::String(id)) = r.get("id") {
                    let n = new_ids.len();
                    new_ids.insert(id.clone(), format!("<new:{n}>"));
                }
            }
        }
    }
    fn norm(
        v: &Value,
        new_ids: &HashMap<String, String>,
        start: DateTime<Utc>,
        today: &str,
    ) -> Value {
        match v {
            Value::String(s) => {
                if let Some(p) = new_ids.get(s) {
                    return Value::String(p.clone());
                }
                if s == today {
                    return json!("<today>");
                }
                let c: Vec<char> = s.chars().collect();
                if c.len() >= 20 && c[4] == '-' && c[10] == 'T' {
                    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
                        if dt.with_timezone(&Utc) >= start {
                            return json!("<now>");
                        }
                    }
                }
                v.clone()
            }
            Value::Array(a) => {
                Value::Array(a.iter().map(|x| norm(x, new_ids, start, today)).collect())
            }
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), norm(x, new_ids, start, today)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    let mut out = Vec::new();
    for t in &names {
        let empty = Vec::new();
        let old_rows = before.get(*t).unwrap_or(&empty);
        let old: HashMap<String, &Value> = old_rows.iter().map(|r| (row_key(r), r)).collect();
        let mut seen = HashSet::new();
        for r in &after[*t] {
            let k = row_key(r);
            seen.insert(k.clone());
            match old.get(&k) {
                None => out.push(json!({"table": t, "new": norm(r, &new_ids, start, today)})),
                Some(o) if *o != r => {
                    let mut changes = serde_json::Map::new();
                    for (c, v) in r.as_object().unwrap() {
                        if o.get(c) != Some(v) {
                            changes.insert(c.clone(), norm(v, &new_ids, start, today));
                        }
                    }
                    out.push(json!({"table": t, "id": k, "changes": changes}));
                }
                _ => {}
            }
        }
        for r in old_rows {
            let k = row_key(r);
            if !seen.contains(&k) {
                out.push(json!({"table": t, "deleted": k}));
            }
        }
    }
    out
}

fn py(v: &Facts) -> String {
    pyjson::dumps(
        &PyValue::from(&Value::Object(v.clone())),
        &DumpOpts::default(),
    )
    .unwrap()
}

async fn auth_user(conn: &mut PgConnection, id: &str) -> AuthUser {
    let mut u: AuthUser = sqlx::query_as(
        "SELECT id, email, name, phone, avatar_url, department, position, is_active, is_superadmin, \
         is_system_admin, portal_type, partner_account_id, vendor_id, last_login_at, must_change_password, \
         created_at, updated_at FROM users WHERE id = $1",
    )
    .bind(Uuid::parse_str(id).unwrap())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    u.perms = HashSet::new();
    u
}

fn err_json(e: &ApiError) -> Value {
    let body = e.body();
    json!({"status": e.status().as_u16(), "detail": body["detail"]})
}

async fn run_hook(
    pool: &PgPool,
    entity_type: &str,
    id: &str,
    hook: &Value,
    tables: &[String],
    today: &str,
) -> Value {
    let adapter = get_adapter(entity_type).unwrap();
    let start = Utc::now() - chrono::Duration::seconds(1);
    let mut conn = pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    sqlx::query("SET TIME ZONE 'UTC'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let before = dump(&mut tx, tables).await;
    let mut entity = adapter.load(&mut tx, id).await.unwrap().unwrap();
    let name = hook[0].as_str().unwrap();
    let res = match name {
        "on_submitted" => {
            let u = auth_user(&mut tx, hook[1].as_str().unwrap()).await;
            adapter.on_submitted(&mut tx, &mut entity, &u).await
        }
        "on_cancelled" => adapter.on_cancelled(&mut tx, &mut entity).await,
        _ => {
            let actor = match hook[2].as_str() {
                Some(uid) => Some(auth_user(&mut tx, uid).await),
                None => None,
            };
            adapter
                .on_decision(
                    &mut tx,
                    &mut entity,
                    hook[1].as_str().unwrap(),
                    actor.as_ref(),
                    hook[3].as_str(),
                )
                .await
        }
    };
    let out = match res {
        Err(e) => json!({"hook": hook, "error": err_json(&e)}),
        Ok(()) => {
            let after = dump(&mut tx, tables).await;
            json!({"ok": true, "hook": hook, "diff": diff(&before, &after, start, today)})
        }
    };
    tx.rollback().await.unwrap();
    out
}

#[tokio::test]
async fn golden_adapters() {
    let pool = support::fresh_db("s14_adapters").await;
    load_fixture(&pool).await;
    let db = fixture("db.json");
    let today_fixture = db["today"].as_str().unwrap().to_string();
    let mut cases = fixture("cases.json");
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET TIME ZONE 'UTC'")
        .execute(&mut *conn)
        .await
        .unwrap();
    let all = tables(&mut conn).await;
    let today = granary_core::clock::today_local(&mut conn).await.unwrap();
    let today = pyjson::date_isoformat(today);
    drop(conn);
    let mut failures = Vec::new();

    for case in cases["cases"].as_array_mut().unwrap() {
        let name = case["name"].as_str().unwrap().to_string();
        let name = name.as_str();
        let etype = case["entity_type"].as_str().unwrap().to_string();
        let etype = etype.as_str();
        let id = case["id"].as_str().unwrap().to_string();
        let id = id.as_str();
        let adapter = get_adapter(etype).unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let entity = adapter.load(&mut conn, id).await.unwrap().expect(name);
        let check = match adapter.check_submittable(&entity) {
            Ok(()) => Value::Null,
            Err(e) => err_json(&e),
        };
        let mut got = json!({
            "number": adapter.number(&entity),
            "document_label": adapter.document_label(&entity),
            "revision": adapter.revision(&entity),
            "expiry_date": adapter.expiry_date(&entity).map(pyjson::date_isoformat),
            "link": adapter.link_for_id(id),
            "check_submittable": check,
        });
        let facts = adapter.document_facts(&mut conn, &entity).await;
        let snap = adapter.snapshot(&mut conn, &entity).await;
        match (&facts, &snap) {
            (Ok(f), Ok(s)) => {
                got["facts"] = json!(py(f));
                got["snapshot"] = json!(py(s));
                got["fingerprint"] = json!(granary_services::approvals::engine::facts_fingerprint(
                    &Value::Object(f.clone()),
                    &["pre_approved".to_string(), "customer_reference".to_string()],
                ));
            }
            _ => {
                failures.push(format!(
                    "{name}: facts {:?} snapshot {:?}",
                    facts.err(),
                    snap.err()
                ));
                continue;
            }
        }
        for key in [
            "number",
            "document_label",
            "revision",
            "expiry_date",
            "link",
            "check_submittable",
            "facts",
            "snapshot",
            "fingerprint",
        ] {
            if bless::blessing() {
                case[key] = got[key].clone();
            } else if got[key] != case[key] {
                failures.push(format!(
                    "{name}.{key}:\n  python {}\n  rust   {}",
                    case[key], got[key]
                ));
            }
        }
        drop(conn);
        for (i, hook) in case["hooks"].as_array_mut().unwrap().iter_mut().enumerate() {
            let got = run_hook(&pool, etype, id, &hook["hook"], &all, &today).await;
            if bless::blessing() {
                // "today" is kept as written by the recording day
                *hook = serde_json::from_str(
                    &got.to_string()
                        .replace("\"<today>\"", &format!("\"{today_fixture}\"")),
                )
                .unwrap();
                continue;
            }
            let mut want = hook.clone();
            // Dates written as "today" by the Python run were normalized against the recording day.
            if today_fixture != today {
                want = serde_json::from_str(
                    &want
                        .to_string()
                        .replace(&format!("\"{today_fixture}\""), "\"<today>\""),
                )
                .unwrap();
            }
            let want = canon(&want);
            if got != want {
                failures.push(format!(
                    "{name}.hook[{i}]:\n  python {want}\n  rust   {got}"
                ));
            }
        }
    }
    bless::update("approvals_adapters/cases.json", |v| {
        v["cases"] = cases["cases"].clone()
    });
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test]
async fn golden_load_and_registry() {
    let pool = support::fresh_db("s14_adapters_load").await;
    load_fixture(&pool).await;
    let cases = fixture("cases.json");
    let mut conn = pool.acquire().await.unwrap();
    for l in cases["loads"].as_array().unwrap() {
        let adapter = get_adapter(l["entity_type"].as_str().unwrap()).unwrap();
        let e = adapter
            .load(&mut conn, l["raw"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            e.map(|e| e.id().to_string()),
            l["found"].as_str().map(str::to_string),
            "{l}"
        );
    }
    let adapters: Vec<Value> = ADAPTERS
        .iter()
        .map(|a| {
            json!({"entity_type": a.entity_type, "label": a.label, "link_template": a.link_template,
                   "submittable_statuses": a.submittable_statuses.map(|s| s.to_vec())})
        })
        .collect();
    if bless::blessing() {
        bless::update("approvals_adapters/cases.json", |v| {
            v["adapters"] = Value::Array(adapters.clone())
        });
    } else {
        assert_eq!(Value::Array(adapters), cases["adapters"]);
    }
    for u in cases["unknown"].as_array().unwrap() {
        let e = get_adapter(u["entity_type"].as_str().unwrap()).unwrap_err();
        assert_eq!(
            err_json(&e),
            json!({"status": u["status"], "detail": u["detail"]})
        );
    }
}

#[test]
fn golden_derive_terms_days() {
    let cases = fixture("cases.json");
    for t in cases["terms"].as_array().unwrap() {
        let input = &t["in"];
        let got = derive_terms_days(if input.is_null() { None } else { Some(input) });
        assert_eq!(got, t["out"].as_i64(), "{input}");
    }
}

/// test_audit_medium_w3.py::test_single_line_rule_reads_the_largest_line_not_the_total
#[tokio::test]
async fn test_single_line_rule_reads_the_largest_line_not_the_total() {
    let pool = support::fresh_db("s14_adapters_w3").await;
    let mut conn = pool.acquire().await.unwrap();
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) \
         VALUES ($1, 'pytest.w3.s14@example.com', 'Pytest W3', 'x', true, false, false, 'internal') RETURNING id",
    )
    .bind(Uuid::new_v4())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let vendor: Uuid = sqlx::query_scalar(
        "INSERT INTO vendors (id, name, vendor_type, status, is_deleted) VALUES ($1, 'Pytest W3 Supplier', \
         'component_supplier', 'active', false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let po: Uuid = sqlx::query_scalar(
        "INSERT INTO purchase_orders (id, number, vendor_id, status, currency, subtotal, total, requested_by_id, is_deleted) \
         VALUES ($1, 'PYT-PO-s14', $2, 'draft', 'JPY', 7000000, 7000000, $3, false) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(vendor)
    .bind(user)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    for amount in [3_000_000_i64, 4_000_000] {
        sqlx::query(
            "INSERT INTO purchase_order_items (id, po_id, description, quantity_ordered, unit_price, currency, amount) \
             VALUES ($1, $2, 'Part', 1, $3, 'JPY', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(po)
        .bind(rust_decimal::Decimal::from(amount))
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    let adapter = get_adapter("purchase_order").unwrap();
    let entity = adapter
        .load(&mut conn, &po.to_string())
        .await
        .unwrap()
        .unwrap();
    let snap = adapter.snapshot(&mut conn, &entity).await.unwrap();
    assert_eq!(snap["amount_base"].as_f64(), Some(7_000_000.0));
    assert_eq!(snap["max_line_amount_base"].as_f64(), Some(4_000_000.0));
}
