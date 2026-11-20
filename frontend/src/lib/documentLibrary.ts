/**
 * The document library (Knowledge Base > Documents): the company's own files with versions, approvals, a
 * watermark on every copy that leaves the system and a fingerprint per download. Types, API calls
 * and the small pure helpers the screens share. Contract: rust-version/docs/DOCUMENT_LIBRARY_BRIEF.md.
 */
import api from "@/lib/api";
import { saveFile } from "@/lib/safeFile";

/** The knowledge router's prefix (the library lives under it). */
export const KB_PREFIX = "/knowledge";
const DOCS = `${KB_PREFIX}/documents`;

export type Audience = "internal" | "customer" | "partner";
export const AUDIENCES: { value: Audience; label: string }[] = [
  { value: "internal", label: "Internal" },
  { value: "customer", label: "Customer-facing" },
  { value: "partner", label: "Partner" },
];

export type VersionStatus = "draft" | "pending_approval" | "live" | "superseded" | "rejected";

export interface DocCategory {
  id: string;
  key: string;
  name: string;
  sort_order?: number;
  /** Role slugs that may see the category; empty means every internal user. */
  visible_roles?: string[];
  /** False when the signed-in user's roles do not see it. */
  can_view?: boolean;
}

export interface PersonRef { id: string; name: string }

/** A linked catalog product, or a whole product family. */
export interface DocProductRef { id?: string | null; sku?: string | null; name?: string | null; family?: string | null }

export interface CurrentVersion {
  id: string;
  label: string;
  change_note: string;
  went_live_at?: string | null;
  file_name: string;
  file_size?: number | null;
  page_count?: number | null;
  file_mime?: string | null;
  /** A video's length in seconds (5 Nov 2026). */
  duration_seconds?: number | null;
}

export interface LibraryDocument {
  id: string;
  title: string;
  category: { key: string; name: string } | null;
  audiences: Audience[];
  owner: PersonRef | null;
  language: string;
  language_variants?: { id: string; language: string; title: string }[];
  products?: DocProductRef[];
  review_by?: string | null;
  needs_review?: boolean;
  current_version: CurrentVersion | null;
  pending_version: { id: string; label: string } | null;
  can_edit?: boolean;
  updated_at?: string;
}

export interface DocVersion {
  id: string;
  label: string;
  change_note: string;
  status: VersionStatus | string;
  file_name?: string;
  file_mime?: string;
  file_size?: number | null;
  page_count?: number | null;
  /** A video's length in seconds (5 Nov 2026). */
  duration_seconds?: number | null;
  source_file_name?: string | null;
  uploaded_by?: PersonRef | string | null;
  approved_by?: PersonRef | string | null;
  approved_at?: string | null;
  rejected_reason?: string | null;
  went_live_at?: string | null;
  created_at?: string;
}

export interface DocPermissions {
  edit: boolean; approve: boolean; history: boolean; reassign: boolean; trace: boolean;
  upload?: boolean; restore?: boolean; delete?: boolean; download_source?: boolean;
}

export interface DocumentDetail extends LibraryDocument {
  description?: string | null;
  articles?: { id: string; title: string }[];
  linked_articles?: { id: string; title: string }[];
  /** Only doc admins and directors get the history; null for everyone else. */
  versions: DocVersion[] | null;
  permissions: DocPermissions;
  current_version: (CurrentVersion & { file_mime?: string; source_file_name?: string | null; has_source?: boolean }) | null;
}

/** Who a leaked copy was given to. */
export interface TraceHit {
  fingerprint: string;
  /** id (typed in), fingerprint (a mark in the file) or file_hash (a byte copy of a stamped file). */
  matched_by?: string;
  kind: string;
  created_at?: string;
  document?: PersonRef | null;
  version?: { id: string; label: string; status: string } | null;
  user?: PersonRef | null;
  partner_account?: PersonRef | null;
  recipient_name?: string | null;
  recipient_company?: string | null;
  channel?: string | null;
  /** 5 Nov 2026: false for a video (logged, not watermarked). */
  watermarked?: boolean;
  /** 5 Nov 2026: what the screen says about a video copy. */
  note?: string | null;
}

const MATCHED_BY: Record<string, string> = {
  id: "The ID you typed", fingerprint: "The hidden mark in the file", file_hash: "An exact copy of a stamped file",
};
export const matchedByLabel = (m?: string | null) => (m ? MATCHED_BY[m] ?? m : "-");

/** Who received the traced copy, in one line. */
export function traceRecipient(h: TraceHit): string {
  const outside = [h.recipient_name, h.recipient_company].filter(Boolean).join(", ");
  if (outside) return outside;
  if (h.partner_account?.name) return `${h.partner_account.name} (partner)`;
  return h.user?.name ?? "-";
}

