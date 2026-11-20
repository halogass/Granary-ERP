//! Golden replay of compat-tests/golden/approvals_engine/scenarios.json (first recorded from the retired
//! Python engine): the same fixture SQL and engine calls, in the same order, on a fresh database; every result,
//! queued outbox item and written row must match after normalisation (ids -> labels, timestamps and hashes
//! masked). Re-record the results with GOLDEN_BLESS=1 after an intended change and review the diff.

#[path = "golden/bless.rs"]
mod bless;
mod support;

use std::collections::HashMap;

use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
use granary_core::auth::{load_active_user, AuthUser};
use granary_core::errors::ApiError;
use granary_services::approvals::{engine, notify};
use regex::Regex;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

fn fixture() -> Value {
    let path = format!(
        "{}/../../compat-tests/golden/approvals_engine/scenarios.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

struct Run {
    pool: PgPool,
    ids: HashMap<String, String>,
}

fn s<'a>(op: &'a Value, k: &str) -> &'a str {
    op[k].as_str().unwrap_or_else(|| panic!("{k} in {op}"))
}

fn f(op: &Value, k: &str) -> f64 {
    op[k].as_f64().unwrap()
}

fn outbox_json() -> Value {
    json!(notify::pending_outbox()
        .into_iter()
        .map(
            |i| json!({"user_ids": i.user_ids, "title": i.title, "body": i.body, "link": i.link,
                         "email": i.email, "whatsapp": i.whatsapp})
        )
        .collect::<Vec<_>>())
}

fn failure(e: &ApiError) -> Map<String, Value> {
    let mut m = Map::new();
    match e {
        ApiError::Http { status, detail, .. } => {
            m.insert("status".into(), json!(status.as_u16()));
            m.insert("detail".into(), detail.clone());
        }
        ApiError::App { .. } => {
            m.insert("exception".into(), json!("AppException"));
        }
        other => {
            m.insert("exception".into(), json!(format!("{other}")));
        }
    }
    m
}

fn with_outbox(mut m: Map<String, Value>, ob: Value) -> Value {
    m.insert("outbox".into(), ob);
    Value::Object(m)
}

impl Run {
    fn id(&self, key: &str) -> String {
        self.ids
            .get(key)
            .unwrap_or_else(|| panic!("unknown key {key}"))
            .clone()
    }
    fn uuid(&self, key: &str) -> Uuid {
        Uuid::parse_str(&self.id(key)).unwrap()
    }

