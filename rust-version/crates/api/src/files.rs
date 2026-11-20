//! security 26 Oct (finding 1): one way to send a stored file's bytes to the browser.

use axum::body::Body;
use axum::response::Response;

use granary_services::safe_files;

use crate::resp;

/// A stored file as a response: an allowlisted Content-Type (anything else is an `application/octet-stream`
/// download), `inline` only when asked and the type is safe to show, `nosniff`, a no-script sandbox policy
/// and an RFC 6266 filename. Add Cache-Control / Content-Length with [`resp::with_headers`].
pub fn safe_file_response(
    data: impl Into<Body>,
    stored_mime: &str,
    filename: &str,
    want_inline: bool,
) -> Response {
    let served = safe_files::served(stored_mime, want_inline);
    let name = if filename.is_empty() {
        "download"
    } else {
        filename
    };
    resp::with_headers(
        resp::bytes_response(200, data, &served.content_type),
        &[
            (
                "content-disposition",
                safe_files::content_disposition(served.inline, name),
            ),
            ("x-content-type-options", "nosniff".to_string()),
            ("content-security-policy", safe_files::CSP.to_string()),
        ],
    )
}
