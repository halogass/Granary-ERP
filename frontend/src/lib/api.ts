import axios, { type AxiosResponse, type InternalAxiosRequestConfig } from "axios";
import { getSession } from "next-auth/react";
import type { Session } from "next-auth";
import type { LineIn, PricingPreview } from "@/lib/types/catalog";
import type { TaxNotes } from "@/lib/taxNotes";

const api = axios.create({
  baseURL: "/api/v1",
  headers: { "Content-Type": "application/json" },
});

// getSession() is a round-trip to /api/auth/session, which re-runs the NextAuth JWT
// callback: including its refresh of the backend token. Calling it once per API
// request means a page that loads a dozen widgets hits it a dozen times at once, and
// at the hour mark every one of those tries to refresh. Share the in-flight call and
// reuse the answer briefly; the access token is valid for an hour, so a few seconds
// of reuse costs nothing.
const SESSION_TTL_MS = 5_000;
let sessionPromise: Promise<Session | null> | null = null;
let sessionFetchedAt = 0;

function currentSession(forceFresh = false): Promise<Session | null> {
  if (forceFresh || !sessionPromise || Date.now() - sessionFetchedAt > SESSION_TTL_MS) {
    sessionFetchedAt = Date.now();
    sessionPromise = getSession().catch((err) => {
      sessionPromise = null;   // don't cache a failure
      throw err;
    });
  }
  return sessionPromise;
}

/** The signed-in user's API token (for requests made outside axios, e.g. the face engine download). */
export async function accessToken(): Promise<string | null> {
  const session = await currentSession();
  return (session as { access_token?: string } | null)?.access_token ?? null;
}

api.interceptors.request.use(async (config) => {
  const session = await currentSession();
  if (session?.access_token) {
    config.headers.Authorization = `Bearer ${session.access_token}`;
  }
  // This instance defaults to a JSON Content-Type, and axios 1.x reacts to that by
  // SERIALISING a FormData body into JSON, the file becomes `{}` and the upload
  // reaches the API as a JSON object, which FastAPI rejects with 422. Clearing the
  // header lets the browser set multipart/form-data with its boundary.
  if (typeof FormData !== "undefined" && config.data instanceof FormData) {
    config.headers.delete("Content-Type");
  }
  return config;
});

// Queue of requests that fired while a token refresh was in-flight
let isRefreshing = false;
let pendingQueue: Array<{
  resolve: (token: string) => void;
  reject: (err: unknown) => void;
}> = [];

function drainQueue(err: unknown, token: string | null) {
  pendingQueue.forEach((p) => (err ? p.reject(err) : p.resolve(token!)));
  pendingQueue = [];
}

api.interceptors.response.use(
  (res) => res,
  async (error) => {
    const original = error.config as InternalAxiosRequestConfig & { _retry?: boolean };

    if (error.response?.status !== 401 || original._retry) {
      return Promise.reject(error);
    }

    // If a refresh is already in-flight, queue this request until it settles
    if (isRefreshing) {
      return new Promise<string>((resolve, reject) => {
        pendingQueue.push({ resolve, reject });
      }).then((token) => {
        original.headers.Authorization = `Bearer ${token}`;
        return api(original);
      });
    }

    original._retry = true;
    isRefreshing = true;

    try {
      // Bypass the cache here: the token we just used was rejected, so we need the
      // JWT callback to run again and (if needed) refresh against the backend.
      const session = await currentSession(true);

      if (session?.access_token && !session.error) {
        const token = session.access_token;
        original.headers.Authorization = `Bearer ${token}`;
        drainQueue(null, token);
        return api(original);
      }
    } catch {
      // getSession itself failed: fall through to redirect
    } finally {
      isRefreshing = false;
    }

    drainQueue(new Error("session_expired"), null);
    window.location.href = "/login";
    return Promise.reject(error);
  }
);

export default api;

// ─── Typed API helpers ────────────────────────────────────────────────────────

export const authApi = {
  login: (email: string, password: string) =>
    api.post("/auth/login", { email, password }),
  logout: (refresh_token: string) =>
    api.post("/auth/logout", { refresh_token }),
  me: () => api.get("/auth/me"),
  updateMe: (data: { phone?: string; wa_signature_name?: string; wa_signature_custom?: string }) =>
    api.patch("/auth/me", data),
  changePassword: (data: { current_password: string; new_password: string }) =>
    api.post("/auth/change-password", data),
  uploadAvatar: (formData: FormData) =>
    api.post("/auth/me/avatar", formData),
  getSignature: () => api.get("/auth/me/signature"),
  saveSignature: (signature_data: string) => api.put("/auth/me/signature", { signature_data }),
  deleteSignature: () => api.delete("/auth/me/signature"),
  // Two-factor sign-in (TOTP, 2 Nov 2026). Codes are sent once and never stored here.
  totpStatus: () => api.get<TotpStatus>("/auth/totp"),
  /** Stage 8: the current password is asked again before a new secret is made. */
  totpSetup: (password: string) => api.post<TotpSetup>("/auth/totp/setup", { password }),
  /** Stage 8: a single-use ticket for opening a WebSocket (the access token never goes in the URL). */
  wsTicket: () => api.post<{ ticket: string; expires_in: number }>("/auth/ws-ticket", {}),
  totpConfirm: (code: string) => api.post<{ enabled: boolean; recovery_codes: string[] }>("/auth/totp/confirm", { code }),
  /** A current code or a recovery code. */
  totpDisable: (code: string) => api.post<{ enabled: boolean }>("/auth/totp/disable", { code }),
  totpRegenerate: (code: string) => api.post<{ recovery_codes: string[] }>("/auth/totp/recovery-codes", { code }),
};

/** The signed-in person's two-factor sign-in. `required`: their role must have it. */
export interface TotpStatus {
  enabled: boolean; pending: boolean; confirmed_at: string | null; recovery_codes_left: number; required?: boolean;
}

/** A setup in progress: the secret as a QR (PNG data URI) and as text. */
export interface TotpSetup { secret: string; otpauth_uri: string; issuer: string; account: string; qr_data_uri: string }

/** A shared CRM inbox as Settings manages it. */
export interface CrmMailbox {
  id: string; prefix: string; address: string; display_name: string; purpose: "general" | "sales";
  signature_html: string | null; roles: string[] | null; active: boolean; is_default: boolean; sort: number;
  connected_at: string | null; backfill_days: number; backfill_done: boolean;
}
/** A shared CRM inbox as any internal user sees it (inbox filter, From selects). */
export interface InboxMailbox { id: string; address: string; name: string; purpose: string; active: boolean; default: boolean }
/** An activity someone deleted, kept so the Owner or an admin can restore it. */
export interface DeletedActivity {
  id: string; activity_id: string; type: string | null; subject: string | null;
  entity_type: string | null; entity_id: string | null; reason: string | null;
  voided_by: string | null; voided_at: string; restored_at: string | null;
}
export interface ReplyDefaults { from_inbox: string; to: string[]; cc: string[]; subject: string }

export const crmInboxApi = {
  mailboxes: () => api.get<{ mailboxes: InboxMailbox[] }>("/crm/inbox/mailboxes"),
  replyDefaults: (threadId: string) => api.get<ReplyDefaults>(`/crm/inbox/threads/${threadId}/reply-defaults`),
  listThreads: (params?: Record<string, unknown>) => api.get("/crm/inbox/threads", { params }),
  getThread: (id: string) => api.get(`/crm/inbox/threads/${id}`),
  updateThread: (id: string, data: unknown) => api.patch(`/crm/inbox/threads/${id}`, data),
  markUnread: (id: string) => api.post(`/crm/inbox/threads/${id}/unread`),
  deleteThread: (id: string) => api.delete(`/crm/inbox/threads/${id}`),
  /** One action on many conversations: the ticked `ids`, or everything matching `filter`. */
  bulk: (data: {
    action: "delete" | "read" | "unread" | "status" | "assign";
    ids?: string[];
    filter?: { status?: string; search?: string; box?: string; unread?: boolean; assigned?: string };
    status?: string;
    assignee_id?: string | null;
  }) => api.post("/crm/inbox/threads/bulk", data, { timeout: 120000 }),
  deleteMessage: (id: string) => api.delete(`/crm/inbox/messages/${id}`),
  reply: (threadId: string, formData: FormData) =>
    api.post(`/crm/inbox/threads/${threadId}/reply`, formData, {
      headers: { "Content-Type": "multipart/form-data" },
    }),
  sync: () => api.post("/crm/inbox/sync"),
  syncOlder: () => api.post("/crm/inbox/sync-older"),
  getUnreadCount: () => api.get("/crm/inbox/unread-count"),
  downloadAttachment: (id: string) => api.get(`/crm/inbox/attachments/${id}`, { responseType: "blob" }),
  /** Link a WhatsApp chat to a contact: its real number, and/or the chat id when WhatsApp hides the number. */
  waLink: (data: { phone?: string | null; chat_id?: string | null; contact_id: string }) => api.post("/crm/inbox/wa-link", data),
  waUnlink: (phone: string) => api.delete(`/crm/inbox/wa-link/${encodeURIComponent(phone)}`),
};

export interface CrmReminder {
  id: string; title: string; note: string | null; source: string;
  entity_type: string; entity_id: string | null; chat_id: string | null; link: string;
  due_at: string; status: string; snooze_count: number;
  owner_user_id: string; owner_name?: string | null; completed_at: string | null; created_at: string | null;
}
export interface ReminderDefaults {
  default_delay_days: number; default_due_at: string;
  presets: { key: string; label: string; due_at: string }[];
}

/** CRM follow-up reminders: manual "remind me", plus the ones the rules raise. */
export const remindersApi = {
  list: (params?: { scope?: "mine" | "team"; status?: string; limit?: number }) =>
    api.get<CrmReminder[]>("/crm/reminders", { params }),
  count: () => api.get<{ overdue: number; today: number }>("/crm/reminders/count"),
  defaults: () => api.get<ReminderDefaults>("/crm/reminders/defaults"),
  forRecord: (params: { entity_type: string; entity_id?: string; chat_id?: string }) =>
    api.get<CrmReminder[]>("/crm/reminders/for-record", { params }),
  create: (data: {
    entity_type: string; entity_id?: string; chat_id?: string; title: string; note?: string; due_at?: string;
  }) => api.post<CrmReminder>("/crm/reminders", data),
  snooze: (id: string, data: { days?: number; business_days?: number; until?: string }) =>
    api.post<CrmReminder>(`/crm/reminders/${id}/snooze`, data),
  done: (id: string) => api.post<CrmReminder>(`/crm/reminders/${id}/done`),
  cancel: (id: string) => api.post<CrmReminder>(`/crm/reminders/${id}/cancel`),
};

export const crmApi = {
  listOrgs: (params?: Record<string, unknown>) => api.get("/crm/organizations", { params }),
  createOrg: (data: unknown) => api.post("/crm/organizations", data),
  updateOrg: (id: string, data: unknown) => api.patch(`/crm/organizations/${id}`, data),
  deleteOrg: (id: string) => api.delete(`/crm/organizations/${id}`),
  enrichOrg: (id: string) => api.post(`/crm/organizations/${id}/enrich`),
  getOrg: (id: string) => api.get(`/crm/organizations/${id}`),
  listContacts: (params?: Record<string, unknown>) => api.get("/crm/contacts", { params }),
  getContact: (id: string) => api.get(`/crm/contacts/${id}`),
  createContact: (data: unknown) => api.post("/crm/contacts", data),
  updateContact: (id: string, data: unknown) => api.patch(`/crm/contacts/${id}`, data),
  deleteContact: (id: string) => api.delete(`/crm/contacts/${id}`),
  listLeads: (params?: Record<string, unknown>) => api.get("/crm/leads", { params }),
  getLead: (id: string) => api.get(`/crm/leads/${id}`),
  scoreLead: (id: string) => api.post(`/crm/leads/${id}/score`),
  createLead: (data: unknown) => api.post("/crm/leads", data),
  updateLead: (id: string, data: unknown) => api.patch(`/crm/leads/${id}`, data),
  deleteLead: (id: string) => api.delete(`/crm/leads/${id}`),
  listDeals: (params?: Record<string, unknown>) => api.get("/crm/deals", { params }),
  createDeal: (data: unknown) => api.post("/crm/deals", data),
  updateDeal: (id: string, data: unknown) => api.patch(`/crm/deals/${id}`, data),
  updateDealStage: (id: string, data: unknown) => api.patch(`/crm/deals/${id}/stage`, data),
  deleteDeal: (id: string) => api.delete(`/crm/deals/${id}`),
  listActivities: (params?: Record<string, unknown>) => api.get("/crm/activities", { params }),
  // 23 Oct: one activity with its full text (the timeline shows a 280-character preview)
  getActivity: (id: string) => api.get(`/crm/activities/${id}`),
  createActivity: (data: unknown) => api.post("/crm/activities", data),
  updateActivity: (id: string, data: unknown) => api.patch(`/crm/activities/${id}`, data),
  deleteActivity: (id: string) => api.delete(`/crm/activities/${id}`),
  // 26 Oct: deleted activities are kept; the Owner and admins list and restore them
  listDeletedActivities: () => api.get<{ items: DeletedActivity[] }>("/crm/activities-deleted"),
  restoreDeletedActivity: (id: string) =>
    api.post<{ activity_id: string; restored: boolean }>(`/crm/activities-deleted/${id}/restore`),
  getDeal: (id: string) => api.get(`/crm/deals/${id}`),
  getStages: () => api.get("/crm/stages"),
  promoteLead: (id: string) => api.post(`/crm/leads/${id}/promote`),
};

