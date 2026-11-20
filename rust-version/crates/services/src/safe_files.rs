//! security 26 Oct (finding 1): the type a stored file is served to a browser with.
//!
//! A file's type is chosen by whoever sent it (an email, a WhatsApp message, an upload). Served as is, an
//! HTML or SVG file opens as a page on the ERP's domain and its script runs there. So only types that
//! cannot run as a page keep their type, and only some of those may open inline; everything else downloads
//! as `application/octet-stream`. Routes add `nosniff` and [`CSP`] on top (api `files::safe_file_response`).

/// The policy on every stored file served: nothing loads, nothing runs.
pub const CSP: &str = "default-src 'none'; sandbox";
pub const OCTET: &str = "application/octet-stream";

/// Types a browser shows without running anything: kept, and may open inline.
const INLINE: &[&str] = &[
    "application/pdf",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "text/plain",
    "audio/mpeg",
    "audio/mp3",
    "audio/mp4",
    "audio/aac",
    "audio/ogg",
    "audio/opus",
    "audio/wav",
    "audio/x-wav",
    "audio/webm",
    "audio/amr",
    "audio/x-m4a",
    "video/mp4",
    "video/webm",
    "video/ogg",
    "video/quicktime",
    "video/3gpp",
];

/// Types kept for the right app to open, always as a download.
const DOWNLOAD: &[&str] = &[
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.ms-excel",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.ms-powerpoint",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.oasis.opendocument.text",
    "application/vnd.oasis.opendocument.spreadsheet",
    "application/vnd.oasis.opendocument.presentation",
    "application/rtf",
    "application/zip",
    "application/x-zip-compressed",
    "application/x-7z-compressed",
    "application/x-rar-compressed",
    "application/vnd.rar",
    "application/gzip",
    "application/json",
    "text/csv",
    "text/markdown",
    "text/calendar",
];

/// How one file is served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// The Content-Type to send (no parameters).
    pub content_type: String,
    /// Whether it opens inline (else `attachment`).
    pub inline: bool,
}

/// `image/PNG; name=x` -> `image/png`.
pub fn base_type(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// The type `stored_mime` is served with; `want_inline` is honoured only for [`INLINE`] types.
pub fn served(stored_mime: &str, want_inline: bool) -> Served {
    let t = base_type(stored_mime);
    if INLINE.contains(&t.as_str()) {
        Served {
            content_type: t,
            inline: want_inline,
        }
    } else if DOWNLOAD.contains(&t.as_str()) {
        Served {
            content_type: t,
            inline: false,
        }
    } else {
        Served {
            content_type: OCTET.into(),
            inline: false,
        }
    }
}

/// Whether `stored_mime` may open inline at all.
pub fn is_inline_type(stored_mime: &str) -> bool {
    INLINE.contains(&base_type(stored_mime).as_str())
}

/// `Content-Disposition` with an ASCII fallback and the exact UTF-8 name (RFC 6266 / RFC 5987).
pub fn content_disposition(inline: bool, filename: &str) -> String {
    let fallback: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " -_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let fallback = fallback.trim();
    let fallback = if fallback.is_empty() {
        "download"
    } else {
        fallback
    };
    let mut quoted = String::new();
    for b in filename.as_bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(b) {
            quoted.push(*b as char);
        } else {
            quoted.push_str(&format!("%{b:02X}"));
        }
    }
    let kind = if inline { "inline" } else { "attachment" };
    format!("{kind}; filename=\"{fallback}\"; filename*=UTF-8''{quoted}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scriptable_types_download_as_octet_stream() {
        for t in [
            "text/html",
            "image/svg+xml",
            "application/xhtml+xml",
            "text/xml",
            "application/xml",
            "application/javascript",
            "text/javascript",
            "",
            "TEXT/HTML; charset=utf-8",
        ] {
            assert_eq!(
                served(t, true),
                Served {
                    content_type: OCTET.into(),
                    inline: false
                },
                "{t}"
            );
        }
    }

    #[test]
    fn safe_types_keep_their_type() {
        assert_eq!(
            served("Image/PNG", true),
            Served {
                content_type: "image/png".into(),
                inline: true
            }
        );
        assert_eq!(
            served("audio/ogg; codecs=opus", true).content_type,
            "audio/ogg"
        );
        assert!(!served("application/pdf", false).inline);
        let docx = served(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            true,
        );
        assert!(!docx.inline);
        assert!(docx.content_type.ends_with("wordprocessingml.document"));
    }

    #[test]
    fn disposition_has_ascii_fallback_and_utf8_name() {
        assert_eq!(
            content_disposition(false, "laporan \"akhir\".pdf"),
            "attachment; filename=\"laporan _akhir_.pdf\"; filename*=UTF-8''laporan%20%22akhir%22.pdf"
        );
        assert_eq!(
            content_disposition(true, "ñ"),
            "inline; filename=\"_\"; filename*=UTF-8''%C3%B1"
        );
    }
}
