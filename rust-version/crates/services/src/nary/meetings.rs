//! Port of backend/app/services/nary/meetings.py: slots, booking with Google Meet, reminders, the Owner's
//! brief, the debrief (design §8; Nary Phase 3 item C).
//!
//! ```text
//!     propose()      Nary finds slots (slots.rs) against Google free/busy and proposes them to the Owner
//!                    as meeting.slots.send (a card): the Owner can edit the times before approving.
//!     on approval    the slots go to the customer, numbered, with the recording notice.
//!     handle_pick()  the customer's answer is read (Haiku, nary.slot_pick) and meeting.book books it
//!                    automatically (design §9): a Google event with a Meet link, inviting their email.
//!     tick()         reminders to the customer the day before and an hour before, the Owner's one-page
//!                    brief 30 minutes before, a no-show question when nobody recorded the meeting, done.
//!     book_on_request()  a meeting at a time a person gave (propose_meeting card): the Google event with
//!                    Meet and its invitations, the CRM activity, then the customer's message with the link.
//!                    No customer reminders (the person agreed the time themselves); brief and debrief as usual.
//!     debrief()      the Owner's voice note (speech-to-text on the server) or the call transcript (txt, SRT,
//!                    VTT, docx): Sonnet extracts BANT changes, requirements, commitments (each a reminder),
//!                    objections and next steps; Opus drafts the summary email (summary.email.send).
//! ```
//!
//! Python changes ORM objects and lets the session flush them; here every change is an UPDATE at the
//! point Python makes it (with `updated_at = now()`, the models' onupdate), so a later commit or rollback
//! of the caller has the same effect.
#![allow(clippy::too_many_arguments)]

use std::collections::HashMap;

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use granary_compat::pyfmt::strftime;
use granary_compat::pyjson::{dumps, DumpOpts, PyDateTime, PyTz, PyValue};
use granary_compat::text::pystr;
use granary_core::auth::AuthUser;
use granary_core::clock::local_tz;
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_json, SettingError};
use granary_db::rows::crm::{Contact, Deal, Lead, Organization};
use granary_db::rows::nary::{AgentRun, NaryInsight, NaryMeeting, WaChat};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::approvals::notify::{notify, Notice};
use crate::google_calendar;
use crate::nary::governance::{http_detail, is_http_exception, py_exception};
use crate::nary::replies::{self, Conversation};
use crate::nary::wa_send::SendError;
use crate::nary::{
    commit, events, executor, llm, owner_channel, persona, py_slice, py_slice_tail, roles, runs,
    slots,
};

pub const WHERE: &str = "Nary & AI → Meetings";
pub const OPEN: [&str; 3] = ["proposed", "slots_sent", "booked"];
pub const MESSAGE_FIELDS: [&str; 7] = [
    "channel",
    "text",
    "chat_id",
    "thread_id",
    "contact_id",
    "to",
    "subject",
];

const MEETING_COLS: &str = "deal_id, lead_id, contact_id, chat_id, channel, language, customer_tz, country, slots, \
    status, picked, starts_at, ends_at, event_id, meet_link, attendee_email, title, day_reminder_at, \
    hour_reminder_at, brief_at, card_id, notes, origin, booked_by_id, attendees, id, created_at, updated_at";

fn internal(msg: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("{msg}"))
}

/// Python truthiness of an optional JSON value.
fn truthy(v: Option<&Value>) -> bool {
    crate::nary::slots::truthy(v)
}

/// `str(value)` of a JSON value as Python prints it inside an f-string.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        other => other.to_string(),
    }
}

/// `t[key]` of the texts dict (a KeyError is a 500).
fn tget<'a>(t: &'a Value, key: &str) -> ApiResult<&'a str> {
    t.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("KeyError: '{key}'")))
}

pub fn slots_message(slot_list: &[Value], t: &Value) -> String {
    let get = |k: &str| t.get(k).map(py_str).unwrap_or_default();
    let mut lines = vec![get("slots_intro")];
    for (i, s) in slot_list.iter().enumerate() {
        // s.get("customer") or s.get("company")
        let when = match s.get("customer") {
            Some(c) if truthy(Some(c)) => py_str(c),
            _ => s
                .get("company")
                .map(py_str)
                .unwrap_or_else(|| "None".into()),
        };
        let ext = if truthy(s.get("extended")) {
            format!(" ({})", get("extended"))
        } else {
            String::new()
        };
        lines.push(format!("{}. {when}{ext}", i + 1));
    }
    lines.push(get("recording"));
    lines.join("\n")
}

/// `template.format(**kw)` for named fields (`{name}`, `{{`, `}}`); anything else fails like Python.
fn py_format(template: &str, kw: &[(&str, &str)]) -> ApiResult<String> {
    let mut out = String::new();
    let mut it = template.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '{' if it.peek() == Some(&'{') => {
                it.next();
                out.push('{');
            }
            '}' if it.peek() == Some(&'}') => {
                it.next();
                out.push('}');
            }
            '{' => {
                let mut name = String::new();
                loop {
                    match it.next() {
                        Some('}') => break,
                        Some(ch) => name.push(ch),
                        None => return Err(internal("Single '{' encountered in format string")),
                    }
                }
                if name.is_empty() {
                    return Err(internal(
                        "Replacement index 0 out of range for positional args tuple",
                    ));
                }
                match kw.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => out.push_str(v),
                    None => return Err(internal(format!("KeyError: '{name}'"))),
                }
            }
            '}' => return Err(internal("Single '}' encountered in format string")),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// `datetime.fromisoformat(s)` for the slot times (aware with an offset, or naive).
pub(crate) fn fromisoformat(s: &str) -> ApiResult<PyDateTime> {
    let bad = || internal(format!("Invalid isoformat string: {s:?}"));
    let t = s.replace('Z', "+00:00");
    if t.len() < 10 {
        return Err(bad());
    }
    // An offset at the end: [+-]HH:MM
    let (body, off) = match t
        .len()
        .checked_sub(6)
        .and_then(|i| t.get(i..).map(|tail| (i, tail)))
    {
        Some((i, tail))
            if (tail.starts_with('+') || tail.starts_with('-'))
                && tail.as_bytes()[3] == b':'
                && i > 10 =>
        {
            let sign = if tail.starts_with('-') { -1 } else { 1 };
            let h: i32 = tail[1..3].parse().map_err(|_| bad())?;
            let m: i32 = tail[4..6].parse().map_err(|_| bad())?;
            (&t[..i], Some(sign * (h * 3600 + m * 60)))
        }
        _ => (t.as_str(), None),
    };
    let body = body.replacen(' ', "T", 1);
    let local = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(&body, f).ok())
        .or_else(|| {
            NaiveDate::parse_from_str(&body, "%Y-%m-%d")
                .ok()
                .map(|d| d.and_time(NaiveTime::MIN))
        })
        .ok_or_else(bad)?;
    Ok(PyDateTime {
        local,
        tz: off.map(|o| {
            if o == 0 {
                PyTz::Fixed(0)
            } else {
                PyTz::Fixed(o)
            }
        }),
    })
}

/// A Python datetime's instant (naive ones are read as UTC, the server's zone).
fn instant(dt: &PyDateTime) -> DateTime<Utc> {
    dt.to_utc_naive().and_utc()
}

/// `dt.astimezone(zone)`: a datetime already in that same zone is left as it is (Python: same tzinfo).
fn astimezone(dt: &PyDateTime, zone: Tz) -> PyDateTime {
    match dt.tz {
        Some(PyTz::Zone(z)) | Some(PyTz::ZoneFold1(z)) if z == zone => *dt,
        _ => PyDateTime::from_utc_in_zone(dt.to_utc_naive(), zone),
    }
}

/// `utc_dt.astimezone(zone).strftime("%a %d %b %H:%M")`.
fn when_in(dt: DateTime<Utc>, zone: Tz) -> String {
    strftime(
        &PyDateTime::from_utc_in_zone(dt.naive_utc(), zone),
        "%a %d %b %H:%M",
    )
}

fn zone(name: &str) -> ApiResult<Tz> {
    slots::zone(name)
}

fn py_json(v: &Value) -> String {
    dumps(&PyValue::from(v), &DumpOpts::unicode()).unwrap_or_default()
}

pub async fn rules(db: &mut PgConnection) -> ApiResult<Value> {
    let r = require_json(db, "nary.meetings.rules", WHERE).await?;
    for k in [
        "duration_minutes",
        "buffer_minutes",
        "max_per_day",
        "start_window",
        "slots_offered",
        "horizon_days",
    ] {
        if r.as_object().is_none_or(|o| !o.contains_key(k)) {
            return Err(
                SettingError::new("nary.meetings.rules", &format!("has no '{k}'"), WHERE).into(),
            );
        }
    }
    Ok(r)
}

pub async fn texts(db: &mut PgConnection, language: Option<&str>) -> ApiResult<Value> {
    let t = require_json(db, "nary.meetings.text", WHERE).await?;
    // The customer's language when the setting has texts for it, else English.
    let lang = language.filter(|l| t.get(*l).is_some()).unwrap_or("en");
    match t.as_object().and_then(|o| o.get(lang)) {
        Some(v) => Ok(v.clone()),
        None => Err(SettingError::new(
            "nary.meetings.text",
            &format!("has no '{lang}' texts"),
            WHERE,
        )
        .into()),
    }
}

