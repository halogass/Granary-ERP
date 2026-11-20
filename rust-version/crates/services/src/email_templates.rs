//! Port of backend/app/services/email_templates.py.
//!
//! Styled HTML email templates: visual identity matches the Granary UI.
//! Primary accent: #1f4e79 | Font stack: Arial, Helvetica, sans-serif.
//! All templates are table-based for maximum email-client compatibility.
//!
//! Byte-identical to Python (golden-tested: compat-tests/golden/gen_email_templates.py). Python's keyword
//! defaults are spelled out by the callers here; the `DEFAULT_*` constants carry them. Python quirks kept:
//! several titles and subjects are interpolated raw (not escaped) exactly where Python does so
//! (`_h1(title)`, `_h1(f"Quotation {quotation_number}")`, `_p(message)` in notifications); the
//! notification preheader is `message[:120]` in code points, escaped after slicing.

use granary_compat::pyfmt::html_escape;
use granary_compat::text::pystr;

/// Default `company_name` of the document templates.
pub const DEFAULT_COMPANY_NAME: &str = "Your Company";
/// Default `company_name` of the notification and password reset templates.
pub const DEFAULT_ERP_NAME: &str = "Granary";
/// Default `company_website`.
pub const DEFAULT_COMPANY_WEBSITE: &str = "https://example.com";
/// Default `from_name` of `test_email_template`.
pub const DEFAULT_FROM_NAME: &str = "Granary";
/// Default `expires_minutes` of `password_reset_email_template`.
pub const DEFAULT_EXPIRES_MINUTES: i64 = 30;

/// `_esc(s)`: `html.escape(s or "")` (quote=True).
fn _esc(s: &str) -> String {
    html_escape(s, true)
}

// ─── Base layout ─────────────────────────────────────────────────────────────

