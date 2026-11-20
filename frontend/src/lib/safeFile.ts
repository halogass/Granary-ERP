/**
 * Received files (email, WhatsApp, CRM attachments, uploads) are untrusted. A blob opened in a
 * tab runs as a page on the ERP's own origin, so an HTML or SVG file with the sender's type is
 * script with the user's session. Every place that opens, shows or saves a fetched file goes
 * through here:
 *  - the blob is rebuilt with a type from a fixed allowlist, anything else becomes
 *    application/octet-stream (the browser can then only save it);
 *  - only PDF, plain raster images, plain text, audio and common video open in a tab;
 *    everything else (SVG, HTML, XML, office files...) is downloaded.
 */

const IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
// 1 Nov 2026: MOV too (videos your AI client or Nary upload; a QuickTime H.264 file plays in the browser)
const VIDEO_TYPES = new Set(["video/mp4", "video/webm", "video/ogg", "video/quicktime"]);
const OTHER_TYPES = new Set(["application/pdf", "text/plain"]);
const AUDIO_RE = /^audio\/[a-z0-9.+-]+$/;

export const DOWNLOAD_TYPE = "application/octet-stream";

/** The bare media type ("Image/SVG+XML; charset=x" -> "image/svg+xml"), or "". */
function bareType(type: string | null | undefined): string {
  return (type || "").split(";")[0].trim().toLowerCase();
}

/** The type a received file may carry: an allowlisted type, else application/octet-stream. */
export function safeType(type: string | null | undefined): string {
  const bare = bareType(type);
  const t = bare === "image/jpg" ? "image/jpeg" : bare;
  if (IMAGE_TYPES.has(t) || VIDEO_TYPES.has(t) || OTHER_TYPES.has(t)) return t;
  if (AUDIO_RE.test(t) && !t.includes("xml") && !t.includes("html")) return t;
  return DOWNLOAD_TYPE;
}

/** True for the raster image types that may be shown in an <img> (never SVG). */
export function isSafeImageType(type: string | null | undefined): boolean {
  return IMAGE_TYPES.has(safeType(type));
}

/** True for the video types that may play inline. */
export function isSafeVideoType(type: string | null | undefined): boolean {
  return VIDEO_TYPES.has(safeType(type));
}

/** True when a file of this type may open in a browser tab. */
export function canOpenInTab(type: string | null | undefined): boolean {
  return safeType(type) !== DOWNLOAD_TYPE;
}

/**
 * Rebuild a fetched file with a safe type. `declared` is the type the record carries (attachment
 * mime type); without it the blob's own (server) type is used. Either way only the allowlist survives.
 */
export function safeBlob(data: Blob | BlobPart, declared?: string | null): Blob {
  const own = data instanceof Blob ? data.type : "";
  return new Blob([data], { type: safeType(declared || own) });
}

/** An object URL for a safe copy of the file. The caller revokes it. */
export function safeObjectUrl(data: Blob | BlobPart, declared?: string | null): string {
  return URL.createObjectURL(safeBlob(data, declared));
}

/** Save the file to disk (never rendered). */
export function saveFile(data: Blob | BlobPart, filename: string, declared?: string | null): void {
  const url = safeObjectUrl(data, declared);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename || "download";
  a.rel = "noopener";
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
}

/**
 * Open the file in a new tab when its type is safe to render (PDF, PNG/JPEG/GIF/WebP, plain text,
 * audio, video); download it otherwise. Returns what was done.
 */
export function openOrSaveFile(
  data: Blob | BlobPart,
  filename: string,
  declared?: string | null,
): "opened" | "saved" {
  const blob = safeBlob(data, declared);
  if (blob.type === DOWNLOAD_TYPE) {
    saveFile(blob, filename);
    return "saved";
  }
  const url = URL.createObjectURL(blob);
  window.open(url, "_blank", "noopener");
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
  return "opened";
}