    async fn exec(&self, sql: &str, binds: Vec<Value>) {
        let mut q = sqlx::query(sql);
        for b in binds {
            q = match b {
                Value::Null => q.bind(None::<String>),
                Value::Bool(x) => q.bind(x),
                Value::Number(n) if n.is_i64() => q.bind(n.as_i64().unwrap()),
                Value::Number(n) => q.bind(n.as_f64().unwrap()),
                Value::String(x) => match Uuid::parse_str(&x) {
                    Ok(u) => q.bind(u),
                    Err(_) => q.bind(x),
                },
                other => q.bind(other),
            };
        }
        q.execute(&self.pool)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    async fn role_id(db: &mut PgConnection, slug: &str) -> Uuid {
        if let Some(id) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM roles WHERE slug = $1")
            .bind(slug)
            .fetch_optional(&mut *db)
            .await
            .unwrap()
        {
            return id;
        }
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO roles (id, name, slug, is_system_role) VALUES ($1, $2, $2, true)")
            .bind(id)
            .bind(slug)
            .execute(&mut *db)
            .await
            .unwrap();
        id
    }

    async fn fixture(&mut self, op: &Value) {
        let new = || Uuid::new_v4().to_string();
        match s(op, "op") {
            "person" => {
                let key = s(op, "key");
                let (uid, eid) = (Uuid::new_v4(), Uuid::new_v4());
                let name = format!("Golden {key}");
                let email = format!("golden.{key}@example.com");
                let manager = op["manager"]
                    .as_str()
                    .map(|m| self.uuid(&format!("{m}:employee")));
                let mut tx = self.pool.begin().await.unwrap();
                sqlx::query(
                    "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, \
                     portal_type, email_signature_enabled, must_change_password) \
                     VALUES ($1, $2, $3, 'x', true, $4, false, 'internal', true, false)",
                )
                .bind(uid)
                .bind(&email)
                .bind(&name)
                .bind(op["superadmin"].as_bool().unwrap())
                .execute(&mut *tx)
                .await
                .unwrap();
                for slug in op["roles"].as_array().unwrap() {
                    let rid = Self::role_id(&mut tx, slug.as_str().unwrap()).await;
                    sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
                        .bind(uid)
                        .bind(rid)
                        .execute(&mut *tx)
                        .await
                        .unwrap();
                }
                sqlx::query(
                    "INSERT INTO employees (id, first_name, email, status, user_id, department, reports_to_id, \
                     salary_currency, id_type, is_deleted) VALUES ($1, $2, $3, 'active', $4, $5, $6, 'JPY', 'national_id', false)",
                )
                .bind(eid)
                .bind(&name)
                .bind(&email)
                .bind(uid)
                .bind(op["department"].as_str())
                .bind(manager)
                .execute(&mut *tx)
                .await
                .unwrap();
                tx.commit().await.unwrap();
                self.ids.insert(key.into(), uid.to_string());
                self.ids.insert(format!("{key}:employee"), eid.to_string());
            }
            "product" => {
                let pid = new();
                self.ids.insert(s(op, "key").into(), pid.clone());
                self.exec(
                    "INSERT INTO product_catalog (id, sku, name, product_type, unit, cost_price, is_active, sale_model, \
                     price_metric, modules, pricing_mode, addon_for, is_whitelabel, is_orderable, is_deleted) VALUES \
                     ($1, $2, 'Golden Laptop', 'physical', 'Unit', $3, true, 'one_time', 'unit', '{}', 'list', '{}', \
                     false, true, false)",
                    vec![json!(pid), json!(format!("GOLD-{}", s(op, "key"))), op["cost"].as_f64().map(|c| json!(c)).unwrap_or(Value::Null)],
                )
                .await;
                self.exec(
                    "INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES \
                     ($1, $2, 'JPY', $3, '2000-01-01')",
                    vec![json!(new()), json!(pid), json!(f(op, "price"))],
                )
                .await;
            }
            "quotation" => {
                let qid = new();
                self.ids.insert(s(op, "key").into(), qid.clone());
                let amount = f(op, "qty") * f(op, "unit_price") * (1.0 - f(op, "discount") / 100.0);
                self.exec(
                    "INSERT INTO quotations (id, number, revision, currency, status, issue_date, valid_until, subtotal, \
                     total, payment_terms, discount_value, discount_amount, tax_amount, tax_lines, rounded_total, \
                     total_quantity, validity_days, procurement_type, customer_po_accepted_verbally, is_deleted) VALUES \
                     ($1, $2, 1, 'JPY', 'draft', '2026-10-01', '2026-10-15', $3, $3, '{\"text\": \"NET30\"}', 0, 0, 0, \
                     '[]', 0, 0, 14, 'standard', false, false)",
                    vec![json!(qid), json!(format!("GOLD-{}", s(op, "key"))), json!(amount)],
                )
                .await;
                let product = op["product"]
                    .as_str()
                    .map(|p| json!(self.id(p)))
                    .unwrap_or(Value::Null);
                let product_bind = if product.is_null() {
                    json!(null)
                } else {
                    product
                };
                // product_id is a UUID column: bind None as a uuid.
                let mut q = sqlx::query(
                    "INSERT INTO quotation_items (id, quotation_id, product_id, name, quantity, unit_price, currency, \
                     discount_percentage, discount_type, amount, sort_order, unit, is_optional, tax_pct, tax_amount) \
                     VALUES ($1, $2, $3, 'Line', $4, $5, 'JPY', $6, $7, $8, 0, 'Unit', false, 0, 0)",
                )
                .bind(Uuid::new_v4())
                .bind(Uuid::parse_str(&qid).unwrap());
                q = q.bind(product_bind.as_str().map(|p| Uuid::parse_str(p).unwrap()));
                q.bind(f(op, "qty"))
                    .bind(f(op, "unit_price"))
                    .bind(f(op, "discount"))
                    .bind(s(op, "discount_type"))
                    .bind(amount)
                    .execute(&self.pool)
                    .await
                    .unwrap();
            }
            "expense" => {
                let id = new();
                self.ids.insert(s(op, "key").into(), id.clone());
                self.exec(
                    "INSERT INTO expenses (id, category, description, amount, currency, exchange_rate, expense_date, \
                     status, submitted_by_id, tax_deductible, withholding_applicable, is_deleted) VALUES ($1, 'Travel', \
                     'Client visit', $2, 'JPY', 1, '2026-10-01', 'pending', $3, false, false, false)",
                    vec![json!(id), json!(f(op, "amount")), json!(self.id(s(op, "claimant")))],
                )
                .await;
            }
            "vendor_wo" => {
                let (vid, wid) = (new(), new());
                self.ids.insert(s(op, "vendor").into(), vid.clone());
                self.ids.insert(s(op, "wo").into(), wid.clone());
                self.exec(
                    "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, has_portal_access, \
                     total_spend, spend_currency, is_deleted) VALUES ($1, $2, 'professional_services', 'active', false, \
                     false, 0, 'JPY', false)",
                    vec![json!(vid), json!(format!("Golden Vendor {}", s(op, "vendor")))],
                )
                .await;
                self.exec(
                    "INSERT INTO work_orders (id, number, vendor_id, title, status, currency, payment_type, \
                     agreed_value, withholding_applicable, is_deleted) VALUES ($1, $2, $3, 'Install', $4, 'JPY', 'fixed', \
                     3000000, false, false)",
                    vec![json!(wid), json!(format!("GOLD-{}", s(op, "wo"))), json!(vid), json!(s(op, "status"))],
                )
                .await;
            }
            "vendor_invoice" => {
                let id = new();
                self.ids.insert(s(op, "key").into(), id.clone());
                self.exec(
                    "INSERT INTO vendor_invoices (id, vendor_id, work_order_id, vendor_invoice_number, invoice_date, \
                     amount, currency, status, tax_amount, withholding_amount, is_deleted) VALUES ($1, $2, $3, $4, \
                     '2026-10-01', 3000000, 'JPY', 'submitted', 0, 0, false)",
                    vec![
                        json!(id),
                        json!(self.id(s(op, "vendor"))),
                        json!(self.id(s(op, "wo"))),
                        json!(format!("GOLD-{}", s(op, "key"))),
                    ],
                )
                .await;
            }
            "po" => {
                let (vid, pid) = (new(), new());
                self.ids.insert(s(op, "key").into(), pid.clone());
                self.exec(
                    "INSERT INTO vendors (id, name, vendor_type, status, withholding_applicable, has_portal_access, \
                     total_spend, spend_currency, is_deleted) VALUES ($1, $2, 'component_supplier', 'active', false, \
                     false, 0, 'JPY', false)",
                    vec![json!(vid), json!(format!("Golden Supplier {}", s(op, "key")))],
                )
                .await;
                self.exec(
                    "INSERT INTO purchase_orders (id, number, vendor_id, status, currency, subtotal, total, \
                     requested_by_id, tax_amount, shipping_cost, approval_status, is_deleted) VALUES ($1, $2, $3, \
                     'draft', 'JPY', $4, $4, $5, 0, 0, 'not_required', false)",
                    vec![
                        json!(pid),
                        json!(format!("GOLD-{}", s(op, "key"))),
                        json!(vid),
                        json!(f(op, "total")),
                        json!(self.id(s(op, "requested_by"))),
                    ],
                )
                .await;
            }
            "attachment" => {
                let n = op["n"].as_i64().unwrap();
                // entity_id is a VARCHAR column: bind the id as text.
                sqlx::query(
                    "INSERT INTO attachments (id, entity_type, entity_id, file_name, file_size, mime_type, object_key, \
                     url, file_type) VALUES ($1, $2, $3, $4, 10, 'application/pdf', $5, $6, 'file')",
                )
                .bind(Uuid::new_v4())
                .bind(s(op, "entity_type"))
                .bind(self.id(s(op, "entity")))
                .bind(format!("quote-{n}.pdf"))
                .bind(format!("k/{n}"))
                .bind(format!("/q/{n}"))
                .execute(&self.pool)
                .await
                .unwrap();
            }
            "step_department" => {
                sqlx::query(
                    "UPDATE approval_policy_steps SET department = $1 WHERE sequence = $2 AND policy_id = \
                     (SELECT id FROM approval_policies WHERE entity_type = $3)",
                )
                .bind(s(op, "department"))
                .bind(op["sequence"].as_i64().unwrap() as i32)
                .bind(s(op, "entity_type"))
                .execute(&self.pool)
                .await
                .unwrap();
            }
            "edit_items" => {
                sqlx::query(&format!(
                    "UPDATE quotation_items SET {} WHERE quotation_id = $1",
                    s(op, "set")
                ))
                .bind(self.uuid(s(op, "entity")))
                .execute(&self.pool)
                .await
                .unwrap();
            }
            other => panic!("unknown fixture op {other}"),
        }
    }

    async fn user(&self, key: &str) -> AuthUser {
        let mut conn = self.pool.acquire().await.unwrap();
        load_active_user(&mut conn, self.uuid(key))
            .await
            .unwrap()
            .unwrap()
    }

    async fn latest_id(&self, key: &str) -> String {
        let id: Uuid = sqlx::query_scalar(
            "SELECT id FROM approval_requests WHERE entity_id = $1 ORDER BY submitted_at DESC LIMIT 1",
        )
        .bind(self.uuid(key))
        .fetch_one(&self.pool)
        .await
        .unwrap();
        id.to_string()
    }

    async fn run(&self, op: &Value) -> Value {
        let kind = s(op, "op");
        if matches!(kind, "gate" | "remind" | "supersede" | "states_for") {
            return self.run_other(op).await;
        }
        let user = match op.get("user").and_then(|u| u.as_str()) {
            Some(k) => Some(self.user(k).await),
            None => None,
        };
        let user_ref = user.as_ref().unwrap();
        let mut tx = self.pool.begin().await.unwrap();
        let (result, ob) = notify::scope(async {
            let r = match kind {
                "submit" => {
                    let flags: Vec<String> = op["flags"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|f| f.as_str().unwrap().to_string())
                        .collect();
                    engine::submit(
                        &mut tx,
                        s(op, "entity_type"),
                        &self.id(s(op, "entity")),
                        user_ref,
                        Some(&flags),
                        None,
                        None,
                    )
                    .await
                    .map(Some)
                }
                "act" => {
                    let rid = self.latest_id(s(op, "entity")).await;
                    engine::act(
                        &mut tx,
                        &rid,
                        user_ref,
                        s(op, "decision"),
                        op["comment"].as_str(),
                        None,
                        None,
                    )
                    .await
                    .map(Some)
                }
                "cancel" => {
                    let rid = self.latest_id(s(op, "entity")).await;
                    engine::cancel(&mut tx, &rid, user_ref, op["reason"].as_str())
                        .await
                        .map(Some)
                }
                "ensure_or_submit" => {
                    engine::ensure_or_submit(
                        &mut tx,
                        s(op, "entity_type"),
                        self.uuid(s(op, "entity")),
                        user_ref,
                        s(op, "action"),
                        None,
                    )
                    .await
                }
                other => panic!("unknown op {other}"),
            };
            let ob = outbox_json();
            if r.is_ok() {
                notify::dispatch_outbox();
            }
            (r, ob)
        })
        .await;
        match result {
            Ok(request) => {
                tx.commit().await.unwrap();
                if kind == "ensure_or_submit" {
                    return json!({"ok": request.map(|r| r.id.to_string()), "outbox": ob});
                }
                let request = request.unwrap();
                let mut conn = self.pool.acquire().await.unwrap();
                let status = if kind == "submit" { 201 } else { 200 };
                let body = engine::serialize(&mut conn, &request, user.as_ref())
                    .await
                    .unwrap();
                json!({"status": status, "body": body, "outbox": ob})
            }
            Err(e) => {
                tx.rollback().await.unwrap();
                with_outbox(failure(&e), ob)
            }
        }
    }

    async fn run_other(&self, op: &Value) -> Value {
        match s(op, "op") {
            "gate" => {
                let mut conn = self.pool.acquire().await.unwrap();
                match engine::ensure_approved(
                    &mut conn,
                    s(op, "entity_type"),
                    self.uuid(s(op, "entity")),
                    "sent",
                )
                .await
                {
                    Ok(r) => json!({"ok": r.map(|r| r.id.to_string())}),
                    Err(e) => with_outbox(failure(&e), json!([])),
                }
            }
            "remind" => {
                let rid = Uuid::parse_str(&self.latest_id(s(op, "entity")).await).unwrap();
                let mut row: granary_db::rows::approval::ApprovalRequest =
                    sqlx::query_as("SELECT * FROM approval_requests WHERE id = $1")
                        .bind(rid)
                        .fetch_one(&self.pool)
                        .await
                        .unwrap();
                if let Some(seq) = row.current_step_seq {
                    let due = granary_compat::pyjson::PyDateTime::from_chrono_utc(
                        &(chrono::Utc::now()
                            - chrono::Duration::hours(op["hours"].as_i64().unwrap())),
                    )
                    .isoformat();
                    for st in row.steps.as_array_mut().unwrap() {
                        if st["seq"] == seq && !st["is_notify_only"].as_bool().unwrap_or(false) {
                            st["due_at"] = json!(due);
                        }
                    }
                    sqlx::query(
                        "UPDATE approval_requests SET steps = $2, updated_at = now() WHERE id = $1",
                    )
                    .bind(rid)
                    .bind(&row.steps)
                    .execute(&self.pool)
                    .await
                    .unwrap();
                }
                let mut tx = self.pool.begin().await.unwrap();
                let (n, ob) = notify::scope(async {
                    let n = engine::remind_overdue(&mut tx, None).await.unwrap();
                    (n, outbox_json())
                })
                .await;
                tx.commit().await.unwrap();
                json!({"count": n, "outbox": ob})
            }
            "supersede" => {
                let mut tx = self.pool.begin().await.unwrap();
                engine::supersede_for_entity(
                    &mut tx,
                    s(op, "entity_type"),
                    self.uuid(s(op, "entity")),
                    s(op, "reason"),
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
                json!({"ok": null})
            }
            "states_for" => {
                let ids: Vec<Uuid> = op["entities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|e| self.ids.get(e.as_str().unwrap()))
                    .map(|i| Uuid::parse_str(i).unwrap())
                    .collect();
                let mut conn = self.pool.acquire().await.unwrap();
                json!({"ok": Value::Object(engine::states_for(&mut conn, s(op, "entity_type"), &ids).await.unwrap())})
            }
            other => panic!("unknown op {other}"),
        }
    }

    async fn dump_rows(&self, keys: &[String]) -> Value {
        let ent: Vec<String> = keys.iter().map(|k| self.id(k)).collect();
        let reqs: Vec<(String, String, String, Option<i32>, String, i32, Option<String>, Option<String>, bool, Option<i32>, Value, Value, Value, bool)> =
            sqlx::query_as(
                "SELECT id::text, entity_type, entity_id::text, policy_version, status, document_revision, \
                 document_label, submitted_by_id::text, completed_at IS NOT NULL AS completed, current_step_seq, \
                 snapshot, flags, steps, last_reminded_at IS NOT NULL AS reminded FROM approval_requests \
                 WHERE entity_id::text = ANY($1) ORDER BY submitted_at",
            )
            .bind(&ent)
            .fetch_all(&self.pool)
            .await
            .unwrap();
        let rids: Vec<String> = reqs.iter().map(|r| r.0.clone()).collect();
        let requests: Vec<Value> = reqs
            .into_iter()
            .map(|mut r| {
                // 17 Nov 2026: facts_bound marks approvals made by Rust from then on (not in the Python golden)
                if let Some(o) = r.10.as_object_mut() {
                    o.remove("facts_bound");
                }
                json!({"id": r.0, "entity_type": r.1, "entity_id": r.2, "policy_version": r.3, "status": r.4,
                       "document_revision": r.5, "document_label": r.6, "submitted_by_id": r.7, "completed": r.8,
                       "current_step_seq": r.9, "snapshot": r.10, "flags": r.11, "steps": r.12, "reminded": r.13})
            })
            .collect();
        let acts: Vec<(String, Option<i32>, Option<String>, String, Option<String>, Option<String>, Option<String>)> =
            sqlx::query_as(
                "SELECT request_id::text, step_seq, actor_id::text, action, comment, ip_address, user_agent \
                 FROM approval_actions WHERE request_id::text = ANY($1)",
            )
            .bind(&rids)
            .fetch_all(&self.pool)
            .await
            .unwrap();
        let actions: Vec<Value> = acts
            .into_iter()
            .map(|a| json!({"request_id": a.0, "step_seq": a.1, "actor_id": a.2, "action": a.3, "comment": a.4,
                            "ip_address": a.5, "user_agent": a.6}))
            .collect();
        let notes: Vec<(String, String, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<bool>, Option<String>, Option<String>)> =
            sqlx::query_as(
                "SELECT user_id::text, type, title, body, link, entity_type, entity_id, is_read, read_at, priority \
                 FROM notifications WHERE entity_id = ANY($1)",
            )
            .bind(&ent)
            .fetch_all(&self.pool)
            .await
            .unwrap();
        let notifications: Vec<Value> = notes
            .into_iter()
            .map(|n| json!({"user_id": n.0, "type": n.1, "title": n.2, "body": n.3, "link": n.4, "entity_type": n.5,
                            "entity_id": n.6, "is_read": n.7, "read_at": n.8, "priority": n.9}))
            .collect();
        json!({"requests": requests, "actions": actions, "notifications": notifications})
    }

    async fn labels(&self) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = self
            .ids
            .iter()
            .map(|(k, v)| (v.clone(), k.clone()))
            .collect();
        let users: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, email FROM users")
            .fetch_all(&self.pool)
            .await
            .unwrap();
        for (id, email) in users {
            out.insert(id.to_string(), format!("user:{email}"));
        }
        let rows: Vec<(Uuid, Uuid)> =
            sqlx::query_as("SELECT id, entity_id FROM approval_requests ORDER BY submitted_at")
                .fetch_all(&self.pool)
                .await
                .unwrap();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for (rid, eid) in rows {
            let ent = out
                .get(&eid.to_string())
                .cloned()
                .unwrap_or_else(|| "?".into());
            let n = seen.entry(ent.clone()).or_insert(0);
            *n += 1;
            out.insert(rid.to_string(), format!("request:{ent}:{n}"));
        }
        out
    }
}

struct Norm {
    names: HashMap<String, String>,
    uuid: Regex,
    ts: Regex,
    sha: Regex,
}

impl Norm {
    fn str(&self, s: &str) -> String {
        let s = self.uuid.replace_all(s, |c: &regex::Captures| {
            self.names
                .get(&c[0])
                .cloned()
                .unwrap_or_else(|| "<uuid>".into())
        });
        let s = self.ts.replace_all(&s, "<ts>");
        self.sha.replace_all(&s, "<sha>").into_owned()
    }
    fn value(&self, v: &Value) -> Value {
        match v {
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, x)| (self.str(k), self.value(x)))
                    .collect(),
            ),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect()),
            Value::String(s) => Value::String(self.str(s)),
            other => other.clone(),
        }
    }
}

