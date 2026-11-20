//! Port of backend/app/services/nary/intake.py: Web3forms intake (Nary Phase 2, item C): from a form
//! email in hello@ to a lead.
//!
//! `detect_and_queue()` runs inside the mail poller (crm_inbox._ingest): it checks the envelope and,
//! for a Web3forms email, emits intake.form in the same transaction. `process_form()` is the subscriber,
//! after commit:
//!   1. parse the text/plain fields; Haiku extracts them when required ones are missing; the email field
//!      is checked against Reply-To; anything unexpected warns the Owner (format drift);
//!   2. a lead with every field kept (raw_channel_data), attribution (first_referrer, landing page,
//!      UTM), country and currency from the phone code and time zone, and interested_in mapped to a
//!      product family as a hint; a repeat submission merges into the open lead instead;
//!   3. the notification thread is linked to the lead's contact (via Reply-To), never auto-linked to
//!      the Web3forms sender;
//!   4. routing by submission_source: an email journey gets an acknowledgement by email (a new email,
//!      never "Re: New Lead..."); a [whatsapp] form waits for its chat (an expected arrival; nothing is
//!      sent, and the Owner alone hears if it never comes). The chat may already be here (both orders).
//!
//! Python mutates ORM objects and lets the session flush them; here every such change is an UPDATE
//! written at the point Python makes it (with `updated_at = now()`, SQLAlchemy's onupdate), and
//! `db.commit()` in the middle of a function is `nary::commit` (COMMIT; BEGIN inside a transaction).

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use granary_compat::pyjson::{self, PyDateTime, PyValue};
use granary_compat::text::pystr;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_int, require_json};
use granary_db::rows::crm::{Contact, Lead, Organization};
use granary_db::rows::crm_inbox::{CrmEmailMessage, CrmEmailThread};
use granary_db::rows::nary::{AgentRun, DomainEvent, NaryExpectedArrival, WaChat};

use super::{commit, identity_match, phones, runs, triage, web3forms};
use crate::ai::base::ChatRequest;
use crate::ai::registry::{use_ai, UseAiOptions};

pub const WHERE: &str = "Nary & AI";

// ─── Small Python helpers ─────────────────────────────────────────────────────

/// Python truthiness of a JSON value.
pub(crate) fn truthy(v: &Value) -> bool {
    crate::whatsapp::py_truthy(v)
}

/// `str(v)` of a JSON value as Python holds it.
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => pyjson::str_value(&PyValue::from(other)),
    }
}

/// `fields.get(k)` as a str ("" when missing or falsy; values are str).
fn fget(fields: &Map<String, Value>, k: &str) -> String {
    match fields.get(k) {
        Some(v) if truthy(v) => py_str(v),
        _ => String::new(),
    }
}

/// `fields.get(k) or None`.
fn fopt(fields: &Map<String, Value>, k: &str) -> Option<String> {
    Some(fget(fields, k)).filter(|s| !s.is_empty())
}

/// `str(datetime)` of an aware UTC datetime from the database.
pub(crate) fn str_dt(dt: &DateTime<Utc>) -> String {
    PyDateTime::from_chrono_utc(dt).isoformat_sep(' ')
}

fn str_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().map(py_str).collect(),
        Value::Object(m) => m.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

fn chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `str.partition(" ")`.
fn partition_space(s: &str) -> (String, String) {
    match s.split_once(' ') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (s.to_string(), String::new()),
    }
}

// ─── urllib.parse (Python 3.12) ───────────────────────────────────────────────

/// What `urlsplit` gives (only the parts used here), or ValueError.
pub(crate) struct Split {
    pub netloc: String,
    pub query: String,
}

/// `urllib.parse.urlsplit(url)` (allow_fragments=True): netloc and query, or the ValueError text.
pub(crate) fn urlsplit(url: &str) -> Result<Split, String> {
    // _WHATWG_C0_CONTROL_OR_SPACE stripped on the left, then tab/CR/LF removed everywhere.
    let url: String = url
        .trim_start_matches(|c: char| (c as u32) <= 0x20)
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut rest: &str = &url;
    if let Some(i) = rest.find(':') {
        let first = rest.chars().next().unwrap();
        if i > 0 && first.is_ascii() && first.is_ascii_alphabetic() {
            let scheme = &rest[..i];
            if scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
            {
                rest = &rest[i + 1..];
            }
        }
    }
    let mut netloc = String::new();
    if rest.starts_with("//") {
        let after = &rest[2..];
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..end].to_string();
        rest = &after[end..];
        let (ob, cb) = (netloc.contains('['), netloc.contains(']'));
        if (ob && !cb) || (cb && !ob) {
            return Err("Invalid IPv6 URL".into());
        }
        if ob && cb {
            let host = netloc.split_once('[').unwrap().1;
            let host = host.split_once(']').map(|x| x.0).unwrap_or(host);
            check_bracketed_host(host)?;
        }
    }
    let rest = match rest.split_once('#') {
        Some((a, _)) => a,
        None => rest,
    };
    let query = rest
        .split_once('?')
        .map(|x| x.1.to_string())
        .unwrap_or_default();
    Ok(Split { netloc, query })
}

fn check_bracketed_host(host: &str) -> Result<(), String> {
    if let Some(stripped) = host.strip_prefix('v') {
        // \Av[a-fA-F0-9]+\..+\z
        let ok = match stripped.split_once('.') {
            Some((hex, tail)) => {
                !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) && !tail.is_empty()
            }
            None => false,
        };
        if !ok {
            return Err("IPvFuture address is invalid".into());
        }
        return Ok(());
    }
    // ipaddress.ip_address(host): an IPv4 address is rejected too ("... is not an IPv6 address" text
    // differs per case; only the fact that it raises matters to callers here).
    let (addr, scope) = match host.split_once('%') {
        Some((a, s)) => (a, Some(s)),
        None => (host, None),
    };
    if scope == Some("") || addr.parse::<std::net::Ipv6Addr>().is_err() {
        return Err(format!(
            "'{host}' does not appear to be an IPv4 or IPv6 address"
        ));
    }
    Ok(())
}

