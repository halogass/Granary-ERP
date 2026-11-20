import DOMPurify from "dompurify";

// Strips all script, iframe, and event-handler attributes from untrusted HTML.
// Safe to pass to dangerouslySetInnerHTML.
export function sanitizeHtml(html: string, forceBody = false): string {
  if (typeof window === "undefined") return "";
  return DOMPurify.sanitize(html, {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ["script", "iframe", "object", "embed", "form"],
    FORBID_ATTR: ["onerror", "onload", "onclick", "onmouseover", "onfocus"],
    // Keeps a <style> that opens the body (the parser would move it to <head> and drop it). Only for
    // HTML shown inside a sandboxed frame, where those styles cannot reach the ERP (lib/emailFrame.ts).
    FORCE_BODY: forceBody,
  });
}

/**
 * An email body, sanitized for display. Inline images that still point at "cid:" (mail synced
 * before those images were embedded, or ones too large to embed) can't be loaded by a browser:
 * they are shown as a short "[inline image]" note instead of a broken image and a failed request.
 * `forFrame`: the HTML goes into the sandboxed email frame, so the email's own <style> tags are kept.
 */
export function sanitizeEmailHtml(html: string, forFrame = false): string {
  return sanitizeHtml(html, forFrame).replace(
    /<img\b([^>]*?)\ssrc=(["'])cid:[^"']*\2([^>]*)>/gi,
    (_m, before: string, _q: string, after: string) => {
      const rest = `${before} ${after}`.replace(/\salt=(["'])[^"']*\1/i, "");
      return `<img${rest} alt="[inline image]" title="Inline image not available here">`;
    },
  );
}

const SAFE_URL_PROTOCOLS = new Set(["http:", "https:", "mailto:", "tel:"]);

// Returns a URL that is safe to put in href/src, or undefined. Allows http(s), mailto,
// tel and same-origin relative paths; a bare domain ("example.com/x") becomes https.
// Blocks javascript:, data:, vbscript: and protocol-relative "//host" URLs.
export function safeHref(url?: string | null): string | undefined {
  const trimmed = url?.trim();
  if (!trimmed) return undefined;
  if (trimmed.startsWith("/") && !trimmed.startsWith("//") && !trimmed.startsWith("/\\")) return trimmed;
  if (/^[a-z0-9-]+(\.[a-z0-9-]+)+(:\d+)?([/?#].*)?$/i.test(trimmed)) return `https://${trimmed}`;
  try {
    const parsed = new URL(trimmed);
    return SAFE_URL_PROTOCOLS.has(parsed.protocol) ? parsed.href : undefined;
  } catch {
    return undefined;
  }
}
