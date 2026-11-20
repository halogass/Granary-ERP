//! doclib A2: the fingerprint log (`library_downloads`) and the trace of a leaked copy.
//!
//! Every copy that leaves the system gets a fresh id (`ID-` + 10 random Crockford base32 characters), one
//! `library_downloads` row (who, what, when, which channel) and, once stamped, the sha256 of the stamped
//! bytes. `trace` reads the marks a copy carries (stamp.rs) or, failing that, matches its hash.

use chrono::{DateTime, Utc};
use granary_core::errors::{ApiError, ApiResult};
use granary_core::settings_store::{require_from, require_int, values};
use rand::Rng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use super::stamp::{self, Stamp};
use super::{Version, WHERE};

/// `library_downloads.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    StaffDownload,
    CustomerSend,
    PartnerDownload,
    NarySend,
    SourceDownload,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::StaffDownload => "staff_download",
            Kind::CustomerSend => "customer_send",
            Kind::PartnerDownload => "partner_download",
            Kind::NarySend => "nary_send",
            Kind::SourceDownload => "source_download",
        }
    }

    /// Copies for people outside the company carry the customer text; staff copies the staff text.
    pub fn is_external(self) -> bool {
        matches!(
            self,
            Kind::CustomerSend | Kind::PartnerDownload | Kind::NarySend
        )
    }
}