fn _base(
    subject: &str,
    preheader: &str,
    body_html: &str,
    company_name: &str,
    company_email: &str,
    company_website: &str,
) -> String {
    let mut footer_contact = String::new();
    if !company_email.is_empty() {
        footer_contact = format!(
            "<a href=\"mailto:{}\" style=\"color:#1f4e79;text-decoration:none\">{}</a>",
            _esc(company_email),
            _esc(company_email)
        );
    }
    let contact_part = if !footer_contact.is_empty() {
        format!("&nbsp;&middot;&nbsp;{footer_contact}")
    } else {
        String::new()
    };
    let website_part = if !company_website.is_empty() {
        format!(
            "&nbsp;&middot;&nbsp;<a href=\"{}\" style=\"color:#1f4e79;text-decoration:none\">{}</a>",
            _esc(company_website),
            _esc(company_website)
        )
    } else {
        String::new()
    };
    let subject_e = _esc(subject);
    let preheader_e = _esc(preheader);
    let company_e = _esc(company_name);

    let mut out = String::with_capacity(4096 + body_html.len());
    out.push_str(
        "<!DOCTYPE html>
<html lang=\"en\" xmlns=\"http://www.w3.org/1999/xhtml\">
<head>
  <meta charset=\"UTF-8\">
  <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">
  <meta http-equiv=\"X-UA-Compatible\" content=\"IE=edge\">
  <title>",
    );
    out.push_str(&subject_e);
    out.push_str(
        "</title>
  <!--[if mso]><style>table{border-collapse:collapse}</style><![endif]-->
</head>
<body style=\"margin:0;padding:0;background-color:#f3f4f6;-webkit-text-size-adjust:100%\">

<!-- Preheader (hidden preview text) -->
<div style=\"display:none;max-height:0;overflow:hidden;mso-hide:all\">
  ",
    );
    out.push_str(&preheader_e);
    out.push_str(
        "&nbsp;&#8204;&nbsp;&#8204;&nbsp;&#8204;&nbsp;&#8204;
</div>

<table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" role=\"presentation\"
       style=\"background-color:#f3f4f6;min-width:100%\">
  <tr>
    <td align=\"center\" style=\"padding:32px 16px\">

      <!-- Outer card -->
      <table width=\"600\" cellpadding=\"0\" cellspacing=\"0\" role=\"presentation\"
             style=\"max-width:600px;width:100%;border-radius:16px;overflow:hidden;
                    box-shadow:0 4px 24px rgba(0,0,0,0.08)\">

        <!-- ── Header bar ── -->
        <tr>
          <td style=\"background-color:#1f4e79;padding:28px 36px\">
            <table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" role=\"presentation\">
              <tr>
                <td>
                  <span style=\"font-family:Arial,Helvetica,sans-serif;font-size:20px;
                               font-weight:900;color:#ffffff;letter-spacing:0.5px;
                               text-decoration:none\">
                    ",
    );
    out.push_str(&company_e);
    out.push_str(
        "
                  </span>
                </td>
                <td align=\"right\">
                  <span style=\"font-family:Arial,Helvetica,sans-serif;font-size:11px;
                               color:rgba(255,255,255,0.7);text-transform:uppercase;
                               letter-spacing:1.5px;font-weight:700\">
                    GRANARY
                  </span>
                </td>
              </tr>
            </table>
          </td>
        </tr>

        <!-- ── Body ── -->
        <tr>
          <td style=\"background-color:#ffffff;padding:40px 36px 32px\">
            ",
    );
    out.push_str(body_html);
    out.push_str(
        "
          </td>
        </tr>

        <!-- ── Footer ── -->
        <tr>
          <td style=\"background-color:#f9fafb;padding:20px 36px;
                     border-top:1px solid #e5e7eb;border-radius:0 0 16px 16px\">
            <p style=\"margin:0;font-family:Arial,Helvetica,sans-serif;font-size:12px;
                      color:#9ca3af;line-height:1.7;text-align:center\">
              ",
    );
    out.push_str(&company_e);
    out.push_str("\n              ");
    out.push_str(&contact_part);
    out.push_str("\n              ");
    out.push_str(&website_part);
    out.push_str(
        "
              <br>
              This email was sent by Granary. Please do not reply to this address if
              you did not expect to receive it.
            </p>
          </td>
        </tr>

      </table>
    </td>
  </tr>
</table>
</body>
</html>",
    );
    out
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn _p(text: &str) -> String {
    format!(
        "<p style=\"margin:0 0 16px;font-family:Arial,Helvetica,sans-serif;\
font-size:15px;color:#374151;line-height:1.7\">{text}</p>"
    )
}

fn _h1(text: &str) -> String {
    format!(
        "<h1 style=\"margin:0 0 24px;font-family:Arial,Helvetica,sans-serif;\
font-size:24px;font-weight:900;color:#111827;letter-spacing:-0.3px\">{text}</h1>"
    )
}

fn _divider() -> String {
    "<hr style=\"margin:28px 0;border:none;border-top:1px solid #e5e7eb\">".to_string()
}

/// `_badge(label, color, bg)` (unused by the templates, kept for parity).
#[allow(dead_code)]
fn _badge(label: &str, color: &str, bg: &str) -> String {
    format!(
        "<span style=\"display:inline-block;padding:4px 12px;border-radius:20px;\
background-color:{bg};color:{color};font-family:Arial,Helvetica,sans-serif;\
font-size:12px;font-weight:700;text-transform:uppercase;letter-spacing:0.5px\">{}</span>",
        _esc(label)
    )
}

fn _highlight_box(content_html: &str) -> String {
    let border_color = "#1f4e79";
    format!(
        "<div style=\"background-color:#fef2f2;border-left:4px solid {border_color};\
border-radius:0 8px 8px 0;padding:16px 20px;margin:0 0 24px\">{content_html}</div>"
    )
}

fn _info_row(label: &str, value: &str) -> String {
    format!(
        "<tr>\
<td style=\"padding:10px 16px;font-family:Arial,Helvetica,sans-serif;\
font-size:13px;color:#6b7280;white-space:nowrap;border-bottom:1px solid #f3f4f6\">{}</td>\
<td style=\"padding:10px 16px;font-family:Arial,Helvetica,sans-serif;\
font-size:13px;color:#111827;font-weight:600;border-bottom:1px solid #f3f4f6\">{}</td>\
</tr>",
        _esc(label),
        _esc(value)
    )
}

fn _cta_button(label: &str, href: &str) -> String {
    format!(
        "<table cellpadding=\"0\" cellspacing=\"0\" role=\"presentation\" style=\"margin:24px 0\">\
<tr><td style=\"background-color:#1f4e79;border-radius:10px;padding:0\">\
<a href=\"{}\" style=\"display:inline-block;padding:14px 28px;\
font-family:Arial,Helvetica,sans-serif;font-size:15px;font-weight:700;\
color:#ffffff;text-decoration:none;border-radius:10px\">{}</a>\
</td></tr></table>",
        _esc(href),
        _esc(label)
    )
}

/// Point the reader at the QR verification page for the attached document.
fn _verify_block(verify_url: &str) -> String {
    if verify_url.is_empty() {
        return String::new();
    }
    _p("<span style=\"font-size:13px;color:#6b7280\">The attached PDF carries a verification QR code on \
every page. You can also confirm it is authentic and current here:</span>")
        + &_cta_button("Verify this document", verify_url)
}

fn _table(rows: &str, margin_bottom: &str) -> String {
    format!(
        "<table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" role=\"presentation\" \
style=\"border:1px solid #e5e7eb;border-radius:10px;overflow:hidden;\
margin-bottom:{margin_bottom}\"><tbody>{rows}</tbody></table>"
    )
}

fn _notes(notes: &str) -> String {
    if notes.is_empty() {
        return String::new();
    }
    format!(
        "<p style=\"margin:0 0 24px;font-family:Arial,Helvetica,sans-serif;\
font-size:15px;color:#374151;line-height:1.7\">{}</p>",
        _esc(notes)
    )
}

fn _regards(company_name: &str, company_email: &str) -> String {
    let mut s = format!(
        "<span style=\"color:#6b7280;font-size:13px\">Best regards,<br>\
<strong style=\"color:#111827\">{}</strong>",
        _esc(company_name)
    );
    if !company_email.is_empty() {
        s.push_str(&format!(
            "<br><a href=\"mailto:{}\" style=\"color:#1f4e79;text-decoration:none\">{}</a>",
            _esc(company_email),
            _esc(company_email)
        ));
    }
    s.push_str("</span>");
    _p(&s)
}

// ─── Templates ───────────────────────────────────────────────────────────────

/// `test_email_template(from_name="Granary")`.
pub fn test_email_template(from_name: &str) -> String {
    let body = _h1("SMTP Test Successful")
        + &_p(
            "This is a test email sent from <strong>Granary</strong> to confirm \
that your SMTP configuration is working correctly.",
        )
        + &_highlight_box(
            "<p style=\"margin:0;font-family:Arial,Helvetica,sans-serif;font-size:14px;\
color:#7f1d1d;font-weight:600\">\
\u{2713} &nbsp;Your system email is correctly configured and ready to send.\
</p>",
        )
        + &_p(
            "You can now send transactional emails including quotations, invoices, \
notifications, and password resets.",
        )
        + &_divider()
        + &_p(&format!(
            "<span style=\"color:#9ca3af;font-size:13px\">Sent by Granary &middot; \
System email account: <em>{}</em></span>",
            _esc(from_name)
        ));
    _base(
        "Granary: SMTP test",
        "Your SMTP configuration is working correctly.",
        &body,
        DEFAULT_COMPANY_NAME,
        "",
        DEFAULT_COMPANY_WEBSITE,
    )
}

/// `quotation_email_template(...)`.
#[allow(clippy::too_many_arguments)]
pub fn quotation_email_template(
    contact_name: &str,
    quotation_number: &str,
    company_name: &str,
    company_email: &str,
    company_website: &str,
    notes: &str,
    verify_url: &str,
) -> String {
    let rows =
        _info_row("Quotation No.", quotation_number) + &_info_row("Prepared for", contact_name);
    let table_html = _table(&rows, "28px");
    let body = _h1(&format!("Quotation {quotation_number}"))
        + &_p(&format!("Dear <strong>{}</strong>,", _esc(contact_name)))
        + &_p(&format!(
            "Thank you for your interest. Please find attached quotation \
<strong>{}</strong> from {}.",
            _esc(quotation_number),
            _esc(company_name)
        ))
        + &table_html
        + &_notes(notes)
        + &_p(
            "We would be happy to discuss the details or answer any questions at your \
convenience. Please reply to this email or contact us directly.",
        )
        + &_verify_block(verify_url)
        + &_divider()
        + &_regards(company_name, company_email);
    _base(
        &format!("Quotation {quotation_number} - {company_name}"),
        &format!("Your quotation {quotation_number} from {company_name} is attached."),
        &body,
        company_name,
        company_email,
        company_website,
    )
}

/// `invoice_email_template(...)`.
#[allow(clippy::too_many_arguments)]
pub fn invoice_email_template(
    contact_name: &str,
    invoice_number: &str,
    due_date: &str,
    company_name: &str,
    company_email: &str,
    company_website: &str,
    notes: &str,
    verify_url: &str,
) -> String {
    let rows = _info_row("Invoice No.", invoice_number)
        + &_info_row("Billed to", contact_name)
        + &_info_row("Payment due", due_date);
    let table_html = _table(&rows, "28px");
    let body = _h1(&format!("Invoice {invoice_number}"))
        + &_p(&format!("Dear <strong>{}</strong>,", _esc(contact_name)))
        + &_p(&format!(
            "Please find attached invoice <strong>{}</strong> from {}.",
            _esc(invoice_number),
            _esc(company_name)
        ))
        + &table_html
        + &_highlight_box(&format!(
            "<p style=\"margin:0;font-family:Arial,Helvetica,sans-serif;font-size:14px;\
color:#7f1d1d\"><strong>Payment is due by {}.</strong> \
Please ensure payment is made before the due date to avoid late charges.</p>",
            _esc(due_date)
        ))
        + &_notes(notes)
        + &_p(
            "For payment inquiries or any questions, please reply to this email or \
contact us directly.",
        )
        + &_verify_block(verify_url)
        + &_divider()
        + &_regards(company_name, company_email);
    _base(
        &format!("Invoice {invoice_number} - {company_name}"),
        &format!("Invoice {invoice_number} due {due_date} from {company_name}."),
        &body,
        company_name,
        company_email,
        company_website,
    )
}

/// `notification_email_template(...)`. `title` and `message` are HTML (not escaped), as in Python.
pub fn notification_email_template(
    recipient_name: &str,
    title: &str,
    message: &str,
    action_label: Option<&str>,
    action_url: Option<&str>,
    company_name: &str,
    company_email: &str,
) -> String {
    let cta = match (action_label, action_url) {
        (Some(l), Some(u)) if !l.is_empty() && !u.is_empty() => _cta_button(l, u),
        _ => String::new(),
    };
    let body = _h1(title)
        + &_p(&format!("Hi <strong>{}</strong>,", _esc(recipient_name)))
        + &_p(message)
        + &cta
        + &_divider()
        + &_p(&format!(
            "<span style=\"color:#9ca3af;font-size:13px\">\
This is an automated notification from {}.</span>",
            _esc(company_name)
        ));
    // `message[:120]`: code points.
    let preheader: String = message.chars().take(120).collect();
    _base(
        title,
        &preheader,
        &body,
        company_name,
        company_email,
        DEFAULT_COMPANY_WEBSITE,
    )
}

/// `portal_invite_email_template(...)`.
#[allow(clippy::too_many_arguments)]
pub fn portal_invite_email_template(
    recipient_name: &str,
    login_email: &str,
    temp_password: &str,
    portal_type: &str,
    login_url: &str,
    company_name: &str,
    company_email: &str,
) -> String {
    let portal_label = if portal_type == "vendor" {
        "Vendor / Contractor Portal"
    } else {
        "Partner Portal"
    };
    let rows = _info_row("Portal", portal_label)
        + &_info_row("Login Email", login_email)
        + &_info_row("Temporary Password", temp_password);
    let table_html = _table(&rows, "24px");
    let contact = if !company_email.is_empty() {
        format!(
            "<a href=\"mailto:{}\" style=\"color:#1f4e79;text-decoration:none\">{}</a>",
            _esc(company_email),
            _esc(company_email)
        )
    } else {
        _esc(company_name)
    };
    let body = _h1(&format!("You've been invited to the {portal_label}"))
        + &_p(&format!("Hi <strong>{}</strong>,", _esc(recipient_name)))
        + &_p(&format!(
            "{} has created a portal account for you on <strong>Granary</strong>. \
Use the credentials below to log in.",
            _esc(company_name)
        ))
        + &table_html
        + &_highlight_box(
            "<p style=\"margin:0;font-family:Arial,Helvetica,sans-serif;font-size:14px;\
color:#7f1d1d;font-weight:600\">\
\u{26a0}\u{fe0f} &nbsp;You must change your password immediately after logging in for the first time.\
</p>",
        )
        + &_cta_button("Log In Now", login_url)
        + &_divider()
        + &_p(&format!(
            "<span style=\"color:#9ca3af;font-size:13px\">\
If you did not expect this invitation, please contact {contact} immediately.</span>"
        ));
    _base(
        &format!("Your {portal_label} Access - {company_name}"),
        &format!("You've been invited to the {portal_label} by {company_name}."),
        &body,
        company_name,
        company_email,
        DEFAULT_COMPANY_WEBSITE,
    )
}

/// `password_reset_email_template(...)`.
pub fn password_reset_email_template(
    recipient_name: &str,
    reset_url: &str,
    expires_minutes: i64,
    company_name: &str,
    company_email: &str,
) -> String {
    let body = _h1("Reset Your Password")
        + &_p(&format!("Hi <strong>{}</strong>,", _esc(recipient_name)))
        + &_p(
            "We received a request to reset the password for your Granary account. \
Click the button below to set a new password.",
        )
        + &_cta_button("Reset Password", reset_url)
        + &_highlight_box(&format!(
            "<p style=\"margin:0;font-family:Arial,Helvetica,sans-serif;font-size:13px;\
color:#7f1d1d\">This link expires in <strong>{expires_minutes} minutes</strong>. \
If you did not request a password reset, you can safely ignore this email.</p>"
        ))
        + &_divider()
        + &_p(&format!(
            "<span style=\"color:#9ca3af;font-size:13px\">\
If the button doesn't work, copy and paste this URL:<br>\
<a href=\"{}\" style=\"color:#1f4e79;word-break:break-all\">{}</a></span>",
            _esc(reset_url),
            _esc(reset_url)
        ));
    _base(
        "Reset your Granary password",
        "You requested a password reset for your Granary account.",
        &body,
        company_name,
        company_email,
        DEFAULT_COMPANY_WEBSITE,
    )
}

/// `document_email_template(...)`: purchase orders, work orders and any other approved document sent
/// with its verified PDF.
#[allow(clippy::too_many_arguments)]
pub fn document_email_template(
    contact_name: &str,
    document_label: &str,
    document_number: &str,
    company_name: &str,
    company_email: &str,
    company_website: &str,
    notes: &str,
    verify_url: &str,
) -> String {
    let rows = _info_row(&format!("{document_label} No."), document_number)
        + &_info_row("Addressed to", contact_name);
    let table_html = _table(&rows, "28px");
    let body = _h1(&format!(
        "{} {}",
        _esc(document_label),
        _esc(document_number)
    )) + &_p(&format!("Dear <strong>{}</strong>,", _esc(contact_name)))
        + &_p(&format!(
            "Please find attached {} <strong>{}</strong> from {}.",
            _esc(&pystr::lower(document_label)),
            _esc(document_number),
            _esc(company_name)
        ))
        + &table_html
        + &_notes(notes)
        + &_p("If anything is unclear, please reply to this email or contact us directly.")
        + &_verify_block(verify_url)
        + &_divider()
        + &_regards(company_name, company_email);
    _base(
        &format!("{document_label} {document_number} - {company_name}"),
        &format!("{document_label} {document_number} from {company_name} is attached."),
        &body,
        company_name,
        company_email,
        company_website,
    )
}
