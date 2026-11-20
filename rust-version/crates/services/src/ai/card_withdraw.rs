//! Nary fix batch 4, agent D3: taking back confirm cards in a chat conversation, and one waiting
//! quotation or invoice card per deal or customer.
//!
//! The repro (20 Oct, 18:00 company time): `withdraw_cards(match="quotation")` returned nothing, because a
//! quotation card has no summary or recipient (only `kind`, `customer` and `payload.subject`) and the match
//! looked at the summary, recipient, address, label and title only. Now:
//! - a card matches on its kind, action, summary, recipient, customer, subject, the deal's title and the
//!   contact's name, and every word of `match` must be found (in any order);
//! - the waiting cards are listed with their card id, kind, customer, total and time, and `card_id`
//!   withdraws exactly one (or several);
//! - when nothing was withdrawn the answer says why: no card matches, it was already decided, it belongs
//!   to another conversation, or it is a Nary approval card (decided on the Approvals page);
//! - a new quotation or invoice card for the same deal or customer replaces the older waiting one in the
//!   same conversation, like message cards to the same recipient ([`supersede_drafts`]).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

/// `v` as plain text ("" for null/false/empty).
fn txt(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => String::new(),
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Array(a)) if a.is_empty() => String::new(),
        Some(Value::Object(o)) if o.is_empty() => String::new(),
        Some(other) => other.to_string(),
    }
}

fn get<'a>(p: &'a Map<String, Value>, path: &[&str]) -> Option<&'a Value> {
    let mut cur = p.get(path[0])?;
    for k in &path[1..] {
        cur = cur.get(*k)?;
    }
    Some(cur)
}

/// One message's cards: (message id, created_at, tool_calls).
type Row = (Uuid, DateTime<Utc>, Value);

/// The last 40 assistant messages of the conversation with cards (newest first), as assistant.rs reads them.
async fn rows(db: &mut PgConnection, conversation_id: Uuid) -> Result<Vec<Row>, sqlx::Error> {
    let rows: Vec<(Uuid, DateTime<Utc>, Option<Value>)> = sqlx::query_as(
        "SELECT ai_messages.id, ai_messages.created_at, ai_messages.tool_calls FROM ai_messages \
         WHERE ai_messages.conversation_id = $1 AND ai_messages.role = 'assistant' \
         ORDER BY ai_messages.created_at DESC LIMIT 40",
    )
    .bind(conversation_id)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, at, tc)| tc.filter(Value::is_object).map(|tc| (id, at, tc)))
        .collect())
}

async fn write(db: &mut PgConnection, id: Uuid, tc: &Value) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE ai_messages SET tool_calls = $2, updated_at = now() WHERE ai_messages.id = $1",
    )
    .bind(id)
    .bind(tc)
    .execute(db)
    .await?;
    Ok(())
}

fn cards_mut(tc: &mut Value) -> Option<&mut Vec<Value>> {
    tc.get_mut("proposals").and_then(Value::as_array_mut)
}