/// `pytz.country_timezones[c][0]` for every country pytz knows (pytz 2025.x, generated).
const COUNTRY_FIRST_ZONE: &[(&str, &str)] = &[
    ("AD", "Europe/Andorra"),
    ("AE", "Asia/Dubai"),
    ("AF", "Asia/Kabul"),
    ("AG", "America/Antigua"),
    ("AI", "America/Anguilla"),
    ("AL", "Europe/Tirane"),
    ("AM", "Asia/Yerevan"),
    ("AO", "Africa/Luanda"),
    ("AQ", "Antarctica/McMurdo"),
    ("AR", "America/Argentina/Buenos_Aires"),
    ("AS", "Pacific/Pago_Pago"),
    ("AT", "Europe/Vienna"),
    ("AU", "Australia/Lord_Howe"),
    ("AW", "America/Aruba"),
    ("AX", "Europe/Mariehamn"),
    ("AZ", "Asia/Baku"),
    ("BA", "Europe/Sarajevo"),
    ("BB", "America/Barbados"),
    ("BD", "Asia/Dhaka"),
    ("BE", "Europe/Brussels"),
    ("BF", "Africa/Ouagadougou"),
    ("BG", "Europe/Sofia"),
    ("BH", "Asia/Bahrain"),
    ("BI", "Africa/Bujumbura"),
    ("BJ", "Africa/Porto-Novo"),
    ("BL", "America/St_Barthelemy"),
    ("BM", "Atlantic/Bermuda"),
    ("BN", "Asia/Brunei"),
    ("BO", "America/La_Paz"),
    ("BQ", "America/Kralendijk"),
    ("BR", "America/Noronha"),
    ("BS", "America/Nassau"),
    ("BT", "Asia/Thimphu"),
    ("BW", "Africa/Gaborone"),
    ("BY", "Europe/Minsk"),
    ("BZ", "America/Belize"),
    ("CA", "America/St_Johns"),
    ("CC", "Indian/Cocos"),
    ("CD", "Africa/Kinshasa"),
    ("CF", "Africa/Bangui"),
    ("CG", "Africa/Brazzaville"),
    ("CH", "Europe/Zurich"),
    ("CI", "Africa/Abidjan"),
    ("CK", "Pacific/Rarotonga"),
    ("CL", "America/Santiago"),
    ("CM", "Africa/Douala"),
    ("CN", "Asia/Shanghai"),
    ("CO", "America/Bogota"),
    ("CR", "America/Costa_Rica"),
    ("CU", "America/Havana"),
    ("CV", "Atlantic/Cape_Verde"),
    ("CW", "America/Curacao"),
    ("CX", "Indian/Christmas"),
    ("CY", "Asia/Nicosia"),
    ("CZ", "Europe/Prague"),
    ("DE", "Europe/Berlin"),
    ("DJ", "Africa/Djibouti"),
    ("DK", "Europe/Copenhagen"),
    ("DM", "America/Dominica"),
    ("DO", "America/Santo_Domingo"),
    ("DZ", "Africa/Algiers"),
    ("EC", "America/Guayaquil"),
    ("EE", "Europe/Tallinn"),
    ("EG", "Africa/Cairo"),
    ("EH", "Africa/El_Aaiun"),
    ("ER", "Africa/Asmara"),
    ("ES", "Europe/Madrid"),
    ("ET", "Africa/Addis_Ababa"),
    ("FI", "Europe/Helsinki"),
    ("FJ", "Pacific/Fiji"),
    ("FK", "Atlantic/Stanley"),
    ("FM", "Pacific/Chuuk"),
    ("FO", "Atlantic/Faroe"),
    ("FR", "Europe/Paris"),
    ("GA", "Africa/Libreville"),
    ("GB", "Europe/London"),
    ("GD", "America/Grenada"),
    ("GE", "Asia/Tbilisi"),
    ("GF", "America/Cayenne"),
    ("GG", "Europe/Guernsey"),
    ("GH", "Africa/Accra"),
    ("GI", "Europe/Gibraltar"),
    ("GL", "America/Nuuk"),
    ("GM", "Africa/Banjul"),
    ("GN", "Africa/Conakry"),
    ("GP", "America/Guadeloupe"),
    ("GQ", "Africa/Malabo"),
    ("GR", "Europe/Athens"),
    ("GS", "Atlantic/South_Georgia"),
    ("GT", "America/Guatemala"),
    ("GU", "Pacific/Guam"),
    ("GW", "Africa/Bissau"),
    ("GY", "America/Guyana"),
    ("HK", "Asia/Hong_Kong"),
    ("HN", "America/Tegucigalpa"),
    ("HR", "Europe/Zagreb"),
    ("HT", "America/Port-au-Prince"),
    ("HU", "Europe/Budapest"),
    ("ID", "Asia/Jakarta"),
    ("IE", "Europe/Dublin"),
    ("IL", "Asia/Jerusalem"),
    ("IM", "Europe/Isle_of_Man"),
    ("IN", "Asia/Kolkata"),
    ("IO", "Indian/Chagos"),
    ("IQ", "Asia/Baghdad"),
    ("IR", "Asia/Tehran"),
    ("IS", "Atlantic/Reykjavik"),
    ("IT", "Europe/Rome"),
    ("JE", "Europe/Jersey"),
    ("JM", "America/Jamaica"),
    ("JO", "Asia/Amman"),
    ("JP", "Asia/Tokyo"),
    ("KE", "Africa/Nairobi"),
    ("KG", "Asia/Bishkek"),
    ("KH", "Asia/Phnom_Penh"),
    ("KI", "Pacific/Tarawa"),
    ("KM", "Indian/Comoro"),
    ("KN", "America/St_Kitts"),
    ("KP", "Asia/Pyongyang"),
    ("KR", "Asia/Seoul"),
    ("KW", "Asia/Kuwait"),
    ("KY", "America/Cayman"),
    ("KZ", "Asia/Almaty"),
    ("LA", "Asia/Vientiane"),
    ("LB", "Asia/Beirut"),
    ("LC", "America/St_Lucia"),
    ("LI", "Europe/Vaduz"),
    ("LK", "Asia/Colombo"),
    ("LR", "Africa/Monrovia"),
    ("LS", "Africa/Maseru"),
    ("LT", "Europe/Vilnius"),
    ("LU", "Europe/Luxembourg"),
    ("LV", "Europe/Riga"),
    ("LY", "Africa/Tripoli"),
    ("MA", "Africa/Casablanca"),
    ("MC", "Europe/Monaco"),
    ("MD", "Europe/Chisinau"),
    ("ME", "Europe/Podgorica"),
    ("MF", "America/Marigot"),
    ("MG", "Indian/Antananarivo"),
    ("MH", "Pacific/Majuro"),
    ("MK", "Europe/Skopje"),
    ("ML", "Africa/Bamako"),
    ("MM", "Asia/Yangon"),
    ("MN", "Asia/Ulaanbaatar"),
    ("MO", "Asia/Macau"),
    ("MP", "Pacific/Saipan"),
    ("MQ", "America/Martinique"),
    ("MR", "Africa/Nouakchott"),
    ("MS", "America/Montserrat"),
    ("MT", "Europe/Malta"),
    ("MU", "Indian/Mauritius"),
    ("MV", "Indian/Maldives"),
    ("MW", "Africa/Blantyre"),
    ("MX", "America/Mexico_City"),
    ("MY", "Asia/Kuala_Lumpur"),
    ("MZ", "Africa/Maputo"),
    ("NA", "Africa/Windhoek"),
    ("NC", "Pacific/Noumea"),
    ("NE", "Africa/Niamey"),
    ("NF", "Pacific/Norfolk"),
    ("NG", "Africa/Lagos"),
    ("NI", "America/Managua"),
    ("NL", "Europe/Amsterdam"),
    ("NO", "Europe/Oslo"),
    ("NP", "Asia/Kathmandu"),
    ("NR", "Pacific/Nauru"),
    ("NU", "Pacific/Niue"),
    ("NZ", "Pacific/Auckland"),
    ("OM", "Asia/Muscat"),
    ("PA", "America/Panama"),
    ("PE", "America/Lima"),
    ("PF", "Pacific/Tahiti"),
    ("PG", "Pacific/Port_Moresby"),
    ("PH", "Asia/Manila"),
    ("PK", "Asia/Karachi"),
    ("PL", "Europe/Warsaw"),
    ("PM", "America/Miquelon"),
    ("PN", "Pacific/Pitcairn"),
    ("PR", "America/Puerto_Rico"),
    ("PS", "Asia/Gaza"),
    ("PT", "Europe/Lisbon"),
    ("PW", "Pacific/Palau"),
    ("PY", "America/Asuncion"),
    ("QA", "Asia/Qatar"),
    ("RE", "Indian/Reunion"),
    ("RO", "Europe/Bucharest"),
    ("RS", "Europe/Belgrade"),
    ("RU", "Europe/Kaliningrad"),
    ("RW", "Africa/Kigali"),
    ("SA", "Asia/Riyadh"),
    ("SB", "Pacific/Guadalcanal"),
    ("SC", "Indian/Mahe"),
    ("SD", "Africa/Khartoum"),
    ("SE", "Europe/Stockholm"),
    ("SG", "Asia/Singapore"),
    ("SH", "Atlantic/St_Helena"),
    ("SI", "Europe/Ljubljana"),
    ("SJ", "Arctic/Longyearbyen"),
    ("SK", "Europe/Bratislava"),
    ("SL", "Africa/Freetown"),
    ("SM", "Europe/San_Marino"),
    ("SN", "Africa/Dakar"),
    ("SO", "Africa/Mogadishu"),
    ("SR", "America/Paramaribo"),
    ("SS", "Africa/Juba"),
    ("ST", "Africa/Sao_Tome"),
    ("SV", "America/El_Salvador"),
    ("SX", "America/Lower_Princes"),
    ("SY", "Asia/Damascus"),
    ("SZ", "Africa/Mbabane"),
    ("TC", "America/Grand_Turk"),
    ("TD", "Africa/Ndjamena"),
    ("TF", "Indian/Kerguelen"),
    ("TG", "Africa/Lome"),
    ("TH", "Asia/Bangkok"),
    ("TJ", "Asia/Dushanbe"),
    ("TK", "Pacific/Fakaofo"),
    ("TL", "Asia/Dili"),
    ("TM", "Asia/Ashgabat"),
    ("TN", "Africa/Tunis"),
    ("TO", "Pacific/Tongatapu"),
    ("TR", "Europe/Istanbul"),
    ("TT", "America/Port_of_Spain"),
    ("TV", "Pacific/Funafuti"),
    ("TW", "Asia/Taipei"),
    ("TZ", "Africa/Dar_es_Salaam"),
    ("UA", "Europe/Simferopol"),
    ("UG", "Africa/Kampala"),
    ("UM", "Pacific/Midway"),
    ("US", "America/New_York"),
    ("UY", "America/Montevideo"),
    ("UZ", "Asia/Samarkand"),
    ("VA", "Europe/Vatican"),
    ("VC", "America/St_Vincent"),
    ("VE", "America/Caracas"),
    ("VG", "America/Tortola"),
    ("VI", "America/St_Thomas"),
    ("VN", "Asia/Ho_Chi_Minh"),
    ("VU", "Pacific/Efate"),
    ("WF", "Pacific/Wallis"),
    ("WS", "Pacific/Apia"),
    ("YE", "Asia/Aden"),
    ("YT", "Indian/Mayotte"),
    ("ZA", "Africa/Johannesburg"),
    ("ZM", "Africa/Lusaka"),
    ("ZW", "Africa/Harare"),
];

/// The customer's time zone from their country (the first listed zone); unknown countries use company
/// time.
pub fn customer_tz(country: Option<&str>, company_tz: &str) -> String {
    let Some(c) = country.filter(|c| !c.is_empty()) else {
        return company_tz.to_string();
    };
    let upper = c.to_uppercase();
    COUNTRY_FIRST_ZONE
        .iter()
        .find(|(k, _)| *k == upper)
        .map(|(_, z)| z.to_string())
        .unwrap_or_else(|| company_tz.to_string())
}