/// Python equality (ints vs floats by value; dict order ignored) through the compat value model.
fn canon(v: &Value) -> String {
    dumps(&PyValue::from(v), &DumpOpts::sorted()).unwrap()
}

/// Rows have no defined order: compare them as multisets.
fn canon_rows(v: &Value) -> Value {
    let mut out = Map::new();
    for (k, rows) in v.as_object().unwrap() {
        let mut list: Vec<String> = rows.as_array().unwrap().iter().map(canon).collect();
        if k != "requests" {
            list.sort();
        }
        out.insert(k.clone(), json!(list));
    }
    Value::Object(out)
}

async fn replay(include_emit: bool) {
    let mut doc = fixture();
    let pool = support::fresh_db(if include_emit {
        "s13_engine_golden_all"
    } else {
        "s13_engine_golden"
    })
    .await;
    bless::neutral_owner(&pool).await;
    notify::set_external_delivery_enabled(false);
    let mut run = Run {
        pool,
        ids: HashMap::new(),
    };
    let mut got = vec![];
    let mut wanted = vec![];
    for (scenario, expected) in doc["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .zip(doc["results"].as_array().unwrap())
    {
        if scenario["needs_emit"].as_bool().unwrap_or(false) && !include_emit {
            break;
        }
        let mut ops = vec![];
        let mut keys = vec![];
        for op in scenario["ops"].as_array().unwrap() {
            let kind = s(op, "op");
            if matches!(kind, "quotation" | "expense" | "vendor_invoice" | "po") {
                keys.push(s(op, "key").to_string());
            }
            if matches!(
                kind,
                "person"
                    | "product"
                    | "quotation"
                    | "expense"
                    | "vendor_wo"
                    | "vendor_invoice"
                    | "po"
                    | "attachment"
                    | "step_department"
                    | "edit_items"
            ) {
                run.fixture(op).await;
                ops.push(Value::Null);
            } else {
                ops.push(run.run(op).await);
            }
        }
        let rows = run.dump_rows(&keys).await;
        got.push((scenario["name"].clone(), ops, rows));
        wanted.push(expected.clone());
    }
    let norm = Norm {
        names: run.labels().await,
        uuid: Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap(),
        ts: Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?(?:[+-]\d{2}:\d{2}|Z)?")
            .unwrap(),
        sha: Regex::new(r"\b[0-9a-f]{64}\b").unwrap(),
    };
    if bless::blessing() {
        // the run with every scenario records them all
        if include_emit {
            let results = doc["results"].as_array_mut().unwrap();
            for (i, (name, ops, rows)) in got.iter().enumerate() {
                let ops: Vec<Value> = ops.iter().map(|o| norm.value(o)).collect();
                results[i] = json!({"name": name, "ops": ops, "rows": norm.value(rows)});
            }
            bless::write("approvals_engine/scenarios.json", &doc);
        }
        return;
    }
    let mut failures = vec![];
    for ((name, ops, rows), expected) in got.iter().zip(&wanted) {
        for (i, (op, want)) in ops
            .iter()
            .zip(expected["ops"].as_array().unwrap())
            .enumerate()
        {
            let op = norm.value(op);
            if canon(&op) != canon(want) {
                failures.push(format!(
                    "{name} op #{i}:\n  want {}\n  got  {}",
                    canon(want),
                    canon(&op)
                ));
            }
        }
        let rows = canon_rows(&norm.value(rows));
        let want_rows = canon_rows(&expected["rows"]);
        for k in ["requests", "actions", "notifications"] {
            if rows[k] != want_rows[k] {
                failures.push(format!(
                    "{name} rows.{k}:\n  want {}\n  got  {}",
                    want_rows[k], rows[k]
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(!got.is_empty());
}

#[tokio::test]
async fn engine_scenarios_match_python() {
    replay(false).await;
}

#[tokio::test]
async fn engine_scenarios_with_decisions_match_python() {
    replay(true).await;
}