export interface DocListParams {
  view?: string;
  category?: string;
  audience?: string;
  language?: string;
  product_id?: string;
  q?: string;
  limit?: number;
  offset?: number;
  /** a version id: the list holds only the document it belongs to (approval links) */
  version?: string;
}

/** Document fields a create or edit sends. */
export interface DocMeta {
  title: string;
  description?: string;
  category: string;
  audiences: Audience[];
  language: string;
  /** Another document whose language group this one joins (a translation of it). */
  language_group_document_id?: string | null;
  products: DocProductRef[];
  review_by?: string | null;
  owner_id?: string;
}

type Rows<T> = T[] | { items?: T[] };
export function itemsOf<T>(data: Rows<T> | null | undefined): T[] {
  if (Array.isArray(data)) return data;
  return data?.items ?? [];
}

function multipart(fields: Record<string, string | Blob | undefined | null>) {
  const fd = new FormData();
  for (const [k, v] of Object.entries(fields)) if (v != null) fd.append(k, v);
  return fd;
}

const LONG = { timeout: 300000 };

/** Products as the API takes them: a catalog product id or a family name. */
type ProductPayload = { product_id: string } | { family: string };
export function productsPayload(products: DocProductRef[]): ProductPayload[] {
  return products.flatMap((p): ProductPayload[] => (p.id ? [{ product_id: p.id }] : p.family ? [{ family: p.family }] : []));
}

function metaPayload<T extends Partial<DocMeta>>(m: T) {
  return m.products ? { ...m, products: productsPayload(m.products) } : m;
}

export const docLibApi = {
  list: (params: DocListParams) =>
    api.get<{ items: LibraryDocument[]; total: number }>(DOCS, { params }),
  get: (id: string) => api.get<DocumentDetail>(`${DOCS}/${id}`),
  /** A document and its first version (label, change note, file, optional source). */
  create: (meta: DocMeta & { label: string; change_note: string }, file: File, source?: File | null) =>
    api.post<DocumentDetail>(DOCS, multipart({ meta: JSON.stringify(metaPayload(meta)), file, source: source ?? undefined }), LONG),
  update: (id: string, data: Partial<DocMeta>) => api.patch<DocumentDetail>(`${DOCS}/${id}`, metaPayload(data)),
  remove: (id: string) => api.delete(`${DOCS}/${id}`),
  suggestLabel: (id: string) => api.get<{ label: string } | string>(`${DOCS}/${id}/suggest-label`),
  uploadVersion: (id: string, v: { label: string; change_note: string; file: File; source?: File | null }) =>
    api.post<DocVersion>(`${DOCS}/${id}/versions`,
      multipart({ label: v.label, change_note: v.change_note, file: v.file, source: v.source ?? undefined }), LONG),
  /** The stamped copy for the browser viewer (staff footer + fingerprint). */
  view: (id: string, version?: string) =>
    api.get<Blob>(`${DOCS}/${id}/view`, { params: version ? { version } : {}, responseType: "blob", ...LONG }),
  download: (id: string, opts: { version?: string; source?: boolean } = {}) =>
    api.get<Blob>(`${DOCS}/${id}/download`, {
      params: { ...(opts.version ? { version: opts.version } : {}), ...(opts.source ? { source: 1 } : {}) },
      responseType: "blob", ...LONG,
    }),
  restore: (id: string, versionId: string) => api.post<DocVersion>(`${DOCS}/${id}/versions/${versionId}/restore`),
  traceFile: (file: File) => api.post<TraceHit | null>(`${DOCS}/trace`, multipart({ file }), LONG),
  traceId: (fingerprint: string) => api.post<TraceHit | null>(`${DOCS}/trace`, { fingerprint }),
  categories: () => api.get<Rows<DocCategory>>(`${KB_PREFIX}/document-categories`),
  saveCategories: (rows: { id: string; key: string; visible_roles: string[] }[]) =>
    api.put(`${KB_PREFIX}/document-categories`, rows),
  generatePriceList: () => api.post<DocumentDetail>(`${DOCS}/price-list/generate`, undefined, LONG),
  articleDocuments: (articleId: string) => api.get<Rows<LibraryDocument>>(`${KB_PREFIX}/articles/${articleId}/documents`),
  setArticleDocuments: (articleId: string, documentIds: string[]) =>
    api.put(`${KB_PREFIX}/articles/${articleId}/documents`, { document_ids: documentIds }),
  catalogDocuments: (productId: string) => api.get<Rows<LibraryDocument>>(`/sales/catalog/${productId}/documents`),
};

