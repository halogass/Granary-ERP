//! Port of backend/app/services/nary/labels.py: WhatsApp Business labels kept in step with the funnel
//! (Nary Phase 2, item F).
//!
//! Off until nary.wa.labels_enabled. nary.wa.label_map maps a deal stage (or a lead status, prefixed
//! "lead:") to the name of a label that already exists on the company's WhatsApp Business app. When a deal
//! moves, its chats get the new stage's label and lose the other mapped ones; labels that aren't in the map
//! are left alone. When the service has no labels (501) the Owner is told once and nothing else happens.
//!
//! Port note: one `httpx.AsyncClient(timeout=httpx.Timeout(20.0, connect=5.0))` for the whole sync; an
//! httpx error while listing the labels is "WhatsApp unreachable"; one while posting a chat's labels is not
//! caught (it propagates), as in Python.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{get_optional, require_json};
use granary_db::rows::nary::DomainEvent;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::nary::events::{subscriber, Subscription};
use crate::nary::wa_ingest::py_str;
use crate::whatsapp::{httpx_json_body, httpx_url, py_truthy, HttpxError, HttpxResponse};

pub const WHERE: &str = "Nary & AI → Nary on WhatsApp";

/// The `@events.subscribe(...)` registrations of this module.
/// Quiet (29 Oct 2026): they run while Nary is paused too. Labels on the company's own WhatsApp Business app are
/// bookkeeping the customer never sees; nothing is sent and no card is raised.
pub fn subscriptions() -> Vec<Subscription> {
    vec![
        Subscription::quiet(
            &["deal.created", "deal.staged"],
            subscriber(|db, ev| Box::pin(on_deal(db, ev))),
        ),
        Subscription::quiet(
            &["lead.created"],
            subscriber(|db, ev| Box::pin(on_lead(db, ev))),
        ),
    ]
}

pub async fn enabled(db: &mut PgConnection) -> ApiResult<bool> {
    let v = get_optional(db, "nary.wa.labels_enabled").await?;
    Ok(granary_compat::text::pystr::lower(v.as_deref().unwrap_or("false")) == "true")
}

fn client() -> reqwest::Client {
    crate::whatsapp::httpx_client(Duration::from_secs(5))
}

fn base() -> String {
    granary_core::config::settings()
        .WHATSAPP_SERVICE_URL
        .clone()
}

fn headers(mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    for (k, v) in crate::whatsapp::_headers() {
        rb = rb.header(k, v);
    }
    rb.timeout(Duration::from_secs(20))
}

/// `_labels(c)`: label name (stripped, lower-cased) to id, or None when labels aren't available.
pub async fn _labels(c: &reqwest::Client) -> Result<Option<HashMap<String, String>>, HttpxError> {
    let url = httpx_url(&format!("{}/labels", base()), &[])?;
    let resp = HttpxResponse::read(headers(c.get(url)).send().await?).await?;
    if resp.status != 200 {
        return Ok(None);
    }
    let data = resp.json()?;
    let mut out = HashMap::new();
    if let Value::Array(items) = data {
        for l in items {
            if l.get("name").is_some_and(py_truthy) {
                let name = granary_compat::text::pystr::lower(granary_compat::text::pystr::strip(
                    &py_str(&l["name"]),
                ));
                let id = l.get("id").map(py_str).unwrap_or_else(|| "None".into());
                out.insert(name, id);
            }
        }
    }
    Ok(Some(out))
}