fn cards(tc: &Value) -> Vec<&Map<String, Value>> {
    tc.get("proposals")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

fn status(p: &Map<String, Value>) -> &str {
    p.get("status").and_then(Value::as_str).unwrap_or("")
}

/// The name a card is known by (unchanged from before: summary, label or title, else its kind).
pub fn card_name(p: &Map<String, Value>) -> Value {
    ["summary", "label", "title"]
        .iter()
        .find_map(|k| p.get(*k).filter(|v| !txt(Some(v)).is_empty()).cloned())
        .unwrap_or_else(|| p.get("kind").cloned().unwrap_or(Value::Null))
}

/// Deal titles and contact names the cards point at (`payload.deal_id`, `payload.customer_contact_id`).
#[derive(Default)]
struct Names {
    deals: HashMap<String, String>,
    contacts: HashMap<String, String>,
}

async fn names(db: &mut PgConnection, all: &[Row]) -> Result<Names, sqlx::Error> {
    let mut deal_ids: Vec<Uuid> = vec![];
    let mut contact_ids: Vec<Uuid> = vec![];
    for (_, _, tc) in all {
        for p in cards(tc) {
            for (path, is_deal) in [
                (&["payload", "deal_id"][..], true),
                (&["payload", "customer_contact_id"][..], false),
                (&["payload", "contact_id"][..], false),
            ] {
                let out = if is_deal {
                    &mut deal_ids
                } else {
                    &mut contact_ids
                };
                if let Some(id) = get(p, path)
                    .and_then(Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok())
                {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
            }
        }
    }
    let mut n = Names::default();
    if !deal_ids.is_empty() {
        let found: Vec<(Uuid, String)> =
            sqlx::query_as("SELECT id, title FROM deals WHERE id = ANY($1)")
                .bind(&deal_ids)
                .fetch_all(&mut *db)
                .await?;
        n.deals = found
            .into_iter()
            .map(|(id, t)| (id.to_string(), t))
            .collect();
    }
    if !contact_ids.is_empty() {
        let found: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, trim(first_name || ' ' || COALESCE(last_name, '')) FROM contacts WHERE id = ANY($1)",
        )
        .bind(&contact_ids)
        .fetch_all(&mut *db)
        .await?;
        n.contacts = found
            .into_iter()
            .map(|(id, t)| (id.to_string(), t))
            .collect();
    }
    Ok(n)
}

/// Everything a card can be found by, lower case.
fn haystack(p: &Map<String, Value>, n: &Names) -> String {
    let mut parts: Vec<String> = [
        "summary",
        "recipient",
        "to",
        "chat_id",
        "label",
        "title",
        "kind",
        "action",
        "customer",
    ]
    .iter()
    .map(|k| txt(p.get(*k)))
    .collect();
    for path in [
        &["payload", "subject"][..],
        &["payload", "customer_name"][..],
        &["payload", "quotation_number"][..],
    ] {
        parts.push(txt(get(p, path)));
    }
    if let Some(d) = get(p, &["payload", "deal_id"]).and_then(Value::as_str) {
        parts.push(n.deals.get(d).cloned().unwrap_or_default());
    }
    for path in [
        &["payload", "customer_contact_id"][..],
        &["payload", "contact_id"][..],
    ] {
        if let Some(c) = get(p, path).and_then(Value::as_str) {
            parts.push(n.contacts.get(c).cloned().unwrap_or_default());
        }
    }
    // "draft quotation" finds a quotation card; "invoice" a proforma
    match p.get("kind").and_then(Value::as_str) {
        Some("quotation") => parts.push("draft quotation quote".into()),
        Some("invoice") => parts.push("draft invoice proforma".into()),
        Some("customer_message") => parts.push("message".into()),
        _ => {}
    }
    parts.retain(|s| !s.is_empty());
    parts.join(" ").to_lowercase()
}

/// `m` (already lower case) is in the text, or every word of it is.
fn matches(text: &str, m: &str) -> bool {
    if m.is_empty() {
        return false;
    }
    if text.contains(m) {
        return true;
    }
    let words: Vec<&str> = m.split_whitespace().collect();
    !words.is_empty() && words.iter().all(|w| text.contains(w))
}

/// What the model sees of a waiting card: id, kind, name, customer, total, when.
fn entry(p: &Map<String, Value>, at: &DateTime<Utc>, n: &Names) -> Value {
    let mut e = Map::new();
    e.insert(
        "card_id".into(),
        p.get("proposal_id").cloned().unwrap_or(Value::Null),
    );
    let kind = txt(p.get("kind"));
    e.insert(
        "kind".into(),
        json!(
            if kind == "nary_action" && !txt(p.get("action")).is_empty() {
                txt(p.get("action"))
            } else {
                kind.clone()
            }
        ),
    );
    let name = card_name(p);
    let name = if matches!(kind.as_str(), "quotation" | "invoice") && name == json!(kind) {
        match txt(get(p, &["payload", "subject"])) {
            s if !s.is_empty() => json!(s),
            _ => name,
        }
    } else {
        name
    };
    e.insert("summary".into(), name);
    let who = ["customer", "recipient", "to"]
        .iter()
        .map(|k| txt(p.get(*k)))
        .find(|s| !s.is_empty());
    if let Some(w) = who {
        e.insert("customer".into(), json!(w));
    }
    if let Some(d) = get(p, &["payload", "deal_id"]).and_then(Value::as_str) {
        if let Some(t) = n.deals.get(d) {
            e.insert("deal".into(), json!(t));
        }
    }
    if let Some(t) = p.get("total").filter(|v| v.is_number()) {
        e.insert("total".into(), t.clone());
        if let Some(c) = p.get("currency").filter(|v| !txt(Some(v)).is_empty()) {
            e.insert("currency".into(), c.clone());
        }
    }
    e.insert(
        "created_at".into(),
        json!(at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    );
    Value::Object(e)
}

/// `card_id` as one id or a list of ids.
fn card_ids(args: &Value) -> Vec<String> {
    match args.get("card_id") {
        Some(Value::String(s)) => s
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| txt(Some(v)).to_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => vec![],
    }
}

fn id_matches(p: &Map<String, Value>, ids: &[String]) -> bool {
    let pid = txt(p.get("proposal_id")).to_lowercase();
    !pid.is_empty()
        && ids
            .iter()
            .any(|i| *i == pid || (i.len() >= 8 && pid.starts_with(i.as_str())))
}

/// What a decided card's status means ("applied" -> confirmed...).
fn decided_words(s: &str) -> &'static str {
    match s {
        "applied" => "already confirmed and saved",
        "discarded" => "already discarded by the user",
        "failed" => "already confirmed, and it failed when applied",
        "withdrawn" => "already withdrawn",
        "superseded" => "already replaced by a newer card",
        _ => "already decided",
    }
}

/// `withdraw_cards(db, conversation_id, args)`: Nary takes back its own confirm cards in this conversation.
/// `card_id` (one id, a list, or ids separated by commas) picks exact cards; `match` picks the cards whose
/// text has every word of it; `all` takes back every one; with none of them the waiting cards are listed.
pub async fn withdraw_cards(
    db: &mut PgConnection,
    conversation_id: Uuid,
    args: &Value,
) -> Result<Value, sqlx::Error> {
    let m = txt(args.get("match")).to_lowercase();
    let ids = card_ids(args);
    let take_all = args.get("all").is_some_and(|v| match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
        _ => true,
    });
    let mut all = rows(&mut *db, conversation_id).await?;
    let n = names(&mut *db, &all).await?;
    let mut pending: Vec<Value> = vec![];
    let mut withdrawn: Vec<Value> = vec![];
    let mut decided: Vec<String> = vec![];
    for (id, at, tc) in all.iter_mut() {
        let mut changed = false;
        if let Some(list) = cards_mut(tc) {
            for p in list.iter_mut() {
                let Some(p) = p.as_object_mut() else { continue };
                let hit = take_all
                    || (!ids.is_empty() && id_matches(p, &ids))
                    || (ids.is_empty() && matches(&haystack(p, &n), &m));
                if status(p) != "pending" {
                    if hit && !take_all {
                        decided.push(format!(
                            "{} ({})",
                            txt(Some(&card_name(p))),
                            decided_words(status(p))
                        ));
                    }
                    continue;
                }
                if hit {
                    p.insert("status".into(), json!("withdrawn"));
                    withdrawn.push(card_name(p));
                    changed = true;
                } else {
                    pending.push(entry(p, at, &n));
                }
            }
        }
        if changed {
            write(&mut *db, *id, tc).await?;
        }
    }
    if !(take_all || !m.is_empty() || !ids.is_empty()) {
        return Ok(json!({"pending_cards": pending,
                         "note": "Call again with `card_id` (one or more of the ids above), `match` (words of a \
                                  card's summary, customer, subject or deal) or `all` to withdraw."}));
    }
    let mut out = json!({"withdrawn": withdrawn, "still_pending": pending});
    if !withdrawn.is_empty() {
        out["note"] =
            json!("Withdrawn cards can't be confirmed any more; say which ones you took back.");
        return Ok(out);
    }
    // Nothing was taken back: say why.
    let what = if ids.is_empty() {
        format!("\u{201c}{m}\u{201d}")
    } else {
        format!("card_id {}", ids.join(", "))
    };
    if take_all {
        out["reason"] = json!("no_pending_cards");
        out["note"] = json!("There are no waiting cards in this conversation.");
        return Ok(out);
    }
    if !decided.is_empty() {
        out["reason"] = json!("already_decided");
        out["note"] = json!(format!(
            "Nothing withdrawn: the card(s) matching {what} were decided already: {}. A decided card can't be \
             withdrawn; tell the user.",
            decided.join("; ")
        ));
        return Ok(out);
    }
    if let Some(other) = elsewhere(&mut *db, conversation_id, &ids, &m).await? {
        out["reason"] = json!("other_conversation");
        out["note"] = json!(format!(
            "Nothing withdrawn: {other} It is not from this conversation, so it can't be withdrawn here; the user \
             can discard it where it is."
        ));
        return Ok(out);
    }
    let queued = approval_cards(&mut *db, &ids, &m).await?;
    if !queued.is_empty() {
        out["reason"] = json!("approval_card");
        out["approval_cards"] = Value::Array(queued);
        out["note"] = json!(
            "Nothing withdrawn here: the matching card is a Nary approval card (Nary \u{2192} Approvals), not a card \
             of this conversation. The Owner declines it there (or with nary_decide_card)."
        );
        return Ok(out);
    }
    out["reason"] = json!("no_match");
    out["note"] = json!(format!(
        "Nothing withdrawn: no waiting card in this conversation matches {what}. The waiting cards are in \
         still_pending; call again with their card_id."
    ));
    Ok(out)
}