/// `urllib.parse.unquote(s.replace("+", " "))` (utf-8, errors="replace").
fn unquote_plus(s: &str) -> String {
    let s = s.replace('+', " ");
    if !s.contains('%') {
        return s;
    }
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            let h = &b[i + 1..i + 3];
            if h.iter().all(|c| c.is_ascii_hexdigit()) {
                out.push(u8::from_str_radix(std::str::from_utf8(h).unwrap(), 16).unwrap());
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `parse_qs(query)` (keep_blank_values=False, separator "&"), insertion-ordered.
fn parse_qs(query: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for nv in query.split('&') {
        if nv.is_empty() {
            continue;
        }
        let Some((n, v)) = nv.split_once('=') else {
            continue;
        };
        if v.is_empty() {
            continue;
        }
        let (name, value) = (unquote_plus(n), unquote_plus(v));
        match out.iter_mut().find(|(k, _)| *k == name) {
            Some((_, vs)) => vs.push(value),
            None => out.push((name, vec![value])),
        }
    }
    out
}

pub fn _utm(url: Option<&str>) -> Vec<(String, String)> {
    let Some(url) = url.filter(|u| !u.is_empty()) else {
        return Vec::new();
    };
    let Ok(sp) = urlsplit(url) else {
        return Vec::new();
    };
    parse_qs(&sp.query)
        .into_iter()
        .filter(|(k, v)| k.starts_with("utm_") && !v.is_empty())
        .map(|(k, v)| (k, v[0].clone()))
        .collect()
}

/// Where the lead came from: first_referrer (chatgpt.com, google...), the landing page and UTM.
/// Err is the ValueError urlparse raises on a malformed referrer (Python does not catch it).
pub fn attribution(fields: &Map<String, Value>) -> ApiResult<Map<String, Value>> {
    let landing = fopt(fields, "landing_page");
    let utm = _utm(landing.as_deref());
    let refr = pystr::strip(&fget(fields, "first_referrer")).to_string();
    let ref_host: Option<String> = if refr.starts_with("http") {
        let sp =
            urlsplit(&refr).map_err(|e| ApiError::internal(anyhow::anyhow!("ValueError: {e}")))?;
        Some(pystr::lower(&sp.netloc))
    } else if refr.is_empty() {
        None
    } else {
        Some(refr.clone())
    };
    let mut out = Map::new();
    out.insert(
        "form".into(),
        fields
            .get("submission_source")
            .cloned()
            .unwrap_or(Value::Null),
    );
    out.insert("from_page".into(), json!(fopt(fields, "from_page")));
    out.insert("landing_page".into(), json!(landing));
    out.insert(
        "referrer".into(),
        json!(Some(refr.clone()).filter(|r| !r.is_empty())),
    );
    out.insert(
        "referrer_host".into(),
        json!(ref_host.filter(|h| !h.is_empty() && h != "(direct)")),
    );
    out.insert("timezone".into(), json!(fopt(fields, "timezone")));
    for (k, v) in utm {
        out.insert(k, Value::String(v));
    }
    Ok(out.into_iter().filter(|(_, v)| truthy(v)).collect())
}

/// The lead's description: the message, plus what the website's price configurator sent
/// (request_type, pricing_config) so a quotation request carries its configuration.
pub fn _with_config(fields: &Map<String, Value>) -> Option<String> {
    let mut parts = vec![pystr::strip(&fget(fields, "message")).to_string()];
    if let Some(rt) = fopt(fields, "request_type") {
        parts.push(format!("Request type: {rt}"));
    }
    if let Some(pc) = fopt(fields, "pricing_config") {
        parts.push(format!("Configuration chosen on the website: {pc}"));
    }
    let s = parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(s).filter(|s| !s.is_empty())
}

// ─── The poller's side ────────────────────────────────────────────────────────

/// Called by the mail poller for each new inbound email. True when it was a Web3forms form (then
/// `msg.auth["web3forms"]` is set in memory, the caller saves it, and intake.form is emitted).
pub async fn detect_and_queue(
    db: &mut PgConnection,
    msg: &mut CrmEmailMessage,
    thread: &CrmEmailThread,
) -> ApiResult<bool> {
    let sender_domains = str_list(&require_json(db, "nary.web3forms.sender_domains", WHERE).await?);
    let dkim_domains = str_list(&require_json(db, "nary.web3forms.dkim_domains", WHERE).await?);
    let Some(found) = web3forms::detect(
        &msg.from_email,
        msg.auth.as_ref(),
        &sender_domains,
        &dkim_domains,
    ) else {
        return Ok(false);
    };
    let mut auth = auth_map(msg.auth.as_ref());
    auth.insert("web3forms".into(), found.clone());
    msg.auth = Some(Value::Object(auth));
    emit(
        db,
        "intake.form",
        Some("email_message"),
        Some(msg.id),
        json!({"thread_id": thread.id.to_string(), "verified": found["verified"]}),
    )
    .await?;
    Ok(true)
}

/// `{**(msg.auth or {})}`.
pub(crate) fn auth_map(auth: Option<&Value>) -> Map<String, Value> {
    match auth {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    }
}

/// The `@events.subscribe` registrations of this module. Split (29 Oct 2026): while Nary is paused the form still
/// becomes a lead ([`process_form_quiet`]); the acknowledgement and an identity card wait for the resume
/// ([`resume_form`]).
pub fn subscriptions() -> Vec<super::events::Subscription> {
    use super::events::{subscriber, Subscription};
    vec![Subscription::split(
        &["intake.form"],
        subscriber(|db, ev| Box::pin(process_form(db, ev))),
        subscriber(|db, ev| Box::pin(process_form_quiet(db, ev))),
        subscriber(|db, ev| Box::pin(resume_form(db, ev))),
    )]
}

// ─── The subscriber ───────────────────────────────────────────────────────────

async fn get_msg(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<CrmEmailMessage>> {
    Ok(
        sqlx::query_as("SELECT * FROM crm_email_messages WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

async fn get_thread(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<CrmEmailThread>> {
    Ok(
        sqlx::query_as("SELECT * FROM crm_email_threads WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

async fn get_lead(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<Lead>> {
    Ok(sqlx::query_as("SELECT * FROM leads WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?)
}

/// Subscriber of "intake.form".
pub async fn process_form(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    process_form_in(db, event, false).await
}

/// "intake.form" while Nary is paused: the lead, its contact and attribution, a merge into their open lead, the
/// triage and the Owner's drift warning; nothing goes to the visitor and no Nary card is raised (they are deferred).
pub async fn process_form_quiet(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    process_form_in(db, event, true).await
}

async fn process_form_in(db: &mut PgConnection, event: &DomainEvent, quiet: bool) -> ApiResult<()> {
    let Some(id) = event.entity_id else {
        return Ok(());
    };
    let Some(mut msg) = get_msg(db, id).await? else {
        return Ok(());
    };
    let lead_id = auth_map(msg.auth.as_ref())
        .get("web3forms")
        .and_then(|w| w.get("lead_id"))
        .map(truthy)
        .unwrap_or(false);
    if lead_id {
        return Ok(());
    }
    let mut thread = get_thread(db, msg.thread_id).await?;
    let mut run = runs::start_run(
        db,
        "event",
        "nary.intake",
        Some(&event.id.to_string()),
        "internal",
        None,
    )
    .await?;
    let quiet = quiet.then_some(event.id);
    let out = process_in(db, &mut msg, thread.as_mut(), Some(&mut run), quiet).await?;
    let summary = out.get("summary").map(py_str);
    runs::finish_run(db, &mut run, "done", summary.as_deref(), None).await?;
    Ok(())
}

async fn save_thread_links(db: &mut PgConnection, thread: &CrmEmailThread) -> ApiResult<()> {
    sqlx::query(
        "UPDATE crm_email_threads SET linked_contact_id = $2, linked_org_id = $3, linked_lead_id = $4, \
         linked_deal_id = $5, updated_at = now() WHERE id = $1",
    )
    .bind(thread.id)
    .bind(thread.linked_contact_id)
    .bind(thread.linked_org_id)
    .bind(thread.linked_lead_id)
    .bind(thread.linked_deal_id)
    .execute(&mut *db)
    .await?;
    Ok(())
}

pub(crate) async fn save_msg_auth(db: &mut PgConnection, msg: &CrmEmailMessage) -> ApiResult<()> {
    sqlx::query("UPDATE crm_email_messages SET auth = $2, updated_at = now() WHERE id = $1")
        .bind(msg.id)
        .bind(&msg.auth)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// `json.loads(json.dumps(value, default=str))` for the submission (received_at becomes `str()`).
fn submission_json(
    fields: &Map<String, Value>,
    msg: &CrmEmailMessage,
    problems: &[String],
) -> Value {
    let verified = auth_map(msg.auth.as_ref())
        .get("web3forms")
        .and_then(|w| w.get("verified").cloned())
        .unwrap_or(Value::Null);
    json!({
        "fields": fields,
        "message_id": msg.message_id_header,
        "received_at": msg.received_at.as_ref().map(str_dt),
        "problems": problems,
        "verified": verified,
    })
}

pub async fn process(
    db: &mut PgConnection,
    msg: &mut CrmEmailMessage,
    thread: Option<&mut CrmEmailThread>,
    run: Option<&mut AgentRun>,
) -> ApiResult<Value> {
    process_in(db, msg, thread, run, None).await
}

/// [`process`]; `quiet`: the event id while Nary is paused (the visitor's acknowledgement and any Nary card are
/// left for the resume pass of that event).
pub(crate) async fn process_in(
    db: &mut PgConnection,
    msg: &mut CrmEmailMessage,
    thread: Option<&mut CrmEmailThread>,
    mut run: Option<&mut AgentRun>,
    quiet: Option<Uuid>,
) -> ApiResult<Value> {
    let routes = require_json(db, "nary.web3forms.routes", WHERE).await?;
    let known = str_list(&require_json(db, "nary.web3forms.known_fields", WHERE).await?);
    let required = str_list(&require_json(db, "nary.web3forms.required_fields", WHERE).await?);
    let interest_map = require_json(db, "nary.web3forms.interest_map", WHERE).await?;
    let recent_days = require_int(db, "nary.identity.recent_form_days", WHERE).await?;

    let mut text = msg.body_text.clone().unwrap_or_default();
    if pystr::strip(&text).is_empty() {
        if let Some(html) = msg.body_html.as_deref().filter(|h| !h.is_empty()) {
            text = crate::crm_inbox::_html_to_text(html); // only as input for the AI fallback below
        }
    }
    let body_text = msg.body_text.clone().filter(|t| !t.is_empty());
    let parsed = web3forms::parse_text(msg.body_text.as_deref());
    let chk = web3forms::check(&parsed, msg.reply_to.as_deref(), &known, &required, &routes);
    let mut fields = parsed.fields_json();
    let mut used_ai = false;
    if !chk.missing.is_empty() || body_text.is_none() {
        let extracted = _extract_with_ai(db, &text, run.as_deref_mut()).await?;
        for (k, v) in extracted {
            if !fields.get(&k).map(truthy).unwrap_or(false) {
                fields.insert(k, Value::String(v));
            }
        }
        used_ai = true;
    }
    let mut problems = chk.problems.clone();
    if body_text.is_none() {
        problems.insert(0, "the email had no plain-text part".into());
    }
    if used_ai {
        problems.push("the fields were read by the AI fallback; check them".into());
    }
    if pystr::strip(&fget(&fields, "email")).is_empty() {
        if let Some(rt) = msg.reply_to.as_deref().filter(|r| !r.is_empty()) {
            fields.insert(
                "email".into(),
                Value::String(pystr::lower(pystr::strip(rt))),
            ); // Reply-To is the lead's address
        }
    }
    if !problems.is_empty() {
        let mut keys: Vec<String> = chk
            .missing
            .iter()
            .chain(chk.unknown.iter())
            .cloned()
            .collect();
        keys.sort();
        keys.dedup();
        let link = match &thread {
            Some(t) => format!("/crm/email?thread={}", t.id),
            None => "/crm/email".to_string(),
        };
        super::governance::error_card(
            db,
            &format!("web3forms.drift:{}", keys.join(",")),
            "The website form email looks different",
            &format!(
                "Nary still made the lead, but check it: {}.",
                problems.join("; ")
            ),
            &link,
        )
        .await?;
    }

    let submission = submission_json(&fields, msg, &problems);
    let fl = form_lead(
        db,
        &fields,
        "web3forms",
        submission,
        thread,
        &routes,
        &interest_map,
        recent_days,
    )
    .await?;
    let lead = &fl.lead;
    let mut auth = auth_map(msg.auth.as_ref());
    let mut w3 = match auth.get("web3forms") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    w3.insert("lead_id".into(), Value::String(lead.id.to_string()));
    auth.insert("web3forms".into(), Value::Object(w3));
    msg.auth = Some(Value::Object(auth));
    save_msg_auth(db, msg).await?;
    form_finish(
        db,
        &fields,
        fl,
        problems,
        msg.received_at.unwrap_or_else(Utc::now),
        run,
        quiet,
    )
    .await
}

/// What [`form_lead`] made: the lead (new or merged into), its journey and source, the visitor's number.
pub(crate) struct FormLead {
    pub lead: Lead,
    pub merged: bool,
    pub journey: String,
    pub source: Option<String>,
    pub e164: Option<String>,
}

/// The part of a form intake: the journey from submission_source, the contact, a new lead or a merge into their
/// open one, attribution. `raw_key` names the submission in a new lead's raw_channel_data ("web3forms").
#[allow(clippy::too_many_arguments)]
pub(crate) async fn form_lead(
    db: &mut PgConnection,
    fields: &Map<String, Value>,
    raw_key: &str,
    submission: Value,
    mut thread: Option<&mut CrmEmailThread>,
    routes: &Value,
    interest_map: &Value,
    recent_days: i64,
) -> ApiResult<FormLead> {
    let fields = fields.clone();
    // C14: an inline_* variant of a known form ('inline_id_web3forms') is routed as that form
    let source = Some(pystr::strip(&fget(&fields, "submission_source")).to_string())
        .filter(|s| !s.is_empty())
        .map(|s| web3forms::route_key(&s, routes).unwrap_or(s));
    let journey = match routes {
        Value::Object(m) => m
            .get(source.as_deref().unwrap_or(""))
            .map(py_str)
            .unwrap_or_else(|| "email".into()),
        _ => {
            return Err(ApiError::internal(anyhow::anyhow!(
                "AttributeError: nary.web3forms.routes has no attribute 'get'"
            )))
        }
    };
    let country_default = phones::default_country(db).await?;
    let e164 = phones::to_e164(fopt(&fields, "phone").as_deref(), &country_default);
    let locale =
        phones::derive_locale(db, e164.as_deref(), fopt(&fields, "timezone").as_deref()).await?;
    let country = locale
        .get("country")
        .and_then(|c| c.as_str())
        .map(str::to_string);
    let contact = _contact(db, &fields, e164.as_deref(), country.as_deref()).await?;
    if let Some(t) = thread.as_deref_mut() {
        t.linked_contact_id = Some(contact.id);
        t.linked_org_id = contact.organization_id;
        save_thread_links(db, t).await?;
    }
    let interest_code = fopt(&fields, "interested_in");
    let family: Option<Value> = match &interest_code {
        Some(code) => match interest_map {
            Value::Object(m) => m.get(code).cloned(),
            v if !truthy(v) => None,
            _ => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "AttributeError: nary.web3forms.interest_map has no attribute 'get'"
                )))
            }
        },
        None => None,
    }
    .filter(|f| !f.is_null());

    // the segment the form names (crate::segment), when it is a valid one
    let segment = crate::segment::from_form(db, &fields).await?;
    let existing = _open_intake(db, &contact, recent_days).await?;
    let merged = existing.is_some();
    let lead: Lead = if let Some(lead) = existing {
        let mut data = match &lead.raw_channel_data {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        // data.setdefault("submissions", []).append(...)
        match data.get_mut("submissions") {
            Some(Value::Array(a)) => a.push(submission.clone()),
            Some(_) => {
                return Err(ApiError::internal(anyhow::anyhow!(
                    "AttributeError: 'submissions' has no attribute 'append'"
                )))
            }
            None => {
                data.insert("submissions".into(), Value::Array(vec![submission.clone()]));
            }
        }
        let raw_message = [lead.raw_message.clone(), fopt(&fields, "message")]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        sqlx::query(
            "UPDATE leads SET raw_channel_data = $2, raw_message = $3, updated_at = now() WHERE id = $1",
        )
        .bind(lead.id)
        .bind(Value::Object(data))
        .bind(&raw_message)
        .execute(&mut *db)
        .await?;
        // the form names a segment and the lead has none yet: the lead takes it
        if segment.is_some() && lead.segment.is_none() {
            sqlx::query("UPDATE leads SET segment = $2 WHERE id = $1")
                .bind(lead.id)
                .bind(&segment)
                .execute(&mut *db)
                .await?;
        }
        if let Some(deal_id) = lead.converted_to_deal_id {
            // it's a deal now: the new submission shows on the deal
            let now = Utc::now();
            sqlx::query(
                "INSERT INTO activities (id, entity_type, entity_id, deal_id, contact_id, activity_type, direction, \
                 subject, content, is_completed, completed_at, scheduled_at) \
                 VALUES ($1,'deal',$2,$2,$3,'note','inbound','They sent the website form again',$4,true,$5,$5)",
            )
            .bind(Uuid::new_v4())
            .bind(deal_id)
            .bind(contact.id)
            .bind(_with_config(&fields).unwrap_or_default())
            .bind(now)
            .execute(&mut *db)
            .await?;
        }
        get_lead(db, lead.id).await?.unwrap_or(lead)
    } else {
        // `except Exception`: captured unowned; the brief shows it
        let owner = crate::crm::stages::default_lead_owner_id(db).await.ok();
        let lead_source = match source.as_deref() {
            Some("inline_whatsapp") => "whatsapp".to_string(),
            Some(s @ ("web3forms" | "inline_web3forms" | "whatsapp")) => s.to_string(),
            _ => "website_form".to_string(),
        };
        let who = fopt(&fields, "company")
            .or_else(|| fopt(&fields, "name"))
            .unwrap_or_else(|| "Web form".into());
        let what = family
            .as_ref()
            .filter(|f| truthy(f))
            .map(py_str)
            .or_else(|| interest_code.clone())
            .unwrap_or_else(|| "enquiry".into());
        let title = chars(&format!("{who}: {what}"), 255);
        let products: Vec<String> = match &family {
            Some(f) if truthy(f) => vec![py_str(f)],
            _ => vec![],
        };
        let raw_channel_data = {
            let mut m = Map::new();
            m.insert(raw_key.to_string(), submission.clone());
            m.insert("interested_in".into(), json!(interest_code));
            m.insert("locale".into(), json!(locale));
            Value::Object(m)
        };
        let lead: Lead = sqlx::query_as(
            "INSERT INTO leads (id, contact_id, organization_id, source, source_detail, title, description, \
             raw_message, interested_products, currency, status, priority, assigned_to_id, attribution, \
             raw_channel_data, custom, is_deleted, segment) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'new','medium',$11,$12,$13,'{}'::jsonb,false,$14) RETURNING *",
        )
        .bind(Uuid::new_v4())
        .bind(contact.id)
        .bind(contact.organization_id)
        .bind(&lead_source)
        .bind(fopt(&fields, "from_page"))
        .bind(&title)
        .bind(_with_config(&fields))
        .bind(fopt(&fields, "message"))
        .bind(&products)
        .bind(locale.get("currency").filter(|c| !c.is_null()).map(py_str))
        .bind(owner)
        .bind(Value::Object(attribution(&fields)?))
        .bind(&raw_channel_data)
        .bind(&segment)
        .fetch_one(&mut *db)
        .await?;
        _attribution_row(db, &lead, &fields).await?;
        emit(
            db,
            "lead.created",
            Some("lead"),
            Some(lead.id),
            json!({"source": lead_source, "status": "new", "origin": "web3forms"}),
        )
        .await?;
        lead
    };
    if let Some(t) = thread {
        t.linked_lead_id = Some(lead.id);
        save_thread_links(db, t).await?;
    }
    Ok(FormLead {
        lead,
        merged,
        journey,
        source,
        e164,
    })
}

/// The rest of a form intake, after the lead is saved: a [whatsapp] form waits for its chat; commit; a new
/// lead is triaged and, on the email journey, acknowledged by email.
///
/// `quiet` (29 Oct 2026): the event id while Nary is paused. The lead, the expected chat and the triage happen as
/// always; the acknowledgement and an identity card are not made (the pause refuses Nary's cards) but deferred to
/// the event's resume pass ([`resume_form`]).
pub(crate) async fn form_finish(
    db: &mut PgConnection,
    fields: &Map<String, Value>,
    fl: FormLead,
    problems: Vec<String>,
    received_at: DateTime<Utc>,
    mut run: Option<&mut AgentRun>,
    quiet: Option<Uuid>,
) -> ApiResult<Value> {
    let FormLead {
        lead,
        merged,
        journey,
        source,
        e164,
    } = fl;
    let fields = fields.clone();

    let summary = format!(
        "{} lead {} ({}, {journey})",
        if merged { "merged into" } else { "new" },
        lead.title
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| "None".into()),
        source.as_deref().unwrap_or("unknown source"),
    );
    if journey == "whatsapp" && !merged {
        _expect_chat(db, &lead, &fields, e164.as_deref(), quiet).await?;
    }
    commit(db).await?;

    if !merged {
        let verdict = _triage_form(db, &lead, &fields, run.as_deref_mut()).await?;
        if let Some(v) = verdict {
            let email = fopt(&fields, "email");
            if journey == "email"
                && triage::BUSINESS.contains(&v.verdict.as_str())
                && email.is_some()
            {
                if let Some(event_id) = quiet {
                    // Nary is paused: nothing goes to the visitor now; the resume pass acknowledges a fresh one
                    super::events::defer(
                        db,
                        event_id,
                        FORM_ACK,
                        json!({"lead_id": lead.id.to_string(), "to": email, "name": fopt(&fields, "name"),
                               "language": v.language, "received_at": received_at.to_rfc3339()}),
                    )
                    .await?;
                    commit(db).await?;
                    return Ok(
                        json!({"lead_id": lead.id.to_string(), "merged": merged, "journey": journey,
                                     "summary": format!("{summary}; acknowledgement waits for Nary"),
                                     "problems": problems}),
                    );
                }
                let lead = get_lead(db, lead.id).await?.unwrap_or(lead.clone());
                super::ack::send_for_email(
                    db,
                    &lead,
                    email.as_deref().unwrap(),
                    fopt(&fields, "name").as_deref(),
                    v.language.as_deref(),
                    received_at,
                    run.as_deref(),
                    None,
                    None,
                    None,
                )
                .await?;
            }
        }
    }
    Ok(
        json!({"lead_id": lead.id.to_string(), "merged": merged, "journey": journey, "summary": summary,
              "problems": problems}),
    )
}

/// The AI fallback: `{k: str(v).strip() for k, v in out.items() if v}`, {} on unreadable JSON.
async fn _extract_with_ai(
    db: &mut PgConnection,
    text: &str,
    run: Option<&mut AgentRun>,
) -> ApiResult<Vec<(String, String)>> {
    let head = chars(text, 8000);
    let answer = extract_chat(db, &head, run).await?;
    let out: Value = match serde_json::from_str(if answer.is_empty() { "{}" } else { &answer }) {
        Ok(v) => v,
        Err(_) => return Ok(Vec::new()),
    };
    match out {
        Value::Object(m) => Ok(m
            .into_iter()
            .filter(|(_, v)| truthy(v))
            .map(|(k, v)| (k, pystr::strip(&py_str(&v)).to_string()))
            .collect()),
        // `out.items()` on a non-dict raises AttributeError, which `except ValueError` does not catch.
        other => Err(ApiError::internal(anyhow::anyhow!(
            "AttributeError: '{}' object has no attribute 'items'",
            match other {
                Value::Array(_) => "list",
                Value::String(_) => "str",
                Value::Bool(_) => "bool",
                Value::Null => "NoneType",
                Value::Number(n) if n.is_f64() => "float",
                _ => "int",
            }
        ))),
    }
}

/// The organisation of a company name: the one whose name matches it (legal forms and case ignored), else a new
/// prospect organisation. None for an empty name. (Shared by the web form intake and, 3 Nov 2026, the MCP
/// tool propose_whatsapp_lead.)
pub async fn organization_for(
    db: &mut PgConnection,
    company: &str,
    country: Option<&str>,
) -> ApiResult<Option<Organization>> {
    let company = pystr::strip(company).to_string();
    if company.is_empty() {
        return Ok(None);
    }
    let mut org: Option<Organization> = None;
    let target = identity_match::clean_company(Some(&company));
    if !target.is_empty() {
        let first = pystr::split(&target)
            .first()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let rows: Vec<Organization> = sqlx::query_as(
            "SELECT * FROM organizations WHERE is_deleted = false AND lower(name) LIKE $1 ESCAPE '\\' LIMIT 20",
        )
        // The first word is taken literally: `_` and `%` in it are not LIKE wildcards.
        .bind(format!("%{}%", granary_compat::text::utils::escape_like(&first)))
        .fetch_all(&mut *db)
        .await?;
        org = rows
            .into_iter()
            .find(|o| identity_match::clean_company(Some(&o.name)) == target);
    }
    if org.is_none() {
        org = Some(
            sqlx::query_as(
                "INSERT INTO organizations (id, name, country, type, revenue_currency, tags, custom, is_deleted) \
                 VALUES ($1,$2,$3,ARRAY['prospect'],'USD','{}','{}'::jsonb,false) RETURNING *",
            )
            .bind(Uuid::new_v4())
            .bind(chars(&company, 255))
            .bind(country)
            .fetch_one(&mut *db)
            .await?,
        );
    }
    Ok(org)
}

/// The lead's contact: by email, else by number, else a new one (with an organisation).
pub async fn _contact(
    db: &mut PgConnection,
    fields: &Map<String, Value>,
    e164: Option<&str>,
    country: Option<&str>,
) -> ApiResult<Contact> {
    let email = pystr::lower(pystr::strip(&fget(fields, "email")));
    let mut contact: Option<Contact> = None;
    if !email.is_empty() {
        contact = sqlx::query_as(
            "SELECT * FROM contacts WHERE lower(email) = $1 AND is_deleted = false LIMIT 1",
        )
        .bind(&email)
        .fetch_optional(&mut *db)
        .await?;
    }
    if contact.is_none() {
        if let Some(e) = e164.filter(|e| !e.is_empty()) {
            let d = phones::digits(Some(e));
            contact = crate::crm_match::resolve_contact_for_phone(db, Some(&d)).await;
        }
    }
    if let Some(mut c) = contact {
        let mut changed = false;
        if !email.is_empty() && c.email.as_deref().unwrap_or("").is_empty() {
            c.email = Some(email.clone());
            changed = true;
        }
        if let Some(e) = e164.filter(|e| !e.is_empty()) {
            if c.whatsapp.as_deref().unwrap_or("").is_empty() {
                c.whatsapp = Some(e.to_string());
                changed = true;
            }
        }
        if changed {
            sqlx::query(
                "UPDATE contacts SET email = $2, whatsapp = $3, updated_at = now() WHERE id = $1",
            )
            .bind(c.id)
            .bind(&c.email)
            .bind(&c.whatsapp)
            .execute(&mut *db)
            .await?;
        }
        return Ok(c);
    }
    let company = pystr::strip(&fget(fields, "company")).to_string();
    let org = organization_for(db, &company, country).await?;
    let name_src = fopt(fields, "name").unwrap_or_else(|| {
        if email.is_empty() {
            "Web form".into()
        } else {
            email.clone()
        }
    });
    let name = pystr::strip(&name_src).to_string();
    let (first, last) = partition_space(&name);
    let last = chars(&last, 100);
    let c: Contact = sqlx::query_as(
        "INSERT INTO contacts (id, first_name, last_name, email, phone, whatsapp, country, organization_id, \
         is_primary, tags, custom, is_deleted) \
         VALUES ($1,$2,$3,$4,$5,$5,$6,$7,false,'{}','{}'::jsonb,false) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(chars(&first, 100))
    .bind(Some(last).filter(|l| !l.is_empty()))
    .bind(Some(email).filter(|e| !e.is_empty()))
    .bind(e164)
    .bind(country)
    .bind(org.as_ref().map(|o| o.id))
    .fetch_one(&mut *db)
    .await?;
    Ok(c)
}

/// An open lead for this person (or their company) created recently: a repeat submission merges
/// into it instead of starting another.
async fn _open_intake(
    db: &mut PgConnection,
    contact: &Contact,
    days: i64,
) -> ApiResult<Option<Lead>> {
    let active: Vec<String> = crate::crm::stages::ACTIVE_LEAD_STATUSES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let since = Utc::now() - Duration::days(days);
    let base =
        "SELECT * FROM leads WHERE is_deleted = false AND status = ANY($1) AND created_at >= $2";
    let lead: Option<Lead> = sqlx::query_as(&format!(
        "{base} AND contact_id = $3 ORDER BY created_at DESC LIMIT 1"
    ))
    .bind(&active)
    .bind(since)
    .bind(contact.id)
    .fetch_optional(&mut *db)
    .await?;
    let lead = match (lead, contact.organization_id) {
        (None, Some(org)) => {
            sqlx::query_as(&format!(
                "{base} AND organization_id = $3 ORDER BY created_at DESC LIMIT 1"
            ))
            .bind(&active)
            .bind(since)
            .bind(org)
            .fetch_optional(&mut *db)
            .await?
        }
        (l, _) => l,
    };
    if lead.is_some() {
        return Ok(lead);
    }
    // Any open lead of theirs, however old, or the lead behind their open deal (12 Oct: a customer's form made a
    // second lead beside the deal their email had become).
    let (lead_id, deal_id) = identity_match::open_opportunity(db, contact).await?;
    if let Some(l) = lead_id {
        return get_lead(db, l).await;
    }
    if let Some(d) = deal_id {
        return Ok(sqlx::query_as(
            "SELECT * FROM leads WHERE converted_to_deal_id = $1 AND is_deleted = false ORDER BY created_at LIMIT 1",
        )
        .bind(d)
        .fetch_optional(&mut *db)
        .await?);
    }
    Ok(None)
}

/// The marketing attribution table (never written before Phase 2).
async fn _attribution_row(
    db: &mut PgConnection,
    lead: &Lead,
    fields: &Map<String, Value>,
) -> ApiResult<()> {
    let utm = _utm(fopt(fields, "landing_page").as_deref());
    let get = |k: &str| utm.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    sqlx::query(
        "INSERT INTO lead_attributions (id, lead_id, utm_source, utm_medium, utm_campaign, referrer_url, landing_page, \
         attribution_type) VALUES ($1,$2,$3,$4,$5,$6,$7,'first_touch')",
    )
    .bind(Uuid::new_v4())
    .bind(lead.id)
    .bind(get("utm_source"))
    .bind(get("utm_medium"))
    .bind(get("utm_campaign"))
    .bind(fopt(fields, "first_referrer"))
    .bind(fopt(fields, "landing_page").or_else(|| fopt(fields, "from_page")))
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// A [whatsapp] form: wait for the chat. If it is already here (the chat came first), link now.
async fn _expect_chat(
    db: &mut PgConnection,
    lead: &Lead,
    fields: &Map<String, Value>,
    e164: Option<&str>,
    quiet: Option<Uuid>,
) -> ApiResult<NaryExpectedArrival> {
    let minutes = require_int(db, "nary.intake.arrival_window_minutes", WHERE).await?;
    let now = Utc::now();
    let arrival: NaryExpectedArrival = sqlx::query_as(
        "INSERT INTO nary_expected_arrivals (id, lead_id, phone, email, name, company, message, due_at, status) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'pending') RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(lead.id)
    .bind(e164)
    .bind(fopt(fields, "email"))
    .bind(fopt(fields, "name"))
    .bind(fopt(fields, "company"))
    .bind(fopt(fields, "message"))
    .bind(now + Duration::minutes(minutes))
    .fetch_one(&mut *db)
    .await?;
    let recent =
        now - Duration::days(require_int(db, "nary.identity.recent_form_days", WHERE).await?);
    let mut chat: Option<WaChat> = None;
    if let Some(e) = e164.filter(|e| !e.is_empty()) {
        chat = sqlx::query_as(
            "SELECT * FROM wa_chats WHERE phone = $1 AND status = 'active' AND created_at >= $2 LIMIT 1",
        )
        .bind(e)
        .bind(recent)
        .fetch_optional(&mut *db)
        .await?;
    }
    if let Some(mut chat) = chat {
        let label = format!(
            "{} ({})",
            arrival
                .name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("?"),
            arrival
                .company
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("web form")
        );
        let mut cand = identity_match::Candidate::new("arrival", arrival.id, &label);
        cand.add(
            "form_after_chat",
            100.0,
            "the web form arrived after the chat, same number",
        );
        identity_match::link_chat(db, &mut chat, &cand, "certain", false, None, "linked").await?;
        return Ok(arrival);
    }
    // The chat may have come first without a number (a WhatsApp privacy id): score the recent chats that
    // are still unlinked, or only have the intake Nary made from them, against this form with the same
    // signals and thresholds as the chat-side matching.
    _match_form_to_chats(db, &arrival, recent, quiet).await?;
    Ok(arrival)
}

async fn _match_form_to_chats(
    db: &mut PgConnection,
    arrival: &NaryExpectedArrival,
    recent: DateTime<Utc>,
    quiet: Option<Uuid>,
) -> ApiResult<()> {
    let cfg = identity_match::settings_bundle(db).await?;
    let chats: Vec<WaChat> = sqlx::query_as(
        "SELECT * FROM wa_chats WHERE status = 'active' AND created_at >= $1 AND staff_user_id IS NULL \
         AND deal_id IS NULL ORDER BY created_at DESC LIMIT 50",
    )
    .bind(recent)
    .fetch_all(&mut *db)
    .await?;
    let mut best: Option<(WaChat, identity_match::Candidate, String)> = None;
    for c in chats {
        if c.lead_id.is_some()
            && super::identity_match::fresh_own_intake(db, &c)
                .await?
                .is_none()
        {
            continue; // the chat already belongs to a real lead or deal
        }
        let text = super::channels::conversation_text(db, &c, None, 12).await?;
        for cand in identity_match::candidates_for_chat(db, &c, Some(&text), &cfg, None).await? {
            if cand.kind == "arrival"
                && cand.target_id == arrival.id
                && best
                    .as_ref()
                    .map(|b| cand.score > b.1.score)
                    .unwrap_or(true)
            {
                best = Some((c.clone(), cand, text.clone()));
            }
        }
    }
    let Some((mut chat, cand, text)) = best else {
        return Ok(());
    };
    match identity_match::tier_for(cand.score, &cfg.thresholds) {
        Some("certain") => {
            identity_match::link_chat(db, &mut chat, &cand, "certain", false, None, "linked")
                .await?;
        }
        Some("likely") => {
            let (same, reason) =
                identity_match::review_likely(db, &chat, Some(&text), &cand, None).await?;
            if same {
                identity_match::link_chat(
                    db,
                    &mut chat,
                    &cand,
                    "likely",
                    true,
                    Some(&reason),
                    "linked",
                )
                .await?;
            } else {
                let link = identity_match::link_chat(
                    db,
                    &mut chat,
                    &cand,
                    "weak",
                    false,
                    Some(&reason),
                    "pending",
                )
                .await?;
                propose_identity(db, &link.id.to_string(), &chat, quiet).await?;
            }
        }
        Some("weak") => {
            let link =
                identity_match::link_chat(db, &mut chat, &cand, "weak", false, None, "pending")
                    .await?;
            propose_identity(db, &link.id.to_string(), &chat, quiet).await?;
        }
        _ => {}
    }
    Ok(())
}

/// The identity card of a weak chat match; while Nary is paused it waits for the resume pass (the pause refuses it).
async fn propose_identity(
    db: &mut PgConnection,
    link_id: &str,
    chat: &WaChat,
    quiet: Option<Uuid>,
) -> ApiResult<()> {
    match quiet {
        Some(event_id) => {
            super::events::defer(
                db,
                event_id,
                IDENTITY_LINK,
                json!({"link_id": link_id, "chat_id": chat.id.to_string()}),
            )
            .await
        }
        None => super::channels::_propose_identity(db, link_id, chat, None).await,
    }
}

/// The deferred kinds of a form intake.
pub const FORM_ACK: &str = "form.ack";
pub const IDENTITY_LINK: &str = "identity.link";

/// Resume pass of "intake.form" and "intake.website" (29 Oct 2026): what the intake left while Nary was paused.
/// The visitor's acknowledgement (only for a lead still open, and through the executor as always) and the
/// identity card of a chat match still pending. Each piece is marked done first: a customer message goes at most
/// once.
pub async fn resume_form(db: &mut PgConnection, event: &DomainEvent) -> ApiResult<()> {
    for d in super::events::deferred(db, event.id).await? {
        super::events::deferred_done(db, d.id).await?;
        commit(db).await?;
        let p = &d.payload;
        let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        match d.kind.as_str() {
            FORM_ACK => {
                let Some(lead_id) = s("lead_id").and_then(|v| Uuid::parse_str(&v).ok()) else {
                    continue;
                };
                let (Some(lead), Some(to)) = (get_lead(db, lead_id).await?, s("to")) else {
                    continue;
                };
                if lead.is_deleted
                    || matches!(lead.status.as_deref(), Some("disqualified" | "converted"))
                {
                    continue;
                }
                let received_at = s("received_at")
                    .and_then(|v| DateTime::parse_from_rfc3339(&v).ok())
                    .map(|t| t.with_timezone(&Utc))
                    .unwrap_or(d.created_at);
                let mut run = runs::start_run(
                    db,
                    "event",
                    "nary.intake",
                    Some(&event.id.to_string()),
                    "internal",
                    None,
                )
                .await?;
                let out = super::ack::send_for_email(
                    db,
                    &lead,
                    &to,
                    s("name").as_deref(),
                    s("language").as_deref(),
                    received_at,
                    Some(&run),
                    None,
                    None,
                    None,
                )
                .await?;
                let summary = format!(
                    "acknowledgement after Nary resumed: {}",
                    out.get("status")
                        .map(py_str)
                        .unwrap_or_else(|| "None".into())
                );
                runs::finish_run(db, &mut run, "done", Some(&summary), None).await?;
            }
            IDENTITY_LINK => {
                let (Some(link_id), Some(chat_id)) = (
                    s("link_id"),
                    s("chat_id").and_then(|v| Uuid::parse_str(&v).ok()),
                ) else {
                    continue;
                };
                let pending: Option<String> = sqlx::query_scalar(
                    "SELECT status FROM nary_identity_links WHERE id::text = $1",
                )
                .bind(&link_id)
                .fetch_optional(&mut *db)
                .await?;
                let chat: Option<WaChat> = sqlx::query_as("SELECT * FROM wa_chats WHERE id = $1")
                    .bind(chat_id)
                    .fetch_optional(&mut *db)
                    .await?;
                if let (Some("pending"), Some(chat)) = (pending.as_deref(), chat) {
                    super::channels::_propose_identity(db, &link_id, &chat, None).await?;
                }
            }
            _ => {}
        }
        commit(db).await?;
    }
    Ok(())
}

/// Forms are triaged too: a spam form is disqualified (reason spam) and never acknowledged.
async fn _triage_form(
    db: &mut PgConnection,
    lead: &Lead,
    fields: &Map<String, Value>,
    run: Option<&mut AgentRun>,
) -> ApiResult<Option<triage::Verdict>> {
    let text = fields
        .iter()
        .filter(|(k, v)| {
            ["name", "company", "interested_in", "message"].contains(&k.as_str()) && truthy(v)
        })
        .map(|(k, v)| format!("{k}: {}", py_str(v)))
        .collect::<Vec<_>>()
        .join("\n");
    let verdict = match triage::classify(
        db,
        &text,
        run,
        "This came through the website contact form.",
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            // the lead exists; triage can be redone
            tracing::warn!(lead_id = %lead.id, error = %e, "nary_form_triage_failed");
            return Ok(None);
        }
    };
    triage::record(
        db,
        "lead",
        &lead.id.to_string(),
        &verdict,
        &text,
        Some(lead.id),
    )
    .await?;
    let Some(cur) = get_lead(db, lead.id).await? else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "AttributeError: 'NoneType' object has no attribute 'status'"
        )));
    };
    let (status, reason) = match verdict.verdict.as_str() {
        "spam" | "scam" | "wrong_number" => ("disqualified", Some("spam")),
        "prospect" | "customer" => ("qualifying", None),
        _ => ("triaging", None),
    };
    let mut score = match &cur.ai_score {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    score.insert("triage".into(), Value::String(verdict.verdict.clone()));
    score.insert("language".into(), json!(verdict.language));
    let reason_sql = if reason.is_some() {
        "$3"
    } else {
        "disqualification_reason"
    };
    sqlx::query(&format!(
        "UPDATE leads SET status = $2, disqualification_reason = {reason_sql}, ai_score = $4, updated_at = now() \
         WHERE id = $1"
    ))
    .bind(cur.id)
    .bind(status)
    .bind(reason)
    .bind(Value::Object(score))
    .execute(&mut *db)
    .await?;
    commit(db).await?;
    Ok(Some(verdict))
}

/// `events.emit(db, type, entity_type=..., entity_id=..., payload=...)`.
async fn emit(
    db: &mut PgConnection,
    type_: &str,
    entity_type: Option<&str>,
    entity_id: Option<Uuid>,
    payload: Value,
) -> ApiResult<()> {
    super::events::emit(db, type_, entity_type, entity_id, Some(payload), None).await?;
    Ok(())
}

/// `use_ai(db, "nary.web3forms_extract", run_id=...)`, `ai.chat(...)` and the run step; the answer's text.
async fn extract_chat(
    db: &mut PgConnection,
    text: &str,
    run: Option<&mut AgentRun>,
) -> ApiResult<String> {
    let ai = use_ai(
        db,
        "nary.web3forms_extract",
        UseAiOptions::run(run.as_ref().map(|r| r.id)),
    )
    .await?;
    let req = ChatRequest::new(
        web3forms::EXTRACT_PROMPT,
        vec![json!({"role": "user", "content": text})],
    )
    .response_schema(web3forms::EXTRACT_SCHEMA.clone());
    let result = ai.chat(db, req).await?;
    if let Some(run) = run {
        let output = json!({"text": result.text});
        runs::add_step(
            db,
            run,
            "llm",
            Some("web3forms.extract"),
            runs::StepOpts {
                output: Some(&output),
                result: Some(&result),
                cost: ai.last_cost(),
                model: Some(ai.model()),
                ..Default::default()
            },
        )
        .await?;
    }
    Ok(result.text)
}
