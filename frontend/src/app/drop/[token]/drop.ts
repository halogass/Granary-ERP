/**
 * 5 Nov 2026: the drop page's logic (one-time upload links), kept apart from the page so it is tested alone.
 * The server judges every file by its bytes; these checks only spare a phone a long upload that would be refused.
 */

export interface DropStatus {
  target_label: string;
  state: "open" | "closed" | "expired" | "full";
  reason: string | null;
  expires_at: string;
  max_files: number;
  files_used: number;
  files: { name: string; size: number; size_label: string; type: string }[];
  allowed: string;
  accept: string;
  file_max_bytes: number;
  file_max_label: string;
  video_max_bytes: number;
  video_max_label: string;
}

export interface UploadResult {
  name: string;
  size: number;
  size_label: string;
  type: string;
  duration_label?: string | null;
  files_used: number;
  max_files: number;
}

export const ALLOWED_EXTS = ["png", "jpg", "jpeg", "webp", "gif", "pdf", "docx", "xlsx", "mp4", "mov", "webm"];
const VIDEO_EXTS = ["mp4", "mov", "webm"];

export function extOf(name: string): string {
  const i = name.lastIndexOf(".");
  return i < 0 ? "" : name.slice(i + 1).toLowerCase();
}

export function isVideo(name: string, type?: string): boolean {
  return (type ?? "").startsWith("video/") || VIDEO_EXTS.includes(extOf(name));
}

/** "1.2 MB", "340 KB", "900 B" (the server's human_size). */
export function humanSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Why this file would be refused before it is sent, or null. */
export function precheck(file: { name: string; size: number; type?: string }, s: Pick<DropStatus,
  "allowed" | "file_max_bytes" | "file_max_label" | "video_max_bytes" | "video_max_label">): string | null {
  if (file.size === 0) return "The file is empty.";
  const ext = extOf(file.name);
  if (ext && !ALLOWED_EXTS.includes(ext)) return `.${ext} files can't be uploaded: only ${s.allowed}.`;
  if (isVideo(file.name, file.type)) {
    if (file.size > s.video_max_bytes) return `The video is ${humanSize(file.size)}, over the ${s.video_max_label} limit.`;
  } else if (file.size > s.file_max_bytes) {
    return `The file is ${humanSize(file.size)}, over the ${s.file_max_label} limit.`;
  }
  return null;
}

/** How many more files the link takes. */
export function slotsLeft(s: Pick<DropStatus, "max_files" | "files_used">, uploadedHere: number): number {
  return Math.max(0, s.max_files - s.files_used - uploadedHere);
}

export function uploadUrl(token: string, name: string): string {
  return `/api/v1/public/drop/${encodeURIComponent(token)}/files?name=${encodeURIComponent(name)}`;
}

/** The server's reason from an error body ({detail}), else a plain line for the status. */
export function reasonOf(status: number, body: string): string {
  try {
    const d = JSON.parse(body)?.detail;
    if (typeof d === "string" && d) return d;
  } catch { /* not JSON */ }
  if (status === 413) return "The file is too large for this link.";
  if (status === 429) return "Too many uploads at once: wait a minute and try again.";
  if (status === 410) return "This upload link no longer takes files.";
  if (status === 404) return "This upload link does not exist.";
  if (status === 0) return "The connection was lost. Try again.";
  return "The upload failed. Try again.";
}

/** One PUT of the raw file with progress (XMLHttpRequest: fetch has no upload progress). */
export function putFile(token: string, file: File, onProgress: (fraction: number) => void,
  XHR: typeof XMLHttpRequest = XMLHttpRequest): Promise<UploadResult> {
  return new Promise((resolve, reject) => {
    const xhr = new XHR();
    xhr.open("PUT", uploadUrl(token, file.name));
    xhr.setRequestHeader("Content-Type", "application/octet-stream");
    xhr.upload.onprogress = (e: ProgressEvent) => {
      if (e.lengthComputable && e.total > 0) onProgress(e.loaded / e.total);
    };
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        try { resolve(JSON.parse(xhr.responseText)); } catch { reject(new Error("The server's answer was not readable.")); }
      } else {
        reject(new Error(reasonOf(xhr.status, xhr.responseText)));
      }
    };
    xhr.onerror = () => reject(new Error(reasonOf(0, "")));
    xhr.send(file);
  });
}
