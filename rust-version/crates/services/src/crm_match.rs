//! Port of backend/app/services/crm_match.py: resolve an inbound WhatsApp number to a CRM contact.
//!
//! Matching order:
//!   1. explicit `WaContactLink` override (keyed by normalized phone)
//!   2. a Contact whose `whatsapp` or `phone`, once digit-stripped, equals any accepted digit-form of the
//!      number (handles +CC / CC / 0 national prefix / spaces / dashes, CC = the default country code).
//!
//! Neither lookup ever fails: a database error is logged, the session rolled back (as Python's
//! `_safe_rollback` does) and the lookup answers "no match".

use std::collections::{HashMap, HashSet};

use granary_compat::text::utils::normalize_phone;
use granary_db::rows::crm::Contact;
use sqlx::PgConnection;
use uuid::Uuid;

/// `_digits(col)`: every non-digit stripped from a nullable text column.
fn digits(col: &str) -> String {
    format!("regexp_replace(coalesce({col}, ''), '[^0-9]', '', 'g')")
}

/// Accepted digit-only variants for a normalized number, e.g. with the default country code 44:
/// "447700900123" -> {"447700900123", "07700900123", "7700900123"}.
/// Python builds a set, so its list order is hash order (arbitrary); only membership matters to callers.
pub fn phone_digit_forms(norm: &str) -> Vec<String> {
    let mut forms: Vec<String> = vec![norm.to_string()];
    let cc = granary_compat::text::utils::default_country_code();
    if let Some(local) = norm.strip_prefix(cc).filter(|_| !cc.is_empty()) {
        if !local.is_empty() {
            for f in [format!("0{local}"), local.to_string()] {
                if !forms.contains(&f) {
                    forms.push(f);
                }
            }
        }
    }
    forms.into_iter().filter(|f| !f.is_empty()).collect()
}

/// `_safe_rollback(db)`: clears an aborted transaction so the next lookup can run. When the caller's
/// connection is inside a transaction (healthy or aborted) it rolls back and begins a new one, as the
/// SQLAlchemy session would; outside one there is nothing to do.
async fn _safe_rollback(db: &mut PgConnection) {
    let in_tx = match sqlx::query("SAVEPOINT granary_rollback_probe")
        .execute(&mut *db)
        .await
    {
        Ok(_) => true,
        Err(sqlx::Error::Database(e)) => e.code().as_deref() == Some("25P02"),
        Err(_) => false,
    };
    if in_tx {
        let _ = sqlx::query("ROLLBACK").execute(&mut *db).await;
        let _ = sqlx::query("BEGIN").execute(&mut *db).await;
    }
}

async fn contact_by_link(
    db: &mut PgConnection,
    norm: &str,
) -> Result<Option<Contact>, sqlx::Error> {
    let link: Option<(Uuid,)> =
        sqlx::query_as("SELECT contact_id FROM wa_contact_links WHERE phone = $1")
            .bind(norm)
            .fetch_optional(&mut *db)
            .await?;
    match link {
        Some((cid,)) => {
            sqlx::query_as::<_, Contact>(
                "SELECT * FROM contacts WHERE id = $1 AND is_deleted = false",
            )
            .bind(cid)
            .fetch_optional(&mut *db)
            .await
        }
        None => Ok(None),
    }
}

/// Best-effort contact match. Never raises: None on any error so the inbox keeps working even if CRM
/// matching has a problem.
pub async fn resolve_contact_for_phone(
    db: &mut PgConnection,
    raw_phone: Option<&str>,
) -> Option<Contact> {
    let norm = normalize_phone(raw_phone);
    if norm.is_empty() {
        return None;
    }

    // 1) explicit mapping override
    match contact_by_link(db, &norm).await {
        Ok(Some(c)) => return Some(c),
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(phone = %norm, error = %e, "wa mapping lookup failed");
            _safe_rollback(db).await;
        }
    }

    // 2) normalized field match (strip non-digits in SQL so stored formatting doesn't matter).
    // LIMIT 1 without ORDER BY, as Python: with several matches the first row Postgres returns wins.
    let forms = phone_digit_forms(&norm);
    let sql = format!(
        "SELECT * FROM contacts WHERE is_deleted = false AND ({} = ANY($1) OR {} = ANY($1)) LIMIT 1",
        digits("contacts.whatsapp"),
        digits("contacts.phone")
    );
    match sqlx::query_as::<_, Contact>(&sql)
        .bind(&forms)
        .fetch_optional(&mut *db)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(phone = %norm, error = %e, "contact phone match failed");
            _safe_rollback(db).await;
            None
        }
    }
}