/** Attribute schema + record surface (timeline, AI brief, AI attribute runs). */
export const crmRecordsApi = {
  listAttributes: (params?: Record<string, unknown>) => api.get("/crm/attributes", { params }),
  createAttribute: (data: unknown) => api.post("/crm/attributes", data),
  updateAttribute: (id: string, data: unknown) => api.patch(`/crm/attributes/${id}`, data),
  deleteAttribute: (id: string) => api.delete(`/crm/attributes/${id}`),

  timeline: (objectType: string, id: string, params?: Record<string, unknown>) =>
    api.get(`/crm/records/${objectType}/${id}/timeline`, { params }),
  aiFields: (objectType: string, id: string) =>
    api.get(`/crm/records/${objectType}/${id}/ai-fields`),
  runAiField: (objectType: string, id: string, key: string) =>
    api.post(`/crm/records/${objectType}/${id}/ai-fields/${key}/run`),
  brief: (objectType: string, id: string) =>
    api.post(`/crm/records/${objectType}/${id}/brief`),

  // Phase 2: confirmed AI writes
  previewChange: (data: unknown) => api.post("/crm/ai/preview", data),
  applyChange: (data: unknown) => api.post("/crm/ai/apply", data),

  // Phase 4: search + bulk runs
  searchSchema: (objectType: string) =>
    api.get("/crm/search/schema", { params: { object_type: objectType } }),
  searchFilters: (data: unknown) => api.post("/crm/search/filters", data),
  bulkRunAiField: (data: unknown) => api.post("/crm/records/bulk/ai-fields/run", data),

  // Phase 5: duplicates
  scanDuplicates: (objectType: string, threshold?: number) =>
    api.post("/crm/duplicates/scan", null, { params: { object_type: objectType, threshold } }),
  listDuplicates: (params?: Record<string, unknown>) => api.get("/crm/duplicates", { params }),
  mergePreview: (params: Record<string, unknown>) =>
    api.get("/crm/duplicates/preview", { params }),
  mergeRecords: (data: unknown) => api.post("/crm/duplicates/merge", data),
  dismissDuplicate: (id: string) => api.post(`/crm/duplicates/${id}/dismiss`),
};

/** AI knowledge: documents only the AI reads (admin only). */
export interface AiKnowledgeDoc {
  id: string; title: string; description?: string | null; source_type: "markdown" | "pdf";
  /** The ERP guide, rebuilt from the code: read-only, can only be switched on or off. */
  system?: boolean;
  original_filename?: string | null; original_size?: number | null;
  status: "processing" | "ready" | "failed"; error?: string | null; is_active: boolean;
  /** Nary may use it in a customer conversation (no costs, tiers or internal rules). */
  customer_safe?: boolean;
  chunk_count: number; use_count: number; last_used_at?: string | null;
  created_at?: string | null; updated_at?: string | null; characters: number; content?: string;
}
export const aiKnowledgeApi = {
  list: () => api.get<{ items: AiKnowledgeDoc[] }>("/ai/knowledge"),
  get: (id: string) => api.get<AiKnowledgeDoc>(`/ai/knowledge/${id}`),
  upload: (form: FormData) => api.post<AiKnowledgeDoc>("/ai/knowledge", form, { timeout: 300000 }),
  update: (id: string, data: Partial<Pick<AiKnowledgeDoc, "title" | "description" | "is_active" | "content" | "customer_safe">>) =>
    api.patch<AiKnowledgeDoc>(`/ai/knowledge/${id}`, data),
  reindex: (id: string) => api.post<AiKnowledgeDoc>(`/ai/knowledge/${id}/reindex`),
  remove: (id: string) => api.delete(`/ai/knowledge/${id}`),
  search: (query: string) =>
    api.post<{ results: { doc_id: string; title: string; content: string; distance: number; used: boolean }[] }>(
      "/ai/knowledge/search", { query }),
};

/** Web research on companies and contacts (SearXNG + the research service). */
export type ResearchObject = "organization" | "contact";
export interface ResearchRun {
  id: string; object_type: ResearchObject; record_id: string; depth: string;
  status: "queued" | "running" | "done" | "failed";
  summary?: string | null; error?: string | null;
  sources: { url: string; title?: string | null }[];
  news: { title: string; url: string; date?: string }[];
  channels: { phone?: string; email?: string; address?: string; socials?: string[] };
  written: Record<string, unknown>; suggested: Record<string, unknown>;
  created_at?: string | null; finished_at?: string | null;
}
export interface ResearchPerson {
  id: string; name: string; title?: string | null; source_url?: string | null; evidence?: string | null;
  status: "pending" | "added" | "dismissed"; contact_id?: string | null; matched_contact_id?: string | null;
}
export const researchApi = {
  start: (objectType: ResearchObject, id: string) =>
    api.post<{ run: ResearchRun; queued: boolean }>(`/crm/research/${objectType}/${id}`),
  state: (objectType: ResearchObject, id: string) =>
    api.get<{ run: ResearchRun | null; people: ResearchPerson[] }>(`/crm/research/${objectType}/${id}`),
  addPerson: (personId: string, data: { name?: string; title?: string }) =>
    api.post<{ contact_id: string; person: ResearchPerson }>(`/crm/research/people/${personId}/add`, data),
  dismissPerson: (personId: string) => api.post<ResearchPerson>(`/crm/research/people/${personId}/dismiss`),
};

/** MCP server: personal access tokens for external agents. */
export const mcpApi = {
  listTokens: () => api.get("/mcp/tokens"),
  /** `password` is required for act / full power (the backend refuses those without it). Never stored. */
  createToken: (data: { name: string; expires_in_days?: number | null; scopes?: string[]; password?: string; totp_code?: string }) =>
    api.post("/mcp/tokens", data),
  revokeToken: (id: string) => api.delete(`/mcp/tokens/${id}`),
  /** Personal tokens and agents logged in by browser; `all` (admins) lists everyone's. */
  connections: (all = false) => api.get("/mcp/connections", { params: all ? { all: true } : {} }),
  revokeConnection: (id: string) => api.delete(`/mcp/connections/${id}`),
  levels: () => api.get("/mcp/levels"),
  /** The agent asking to connect (consent page). */
  oauthClient: (clientId: string, redirectUri: string) =>
    api.get("/mcp/oauth/client", { params: { client_id: clientId, redirect_uri: redirectUri } }),
  /** `password` (and `totp_code` with TOTP on) are required for act / full power. Never stored. */
  oauthAuthorize: (body: { client_id: string; redirect_uri: string; code_challenge: string;
                           code_challenge_method: string; scopes: string[]; state?: string | null;
                           password?: string; totp_code?: string }) =>
    api.post("/mcp/oauth/authorize", body),
};

export const noteTemplatesApi = {
  list: (docType?: string) => api.get("/note-templates", { params: docType ? { doc_type: docType } : {} }),
  create: (data: { name: string; body: string; doc_type?: string | null }) => api.post("/note-templates", data),
  update: (id: string, data: unknown) => api.patch(`/note-templates/${id}`, data),
  remove: (id: string) => api.delete(`/note-templates/${id}`),
};

export const salesApi = {
  listCatalog: (params?: Record<string, unknown>) => api.get("/sales/catalog", { params }),
  catalogAccess: () => api.get<{ can_edit: boolean }>("/sales/catalog/access"),
  createProduct: (data: unknown) => api.post("/sales/catalog", data),
  getProduct: (id: string) => api.get(`/sales/catalog/${id}`),
  updateProduct: (id: string, data: unknown) => api.patch(`/sales/catalog/${id}`, data),
  deleteProduct: (id: string) => api.delete(`/sales/catalog/${id}`),
  // Price list (one row per currency and effective date) and volume tiers; catalog managers only.
  addCatalogPrice: (productId: string, data: { currency: string; amount: number; valid_from: string; valid_to?: string | null }) =>
    api.post(`/sales/catalog/${productId}/prices`, data),
  updateCatalogPrice: (priceId: string, data: { amount?: number; valid_from?: string; valid_to?: string | null; currency?: string }) =>
    api.patch(`/sales/catalog/prices/${priceId}`, data),
  deleteCatalogPrice: (priceId: string) => api.delete(`/sales/catalog/prices/${priceId}`),
  putCatalogTiers: (productId: string, tiers: { min_qty: number; max_qty: number | null; discount_pct: number }[]) =>
    api.put(`/sales/catalog/${productId}/tiers`, { tiers }),
  /** Validity and payment-term days a new quotation starts with (Settings). */
  quotationDefaults: () => api.get("/sales/quotation-defaults"),
  /** 2 Nov 2026: the "Prices exclude tax" sentence and the phrases the notes/tax check reads (Settings). */
  taxNotes: () => api.get<TaxNotes>("/sales/tax-notes"),
  /** 4 Nov 2026: the house notes template of the form's lines (family, clauses, notes to fill, payment terms). */
  quotationNotesTemplate: (data: object) => api.post("/sales/quotation-notes/template", data),
  /** 4 Nov 2026: the notes check on the form as it stands (what the approval would refuse). */
  quotationNotesCheck: (data: object) => api.post("/sales/quotation-notes/check", data),
  /** Server-priced lines (list price, tier, add-on %, white-label, tax, totals, payment terms). */
  pricingPreview: (data: { currency: string; issue_date?: string; customer_org_id?: string; lines: LineIn[];
    discount_type?: string; discount_value?: number }) =>
    api.post<PricingPreview>("/sales/pricing/preview", data),
  listQuotations: (params?: Record<string, unknown>) => api.get("/sales/quotations", { params }),
  createQuotation: (data: unknown) => api.post("/sales/quotations", data),
  updateQuotation: (id: string, data: unknown) => api.patch(`/sales/quotations/${id}`, data),
  reviseQuotation: (id: string, reason: string) => api.post(`/sales/quotations/${id}/revise`, { reason }),
  // 1 Nov 2026: withdraw from approval (submitter or Owner), cancel (kept for history), reprice to today
  withdrawQuotation: (id: string, reason?: string) => api.post(`/sales/quotations/${id}/withdraw`, { reason }),
  cancelQuotation: (id: string, reason: string) => api.post(`/sales/quotations/${id}/cancel`, { reason }),
  repriceQuotation: (id: string) => api.post(`/sales/quotations/${id}/reprice`, {}),
  // 5 Nov 2026: link a sent or approved quotation to a deal without a revision (returns the quotation)
  linkQuotationDeal: (id: string, dealId: string, setDealValue = true) =>
    api.post(`/sales/quotations/${id}/link-deal`, { deal_id: dealId, set_deal_value: setDealValue }),
  // 5 Nov 2026 (E7): the Owner lets a draft through the pricing rules with a reason (empty removes it)
  pricingOverride: (id: string, reason: string) => api.post(`/sales/quotations/${id}/pricing-override`, { reason }),
  generateInvoiceFromQuotation: (id: string, data: unknown) => api.post(`/sales/quotations/${id}/generate-invoice`, data),
  getQuotation: (id: string) => api.get(`/sales/quotations/${id}`),
  updateStatus: (id: string, status: string) => api.patch(`/sales/quotations/${id}/status`, { status }),
  sendQuotation: (id: string, opts?: { send_email?: boolean; recipient_email?: string }) =>
    api.post(`/sales/quotations/${id}/send`, opts ?? {}),
  recordPO: (id: string, data: unknown) => api.patch(`/sales/quotations/${id}/record-po`, data),
  deleteQuotation: (id: string) => api.delete(`/sales/quotations/${id}`),
  listContracts: (params?: Record<string, unknown>) => api.get("/sales/contracts", { params }),
  createContract: (data: unknown) => api.post("/sales/contracts", data),
  getContract: (id: string) => api.get(`/sales/contracts/${id}`),
  updateContractStatus: (id: string, data: unknown) => api.patch(`/sales/contracts/${id}/status`, data),
  deleteContract: (id: string) => api.delete(`/sales/contracts/${id}`),
};

