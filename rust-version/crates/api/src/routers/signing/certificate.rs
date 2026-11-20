//! routers/signing.py `_build_certificate_html` and its constants (`LEGAL_BASIS`, `_SIGNATURE_LEVEL_LABEL`,
//! `_SEAL_LEVEL_LABEL`, `_legal_basis_for`). The template text is the Python f-string verbatim (generated
//! from the source; `@@N@@` marks the N-th interpolation).

use chrono::{DateTime, Utc};
use granary_compat::pyfmt::{html_escape, strftime};
use granary_compat::pyjson::PyDateTime;
use granary_db::rows::signing::{SignAuditLog, SignDocument, SignSigner};

const EM: &str = "-";

/// `LEGAL_BASIS` (dict order).
pub const LEGAL_BASIS: [(&str, &str); 2] = [
    (
        "US",
        "United States: ESIGN Act (15 U.S.C. ch. 96) and the Uniform Electronic Transactions Act (UETA).",
    ),
    (
        "EU",
        "European Union: eIDAS Regulation (EU) No 910/2014 (electronic signatures admissible; tamper-evident advanced seal applied).",
    ),
];

/// `_SIGNATURE_LEVEL_LABEL.get(level)`.
pub fn signature_level_label(level: &str) -> Option<&'static str> {
    match level {
        "ses" => Some("Simple Electronic Signature (SES)"),
        "aes" => Some("Advanced Electronic Signature (AES)"),
        "qes" => Some("Qualified Electronic Signature (QES)"),
        _ => None,
    }
}

/// `_SEAL_LEVEL_LABEL.get(level)`.
pub fn seal_level_label(level: &str) -> Option<&'static str> {
    match level {
        "pades-b-b" => Some("PAdES-B-B (baseline)"),
        "pades-b-t" => Some("PAdES-B-T (trusted timestamp)"),
        "pades-b-lt" => Some("PAdES-B-LT (long-term)"),
        "pades-b-lta" => Some("PAdES-B-LTA (long-term archival)"),
        "none" => Some("Not sealed"),
        _ => None,
    }
}

/// `_legal_basis_for(doc)`.
pub fn legal_basis_for(doc: &SignDocument) -> Vec<&'static str> {
    let j = doc
        .jurisdictions
        .as_deref()
        .filter(|j| !j.is_empty())
        .unwrap_or("US,EU,ID");
    j.split(',')
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .filter_map(|c| LEGAL_BASIS.iter().find(|(k, _)| *k == c).map(|(_, v)| *v))
        .collect()
}

/// `e(v)`: `html.escape(str(v), quote=True)`.
fn e(v: &str) -> String {
    html_escape(v, true)
}

fn utc_fmt(dt: &DateTime<Utc>, fmt: &str) -> String {
    strftime(&PyDateTime::from_chrono_utc(dt), fmt)
}

/// `str.title()`: the first cased character of each run of cased characters upper-cased, the rest lower.
pub fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        let cased = c.is_lowercase() || c.is_uppercase();
        if cased {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
        } else {
            out.push(c);
        }
        prev_cased = cased;
    }
    out
}

/// `_is_png_data_url(value)`.
pub fn is_png_data_url(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.starts_with("data:image/png;base64,"))
}

fn fill(template: &str, values: &[String]) -> String {
    let mut out =
        String::with_capacity(template.len() + values.iter().map(|v| v.len()).sum::<usize>());
    let mut rest = template;
    while let Some(start) = rest.find("@@") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("@@") {
            if let Ok(n) = after[..end].parse::<usize>() {
                out.push_str(&rest[..start]);
                out.push_str(&values[n]);
                rest = &after[end + 2..];
                continue;
            }
        }
        out.push_str(&rest[..start + 2]);
        rest = after;
    }
    out.push_str(rest);
    out
}