/// The same user's other conversations: a waiting card with this id (or text). Their text for the note.
async fn elsewhere(
    db: &mut PgConnection,
    conversation_id: Uuid,
    ids: &[String],
    m: &str,
) -> Result<Option<String>, sqlx::Error> {
    let found: Vec<(Option<String>, DateTime<Utc>, Value)> = sqlx::query_as(
        "SELECT c.title, m.created_at, m.tool_calls FROM ai_messages m \
         JOIN ai_conversations c ON c.id = m.conversation_id \
         WHERE c.user_id = (SELECT user_id FROM ai_conversations WHERE id = $1) AND c.id <> $1 \
           AND m.role = 'assistant' AND m.tool_calls ? 'proposals' \
           AND m.tool_calls::text LIKE '%\"pending\"%' \
         ORDER BY m.created_at DESC LIMIT 200",
    )
    .bind(conversation_id)
    .fetch_all(&mut *db)
    .await?;
    let titles: Vec<Option<String>> = found.iter().map(|(t, _, _)| t.clone()).collect();
    let found: Vec<Row> = found
        .into_iter()
        .map(|(_, at, tc)| (Uuid::nil(), at, tc))
        .collect();
    let n = names(&mut *db, &found).await?;
    for ((_, at, tc), title) in found.iter().zip(titles) {
        for p in cards(tc) {
            if status(p) != "pending" {
                continue;
            }
            let hit = if ids.is_empty() {
                matches(&haystack(p, &n), m)
            } else {
                id_matches(p, ids)
            };
            if hit {
                return Ok(Some(format!(
                    "the waiting card {} is in the conversation \u{201c}{}\u{201d} ({}).",
                    txt(Some(&card_name(p))),
                    title.unwrap_or_else(|| "untitled".into()),
                    at.format("%Y-%m-%d %H:%M UTC")
                )));
            }
        }
    }
    Ok(None)
}