async fn load_meeting(db: &mut PgConnection, id: Uuid) -> ApiResult<Option<NaryMeeting>> {
    Ok(sqlx::query_as::<_, NaryMeeting>(&format!(
        "SELECT {MEETING_COLS} FROM nary_meetings WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *db)
    .await?)
}

async fn load_contact(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<Contact>> {
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query_as::<_, Contact>("SELECT * FROM contacts WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

async fn load_deal(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<Deal>> {
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query_as::<_, Deal>("SELECT * FROM deals WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

async fn load_lead(db: &mut PgConnection, id: Option<Uuid>) -> ApiResult<Option<Lead>> {
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query_as::<_, Lead>("SELECT * FROM leads WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?,
    )
}

/// `uuid.UUID(str(value))`.
fn parse_uuid(v: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(v.trim()).map_err(|_| internal("badly formed hexadecimal UUID string"))
}

pub async fn open_meeting(
    db: &mut PgConnection,
    chat_id: Option<&str>,
    deal_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    contact_id: Option<Uuid>,
) -> ApiResult<Option<NaryMeeting>> {
    let chat_id = chat_id.filter(|c| !c.is_empty());
    if chat_id.is_none() && deal_id.is_none() && lead_id.is_none() && contact_id.is_none() {
        return Ok(None);
    }
    let mut conds = Vec::new();
    if chat_id.is_some() {
        conds.push("chat_id = $1");
    }
    if deal_id.is_some() {
        conds.push("deal_id = $2");
    }
    if lead_id.is_some() {
        conds.push("lead_id = $3");
    }
    if contact_id.is_some() {
        conds.push("contact_id = $4");
    }
    let sql = format!(
        "SELECT {MEETING_COLS} FROM nary_meetings WHERE ({}) AND status IN ('proposed', 'slots_sent', 'booked') \
         ORDER BY created_at DESC LIMIT 1",
        conds.join(" OR ")
    );
    Ok(sqlx::query_as::<_, NaryMeeting>(&sql)
        .bind(chat_id)
        .bind(deal_id)
        .bind(lead_id)
        .bind(contact_id)
        .fetch_optional(&mut *db)
        .await?)
}

pub async fn had_meeting(
    db: &mut PgConnection,
    deal_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    chat_id: Option<&str>,
) -> ApiResult<bool> {
    let chat_id = chat_id.filter(|c| !c.is_empty());
    let mut conds = Vec::new();
    if deal_id.is_some() {
        conds.push("deal_id = $1");
    }
    if lead_id.is_some() {
        conds.push("lead_id = $2");
    }
    if chat_id.is_some() {
        conds.push("chat_id = $3");
    }
    if conds.is_empty() {
        return Ok(false);
    }
    let sql = format!(
        "SELECT id FROM nary_meetings WHERE ({}) AND status IN ('booked', 'done') LIMIT 1",
        conds.join(" OR ")
    );
    let row: Option<(Uuid,)> = sqlx::query_as(&sql)
        .bind(deal_id)
        .bind(lead_id)
        .bind(chat_id)
        .fetch_optional(&mut *db)
        .await?;
    Ok(row.is_some())
}

/// (slots as dicts, the customer's time zone, a warning when the calendar couldn't be read).
pub async fn find_slots(
    db: &mut PgConnection,
    country: Option<&str>,
    now: Option<DateTime<Utc>>,
) -> ApiResult<(Vec<Value>, String, Option<String>)> {
    let r = rules(db).await?;
    let tz = local_tz(db).await?;
    let company_tz = tz.name().to_string();
    let cust_tz = customer_tz(country, &company_tz);
    let now = now.unwrap_or_else(Utc::now);
    let mut warning = None;
    let horizon = slots::py_int(&r["horizon_days"])?;
    let busy = match google_calendar::free_busy(db, now, now + Duration::days(horizon + 1)).await {
        Ok(b) => b,
        Err(e) if is_http_exception(&e) => {
            warning = Some(format!(
                "Google Calendar wasn't read ({}); check your calendar before approving.",
                http_detail(&e).unwrap_or_default()
            ));
            Vec::new()
        }
        Err(e) => return Err(e),
    };
    let mut booked: HashMap<NaiveDate, i64> = HashMap::new();
    let rows: Vec<(DateTime<Utc>,)> = sqlx::query_as(
        "SELECT starts_at FROM nary_meetings WHERE status = 'booked' AND starts_at >= $1",
    )
    .bind(now)
    .fetch_all(&mut *db)
    .await?;
    for (s,) in rows {
        let d = PyDateTime::from_utc_in_zone(s.naive_utc(), tz).local.date();
        *booked.entry(d).or_insert(0) += 1;
    }
    let holidays = require_json(db, "nary.meetings.holidays", WHERE).await?;
    let reduced_hours = require_json(db, "nary.meetings.reduced_hours_periods", WHERE).await?;
    let customer_hours = require_json(db, "nary.meetings.customer_hours", WHERE).await?;
    let found = slots::find(
        &r,
        &holidays,
        &reduced_hours,
        &customer_hours,
        &company_tz,
        Some(&cust_tz),
        country,
        &busy,
        now,
        Some(&booked),
    )?;
    let dicts = found
        .iter()
        .map(|s| s.as_dict(&company_tz, Some(&cust_tz)))
        .collect::<ApiResult<Vec<Value>>>()?;
    Ok((dicts, cust_tz, warning))
}

/// `propose(db, conv, *, run=None, reason="", only_if_none=False)`: find slots and put them on a card for
/// the Owner (meeting.slots.send).
pub async fn propose(
    db: &mut PgConnection,
    conv: &Conversation,
    run: Option<&AgentRun>,
    reason: &str,
    only_if_none: bool,
) -> ApiResult<Option<Value>> {
    let deal_id = conv.deal.as_ref().map(|d| d.id);
    let lead_id = conv.lead.as_ref().map(|l| l.id);
    let mut existing = open_meeting(db, conv.chat_id(), deal_id, lead_id, None).await?;
    if let Some(m) = existing.as_ref() {
        if m.status == "proposed" {
            if let Some(card_id) = m.card_id {
                // Times the Owner rejected (or that expired) never reached the customer: that offer is over, a new one may go.
                let card: Option<(String,)> =
                    sqlx::query_as("SELECT status FROM agent_pending_actions WHERE id = $1")
                        .bind(card_id)
                        .fetch_optional(&mut *db)
                        .await?;
                if card.as_ref().is_none_or(|(s,)| {
                    matches!(s.as_str(), "rejected" | "expired" | "failed" | "cancelled")
                }) {
                    set_status(db, m.id, "cancelled").await?;
                    existing = None;
                }
            }
        }
    }
    if let Some(m) = existing {
        return Ok(if only_if_none {
            None
        } else {
            Some(json!({"meeting_id": m.id.to_string(), "status": m.status}))
        });
    }
    if only_if_none && had_meeting(db, deal_id, lead_id, conv.chat_id()).await? {
        return Ok(None); // Nary offers a first call by itself; later ones only when the customer asks
    }
    let (slot_list, cust_tz, warning) = find_slots(db, conv.country.as_deref(), None).await?;
    if slot_list.is_empty() {
        // f"meeting.noslot:{conv.chat_id or conv.lead and conv.lead.id}"
        let who = match conv.chat_id().filter(|c| !c.is_empty()) {
            Some(c) => c.to_string(),
            None => lead_id
                .map(|l| l.to_string())
                .unwrap_or_else(|| "None".into()),
        };
        crate::nary::governance::error_card(
            db,
            &format!("meeting.noslot:{who}"),
            &format!("No meeting slot fits {}", conv.label()),
            "Nothing in the next days fits both your calendar rules and their working hours. Offer times yourself.",
            "/nary/meetings",
        )
        .await?;
        return Ok(None);
    }
    let t = texts(db, conv.language.as_deref()).await?;
    let meeting_id = Uuid::new_v4();
    let slots_json = Value::Array(slot_list.clone());
    sqlx::query(
        "INSERT INTO nary_meetings (id, deal_id, lead_id, contact_id, chat_id, channel, language, customer_tz, \
         country, slots, status, attendee_email, origin, attendees) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'proposed', $11, 'nary', '[]'::jsonb)",
    )
    .bind(meeting_id)
    .bind(deal_id)
    .bind(lead_id)
    .bind(conv.contact.as_ref().map(|c| c.id))
    .bind(conv.chat_id())
    .bind(&conv.channel)
    .bind(conv.language.clone().filter(|l| !l.is_empty()).unwrap_or_else(|| "en".into()))
    .bind(&cust_tz)
    .bind(conv.country.as_deref())
    .bind(&slots_json)
    .bind(conv.contact.as_ref().and_then(|c| c.email.clone()))
    .execute(&mut *db)
    .await?;
    let rationale = format!("Propose a call with {} ({reason}).", conv.label())
        + &warning.map(|w| format!(" {w}")).unwrap_or_default();
    let card = replies::_propose(
        db,
        "meeting.slots.send",
        json!({"meeting_id": meeting_id.to_string(), "slots": slots_json, "text": slots_message(&slot_list, &t)}),
        run,
        None,
        Some(&rationale),
        None,
    )
    .await?;
    let Some(card) = card else { return Ok(None) };
    sqlx::query("UPDATE nary_meetings SET card_id = $2, updated_at = now() WHERE id = $1")
        .bind(meeting_id)
        .bind(card.id)
        .execute(&mut *db)
        .await?;
    commit(db).await?;
    Ok(Some(
        json!({"meeting_id": meeting_id.to_string(), "card_id": card.id.to_string(), "slots": slot_list.len()}),
    ))
}

async fn set_status(db: &mut PgConnection, id: Uuid, status: &str) -> ApiResult<()> {
    sqlx::query("UPDATE nary_meetings SET status = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// Applying meeting.slots.send: the (possibly edited) slots go to the customer.
/// (A QuietHours from the send reaches the executor as its 409 HTTPException, which it recognises.)
pub async fn send_slots(
    db: &mut PgConnection,
    meeting_id: Uuid,
    slot_list: &[Value],
    text: &str,
) -> ApiResult<Value> {
    let meeting = load_meeting(db, meeting_id).await?;
    let Some(mut meeting) = meeting.filter(|m| m.status == "proposed" || m.status == "slots_sent")
    else {
        return Err(ApiError::http(
            409,
            "This meeting is no longer waiting for slots",
        ));
    };
    meeting.slots = Value::Array(slot_list.to_vec());
    sqlx::query("UPDATE nary_meetings SET slots = $2, updated_at = now() WHERE id = $1")
        .bind(meeting_id)
        .bind(&meeting.slots)
        .execute(&mut *db)
        .await?;
    let out = _send_to_customer(db, &meeting, text, "Meeting times").await?;
    set_status(db, meeting_id, "slots_sent").await?;
    commit(db).await?;
    let mut out = out;
    out.insert(
        "label".into(),
        Value::String(format!("{} meeting times sent", slot_list.len())),
    );
    Ok(Value::Object(out))
}

/// `_send_to_customer(db, meeting, text, *, subject)`: WhatsApp when the meeting has a chat, else email.
pub async fn _send_to_customer(
    db: &mut PgConnection,
    meeting: &NaryMeeting,
    text: &str,
    subject: &str,
) -> Result<Map<String, Value>, SendError> {
    if let Some(chat_id) = meeting.chat_id.as_deref().filter(|c| !c.is_empty()) {
        let sent = crate::nary::wa_send::send_text(db, chat_id, text, "reply", None, false).await?;
        return Ok(match sent.to_json() {
            Value::Object(m) => m,
            _ => Map::new(),
        });
    }
    let contact = load_contact(db, meeting.contact_id).await?;
    let Some((contact, email)) = contact.and_then(|c| {
        let e = c.email.clone().filter(|e| !e.is_empty())?;
        Some((c, e))
    }) else {
        return Err(ApiError::http(
            422,
            "The customer has neither a WhatsApp chat nor an email address",
        )
        .into());
    };
    let lead = meeting.lead_id.map(|l| l.to_string());
    let out = crate::nary::channel_actions::send_email(
        db,
        &email,
        subject,
        text,
        Some(contact.id),
        lead.as_deref(),
        meeting.deal_id,
        None,
        None,
        None,
    )
    .await?;
    Ok(match out {
        Value::Object(m) => m,
        _ => Map::new(),
    })
}

/// `PICK_SCHEMA`.
/// The schema of the follow-up email after a call.
pub fn summary_email_schema() -> Value {
    json!({"type": "object", "properties": {"subject": {"type": "string"}, "body": {"type": "string"}},
           "required": ["subject", "body"], "additionalProperties": false})
}

pub fn pick_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"picked": {"type": ["integer", "null"]}, "email": {"type": ["string", "null"]},
                       "wants_other_times": {"type": "boolean"}},
        "required": ["picked", "email", "wants_other_times"],
        "additionalProperties": false,
    })
}

/// The customer answered the offered times. A clear pick → meeting.book (automatic).
/// The model call's tokens are added to `run`, as in Python.
pub async fn handle_pick(
    db: &mut PgConnection,
    conv: &mut Conversation,
    mut run: Option<&mut AgentRun>,
) -> ApiResult<Option<Value>> {
    let contact_id = if conv.chat_id().is_some_and(|c| !c.is_empty()) {
        None
    } else {
        conv.contact.as_ref().map(|c| c.id)
    };
    let chat_id = conv.chat_id().map(str::to_string);
    let meeting = open_meeting(db, chat_id.as_deref(), None, None, contact_id).await?;
    let Some(meeting) = meeting.filter(|m| m.status == "slots_sent") else {
        return Ok(None);
    };
    let slot_list: Vec<Value> = meeting.slots.as_array().cloned().unwrap_or_default();
    let options = slot_list
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let when = match s.get("customer") {
                Some(c) if truthy(Some(c)) => py_str(c),
                _ => s
                    .get("company")
                    .map(py_str)
                    .unwrap_or_else(|| "None".into()),
            };
            format!("{}. {when}", i + 1)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let system = persona::system_prompt(
        "staff",
        "The customer was offered numbered meeting times. Which one did they pick \
(its number), or null if unclear? If they wrote an email address for the \
invitation, return it. wants_other_times: they asked for different times.",
        "",
        "",
    );
    let content = format!("OFFERED:\n{options}\n\nREPLY:\n")
        + &llm::information("customer", py_slice_tail(&conv.transcript, 1500), 8000);
    let data = llm::ask_json(
        db,
        "nary.slot_pick",
        &system,
        &content,
        &pick_schema(),
        run.as_deref_mut(),
        None,
        None,
    )
    .await?;
    let Some(data) = data
        .and_then(|d| d.as_object().cloned())
        .filter(|d| !d.is_empty())
    else {
        return Ok(None);
    };
    if truthy(data.get("email")) {
        if let Some(contact) = conv.contact.as_mut() {
            if !contact.email.as_deref().is_some_and(|e| !e.is_empty()) {
                let raw = data["email"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| py_str(&data["email"]));
                let email = py_slice(pystr::strip(&raw), 255).to_string();
                contact.email = Some(email.clone());
                sqlx::query("UPDATE contacts SET email = $2, updated_at = now() WHERE id = $1")
                    .bind(contact.id)
                    .bind(&email)
                    .execute(&mut *db)
                    .await?;
                sqlx::query("UPDATE nary_meetings SET attendee_email = $2, updated_at = now() WHERE id = $1")
                    .bind(meeting.id)
                    .bind(&email)
                    .execute(&mut *db)
                    .await?;
            }
        }
    }
    let wants_other = truthy(data.get("wants_other_times"));
    let picked: Option<i64> = data
        .get("picked")
        .filter(|p| valid_pick(p, slot_list.len()))
        .and_then(Value::as_i64);
    if wants_other || picked.is_none() {
        commit(db).await?;
        return Ok(if !wants_other {
            None
        } else {
            Some(json!({"meeting_id": meeting.id.to_string(), "status": "wants_other_times"}))
        });
    }
    let picked = picked.expect("in range");
    let picked_json = json!(picked);
    let card = replies::_propose(
        db,
        "meeting.book",
        json!({"meeting_id": meeting.id.to_string(), "picked": picked_json}),
        run.as_deref(),
        None,
        Some(&format!(
            "{} picked option {}.",
            conv.label(),
            py_str(&picked_json)
        )),
        None,
    )
    .await?;
    Ok(Some(json!({
        "meeting_id": meeting.id.to_string(), "picked": picked_json,
        "card_id": card.as_ref().map(|c| c.id.to_string()),
        "status": card.as_ref().map(|c| c.status.clone()),
    })))
}

/// `valid_pick(picked, slots)`: a pick is the number of an offered slot, 1 to len(slots). Not a bool (True is an
/// int in Python) and not 0, which slots[picked - 1] would read as the last slot.
pub fn valid_pick(picked: &Value, slots: usize) -> bool {
    match picked {
        Value::Number(n) => n.as_i64().is_some_and(|p| 1 <= p && p <= slots as i64),
        _ => false,
    }
}

/// Applying meeting.book: the Google event with Meet, then a confirmation to the customer.
pub async fn book(db: &mut PgConnection, meeting_id: Uuid, picked: i64) -> ApiResult<Value> {
    let meeting = load_meeting(db, meeting_id).await?;
    let Some(meeting) = meeting.filter(|m| m.status == "slots_sent") else {
        return Err(ApiError::http(409, "This meeting isn't waiting for a pick"));
    };
    let list: Vec<Value> = meeting.slots.as_array().cloned().unwrap_or_default();
    if !valid_pick(&json!(picked), list.len()) {
        return Err(ApiError::http(
            400,
            format!("Pick one of the offered times, 1 to {}", list.len()),
        ));
    }
    let slot = &list[(picked - 1) as usize];
    let start = fromisoformat(
        slot.get("start")
            .and_then(|v| v.as_str())
            .ok_or_else(|| internal("KeyError: 'start'"))?,
    )?;
    let end = fromisoformat(
        slot.get("end")
            .and_then(|v| v.as_str())
            .ok_or_else(|| internal("KeyError: 'end'"))?,
    )?;
    let t = texts(db, meeting.language.as_deref()).await?;
    let company = _company_name(db, &meeting).await?;
    let title = py_format(tget(&t, "title")?, &[("company", &company)])?;
    let recording = tget(&t, "recording")?.to_string();
    let event = google_calendar::create_event(
        db,
        &title,
        &recording,
        instant(&start),
        instant(&end),
        std::slice::from_ref(&meeting.attendee_email),
    )
    .await?;
    let event_id = event.get("id").and_then(|v| v.as_str()).map(str::to_string);
    let meet_link = event
        .get("meet_link")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    sqlx::query(
        "UPDATE nary_meetings SET status = 'booked', picked = $2, starts_at = $3, ends_at = $4, event_id = $5, \
         meet_link = $6, title = $7, updated_at = now() WHERE id = $1",
    )
    .bind(meeting_id)
    .bind(picked as i32)
    .bind(instant(&start))
    .bind(instant(&end))
    .bind(&event_id)
    .bind(&meet_link)
    .bind(&title)
    .execute(&mut *db)
    .await?;
    events::emit(
        db,
        "meeting.booked",
        Some(if meeting.deal_id.is_some() { "deal" } else { "lead" }),
        meeting.deal_id.or(meeting.lead_id),
        Some(json!({"meeting_id": meeting.id.to_string(), "starts_at": start.isoformat(), "label": title})),
        None,
    )
    .await?;
    commit(db).await?;
    let mut meeting = meeting;
    meeting.status = "booked".into();
    let customer_zone = match meeting.customer_tz.as_deref().filter(|z| !z.is_empty()) {
        Some(z) => zone(z)?,
        None => granary_core::clock::local_tz(db).await?,
    };
    let when = strftime(&astimezone(&start, customer_zone), "%a %d %b %H:%M");
    let confirm = match meet_link.as_deref().filter(|l| !l.is_empty()) {
        Some(l) => format!("{when}: {l}"),
        None => when.clone(),
    };
    let text = format!("{title}\n{confirm}\n{recording}");
    match _send_to_customer(db, &meeting, &text, &title).await {
        Ok(_) => {}
        Err(e @ SendError::QuietHours { .. }) => {
            // Booked by Nary at night: the event and Google's invitation stand; the WhatsApp confirmation waits.
            executor::schedule_message(
                db,
                json!({"chat_id": meeting.chat_id, "text": text,
                       "contact_id": meeting.contact_id.map(|c| c.to_string())}),
                &e,
                &format!("The confirmation of {title}."),
            )
            .await?;
        }
        Err(SendError::Api(e)) if !is_http_exception(&e) => return Err(e),
        Err(e) => {
            // the invitation went by Google anyway when there is an email
            tracing::info!(meeting_id = %meeting_id, reason = %e.detail(), "nary_meeting_confirm_not_sent");
        }
    }
    Ok(
        json!({"label": format!("Booked {title} {when}"), "event_id": event.get("id").cloned().unwrap_or(Value::Null),
              "meet_link": event.get("meet_link").cloned().unwrap_or(Value::Null)}),
    )
}

/// The message with the Meet link where it says {link}, or on its own last line.
pub fn with_link(text: &str, link: Option<&str>) -> String {
    match link.filter(|l| !l.is_empty()) {
        None => pystr::strip(&text.replace("{link}", "")).to_string(),
        Some(l) if text.contains("{link}") => text.replace("{link}", l),
        Some(l) => format!("{}\n{l}", pystr::rstrip(text)),
    }
}

/// `book_on_request(db, user, *, title, start, duration_minutes=None, staff_ids=(), customer_email=None,
/// lead_id=None, deal_id=None, contact_id=None, message=None)`: a meeting at the time a person gave,
/// confirmed on its card. The event is created first; a message that then fails leaves the meeting booked
/// and says so. The caller checks permissions. `start` may be naive (company time).
pub async fn book_on_request(
    db: &mut PgConnection,
    user: &AuthUser,
    title: &str,
    start: PyDateTime,
    duration_minutes: Option<i64>,
    staff_ids: &[String],
    customer_email: Option<&str>,
    lead_id: Option<&str>,
    deal_id: Option<&str>,
    contact_id: Option<&str>,
    message: Option<&Value>,
) -> ApiResult<Value> {
    let tz = local_tz(db).await?;
    let start = if start.tz.is_some() {
        start
    } else {
        PyDateTime::in_zone(start.local, tz, 0)
    };
    if instant(&start) <= Utc::now() {
        return Err(ApiError::http(400, "The meeting time has already passed"));
    }
    let minutes = match duration_minutes.filter(|m| *m != 0) {
        Some(m) => m,
        None => slots::py_int(&rules(db).await?["duration_minutes"])?,
    };
    if !(5..=600).contains(&minutes) {
        return Err(ApiError::http(
            400,
            "The duration must be between 5 and 600 minutes",
        ));
    }
    // start + timedelta(minutes=...): wall-clock arithmetic in the start's own zone.
    let end = PyDateTime {
        local: start.local + Duration::minutes(minutes),
        tz: match start.tz {
            Some(PyTz::ZoneFold1(z)) => Some(PyTz::Zone(z)),
            other => other,
        },
    };
    let mut ids: Vec<Uuid> = Vec::new();
    for i in staff_ids {
        ids.push(parse_uuid(i)?);
    }
    ids.push(user.id);
    let staff: Vec<(String,)> =
        sqlx::query_as("SELECT email FROM users WHERE id = ANY($1) AND is_active = true")
            .bind(&ids)
            .fetch_all(&mut *db)
            .await?;
    let deal = match deal_id.filter(|d| !d.is_empty()) {
        Some(d) => load_deal(db, Some(parse_uuid(d)?)).await?,
        None => None,
    };
    let mut lead = match lead_id.filter(|l| !l.is_empty()) {
        Some(l) => load_lead(db, Some(parse_uuid(l)?)).await?,
        None => None,
    };
    if let (Some(d), None) = (deal.as_ref(), lead.as_ref()) {
        if d.lead_id.is_some() {
            lead = load_lead(db, d.lead_id).await?;
        }
    }
    let cid: Option<Uuid> = match contact_id.filter(|c| !c.is_empty()) {
        Some(c) => Some(parse_uuid(c)?),
        None => deal
            .as_ref()
            .and_then(|d| d.contact_id)
            .or_else(|| lead.as_ref().and_then(|l| l.contact_id)),
    };
    let contact = load_contact(db, cid).await?;
    let customer_email: Option<String> = {
        let raw = customer_email
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .or_else(|| {
                contact
                    .as_ref()
                    .and_then(|c| c.email.clone())
                    .filter(|e| !e.is_empty())
            })
            .unwrap_or_default();
        let s = pystr::strip(&raw).to_string();
        (!s.is_empty()).then_some(s)
    };
    let msg = message.filter(|m| truthy(Some(m)));
    let mget = |k: &str| msg.and_then(|m| m.get(k));
    let chat_id: Option<String> = if mget("channel") == Some(&json!("whatsapp")) {
        mget("chat_id").filter(|v| !v.is_null()).map(py_str)
    } else {
        None
    };
    let chat: Option<WaChat> = match chat_id.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => {
            sqlx::query_as::<_, WaChat>("SELECT * FROM wa_chats WHERE chat_id = $1")
                .bind(c)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let country: Option<String> = contact
        .as_ref()
        .and_then(|c| c.country.clone())
        .filter(|c| c.chars().count() == 2)
        .or_else(|| {
            crate::nary::phones::country_of(chat.as_ref().and_then(|c| c.phone.as_deref()))
        });
    let cust_tz = customer_tz(country.as_deref(), tz.name());
    let mut emails: Vec<Option<String>> = staff.into_iter().map(|(e,)| Some(e)).collect();
    emails.push(customer_email.clone());
    let event = google_calendar::create_event(
        db,
        title,
        &format!("Booked by {} through Nary.", user.name),
        instant(&start),
        instant(&end),
        &emails,
    )
    .await?;
    let ev_str = |k: &str| {
        event
            .get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let meet_link = ev_str("meet_link");
    let html_link = ev_str("html_link");
    let event_id = event.get("id").and_then(|v| v.as_str()).map(str::to_string);
    let link = meet_link.clone().or_else(|| html_link.clone());
    let invited: Vec<String> = emails
        .iter()
        .flatten()
        .filter(|e| !e.is_empty())
        .cloned()
        .collect();
    let meeting_id = Uuid::new_v4();
    let channel = mget("channel")
        .filter(|v| truthy(Some(v)))
        .map(py_str)
        .unwrap_or_else(|| "whatsapp".into());
    sqlx::query(
        "INSERT INTO nary_meetings (id, deal_id, lead_id, contact_id, chat_id, channel, customer_tz, country, status, \
         origin, booked_by_id, starts_at, ends_at, event_id, meet_link, title, attendee_email, attendees, slots) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'booked', 'request', $9, $10, $11, $12, $13, $14, $15, $16, '[]'::jsonb)",
    )
    .bind(meeting_id)
    .bind(deal.as_ref().map(|d| d.id))
    .bind(lead.as_ref().map(|l| l.id))
    .bind(contact.as_ref().map(|c| c.id))
    .bind(&chat_id)
    .bind(&channel)
    .bind(&cust_tz)
    .bind(&country)
    .bind(user.id)
    .bind(instant(&start))
    .bind(instant(&end))
    .bind(&event_id)
    .bind(event.get("meet_link").and_then(|v| v.as_str()))
    .bind(title)
    .bind(&customer_email)
    .bind(json!(invited))
    .execute(&mut *db)
    .await?;
    let when = strftime(&astimezone(&start, tz), "%a %d %b %H:%M");
    let entity: Option<(&str, Uuid)> = if let Some(d) = &deal {
        Some(("deal", d.id))
    } else if let Some(l) = &lead {
        Some(("lead", l.id))
    } else {
        contact.as_ref().map(|c| ("contact", c.id))
    };
    if let Some((etype, eid)) = entity {
        let content = format!("{when}, {minutes} min. Invited: {}.", invited.join(", "))
            + &meet_link
                .as_ref()
                .map(|l| format!("\nGoogle Meet: {l}"))
                .unwrap_or_default();
        sqlx::query(
            "INSERT INTO activities (id, entity_type, entity_id, deal_id, lead_id, contact_id, activity_type, direction, \
             subject, scheduled_at, duration_minutes, is_completed, created_by_id, assigned_to_id, content, channel_data) \
             VALUES ($1, $2, $3, $4, $5, $6, 'meeting', 'outbound', $7, $8, $9, false, $10, $10, $11, $12)",
        )
        .bind(Uuid::new_v4())
        .bind(etype)
        .bind(eid)
        .bind(deal.as_ref().map(|d| d.id))
        .bind(lead.as_ref().map(|l| l.id))
        .bind(contact.as_ref().map(|c| c.id))
        .bind(title)
        .bind(instant(&start))
        .bind(minutes as i32)
        .bind(user.id)
        .bind(content)
        .bind(json!({"event_id": event.get("id").cloned().unwrap_or(Value::Null),
                     "meet_link": event.get("meet_link").cloned().unwrap_or(Value::Null)}))
        .execute(&mut *db)
        .await?;
    }
    if deal.is_some() || lead.is_some() {
        events::emit(
            db,
            "meeting.booked",
            Some(if deal.is_some() { "deal" } else { "lead" }),
            deal.as_ref().map(|d| d.id).or(lead.as_ref().map(|l| l.id)),
            Some(json!({"meeting_id": meeting_id.to_string(), "starts_at": start.isoformat(), "label": title})),
            None,
        )
        .await?;
    }
    commit(db).await?;
    let mut label = format!("Booked {title} {when}")
        + &meet_link
            .as_ref()
            .map(|l| format!(", Meet: {l}"))
            .unwrap_or_default();
    let mut out = json!({"id": meeting_id.to_string(), "label": "", "meet_link": event.get("meet_link").cloned().unwrap_or(Value::Null),
                         "html_link": event.get("html_link").cloned().unwrap_or(Value::Null), "sent": null, "sent_error": null});
    if let Some(m) = msg {
        let mut fields = Map::new();
        for k in MESSAGE_FIELDS {
            if let Some(v) = m.get(k).filter(|v| truthy(Some(v))) {
                fields.insert(k.to_string(), v.clone());
            }
        }
        let text = fields.get("text").map(py_str).unwrap_or_default();
        fields.insert(
            "text".into(),
            Value::String(with_link(&text, link.as_deref())),
        );
        // fields["channel"]: a KeyError when the message has no channel (raised again in the except: a 500).
        let channel = fields
            .get("channel")
            .map(py_str)
            .ok_or_else(|| internal("KeyError: 'channel'"))?;
        let what = channel.replace("whatsapp", "WhatsApp");
        match executor::send_on_instruction(
            db,
            user,
            &format!("{channel}.reply.send"),
            Value::Object(fields),
            None,
        )
        .await
        {
            Ok(row) => {
                let pick = |v: &Option<Value>| {
                    v.as_ref()
                        .and_then(|x| x.get("label"))
                        .filter(|x| truthy(Some(x)))
                        .cloned()
                };
                out["sent"] = pick(&row.result)
                    .or_else(|| pick(&row.diff))
                    .unwrap_or_else(|| json!("sent"));
                label += &format!("; the {what} message was sent");
            }
            Err(e) => {
                // str(e.detail if HTTPException else e).removeprefix("Not sent: ")
                let raw = if is_http_exception(&e) {
                    http_detail(&e).unwrap_or_default()
                } else {
                    py_exception(&e).1
                };
                let detail = raw.strip_prefix("Not sent: ").unwrap_or(&raw).to_string();
                out["sent_error"] = Value::String(detail.clone());
                label += &format!("; the {what} message was NOT sent: {detail}");
                if meet_link.is_some() {
                    label += ". Send the Meet link yourself";
                } else if let Some(l) = &link {
                    label += &format!(". Send the event link yourself: {l}");
                }
                tracing::info!(meeting_id = %meeting_id, reason = %py_slice(&detail, 200), "nary_meeting_message_not_sent");
            }
        }
    }
    out["label"] = Value::String(label);
    Ok(out)
}

async fn _company_name(db: &mut PgConnection, meeting: &NaryMeeting) -> ApiResult<String> {
    if let Some(deal) = load_deal(db, meeting.deal_id).await? {
        if let Some(org_id) = deal.organization_id {
            let org: Option<Organization> =
                sqlx::query_as("SELECT * FROM organizations WHERE id = $1")
                    .bind(org_id)
                    .fetch_optional(&mut *db)
                    .await?;
            if let Some(org) = org {
                return Ok(org.name);
            }
        }
    }
    if let Some(c) = load_contact(db, meeting.contact_id).await? {
        let name = [Some(c.first_name.as_str()), c.last_name.as_deref()]
            .into_iter()
            .flatten()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        return Ok(if name.is_empty() {
            "Customer".into()
        } else {
            name
        });
    }
    Ok("Customer".into())
}

// ─── The tick: reminders, brief, no-shows ─────────────────────────────────────

fn rem_int(rem: &Value, key: &str, default: i64) -> ApiResult<i64> {
    if truthy(rem.get(key)) {
        slots::py_int(&rem[key])
    } else {
        Ok(default)
    }
}

pub async fn tick(db: &mut PgConnection) -> ApiResult<Value> {
    let now = Utc::now();
    let tz = local_tz(db).await?;
    let rem = require_json(db, "nary.meetings.reminders", WHERE).await?;
    let mut done = Map::new();
    for k in ["day", "hour", "brief", "debrief_asked"] {
        done.insert(k.into(), json!(0));
    }
    let bump = |done: &mut Map<String, Value>, k: &str| {
        let n = done.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
        done.insert(k.into(), json!(n + 1));
    };
    let rows: Vec<NaryMeeting> = sqlx::query_as(&format!(
        "SELECT {MEETING_COLS} FROM nary_meetings WHERE status = 'booked'"
    ))
    .fetch_all(&mut *db)
    .await?;
    let day_at_raw = rem
        .get("day_before_at")
        .filter(|v| truthy(Some(v)))
        .map(py_str)
        .unwrap_or_else(|| "09:00".into());
    let day_at = NaiveTime::parse_from_str(&day_at_raw, "%H:%M").map_err(|_| {
        internal(format!(
            "time data {day_at_raw:?} does not match format '%H:%M'"
        ))
    })?;
    for mut m in rows {
        let starts_at = m
            .starts_at
            .ok_or_else(|| internal("'NoneType' object has no attribute 'astimezone'"))?;
        let local_start = PyDateTime::from_utc_in_zone(starts_at.naive_utc(), tz);
        let t = texts(db, m.language.as_deref()).await?;
        let cust_zone = zone(
            m.customer_tz
                .as_deref()
                .filter(|z| !z.is_empty())
                .unwrap_or(tz.name()),
        )?;
        let when = when_in(starts_at, cust_zone);
        let day_due = PyDateTime::in_zone(
            (local_start.local.date() - Duration::days(1)).and_time(day_at),
            tz,
            0,
        )
        .to_utc_naive()
        .and_utc();
        let nudge = m.origin != "request"; // booked on a person's request: they tell the customer themselves
        let hour_before = Duration::minutes(rem_int(&rem, "hour_before_minutes", 60)?);
        let day_window = day_due <= now && now < starts_at - Duration::hours(3);
        let hour_window = starts_at - hour_before <= now && now < starts_at;
        let reminder_due = nudge
            && ((m.day_reminder_at.is_none() && day_window)
                || (m.hour_reminder_at.is_none() && hour_window));
        if reminder_due && !_still_in_calendar(db, &m).await? {
            cancel(
                db,
                &mut m,
                Some("it was deleted in Google Calendar"),
                None,
                false,
            )
            .await?;
            bump(&mut done, "cancelled");
            commit(db).await?;
            continue;
        }
        let link = m.meet_link.clone().unwrap_or_default();
        if nudge && m.day_reminder_at.is_none() && day_window {
            m.day_reminder_at = Some(now);
            sqlx::query(
                "UPDATE nary_meetings SET day_reminder_at = $2, updated_at = now() WHERE id = $1",
            )
            .bind(m.id)
            .bind(now)
            .execute(&mut *db)
            .await?;
            let text = py_format(
                tget(&t, "reminder_day")?,
                &[("when", &when), ("link", &link)],
            )?;
            replies::_propose(
                db,
                "meeting.reminder.send",
                json!({"meeting_id": m.id.to_string(), "text": text}),
                None,
                None,
                Some("Meeting reminder (day before)"),
                None,
            )
            .await?;
            bump(&mut done, "day");
        }
        if nudge && m.hour_reminder_at.is_none() && hour_window {
            m.hour_reminder_at = Some(now);
            sqlx::query(
                "UPDATE nary_meetings SET hour_reminder_at = $2, updated_at = now() WHERE id = $1",
            )
            .bind(m.id)
            .bind(now)
            .execute(&mut *db)
            .await?;
            let text = py_format(
                tget(&t, "reminder_hour")?,
                &[("when", &when), ("link", &link)],
            )?;
            replies::_propose(
                db,
                "meeting.reminder.send",
                json!({"meeting_id": m.id.to_string(), "text": text}),
                None,
                None,
                Some("Meeting reminder (1 hour before)"),
                None,
            )
            .await?;
            bump(&mut done, "hour");
        }
        let brief_before = Duration::minutes(rem_int(&rem, "owner_brief_minutes", 30)?);
        if m.brief_at.is_none() && starts_at - brief_before <= now && now < starts_at {
            if !_still_in_calendar(db, &m).await? {
                cancel(
                    db,
                    &mut m,
                    Some("it was deleted in Google Calendar"),
                    None,
                    false,
                )
                .await?;
                bump(&mut done, "cancelled");
                commit(db).await?;
                continue;
            }
            m.brief_at = Some(now);
            sqlx::query("UPDATE nary_meetings SET brief_at = $2, updated_at = now() WHERE id = $1")
                .bind(m.id)
                .bind(now)
                .execute(&mut *db)
                .await?;
            owner_brief(db, &m).await?;
            bump(&mut done, "brief");
        }
        let no_show_after = Duration::minutes(rem_int(&rem, "no_show_minutes", 15)?);
        if let Some(ends_at) = m.ends_at {
            if now > ends_at + no_show_after {
                m.status = "done".into();
                set_status(db, m.id, "done").await?;
                let ids = roles::approver_ids(db).await?;
                let title = pystr::strip(&format!(
                    "How did the call go? {}",
                    m.title.clone().unwrap_or_default()
                ))
                .to_string();
                let link = format!("/nary/meetings?meeting={}", m.id);
                let eid = m.id.to_string();
                notify(
                    db,
                    ids,
                    Notice::new(
                        &title,
                        "Send Nary a voice note or upload the transcript (Nary → Meetings). If the customer didn't \
show up, mark it there and Nary proposes a new time.",
                        &link,
                        "nary_meeting",
                        &eid,
                    )
                    .type_("nary_meeting")
                    .email(false)
                    .whatsapp(true),
                )
                .await?;
                bump(&mut done, "debrief_asked");
            }
        }
        commit(db).await?;
    }
    Ok(Value::Object(done))
}

/// Before a reminder or the brief: is the Google event still there? (13 Oct: the Owner deleted the TEST
/// meetings in Google Calendar and still got their briefs.) Google unreachable: assume it is.
pub async fn _still_in_calendar(db: &mut PgConnection, m: &NaryMeeting) -> ApiResult<bool> {
    let Some(event_id) = m.event_id.as_deref().filter(|e| !e.is_empty()) else {
        return Ok(true);
    };
    match google_calendar::get_event(db, event_id).await {
        Ok(ev) => Ok(ev.is_some()),
        Err(e) => {
            tracing::info!(meeting_id = %m.id, error = %py_slice(&py_exception(&e).1, 200), "nary_meeting_event_check_failed");
            Ok(true)
        }
    }
}

/// Cancel a booked meeting: the Google event (Google tells the invitees), the meeting marked cancelled
/// (no reminders, brief or debrief), its activity marked. Kept, not deleted: the history stays. The
/// caller commits.
pub async fn cancel(
    db: &mut PgConnection,
    m: &mut NaryMeeting,
    reason: Option<&str>,
    actor: Option<&AuthUser>,
    in_google: bool,
) -> ApiResult<Value> {
    if !OPEN.contains(&m.status.as_str()) {
        return Err(ApiError::http(
            409,
            format!("This meeting is already {}", m.status),
        ));
    }
    let event_id = m.event_id.clone().filter(|e| !e.is_empty());
    if in_google {
        if let Some(e) = &event_id {
            google_calendar::cancel_event(db, e).await?;
        }
    }
    m.status = "cancelled".into();
    let why = pystr::strip(reason.unwrap_or("")).to_string();
    let line = if why.is_empty() {
        "Cancelled".to_string()
    } else {
        format!("Cancelled: {why}")
    };
    m.notes = Some(
        [m.notes.clone().filter(|n| !n.is_empty()), Some(line)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n"),
    );
    sqlx::query("UPDATE nary_meetings SET status = 'cancelled', notes = $2, updated_at = now() WHERE id = $1")
        .bind(m.id)
        .bind(&m.notes)
        .execute(&mut *db)
        .await?;
    if let Some(e) = &event_id {
        let acts: Vec<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, subject, content FROM activities WHERE activity_type = 'meeting' AND (channel_data ->> 'event_id') = $1",
        )
        .bind(e)
        .fetch_all(&mut *db)
        .await?;
        for (id, subject, content) in acts {
            let subject_now = subject.clone().unwrap_or_default();
            let new_subject = if !subject_now.starts_with("Cancelled: ") {
                let base = subject
                    .filter(|s| !s.is_empty())
                    .or_else(|| m.title.clone().filter(|t| !t.is_empty()))
                    .unwrap_or_else(|| "meeting".into());
                Some(py_slice(&format!("Cancelled: {base}"), 255).to_string())
            } else {
                subject
            };
            let tail = if why.is_empty() {
                String::new()
            } else {
                format!(" ({why})")
            };
            let new_content = pystr::strip(&format!(
                "{}\nCancelled{tail}.",
                content.unwrap_or_default()
            ))
            .to_string();
            sqlx::query(
                "UPDATE activities SET subject = $2, content = $3, is_completed = true, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(new_subject)
            .bind(new_content)
            .execute(&mut *db)
            .await?;
        }
    }
    let name = m.title.clone().filter(|t| !t.is_empty());
    if actor.is_none() {
        // noticed by Nary (deleted in Google): tell the Owner in one line
        let title = format!(
            "Meeting cancelled: {}",
            name.clone().unwrap_or_else(|| "meeting".into())
        );
        let the = name.clone().unwrap_or_else(|| "The meeting".into());
        let ids = roles::approver_ids(db).await?;
        let on = owner_channel::enabled(db).await?;
        let body = format!("{the}: {why}. No brief or reminders.");
        let link = format!("/nary/meetings?meeting={}", m.id);
        let eid = m.id.to_string();
        notify(
            db,
            ids,
            Notice::new(&title, &body, &link, "nary_meeting", &eid)
                .type_("nary_meeting")
                .email(false)
                .whatsapp(!on),
        )
        .await?;
        if owner_channel::enabled(db).await? {
            owner_channel::raise_topic(
                db,
                owner_channel::Topic::new(
                    "alert",
                    &title,
                    &format!("{the} is cancelled: {why}. No brief or reminders."),
                ),
            )
            .await?;
        }
    }
    let when = match m.starts_at {
        Some(s) => when_in(s, local_tz(db).await?),
        None => String::new(),
    };
    Ok(json!({"id": m.id.to_string(),
              "label": pystr::strip(&format!("Cancelled {} {when}", name.unwrap_or_else(|| "meeting".into()))).to_string()}))
}

pub async fn send_reminder(
    db: &mut PgConnection,
    meeting_id: Uuid,
    text: &str,
) -> ApiResult<Value> {
    let meeting = load_meeting(db, meeting_id).await?;
    let Some(meeting) = meeting.filter(|m| m.status == "booked") else {
        return Err(ApiError::http(409, "The meeting isn't booked any more"));
    };
    let subject = meeting
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Meeting reminder".into());
    let mut out = _send_to_customer(db, &meeting, text, &subject).await?;
    out.insert("label".into(), json!("Meeting reminder"));
    Ok(Value::Object(out))
}

/// A one-page brief for the Owner 30 minutes before (Opus, internal).
pub async fn owner_brief(db: &mut PgConnection, meeting: &NaryMeeting) -> ApiResult<()> {
    let starts = meeting
        .starts_at
        .map(|s| PyDateTime::from_chrono_utc(&s).isoformat())
        .ok_or_else(|| internal("'NoneType' object has no attribute 'isoformat'"))?;
    let mut facts = Map::new();
    facts.insert("meeting".into(), json!(meeting.title));
    facts.insert("starts".into(), json!(starts));
    if meeting.deal_id.is_some() {
        let deal = load_deal(db, meeting.deal_id)
            .await?
            .ok_or_else(|| internal("'NoneType' object has no attribute 'title'"))?;
        let value = deal
            .value
            .filter(|v| !v.is_zero())
            .map(|v| v.to_string())
            .unwrap_or_default();
        facts.insert(
            "deal".into(),
            json!({"title": deal.title, "stage": deal.stage, "value": value, "currency": deal.currency}),
        );
    }
    if let Some(s) = crate::nary::scoring::latest(db, meeting.lead_id, meeting.deal_id).await? {
        facts.insert(
            "bant".into(),
            json!({"budget": s.budget, "authority": s.authority, "need": s.need, "timeline": s.timeline,
                   "class": s.class, "rationale": s.rationale, "red_flags": s.red_flags}),
        );
    }
    if let Some(cid) = meeting.contact_id {
        let entries = crate::nary::memory::recall(
            db,
            crate::nary::memory::Recall {
                layers: vec!["customer".into()],
                subject_type: Some("contact".into()),
                subject_id: Some(cid.to_string()),
                limit: 15,
                ..Default::default()
            },
        )
        .await?;
        facts.insert(
            "notes".into(),
            json!(crate::nary::memory::as_context(&entries)),
        );
    }
    // Only the meeting's own deal or lead: a missing one compared as IS NULL and pulled every other customer's
    // notes. Python leaves a missing id out of the OR; `= NULL` is never true, which is the same filter.
    let insights: Vec<NaryInsight> = if meeting.deal_id.is_some() || meeting.lead_id.is_some() {
        sqlx::query_as(
            "SELECT * FROM nary_insights WHERE deal_id = $1 OR lead_id = $2 ORDER BY created_at DESC LIMIT 3",
        )
        .bind(meeting.deal_id)
        .bind(meeting.lead_id)
        .fetch_all(&mut *db)
        .await?
    } else {
        Vec::new()
    };
    facts.insert(
        "insights".into(),
        Value::Array(
            insights
                .iter()
                .map(|i| json!({"kind": i.kind, "content": i.content}))
                .collect(),
        ),
    );
    let facts = Value::Object(facts);
    let facts_json = py_json(&facts);
    let mut text: Option<String> = None;
    let system = persona::system_prompt(
        "staff",
        "Write the Owner's one-page brief for this call: who, \
what they need, BANT gaps to close, risks, the goal \
of the call. Short numbered points.",
        "",
        "",
    );
    let attempt = async {
        use crate::ai::base::{AiError, ChatRequest};
        use crate::ai::registry::{use_ai, UseAiOptions};
        let ai = use_ai(db, "nary.meeting_brief", UseAiOptions::default()).await?;
        let result = ai
            .chat(db, ChatRequest::user(system, facts_json.clone()))
            .await?;
        Ok::<_, AiError>(if result.refused() {
            None
        } else {
            Some(pystr::strip(&result.text).to_string())
        })
    };
    match attempt.await {
        Ok(t) => text = t,
        Err(e) if e.is_http_exception() => {
            tracing::info!(reason = %e.detail().unwrap_or_default(), "nary_meeting_brief_plain");
        }
        Err(e) => return Err(e.into()),
    }
    let title = format!(
        "In 30 minutes: {}",
        meeting.title.clone().unwrap_or_else(|| "None".into())
    );
    let body = match text.filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => py_slice(&facts_json, 1500).to_string(),
    };
    let link = format!("/nary/meetings?meeting={}", meeting.id);
    let eid = meeting.id.to_string();
    let mut tell =
        owner_channel::Tell::new(&title, &body, &link, "nary_meeting", &eid, "nary_meeting");
    tell.kind = "brief";
    tell.anytime = true;
    tell.facts = Some(facts);
    tell.ref_type = Some("nary_meeting");
    tell.ref_id = Some(meeting.id);
    owner_channel::tell(db, tell).await?;
    Ok(())
}

// ─── After the call ───────────────────────────────────────────────────────────

/// `EXTRACT_SCHEMA`.
pub fn extract_schema() -> Value {
    let part = json!({"type": ["integer", "null"]});
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string"},
            "bant": {"type": "object", "properties": {"budget": part, "authority": part, "need": part, "timeline": part},
                     "required": ["budget", "authority", "need", "timeline"], "additionalProperties": false},
            "requirements": {"type": "array", "items": {"type": "string"}},
            "commitments": {"type": "array", "items": {"type": "object", "properties": {
                "who": {"type": "string", "enum": ["us", "customer"]}, "what": {"type": "string"},
                "due": {"type": ["string", "null"]}}, "required": ["who", "what", "due"], "additionalProperties": false}},
            "objections": {"type": "array", "items": {"type": "string"}},
            "next_steps": {"type": "array", "items": {"type": "string"}},
            "language": {"type": "string", "enum": ["en"]},
            // batch 2 T3 (A6): what the call settled about the deal (proposed to the Owner as a card)
            "deal_update": {"type": "object", "properties": {
                "stage": {"anyOf": [{"type": "string", "enum": DEAL_UPDATE_STAGES}, {"type": "null"}]},
                "value": {"type": ["number", "null"]},
                "currency": {"type": ["string", "null"]},
                "reason": {"type": "string"}},
                "required": ["stage", "value", "currency", "reason"], "additionalProperties": false},
        },
        "required": ["summary", "bant", "requirements", "commitments", "objections", "next_steps", "language",
                     "deal_update"],
        "additionalProperties": false,
    })
}

/// The stages a call may move a deal to (batch 2 T3): the open ladder, won and nurture. Lost and
/// disqualified need reasons from their picklists, so a person sets them.
pub const DEAL_UPDATE_STAGES: [&str; 7] = [
    "qualifying",
    "discovery",
    "assessment",
    "proposal",
    "contract",
    "won",
    "nurture",
];

const EXTRACT_REQUIRED: [&str; 7] = [
    "summary",
    "bant",
    "requirements",
    "commitments",
    "objections",
    "next_steps",
    "language",
];

/// `bytes.decode("utf-8", "ignore")`: invalid sequences (maximal subparts) are dropped.
fn decode_ignore(data: &[u8]) -> String {
    data.utf8_chunks().map(|c| c.valid()).collect()
}

/// `zipfile.ZipFile(io.BytesIO(data)).read(name)`: the central directory, then the entry (stored or
/// deflated). Not a zip, or no such entry: an error (BadZipFile / KeyError, a 500 in Python).
fn zip_read(data: &[u8], name: &str) -> ApiResult<Vec<u8>> {
    let bad = |m: &str| ApiError::internal(anyhow::anyhow!("{m}"));
    let u16le = |at: usize| -> ApiResult<usize> {
        data.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
            .ok_or_else(|| bad("File is not a zip file"))
    };
    let u32le = |at: usize| -> ApiResult<usize> {
        data.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| bad("File is not a zip file"))
    };
    // End of central directory: the last "PK\x05\x06" within the final 64 KiB + 22 bytes.
    let lo = data.len().saturating_sub(22 + 65535);
    let eocd = (lo..data.len().saturating_sub(21))
        .rev()
        .find(|&i| data[i..i + 4] == *b"PK\x05\x06")
        .ok_or_else(|| bad("File is not a zip file"))?;
    let count = u16le(eocd + 10)?;
    let mut at = u32le(eocd + 16)?;
    for _ in 0..count {
        if data.get(at..at + 4) != Some(b"PK\x01\x02") {
            return Err(bad("Bad magic number for central directory"));
        }
        let method = u16le(at + 10)?;
        let csize = u32le(at + 20)?;
        let nlen = u16le(at + 28)?;
        let elen = u16le(at + 30)?;
        let clen = u16le(at + 32)?;
        let local = u32le(at + 42)?;
        let fname = data
            .get(at + 46..at + 46 + nlen)
            .ok_or_else(|| bad("Truncated central directory"))?;
        if fname == name.as_bytes() {
            if data.get(local..local + 4) != Some(b"PK\x03\x04") {
                return Err(bad("Bad magic number for file header"));
            }
            let start = local + 30 + u16le(local + 26)? + u16le(local + 28)?;
            let raw = data
                .get(start..start + csize)
                .ok_or_else(|| bad("Truncated file"))?;
            return match method {
                0 => Ok(raw.to_vec()),
                8 => {
                    use std::io::Read;
                    let mut out = Vec::new();
                    flate2::read::DeflateDecoder::new(raw)
                        .read_to_end(&mut out)
                        .map_err(|e| bad(&format!("Error -3 while decompressing data: {e}")))?;
                    Ok(out)
                }
                m => Err(bad(&format!(
                    "That compression method is not supported ({m})"
                ))),
            };
        }
        at += 46 + nlen + elen + clen;
    }
    Err(bad(&format!(
        "There is no item named '{name}' in the archive"
    )))
}

/// Python `\s` (str patterns) also matches \x1c-\x1f, which Rust's White_Space does not.
static TIMECODE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?m)^\d+[\s\x1c-\x1f]*$|^\d{2}:\d{2}:\d{2}[.,]\d{3}[\s\x1c-\x1f]*-->.*$|^WEBVTT.*$",
    )
    .expect("regex")
});
static BLANKS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n{2,}").expect("regex"));
static PARA_END: Lazy<Regex> = Lazy::new(|| Regex::new(r"</w:p>").expect("regex"));
static TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").expect("regex"));