/// `_build_certificate_html(doc, verification_url)` with the parts that vary made explicit: `now` is
/// `datetime.now(timezone.utc)`, `qr_b64` is `_generate_qr_base64(verification_url)` ("" on failure),
/// `signers` / `audit_logs` are `doc.signers` / `doc.audit_logs`, `seal_name` is `settings.SIGN_SEAL_NAME`.
pub fn build_certificate_html(
    doc: &SignDocument,
    signers: &[SignSigner],
    audit_logs: &[SignAuditLog],
    now: DateTime<Utc>,
    qr_b64: &str,
    seal_name: &str,
) -> String {
    let now_str = utc_fmt(&now, "%d %B %Y, %H:%M UTC");
    let mut audit_rows = String::new();
    for l in audit_logs {
        audit_rows.push_str(&format!(
            "<tr>\n            <td style=\"padding:8px 12px;border-bottom:1px solid #f0f0f0;font-size:12px;color:#555\">{}</td>\n            <td style=\"padding:8px 12px;border-bottom:1px solid #f0f0f0;font-size:12px;font-weight:600;color:#111\">{}</td>\n            <td style=\"padding:8px 12px;border-bottom:1px solid #f0f0f0;font-size:12px;color:#555\">{}</td>\n            <td style=\"padding:8px 12px;border-bottom:1px solid #f0f0f0;font-size:12px;color:#888;font-family:monospace\">{}</td>\n        </tr>",
            utc_fmt(&l.created_at, "%d %b %Y %H:%M UTC"),
            e(&py_title(&l.action.replace('_', " "))),
            e(l.description.as_deref().unwrap_or("")),
            e(l.ip_address.as_deref().filter(|s| !s.is_empty()).unwrap_or(EM)),
        ));
    }
    let mut signer_blocks = String::new();
    for s in signers {
        let sig_html = if is_png_data_url(s.signature_data.as_deref()) {
            format!(
                "<img src=\"{}\" style=\"max-height:60px;border:1px solid #e0e0e0;border-radius:4px;padding:4px;background:#fff\">",
                e(s.signature_data.as_deref().unwrap_or(""))
            )
        } else {
            "<span style=\"color:#aaa;font-size:12px;font-style:italic\">Not yet signed</span>"
                .to_string()
        };
        let status_color = match s.status.as_deref() {
            Some("signed") => "#16a34a",
            Some("declined") => "#dc2626",
            _ => "#6b7280",
        };
        let signed = match &s.signed_at {
            Some(t) => format!("Signed {}", utc_fmt(t, "%d %b %Y %H:%M UTC")),
            None => String::new(),
        };
        let consent = match &s.consent_agreed_at {
            Some(t) => format!(
                "Agreed ({}) {}",
                s.consent_version.as_deref().unwrap_or(""),
                utc_fmt(t, "%d %b %Y %H:%M UTC")
            ),
            None => EM.to_string(),
        };
        signer_blocks.push_str(&format!(
            "\n        <div style=\"display:inline-block;vertical-align:top;width:48%;margin-bottom:20px;padding:16px;background:#fafafa;border:1px solid #e5e5e5;border-radius:8px;box-sizing:border-box\">\n            <p style=\"margin:0 0 4px;font-size:14px;font-weight:700;color:#111\">{}</p>\n            <p style=\"margin:0 0 4px;font-size:12px;color:#555\">{}</p>\n            <p style=\"margin:0 0 8px;font-size:11px;font-weight:600;color:{}\">{} {}</p>\n            <p style=\"margin:0 0 4px;font-size:11px;color:#888\">IP: {}</p>\n            <p style=\"margin:0 0 4px;font-size:11px;color:#888\">E-records consent: {}</p>\n            <div style=\"margin-top:8px\">{}</div>\n        </div>",
            e(&s.name),
            e(&s.email),
            status_color,
            e(&s.status.as_deref().unwrap_or("").to_uppercase()),
            signed,
            e(s.ip_address.as_deref().filter(|v| !v.is_empty()).unwrap_or(EM)),
            e(&consent),
            sig_html,
        ));
    }
    let qr_section = if qr_b64.is_empty() {
        String::new()
    } else {
        format!("<img src=\"data:image/png;base64,{qr_b64}\" style=\"width:100px;height:100px\">")
    };
    let seal_level = doc.seal_level.as_deref().unwrap_or("");
    let seal = if doc.sealed_at.is_some() {
        let lvl = if seal_level.is_empty() {
            "pades-b-b"
        } else {
            seal_level
        };
        format!("{} by {seal_name}", seal_level_label(lvl).unwrap_or("None"))
    } else {
        "Not sealed".to_string()
    };
    let level = doc
        .signature_level
        .as_deref()
        .filter(|l| !l.is_empty())
        .unwrap_or("ses");
    let values = vec![
        now_str,
        e(&doc.title),
        e(&doc.id.to_string()),
        e(doc.document_hash.as_deref().filter(|h| !h.is_empty()).unwrap_or("N/A")),
        e(&doc.status.as_deref().unwrap_or("").to_uppercase()),
        match &doc.completed_at {
            Some(t) => utc_fmt(t, "%d %B %Y, %H:%M UTC"),
            None => EM.to_string(),
        },
        signature_level_label(level).map(e).unwrap_or_default(),
        e(&seal),
        if seal_level.ends_with("-t") || seal_level.ends_with("-lta") {
            " \u{b7} trusted timestamp".to_string()
        } else {
            String::new()
        },
        match doc.seal_fingerprint.as_deref().filter(|f| !f.is_empty()) {
            Some(fp) => format!(
                "<tr><td style=\"padding:4px 0;font-size:12px;color:#666\">Seal Cert SHA-256</td><td style=\"font-size:11px;font-family:monospace;color:#333;word-break:break-all\">{}</td></tr>",
                e(fp)
            ),
            None => String::new(),
        },
        e(seal_name),
        signer_blocks,
        audit_rows,
        legal_basis_for(doc)
            .iter()
            .map(|b| format!("<br>&bull; {}", e(b)))
            .collect::<String>(),
        e(&doc.id.to_string()),
        qr_section,
    ];
    fill(CERT_TEMPLATE, &values)
}