/// Waiting Nary approval cards (agent_pending_actions) whose id, action or payload matches.
async fn approval_cards(
    db: &mut PgConnection,
    ids: &[String],
    m: &str,
) -> Result<Vec<Value>, sqlx::Error> {
    let found: Vec<(Uuid, String, Value, Option<Value>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, action, payload, diff, created_at FROM agent_pending_actions WHERE status = 'pending' \
         ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(&mut *db)
    .await?;
    let mut out = vec![];
    for (id, action, payload, diff, at) in found {
        let label = diff
            .as_ref()
            .map(|d| txt(d.get("label")))
            .unwrap_or_default();
        let hit = if ids.is_empty() {
            let text = format!("{action} {label} {payload}")
                .to_lowercase()
                .replace(['_', '.'], " ");
            matches(&text, &m.replace(['_', '.'], " "))
        } else {
            let sid = id.to_string();
            ids.iter()
                .any(|i| *i == sid || (i.len() >= 8 && sid.starts_with(i.as_str())))
        };
        if hit {
            out.push(
                json!({"card_id": id.to_string(), "action": action, "summary": label,
                            "created_at": at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)}),
            );
        }
        if out.len() >= 5 {
            break;
        }
    }
    Ok(out)
}

// ─── One waiting quotation/invoice card per deal or customer ───────────────────