export const financeApi = {
  listInvoices: (params?: Record<string, unknown>) => api.get("/finance/invoices", { params }),
  invoiceSummary: () => api.get("/finance/invoices/summary"),
  createInvoice: (data: unknown) => api.post("/finance/invoices", data),
  updateInvoice: (id: string, data: unknown) => api.patch(`/finance/invoices/${id}`, data),
  reviseInvoice: (id: string, reason: string) => api.post(`/finance/invoices/${id}/revise`, { reason }),
  issueCreditNote: (id: string, reason: string) => api.post(`/finance/invoices/${id}/credit-note`, { reason }),
  getInvoice: (id: string) => api.get(`/finance/invoices/${id}`),
  updateInvoiceStatus: (id: string, status: string) => api.patch(`/finance/invoices/${id}/status`, { status }),
  convertToCommercial: (proformaId: string, data: unknown) => api.post(`/finance/invoices/${proformaId}/convert-to-commercial`, data),
  deleteInvoice: (id: string) => api.delete(`/finance/invoices/${id}`),
  recordPayment: (data: unknown) => api.post("/finance/payments", data),
  listPayments: (params?: Record<string, unknown>) => api.get("/finance/payments", { params }),
  listExpenses: (params?: Record<string, unknown>) => api.get("/finance/expenses", { params }),
  submitExpense: (data: unknown) => api.post("/finance/expenses", data),
  reviewExpense: (id: string, data: unknown) => api.patch(`/finance/expenses/${id}/review`, data),
  deleteExpense: (id: string) => api.delete(`/finance/expenses/${id}`),
  /** Pay an approved expense back to the employee (empty bank_account_id = the bank_default GL role). */
  reimburseExpense: (id: string, data: { paid_date?: string; bank_account_id?: string } = {}) =>
    api.post(`/finance/expenses/${id}/reimburse`, data),
  listSchedules: (params?: Record<string, unknown>) => api.get("/finance/payment-schedules", { params }),
  createSchedule: (data: unknown) => api.post("/finance/payment-schedules", data),
  /** Raise the proforma invoice for one milestone (the customer comes from its quotation). */
  raiseProforma: (scheduleId: string) => api.post(`/finance/payment-schedules/${scheduleId}/raise-proforma`, {}),
  // Chart of Accounts
  listAccounts: (params?: Record<string, unknown>) => api.get("/finance/accounts", { params }),
  createAccount: (data: unknown) => api.post("/finance/accounts", data),
  updateAccount: (id: string, data: unknown) => api.patch(`/finance/accounts/${id}`, data),
  deleteAccount: (id: string) => api.delete(`/finance/accounts/${id}`),
  // Fiscal Periods
  listFiscalPeriods: (params?: Record<string, unknown>) => api.get("/finance/fiscal-periods", { params }),
  togglePeriodLock: (id: string, is_locked: boolean) => api.patch(`/finance/fiscal-periods/${id}/lock`, { is_locked }),
  generateFiscalPeriods: (year: number) => api.post("/finance/fiscal-periods/generate", { year }),
  // Journal Entries
  listJournal: (params?: Record<string, unknown>) => api.get("/finance/journal", { params }),
  createJournalEntry: (data: unknown) => api.post("/finance/journal", data),
  getJournalEntry: (id: string) => api.get(`/finance/journal/${id}`),
  postJournalEntry: (id: string) => api.post(`/finance/journal/${id}/post`),
  unpostJournalEntry: (id: string) => api.post(`/finance/journal/${id}/unpost`),
  reverseJournalEntry: (id: string, data: unknown) => api.post(`/finance/journal/${id}/reverse`, data),
  /** Automatic ledger entries (and reversals) of one document event. */
  journalBySource: (source: string, sourceId: string) =>
    api.get("/finance/journal/by-source", { params: { source, source_id: sourceId } }),
  // GL account roles (automatic posting)
  getGlRoles: () => api.get("/finance/gl-roles"),
  updateGlRoles: (roles: Record<string, string | null>) => api.put("/finance/gl-roles", { roles }),
  // Reports
  getTrialBalance: (params?: Record<string, unknown>) => api.get("/finance/reports/trial-balance", { params }),
  getProfitAndLoss: (params?: Record<string, unknown>) => api.get("/finance/reports/pl", { params }),
  getBalanceSheet: (params?: Record<string, unknown>) => api.get("/finance/reports/balance-sheet", { params }),
  getGeneralLedger: (accountId: string, params?: Record<string, unknown>) => api.get(`/finance/reports/gl/${accountId}`, { params }),
  getAgedReceivables: (params?: Record<string, unknown>) => api.get("/finance/reports/aged-receivables", { params }),
  // Taxes
  listTaxes: (params?: Record<string, unknown>) => api.get("/finance/taxes", { params }),
  createTax: (data: unknown) => api.post("/finance/taxes", data),
  updateTax: (id: string, data: unknown) => api.patch(`/finance/taxes/${id}`, data),
  deleteTax: (id: string) => api.delete(`/finance/taxes/${id}`),
  // Exchange Rates
  listExchangeRates: (params?: Record<string, unknown>) => api.get("/finance/exchange-rates", { params }),
  createExchangeRate: (data: unknown) => api.post("/finance/exchange-rates", data),
  deleteExchangeRate: (id: string) => api.delete(`/finance/exchange-rates/${id}`),
  getSpotRate: (from: string, to: string, onDate?: string) =>
    api.get("/finance/fx/rate", { params: { from_currency: from, to_currency: to, ...(onDate ? { on_date: onDate } : {}) } }),
  // Bank
  listBankAccounts: () => api.get("/finance/bank-accounts"),
  createBankAccount: (data: unknown) => api.post("/finance/bank-accounts", data),
  updateBankAccount: (id: string, data: unknown) => api.patch(`/finance/bank-accounts/${id}`, data),
  listStatements: (bankAccountId: string) => api.get(`/finance/bank-accounts/${bankAccountId}/statements`),
  createStatement: (bankAccountId: string, data: unknown) => api.post(`/finance/bank-accounts/${bankAccountId}/statements`, data),
  listStatementLines: (statementId: string) => api.get(`/finance/bank-statements/${statementId}/lines`),
  addStatementLines: (statementId: string, data: unknown) => api.post(`/finance/bank-statements/${statementId}/lines/bulk`, data),
  matchStatementLine: (statementId: string, lineId: string, data: unknown) => api.post(`/finance/bank-statements/${statementId}/lines/${lineId}/match`, data),
  listCandidateLines: (statementId: string, lineId: string) =>
    api.get(`/finance/bank-statements/${statementId}/candidate-lines`, { params: { line_id: lineId } }),
  reconcileStatement: (statementId: string) => api.patch(`/finance/bank-statements/${statementId}/reconcile`),
  /** Leave a statement line out of matching (it still counts toward the balance check). */
  excludeStatementLine: (statementId: string, lineId: string, excluded: boolean) =>
    api.post(`/finance/bank-statements/${statementId}/lines/${lineId}/exclude`, { excluded }),
  /** Pay back an invoice's refund_due (money a credit note left owing to the customer). */
  recordRefund: (invoiceId: string, data: { amount: number | string; paid_date?: string; bank_account_id?: string; reference?: string }) =>
    api.post(`/finance/invoices/${invoiceId}/refund`, data),
  // Opening migration
  migrateOpeningEntries: () => api.post("/finance/accounting/migrate-opening"),
  deleteJournalEntry: (id: string) => api.delete(`/finance/journal/${id}`),
  deleteSchedule: (id: string) => api.delete(`/finance/payment-schedules/${id}`),
};

export const systemApi = {
  getActiveConfig: () => api.get("/system/tax-configs/active"),
  listSettings: () => api.get("/system/settings"),
  updateSetting: (key: string, value: string) => api.patch(`/system/settings/${key}`, { value }),
  listTaxConfigs: () => api.get("/system/tax-configs"),
  createTaxConfig: (data: unknown) => api.post("/system/tax-configs", data),
  activateTaxConfig: (id: string) => api.post(`/system/tax-configs/${id}/activate`),
  addTaxRule: (configId: string, data: unknown) => api.post(`/system/tax-configs/${configId}/rules`, data),
  updateTaxRule: (ruleId: string, data: unknown) => api.patch(`/system/tax-rules/${ruleId}`, data),
  getAuditLog: (params?: Record<string, unknown>) => api.get("/system/audit-log", { params }),
  getCompanySettings: () => api.get("/system/company"),
  getEmailConfig: () => api.get("/system/email-config"),
  updateEmailConfig: (key: string, value: string) => api.patch("/system/email-config", { key, value }),
  testEmailConfig: (data: Record<string, unknown>) => api.post("/system/email-config/test", data),
  testImapConfig: (data: Record<string, unknown>) => api.post("/system/email-config/test-imap", data),
  /** The CRM's shared inboxes (hello@ is prefix "crm"); each one's servers live in email-config under its prefix. */
  listCrmMailboxes: () => api.get<{ mailboxes: CrmMailbox[] }>("/system/crm-mailboxes"),
  createCrmMailbox: (data: { address: string; display_name: string; purpose: "general" | "sales"; backfill_days: number }) =>
    api.post<{ mailbox: CrmMailbox }>("/system/crm-mailboxes", data),
  updateCrmMailbox: (id: string, data: Partial<Pick<CrmMailbox,
    "display_name" | "purpose" | "active" | "is_default" | "backfill_days" | "signature_html" | "sort">>) =>
    api.patch<{ mailbox: CrmMailbox }>(`/system/crm-mailboxes/${id}`, data),
  getLetterhead: () => api.get("/system/settings/letterhead"),
  uploadLetterhead: (file: File) => {
    const fd = new FormData();
    fd.append("file", file);
    return api.post("/system/settings/letterhead", fd, {
      headers: { "Content-Type": "multipart/form-data" },
    });
  },
  deleteLetterhead: () => api.delete("/system/settings/letterhead"),
  getBranding: () => api.get("/system/branding"),
  updateBranding: (data: {
    logo_type?: string; logo_text?: string; logo_subtitle?: string;
    login_badge?: string; login_headline?: string; login_headline_accent?: string;
    login_tagline?: string; login_features?: string; login_footer?: string;
    login_subtitle?: string; login_copyright?: string;
  }) => api.patch("/system/branding", data),
  uploadLogo: (file: File) => {
    const fd = new FormData();
    fd.append("file", file);
    return api.post("/system/branding/logo", fd, { headers: { "Content-Type": "multipart/form-data" } });
  },
  deleteLogo: () => api.delete("/system/branding/logo"),
  uploadFavicon: (file: File) => {
    const fd = new FormData();
    fd.append("file", file);
    return api.post("/system/branding/favicon", fd, { headers: { "Content-Type": "multipart/form-data" } });
  },
  deleteFavicon: () => api.delete("/system/branding/favicon"),
  getDashboardKpis: () => api.get("/system/dashboard-kpis"),
  listNotifications: (params?: Record<string, unknown>) => api.get("/system/notifications", { params }),
  getUnreadCount: () => api.get("/system/notifications/unread-count"),
  markNotificationRead: (id: string) => api.patch(`/system/notifications/${id}/read`),
  markAllNotificationsRead: () => api.post("/system/notifications/read-all"),
  // Currency settings
  getCurrencySettings: () => api.get("/system/currencies"),
  updateCurrencySettings: (data: { functional?: string; active?: string[]; api_key?: string }) =>
    api.patch("/system/currencies", data),
  syncExchangeRates: () => api.post("/system/currencies/sync-now"),
  testFxApiKey: () => api.post("/system/currencies/test-api-key"),
  // AI settings
  getAiSettings: () => api.get("/system/ai-settings"),
  listAiModels: (provider?: string) => api.get("/system/ai-settings/models", { params: provider ? { provider } : {} }),
  updateAiSettings: (data: Record<string, unknown>) => api.put("/system/ai-settings", data),
  testAiConnection: (job?: string) => api.post("/system/ai-settings/test", null, { params: job ? { job } : {} }),
  testAiProvider: (provider: string) => api.post("/system/ai-settings/test-provider", null, { params: { provider } }),
  aiUsage: () => api.get("/system/ai-usage"),
  reindexKb: () => api.post("/system/ai-settings/reindex"),
};

/** Cloudflare cuts a request at 100 MB: a large file (a meeting recording) goes up in pieces of this size, one
 *  after the other, and the server puts them back together after the last one. */
const UPLOAD_PIECE_BYTES = 45 * 1024 * 1024;