/// One candidate of `resolve_many`: a contact id (Nary's own link) or a number / WhatsApp id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Candidate {
    Id(Uuid),
    Number(String),
}

/// Contacts for many chats at once (the inbox list: a few queries, not a few per chat).
///
/// `wanted` maps a key (the chat id) to candidates in priority order: a contact id, or a number / WhatsApp
/// id whose digits are matched like `resolve_contact_for_phone` does (explicit WaContactLink first, then
/// the contact's own fields). Never raises.
pub async fn resolve_many(
    db: &mut PgConnection,
    wanted: &[(String, Vec<Candidate>)],
) -> HashMap<String, Contact> {
    match resolve_many_inner(db, wanted).await {
        Ok(out) => out,
        Err(e) => {
            tracing::warn!(error = %e, "contact bulk match failed");
            _safe_rollback(db).await;
            HashMap::new()
        }
    }
}

async fn resolve_many_inner(
    db: &mut PgConnection,
    wanted: &[(String, Vec<Candidate>)],
) -> Result<HashMap<String, Contact>, sqlx::Error> {
    let mut ids: HashSet<Uuid> = HashSet::new();
    let mut numbers: Vec<String> = Vec::new();
    for (_, cands) in wanted {
        for c in cands {
            match c {
                Candidate::Id(id) => {
                    ids.insert(*id);
                }
                Candidate::Number(s) => {
                    let n = normalize_phone(Some(s));
                    if !s.is_empty() && !n.is_empty() && !numbers.contains(&n) {
                        numbers.push(n);
                    }
                }
            }
        }
    }
    let mut links: HashMap<String, Uuid> = HashMap::new();
    if !numbers.is_empty() {
        let rows: Vec<(String, Uuid)> =
            sqlx::query_as("SELECT phone, contact_id FROM wa_contact_links WHERE phone = ANY($1)")
                .bind(&numbers)
                .fetch_all(&mut *db)
                .await?;
        links.extend(rows);
    }
    // {form: number}: a later number overwrites an earlier one's identical form, as the dict comprehension.
    // Python iterates a set of numbers (hash order); two different normalized numbers share a form only
    // in contrived cases (country code + x vs x), where the winner is arbitrary in Python too.
    let mut forms: HashMap<String, String> = HashMap::new();
    for n in &numbers {
        for f in phone_digit_forms(n) {
            forms.insert(f, n.clone());
        }
    }
    let mut by_form: HashMap<String, Uuid> = HashMap::new();
    if !forms.is_empty() {
        let list: Vec<String> = forms.keys().cloned().collect();
        let (wa_d, ph_d) = (digits("contacts.whatsapp"), digits("contacts.phone"));
        let sql = format!(
            "SELECT contacts.id, {wa_d}, {ph_d} FROM contacts \
             WHERE contacts.is_deleted = false AND ({wa_d} = ANY($1) OR {ph_d} = ANY($1))"
        );
        let rows: Vec<(Uuid, String, String)> =
            sqlx::query_as(&sql).bind(&list).fetch_all(&mut *db).await?;
        for (cid, wa, ph) in rows {
            for d in [wa, ph] {
                if let Some(n) = forms.get(&d) {
                    by_form.entry(n.clone()).or_insert(cid);
                }
            }
        }
    }
    ids.extend(links.values().copied());
    ids.extend(by_form.values().copied());
    let mut contacts: HashMap<Uuid, Contact> = HashMap::new();
    if !ids.is_empty() {
        let list: Vec<Uuid> = ids.into_iter().collect();
        let rows = sqlx::query_as::<_, Contact>(
            "SELECT * FROM contacts WHERE id = ANY($1) AND is_deleted = false",
        )
        .bind(&list)
        .fetch_all(&mut *db)
        .await?;
        contacts.extend(rows.into_iter().map(|c| (c.id, c)));
    }
    let mut out: HashMap<String, Contact> = HashMap::new();
    for (key, cands) in wanted {
        for c in cands {
            let hit = match c {
                Candidate::Id(id) => contacts.get(id),
                Candidate::Number(s) => {
                    let n = if s.is_empty() {
                        String::new()
                    } else {
                        normalize_phone(Some(s))
                    };
                    if n.is_empty() {
                        None
                    } else {
                        links
                            .get(&n)
                            .and_then(|id| contacts.get(id))
                            .or_else(|| by_form.get(&n).and_then(|id| contacts.get(id)))
                    }
                }
            };
            if let Some(h) = hit {
                out.insert(key.clone(), h.clone());
                break;
            }
        }
    }
    Ok(out)
}