/** The partner portal's view: partner documents, current live version only. */
export const partnerDocsApi = {
  list: () => api.get<Rows<LibraryDocument>>("/partner/documents"),
  download: (id: string) => api.get<Blob>(`/partner/documents/${id}/download`, { responseType: "blob", ...LONG }),
};

// ---------------------------------------------------------------------------------------------
// Pure helpers

/** The saved views of the list page and the `view` the API takes for each. */
export const DOC_VIEWS = [
  { id: "all", label: "All" },
  { id: "mine", label: "Mine" },
  { id: "customer", label: "Customer-facing" },
  { id: "partner", label: "Partner" },
  { id: "internal", label: "Internal" },
  { id: "needs_review", label: "Needs review" },
  { id: "pending", label: "Waiting for approval" },
] as const;

export interface DocFilters { view?: string; category?: string; audience?: string; language?: string; product_id?: string }

/** The API params for a view plus filters and a search; empty values are left out. */
export function listParams(f: DocFilters, q = "", page = 1, pageSize = 50): DocListParams {
  const out: DocListParams = { view: f.view || "all", limit: pageSize, offset: (Math.max(1, page) - 1) * pageSize };
  if (f.category) out.category = f.category;
  if (f.audience) out.audience = f.audience;
  if (f.language) out.language = f.language;
  if (f.product_id) out.product_id = f.product_id;
  if (q.trim()) out.q = q.trim();
  return out;
}

/** A customer or partner audience means a new version waits for an executive's approval. */
export function needsApproval(audiences: readonly string[]): boolean {
  return audiences.some((a) => a === "customer" || a === "partner");
}

/** Same rule as the server's suggest_label: the last number + 1, keeping the rest. */
export function nextLabel(prev?: string | null): string {
  const p = (prev ?? "").trim();
  if (!p) return "v1";
  const m = /^(.*?)(\d+)(\D*)$/.exec(p);
  if (!m) return `${p} 2`;
  const [, head, num, tail] = m;
  const next = String(Number(num) + 1).padStart(num.length, "0");
  return `${head}${next}${tail}`;
}

const SOURCE_EXT = ["docx", "xlsx", "pptx"];
const IMAGE_EXT = ["png", "jpg", "jpeg", "webp"];
const VIDEO_EXT = ["mp4", "m4v", "mov", "webm"];
/** The video types a version may be (the server checks the content too). */
export const VIDEO_MIMES = ["video/mp4", "video/quicktime", "video/webm"];

function ext(name: string) {
  const i = name.lastIndexOf(".");
  return i < 0 ? "" : name.slice(i + 1).toLowerCase();
}

export type FileKind = "pdf" | "image" | "video" | "office" | "other";

export function fileKind(name?: string | null, mime?: string | null): FileKind {
  const m = (mime ?? "").toLowerCase();
  const e = ext(name ?? "");
  if (m === "application/pdf" || e === "pdf") return "pdf";
  if (m.startsWith("image/") || IMAGE_EXT.includes(e)) return "image";
  if (m.startsWith("video/") || VIDEO_EXT.includes(e)) return "video";
  if (SOURCE_EXT.includes(e) || m.includes("officedocument")) return "office";
  return "other";
}

/** Why an upload cannot go (null when it can): file type, the PDF a source needs, the size limit. */
export function uploadProblem(file: { name: string; type?: string; size: number } | null, source: { name: string; type?: string; size: number } | null, maxMb: number, videoMaxMb?: number): string | null {
  if (!file) return "Choose the file to publish (PDF, image or video).";
  const k = fileKind(file.name, file.type);
  if (k === "office") return "Upload the PDF here and the Word, Excel or PowerPoint file as the source.";
  if (k !== "pdf" && k !== "image" && k !== "video") return "The file must be a PDF, an image (PNG, JPG or WEBP) or a video (MP4, MOV or WebM).";
  if (source) {
    if (fileKind(source.name, source.type) !== "office") return "The source must be a DOCX, XLSX or PPTX file.";
    if (k !== "pdf") return "A source file needs its PDF: publish the PDF and attach the source.";
  }
  // 5 Nov 2026: videos have their own cap (nary.upload.video_max_mb); the server checks it again
  if (k === "video" && videoMaxMb != null) {
    if (file.size > videoMaxMb * 1024 * 1024) return `The video is larger than ${videoMaxMb} MB.`;
    return null;
  }
  const limit = maxMb * 1024 * 1024;
  if (k !== "video" && file.size > limit) return `The file is larger than ${maxMb} MB.`;
  if (source && source.size > limit) return `The source file is larger than ${maxMb} MB.`;
  return null;
}