function uploadId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const hex = Array.from({ length: 32 }, () => Math.floor(Math.random() * 16).toString(16)).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-4${hex.slice(13, 16)}-a${hex.slice(17, 20)}-${hex.slice(20)}`;
}

/** POST a file as multipart `file`, in pieces when it is large; the last answer is the server's. */
async function postInPieces(path: string, file: File, onProgress?: (percent: number) => void) {
  const total = Math.max(1, Math.ceil(file.size / UPLOAD_PIECE_BYTES));
  const id = uploadId();
  let last: AxiosResponse | null = null;
  for (let i = 0; i < total; i++) {
    const start = i * UPLOAD_PIECE_BYTES;
    const piece = file.slice(start, Math.min(file.size, start + UPLOAD_PIECE_BYTES), file.type);
    const form = new FormData();
    form.append("file", piece, file.name);
    if (total > 1) {
      form.append("upload_id", id);
      form.append("index", String(i));
      form.append("total", String(total));
    }
    last = await api.post(path, form, {
      timeout: 0,
      onUploadProgress: (e) => {
        if (onProgress && file.size) onProgress(Math.min(100, Math.round(((start + e.loaded) * 100) / file.size)));
      },
    });
  }
  return last!;
}

export const aiApi = {
  listConversations: () => api.get("/ai/conversations"),
  getConversation: (id: string) => api.get(`/ai/conversations/${id}`),
  renameConversation: (id: string, title: string) =>
    api.patch(`/ai/conversations/${id}`, { title }),
  deleteConversation: (id: string) => api.delete(`/ai/conversations/${id}`),
  setProposalStatus: (conversationId: string, proposalId: string,
                      body: { status: "applied" | "discarded"; result?: { id?: string; label?: string } }) =>
    api.patch(`/ai/conversations/${conversationId}/proposals/${proposalId}`, body),
  suggestReply: (body: { channel: "whatsapp" | "email"; chat_id?: string; thread_id?: string; instruction?: string }) =>
    api.post("/ai/suggest/reply", body),
  analyzeDocument: (body: { source: "attachment" | "email" | "crm_email"; attachment_id?: string; message_id?: string; att_id?: string; question?: string }) =>
    api.post("/ai/document/analyze", body),
  /** Streams the assistant reply (SSE). Calls onEvent for each parsed event. */
  chatStream: async (
    body: { message: string; conversation_id?: string | null; context?: Record<string, unknown> | null },
    handlers: { onEvent: (ev: Record<string, unknown>) => void; signal?: AbortSignal },
  ) => {
    const session = await getSession();
    const res = await fetch("/api/v1/ai/assistant/chat", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(session?.access_token ? { Authorization: `Bearer ${session.access_token}` } : {}),
      },
      body: JSON.stringify(body),
      signal: handlers.signal,
    });
    if (!res.ok || !res.body) {
      // Keep the server's reason (409: a turn of this chat is still running) for the chat to show.
      let detail = "";
      try { const b = await res.json(); if (typeof b?.detail === "string") detail = b.detail; } catch { /* no JSON body */ }
      throw Object.assign(new Error(`AI chat failed (${res.status})`), { status: res.status, detail });
    }
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      const parts = buf.split("\n\n");
      buf = parts.pop() || "";
      for (const part of parts) {
        const line = part.trim();
        if (!line.startsWith("data:")) continue;
        const payload = line.slice(5).trim();
        if (!payload) continue;
        try { handlers.onEvent(JSON.parse(payload)); } catch { /* ignore partial */ }
      }
    }
  },
};

/** One person of the staff directory: safe fields only (no e-mail, roles or flags). */
export interface StaffPerson {
  id: string;
  name: string;
  avatar_url?: string | null;
  department?: string | null;
  position?: string | null;
}

/**
 * The staff directory every internal user may read: active internal users, never portal accounts
 * (GET /messaging/people). Use it for people pickers (assignees, owners, members). GET /users is the
 * admin list (system:manage_users) and refuses everyone else.
 */
export const peopleApi = {
  directory: () => api.get<StaffPerson[]>("/messaging/people"),
};

export const usersApi = {
  list: (params?: Record<string, unknown>) => api.get("/users", { params }),
  get: (id: string) => api.get(`/users/${id}`),
  create: (data: unknown) => api.post("/users", data),
  update: (id: string, data: unknown) => api.patch(`/users/${id}`, data),
  deactivate: (id: string) => api.delete(`/users/${id}`),
  resetPassword: (id: string, newPassword: string) => api.post(`/users/${id}/reset-password`, { new_password: newPassword }),
  changeMyPassword: (currentPassword: string, newPassword: string) =>
    api.patch("/users/me/password", { current_password: currentPassword, new_password: newPassword }),
  assignRole: (userId: string, role: string) => api.post(`/users/${userId}/roles/${encodeURIComponent(role)}`),
  removeRole: (userId: string, role: string) => api.delete(`/users/${userId}/roles/${encodeURIComponent(role)}`),
  /** Two-factor sign-in of a user (Owner and system admins). */
  totpStatus: (id: string) => api.get<{ enabled: boolean; confirmed_at: string | null; recovery_codes_left: number }>(`/users/${id}/totp`),
  /** Removes the user's two-factor sign-in (audit-logged); they set it up again. */
  resetTotp: (id: string) => api.delete<{ reset: boolean; enabled: boolean }>(`/users/${id}/totp`),
};

/** Roles and the permission catalogue (Settings > Roles). Creating and editing: system admins. */
export interface RoleRecord { id: string; name: string; slug: string; description: string | null; is_system_role: boolean | null; permissions: string[] }
export interface PermissionRecord { code: string; name: string; description: string | null }
export const rolesApi = {
  list: () => api.get<RoleRecord[]>("/roles"),
  permissions: () => api.get<PermissionRecord[]>("/roles/permissions"),
  create: (data: { name: string; description?: string; permission_codes: string[] }) => api.post<{ id: string; name: string }>("/roles", data),
  /** A custom role; `permission_codes` replaces the whole set. Built-in roles are refused. */
  update: (id: string, data: { description?: string; permission_codes?: string[] }) => api.patch<{ id: string; name: string }>(`/roles/${id}`, data),
};

export const projectsApi = {
  list: (params?: Record<string, unknown>) => api.get("/projects", { params }),
  create: (data: unknown) => api.post("/projects", data),
  get: (id: string) => api.get(`/projects/${id}`),
  update: (id: string, data: unknown) => api.patch(`/projects/${id}`, data),
  delete: (id: string) => api.delete(`/projects/${id}`),
  logTime: (data: unknown) => api.post("/projects/time-logs", data),
  listTimeLogs: (params?: Record<string, unknown>) => api.get("/projects/time-logs", { params }),
  createTask: (data: unknown) => api.post("/projects/tasks", data),
  updateTask: (id: string, data: unknown) => api.patch(`/projects/tasks/${id}`, data),
  deleteTask: (id: string) => api.delete(`/projects/tasks/${id}`),
  createMilestone: (data: unknown) => api.post("/projects/milestones", data),
  listMilestones: (projectId: string) => api.get(`/projects/${projectId}/milestones`),
  /** Puts a staff user on the project's team (projects:edit). */
  addMember: (projectId: string, data: { user_id: string; role?: string; allocation_percentage?: number | null }) =>
    api.post(`/projects/${projectId}/members`, data),
  /** Changes a member's role and/or allocation in place; `allocation_percentage: null` clears it. */
  updateMember: (projectId: string, userId: string, data: { role?: string; allocation_percentage?: number | null }) =>
    api.patch(`/projects/${projectId}/members/${userId}`, data),
  /** Takes someone off the team; the project manager stays until another is chosen. */
  removeMember: (projectId: string, userId: string) => api.delete(`/projects/${projectId}/members/${userId}`),
};

export const hrApi = {
  listJobTitles: () => api.get("/hr/job-titles"),
  listEmployees: (params?: Record<string, unknown>) => api.get("/hr/employees", { params }),
  createEmployee: (data: unknown) => api.post("/hr/employees", data),
  updateEmployee: (id: string, data: unknown) => api.patch(`/hr/employees/${id}`, data),
  getEmployee: (id: string) => api.get(`/hr/employees/${id}`),
  deleteEmployee: (id: string) => api.delete(`/hr/employees/${id}`),
  listAllowances: (employeeId: string) => api.get(`/hr/employees/${employeeId}/allowances`),
  createAllowance: (employeeId: string, data: unknown) => api.post(`/hr/employees/${employeeId}/allowances`, data),
  updateAllowance: (id: string, data: unknown) => api.patch(`/hr/allowances/${id}`, data),
  deleteAllowance: (id: string) => api.delete(`/hr/allowances/${id}`),
  submitLeave: (data: unknown) => api.post("/hr/leave", data),
  listLeave: (params?: Record<string, unknown>) => api.get("/hr/leave", { params }),
  reviewLeave: (id: string, data: unknown) => api.patch(`/hr/leave/${id}/review`, data),
  deleteLeave: (id: string) => api.delete(`/hr/leave/${id}`),
  listHiringRequests: (params?: Record<string, unknown>) => api.get("/hr/hiring-requests", { params }),
  createHiringRequest: (data: unknown) => api.post("/hr/hiring-requests", data),
  deleteHiringRequest: (id: string) => api.delete(`/hr/hiring-requests/${id}`),
  listJobOpenings: (params?: Record<string, unknown>) => api.get("/hr/job-openings", { params }),
  createJobOpening: (data: unknown) => api.post("/hr/job-openings", data),
  deleteJobOpening: (id: string) => api.delete(`/hr/job-openings/${id}`),
  listApplicants: (params?: Record<string, unknown>) => api.get("/hr/applicants", { params }),
  createApplicant: (data: unknown) => api.post("/hr/applicants", data),
  updateApplicantStage: (id: string, data: unknown) => api.patch(`/hr/applicants/${id}/stage`, data),
  listOfferLetters: (params?: Record<string, unknown>) => api.get("/hr/offer-letters", { params }),
  getOfferLetter: (id: string) => api.get(`/hr/offer-letters/${id}`),
  createOfferLetter: (data: unknown) => api.post("/hr/offer-letters", data),
  updateOfferLetter: (id: string, data: unknown) => api.patch(`/hr/offer-letters/${id}`, data),
  sendOfferLetter: (id: string) => api.post(`/hr/offer-letters/${id}/send`),
  recordOfferResponse: (id: string, data: { response: string; notes?: string }) =>
    api.post(`/hr/offer-letters/${id}/response`, data),
  cancelOfferLetter: (id: string, reason?: string) => api.post(`/hr/offer-letters/${id}/cancel`, { reason }),
  deleteOfferLetter: (id: string) => api.delete(`/hr/offer-letters/${id}`),
  listAttendance: (params?: Record<string, unknown>) => api.get("/hr/attendance", { params }),
  recordAttendance: (data: unknown) => api.post("/hr/attendance", data),
  listPayrollRuns: () => api.get("/hr/payroll"),
  createPayrollRun: (data: unknown) => api.post("/hr/payroll", data),
  getPayrollRun: (id: string) => api.get(`/hr/payroll/${id}`),
  approvePayrollRun: (id: string) => api.patch(`/hr/payroll/${id}/approve`),
  markPayrollPaid: (id: string, data: { paid_date?: string; bank_account_id?: string } = {}) =>
    api.patch(`/hr/payroll/${id}/mark-paid`, data),
  // Face attendance
  getFaceEnrollment: (employeeId: string) => api.get(`/hr/employees/${employeeId}/face`),
  deleteFaceEnrollment: (employeeId: string) => api.delete(`/hr/employees/${employeeId}/face`),
  enrollFace: (employeeId: string, formData: FormData) =>
    api.post(`/hr/employees/${employeeId}/face/enroll`, formData, { headers: { "Content-Type": "multipart/form-data" } }),
  // Kiosk tablets (registered on the tablet itself; listed and revoked in Settings → People & payroll → Kiosk tablets)
  listKiosks: () => api.get<KioskDevice[]>("/hr/kiosks"),
  registerKiosk: (data: { name: string; office_location_id: string }) =>
    api.post<KioskDevice & { token: string }>("/hr/kiosks", data),
  revokeKiosk: (id: string) => api.post(`/hr/kiosks/${id}/revoke`),
  listOfficeLocations: () => api.get("/hr/office-locations"),
  createOfficeLocation: (data: unknown) => api.post("/hr/office-locations", data),
  updateOfficeLocation: (id: string, data: unknown) => api.patch(`/hr/office-locations/${id}`, data),
  deleteOfficeLocation: (id: string) => api.delete(`/hr/office-locations/${id}`),
  // Personal-device clock-in: a server-issued liveness challenge, then three key frames + GPS.
  // With `passive.enabled` (Release 2) the phone flashes the server's colours first (clockPassive).
  faceChallenge: (eventType: "clock_in" | "clock_out") =>
    api.post<FaceChallenge>("/hr/attendance/challenge", { event_type: eventType }),
  // How one face check ended (telemetry; the clocks fire and forget it).
  reportFaceAttempt: (data: FaceAttemptReport) => api.post("/hr/attendance/face-attempts", data),
  faceAttemptsSummary: (days = 7) =>
    api.get<FaceAttemptsSummary>("/hr/attendance/face-attempts/summary", { params: { days } }),
  clockWithFrames: (eventType: "clock_in" | "clock_out", formData: FormData) =>
    api.post(`/hr/attendance/${eventType === "clock_in" ? "clock-in" : "clock-out"}`, formData,
      { headers: { "Content-Type": "multipart/form-data" } }),
  // Passive clock (Release 2): nonce, mode=passive, frame_neutral, flash_0..k-1 and GPS. 200 is either the clock
  // result (as clockWithFrames) or {active_required, reason, challenge} when the active step is needed.
  clockPassive: (eventType: "clock_in" | "clock_out", formData: FormData) =>
    api.post<PassiveClockResponse>(`/hr/attendance/${eventType === "clock_in" ? "clock-in" : "clock-out"}`, formData,
      { headers: { "Content-Type": "multipart/form-data" } }),
  listClockEvents: (params?: Record<string, unknown>) => api.get("/hr/attendance/clock-events", { params }),
  listPendingOverrides: () => api.get("/hr/attendance/pending-overrides"),
  overrideClockEvent: (id: string, data: unknown) => api.post(`/hr/attendance/clock-events/${id}/override`, data),
  myToday: () => api.get("/hr/attendance/my-today"),
};

export const manufacturingApi = {
  listProducts: (params?: Record<string, unknown>) => api.get("/manufacturing/products", { params }),
  createProduct: (data: unknown) => api.post("/manufacturing/products", data),
  updateProduct: (id: string, data: unknown) => api.patch(`/manufacturing/products/${id}`, data),
  deleteProduct: (id: string) => api.delete(`/manufacturing/products/${id}`),
  getBOM: (productId: string) => api.get(`/manufacturing/bom/${productId}`),
  createBOM: (data: unknown) => api.post("/manufacturing/bom", data),
  listInventory: (params?: Record<string, unknown>) => api.get("/manufacturing/inventory", { params }),
  createInventoryItem: (data: unknown) => api.post("/manufacturing/inventory", data),
  adjustInventory: (id: string, data: unknown) => api.patch(`/manufacturing/inventory/${id}/adjust`, data),
  createProductionOrder: (data: unknown) => api.post("/manufacturing/production-orders", data),
  listProductionOrders: (params?: Record<string, unknown>) => api.get("/manufacturing/production-orders", { params }),
  deleteProductionOrder: (id: string) => api.delete(`/manufacturing/production-orders/${id}`),
  updateProductionOrderStatus: (id: string, data: unknown) => api.patch(`/manufacturing/production-orders/${id}/status`, data),
  updateProductionCost: (id: string, data: { actual_material_cost: number }) => api.patch(`/manufacturing/production-orders/${id}/cost`, data),
  listQCRecords: (params?: Record<string, unknown>) => api.get("/manufacturing/qc-records", { params }),
  createQCRecord: (data: unknown) => api.post("/manufacturing/qc-records", data),
  logQC: (data: unknown) => api.post("/manufacturing/qc-records", data),
  listWarranty: (params?: Record<string, unknown>) => api.get("/manufacturing/warranty", { params }),
  createWarranty: (data: unknown) => api.post("/manufacturing/warranty", data),
  updateWarrantyStatus: (id: string, data: unknown) => api.patch(`/manufacturing/warranty/${id}/status`, data),
  listUnits: (params?: Record<string, unknown>) => api.get("/manufacturing/units", { params }),
  registerUnit: (data: unknown) => api.post("/manufacturing/units", data),
  updateUnitStatus: (id: string, data: unknown) => api.patch(`/manufacturing/units/${id}/status`, data),
  /** Relabel a unit: the typed serial is normalised and checked by the API (409 when another unit has it). */
  setUnitSerial: (id: string, serial_number: string) => api.put(`/manufacturing/units/${id}/serial`, { serial_number }),
  /** Tracking number and delivery date; a key sent as null clears it. */
  updateUnitDelivery: (id: string, data: { tracking_number?: string | null; delivery_date?: string | null }) =>
    api.patch(`/manufacturing/units/${id}/delivery`, data),
  /** What the printable label shows: serial, product, QR (data URI) and the label size settings. */
  getUnitLabel: (id: string) => api.get(`/manufacturing/units/${id}/label`),
};

export const partnerApi = {
  listAccounts: (params?: Record<string, unknown>) => api.get("/partner/accounts", { params }),
  createAccount: (data: unknown) => api.post("/partner/accounts", data),
  getAccount: (id: string) => api.get(`/partner/accounts/${id}`),
  invitePartner: (id: string) => api.post(`/partner/accounts/${id}/invite`),
  listDealRegs: (params?: Record<string, unknown>) => api.get("/partner/deal-registrations", { params }),
  createDealReg: (data: unknown) => api.post("/partner/deal-registrations", data),
  reviewDealReg: (id: string, data: unknown) => api.patch(`/partner/deal-registrations/${id}/review`, data),
  /** Close an approved registration: a win counts its value toward the partner's revenue. */
  closeDealReg: (id: string, data: { outcome: "won" | "lost"; final_value?: number; closed_on?: string }) =>
    api.patch(`/partner/deal-registrations/${id}/close`, data),
  listCommissions: (params?: Record<string, unknown>) => api.get("/partner/commissions", { params }),
  listResources: (params?: Record<string, unknown>) => api.get("/partner/resources", { params }),
  createResource: (data: unknown) => api.post("/partner/resources", data),
  updateResource: (id: string, data: unknown) => api.patch(`/partner/resources/${id}`, data),
  deletePartnerAccount: (id: string) => api.delete(`/partner/accounts/${id}`),
  deleteDealReg: (id: string) => api.delete(`/partner/deal-registrations/${id}`),
  deleteResource: (id: string) => api.delete(`/partner/resources/${id}`),
  /** The one list of tiers, partner types and account statuses (routers/partner/mod.rs). */
  options: () => api.get<PartnerOptions>("/partner/options"),
  /** Change a partner's tier, type or status; suspending or terminating also ends its portal logins. */
  updateAccount: (id: string, data: { tier?: string; partner_type?: string; status?: string }) =>
    api.patch(`/partner/accounts/${id}`, data),
  /** End every portal login of the partner, keeping the account. */
  revokeAccess: (id: string) => api.delete(`/partner/accounts/${id}/access`),
  /** Expire an approved registration now, or extend its protection to a later date. */
  setProtection: (id: string, data: { action: "expire" } | { action: "extend"; until: string }) =>
    api.patch(`/partner/deal-registrations/${id}/protection`, data),
  trainingProgress: (moduleId: string) => api.get(`/partner/training/modules/${moduleId}/progress`),
  resetAttempts: (progressId: string) => api.post(`/partner/training/progress/${progressId}/reset`),
  /** A claim's proof file (the API path the MDF list gives, with or without the /api/v1 prefix). */
  downloadProof: (path: string) => api.get<Blob>(path.replace(/^\/api\/v1/, ""), { responseType: "blob" }),
};

export interface PartnerOption { value: string; label: string }
export interface PartnerOptions { tiers: PartnerOption[]; partner_types: PartnerOption[]; statuses: PartnerOption[] }

export const vendorApi = {
  listVendors: (params?: Record<string, unknown>) => api.get("/vendor/vendors", { params }),
  createVendor: (data: unknown) => api.post("/vendor/vendors", data),
  deleteVendor: (id: string) => api.delete(`/vendor/vendors/${id}`),
  getVendor: (id: string) => api.get(`/vendor/vendors/${id}`),
  /** Edit a vendor, withholding tax included (vendor_mgmt:manage_vendors). */
  updateVendor: (id: string, data: unknown) => api.patch(`/vendor/vendors/${id}`, data),
  inviteVendor: (id: string) => api.post(`/vendor/vendors/${id}/invite`),
  listContractorRequests: (params?: Record<string, unknown>) => api.get("/vendor/contractor-requests", { params }),
  createContractorRequest: (data: unknown) => api.post("/vendor/contractor-requests", data),
  deleteContractorRequest: (id: string) => api.delete(`/vendor/contractor-requests/${id}`),
  approveContractorRequest: (id: string, data: { action: string; notes?: string }) => api.patch(`/vendor/contractor-requests/${id}/approve`, data),
  listWorkOrders: (params?: Record<string, unknown>) => api.get("/vendor/work-orders", { params }),
  createWorkOrder: (data: unknown) => api.post("/vendor/work-orders", data),
  deleteWorkOrder: (id: string) => api.delete(`/vendor/work-orders/${id}`),
  updateWorkOrderStatus: (id: string, data: unknown) => api.patch(`/vendor/work-orders/${id}/status`, data),
  listPOs: (params?: Record<string, unknown>) => api.get("/vendor/purchase-orders", { params }),
  createPO: (data: unknown) => api.post("/vendor/purchase-orders", data),
  deletePO: (id: string) => api.delete(`/vendor/purchase-orders/${id}`),
  listVendorInvoices: (params?: Record<string, unknown>) => api.get("/vendor/vendor-invoices", { params }),
  vendorInvoiceSummary: () => api.get("/vendor/vendor-invoices/summary"),
  createVendorInvoice: (data: unknown) => api.post("/vendor/vendor-invoices", data),
  deleteVendorInvoice: (id: string) => api.delete(`/vendor/vendor-invoices/${id}`),
  updateVendorInvoiceStatus: (id: string, data: unknown) => api.patch(`/vendor/vendor-invoices/${id}/status`, data),
  // Documents a vendor uploaded in the portal (vendor_mgmt:manage_vendors): list, verify, download.
  listVendorDocuments: (vendorId: string) => api.get(`/vendor/vendors/${vendorId}/documents`),
  verifyVendorDocument: (docId: string) => api.patch(`/vendor/documents/${docId}/verify`),
  downloadVendorDocument: (docId: string) => api.get(`/vendor/portal/documents/${docId}/download`, { responseType: "blob" }),
  // Timesheets vendors submit against work orders: listed with vendor_mgmt:view, reviewed by work order approvers.
  listTimesheets: (params?: Record<string, unknown>) => api.get("/vendor/timesheets", { params }),
  reviewTimesheet: (id: string, data: { status: "approved" | "rejected"; rejection_reason?: string }) =>
    api.patch(`/vendor/timesheets/${id}/review`, data),
};

/** Company email signature: admin-owned HTML template + each user's field values. */
export const emailSignatureApi = {
  getTemplate: () => api.get("/email/signature/template"),
  saveTemplate: (html: string, enabled: boolean) =>
    api.put("/email/signature/template", { html, enabled }),
  getMine: () => api.get("/email/signature/me"),
  saveMine: (fields: Record<string, string>, enabled: boolean) =>
    api.put("/email/signature/me", { fields, enabled }),
  /** The HTML the composer inserts into a new message or reply. */
  render: () => api.get("/email/signature/render"),
};

/** Senders whose remote images the logged-in user always shows (security triage 18). */
export const emailImageSendersApi = {
  list: () => api.get<{ senders: string[] }>("/email/image-senders"),
  add: (email: string) => api.post<{ email: string }>("/email/image-senders", { email }),
  remove: (email: string) => api.delete("/email/image-senders", { params: { email } }),
};

export const kbApi = {
  listCategories: () => api.get("/kb/categories"),
  createCategory: (data: unknown) => api.post("/kb/categories", data),
  listArticles: (params?: Record<string, unknown>) => api.get("/kb/articles", { params }),
  getArticle: (id: string) => api.get(`/kb/articles/${id}`),
  createArticle: (data: unknown) => api.post("/kb/articles", data),
  updateArticle: (id: string, data: unknown) => api.patch(`/kb/articles/${id}`, data),
  publishArticle: (id: string) => api.patch(`/kb/articles/${id}/publish`),
  deleteArticle: (id: string) => api.delete(`/kb/articles/${id}`),
  getRevisions: (id: string) => api.get(`/kb/articles/${id}/revisions`),
  importMarkdown: (file: File) => {
    const fd = new FormData();
    fd.append("file", file);
    return api.post("/kb/articles/import-markdown", fd, {
      headers: { "Content-Type": "multipart/form-data" },
    });
  },
  /** Upload a PDF against an article, then queue the AI conversion into Markdown. */
  importPdf: async (id: string, file: File) => {
    const up = await attachmentApi.upload("kb_article", id, file);
    return api.post(`/kb/articles/${id}/import-pdf`, { attachment_id: up.data.id });
  },
  /** Re-run the conversion on the PDF already attached (replaces the body). */
  reconvertPdf: (id: string) => api.post(`/kb/articles/${id}/import-pdf`, { force: true }),
  signArticle: (id: string, data?: { signature_url?: string }) =>
    api.post(`/kb/articles/${id}/sign`, data || {}),
  signArticleCEO: (id: string, data?: { signature_url?: string }) =>
    api.post(`/kb/articles/${id}/sign/owner`, data || {}),
  exportPdf: (id: string) =>
    api.get(`/kb/articles/${id}/pdf`, { responseType: "blob" }),
};

export const messagingApi = {
  listConversations: () => api.get("/messaging/conversations"),
  getMessages: (otherUserId: string, params?: Record<string, unknown>) =>
    api.get(`/messaging/messages/${otherUserId}`, { params }),
  sendMessage: (data: {
    receiver_id: string; content: string; message_type?: string;
    file_url?: string; file_name?: string; file_size?: number; mime_type?: string;
    reply_to_id?: string; mentions?: { type: string; id: string; name: string }[];
  }) => api.post("/messaging/send", data),
  onlineUsers: () => api.get("/messaging/online"),
  searchDm: (otherUserId: string, q: string) =>
    api.get(`/messaging/dm/${otherUserId}/search`, { params: { q } }),
  uploadFile: (formData: FormData) =>
    api.post("/messaging/upload", formData, { headers: { "Content-Type": "multipart/form-data" } }),
  toggleReaction: (messageId: string, emoji: string, source: string) =>
    api.post(`/messaging/messages/${messageId}/react`, { emoji }, { params: { source } }),
  getReactions: (messageId: string, source: string) =>
    api.get(`/messaging/messages/${messageId}/reactions`, { params: { source } }),
  getThread: (messageId: string, source: string) =>
    api.get(`/messaging/thread/${messageId}`, { params: { source } }),
  postThread: (messageId: string, source: string, data: {
    content: string; message_type?: string; file_url?: string; file_name?: string;
    file_size?: number; mime_type?: string; mentions?: { type: string; id: string; name: string }[];
    receiver_id?: string; channel_id?: string;
  }) => source === "dm"
    ? api.post("/messaging/send", { ...data, reply_to_id: messageId })
    : api.post(`/messaging/channels/${data.channel_id}/messages`, { ...data, reply_to_id: messageId }),
  mentionUsers: (q: string) => api.get("/messaging/mention/users", { params: { q } }),
  /** The messaging people directory: active internal users (id, name, avatar_url, department, position). */
  people: () => api.get("/messaging/people"),
  mentionEntities: (q: string, types: string) =>
    api.get("/messaging/mention/entities", { params: { q, types } }),
  // Formal memos
  createMemo: (data: { subject: string; memo_type: string; body: string; recipient_id?: string; channel_id?: string }) =>
    api.post("/messaging/memos", data),
  listMemos: (filter?: "all" | "sent" | "received") =>
    api.get("/messaging/memos", { params: { filter } }),
  getMemo: (id: string) => api.get(`/messaging/memos/${id}`),
  acknowledgeMemo: (id: string) => api.post(`/messaging/memos/${id}/acknowledge`),
  recallMemo: (id: string) => api.post(`/messaging/memos/${id}/recall`),
  downloadMemoPdf: (id: string) => api.get(`/messaging/memos/${id}/pdf`, { responseType: "blob" }),
};

export const channelApi = {
  list: () => api.get("/messaging/channels"),
  create: (data: { name: string; description?: string; type?: string }) =>
    api.post("/messaging/channels", data),
  get: (id: string) => api.get(`/messaging/channels/${id}`),
  update: (id: string, data: { name?: string; description?: string; is_archived?: boolean }) =>
    api.patch(`/messaging/channels/${id}`, data),
  join: (id: string) => api.post(`/messaging/channels/${id}/join`),
  leave: (id: string) => api.post(`/messaging/channels/${id}/leave`),
  del: (id: string) => api.delete(`/messaging/channels/${id}`),
  invite: (id: string, userId: string) => api.post(`/messaging/channels/${id}/invite`, { user_id: userId }),
  /** Channel owners only; an owner is never removed. */
  removeMember: (id: string, userId: string) => api.delete(`/messaging/channels/${id}/members/${userId}`),
  messages: (id: string, params?: Record<string, unknown>) =>
    api.get(`/messaging/channels/${id}/messages`, { params }),
  sendMessage: (id: string, data: {
    content: string; message_type?: string; file_url?: string; file_name?: string;
    file_size?: number; mime_type?: string; reply_to_id?: string;
    mentions?: { type: string; id: string; name: string }[];
  }) => api.post(`/messaging/channels/${id}/messages`, data),
  search: (id: string, q: string) =>
    api.get(`/messaging/channels/${id}/search`, { params: { q } }),
};

export const callsApi = {
  createRoom: (data: { name?: string; deal_id?: string; project_id?: string; max_participants?: number; invited_user_ids?: string[] }) =>
    api.post("/calls/rooms", data),
  inviteUsers: (roomId: string, userIds: string[]) =>
    api.post(`/calls/rooms/${roomId}/invite`, { user_ids: userIds }),
  getRoom: (roomId: string) => api.get(`/calls/rooms/${roomId}`),
  joinRoom: (roomId: string) => api.post(`/calls/rooms/${roomId}/join`),
  leaveRoom: (roomId: string) => api.post(`/calls/rooms/${roomId}/leave`),
  endRoom: (roomId: string) => api.post(`/calls/rooms/${roomId}/end`),
  kickParticipant: (roomId: string, peerId: string) => api.post(`/calls/rooms/${roomId}/kick/${peerId}`),
  // Waiting room (host)
  listWaiting: (roomId: string) => api.get(`/calls/rooms/${roomId}/waiting`),
  admitGuest: (roomId: string, waitingId: string) => api.post(`/calls/rooms/${roomId}/waiting/${waitingId}/admit`),
  rejectGuest: (roomId: string, waitingId: string) => api.post(`/calls/rooms/${roomId}/waiting/${waitingId}/reject`),
  // Recording
  uploadRecording: (roomId: string, blob: Blob) => {
    const fd = new FormData();
    fd.append("file", blob, "recording.webm");
    return api.post(`/calls/rooms/${roomId}/recording`, fd, { headers: { "Content-Type": "multipart/form-data" } });
  },
  getRecording: (roomId: string) => api.get(`/calls/rooms/${roomId}/recording`),
};

// Guest calls API: no auth header, uses plain fetch (no axios interceptors)
async function _guestFetch(url: string, init?: RequestInit) {
  const r = await fetch(url, init);
  if (!r.ok) throw new Error(`Request failed: ${r.status} ${r.statusText}`);
  return r.json();
}

export const guestCallsApi = {
  getRoomPublic: (roomId: string) =>
    _guestFetch(`/api/v1/calls/rooms/${roomId}/public`),
  enterWaiting: (roomId: string, data: { name: string; email?: string }) =>
    _guestFetch(`/api/v1/calls/rooms/${roomId}/guest-wait`, {
      method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(data),
    }),
  pollStatus: (roomId: string, waitingId: string) =>
    _guestFetch(`/api/v1/calls/rooms/${roomId}/guest-status/${waitingId}`),
};

export const emailApi = {
  listAccounts: () => api.get("/email/accounts"),
  addAccount: (data: unknown) => api.post("/email/accounts", data),
  updateAccount: (id: string, data: unknown) => api.patch(`/email/accounts/${id}`, data),
  deleteAccount: (id: string) => api.delete(`/email/accounts/${id}`),
  getFolders: (id: string) => api.get(`/email/accounts/${id}/folders`),
  getInbox: (accountId: string, params?: Record<string, unknown>) =>
    api.get(`/email/inbox`, { params: { account_id: accountId, ...params } }),
  getMessage: (msgId: string) => api.get(`/email/messages/${msgId}`),
  downloadAttachment: (msgId: string, attId: string) =>
    api.get(`/email/messages/${msgId}/attachments/${attId}/download`, { responseType: "blob" }),
  sendEmail: (data: unknown) => api.post("/email/send", data),
  syncAccount: (accountId: string, folder = "INBOX") =>
    api.post(`/email/accounts/${accountId}/sync`, null, { params: { folder } }),
  syncOlder: (accountId: string, folder = "INBOX") =>
    api.post(`/email/accounts/${accountId}/sync-older`, null, { params: { folder } }),
  starMessage: (msgId: string) => api.patch(`/email/messages/${msgId}/star`),
  markRead: (msgId: string, is_read: boolean) => api.patch(`/email/messages/${msgId}/read`, { is_read }),
  /** To the server's Trash; from Trash, gone for good. */
  deleteMessage: (msgId: string) => api.delete(`/email/messages/${msgId}`),
  countInbox: (accountId: string, params: { folder: string; unread_only?: boolean }) =>
    api.get("/email/inbox/count", { params: { account_id: accountId, ...params } }),
  /** One action on many of your own emails: the ticked `ids`, or everything in a folder. */
  bulk: (data: {
    action: "delete" | "read" | "unread" | "star" | "unstar";
    ids?: string[];
    filter?: { account_id: string; folder: string; unread_only?: boolean };
  }) => api.post("/email/messages/bulk", data, { timeout: 120000 }),
  linkMessage: (msgId: string, contact_id: string | null, apply_to_sender = true) =>
    api.patch(`/email/messages/${msgId}/link`, { contact_id, apply_to_sender }),
};

export const whatsappApi = {
  getStatus: () => api.get("/whatsapp/status"),
  getConversations: (limit = 50) => api.get("/whatsapp/conversations", { params: { limit } }),
  /** A page of messages, oldest first. `before` (unix seconds) pages back through history.
   *  Headers: `x-has-more` (older messages known) and `x-history-end` (ask the phone). */
  getMessages: (chatId: string, limit = 50, before?: number) =>
    api.get("/whatsapp/messages", { params: { chat_id: chatId, limit, ...(before ? { before } : {}) } }),
  /** Ask the linked phone for this chat's older history; it arrives in the background. */
  syncHistory: (chatId: string) =>
    api.post("/whatsapp/sync-history", null, { params: { chat_id: chatId }, timeout: 40000 }),
  send: (data: { chat_id: string; message: string; quoted_message_id?: string }) =>
    api.post("/whatsapp/send", data),
  /** The sign-off the server adds to this user's messages (and whether they may write their own). */
  getSignature: () => api.get("/whatsapp/signature"),
  /** Send an ERP attachment the user may read (by id, never a URL); signed like text sends. */
  sendDocument: (data: { chat_id: string; attachment_id: string; filename?: string; caption?: string }) =>
    api.post("/whatsapp/send-document", data),
  sendMedia: (data: {
    chat_id: string; data: string; mimetype: string;
    filename?: string; caption?: string; quoted_message_id?: string;
  }) => api.post("/whatsapp/send-media", data),
  forwardMessage: (data: { message_id: string; to_chat_id: string }) =>
    api.post("/whatsapp/forward", data),
  markRead: (chatId: string) =>
    api.post("/whatsapp/mark-read", null, { params: { chat_id: chatId } }),
  /** Hide (Owner + system admins only) or mark read the ticked chats. */
  bulk: (data: { action: "hide" | "read"; chats: { id: string; through?: number }[] }) =>
    api.post("/whatsapp/conversations/bulk", data, { timeout: 120000 }),
  /** "Delete chat": hidden in the ERP until a newer message arrives; untouched on WhatsApp. */
  hideChat: (chatId: string, through: number) =>
    api.delete(`/whatsapp/conversations/${encodeURIComponent(chatId)}`, { params: { through } }),
  /** One message's attachment bytes. The service retries until WhatsApp resolves it. */
  getMedia: (messageId: string) =>
    api.get("/whatsapp/media", {
      params: { message_id: messageId }, responseType: "blob", timeout: 90000,
    }),
  /** Transcript download. Reads live from the WhatsApp service, so allow it time. */
  exportConversation: (params: {
    chat_id: string; format: "pdf" | "txt";
    date_from?: string; date_to?: string; chat_name?: string;
  }) => api.get("/whatsapp/export", { params, responseType: "blob", timeout: 240000 }),
};

export const rndApi = {
  // Projects
  listProjects: (params?: Record<string, unknown>) => api.get("/rnd/projects", { params }),
  createProject: (data: unknown) => api.post("/rnd/projects", data),
  getProject: (id: string) => api.get(`/rnd/projects/${id}`),
  updateProject: (id: string, data: unknown) => api.patch(`/rnd/projects/${id}`, data),
  deleteProject: (id: string) => api.delete(`/rnd/projects/${id}`),
  // Sprints
  listSprints: (projectId: string, params?: Record<string, unknown>) =>
    api.get(`/rnd/projects/${projectId}/sprints`, { params }),
  createSprint: (projectId: string, data: unknown) =>
    api.post(`/rnd/projects/${projectId}/sprints`, data),
  updateSprint: (id: string, data: unknown) => api.patch(`/rnd/sprints/${id}`, data),
  startSprint: (id: string) => api.patch(`/rnd/sprints/${id}/start`),
  completeSprint: (id: string, data: unknown) => api.patch(`/rnd/sprints/${id}/complete`, data),
  // Backlog
  listBacklog: (params?: Record<string, unknown>) => api.get("/rnd/backlog", { params }),
  createBacklogItem: (data: unknown) => api.post("/rnd/backlog", data),
  getBacklogItem: (id: string) => api.get(`/rnd/backlog/${id}`),
  updateBacklogItem: (id: string, data: unknown) => api.patch(`/rnd/backlog/${id}`, data),
  deleteBacklogItem: (id: string) => api.delete(`/rnd/backlog/${id}`),
  assignToSprint: (id: string, sprintId: string | null) =>
    api.patch(`/rnd/backlog/${id}/assign-sprint`, { sprint_id: sprintId }),
  // Releases
  listReleases: (projectId: string) => api.get(`/rnd/projects/${projectId}/releases`),
  createRelease: (projectId: string, data: unknown) =>
    api.post(`/rnd/projects/${projectId}/releases`, data),
  updateRelease: (id: string, data: unknown) => api.patch(`/rnd/releases/${id}`, data),
  publishRelease: (id: string) => api.patch(`/rnd/releases/${id}/publish`),
};

export const gitlabApi = {
  // Configs
  listConfigs: () => api.get("/rnd/gitlab/configs"),
  createConfig: (data: unknown) => api.post("/rnd/gitlab/configs", data),
  updateConfig: (id: string, data: unknown) => api.patch(`/rnd/gitlab/configs/${id}`, data),
  testConfig: (id: string) => api.post(`/rnd/gitlab/configs/${id}/test`),
  /** 17 Nov 2026: the GitLab projects the server's token can see (search narrows), each with `linked`. */
  listProjects: (id: string, search?: string) => api.get(`/rnd/gitlab/configs/${id}/projects`, { params: search ? { search } : undefined }),
  // Repos
  listRepos: (params?: Record<string, unknown>) => api.get("/rnd/gitlab/repos", { params }),
  addRepo: (data: unknown) => api.post("/rnd/gitlab/repos", data),
  updateRepo: (id: string, data: unknown) => api.patch(`/rnd/gitlab/repos/${id}`, data),
  syncPipelines: (repoId: string) => api.post(`/rnd/gitlab/repos/${repoId}/sync-pipelines`),
  /** 17 Nov 2026: the R&D backlog <-> GitLab issue sync */
  syncIssues: (repoId: string) => api.post(`/rnd/gitlab/repos/${repoId}/sync-issues`),
  listPeople: (configId: string) => api.get(`/rnd/gitlab/configs/${configId}/people`),
  setPerson: (configId: string, userId: string, gitlabUsername: string) =>
    api.put(`/rnd/gitlab/configs/${configId}/people/${userId}`, { gitlab_username: gitlabUsername }),
  // Pipelines & Events
  listPipelines: (params?: Record<string, unknown>) => api.get("/rnd/gitlab/pipelines", { params }),
  listEvents: (params?: Record<string, unknown>) => api.get("/rnd/gitlab/events", { params }),
};

export const marketingApi = {
  listCampaigns: (params?: Record<string, unknown>) => api.get("/marketing/campaigns", { params }),
  createCampaign: (data: unknown) => api.post("/marketing/campaigns", data),
  getCampaign: (id: string) => api.get(`/marketing/campaigns/${id}`),
  updateCampaign: (id: string, data: unknown) => api.patch(`/marketing/campaigns/${id}`, data),
  deleteCampaign: (id: string) => api.delete(`/marketing/campaigns/${id}`),
  updateStatus: (id: string, status: string) => api.patch(`/marketing/campaigns/${id}/status`, { status }),
  listContent: (params?: Record<string, unknown>) => api.get("/marketing/content", { params }),
  createContent: (data: unknown) => api.post("/marketing/content", data),
  deleteContent: (id: string) => api.delete(`/marketing/content/${id}`),
  updateContentStatus: (id: string, status: string) => api.patch(`/marketing/content/${id}/status`, { status }),
};

export const portalPartnerApi = {
  getMyAccount: () => api.get("/partner/portal/account"),
  listMyDeals: (params?: Record<string, unknown>) => api.get("/partner/portal/deals", { params }),
  createDeal: (data: unknown) => api.post("/partner/portal/deals", data),
  listMyCommissions: (params?: Record<string, unknown>) => api.get("/partner/portal/commissions", { params }),
  listResources: () => api.get("/partner/portal/resources"),
};

export const portalVendorApi = {
  getMyProfile: () => api.get("/vendor/portal/profile"),
  listMyWorkOrders: (params?: Record<string, unknown>) => api.get("/vendor/portal/work-orders", { params }),
  listMyInvoices: (params?: Record<string, unknown>) => api.get("/vendor/portal/invoices", { params }),
  submitInvoice: (data: unknown) => api.post("/vendor/portal/invoices", data),
  listMyTimesheets: (params?: Record<string, unknown>) => api.get("/vendor/portal/timesheets", { params }),
  submitTimesheet: (data: unknown) => api.post("/vendor/portal/timesheets", data),
  listMyDocuments: () => api.get("/vendor/portal/documents"),
  uploadDocument: (formData: FormData) =>
    api.post("/vendor/portal/documents", formData, { headers: { "Content-Type": "multipart/form-data" } }),
  deleteDocument: (id: string) => api.delete(`/vendor/portal/documents/${id}`),
  downloadDocument: (id: string) => api.get(`/vendor/portal/documents/${id}/download`, { responseType: "blob" }),
};

export interface CampaignSender { id: string; name: string; email: string; has_signature: boolean; is_me: boolean }
export interface CampaignPreview {
  subject: string;
  html: string;
  signature_included: boolean;
  recipient: { id: string | null; email: string; first_name: string; full_name: string; company: string; sample: boolean };
}
/** POST /hr/attendance/challenge: the active challenge, plus the passive flash when it is on. */
export interface ActiveChallenge {
  nonce: string;
  action: "blink" | "turn_left" | "turn_right";
  expires_in: number;
  step_timeouts_ms: { find: number; action: number; return: number };
}
export interface FaceChallenge extends ActiveChallenge {
  passive?: { enabled: boolean; colors: [number, number, number][]; flash_ms: number; settle_ms: number } | null;
}
/** A passive clock that needs the active step: a fresh active nonce (the passive one is used up). */
export interface ActiveRequired {
  active_required: true;
  reason: "flash_inconclusive" | "flash_failed" | "spoof" | "no_face";
  challenge: ActiveChallenge;
}
export type PassiveClockResponse =
  | ActiveRequired
  | { active_required?: false; status: "success" | "pending_override"; message: string; [k: string]: unknown };

/** POST /hr/attendance/face-attempts and /hr/kiosk/face-attempts. */
export interface FaceAttemptReport {
  action: "blink" | "turn_left" | "turn_right";
  outcome: "done" | "timeout" | "aborted" | "passive_inconclusive" | "passive_failed";
  step_reached: "find_face" | "hold_still" | "do_action" | "return" | "done";
  duration_ms: number;
  avg_tick_ms: number | null;
  backend: "webgl" | "cpu" | "unknown";
  /** Kiosk only: the employee the tablet recognised. */
  employee_id?: string;
  /** 0167: what went wrong on the device (video_resumed, stall, webgl_lost, cpu_fallback). */
  note?: string;
  /** 0167: the active check's first tick (ms) and number of ticks. */
  first_tick_ms?: number;
  ticks?: number;
  /** 0167, kiosk passive_* attempts: why the passive check did not pass and its numbers. */
  reason?: "flash_inconclusive" | "flash_failed" | "spoof" | "no_face";
  corr?: number | null;
  delta?: number | null;
  spoof_real?: number | null;
  lag?: 0 | 1;
}

/** GET /hr/attendance/face-attempts/summary. */
export interface FaceAttemptsSummary {
  days: number;
  total: number;
  timeouts: number;
  timeout_rate: number;
  avg_tick_ms: number | null;
  by_outcome: Record<string, number>;
  by_mode: Record<string, number>;
  by_step: Record<string, number>;
  by_mode_outcome: Record<string, Record<string, number>>;
  /** 0167: the passive checks: why the fallbacks did not pass, how many passed, median numbers. */
  passive?: {
    passed: number;
    not_passed: number;
    reasons: Record<string, number>;
    median_corr: number | null;
    median_delta: number | null;
    median_spoof_real: number | null;
  };
}

export interface KioskDevice {
  id: string; name: string; office_location_id: string; office_location: string | null;
  token_prefix: string; created_at: string | null; last_seen_at: string | null; revoked_at: string | null;
}
export const emailMarketingApi = {
  /** Email marketing is limited to the marketing team and the Owner. */
  access: () => api.get<{ allowed: boolean }>("/marketing/email/access"),
  // Mailing Lists
  listLists: (params?: Record<string, unknown>) => api.get("/marketing/email/lists", { params }),
  createList: (data: { name: string; description?: string }) => api.post("/marketing/email/lists", data),
  getList: (id: string) => api.get(`/marketing/email/lists/${id}`),
  updateList: (id: string, data: unknown) => api.patch(`/marketing/email/lists/${id}`, data),
  deleteList: (id: string) => api.delete(`/marketing/email/lists/${id}`),

  // Contacts
  listContacts: (listId: string, params?: Record<string, unknown>) =>
    api.get(`/marketing/email/lists/${listId}/contacts`, { params }),
  addContact: (listId: string, data: unknown) =>
    api.post(`/marketing/email/lists/${listId}/contacts`, data),
  updateContact: (listId: string, contactId: string, data: unknown) =>
    api.patch(`/marketing/email/lists/${listId}/contacts/${contactId}`, data),
  deleteContact: (listId: string, contactId: string) =>
    api.delete(`/marketing/email/lists/${listId}/contacts/${contactId}`),

  // Email Campaigns
  listCampaigns: (params?: Record<string, unknown>) =>
    api.get("/marketing/email/campaigns", { params }),
  createCampaign: (data: unknown) => api.post("/marketing/email/campaigns", data),
  /** Internal users a campaign can come from, with whether their email signature is ready. */
  senders: () => api.get<CampaignSender[]>("/marketing/email/senders"),
  /** The whole email exactly as sent (body + signature + footer) for one recipient, never sent. */
  preview: (data: {
    subject: string; blocks: unknown[]; list_ids: string[]; sender_user_id?: string | null;
    include_signature: boolean; contact_id?: string | null;
  }) => api.post<CampaignPreview>("/marketing/email/preview", data),
  getCampaign: (id: string) => api.get(`/marketing/email/campaigns/${id}`),
  updateCampaign: (id: string, data: unknown) => api.patch(`/marketing/email/campaigns/${id}`, data),
  deleteCampaign: (id: string) => api.delete(`/marketing/email/campaigns/${id}`),
  dispatch: (id: string, data: { send_now: boolean; scheduled_at?: string }) =>
    api.post(`/marketing/email/campaigns/${id}/dispatch`, data),
  getReport: (id: string, params?: Record<string, unknown>) =>
    api.get(`/marketing/email/campaigns/${id}/report`, { params }),
};

export const signingApi = {
  listDocuments: (params?: Record<string, unknown>) => api.get("/sign/documents", { params }),
  createDocument: (formData: FormData) => api.post("/sign/documents", formData, { headers: { "Content-Type": "multipart/form-data" } }),
  uploadPdf: (id: string, formData: FormData) => api.post(`/sign/documents/${id}/upload-pdf`, formData, { headers: { "Content-Type": "multipart/form-data" } }),
  getDocument: (id: string) => api.get(`/sign/documents/${id}`),
  cancelDocument: (id: string) => api.delete(`/sign/documents/${id}`),
  addSigner: (id: string, data: unknown) => api.post(`/sign/documents/${id}/signers`, data),
  removeSigner: (docId: string, signerId: string) => api.delete(`/sign/documents/${docId}/signers/${signerId}`),
  addField: (id: string, data: unknown) => api.post(`/sign/documents/${id}/fields`, data),
  sendDocument: (id: string) => api.post(`/sign/documents/${id}/send`),
  remindSigner: (docId: string, signerId: string) => api.post(`/sign/documents/${docId}/remind`, {}, { params: { signer_id: signerId } }),
  // Public: use fetch directly
  viewPublic: (token: string) => fetch(`/api/v1/sign/public/${token}`).then(r => r.json()),
  submitSignature: (token: string, data: unknown) => fetch(`/api/v1/sign/public/${token}/sign`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(data) }).then(r => r.json()),
  declineSigning: (token: string, reason: string) => fetch(`/api/v1/sign/public/${token}/decline?reason=${encodeURIComponent(reason)}`, { method: "POST" }).then(r => r.json()),
  verifyDocument: (docId: string) => fetch(`/api/v1/sign/verify/${docId}`).then(r => r.json()),
};

export const helpdeskApi = {
  // Public (no auth: use fetch directly for these)
  submitTicket: (data: unknown) => fetch("/api/v1/helpdesk/public/tickets", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(data) }).then(r => r.json()),
  trackTicket: (token: string) => fetch(`/api/v1/helpdesk/public/tickets/${token}`).then(r => r.json()),
  // Internal
  getSummary: () => api.get("/helpdesk/summary"),
  listTickets: (params?: Record<string, unknown>) => api.get("/helpdesk/tickets", { params }),
  getTicket: (id: string) => api.get(`/helpdesk/tickets/${id}`),
  updateTicket: (id: string, data: unknown) => api.patch(`/helpdesk/tickets/${id}`, data),
  deleteTicket: (id: string) => api.delete(`/helpdesk/tickets/${id}`),
  addMessage: (ticketId: string, data: unknown) => api.post(`/helpdesk/tickets/${ticketId}/messages`, data),
  listTeams: () => api.get("/helpdesk/teams"),
  createTeam: (data: unknown) => api.post("/helpdesk/teams", data),
  updateTeam: (id: string, data: unknown) => api.patch(`/helpdesk/teams/${id}`, data),
  deleteTeam: (id: string) => api.delete(`/helpdesk/teams/${id}`),
  listCategories: (params?: Record<string, unknown>) => api.get("/helpdesk/categories", { params }),
  createCategory: (data: unknown) => api.post("/helpdesk/categories", data),
  deleteCategory: (id: string) => api.delete(`/helpdesk/categories/${id}`),
  listSLAPolicies: (params?: Record<string, unknown>) => api.get("/helpdesk/sla-policies", { params }),
  createSLAPolicy: (data: unknown) => api.post("/helpdesk/sla-policies", data),
  deleteSLAPolicy: (id: string) => api.delete(`/helpdesk/sla-policies/${id}`),
};

export const maintenanceApi = {
  listEquipment: (params?: Record<string, unknown>) => api.get("/maintenance/equipment", { params }),
  createEquipment: (data: unknown) => api.post("/maintenance/equipment", data),
  updateEquipment: (id: string, data: unknown) => api.patch(`/maintenance/equipment/${id}`, data),
  deleteEquipment: (id: string) => api.delete(`/maintenance/equipment/${id}`),
  listRequests: (params?: Record<string, unknown>) => api.get("/maintenance/requests", { params }),
  createRequest: (data: unknown) => api.post("/maintenance/requests", data),
  reviewRequest: (id: string, data: unknown) => api.patch(`/maintenance/requests/${id}/review`, data),
  deleteRequest: (id: string) => api.delete(`/maintenance/requests/${id}`),
  listWorkOrders: (params?: Record<string, unknown>) => api.get("/maintenance/work-orders", { params }),
  createWorkOrder: (data: unknown) => api.post("/maintenance/work-orders", data),
  updateWOStatus: (id: string, data: unknown) => api.patch(`/maintenance/work-orders/${id}/status`, data),
  deleteWorkOrder: (id: string) => api.delete(`/maintenance/work-orders/${id}`),
  listPlans: (params?: Record<string, unknown>) => api.get("/maintenance/plans", { params }),
  createPlan: (data: unknown) => api.post("/maintenance/plans", data),
  updatePlan: (id: string, data: unknown) => api.patch(`/maintenance/plans/${id}`, data),
  completePlan: (id: string) => api.post(`/maintenance/plans/${id}/complete`),
  deletePlan: (id: string) => api.delete(`/maintenance/plans/${id}`),
};

export const fleetApi = {
  getSummary: () => api.get("/fleet/summary"),
  listVehicles: (params?: Record<string, unknown>) => api.get("/fleet/vehicles", { params }),
  createVehicle: (data: unknown) => api.post("/fleet/vehicles", data),
  updateVehicle: (id: string, data: unknown) => api.patch(`/fleet/vehicles/${id}`, data),
  deleteVehicle: (id: string) => api.delete(`/fleet/vehicles/${id}`),
  getVehicleAssignments: (id: string) => api.get(`/fleet/vehicles/${id}/assignments`),
  listAssignments: (params?: Record<string, unknown>) => api.get("/fleet/assignments", { params }),
  createAssignment: (data: unknown) => api.post("/fleet/assignments", data),
  endAssignment: (id: string) => api.delete(`/fleet/assignments/${id}`),
  listFuelLogs: (params?: Record<string, unknown>) => api.get("/fleet/fuel-logs", { params }),
  createFuelLog: (data: unknown) => api.post("/fleet/fuel-logs", data),
  deleteFuelLog: (id: string) => api.delete(`/fleet/fuel-logs/${id}`),
  listServiceRecords: (params?: Record<string, unknown>) => api.get("/fleet/service-records", { params }),
  createServiceRecord: (data: unknown) => api.post("/fleet/service-records", data),
  deleteServiceRecord: (id: string) => api.delete(`/fleet/service-records/${id}`),
  listOdometerLogs: (params?: Record<string, unknown>) => api.get("/fleet/odometer-logs", { params }),
  createOdometerLog: (data: unknown) => api.post("/fleet/odometer-logs", data),
};

export const p2pApi = {
  // RFQs
  listRFQs: (params?: Record<string, unknown>) => api.get("/vendor/rfqs", { params }),
  createRFQ: (data: unknown) => api.post("/vendor/rfqs", data),
  getRFQ: (id: string) => api.get(`/vendor/rfqs/${id}`),
  deleteRFQ: (id: string) => api.delete(`/vendor/rfqs/${id}`),
  addRFQLine: (rfqId: string, data: unknown) => api.post(`/vendor/rfqs/${rfqId}/lines`, data),
  deleteRFQLine: (rfqId: string, lineId: string) => api.delete(`/vendor/rfqs/${rfqId}/lines/${lineId}`),
  inviteVendor: (rfqId: string, data: { vendor_id: string }) => api.post(`/vendor/rfqs/${rfqId}/invite`, data),
  submitResponse: (rfqId: string, data: unknown) => api.post(`/vendor/rfqs/${rfqId}/respond`, data),
  closeRFQ: (rfqId: string, data: unknown) => api.post(`/vendor/rfqs/${rfqId}/close`, data),
  // PO approval
  approvePO: (poId: string, data: unknown) => api.post(`/vendor/purchase-orders/${poId}/approve`, data),
  // Goods Receipts
  listReceipts: (params?: Record<string, unknown>) => api.get("/vendor/goods-receipts", { params }),
  createReceipt: (data: unknown) => api.post("/vendor/goods-receipts", data),
  // 3-way matching
  getMatching: (poId: string) => api.get(`/vendor/purchase-orders/${poId}/matching`),
};

export const inventoryApi = {
  listWarehouses: () => api.get("/inventory/warehouses"),
  createWarehouse: (data: unknown) => api.post("/inventory/warehouses", data),
  updateWarehouse: (id: string, data: unknown) => api.patch(`/inventory/warehouses/${id}`, data),
  deleteWarehouse: (id: string) => api.delete(`/inventory/warehouses/${id}`),
  getWarehouseLocations: (warehouseId: string) => api.get(`/inventory/warehouses/${warehouseId}/locations`),
  listLocations: (params?: Record<string, unknown>) => api.get("/inventory/locations", { params }),
  createLocation: (data: unknown) => api.post("/inventory/locations", data),
  updateLocation: (id: string, data: unknown) => api.patch(`/inventory/locations/${id}`, data),
  deleteLocation: (id: string) => api.delete(`/inventory/locations/${id}`),
  listMoves: (params?: Record<string, unknown>) => api.get("/inventory/stock/moves", { params }),
  recordMove: (data: unknown) => api.post("/inventory/stock/moves", data),
  transfer: (data: unknown) => api.post("/inventory/stock/transfer", data),
  getLedger: (sku: string, params?: Record<string, unknown>) => api.get(`/inventory/stock/ledger/${sku}`, { params }),
};

export const budgetApi = {
  listBudgets: (params?: Record<string, unknown>) => api.get("/finance/budgets", { params }),
  createBudget: (data: unknown) => api.post("/finance/budgets", data),
  getBudget: (id: string) => api.get(`/finance/budgets/${id}`),
  updateBudget: (id: string, data: unknown) => api.patch(`/finance/budgets/${id}`, data),
  approveBudget: (id: string) => api.post(`/finance/budgets/${id}/approve`),
  deleteBudget: (id: string) => api.delete(`/finance/budgets/${id}`),
  addLine: (budgetId: string, data: unknown) => api.post(`/finance/budgets/${budgetId}/lines`, data),
  updateLine: (budgetId: string, lineId: string, data: unknown) => api.patch(`/finance/budgets/${budgetId}/lines/${lineId}`, data),
  deleteLine: (budgetId: string, lineId: string) => api.delete(`/finance/budgets/${budgetId}/lines/${lineId}`),
  vsActual: (budgetId: string) => api.get(`/finance/budgets/${budgetId}/vs-actual`),
};

export const assetsApi = {
  // Categories
  listCategories: () => api.get("/finance/categories"),
  createCategory: (data: unknown) => api.post("/finance/categories", data),
  updateCategory: (id: string, data: unknown) => api.patch(`/finance/categories/${id}`, data),
  deleteCategory: (id: string) => api.delete(`/finance/categories/${id}`),
  // Assets
  listAssets: (params?: Record<string, unknown>) => api.get("/finance/assets", { params }),
  createAsset: (data: unknown) => api.post("/finance/assets", data),
  getAsset: (id: string) => api.get(`/finance/assets/${id}`),
  updateAsset: (id: string, data: unknown) => api.patch(`/finance/assets/${id}`, data),
  deleteAsset: (id: string) => api.delete(`/finance/assets/${id}`),
  // Depreciation
  getSchedule: (id: string) => api.get(`/finance/assets/${id}/depreciation-schedule`),
  generateSchedule: (id: string) => api.post(`/finance/assets/${id}/generate-schedule`),
  postDepreciation: (id: string) => api.post(`/finance/assets/${id}/post-depreciation`),
  // Disposal
  dispose: (id: string, data: unknown) => api.post(`/finance/assets/${id}/dispose`, data),
};

export const orgChartApi = {
  get: () => api.get("/hr/org-chart"),
  addCoordLink: (data: { from_department: string; to_department: string; notes?: string }) =>
    api.post("/hr/coordinative-links", data),
  deleteCoordLink: (id: string) => api.delete(`/hr/coordinative-links/${id}`),
};

export const attachmentApi = {
  upload: (entityType: string, entityId: string, file: File) => {
    const fd = new FormData();
    fd.append("file", file);
    fd.append("entity_type", entityType);
    fd.append("entity_id", entityId);
    return api.post("/attachments/upload", fd);
  },
  list: (entityType: string, entityId: string) =>
    api.get(`/attachments?entity_type=${entityType}&entity_id=${entityId}`),
  delete: (id: string) => api.delete(`/attachments/${id}`),
  // Authenticated streaming proxy: returns the file bytes as a Blob (works regardless of
  // whether MinIO is publicly reachable).
  download: (id: string) =>
    api.get(`/attachments/${id}/download`, { responseType: "blob" }),
};

// Shared PM collaboration: polymorphic comments + activity feed for issues.
export const collabApi = {
  listComments: (entityType: string, entityId: string) =>
    api.get("/collab/comments", { params: { entity_type: entityType, entity_id: entityId } }),
  addComment: (entityType: string, entityId: string, body: string, mentions: string[] = []) =>
    api.post("/collab/comments", { entity_type: entityType, entity_id: entityId, body, mentions }),
  editComment: (id: string, body: string) => api.patch(`/collab/comments/${id}`, { body }),
  deleteComment: (id: string) => api.delete(`/collab/comments/${id}`),
  listActivity: (entityType: string, entityId: string) =>
    api.get("/collab/activity", { params: { entity_type: entityType, entity_id: entityId } }),
  listEpics: (scopeType: string, scopeId: string) =>
    api.get("/collab/epics", { params: { scope_type: scopeType, scope_id: scopeId } }),
  createEpic: (data: { scope_type: string; scope_id: string; title: string; color?: string }) =>
    api.post("/collab/epics", data),
  updateEpic: (id: string, data: Record<string, unknown>) => api.patch(`/collab/epics/${id}`, data),
  deleteEpic: (id: string) => api.delete(`/collab/epics/${id}`),
  listColumns: (scopeType: string, scopeId: string) =>
    api.get("/collab/columns", { params: { scope_type: scopeType, scope_id: scopeId } }),
  createColumn: (data: { scope_type: string; scope_id: string; key: string; label: string; color?: string; sort_order?: number }) =>
    api.post("/collab/columns", data),
  seedColumns: (data: { scope_type: string; scope_id: string; columns: { key: string; label: string; color: string }[] }) =>
    api.post("/collab/columns/seed", data),
  updateColumn: (id: string, data: Record<string, unknown>) => api.patch(`/collab/columns/${id}`, data),
  deleteColumn: (id: string) => api.delete(`/collab/columns/${id}`),
};

// Shared product-line catalog (replaces hardcoded R&D PRODUCT_META).
export const catalogApi = {
  list: (params?: Record<string, unknown>) => api.get("/catalog/product-lines", { params }),
  create: (data: unknown) => api.post("/catalog/product-lines", data),
  update: (id: string, data: unknown) => api.patch(`/catalog/product-lines/${id}`, data),
  delete: (id: string) => api.delete(`/catalog/product-lines/${id}`),
};

// Approval flow (SOP approval policies), inbox, per-document trail, decisions, policy admin.
export const approvalsApi = {
  count: () => api.get("/approvals/count"),
  list: (params?: { scope?: string; status?: string; entity_type?: string; limit?: number }) =>
    api.get("/approvals", { params }),
  get: (id: string) => api.get(`/approvals/${id}`),
  forEntity: (entityType: string, entityId: string) => api.get(`/approvals/entity/${entityType}/${entityId}`),
  submit: (data: { entity_type: string; entity_id: string; flags?: string[] }) => api.post("/approvals/submit", data),
  approve: (id: string, comment?: string) => api.post(`/approvals/${id}/approve`, { comment }),
  reject: (id: string, comment: string) => api.post(`/approvals/${id}/reject`, { comment }),
  cancel: (id: string, reason?: string) => api.post(`/approvals/${id}/cancel`, { reason }),
  rules: () => api.get("/approvals/rules"),
  listPolicies: () => api.get("/approvals/policies"),
  updatePolicy: (id: string, data: unknown) => api.patch(`/approvals/policies/${id}`, data),
  listAllowances: () => api.get("/approvals/discount-allowances"),
  replaceAllowances: (rows: unknown[]) => api.put("/approvals/discount-allowances", rows),
};

// QR document verification: verified PDFs issued for approved documents.
export const verificationApi = {
  issue: (entityType: string, entityId: string) =>
    api.post("/verify/issue", { entity_type: entityType, entity_id: entityId }),
  forEntity: (entityType: string, entityId: string) => api.get(`/verify/entity/${entityType}/${entityId}`),
  downloadPdf: (id: string) => api.get(`/verify/${id}/pdf`, { responseType: "blob" }),
  revoke: (id: string, reason: string) => api.post(`/verify/${id}/revoke`, { reason }),
  views: (id: string) => api.get(`/verify/${id}/views`),
};

// Send an approved document (its QR-verified PDF) to the client / vendor by email or WhatsApp.
export interface SendDefaults { name: string; email: string; whatsapp: string; approved: boolean; number: string }
export const documentsApi = {
  sendDefaults: (entityType: string, entityId: string) =>
    api.get<SendDefaults>(`/documents/${entityType}/${entityId}/send-defaults`),
  /** Submits for approval when needed (409 while it waits); returns the recipient once approved. */
  prepareSend: (entityType: string, entityId: string) =>
    api.post<SendDefaults>(`/documents/${entityType}/${entityId}/prepare-send`),
  send: (entityType: string, entityId: string, data: {
    channel: "email" | "whatsapp"; to: string[]; cc?: string[]; bcc?: string[]; to_name?: string; message?: string;
    /** The CRM inbox it goes from; null is the deal's inbox. */
    from_inbox?: string | null;
    /** Send again although it just went to the same recipient. */
    force?: boolean;
  }) => api.post(`/documents/${entityType}/${entityId}/send`, data),
};

// Solution Proposals (Nary Phase 4): the documents module.
export const salesDocumentsApi = {
  list: (params?: Record<string, unknown>) => api.get("/sales/documents", { params }),
  create: (data: { title: string; language: string; deal_id?: string; subtitle?: string }) => api.post("/sales/documents", data),
  get: (id: string) => api.get(`/sales/documents/${id}`),
  saveBlocks: (id: string, blocks: unknown[]) => api.put(`/sales/documents/${id}/blocks`, { blocks }),
  patch: (id: string, data: Record<string, unknown>) => api.patch(`/sales/documents/${id}`, data),
  preview: (id: string) => api.get(`/sales/documents/${id}/preview`, { responseType: "arraybuffer" }),
  upload: (id: string, data: FormData) => api.post(`/sales/documents/${id}/assets`, data),
  asset: (id: string, assetId: string) => api.get(`/sales/documents/${id}/assets/${assetId}`, { responseType: "blob" }),
  ask: (id: string, text: string) => api.post(`/sales/documents/${id}/ask`, { text }),
  confirm: (id: string, lines: unknown[]) => api.post(`/sales/documents/${id}/confirm`, { lines }),
  revise: (id: string, reason: string) => api.post(`/sales/documents/${id}/revise`, { reason }),
  linkQuotation: (id: string, quotationId: string, label?: string) =>
    api.post(`/sales/documents/${id}/quotations`, { quotation_id: quotationId, label }),
  costSheet: (id: string) => api.get(`/sales/documents/${id}/cost-sheet`),
  reestimate: (id: string) => api.post(`/sales/documents/${id}/cost-sheet`),
  rateCard: () => api.get("/sales/rate-card"),
  addRate: (data: { role: string; seniority: string; cost_base: string; source: string; valid_from: string }) =>
    api.post("/sales/rate-card", data),
  researchRates: () => api.post("/sales/rate-card/research"),
  // visuals V4: draw a figure spec (nothing saved) and check it by eye; {svg, png_base64, warnings} / {ok, notes}
  visualPreview: (id: string, body: Record<string, unknown>) => api.post(`/sales/documents/${id}/visuals/preview`, body, { timeout: 120000 }),
  visualCheck: (id: string, body: Record<string, unknown>) => api.post(`/sales/documents/${id}/visuals/check`, body, { timeout: 120000 }),
  // fix-longapply: a long job's state (202 answers carry its job_id); reject / reopen a proposal
  job: (id: string, jobId: string) => api.get(`/sales/documents/${id}/jobs/${jobId}`),
  reject: (id: string, reason: string) => api.post(`/sales/documents/${id}/reject`, { reason }),
  reopen: (id: string) => api.post(`/sales/documents/${id}/reopen`),
};

// E-signature started from an approved document (render + QR + designated signers).
export const signingFlowApi = {
  start: (entityType: string, entityId: string, message?: string) =>
    api.post("/sign/from-document", { entity_type: entityType, entity_id: entityId, message }),
  status: (entityType: string, entityId: string) => api.get(`/sign/from-document/${entityType}/${entityId}`),
  extend: (docId: string, days: number) => api.post(`/sign/documents/${docId}/extend`, { days }),
};

/** Nary, the AI sales and operations agent (backend routers/nary.py). */
export const naryApi = {
  status: () => api.get("/nary/status"),
  pause: (on: boolean) => api.post("/nary/pause", { on }),
  shadow: (on: boolean) => api.post("/nary/shadow", { on }),
  send: (body: { channel: "email" | "whatsapp"; text: string; chat_id?: string | null; thread_id?: string | null;
                 contact_id?: string | null; to?: string | null; subject?: string | null;
                 cc?: string[] | null; bcc?: string[] | null; from_inbox?: string | null;
                 attach?: { type: string; id: string }[] | null }) => api.post("/nary/send", body, { timeout: 120000 }),
  /** Link a WhatsApp chat (its messages, the contact's open lead or deal) to a CRM contact. */
  linkChat: (chatId: string, contactId: string) =>
    api.post(`/nary/chats/${encodeURIComponent(chatId)}/link`, { contact_id: contactId }),
  /** Book a meeting at an agreed time (Google Calendar + Meet), then send the customer the link. */
  runAction: (body: { action: string; payload: Record<string, unknown>; message?: Record<string, unknown> | null }) =>
    api.post<{ id: string; status: string; label: string | null; sent: string | null; sent_error: string | null }>(
      "/nary/actions/run", body, { timeout: 120000 }),
  bookMeeting: (body: { title: string; start: string; duration_minutes?: number; staff_ids?: string[];
                        customer_email?: string | null; deal_id?: string | null; lead_id?: string | null;
                        contact_id?: string | null; message?: Record<string, unknown> | null }) =>
    api.post<{ id: string; label: string; meet_link: string | null; html_link: string | null; sent: string | null;
               sent_error?: string | null }>(
      "/nary/meetings/book", body, { timeout: 120000 }),
  sendBrief: (slot: "morning" | "midday" | "wrapup") => api.post(`/nary/briefs/${slot}/send`),
  cards: (params?: Record<string, unknown>) => api.get("/nary/pending", { params }),
  decide: (id: string, data: { decision: "apply" | "reject"; edited_payload?: unknown; reason?: string }) =>
    api.post(`/nary/pending/${id}/decide`, data),
  /** fix-longapply: one card (polled while it is still applying). */
  card: (id: string) => api.get(`/nary/pending/${id}`),
  batchApply: (ids: string[]) => api.post("/nary/pending/batch", { ids }),
  runs: (params?: Record<string, unknown>) => api.get("/nary/runs", { params }),
  run: (id: string) => api.get(`/nary/runs/${id}`),
  memory: (params?: Record<string, unknown>) => api.get("/nary/memory", { params }),
  relearnStyle: () => api.post("/nary/memory/style/relearn"),
  addMemory: (data: unknown) => api.post("/nary/memory", data),
  editMemory: (id: string, data: unknown) => api.patch(`/nary/memory/${id}`, data),
  deleteMemory: (id: string) => api.delete(`/nary/memory/${id}`),
  autonomy: () => api.get("/nary/autonomy"),
  setLevel: (action: string, level: number) => api.put(`/nary/autonomy/${action}`, { level }),
  promote: (action: string) => api.post(`/nary/autonomy/${action}/promote`),
  roles: () => api.get("/nary/roles"),
  setRole: (sopRole: string, data: { is_nary: boolean; user_id?: string | null }) =>
    api.put(`/nary/roles/${encodeURIComponent(sopRole)}`, data),
  staffChats: (userId?: string) => api.get("/nary/staff-chats", { params: userId ? { user_id: userId } : {} }),
  addStaffChat: (data: { chat: string; label?: string; user_id?: string }) => api.post("/nary/staff-chats", data),
  removeStaffChat: (id: string) => api.delete(`/nary/staff-chats/${id}`),
  searchStaffChats: (q: string, userId?: string) =>
    api.get("/nary/staff-chats/search", { params: { q, ...(userId ? { user_id: userId } : {}) } }),
  makeStaffChatPrimary: (id: string) => api.post(`/nary/staff-chats/${id}/primary`),
  ownerTopics: (status: "open" | "all" = "open") => api.get("/nary/owner/topics", { params: { status } }),
  closeOwnerTopic: (id: string) => api.post(`/nary/owner/topics/${id}/close`),
  // Phase 2: channels and intake
  verdicts: (filtered = true) => api.get("/nary/intake/verdicts", { params: { filtered } }),
  correctVerdict: (id: string, verdict: string, note?: string) =>
    api.post(`/nary/intake/verdicts/${id}/correct`, { verdict, note }),
  heldChats: () => api.get("/nary/intake/held"),
  decideHeld: (chatId: string, business: boolean) =>
    api.post(`/nary/intake/held/${encodeURIComponent(chatId)}`, { business }),
  arrivals: (status?: string) => api.get("/nary/intake/arrivals", { params: status ? { status } : {} }),
  identityLinks: () => api.get("/nary/intake/links"),
  undoLink: (id: string) => api.post(`/nary/intake/links/${id}/undo`),
  exclusions: () => api.get("/nary/exclusions"),
  addExclusion: (chat: string) => api.post("/nary/exclusions", { chat }),
  dismissGivenUp: () => api.post("/nary/events/dismiss-given-up"),
  includeAgain: (chatId: string) => api.post(`/nary/exclusions/${encodeURIComponent(chatId)}/include`),
  channelsHealth: () => api.get("/nary/channels/health"),
  history: () => api.get("/nary/history"),
  historyRefresh: () => api.post("/nary/history/refresh"),
  historyDecide: (decisions: Record<string, "include" | "exclude">) => api.post("/nary/history/decisions", { decisions }),
  historyRun: (run: boolean) => api.post("/nary/history/run", { run }),
  historyImportNow: (chatId: string) =>
    api.post(`/nary/history/chats/${encodeURIComponent(chatId)}/import-now`, null, { timeout: 90000 }),
  historyCreate: (chatIds: string[]) => api.post("/nary/history/create-records", { chat_ids: chatIds }),
  // Phase 3: playbooks, meetings, report
  google: () => api.get("/nary/google"),
  googleCredentials: (data: { client_id: string; client_secret?: string }) => api.put("/nary/google/credentials", data),
  googleConnect: () => api.post("/nary/google/connect"),
  meetings: () => api.get("/nary/meetings"),
  meetingDebrief: (id: string, data: FormData) => api.post(`/nary/meetings/${id}/debrief`, data),
  dealDebrief: (dealId: string, data: FormData) => api.post(`/nary/deals/${dealId}/debrief`, data),
  meetingNoShow: (id: string) => api.post(`/nary/meetings/${id}/no-show`),
  // batch 3: meeting recordings and notes files (transcribed and read in the background)
  meetingRecording: (id: string, file: File, onProgress?: (percent: number) => void) =>
    postInPieces(`/nary/meetings/${id}/recording`, file, onProgress),
  meetingFiles: (meetingId?: string) => api.get("/nary/meeting-files", { params: meetingId ? { meeting_id: meetingId } : {} }),
  meetingFileUpload: (file: File, onProgress?: (percent: number) => void) =>
    postInPieces("/nary/meeting-files", file, onProgress),
  meetingFileTranscript: (fileId: string) =>
    api.get(`/nary/meeting-files/${fileId}/transcript`, { responseType: "blob" }),
  // batch 4 D1: any file attached in the chat (notes and recordings still go to the meeting files)
  chatFileUpload: (file: File, onProgress?: (percent: number) => void) =>
    postInPieces("/nary/chat-files", file, onProgress),
  dealPanel: (dealId: string) => api.get(`/nary/deals/${dealId}/panel`),
  leadPanel: (leadId: string) => api.get(`/nary/leads/${leadId}/panel`),
  dealMeeting: (dealId: string) => api.post(`/nary/deals/${dealId}/meeting`),
  report: () => api.get("/nary/report"),
  sendReport: () => api.post("/nary/report/send"),
  sopRevisions: () => api.get("/nary/sop-revisions"),
  draftSops: (codes: string[]) => api.post("/nary/sop-revisions", { codes }),
};