/// Who gets the copy (every field optional; the footer fills `{name}` and `{company}` from it).
#[derive(Debug, Clone, Default)]
pub struct Who {
    /// The staff member downloading, or the sender of a customer copy.
    pub user_id: Option<Uuid>,
    /// `{name}`: the staff member, or the customer contact.
    pub name: Option<String>,
    /// `{company}`: the customer or partner company.
    pub company: Option<String>,
    pub partner_account_id: Option<Uuid>,
    pub recipient_name: Option<String>,
    pub recipient_company: Option<String>,
    /// whatsapp | email | portal | web
    pub channel: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

/// One issued fingerprint.
#[derive(Debug, Clone)]
pub struct Issued {
    /// the library_downloads row
    pub row_id: Uuid,
    /// `ID-XXXXXXXXXX`
    pub id: String,
    /// the footer with every placeholder filled
    pub footer_text: String,
    /// `documents.watermark.font_size`
    pub font_size: f64,
}

impl Issued {
    pub fn stamp(&self) -> Stamp {
        Stamp::new(self.footer_text.clone(), self.id.clone()).with_font_size(self.font_size)
    }
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A random fingerprint id: `ID-` + 10 Crockford base32 characters (50 random bits).
pub fn new_id() -> String {
    let mut rng = rand::thread_rng();
    let s: String = (0..10)
        .map(|_| CROCKFORD[rng.gen_range(0..32)] as char)
        .collect();
    format!("ID-{s}")
}

/// "21 Oct 2026".
pub fn footer_date(d: chrono::NaiveDate) -> String {
    d.format("%-d %b %Y").to_string()
}

/// Fills `{name} {company} {date} {id}` (unknown placeholders stay as they are; an empty value becomes "").
pub fn fill_template(
    tpl: &str,
    name: Option<&str>,
    company: Option<&str>,
    date: &str,
    id: &str,
) -> String {
    let clean = |s: Option<&str>| {
        s.map(|v| v.trim().replace(['\n', '\r'], " "))
            .unwrap_or_default()
    };
    let out = tpl
        .replace("{name}", &clean(name))
        .replace("{company}", &clean(company))
        .replace("{date}", date)
        .replace("{id}", id);
    // tidy what an empty placeholder leaves ("Prepared for  · ..." -> "Prepared for · ...")
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The watermark settings: (customer text, staff text, font size).
pub async fn watermark_settings(db: &mut PgConnection) -> ApiResult<(String, String, f64)> {
    let keys = [
        "documents.watermark.customer_text",
        "documents.watermark.staff_text",
    ];
    let vals = values(db, &keys).await?;
    let customer = require_from(&vals, keys[0], WHERE)?;
    let staff = require_from(&vals, keys[1], WHERE)?;
    let size = require_int(db, "documents.watermark.font_size", WHERE).await?;
    if !(4..=24).contains(&size) {
        return Err(ApiError::http(
            400,
            format!(
                "Setting 'documents.watermark.font_size' must be between 4 and 24. Set it in Settings → {WHERE}."
            ),
        ));
    }
    Ok((customer, staff, size as f64))
}

/// The footer a copy will carry, before any id is drawn (confirm cards show it): `{id}` reads "ID-...".
pub async fn preview_footer(db: &mut PgConnection, kind: Kind, who: &Who) -> ApiResult<String> {
    let (customer_tpl, staff_tpl, _) = watermark_settings(db).await?;
    let date = footer_date(granary_core::clock::today_local(db).await?);
    let tpl = if kind.is_external() {
        customer_tpl
    } else {
        staff_tpl
    };
    let name = who.name.clone().or_else(|| who.recipient_name.clone());
    let company = who
        .company
        .clone()
        .or_else(|| who.recipient_company.clone())
        .filter(|c| !c.trim().is_empty())
        .or_else(|| name.clone());
    Ok(fill_template(
        &tpl,
        name.as_deref(),
        company.as_deref(),
        &date,
        "ID-...",
    ))
}

/// `issue(db, kind, version, who)`: writes the library_downloads row (hash filled by `record_sha`) and
/// returns the id and the footer text (customer text for external copies, staff text otherwise, date in
/// the company time zone).
pub async fn issue(
    db: &mut PgConnection,
    kind: Kind,
    version: &Version,
    who: &Who,
) -> ApiResult<Issued> {
    let (customer_tpl, staff_tpl, font_size) = watermark_settings(db).await?;
    let today = granary_core::clock::today_local(db).await?;
    let date = footer_date(today);
    let tpl = if kind.is_external() {
        customer_tpl
    } else {
        staff_tpl
    };
    let company = who
        .company
        .clone()
        .or_else(|| who.recipient_company.clone())
        .filter(|c| !c.trim().is_empty());
    let name = who
        .name
        .clone()
        .or_else(|| who.recipient_name.clone())
        .filter(|c| !c.trim().is_empty());
    // a customer copy with no company names the person instead
    let company_shown = company.clone().or_else(|| name.clone());
    for _ in 0..8 {
        let id = new_id();
        let row_id = Uuid::new_v4();
        let inserted: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO library_downloads (id, fingerprint, version_id, document_id, kind, user_id, \
             partner_account_id, recipient_name, recipient_company, channel, ip, user_agent, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now()) \
             ON CONFLICT (fingerprint) DO NOTHING RETURNING id",
        )
        .bind(row_id)
        .bind(&id)
        .bind(version.id)
        .bind(version.document_id)
        .bind(kind.as_str())
        .bind(who.user_id)
        .bind(who.partner_account_id)
        .bind(trunc(who.recipient_name.as_deref(), 255))
        .bind(trunc(who.recipient_company.as_deref(), 255))
        .bind(trunc(who.channel.as_deref(), 20))
        .bind(trunc(who.ip.as_deref(), 64))
        .bind(trunc(who.user_agent.as_deref(), 500))
        .fetch_optional(&mut *db)
        .await?;
        if inserted.is_some() {
            let footer_text =
                fill_template(&tpl, name.as_deref(), company_shown.as_deref(), &date, &id);
            return Ok(Issued {
                row_id,
                id,
                footer_text,
                font_size,
            });
        }
    }
    Err(ApiError::internal(anyhow::anyhow!(
        "could not draw a free fingerprint id"
    )))
}

fn trunc(s: Option<&str>, n: usize) -> Option<String> {
    s.map(|v| v.chars().take(n).collect())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Stores the hash of the stamped copy on the fingerprint row.
pub async fn record_sha(db: &mut PgConnection, row_id: Uuid, stamped: &[u8]) -> ApiResult<()> {
    sqlx::query("UPDATE library_downloads SET file_sha256 = $2 WHERE id = $1")
        .bind(row_id)
        .bind(sha256_hex(stamped))
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// Issue a fingerprint, stamp `bytes` with it and record the hash: the stamped copy and the issue.
pub async fn stamped_copy(
    db: &mut PgConnection,
    kind: Kind,
    version: &Version,
    who: &Who,
    bytes: &[u8],
    mime: &str,
) -> ApiResult<(Vec<u8>, Issued)> {
    let issued = issue(db, kind, version, who).await?;
    // 5 Nov 2026: a video is logged (its own row and fingerprint id) but never watermarked: the copy is the
    // original bytes, so its hash is the original's
    if crate::nary::uploads::is_video(mime) {
        record_sha(db, issued.row_id, bytes).await?;
        return Ok((bytes.to_vec(), issued));
    }
    let stamp = issued.stamp();
    let data = bytes.to_vec();
    let mime_s = mime.to_string();
    // MuPDF and image work is CPU bound: off the async threads
    let out = tokio::task::spawn_blocking(move || stamp::stamp_copy(&data, &mime_s, &stamp))
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!("stamp task: {e}")))?;
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            // nothing left the system: drop the row again
            sqlx::query("DELETE FROM library_downloads WHERE id = $1")
                .bind(issued.row_id)
                .execute(&mut *db)
                .await?;
            return Err(e);
        }
    };
    record_sha(db, issued.row_id, &out).await?;
    Ok((out, issued))
}