/// `sync(db, *, stage_key, chat_ids)`: give these chats the label mapped to `stage_key`, removing the other
/// mapped labels.
pub async fn sync(db: &mut PgConnection, stage_key: &str, chat_ids: &[String]) -> ApiResult<Value> {
    if chat_ids.is_empty() || !enabled(db).await? {
        return Ok(json!({"synced": 0}));
    }
    let raw = require_json(db, "nary.wa.label_map", WHERE).await?;
    // `{k: str(v).strip() for k, v in (... or {}).items() if v}`, in the map's order.
    let mut mapping: Vec<(String, String)> = Vec::new();
    if let Value::Object(m) = &raw {
        for (k, v) in m {
            if py_truthy(v) {
                mapping.push((
                    k.clone(),
                    granary_compat::text::pystr::strip(&py_str(v)).to_string(),
                ));
            }
        }
    }
    let wanted_name = mapping
        .iter()
        .find(|(k, _)| k == stage_key)
        .map(|(_, v)| v.clone());
    let c = client();
    let by_name = match _labels(&c).await {
        Ok(v) => v,
        Err(e) if e.is_http_error() => {
            return Ok(json!({"synced": 0, "error": "WhatsApp unreachable"}))
        }
        Err(e) => return Err(ApiError::from(e)),
    };
    let Some(by_name) = by_name else {
        crate::nary::governance::error_card(
            db,
            "nary.labels",
            "WhatsApp labels can't be synced",
            "The WhatsApp service says labels aren't available for this account or build. \
             Turn nary.wa.labels_enabled off, or check after the next WhatsApp update.",
            "/settings",
        )
        .await?;
        return Ok(json!({"synced": 0, "error": "labels not supported"}));
    };
    let lower = granary_compat::text::pystr::lower;
    let mapped_ids: BTreeSet<String> = mapping
        .iter()
        .filter_map(|(_, n)| by_name.get(&lower(n)).cloned())
        .collect();
    let add: Vec<String> = match &wanted_name {
        Some(w) if !w.is_empty() => by_name.get(&lower(w)).cloned().into_iter().collect(),
        _ => vec![],
    };
    // `sorted(mapped_ids - set(add))`: str sort by code point, as BTreeSet orders.
    let remove: Vec<String> = mapped_ids
        .into_iter()
        .filter(|i| !add.contains(i))
        .collect();
    let mut n = 0;
    for chat_id in chat_ids {
        let url =
            httpx_url(&format!("{}/labels/{chat_id}", base()), &[]).map_err(ApiError::from)?;
        let body = httpx_json_body(&json!({"add": add, "remove": remove}));
        let resp = headers(c.post(url))
            .body(body)
            .send()
            .await
            .map_err(|e| ApiError::from(HttpxError::from(e)))?;
        let resp = HttpxResponse::read(resp).await.map_err(ApiError::from)?;
        n += (resp.status == 200) as i64;
    }
    Ok(json!({"synced": n, "add": add, "remove": remove}))
}

/// `_chats_for(db, *, contact_id=None, deal_id=None, lead_id=None)`: the active customer chats of a deal,
/// lead or contact, without repeats, in the database's order.
pub async fn _chats_for(
    db: &mut PgConnection,
    contact_id: Option<Uuid>,
    deal_id: Option<Uuid>,
    lead_id: Option<Uuid>,
) -> ApiResult<Vec<String>> {
    let mut conds: Vec<String> = Vec::new();
    if deal_id.is_some() {
        conds.push("deal_id = $1".into());
    }
    if lead_id.is_some() {
        conds.push("lead_id = $2".into());
    }
    if contact_id.is_some() {
        conds.push("contact_id = $3".into());
    }
    if conds.is_empty() {
        return Ok(vec![]);
    }
    let sql = format!(
        "SELECT chat_id FROM wa_chats WHERE ({}) AND status = 'active' AND staff_user_id IS NULL",
        conds.join(" OR ")
    );
    let rows: Vec<String> = sqlx::query_scalar(&sql)
        .bind(deal_id)
        .bind(lead_id)
        .bind(contact_id)
        .fetch_all(&mut *db)
        .await?;
    let mut out: Vec<String> = Vec::new();
    for r in rows {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    Ok(out)
}

/// `@events.subscribe("deal.created", "deal.staged") on_deal(db, event)`.
pub async fn on_deal(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    if !enabled(db).await? {
        return Ok(());
    }
    let Some(id) = event.entity_id else {
        return Ok(());
    };
    let deal: Option<(Option<String>, Option<Uuid>)> =
        sqlx::query_as("SELECT stage, contact_id FROM deals WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let Some((stage, contact_id)) = deal else {
        return Ok(());
    };
    let chats = _chats_for(db, contact_id, Some(id), None).await?;
    sync(db, stage.as_deref().unwrap_or("None"), &chats).await?;
    Ok(())
}

/// `@events.subscribe("lead.created") on_lead(db, event)`.
pub async fn on_lead(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    if !enabled(db).await? {
        return Ok(());
    }
    let Some(id) = event.entity_id else {
        return Ok(());
    };
    let lead: Option<(Option<String>, Option<Uuid>)> =
        sqlx::query_as("SELECT status, contact_id FROM leads WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let Some((status, contact_id)) = lead else {
        return Ok(());
    };
    let chats = _chats_for(db, contact_id, None, Some(id)).await?;
    let key = format!("lead:{}", status.as_deref().unwrap_or("None"));
    sync(db, &key, &chats).await?;
    Ok(())
}
