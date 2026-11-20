import { sanitizeEmailHtml } from "./sanitize";

/**
 * Received email bodies, made safe to show (security triage 18, 27 Oct 2026).
 *
 * The body is sanitized (DOMPurify, see sanitize.ts) and then rendered as a whole document in a sandboxed
 * iframe (see components/email/EmailBody.tsx), so its <style> tags only style the email, never the ERP.
 *
 * Remote images are read tracking: until the user asks for them, every remote URL an email could load is
 * removed here (img src/srcset, <source>, poster, background="", style="" and <style> url(), @import),
 * and the frame's own Content-Security-Policy refuses any that slip through. Inline images (data:) still show;
 * "cid:" images were already resolved to data: by the server or turned into a note by sanitizeEmailHtml.
 */

/** URLs an email may load without asking: embedded (data:) and inline-part (cid:) images. */
const LOCAL_URL = /^\s*(data:|cid:)/i;

/** Attributes that hold one URL the browser fetches on its own. */
const URL_ATTRS = ["src", "background", "poster", "lowsrc", "dynsrc"];

/** Base look of the email paper: the email's own styles come after and win. */
const BASE_STYLE = [
  "html{color-scheme:light}",
  "body{margin:0;padding:12px 16px;background:#fff;color:#1f2328;",
  "font:14px/1.5 'IBM Plex Sans',system-ui,-apple-system,'Segoe UI',sans-serif;overflow-wrap:anywhere}",
  "img{max-width:100%;height:auto}",
  "img[data-granary-blocked]{display:inline-block;min-width:16px;min-height:16px;background:#f1f3f5}",
  "a{color:#0b5cad}",
  "pre{white-space:pre-wrap}",
  "blockquote{margin:0 0 0 8px;padding-left:8px;border-left:2px solid #d0d7de;color:#57606a}",
].join("");

export interface PreparedEmail {
  /** A full HTML document for the iframe's srcdoc. */
  doc: string;
  /** How many remote images (or remote stylesheet loads) were held back. */
  blocked: number;
}

/** CSS with every non-local url() replaced by `none` and @import rules dropped; the count is added to `n`. */
function blockCss(css: string, n: { count: number }): string {
  return css
    .replace(/@import\s+[^;]*;?/gi, () => {
      n.count += 1;
      return "";
    })
    .replace(/url\(\s*(['"]?)(.*?)\1\s*\)/gi, (whole, _q: string, url: string) => {
      if (LOCAL_URL.test(url)) return whole;
      n.count += 1;
      return "none";
    })
    .replace(/image-set\([^)]*\)/gi, (whole) => {
      if (/^image-set\(\s*(['"])?\s*(data:|cid:)/i.test(whole)) return whole;
      n.count += 1;
      return "none";
    });
}

/** A srcset with only its local candidates (empty when none). */
function localSrcset(srcset: string): string {
  return srcset
    .split(",")
    .map((c) => c.trim())
    .filter((c) => c && LOCAL_URL.test(c))
    .join(", ");
}

function csp(showImages: boolean): string {
  const remote = showImages ? " https: http:" : "";
  return [
    "default-src 'none'",
    "script-src 'none'",
    "style-src 'unsafe-inline'",
    `img-src data:${remote}`,
    `font-src data:${remote}`,
    `media-src data:${remote}`,
    "form-action 'none'",
    "base-uri 'none'",
  ].join("; ");
}

/**
 * The email body as a sandbox document. With `showImages` false, remote images are removed and counted;
 * with it true they are kept (the count still says how many there are, so the caller knows the bar applies).
 */
export function prepareEmailHtml(html: string, showImages: boolean): PreparedEmail {
  const clean = sanitizeEmailHtml(html || "", true);
  const n = { count: 0 };
  let body = "";
  if (typeof window !== "undefined" && clean) {
    const parsed = new DOMParser().parseFromString(`<!doctype html><body>${clean}</body>`, "text/html");
    const tally = { count: 0 };
    parsed.body.querySelectorAll("*").forEach((el) => {
      for (const attr of URL_ATTRS) {
        const v = el.getAttribute(attr);
        if (v == null || LOCAL_URL.test(v) || !v.trim()) continue;
        tally.count += 1;
        if (!showImages) {
          el.removeAttribute(attr);
          el.setAttribute("data-granary-blocked", "1");
        }
      }
      const srcset = el.getAttribute("srcset");
      if (srcset != null) {
        const local = localSrcset(srcset);
        if (local !== srcset.trim()) {
          tally.count += 1;
          if (!showImages) {
            if (local) el.setAttribute("srcset", local);
            else el.removeAttribute("srcset");
            el.setAttribute("data-granary-blocked", "1");
          }
        }
      }
      const style = el.getAttribute("style");
      if (style) {
        const s = { count: 0 };
        const blocked = blockCss(style, s);
        tally.count += s.count;
        if (!showImages && s.count) el.setAttribute("style", blocked);
      }
      if (el.tagName === "STYLE") {
        const s = { count: 0 };
        const blocked = blockCss(el.textContent || "", s);
        tally.count += s.count;
        if (!showImages && s.count) el.textContent = blocked;
      }
      if (el.tagName === "A" && el.hasAttribute("href")) {
        el.setAttribute("target", "_blank");
        el.setAttribute("rel", "noopener noreferrer");
      }
    });
    n.count = tally.count;
    body = parsed.body.innerHTML;
  }
  const doc =
    "<!doctype html><html><head><meta charset=\"utf-8\">" +
    `<meta http-equiv="Content-Security-Policy" content="${csp(showImages)}">` +
    '<meta name="referrer" content="no-referrer">' +
    '<base target="_blank">' +
    `<style>${BASE_STYLE}</style></head><body>${body}</body></html>`;
  return { doc, blocked: n.count };
}

/** A sender address as the server keeps it. */
export function normalizeSender(email: string | null | undefined): string {
  return (email || "").trim().toLowerCase();
}