// ─── trace ────────────────────────────────────────────────────────────────────

/// How a copy was recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedBy {
    /// the id was typed in
    Id,
    /// a mark in the file (PDF Info / hidden text / image metadata / OOXML property)
    Fingerprint,
    /// the file is byte for byte a copy we stamped
    FileHash,
}

#[derive(Debug, Clone, Serialize)]
pub struct Named {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionRef {
    pub id: Uuid,
    pub label: String,
    pub status: String,
}

/// Who got the copy, and when.
#[derive(Debug, Clone, Serialize)]
pub struct TraceHit {
    pub fingerprint: String,
    pub matched_by: MatchedBy,
    /// staff_download | customer_send | partner_download | nary_send | source_download
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub document: Named,
    pub version: VersionRef,
    pub user: Option<Named>,
    pub partner_account: Option<Named>,
    pub recipient_name: Option<String>,
    pub recipient_company: Option<String>,
    pub channel: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    /// 5 Nov 2026: false for a video (logged, not watermarked)
    pub watermarked: bool,
    /// 5 Nov 2026: what the trace screen says about a video copy
    pub note: Option<String>,
}

/// 5 Nov 2026: the trace screen's line for a video copy.
pub const VIDEO_NOTE: &str = "Videos are logged, not watermarked: every copy has its own fingerprint id in the \
    log, but the file itself carries no mark, so a leaked video is matched by its content (every copy of it is the \
    same file) and the newest copy is shown.";

#[derive(sqlx::FromRow)]
struct HitRow {
    fingerprint: String,
    kind: String,
    created_at: DateTime<Utc>,
    document_id: Uuid,
    title: String,
    version_id: Uuid,
    label: String,
    status: String,
    user_id: Option<Uuid>,
    user_name: Option<String>,
    partner_account_id: Option<Uuid>,
    partner_name: Option<String>,
    recipient_name: Option<String>,
    recipient_company: Option<String>,
    channel: Option<String>,
    ip: Option<String>,
    user_agent: Option<String>,
    file_mime: String,
}

const HIT_SQL: &str = "SELECT l.fingerprint, l.kind, l.created_at, l.document_id, d.title, l.version_id, v.label, \
     v.status, l.user_id, u.name AS user_name, l.partner_account_id, p.company_name AS partner_name, \
     l.recipient_name, l.recipient_company, l.channel, l.ip, l.user_agent, v.file_mime \
     FROM library_downloads l \
     JOIN library_documents d ON d.id = l.document_id \
     JOIN library_document_versions v ON v.id = l.version_id \
     LEFT JOIN users u ON u.id = l.user_id \
     LEFT JOIN partner_accounts p ON p.id = l.partner_account_id";

fn hit(r: HitRow, matched_by: MatchedBy) -> TraceHit {
    let video = crate::nary::uploads::is_video(&r.file_mime);
    TraceHit {
        watermarked: !video,
        note: video.then(|| VIDEO_NOTE.to_string()),
        fingerprint: r.fingerprint,
        matched_by,
        kind: r.kind,
        created_at: r.created_at,
        document: Named {
            id: r.document_id,
            name: r.title,
        },
        version: VersionRef {
            id: r.version_id,
            label: r.label,
            status: r.status,
        },
        user: r.user_id.map(|id| Named {
            id,
            name: r.user_name.unwrap_or_default(),
        }),
        partner_account: r.partner_account_id.map(|id| Named {
            id,
            name: r.partner_name.unwrap_or_default(),
        }),
        recipient_name: r.recipient_name,
        recipient_company: r.recipient_company,
        channel: r.channel,
        ip: r.ip,
        user_agent: r.user_agent,
    }
}

async fn by_fingerprint(
    db: &mut PgConnection,
    fp: &str,
    matched_by: MatchedBy,
) -> ApiResult<Option<TraceHit>> {
    let row: Option<HitRow> = sqlx::query_as(&format!("{HIT_SQL} WHERE l.fingerprint = $1"))
        .bind(fp)
        .fetch_optional(&mut *db)
        .await?;
    Ok(row.map(|r| hit(r, matched_by)))
}

/// Normalises a typed id: case, Crockford look-alikes (O -> 0, I/L -> 1), an optional `ID-` prefix.
pub fn normalise_id(raw: &str) -> Option<String> {
    let s: String = raw
        .trim()
        .to_uppercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let body = match s.strip_prefix("ID-") {
        Some(b) => b,
        None if s.len() == 12 && s.starts_with("ID") => &s[2..],
        None => &s,
    };
    let body: String = body
        .chars()
        .filter(|c| *c != '-')
        .map(|c| match c {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    let id = format!("ID-{body}");
    let ok = stamp::FINGERPRINT_RE
        .find(&id)
        .is_some_and(|m| m.as_str() == id);
    ok.then_some(id)
}

/// The copy with this fingerprint id.
pub async fn trace_id(db: &mut PgConnection, raw: &str) -> ApiResult<Option<TraceHit>> {
    let Some(id) = normalise_id(raw) else {
        return Ok(None);
    };
    by_fingerprint(db, &id, MatchedBy::Id).await
}

/// The copy this file came from: its marks first, then its hash.
pub async fn trace_file(db: &mut PgConnection, bytes: &[u8]) -> ApiResult<Option<TraceHit>> {
    let data = bytes.to_vec();
    let marks = tokio::task::spawn_blocking(move || stamp::read_fingerprints(&data))
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!("trace task: {e}")))?;
    for fp in marks {
        if let Some(h) = by_fingerprint(db, &fp, MatchedBy::Fingerprint).await? {
            return Ok(Some(h));
        }
    }
    let sha = sha256_hex(bytes);
    let row: Option<HitRow> = sqlx::query_as(&format!(
        "{HIT_SQL} WHERE l.file_sha256 = $1 ORDER BY l.created_at DESC LIMIT 1"
    ))
    .bind(&sha)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|r| hit(r, MatchedBy::FileHash)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_crockford() {
        for _ in 0..50 {
            let id = new_id();
            assert_eq!(normalise_id(&id).as_deref(), Some(id.as_str()));
        }
        assert_eq!(
            normalise_id(" id-abcdefghjo ").as_deref(),
            Some("ID-ABCDEFGHJ0")
        );
        assert_eq!(normalise_id("ID-123"), None);
    }

    #[test]
    fn template_fill() {
        let t = "Prepared for {company} · {date} · Your Company · Confidential";
        assert_eq!(
            fill_template(t, None, Some("Acme Ltd"), "21 Oct 2026", "ID-X"),
            "Prepared for Acme Ltd · 21 Oct 2026 · Your Company · Confidential"
        );
        let s = "Downloaded by {name} · {date} · Your Company · Confidential · {id}";
        assert_eq!(
            fill_template(s, Some("Rachel"), None, "1 Jan 2027", "ID-0123456789"),
            "Downloaded by Rachel · 1 Jan 2027 · Your Company · Confidential · ID-0123456789"
        );
        assert_eq!(
            footer_date(chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()),
            "1 Oct 2026"
        );
    }
}