fn same_draft_target(old: &Map<String, Value>, new: &Map<String, Value>) -> bool {
    let kind = txt(new.get("kind"));
    if txt(old.get("kind")) != kind {
        return false;
    }
    if kind == "nary_action" {
        // a quotation update replaces the older update of the same quotation
        return txt(new.get("action")) == "quotation.update"
            && txt(old.get("action")) == "quotation.update"
            && !txt(get(new, &["payload", "quotation_id"])).is_empty()
            && txt(get(old, &["payload", "quotation_id"]))
                == txt(get(new, &["payload", "quotation_id"]));
    }
    if !matches!(kind.as_str(), "quotation" | "invoice") {
        return false;
    }
    let same = |path: &[&str]| {
        let a = txt(get(old, path));
        !a.is_empty() && a == txt(get(new, path))
    };
    if same(&["payload", "deal_id"]) {
        return true;
    }
    // two different deals are two quotations, even for one customer
    let (od, nd) = (
        txt(get(old, &["payload", "deal_id"])),
        txt(get(new, &["payload", "deal_id"])),
    );
    if !od.is_empty() && !nd.is_empty() {
        return false;
    }
    if same(&["payload", "customer_org_id"]) || same(&["payload", "customer_contact_id"]) {
        return true;
    }
    let (oc, nc) = (
        txt(old.get("customer")).to_lowercase(),
        txt(new.get("customer")).to_lowercase(),
    );
    !oc.is_empty() && oc == nc
}

/// Whether a new card is one [`supersede_drafts`] looks at.
pub fn is_draft_card(new: &Map<String, Value>) -> bool {
    match new.get("kind").and_then(Value::as_str) {
        Some("quotation" | "invoice") => true,
        Some("nary_action") => {
            new.get("action").and_then(Value::as_str) == Some("quotation.update")
        }
        _ => false,
    }
}

/// `supersede_drafts(db, conversation_id, new)`: older waiting quotation (invoice) cards of this
/// conversation for the same deal or customer as `new`, and older updates of the same quotation, are marked
/// replaced. Returns their names.
pub async fn supersede_drafts(
    db: &mut PgConnection,
    conversation_id: Uuid,
    new: &Map<String, Value>,
) -> Result<Vec<String>, sqlx::Error> {
    if !is_draft_card(new) {
        return Ok(vec![]);
    }
    let mut replaced = vec![];
    let mut all = rows(&mut *db, conversation_id).await?;
    for (id, _, tc) in all.iter_mut() {
        let mut changed = false;
        if let Some(list) = cards_mut(tc) {
            for p in list.iter_mut() {
                let Some(p) = p.as_object_mut() else { continue };
                if status(p) == "pending" && same_draft_target(p, new) {
                    p.insert("status".into(), json!("superseded"));
                    let name = txt(Some(&card_name(p)));
                    let name = match (name.as_str(), txt(p.get("customer"))) {
                        ("quotation" | "invoice", c) if !c.is_empty() => format!("{name} for {c}"),
                        _ => name,
                    };
                    replaced.push(name);
                    changed = true;
                }
            }
        }
        if changed {
            write(&mut *db, *id, tc).await?;
        }
    }
    Ok(replaced)
}