/// Plain text from an uploaded transcript: txt, SRT, VTT (cue numbers and times dropped) or docx.
pub fn transcript_text(filename: &str, data: &[u8]) -> ApiResult<String> {
    let name = pystr::lower(filename);
    if name.ends_with(".docx") {
        let xml = decode_ignore(&zip_read(data, "word/document.xml")?);
        let xml = PARA_END.replace_all(&xml, "\n");
        return Ok(pystr::strip(&TAG.replace_all(&xml, "")).to_string());
    }
    let mut text = decode_ignore(data);
    if name.ends_with(".srt") || name.ends_with(".vtt") {
        text = TIMECODE.replace_all(&text, "").into_owned();
        text = BLANKS.replace_all(&text, "\n").into_owned();
    }
    Ok(pystr::strip(&text).to_string())
}

/// Read a debrief or transcript: extract, update the opportunity, set reminders, draft the summary.
pub async fn debrief(
    db: &mut PgConnection,
    meeting: Option<&NaryMeeting>,
    deal: Option<&Deal>,
    text: &str,
    source: &str,
    user: &AuthUser,
) -> ApiResult<Value> {
    let mut deal: Option<Deal> = match deal {
        Some(d) => Some(d.clone()),
        None => match meeting.and_then(|m| m.deal_id) {
            Some(id) => load_deal(db, Some(id)).await?,
            None => None,
        },
    };
    let lead = load_lead(db, meeting.and_then(|m| m.lead_id)).await?;
    // batch 2 T3 (A6): a lead promoted to a deal lives on as the deal: the results, follow-ups and notes go
    // there (they went to the old lead and nobody saw them on the deal), and the meeting is linked to it.
    if deal.is_none() {
        if let Some(did) = lead.as_ref().and_then(|l| l.converted_to_deal_id) {
            deal = load_deal(db, Some(did)).await?;
        }
    }
    if let (Some(m), Some(d)) = (meeting, deal.as_ref()) {
        if m.deal_id.is_none() {
            sqlx::query("UPDATE nary_meetings SET deal_id = $2, updated_at = now() WHERE id = $1")
                .bind(m.id)
                .bind(d.id)
                .execute(&mut *db)
                .await?;
        }
    }
    let mut run = runs::start_run(
        db,
        "manual",
        "nary.transcript_extract",
        Some(source),
        "internal",
        None,
    )
    .await?;
    let system = persona::system_prompt(
        "staff",
        "Read this call record (an Owner debrief or a meeting transcript). Extract: a \
short summary; BANT evidence as 0-3 per part (null if the call said nothing \
about it); requirements; commitments (who: us or customer, what, due as \
YYYY-MM-DD if a date was said); objections; next steps; the customer's \
language (id or en); a deal update only where the call clearly settled it: the \
sales stage the deal reached (null if unchanged or unclear), the deal value and \
its currency when a budget or price was agreed (null otherwise), and the reason \
in one sentence.",
        "",
        "",
    );
    let data = llm::ask_json(
        db,
        "nary.transcript_extract",
        &system,
        &llm::information(source, text, 60000),
        &extract_schema(),
        Some(&mut run),
        None,
        None,
    )
    .await?;
    let Some(data) = data
        .and_then(|d| d.as_object().cloned())
        .filter(|d| !d.is_empty())
    else {
        runs::finish_run(
            db,
            &mut run,
            "failed",
            None,
            Some("the model returned nothing usable"),
        )
        .await?;
        commit(db).await?;
        return Err(ApiError::http(
            502,
            "Nary couldn't read this record. Try again or check the file.",
        ));
    };
    let mut content = Map::new();
    for k in EXTRACT_REQUIRED {
        content.insert(k.into(), data.get(k).cloned().unwrap_or(Value::Null));
    }
    content.insert("source".into(), json!(source));
    let content = Value::Object(content);
    sqlx::query(
        "INSERT INTO nary_insights (id, kind, deal_id, lead_id, meeting_id, content, status, model, run_id) \
         VALUES ($1, 'call', $2, $3, $4, $5, 'final', $6, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(deal.as_ref().map(|d| d.id))
    .bind(lead.as_ref().map(|l| l.id))
    .bind(meeting.map(|m| m.id))
    .bind(&content)
    .bind(data.get("_model").and_then(|v| v.as_str()))
    .bind(run.id)
    .execute(&mut *db)
    .await?;
    if let Some(m) = meeting {
        if m.status == "booked" {
            set_status(db, m.id, "done").await?;
        }
    }
    let approvers = roles::approver_ids(db).await?;
    let owner = approvers.first().copied().unwrap_or(user.id);
    let mut made = 0i64;
    let commitments: Vec<Value> = data
        .get("commitments")
        .filter(|v| truthy(Some(v)))
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    for c in commitments {
        if c.get("who").and_then(|v| v.as_str()) != Some("us") {
            continue;
        }
        let mut due: Option<DateTime<Utc>> = None;
        if let Some(d) = c.get("due").filter(|v| truthy(Some(v))) {
            // datetime.fromisoformat(c["due"]).replace(hour=9, tzinfo=local_tz)
            if let Some(parsed) = d.as_str().and_then(|s| fromisoformat(s).ok()) {
                let tz = local_tz(db).await?;
                let local = parsed
                    .local
                    .date()
                    .and_time(parsed.local.time().with_hour9());
                due = Some(PyDateTime::in_zone(local, tz, 0).to_utc_naive().and_utc());
            }
        }
        let what = c
            .get("what")
            .map(py_str)
            .ok_or_else(|| internal("KeyError: 'what'"))?;
        let mut r = crate::crm::followups::NewReminder::new(
            owner,
            if deal.is_some() { "deal" } else { "lead" },
            py_slice(&what, 200),
        );
        r.entity_id = deal.as_ref().map(|d| d.id).or(lead.as_ref().map(|l| l.id));
        r.note = Some(format!("Committed on the call ({source})."));
        r.due_at = due;
        r.source = "next_action".into();
        r.rule_key = Some(format!("call:{}:{made}", run.id));
        r.created_by_id = Some(user.id);
        crate::crm::followups::create(db, r)
            .await
            .map_err(internal)?;
        made += 1;
    }
    if let Some(d) = &deal {
        let mut bant = Map::new();
        if let Some(Value::Object(b)) = data.get("bant") {
            for (k, v) in b {
                if !v.is_null() {
                    bant.insert(k.clone(), v.clone());
                }
            }
        }
        let mut custom = match &d.custom {
            Value::Object(m) => m.clone(),
            _ => Map::new(),
        };
        custom.insert(
            "nary_call".into(),
            json!({"bant": bant, "next_steps": data.get("next_steps").cloned().unwrap_or(Value::Null)}),
        );
        sqlx::query("UPDATE deals SET custom = $2, updated_at = now() WHERE id = $1")
            .bind(d.id)
            .bind(Value::Object(custom))
            .execute(&mut *db)
            .await?;
    }
    // batch 2 T3 (A6): the results on the record's timeline (the deal when there is one)
    record_results(
        db,
        meeting,
        deal.as_ref(),
        lead.as_ref(),
        &data,
        source,
        user,
    )
    .await?;
    commit(db).await?;
    // batch 2 T3 (A6): what the call settled about the deal (stage, value) waits for the Owner as a card
    let deal_card = match &deal {
        Some(d) => {
            propose_deal_update(db, d, data.get("deal_update"), source, user, run.id).await?
        }
        None => None,
    };
    let summary = _summary_email(
        db,
        deal.as_ref(),
        lead.as_ref(),
        meeting,
        &content,
        &mut run,
    )
    .await?;
    let outcome = format!(
        "{made} commitment reminder(s); summary email {}",
        if summary.is_some() {
            "drafted"
        } else {
            "not drafted"
        }
    );
    runs::finish_run(db, &mut run, "done", Some(&outcome), None).await?;
    commit(db).await?;
    Ok(
        json!({"commitments": made, "summary_card": summary, "extract": content,
               "deal_update_card": deal_card}),
    )
}

/// batch 2 T3 (A6): the call's results as a note on the record's timeline (deal, else lead, else contact).
async fn record_results(
    db: &mut PgConnection,
    meeting: Option<&NaryMeeting>,
    deal: Option<&Deal>,
    lead: Option<&Lead>,
    data: &Map<String, Value>,
    source: &str,
    user: &AuthUser,
) -> ApiResult<()> {
    let contact_id = meeting
        .and_then(|m| m.contact_id)
        .or(deal.and_then(|d| d.contact_id))
        .or(lead.and_then(|l| l.contact_id));
    let entity: Option<(&str, Uuid)> = match (deal, lead, contact_id) {
        (Some(d), _, _) => Some(("deal", d.id)),
        (None, Some(l), _) => Some(("lead", l.id)),
        (None, None, Some(c)) => Some(("contact", c)),
        _ => None,
    };
    let Some((etype, eid)) = entity else {
        return Ok(());
    };
    let list = |k: &str| -> Vec<String> {
        data.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(py_str).filter(|x| !x.is_empty()).collect())
            .unwrap_or_default()
    };
    let mut content = data.get("summary").map(py_str).unwrap_or_default();
    for (label, key) in [
        ("Requirements", "requirements"),
        ("Objections", "objections"),
        ("Next steps", "next_steps"),
    ] {
        let items = list(key);
        if !items.is_empty() {
            content.push_str(&format!("\n{label}: {}", items.join("; ")));
        }
    }
    let commitments: Vec<String> = data
        .get("commitments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|c| {
                    let who = c.get("who").map(py_str).unwrap_or_default();
                    let what = c.get("what").map(py_str).unwrap_or_default();
                    match c.get("due").and_then(Value::as_str) {
                        Some(d) if !d.is_empty() => format!("{who}: {what} (by {d})"),
                        _ => format!("{who}: {what}"),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if !commitments.is_empty() {
        content.push_str(&format!("\nCommitments: {}", commitments.join("; ")));
    }
    let title = meeting
        .and_then(|m| m.title.clone())
        .filter(|t| !t.is_empty())
        .map(|t| format!("Meeting results: {t}"))
        .unwrap_or_else(|| "Meeting results".to_string());
    sqlx::query(
        "INSERT INTO activities (id, entity_type, entity_id, deal_id, lead_id, contact_id, activity_type, subject, \
         content, completed_at, is_completed, created_by_id, assigned_to_id, channel_data) \
         VALUES ($1, $2, $3, $4, $5, $6, 'note', $7, $8, now(), true, $9, $9, $10)",
    )
    .bind(Uuid::new_v4())
    .bind(etype)
    .bind(eid)
    .bind(deal.map(|d| d.id))
    .bind(lead.map(|l| l.id))
    .bind(contact_id)
    .bind(py_slice(&title, 500))
    .bind(content)
    .bind(user.id)
    .bind(json!({"source": source, "meeting_id": meeting.map(|m| m.id.to_string())}))
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// batch 2 T3 (A6): a crm.record_change card for the deal's stage and value when the call settled them and
/// they differ from what the deal has. Returns the card id.
async fn propose_deal_update(
    db: &mut PgConnection,
    deal: &Deal,
    update: Option<&Value>,
    source: &str,
    user: &AuthUser,
    run_id: Uuid,
) -> ApiResult<Option<String>> {
    let Some(u) = update.filter(|u| u.is_object()) else {
        return Ok(None);
    };
    let mut fields = Map::new();
    if let Some(stage) = u.get("stage").and_then(Value::as_str) {
        if DEAL_UPDATE_STAGES.contains(&stage) && stage != deal.stage {
            fields.insert("stage".into(), json!(stage));
        }
    }
    if let Some(v) = u.get("value").and_then(Value::as_f64).filter(|v| *v > 0.0) {
        let current = deal
            .value
            .and_then(|d| d.to_string().parse::<f64>().ok())
            .unwrap_or(0.0);
        if (v - current).abs() >= 0.5 {
            fields.insert("value".into(), json!(v));
            if let Some(c) = u
                .get("currency")
                .and_then(Value::as_str)
                .map(|c| c.trim().to_uppercase())
                .filter(|c| c.len() == 3 && Some(c.as_str()) != deal.currency.as_deref())
            {
                fields.insert("currency".into(), json!(c));
            }
        }
    }
    if fields.is_empty() {
        return Ok(None);
    }
    let reason = u.get("reason").map(py_str).unwrap_or_default();
    let rationale = format!(
        "From the meeting record ({source}): {}",
        if reason.is_empty() {
            "the call settled it."
        } else {
            reason.as_str()
        }
    );
    let card = match super::executor::propose(
        db,
        "crm.record_change",
        json!({"object_type": "deal", "action": "update", "record_id": deal.id.to_string(),
               "fields": Value::Object(fields)}),
        user,
        super::executor::ProposeOpts {
            run_id: Some(run_id),
            rationale: Some(&rationale),
            sop_ref: None,
            ..Default::default()
        },
    )
    .await
    {
        Ok(c) => c,
        // Nary is paused: the results are recorded, the proposal is not made
        Err(e) if super::executor::is_nary_paused(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(Some(card.id.to_string()))
}

trait Hour9 {
    fn with_hour9(self) -> NaiveTime;
}

impl Hour9 for NaiveTime {
    /// `.replace(hour=9)`: minutes, seconds and microseconds kept.
    fn with_hour9(self) -> NaiveTime {
        use chrono::Timelike;
        self.with_hour(9).expect("9 is a valid hour")
    }
}

async fn _summary_email(
    db: &mut PgConnection,
    deal: Option<&Deal>,
    lead: Option<&Lead>,
    meeting: Option<&NaryMeeting>,
    extract: &Value,
    run: &mut AgentRun,
) -> ApiResult<Option<String>> {
    let contact_id = meeting
        .and_then(|m| m.contact_id)
        .or(deal.and_then(|d| d.contact_id))
        .or(lead.and_then(|l| l.contact_id));
    let contact = load_contact(db, contact_id).await?;
    let Some((contact, email)) = contact.and_then(|c| {
        let e = c.email.clone().filter(|e| !e.is_empty())?;
        Some((c, e))
    }) else {
        return Ok(None);
    };
    let schema = summary_email_schema();
    let lang = extract
        .get("language")
        .filter(|v| truthy(Some(v)))
        .map(py_str)
        .unwrap_or_else(|| "en".into());
    let system = super::writing::customer_prompt(
        db,
        Some("email"),
        &format!(
            "Write the follow-up email after the call: thank them, the summary in a few \
lines, what each side committed to, the next step. In the customer's language \
({lang}). No sign-off (it is added). Never state \
a price or discount that wasn't on a sent quotation."
        ),
        "",
    )
    .await?;
    let mut picked = Map::new();
    for k in ["summary", "commitments", "next_steps", "requirements"] {
        picked.insert(k.into(), extract.get(k).cloned().unwrap_or(Value::Null));
    }
    let content = py_json(&Value::Object(picked));
    let data = llm::ask_json(
        db,
        "nary.summary_email",
        &system,
        &content,
        &schema,
        Some(&mut *run),
        None,
        None,
    )
    .await?;
    let Some(data) = data
        .and_then(|d| d.as_object().cloned())
        .filter(|d| !d.is_empty())
    else {
        return Ok(None);
    };
    let subject = data
        .get("subject")
        .map(py_str)
        .ok_or_else(|| internal("KeyError: 'subject'"))?;
    let body = data
        .get("body")
        .cloned()
        .ok_or_else(|| internal("KeyError: 'body'"))?;
    let card = replies::_propose(
        db,
        "summary.email.send",
        json!({"to": email, "subject": py_slice(&subject, 200), "text": body,
               "contact_id": contact.id.to_string(), "lead_id": lead.map(|l| l.id.to_string())}),
        Some(&*run),
        None,
        Some("The summary email after the call"),
        None,
    )
    .await?;
    Ok(card.map(|c| c.id.to_string()))
}

/// The Owner says the customer didn't come: the event is cancelled and new slots are proposed.
pub async fn no_show(db: &mut PgConnection, meeting_id: Uuid, user: &AuthUser) -> ApiResult<Value> {
    let _ = user;
    let Some(meeting) = load_meeting(db, meeting_id).await? else {
        return Err(ApiError::http(404, "Meeting not found"));
    };
    set_status(db, meeting_id, "no_show").await?;
    if let Some(e) = meeting.event_id.as_deref().filter(|e| !e.is_empty()) {
        match google_calendar::cancel_event(db, e).await {
            Ok(()) => {}
            Err(err) if is_http_exception(&err) => {}
            Err(err) => return Err(err),
        }
    }
    commit(db).await?;
    let chat: Option<WaChat> = match meeting.chat_id.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => {
            sqlx::query_as("SELECT * FROM wa_chats WHERE chat_id = $1")
                .bind(c)
                .fetch_optional(&mut *db)
                .await?
        }
        None => None,
    };
    let Some(chat) = chat else {
        return Ok(json!({"status": "no_show"}));
    };
    let transcript = crate::nary::channels::conversation_text(db, &chat, None, 12).await?;
    let conv = replies::for_chat(db, &chat, &transcript).await?;
    let new = propose(db, &conv, None, "the customer missed the call", false).await?;
    Ok(json!({"status": "no_show", "new": new}))
}

pub async fn upcoming(db: &mut PgConnection) -> ApiResult<Vec<NaryMeeting>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {MEETING_COLS} FROM nary_meetings WHERE status IN ('proposed', 'slots_sent', 'booked', 'done', 'no_show') \
         ORDER BY created_at DESC LIMIT 100"
    ))
    .fetch_all(&mut *db)
    .await?)
}