/** A video's length: "0:42", "1:02:03" (5 Nov 2026). */
export function formatDuration(seconds?: number | null): string | null {
  if (seconds == null || !Number.isFinite(seconds)) return null;
  const total = Math.max(0, Math.round(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

export function formatFileSize(bytes?: number | null): string {
  if (bytes == null || !Number.isFinite(bytes)) return "-";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const LANGUAGES: Record<string, string> = {
  en: "English", ms: "Malay", zh: "Chinese", ja: "Japanese", ko: "Korean", ar: "Arabic",
  th: "Thai", vi: "Vietnamese", nl: "Dutch", de: "German", fr: "French", es: "Spanish",
};
export const LANGUAGE_OPTIONS = Object.entries(LANGUAGES).map(([value, label]) => ({ value, label }));

export function languageLabel(code?: string | null): string {
  if (!code) return "-";
  return LANGUAGES[code.toLowerCase()] ?? code.toUpperCase();
}

export function versionStatusLabel(status?: string | null): string {
  switch (status) {
    case "pending_approval": return "Waiting for approval";
    case "live": return "Live";
    case "superseded": return "Superseded";
    case "rejected": return "Rejected";
    case "draft": return "Draft";
    default: return status ?? "-";
  }
}

export function audienceLabel(a: string): string {
  return AUDIENCES.find((x) => x.value === a)?.label ?? a;
}

/** "Laptop Pro 14 (LAP-P14-01)" or "Family: Laptop". */
export function productLabel(p: DocProductRef): string {
  if (p.family && !p.id) return `Family: ${p.family}`;
  const name = p.name || p.sku || "Product";
  return p.sku && p.name ? `${p.name} (${p.sku})` : name;
}

export function personName(p?: PersonRef | string | null): string {
  if (!p) return "-";
  return typeof p === "string" ? p : p.name || "-";
}

/** The file name a Content-Disposition header gives, else the fallback. */
export function filenameFromDisposition(header: string | null | undefined, fallback: string): string {
  if (!header) return fallback;
  const star = /filename\*\s*=\s*(?:UTF-8'')?([^;]+)/i.exec(header);
  if (star) {
    try { return decodeURIComponent(star[1].trim().replace(/^"|"$/g, "")); } catch { /* fall through */ }
  }
  const plain = /filename\s*=\s*"?([^";]+)"?/i.exec(header);
  return plain ? plain[1].trim() : fallback;
}

/** Hand a downloaded blob to the browser as a file. */
export function saveBlob(blob: Blob, filename: string) {
  saveFile(blob, filename);
}

/** The label the server suggests, or the local rule when the call fails. */
export async function fetchSuggestedLabel(id: string, prev?: string | null): Promise<string> {
  try {
    const r = await docLibApi.suggestLabel(id);
    const d = r.data;
    const label = typeof d === "string" ? d : d?.label;
    return label || nextLabel(prev);
  } catch {
    return nextLabel(prev);
  }
}

export const DOC_MAX_MB_DEFAULT = 100;

/** documents.max_file_mb when this user may read settings, else the shipped default (the server checks anyway). */
export async function fetchMaxFileMb(): Promise<number> {
  try {
    const r = await api.get("/system/settings");
    const rows = itemsOf<{ key: string; value: string | null }>(r.data);
    const v = Number(rows.find((s) => s.key === "documents.max_file_mb")?.value);
    return Number.isFinite(v) && v > 0 ? v : DOC_MAX_MB_DEFAULT;
  } catch {
    return DOC_MAX_MB_DEFAULT;
  }
}

/** 5 Nov 2026: the video cap in MB (nary.upload.video_max_mb); null when it can't be read (the server checks). */
export async function fetchVideoMaxMb(): Promise<number | null> {
  try {
    const r = await api.get("/system/settings");
    const rows = itemsOf<{ key: string; value: string | null }>(r.data);
    const v = Number(rows.find((s) => s.key === "nary.upload.video_max_mb")?.value);
    return Number.isFinite(v) && v > 0 ? v : null;
  } catch {
    return null;
  }
}

/** The honest limit of the watermark, shown wherever a copy leaves or is traced. */
export const WATERMARK_LIMIT =
  "The visible footer survives printing and photos; the hidden id survives forwarding, not retyping or screenshots.";

/** Download a stamped copy (logged with its own fingerprint) and save it under the server's file name. */
export async function downloadStamped(
  fetch: () => Promise<{ data: Blob; headers: Record<string, unknown> | { get?: (k: string) => unknown } }>,
  fallbackName: string,
) {
  const res = await fetch();
  const h = res.headers as Record<string, unknown>;
  const disposition = typeof h["content-disposition"] === "string" ? (h["content-disposition"] as string) : null;
  saveBlob(res.data, filenameFromDisposition(disposition, fallbackName));
}