const CERT_TEMPLATE: &str = concat!(
    r#"<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><style>
  body { font-family: 'Helvetica Neue', Arial, sans-serif; margin: 0; padding: 0; background: white; color: #111; }
  .page { max-width: 900px; margin: 0 auto; padding: 40px; }
</style></head>
<body>
<div class="page">

  <!-- Header -->
  <div style="border-bottom:3px solid #1f4e79;padding-bottom:20px;margin-bottom:32px;display:flex;justify-content:space-between;align-items:flex-start">
    <div>
      <div style="font-size:24px;font-weight:900;color:#1f4e79;letter-spacing:4px;margin-bottom:4px">@@10@@</div>
    </div>
    <div style="text-align:right">
      <div style="font-size:18px;font-weight:800;color:#111">SIGNING CERTIFICATE</div>
      <div style="font-size:11px;color:#888;margin-top:4px">Electronic Signature Evidence Document</div>
      <div style="font-size:11px;color:#888">Generated: @@0@@</div>
    </div>
  </div>

  <!-- Document Info -->
  <div style="background:#fafafa;border:1px solid #e5e5e5;border-radius:8px;padding:20px;margin-bottom:24px">
    <h2 style="margin:0 0 16px;font-size:15px;font-weight:700;color:#111;text-transform:uppercase;letter-spacing:1px">Document Information</h2>
    <table style="width:100%;border-collapse:collapse">
      <tr><td style="padding:4px 0;font-size:12px;color:#666;width:160px">Document Title</td><td style="font-size:13px;font-weight:600;color:#111">@@1@@</td></tr>
      <tr><td style="padding:4px 0;font-size:12px;color:#666">Document ID</td><td style="font-size:12px;font-family:monospace;color:#333">@@2@@</td></tr>
      <tr><td style="padding:4px 0;font-size:12px;color:#666">SHA-256 Hash</td><td style="font-size:11px;font-family:monospace;color:#333;word-break:break-all">@@3@@</td></tr>
      <tr><td style="padding:4px 0;font-size:12px;color:#666">Status</td><td style="font-size:13px;font-weight:700;color:#16a34a">@@4@@</td></tr>
      <tr><td style="padding:4px 0;font-size:12px;color:#666">Completed At</td><td style="font-size:12px;color:#333">@@5@@</td></tr>
      <tr><td style="padding:4px 0;font-size:12px;color:#666">Signature Level</td><td style="font-size:13px;font-weight:600;color:#111">@@6@@</td></tr>
      <tr><td style="padding:4px 0;font-size:12px;color:#666">Digital Seal</td><td style="font-size:12px;color:#333">@@7@@@@8@@</td></tr>
      @@9@@
    </table>
  </div>

  <!-- Integrity note -->
  <div style="background:#f0fdf4;border:1px solid #16a34a;border-radius:6px;padding:12px 16px;margin-bottom:24px;font-size:11px;color:#166534">
    <strong>Document Integrity:</strong> The completed PDF carries a cryptographic <strong>PAdES</strong> certification
    seal by @@10@@. Any modification after sealing invalidates the signature;"#,
    r#" open the signed PDF
    in Adobe Acrobat (or any PAdES-aware reader) to verify the seal and signing certificate. The SHA-256 hash above
    is of the original uploaded document.
  </div>

  <!-- Signers -->
  <h2 style="font-size:15px;font-weight:700;color:#111;text-transform:uppercase;letter-spacing:1px;margin-bottom:16px">Signatories</h2>
  <div style="margin-bottom:24px">@@11@@</div>

  <!-- Audit Timeline -->
  <h2 style="font-size:15px;font-weight:700;color:#111;text-transform:uppercase;letter-spacing:1px;margin-bottom:12px">Audit Trail</h2>
  <table style="width:100%;border-collapse:collapse;border:1px solid #e5e5e5;border-radius:8px;overflow:hidden;margin-bottom:32px">
    <thead>
      <tr style="background:#f5f5f5">
        <th style="padding:10px 12px;text-align:left;font-size:11px;font-weight:700;color:#666;text-transform:uppercase;letter-spacing:1px">Timestamp (UTC)</th>
        <th style="padding:10px 12px;text-align:left;font-size:11px;font-weight:700;color:#666;text-transform:uppercase;letter-spacing:1px">Event</th>
        <th style="padding:10px 12px;text-align:left;font-size:11px;font-weight:700;color:#666;text-transform:uppercase;letter-spacing:1px">Description</th>
        <th style="padding:10px 12px;text-align:left;font-size:11px;font-weight:700;color:#666;text-transform:uppercase;letter-spacing:1px">IP Address</th>
      </tr>
    </thead>
    <tbody>@@12@@</tbody>
  </table>

  <!-- QR + Legal -->
  <div style="display:flex;gap:32px;align-items:flex-start;border-top:2px solid #e5e5e5;padding-top:24px">
    <div style="flex:1">
      <p style="font-size:11px;color:#666;line-height:1.7;margin:0">
        <strong style="color:#111">Legal Basis:</strong> This certificate constitutes evidence of electronic
        signing recognised under: @@13@@<br>
        Each signature is linked to its signatory via the unique signing link sent to their email and the audit
        trail above (timestamp, IP), recorded consent to electronic records, and is bound to the document by a
        tamper-evident PAdES seal;"#,
    r#" any later modification is detectable.
      </p>
      <p style="font-size:11px;color:#666;margin-top:12px">
        <strong style="color:#111">Verify this certificate:</strong> Scan the QR code or visit the verification URL.
        Certificate generated by Granary. Document ID: <code style="font-family:monospace">@@14@@</code>
      </p>
    </div>
    <div style="text-align:center;shrink:0">
      @@15@@
      <p style="font-size:10px;color:#999;margin:4px 0 0">Scan to verify</p>
    </div>
  </div>

</div>
</body>
</html>"#
);
