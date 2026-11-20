"use client";
import ScheduleSendModal, { ScheduledMsg } from "@/components/crm/ScheduleSendModal";

import { useEffect, useState, useCallback, useMemo, useRef } from "react";
import { useSearchParams } from "next/navigation";
import { useSession } from "next-auth/react";
import { sanitizeEmailHtml } from "@/lib/sanitize";
import EmailBody from "@/components/email/EmailBody";
import { crmInboxApi, crmApi, aiApi , emailSignatureApi, type InboxMailbox, type ReplyDefaults } from "@/lib/api";
import ReplyAddressFields, {
  EMPTY_ADDRESSING, addressingFor, appendAddressing, invalidAddress, splitAddresses, type ReplyAddressing, type ReplyMode,
} from "@/components/crm/ReplyAddressFields";
import RichTextEditor from "@/components/ui/RichTextEditor";
import CrmContextPanel from "@/components/crm/CrmContextPanel";
import { useDropToInput } from "@/hooks/useFileDrop";
import DocAnalyzeButton from "@/components/ai/DocAnalyzeButton";
import FollowUpButton from "@/components/crm/FollowUpButton";
import ReplyPromptButton from "@/components/ai/ReplyPromptButton";
import { draftToHtml } from "@/lib/replyDraft";
import {
  Mail, Inbox, RefreshCw, Search, Send,
  Paperclip, X, Download, Link2, CheckCircle2,
  AlertCircle, ArrowLeft, User, Building2, Tag, Clock, Sparkles, MailOpen, Trash2, UserCheck, Flag,
  ChevronDown, ChevronRight, Maximize2, Minimize2, Reply, ReplyAll, Forward,
} from "lucide-react";
import BulkActionBar from "@/components/common/BulkActionBar";
import {
  Button, Checkbox, Dialog, EmptyState, IconButton, Input, Pill, Select, Skeleton, StatusPill,
  Tabs, TabsList, TabsTrigger,
  FullHeightPage, useIsPhone,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { messagingApi } from "@/lib/api";
import { usePermissions } from "@/hooks/usePermissions";
import { useUrlParam } from "@/hooks/useUrlParam";
import { saveFile } from "@/lib/safeFile";

// Types

interface Thread {
  id: string; subject: string; status: string; is_read: boolean;
  message_count: number; last_sender_email: string | null; last_sender_name: string | null;
  last_message_at: string | null;
  linked_contact_id: string | null; linked_contact_name: string | null;
  linked_org_id: string | null; linked_org_name: string | null;
  linked_lead_id: string | null; linked_deal_id: string | null;
  assigned_to_id?: string | null; assigned_to_name?: string | null;
  /** The CRM inbox address the conversation belongs to. */
  inbox?: string | null;
  messages?: Message[];
}

interface Message {
  id: string; thread_id: string; direction: "inbound" | "outbound";
  from_email: string; from_name: string | null;
  to_emails: string[]; cc_emails: string[];
  subject: string | null; body_html: string | null; body_text: string | null;
  received_at: string | null;
  attachments: Attachment[];
}

interface Attachment { id: string; filename: string; content_type: string; size_bytes: number }

// Helpers

const STATUS_LABEL: Record<string, string> = {
  open: "Open", in_progress: "In progress", resolved: "Resolved",
};

function timeAgo(iso: string | null) {
  if (!iso) return "";
  const diff = Date.now() - new Date(iso).getTime();
  const m = Math.floor(diff / 60000);
  if (m < 1) return "just now";
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d}d ago`;
  return new Date(iso).toLocaleDateString();
}

function formatBytes(b: number) {
  if (b < 1024) return `${b} B`;
  if (b < 1048576) return `${(b / 1024).toFixed(1)} KB`;
  return `${(b / 1048576).toFixed(1)} MB`;
}

function initials(name: string | null, email: string | null) {
  const src = name || email || "?";
  return src.charAt(0).toUpperCase();
}

function escapeHtml(t: string) {
  return t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// Reply composer: folded to a bar by default; open/folded and its height are remembered per user.

const COMPOSER_PREFS = "granary.crm.emailComposer";
const COMPOSER_FRAC = { default: 0.4, min: 0.25, max: 0.5 };
interface ComposerPrefs { open: boolean; frac: number }

const clampFrac = (f: number) => Math.min(COMPOSER_FRAC.max, Math.max(COMPOSER_FRAC.min, f));

function readComposerPrefs(key: string): ComposerPrefs {
  try {
    const p = JSON.parse(window.localStorage.getItem(key) || "{}");
    return { open: p.open === true, frac: typeof p.frac === "number" ? clampFrac(p.frac) : COMPOSER_FRAC.default };
  } catch {
    return { open: false, frac: COMPOSER_FRAC.default };
  }
}

function writeComposerPrefs(key: string, patch: Partial<ComposerPrefs>) {
  try {
    window.localStorage.setItem(key, JSON.stringify({ ...readComposerPrefs(key), ...patch }));
  } catch { /* private window: the defaults apply next time */ }
}

const MODE_LABEL: Record<ReplyMode, string> = { reply: "Reply", all: "Reply all", forward: "Forward" };
const MODE_ICON: Record<ReplyMode, React.ElementType> = { reply: Reply, all: ReplyAll, forward: Forward };

/** "Signature: Name, Title" from the user's signature fields (GET /email/signature/me). */
function signatureLabel(fields: { name: string; value: unknown }[] | undefined): string {
  const get = (k: string) => String(fields?.find((f) => f.name === k)?.value ?? "").trim();
  const parts = [get("name"), get("title") || get("position")].filter(Boolean);
  return parts.length ? `Signature: ${parts.join(", ")}` : "Signature";
}

/** The last message quoted under a forward, the way mail apps do it. */
function forwardQuote(msg: Message): string {
  const from = escapeHtml(msg.from_name ? `${msg.from_name} <${msg.from_email}>` : msg.from_email);
  const date = msg.received_at ? escapeHtml(new Date(msg.received_at).toLocaleString()) : "";
  const body = msg.body_html
    ? sanitizeEmailHtml(msg.body_html)
    : `<p>${escapeHtml(msg.body_text || "").replace(/\n/g, "<br>")}</p>`;
  return `<p><br></p><div data-granary-forward><p>---------- Forwarded message ----------<br>From: ${from}<br>`
    + (date ? `Date: ${date}<br>` : "")
    + `Subject: ${escapeHtml(msg.subject || "")}<br>To: ${escapeHtml((msg.to_emails || []).join(", "))}</p>${body}</div>`;
}

// Link modal

function LinkModal({
  thread, onClose, onLinked,
}: {
  thread: Thread;
  onClose: () => void;
  onLinked: (update: Partial<Thread>) => void;
}) {
  const [tab, setTab] = useState<"contact" | "org" | "lead" | "deal">("contact");
  const [search, setSearch] = useState("");
  const [results, setResults] = useState<{ id: string; label: string }[]>([]);
  const [saving, setSaving] = useState(false);

  const doSearch = useCallback(async (q: string) => {
    if (!q.trim()) { setResults([]); return; }
    try {
      if (tab === "contact") {
        const r = await crmApi.listContacts({ q, limit: 10 });
        const items = r.data.items ?? r.data ?? [];
        setResults(items.map((c: { id: string; first_name: string; last_name: string; email: string }) => ({
          id: c.id,
          label: `${c.first_name} ${c.last_name}`.trim() + (c.email ? ` <${c.email}>` : ""),
        })));
      } else if (tab === "org") {
        const r = await crmApi.listOrgs({ q, limit: 10 });
        const items = r.data.items ?? r.data ?? [];
        setResults(items.map((o: { id: string; name: string }) => ({ id: o.id, label: o.name })));
      } else if (tab === "lead") {
        const r = await crmApi.listLeads({ q, limit: 10 });
        const items = r.data.items ?? r.data ?? [];
        setResults(items.map((l: { id: string; title?: string; contact?: { first_name: string; last_name: string } }) => ({
          id: l.id, label: l.title || `Lead ${l.id.slice(0, 8)}`,
        })));
      } else {
        const r = await crmApi.listDeals({ q, limit: 10 });
        const items = r.data.items ?? r.data ?? [];
        setResults(items.map((d: { id: string; title: string }) => ({ id: d.id, label: d.title })));
      }
    } catch { setResults([]); }
  }, [tab]);

  useEffect(() => {
    const t = setTimeout(() => doSearch(search), 300);
    return () => clearTimeout(t);
  }, [search, doSearch]);

  async function link(id: string, label: string) {
    setSaving(true);
    try {
      const payload: Record<string, string | null> = {};
      if (tab === "contact") payload.linked_contact_id = id;
      else if (tab === "org") payload.linked_org_id = id;
      else if (tab === "lead") payload.linked_lead_id = id;
      else payload.linked_deal_id = id;

      await crmInboxApi.updateThread(thread.id, payload);
      const update: Partial<Thread> = {};
      if (tab === "contact") { update.linked_contact_id = id; update.linked_contact_name = label.split(" <")[0]; }
      else if (tab === "org") { update.linked_org_id = id; update.linked_org_name = label; }
      else if (tab === "lead") update.linked_lead_id = id;
      else update.linked_deal_id = id;
      onLinked(update);
      onClose();
    } finally { setSaving(false); }
  }

  const TABS = [
    { id: "contact" as const, label: "Contact", icon: User },
    { id: "org" as const, label: "Organization", icon: Building2 },
    { id: "lead" as const, label: "Lead", icon: Tag },
    { id: "deal" as const, label: "Deal", icon: CheckCircle2 },
  ];

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} title="Link to CRM record">
      <Tabs value={tab} onValueChange={(v) => { setTab(v as typeof tab); setSearch(""); setResults([]); }}>
        <TabsList className="mb-3">
          {TABS.map((t) => (
            <TabsTrigger key={t.id} value={t.id}>
              <t.icon className="size-3.5" aria-hidden />
              {t.label}
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>

      <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
        value={search} onChange={(e) => setSearch(e.target.value)}
        placeholder={`Search ${tab}s`} aria-label={`Search ${tab}s`} autoFocus className="mb-3" />

      <div className="max-h-56 space-y-0.5 overflow-y-auto">
        {results.length === 0 ? (
          <p className="py-6 text-center text-sm text-fg-muted">{search ? "No results" : `Type to search ${tab}s`}</p>
        ) : results.map((r) => (
          <button key={r.id} type="button" onClick={() => link(r.id, r.label)} disabled={saving}
            className="min-h-11 w-full truncate rounded-lg px-3 py-2 text-left text-sm text-fg transition-colors hover:bg-surface-2 disabled:opacity-50 md:min-h-9">
            {r.label}
          </button>
        ))}
      </div>
    </Dialog>
  );
}

// Message bubble

function MessageBubble({ msg, onDelete }: { msg: Message; onDelete?: (id: string) => Promise<void> }) {
  const isOut = msg.direction === "outbound";
  const [showHtml, setShowHtml] = useState(true);
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);

  return (
    <div className={cn("mb-4 flex gap-3", isOut && "flex-row-reverse")}>
      <div className={cn("flex size-8 shrink-0 items-center justify-center rounded-full text-sm font-semibold",
        isOut ? "bg-brand/10 text-brand-ink" : "bg-surface-2 text-fg-muted")} aria-hidden>
        {initials(msg.from_name, msg.from_email)}
      </div>

      <div className={cn("flex min-w-0 max-w-[75%] flex-col", isOut ? "items-end" : "items-start")}>
        <div className="mb-1 flex items-center gap-2">
          <span className="max-w-[200px] truncate text-xs font-medium text-fg">
            {msg.from_name || msg.from_email}
          </span>
          {msg.received_at && (
            <span className="shrink-0 text-xs text-fg-muted tabular">{timeAgo(msg.received_at)}</span>
          )}
          {isOut && (
            <span className="shrink-0 text-xs font-medium text-brand-ink">You</span>
          )}
          {onDelete && (confirming ? (
            <span className="flex shrink-0 items-center gap-1">
              <Button size="sm" variant="danger" loading={deleting}
                onClick={async () => { setDeleting(true); try { await onDelete(msg.id); } finally { setDeleting(false); setConfirming(false); } }}>
                Delete email
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirming(false)}>Cancel</Button>
            </span>
          ) : (
            <IconButton size="sm" icon={Trash2} label="Delete this email (moves it to the mailbox's Trash)"
              onClick={() => setConfirming(true)} className="text-fg-subtle hover:text-tone-red" />
          ))}
        </div>

        <div className={cn("rounded-lg border p-4 text-sm text-fg",
          isOut ? "rounded-tr-sm border-brand/20 bg-brand/[0.07]" : "rounded-tl-sm border-line bg-surface")}>
          {msg.body_html && showHtml ? (
            <EmailBody html={msg.body_html} sender={msg.from_email} trusted={isOut}
              title={msg.subject || "Email message"} />
          ) : (
            <pre className="whitespace-pre-wrap font-sans text-sm leading-relaxed">{msg.body_text || "(empty)"}</pre>
          )}
          {msg.body_html && msg.body_text && (
            <button type="button" onClick={() => setShowHtml((v) => !v)}
              className="mt-2 text-xs text-fg-muted transition-colors hover:text-fg">
              {showHtml ? "Show plain text" : "Show HTML"}
            </button>
          )}
        </div>

        {msg.attachments.length > 0 && (
          <div className="mt-2 flex flex-wrap gap-2">
            {msg.attachments.map((a) => (
              <div key={a.id} className="flex items-center gap-1 rounded-lg border border-line bg-surface">
                <button
                  type="button"
                  onClick={async () => {
                    try {
                      const r = await crmInboxApi.downloadAttachment(a.id);
                      saveFile(r.data as Blob, a.filename);
                    } catch { /* silent */ }
                  }}
                  className="flex min-h-9 items-center gap-1.5 rounded-l-lg px-3 py-1.5 text-xs font-medium text-fg transition-colors hover:bg-surface-2"
                >
                  <Paperclip className="size-3" aria-hidden />
                  <span className="max-w-[160px] truncate">{a.filename}</span>
                  <span className="text-fg-muted tabular">{formatBytes(a.size_bytes)}</span>
                  <Download className="size-3 shrink-0" aria-hidden />
                </button>
                <DocAnalyzeButton
                  body={{ source: "crm_email", attachment_id: a.id }}
                  filename={a.filename}
                  mime={a.content_type}
                  className="rounded-lg p-1.5 text-fg-subtle transition-colors hover:bg-surface-2 hover:text-fg"
                />
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// Main page

const THREAD_PAGE_SIZE = 40;

export default function CrmEmailPage() {
  const { data: session } = useSession();
  const accessToken = (session as { access_token?: string })?.access_token;

  const [threads, setThreads] = useState<Thread[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [canLoadMore, setCanLoadMore] = useState(true);
  const [statusFilter, setStatusFilter] = useState("all");
  const [unreadOnly, setUnreadOnly] = useState(false);
  // "Assigned to me": reached from an assignment notification (?assigned=me).
  const urlParams = useSearchParams();
  const [assignedMe, setAssignedMe] = useState(urlParams.get("assigned") === "me");
  // Inbox: conversations with mail from customers. Sent: conversations we wrote in, from the ERP
  // or from Thunderbird/webmail, since the mailbox's Sent folder is synced too.
  const [box, setBox] = useState<"inbox" | "sent">("inbox");
  // The shared inboxes (hello@, sales@...): a filter and a pill only when there is more than one.
  const [mailboxes, setMailboxes] = useState<InboxMailbox[]>([]);
  const [inboxFilter, setInboxFilter] = useState("");
  const multiInbox = mailboxes.length > 1;
  useEffect(() => {
    crmInboxApi.mailboxes().then((r) => setMailboxes(r.data?.mailboxes ?? [])).catch(() => { /* one inbox */ });
  }, []);
  const perms = usePermissions();
  // Deleting from the shared mailbox is for the Owner (superadmin) and system admins only.
  const canDelete = perms.isAdmin;
  // The API reads the inbox with crm:view_all or crm:view_own, and replies / syncs with
  // crm:send_email (a superadmin holds every code). Hide what would only fail.
  const canView = perms.holds("crm:view_all") || perms.holds("crm:view_own");
  const canSendEmail = perms.holds("crm:send_email");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const [search, setSearch] = useState("");
  const [activeThread, setActiveThread] = useState<Thread | null>(null);
  const [loadingThread, setLoadingThread] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [syncMsg, setSyncMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [showLinkModal, setShowLinkModal] = useState(false);
  const [panelOpen, setPanelOpen] = useState(false);
  // Phones show one pane at a time; the open thread and the reply sheet live in the URL so the
  // phone Back button closes them.
  const [threadParam, setThreadParam] = useUrlParam("thread");
  const [replyParam, setReplyParam] = useUrlParam("reply");
  const replyOpen = replyParam === "1";
  const closeThread = useCallback(() => { setActiveThread(null); setThreadParam(null); }, [setThreadParam]);

  // Reply state. The editor holds only what the user writes; the signature (and a forwarded message)
  // are kept beside it, previewed folded, and added to the HTML when it is sent.
  const [replyText, setReplyText] = useState("");
  const [replyFiles, setReplyFiles] = useState<File[]>([]);
  const [addressing, setAddressing] = useState<ReplyAddressing>(EMPTY_ADDRESSING);
  const [replyDefaults, setReplyDefaults] = useState<ReplyDefaults | null>(null);
  const [replyMode, setReplyMode] = useState<ReplyMode>("all");
  const replyModeRef = useRef<ReplyMode>("all");
  replyModeRef.current = replyMode;
  const [signatureHtml, setSignatureHtml] = useState("");
  const [signatureName, setSignatureName] = useState("Signature");
  const [quoteHtml, setQuoteHtml] = useState("");
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState("");
  const fileInputRef = useRef<HTMLInputElement>(null);
  const { dragActive: replyDrag, dropHandlers: replyDrop } = useDropToInput(fileInputRef);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  // Composer layout: folded to a bar unless the user keeps it open; resizable and poppable on desktop.
  const isPhone = useIsPhone();
  const userId = (session?.user as { id?: string } | undefined)?.id;
  const prefKey = `${COMPOSER_PREFS}.${userId ?? "default"}`;
  const [composerOpen, setComposerOpen] = useState(false);
  const [popped, setPopped] = useState(false);
  const [composerFrac, setComposerFrac] = useState(COMPOSER_FRAC.default);
  const [headerOpen, setHeaderOpen] = useState(false);
  const [sigOpen, setSigOpen] = useState(false);
  const [quoteOpen, setQuoteOpen] = useState(false);
  const detailRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<HTMLDivElement>(null);
  useEffect(() => { setComposerFrac(readComposerPrefs(prefKey).frac); }, [prefKey]);

  /** Draft a reply with the AI, following the user's direction (if any). It replaces what is typed;
   *  the signature stays as it is; the plain-text draft becomes paragraphs. */
  async function suggestReply(instruction: string) {
    if (!activeThread) return;
    try {
      const res = await aiApi.suggestReply({ channel: "email", thread_id: activeThread.id,
                                             instruction: instruction || undefined });
      if (res.data?.draft) setReplyText(draftToHtml(res.data.draft));
    } catch (e) {
      const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      throw new Error(detail || "Couldn't suggest a reply. Check Settings → Nary & AI.");
    }
  }

  // Scheduled messages state
  const [scheduledMessages, setScheduledMessages] = useState<ScheduledMsg[]>([]);
  const [showScheduleModal, setShowScheduleModal] = useState(false);
  const [editingScheduled, setEditingScheduled] = useState<ScheduledMsg | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setCanLoadMore(true);
    try {
      const r = await crmInboxApi.listThreads({
        status: statusFilter !== "all" ? statusFilter : undefined,
        search: search || undefined,
        unread: unreadOnly || undefined,
        assigned: assignedMe ? "me" : undefined,
        box,
        inbox: inboxFilter || undefined,
        limit: THREAD_PAGE_SIZE,
        offset: 0,
      });
      setThreads(r.data.items ?? []);
      setTotal(r.data.total ?? 0);
    } catch { /* silent */ }
    finally { setLoading(false); }
  }, [statusFilter, search, unreadOnly, assignedMe, box, inboxFilter]);

  // Page further back: drain remaining rows from the DB, then reach into IMAP for
  // older history (predating the first sync) once the DB is exhausted.
  const loadMore = useCallback(async () => {
    if (loadingMore) return;
    setLoadingMore(true);
    try {
      const params = {
        status: statusFilter !== "all" ? statusFilter : undefined,
        search: search || undefined,
        unread: unreadOnly || undefined,
        assigned: assignedMe ? "me" : undefined,
        box,
        inbox: inboxFilter || undefined,
        limit: THREAD_PAGE_SIZE,
        offset: threads.length,
      };
      let r = await crmInboxApi.listThreads(params);
      let items: Thread[] = r.data.items ?? [];
      if (items.length === 0 && !canSendEmail) {
        setCanLoadMore(false);      // reaching into the mailbox is a sync: crm:send_email only
      } else if (items.length === 0) {
        const b = await crmInboxApi.syncOlder();
        if ((b.data?.new ?? 0) > 0) {
          r = await crmInboxApi.listThreads(params);
          items = r.data.items ?? [];
          setTotal(r.data.total ?? total);
        } else if (b.data?.status !== "sync_running") {
          // Genuinely nothing older on the server (a timeout keeps the button available).
          setCanLoadMore(false);
        }
      }
      if (items.length > 0) setThreads((prev) => [...prev, ...items]);
    } catch { /* keep the button available to retry */ }
    finally { setLoadingMore(false); }
  }, [statusFilter, search, unreadOnly, assignedMe, box, inboxFilter, threads.length, loadingMore, total, canSendEmail]);

  useEffect(() => { load(); }, [load]);
  // Auto-refresh every 60 s
  useEffect(() => {
    const t = setInterval(load, 60000);
    return () => clearInterval(t);
  }, [load]);

  // Bulk selection
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [allMatching, setAllMatching] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkNotice, setBulkNotice] = useState<string | null>(null);
  const clearSelection = useCallback(() => { setSelected(new Set()); setAllMatching(false); }, []);
  // A different view is a different selection.
  useEffect(() => { clearSelection(); }, [statusFilter, search, unreadOnly, assignedMe, box, inboxFilter, clearSelection]);

  function toggleSelected(id: string) {
    setAllMatching(false);
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }

  async function runBulk(action: string, value?: string | null) {
    setBulkBusy(true);
    setBulkNotice(null);
    try {
      const res = await crmInboxApi.bulk({
        action: action as "delete" | "read" | "unread" | "status" | "assign",
        ...(allMatching
          ? { filter: { status: statusFilter !== "all" ? statusFilter : undefined, search: search || undefined,
                        box, unread: unreadOnly || undefined, assigned: assignedMe ? "me" : undefined } }
          : { ids: [...selected] }),
        ...(action === "status" ? { status: value ?? undefined } : {}),
        ...(action === "assign" ? { assignee_id: value ?? null } : {}),
      });
      const n: number = res.data.affected ?? 0;
      const what = `${n} ${n === 1 ? "conversation" : "conversations"}`;
      const verb = { delete: "deleted (moved to the mailbox Trash)", read: "marked read", unread: "marked unread",
                     status: `set to ${STATUS_LABEL[value ?? ""] ?? value}`,
                     assign: value ? "assigned" : "unassigned" }[action] ?? "updated";
      setBulkNotice(`${what} ${verb}.`);
      if (action === "delete" && activeThread && (allMatching || selected.has(activeThread.id))) closeThread();
      clearSelection();
      await load();
    } catch (e) {
      const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      setBulkNotice(detail || "The action could not be completed");
    } finally {
      setBulkBusy(false);
      setTimeout(() => setBulkNotice(null), 6000);
    }
  }

  const searchPeople = useCallback(async (q: string) => {
    if (!q.trim()) return [];
    const r = await messagingApi.mentionUsers(q.trim());
    return (r.data ?? []).map((u: { id: string; name: string; position?: string | null }) =>
      ({ value: u.id, label: u.position ? `${u.name} · ${u.position}` : u.name }));
  }, []);

  // Deep-link: open a thread from ?lead=/?contact=/?email= (e.g. from a Lead card)
  const searchParams = useSearchParams();
  const autoOpenedRef = useRef(false);
  // Pre-filter the inbox by the target email so the thread is in the loaded set.
  useEffect(() => {
    const email = searchParams.get("email");
    if (email && !autoOpenedRef.current) setSearch(email);
    // An assignment notification links straight to the conversation (opened by the URL sync below).
    if (searchParams.get("thread")) autoOpenedRef.current = true;
    // run once on mount
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    if (autoOpenedRef.current || loading || !threads.length) return;
    const leadId = searchParams.get("lead");
    const contactId = searchParams.get("contact");
    const email = (searchParams.get("email") || "").toLowerCase();
    if (!leadId && !contactId && !email) return;
    let match: Thread | undefined;
    if (leadId) match = threads.find((t) => t.linked_lead_id === leadId);
    if (!match && contactId) match = threads.find((t) => t.linked_contact_id === contactId);
    if (!match && email) {
      match = threads.find((t) => (t.last_sender_email || "").toLowerCase() === email) || threads[0];
    }
    if (match) {
      autoOpenedRef.current = true;
      openThread(match);
    }
    // openThread is stable enough for a one-shot guarded by autoOpenedRef
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threads, loading, searchParams]);

  const fetchScheduledForThread = useCallback(async (threadId: string) => {
    if (!accessToken) return;
    try {
      const res = await fetch(
        `/api/v1/crm/scheduled?channel=email&chat_id=${encodeURIComponent(threadId)}&status=pending`,
        { headers: { Authorization: `Bearer ${accessToken}` } },
      );
      if (res.ok) setScheduledMessages(await res.json());
    } catch { /* silent */ }
  }, [accessToken]);

  async function cancelScheduledEmail(id: string) {
    if (!accessToken) return;
    await fetch(`/api/v1/crm/scheduled/${id}`, {
      method: "DELETE",
      headers: { Authorization: `Bearer ${accessToken}` },
    });
    setScheduledMessages((prev) => prev.filter((m) => m.id !== id));
  }

  // The URL is the source of truth for which thread is open: Back/forward and deep links.
  useEffect(() => {
    if (!threadParam) {
      if (activeThread) setActiveThread(null);
      return;
    }
    if (threadParam !== activeThread?.id) openThread({ id: threadParam } as Thread);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadParam]);

  async function openThread(thread: Thread) {
    setThreadParam(thread.id);
    setConfirmDelete(false);
    setDeleteError("");
    setLoadingThread(true);
    // The same thread again keeps the draft; another one starts a fresh reply
    if (thread.id !== activeThread?.id) {
      setReplyText("");
      setReplyFiles([]);
      setSendError("");
      setQuoteHtml("");
      setReplyMode("all");
      setHeaderOpen(false);
      setSigOpen(false);
      setPopped(false);
      setComposerOpen(readComposerPrefs(prefKey).open);
    }
    setActiveThread({ ...thread });
    setScheduledMessages([]);
    try {
      const r = await crmInboxApi.getThread(thread.id);
      setActiveThread(r.data);
      setThreads((ts) => ts.map((t) => t.id === thread.id ? { ...t, is_read: true } : t));
      fetchScheduledForThread(thread.id);
      setTimeout(() => messagesEndRef.current?.scrollIntoView({ behavior: "smooth" }), 100);
    } finally { setLoadingThread(false); }
  }

  async function handleSync() {
    setSyncing(true);
    setSyncMsg(null);
    try {
      const res = await crmInboxApi.sync();
      await load();
      const n = (res.data?.new as number) ?? 0;
      if (res.data?.status === "sync_running") {
        setSyncMsg({ ok: true, text: "Sync started, fetching in the background." });
      } else {
        setSyncMsg({ ok: true, text: n > 0 ? `Synced ${n} new email${n === 1 ? "" : "s"}` : "Up to date, no new emails" });
      }
    } catch (e: unknown) {
      const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Sync failed";
      setSyncMsg({ ok: false, text: detail });
    } finally { setSyncing(false); }
  }

  // The signature that goes under every reply, and its one-line label for the folded preview.
  useEffect(() => {
    emailSignatureApi.render().then((r) => setSignatureHtml(r.data?.html || "")).catch(() => { /* no signature */ });
    emailSignatureApi.getMine().then((r) => setSignatureName(signatureLabel(r.data?.fields))).catch(() => { /* "Signature" */ });
  }, []);

  // Who a reply goes to by default (GET .../reply-defaults), shaped by Reply, Reply all or Forward.
  const activeThreadId = activeThread?.id;
  useEffect(() => {
    setReplyDefaults(null);
    setAddressing(EMPTY_ADDRESSING);
    if (!activeThreadId) return;
    let live = true;
    crmInboxApi.replyDefaults(activeThreadId).then((r) => {
      if (!live) return;
      setReplyDefaults(r.data);
      if (replyModeRef.current === "forward") {
        setAddressing((a) => (a.from_inbox ? a : { ...a, from_inbox: r.data.from_inbox ?? "" }));
      } else {
        setAddressing(addressingFor(replyModeRef.current, r.data, ""));
      }
    }).catch(() => { /* the server's own defaults apply */ });
    return () => { live = false; };
  }, [activeThreadId]);

  // Display names for the folded address line, from the people who wrote in this thread.
  const addressNames = useMemo(() => {
    const names: Record<string, string> = {};
    for (const m of activeThread?.messages ?? []) {
      if (m.from_name && m.from_email) names[m.from_email.toLowerCase()] = m.from_name;
    }
    return names;
  }, [activeThread?.messages]);

  /** Text the user actually typed. */
  const typedBody = replyText.replace(/<div data-granary-signature[\s\S]*$/i, "").replace(/<[^>]+>/g, "").trim();
  /** What is sent: the typed text, the full signature, then a forwarded message if any. */
  const composedHtml = replyText + signatureHtml + quoteHtml;
  // The signature alone is not a reply; a forward may go without a note.
  const canSend = !!typedBody || (replyMode === "forward" && !!quoteHtml);
  const hasDraft = !!typedBody || replyFiles.length > 0;
  // Kept mounted while folded, so the draft survives; the bar shows while it is folded.
  const composerHidden = !composerOpen && !replyOpen;
  const barHidden = isPhone ? replyOpen : composerOpen;

  function focusComposer(mode: ReplyMode) {
    setTimeout(() => {
      const el = composerRef.current?.querySelector<HTMLElement>(
        mode === "forward" ? 'input[aria-label="To"]' : '[contenteditable="true"]');
      el?.focus();
    }, 50);
  }

  /** Open the composer as Reply, Reply all or Forward. Switching mode resets the recipients only. */
  function openComposer(mode: ReplyMode) {
    if (!activeThread || !canSendEmail) return;
    if (mode !== replyMode) {
      setReplyMode(mode);
      setAddressing(addressingFor(mode, replyDefaults, activeThread.subject));
      const last = (activeThread.messages ?? []).at(-1);
      setQuoteHtml(mode === "forward" && last ? forwardQuote(last) : "");
      setQuoteOpen(false);
      if (mode === "forward") setHeaderOpen(true);
    }
    setComposerOpen(true);
    if (isPhone) setReplyParam("1");
    else writeComposerPrefs(prefKey, { open: true });
    focusComposer(mode);
  }

  /** Fold the composer to its bar; the draft, recipients and attachments stay. */
  function collapseComposer() {
    setPopped(false);
    setComposerOpen(false);
    if (replyOpen) setReplyParam(null);
    if (!isPhone) writeComposerPrefs(prefKey, { open: false });
  }

  // Esc folds the composer (or brings a popped-out one back); R opens Reply.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (!activeThread || activeThread.status === "resolved" || e.defaultPrevented) return;
      if (document.querySelector('[role="dialog"]:not([data-reply-composer])')) return;
      const t = e.target instanceof HTMLElement ? e.target : null;
      if (e.key === "Escape") {
        if (t && t !== document.body && !detailRef.current?.contains(t)) return;
        if (popped) { e.preventDefault(); setPopped(false); return; }
        if (composerOpen || replyOpen) { e.preventDefault(); collapseComposer(); }
        return;
      }
      if ((e.key === "r" || e.key === "R") && !e.ctrlKey && !e.metaKey && !e.altKey) {
        if (t && (t.isContentEditable || t.closest("[contenteditable]") || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
        if (isPhone ? replyOpen : composerOpen) return;
        e.preventDefault();
        openComposer("reply");
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  });

  /** Drag the handle between the thread and the composer (up to half the pane). */
  function startResize(e: React.PointerEvent) {
    const pane = detailRef.current;
    if (!pane) return;
    e.preventDefault();
    const startY = e.clientY;
    const startFrac = composerFrac;
    const height = pane.clientHeight || 1;
    let frac = startFrac;
    const move = (ev: PointerEvent) => { frac = clampFrac(startFrac + (startY - ev.clientY) / height); setComposerFrac(frac); };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      writeComposerPrefs(prefKey, { frac });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function nudgeResize(e: React.KeyboardEvent) {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    const frac = clampFrac(composerFrac + (e.key === "ArrowUp" ? 0.05 : -0.05));
    setComposerFrac(frac);
    writeComposerPrefs(prefKey, { frac });
  }

  async function handleSend() {
    if (!activeThread) return;
    if (!canSend) return;
    if (addressing.loaded) {
      if (!splitAddresses(addressing.to).length) { setHeaderOpen(true); setSendError("Enter at least one recipient."); return; }
      const bad = invalidAddress(addressing);
      if (bad) { setHeaderOpen(true); setSendError(`"${bad}" is not a valid email address.`); return; }
    }
    setSending(true); setSendError("");
    try {
      const fd = new FormData();
      const plain = composedHtml
        .replace(/<br\s*\/?>(?=)/gi, "\n")
        .replace(/<\/(p|div|tr|table)>/gi, "\n")
        .replace(/<[^>]+>/g, "")
        .replace(/\n{3,}/g, "\n\n")
        .trim();
      fd.append("body_html", composedHtml);
      fd.append("body_text", plain);
      for (const file of replyFiles) fd.append("files", file);
      appendAddressing(fd, addressing);

      const r = await crmInboxApi.reply(activeThread.id, fd);
      const newMsg: Message = r.data;

      setActiveThread((prev) => prev ? {
        ...prev,
        messages: [...(prev.messages || []), newMsg],
        message_count: (prev.message_count || 0) + 1,
        last_message_at: newMsg.received_at,
      } : prev);
      setReplyText("");
      setReplyFiles([]);
      setQuoteHtml("");
      setReplyMode("all");
      setAddressing(addressingFor("all", replyDefaults, activeThread.subject));
      setPopped(false);
      setComposerOpen(false);
      setReplyParam(null);
      setTimeout(() => messagesEndRef.current?.scrollIntoView({ behavior: "smooth" }), 100);
    } catch (e: unknown) {
      setSendError((e as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Failed to send");
    } finally { setSending(false); }
  }

  /** Delete the whole conversation: every email moves to the mailbox's Trash. */
  async function deleteConversation() {
    if (!activeThread) return;
    const id = activeThread.id;
    setDeleting(true);
    setDeleteError("");
    try {
      await crmInboxApi.deleteThread(id);
      setThreads((ts) => ts.filter((t) => t.id !== id));
      setTotal((n) => Math.max(0, n - 1));
      closeThread();
      setConfirmDelete(false);
    } catch (e) {
      setDeleteError((e as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Could not delete the conversation.");
    } finally {
      setDeleting(false);
    }
  }

  /** Delete one email; the conversation goes too if it was the last one. */
  async function deleteOneEmail(messageId: string) {
    if (!activeThread) return;
    const id = activeThread.id;
    setDeleteError("");
    try {
      const r = await crmInboxApi.deleteMessage(messageId);
      if (r.data?.threads_removed) {
        setThreads((ts) => ts.filter((t) => t.id !== id));
        closeThread();
        return;
      }
      const fresh = await crmInboxApi.getThread(id);
      setActiveThread(fresh.data);
      setThreads((ts) => ts.map((t) => (t.id === id ? { ...t, message_count: fresh.data.message_count } : t)));
    } catch (e) {
      setDeleteError((e as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Could not delete the email.");
    }
  }

  async function markUnread() {
    if (!activeThread) return;
    const id = activeThread.id;
    try {
      await crmInboxApi.markUnread(id);
      setThreads((ts) => ts.map((t) => (t.id === id ? { ...t, is_read: false } : t)));
      closeThread();
    } catch { /* stays read; the next sync reconciles */ }
  }

  async function changeStatus(status: string) {
    if (!activeThread) return;
    await crmInboxApi.updateThread(activeThread.id, { status });
    setActiveThread((prev) => prev ? { ...prev, status } : prev);
    setThreads((ts) => ts.map((t) => t.id === activeThread.id ? { ...t, status } : t));
  }

  const STATUSES = ["all", "open", "in_progress", "resolved"] as const;
  const pending = scheduledMessages.filter((m) => m.status === "pending");
  const linked = !!(activeThread?.linked_contact_id || activeThread?.linked_org_id || activeThread?.linked_lead_id || activeThread?.linked_deal_id);
  const chip = (on: boolean) => cn(
    "inline-flex min-h-9 items-center gap-1.5 whitespace-nowrap rounded-lg border px-2.5 text-xs transition-colors md:min-h-7",
    "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-[rgb(var(--focus)/0.7)]",
    on ? "border-fg bg-fg text-surface" : "border-line bg-surface text-fg hover:bg-surface-2");

  if (perms.loaded && !canView) {
    return (
      <FullHeightPage className="bg-canvas">
        <EmptyState className="flex-1" icon={Mail} title="No access to the CRM email inbox"
          description="Reading the CRM inbox needs the CRM view permission (all or own). Ask an admin if you need it." />
      </FullHeightPage>
    );
  }

  return (
    <FullHeightPage className="bg-canvas">
      {/* Thread list */}
      <div className={cn("w-full shrink-0 flex-col border-r border-line bg-surface md:flex md:w-80",
        activeThread ? "hidden" : "flex")}>
        <div className="border-b border-line px-4 py-3">
          <div className="mb-3 flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Inbox className="size-4 text-fg-muted" aria-hidden />
              <h1 className="text-base font-semibold text-fg">Email inbox</h1>
              {total > 0 && (
                <span className="rounded-full bg-surface-2 px-1.5 py-0.5 text-xs font-medium text-fg-muted tabular">{total}</span>
              )}
            </div>
            {canSendEmail && (
              <IconButton size="sm" icon={RefreshCw} label="Sync now" onClick={handleSync} disabled={syncing}
                className={syncing ? "[&_svg]:animate-spin" : undefined} />
            )}
          </div>
          {syncMsg && (
            <p role="status" className={cn("mb-3 rounded-lg px-2.5 py-1.5 text-xs",
              syncMsg.ok ? "bg-tone-blue-soft text-tone-blue" : "bg-tone-red-soft text-tone-red")}>
              {syncMsg.text}
            </p>
          )}
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)}
            placeholder="Search threads" aria-label="Search threads" className="mb-3" />
          {/* Inbox / Sent */}
          <div className="mb-2 grid grid-cols-2 gap-1 rounded-lg bg-surface-2 p-1" role="group" aria-label="Mailbox">
            {(["inbox", "sent"] as const).map((b) => (
              <button key={b} type="button" aria-pressed={box === b} onClick={() => { setBox(b); closeThread(); }}
                className={cn("flex min-h-9 items-center justify-center gap-1.5 rounded-md text-xs font-medium transition-colors md:min-h-7",
                  box === b ? "bg-surface text-fg shadow-card" : "text-fg-muted hover:text-fg")}>
                {b === "inbox" ? <Inbox className="size-3" aria-hidden /> : <Send className="size-3" aria-hidden />}
                {b === "inbox" ? "Inbox" : "Sent"}
              </button>
            ))}
          </div>
          {multiInbox && (
            <Select aria-label="Inbox" value={inboxFilter} onChange={(e) => { setInboxFilter(e.target.value); closeThread(); }}
              className="mb-2 text-[13px] md:h-8"
              options={[{ value: "", label: "All inboxes" }, ...mailboxes.map((m) => ({ value: m.id, label: m.address }))]} />
          )}
          {selected.size > 0 ? (
            <BulkActionBar
              count={allMatching ? total : selected.size}
              matchingTotal={total}
              allMatching={allMatching}
              onSelectAllMatching={inboxFilter ? undefined : () => setAllMatching(true)}
              onClear={clearSelection}
              busy={bulkBusy}
              onAction={runBulk}
              actions={[
                { key: "read", label: "Read", icon: MailOpen },
                { key: "unread", label: "Unread", icon: Mail },
                { key: "status", label: "Status", icon: Flag,
                  options: [{ value: "open", label: "Open" }, { value: "in_progress", label: "Active" },
                            { value: "resolved", label: "Resolved" }] },
                { key: "assign", label: "Assign", icon: UserCheck, search: searchPeople,
                  searchPlaceholder: "Type a colleague's name", clearOption: { value: "", label: "Unassign" } },
                ...(canDelete ? [{ key: "delete", label: "Delete", icon: Trash2,
                  confirm: (n: number) => `Delete ${n} ${n === 1 ? "conversation" : "conversations"}? They move to the mailbox Trash.` }] : []),
              ]}
            />
          ) : (
          /* Status filters */
          <div className="flex flex-wrap items-center gap-1" role="group" aria-label="Filters">
            <Checkbox aria-label="Select all loaded conversations" disabled={!threads.length}
              checked={false} onChange={() => setSelected(new Set(threads.map((t) => t.id)))}
              className="mr-1" />
            {STATUSES.map((s) => (
              <button key={s} type="button" aria-pressed={statusFilter === s} onClick={() => setStatusFilter(s)}
                className={chip(statusFilter === s)}>
                {s === "all" ? "All" : s === "in_progress" ? "Active" : s.charAt(0).toUpperCase() + s.slice(1)}
              </button>
            ))}
            {/* Combines with the status and Inbox/Sent choice above */}
            <button type="button" onClick={() => setUnreadOnly((v) => !v)} aria-pressed={unreadOnly}
              title={unreadOnly ? "Showing unread conversations only" : "Show unread conversations only"}
              className={cn(chip(unreadOnly), "ml-auto")}>
              <span className={cn("size-1.5 rounded-full", unreadOnly ? "bg-surface" : "bg-brand")} aria-hidden />
              Unread
            </button>
            {assignedMe && (
              <button type="button" onClick={() => setAssignedMe(false)} title="Show every conversation again"
                className={chip(true)}>
                Assigned to me <X className="size-3" aria-hidden />
              </button>
            )}
          </div>
          )}
          {bulkNotice && <p role="status" className="mt-1.5 text-xs text-fg-muted">{bulkNotice}</p>}
        </div>

        {/* Thread list (room at the bottom for the docked bulk bar on phones) */}
        <div className="flex-1 overflow-y-auto pb-[var(--bulk-bar-h,0px)] md:pb-0">
          {loading ? (
            <div className="space-y-3 p-4" aria-busy>
              {[0, 1, 2, 3, 4].map((i) => (
                <div key={i} className="space-y-1.5">
                  <Skeleton className="h-3.5 w-1/2" />
                  <Skeleton className="h-3 w-5/6" />
                </div>
              ))}
            </div>
          ) : threads.length === 0 ? (
            <EmptyState icon={Inbox}
              title={unreadOnly ? "No unread threads" : "No threads found"}
              description={unreadOnly ? "You're all caught up." : "Sync to fetch new emails."} />
          ) : threads.map((thread, i) => {
            const isActive = activeThread?.id === thread.id;
            const isSel = selected.has(thread.id) || allMatching;
            return (
            <div key={thread.id} className={cn("group flex items-start border-b border-line-soft transition-colors",
                isActive ? "bg-brand/[0.07] shadow-[inset_2px_0_0_rgb(var(--brand))]"
                  : isSel ? "bg-brand/[0.035]" : i % 2 === 1 ? "bg-surface-2/70 hover:bg-brand/[0.035]" : "hover:bg-brand/[0.035]")}>
            <Checkbox aria-label="Select conversation" checked={allMatching || selected.has(thread.id)}
              onChange={() => toggleSelected(thread.id)}
              className={cn("ml-3 mt-3.5 transition-opacity",
                selected.size > 0 ? "opacity-100" : "focus-within:opacity-100 group-hover:opacity-100 [@media(hover:hover)]:opacity-0")} />
            <button
              type="button"
              onClick={() => openThread(thread)}
              aria-current={isActive || undefined}
              className="min-w-0 flex-1 py-3 pl-2 pr-4 text-left"
            >
              <div className="mb-1 flex items-start justify-between gap-2">
                <div className="flex min-w-0 items-center gap-2">
                  {!thread.is_read && (
                    <span className="size-2 shrink-0 rounded-full bg-brand" aria-label="Unread" />
                  )}
                  <span className={cn("truncate text-[13px]", !thread.is_read ? "font-semibold text-fg" : "font-medium text-fg-muted")}>
                    {thread.last_sender_name || thread.last_sender_email || "Unknown"}
                  </span>
                </div>
                <span className="shrink-0 text-xs text-fg-subtle tabular">{timeAgo(thread.last_message_at)}</span>
              </div>
              <p className={cn("mb-1.5 truncate text-[13px]", !thread.is_read ? "font-medium text-fg" : "text-fg-muted")}>
                {thread.subject}
              </p>
              <div className="flex items-center gap-1.5">
                <StatusPill status={thread.status} label={STATUS_LABEL[thread.status] ?? thread.status} className="h-5 px-2 text-[11px]" />
                {thread.linked_contact_name && (
                  <span className="truncate text-xs text-fg-muted">{thread.linked_contact_name}</span>
                )}
                {thread.linked_org_name && !thread.linked_contact_name && (
                  <span className="truncate text-xs text-fg-muted">{thread.linked_org_name}</span>
                )}
                {multiInbox && thread.inbox && <Pill className="h-5 px-2 text-[11px]">{thread.inbox}</Pill>}
                <span className="ml-auto text-xs text-fg-subtle tabular" title="Messages">{thread.message_count}</span>
              </div>
            </button>
            </div>
            );
          })}
          {!loading && threads.length > 0 && canLoadMore && (
            <div className="p-2">
              <Button variant="ghost" size="sm" className="w-full" onClick={loadMore} loading={loadingMore}>
                {loadingMore ? "Loading older emails" : "Load older emails"}
              </Button>
            </div>
          )}
        </div>
      </div>

      {/* Thread detail */}
      <div ref={detailRef} className={cn("min-w-0 flex-1 flex-col bg-canvas md:flex", activeThread ? "flex" : "hidden")}>
        {!activeThread ? (
          <EmptyState className="flex-1" icon={Mail} title="Select a thread to read"
            description="Emails to the CRM inboxes appear here."
            action={canSendEmail ? (
              <Button onClick={handleSync} loading={syncing}>
                {!syncing && <RefreshCw aria-hidden />}
                {syncing ? "Syncing" : "Sync now"}
              </Button>
            ) : undefined} />
        ) : (
          <>
            {/* Thread header */}
            <div className="flex shrink-0 flex-col gap-2 border-b border-line bg-surface px-3 py-2 md:flex-row md:items-start md:justify-between md:gap-4 md:px-6 md:py-3">
              <div className="flex min-w-0 items-start gap-2">
                <IconButton icon={ArrowLeft} label="Back to threads" onClick={closeThread} className="-ml-1 md:hidden" />
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <h2 className="flex-1 truncate text-base font-semibold text-fg">{activeThread.subject}</h2>
                    <StatusPill status={activeThread.status} label={STATUS_LABEL[activeThread.status] ?? activeThread.status} />
                    {multiInbox && activeThread.inbox && <Pill title="The inbox this conversation is in">{activeThread.inbox}</Pill>}
                  </div>
                  <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
                    {activeThread.assigned_to_name && (
                      <span className="flex items-center gap-1 text-xs text-fg-muted" title="Assigned with the bulk Assign action">
                        <UserCheck className="size-3" aria-hidden /> Assigned to {activeThread.assigned_to_name}
                      </span>
                    )}
                    {activeThread.linked_contact_name && (
                      <span className="flex items-center gap-1 text-xs text-fg-muted">
                        <User className="size-3" aria-hidden /> {activeThread.linked_contact_name}
                      </span>
                    )}
                    {activeThread.linked_org_name && (
                      <span className="flex items-center gap-1 text-xs text-fg-muted">
                        <Building2 className="size-3" aria-hidden /> {activeThread.linked_org_name}
                      </span>
                    )}
                  </div>
                </div>
              </div>

              <div className="flex shrink-0 flex-wrap items-center gap-1.5 md:justify-end">
                {canDelete && (confirmDelete ? (
                  <span className="flex items-center gap-1">
                    <Button size="sm" variant="danger" onClick={deleteConversation} loading={deleting}>Delete conversation</Button>
                    <Button size="sm" variant="ghost" onClick={() => setConfirmDelete(false)}>Cancel</Button>
                  </span>
                ) : (
                  <IconButton size="sm" icon={Trash2} label="Delete conversation (every email moves to the mailbox's Trash)"
                    onClick={() => setConfirmDelete(true)} className="hover:text-tone-red" />
                ))}
                <IconButton size="sm" icon={MailOpen} label="Mark as unread (also on the mail server)" onClick={markUnread} />
                <FollowUpButton compact entityType="email_thread" entityId={activeThread.id}
                  subject={activeThread.linked_contact_name || activeThread.subject || "this thread"} />
                <Select
                  aria-label="Thread status"
                  value={activeThread.status}
                  onChange={(e) => changeStatus(e.target.value)}
                  className="w-36 text-[13px] md:h-7"
                  options={[
                    { value: "open", label: "Open" },
                    { value: "in_progress", label: "In progress" },
                    { value: "resolved", label: "Resolved" },
                  ]}
                />
                <Button size="sm" onClick={() => setShowLinkModal(true)}>
                  <Link2 aria-hidden />
                  {linked ? "Relink" : "Link to CRM"}
                </Button>
                <IconButton size="sm" variant="secondary" icon={User} label="CRM context" onClick={() => setPanelOpen(true)} className="lg:hidden" />
              </div>
            </div>

            {/* Messages */}
            <div className="flex-1 overflow-y-auto px-3 py-4 md:px-6 md:py-6">
              {deleteError && (
                <p role="alert" className="mb-3 rounded-lg border border-tone-red/30 bg-tone-red-soft px-3 py-2 text-xs text-tone-red">{deleteError}</p>
              )}
              {loadingThread ? (
                <div className="space-y-4" aria-busy>
                  {[0, 1, 2].map((i) => <Skeleton key={i} className={cn("h-20", i % 2 ? "ml-auto w-2/3" : "w-3/4")} />)}
                </div>
              ) : (activeThread.messages || []).length === 0 ? (
                <p className="py-10 text-center text-sm text-fg-muted">No messages loaded.</p>
              ) : (
                (activeThread.messages || []).map((msg) => (
                  <MessageBubble key={msg.id} msg={msg} onDelete={canDelete ? deleteOneEmail : undefined} />
                ))
              )}
              <div ref={messagesEndRef} />
            </div>

            {/* Scheduled emails panel */}
            {pending.length > 0 && (
              <div className="shrink-0 space-y-2 border-t border-line bg-tone-orange-soft px-5 py-3">
                <p className="flex items-center gap-1.5 text-xs font-semibold text-tone-orange">
                  <Clock className="size-3" aria-hidden /> Scheduled
                </p>
                {pending.map((m) => (
                  <div key={m.id} className="flex items-center gap-2 rounded-lg border border-line bg-surface px-3 py-2">
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-xs font-medium text-fg">{m.subject || "(No subject)"}</p>
                      <p className="text-xs text-fg-muted">
                        <span className="tabular">{new Date(m.scheduled_at).toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}</span>
                        <span className="ml-1.5 text-fg-subtle">To: {m.recipient}</span>
                      </p>
                    </div>
                    <Button size="sm" variant="ghost" onClick={() => { setEditingScheduled(m); setShowScheduleModal(true); }}>Edit</Button>
                    <IconButton size="sm" icon={X} label="Cancel scheduled email" onClick={() => cancelScheduledEmail(m.id)} />
                  </div>
                ))}
              </div>
            )}

            {/* Reply bar: the composer stays folded until Reply, Reply all, Forward or R */}
            {activeThread.status !== "resolved" && canSendEmail && (
              <div hidden={barHidden} className={barHidden ? "hidden" : cn("flex shrink-0 flex-wrap items-center gap-2 border-t border-line bg-surface px-3 py-2 md:px-6",
                composerOpen && "md:hidden", replyOpen && "max-md:hidden")}>
                {(["reply", "all", "forward"] as const).map((m) => {
                  const Icon = MODE_ICON[m];
                  return (
                    <Button key={m} size="sm" variant={m === "reply" ? "primary" : "secondary"} onClick={() => openComposer(m)}>
                      <Icon aria-hidden /> {MODE_LABEL[m]}
                    </Button>
                  );
                })}
                {hasDraft && <span className="text-xs text-fg-muted">Unsent {MODE_LABEL[replyMode].toLowerCase()} draft</span>}
                <span className="ml-auto hidden text-xs text-fg-subtle lg:inline">Press R to reply</span>
              </div>
            )}
            {activeThread.status !== "resolved" && canSendEmail && composerOpen && !popped && (
              <div role="separator" aria-orientation="horizontal" aria-label="Resize the reply"
                aria-valuemin={COMPOSER_FRAC.min * 100} aria-valuemax={COMPOSER_FRAC.max * 100} aria-valuenow={Math.round(composerFrac * 100)}
                tabIndex={0} onPointerDown={startResize} onKeyDown={nudgeResize}
                title="Drag to resize the reply"
                className="group hidden h-2 shrink-0 cursor-row-resize touch-none items-center justify-center border-t border-line bg-surface hover:bg-surface-2 focus-visible:bg-surface-2 focus-visible:outline-none md:flex">
                <span className="h-0.5 w-10 rounded-full bg-line-strong group-hover:bg-fg-subtle" aria-hidden />
              </div>
            )}
            {activeThread.status !== "resolved" && canSendEmail && composerOpen && popped && (
              <div className="fixed inset-0 z-40 hidden bg-black/40 md:block" onClick={() => setPopped(false)} aria-hidden />
            )}
            {activeThread.status !== "resolved" && canSendEmail && (
              <div ref={composerRef} data-reply-composer
                role={popped ? "dialog" : undefined} aria-modal={popped || undefined} aria-label={MODE_LABEL[replyMode]}
                style={{ "--composer-h": `${composerFrac * 100}%` } as React.CSSProperties}
                hidden={composerHidden}
                className={composerHidden ? "hidden" : cn("flex shrink-0 flex-col bg-surface p-4 md:px-4 md:py-2",
                  composerOpen ? "md:flex" : "md:hidden",
                  popped
                    ? "md:fixed md:inset-x-0 md:bottom-[5vh] md:top-[5vh] md:z-50 md:mx-auto md:w-[min(64rem,calc(100vw-2rem))] md:rounded-xl md:border md:border-line md:p-4 md:shadow-pop"
                    : "md:h-[var(--composer-h)] md:min-h-[180px]",
                  replyOpen
                    ? "max-md:fixed max-md:inset-0 max-md:z-40 max-md:border-0 max-md:p-3 max-md:pb-[calc(0.75rem+env(safe-area-inset-bottom))]"
                    : "max-md:hidden")}>
                <div className="mb-3 flex items-center gap-2 md:hidden">
                  <IconButton icon={ArrowLeft} label="Close reply" onClick={collapseComposer} className="-ml-1" />
                  <div className="min-w-0 flex-1">
                    <p className="text-base font-semibold text-fg">{MODE_LABEL[replyMode]}</p>
                    <p className="truncate text-xs text-fg-muted">{activeThread.subject}</p>
                  </div>
                </div>
                <div className="mb-2 hidden flex-wrap items-center gap-1.5 md:flex">
                  <div className="flex flex-wrap items-center gap-1" role="group" aria-label="Reply type">
                    {(["reply", "all", "forward"] as const).map((m) => {
                      const Icon = MODE_ICON[m];
                      return (
                        <button key={m} type="button" aria-pressed={replyMode === m} onClick={() => openComposer(m)} className={chip(replyMode === m)}>
                          <Icon className="size-3" aria-hidden /> {MODE_LABEL[m]}
                        </button>
                      );
                    })}
                  </div>
                  <span className="ml-auto hidden text-xs text-fg-subtle lg:inline">Esc to fold</span>
                  <IconButton size="sm" icon={popped ? Minimize2 : Maximize2} className={cn(!popped && "max-lg:ml-auto")}
                    label={popped ? "Dock the reply under the thread" : "Pop out the reply for a long message"}
                    onClick={() => setPopped((v) => !v)} />
                  <IconButton size="sm" icon={ChevronDown} label="Fold the reply (Esc)" onClick={collapseComposer} />
                </div>
                <div {...replyDrop}
                  className={cn("flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border transition-colors",
                    replyDrag ? "border-brand ring-2 ring-brand/30" : "border-line-strong focus-within:border-brand focus-within:ring-2 focus-within:ring-brand/20")}>
                  <ReplyAddressFields threadId={activeThread.id} mailboxes={mailboxes} value={addressing} onChange={setAddressing}
                    autoLoad={false} summary names={addressNames} expanded={headerOpen} onExpandedChange={setHeaderOpen}
                    className="shrink-0" />
                  <div className="min-h-0 flex-1 overflow-y-auto"
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) handleSend();
                    }}>
                    <RichTextEditor value={replyText} onChange={setReplyText}
                      placeholder={replyMode === "forward" ? "Add a note (Ctrl+Enter to send)" : "Write a reply (Ctrl+Enter to send)"}
                      minHeight={72} />
                  </div>

                  {/* Signature (added in full when sent) and the forwarded message, folded to a line each */}
                  {signatureHtml && (
                    <div className="shrink-0 border-t border-line-soft px-4 py-1">
                      <button type="button" aria-expanded={sigOpen} onClick={() => setSigOpen((v) => !v)}
                        title="Added under your text when you send"
                        className="flex min-h-7 w-full min-w-0 items-center gap-1.5 text-left text-xs text-fg-muted transition-colors hover:text-fg">
                        <ChevronRight className={cn("size-3 shrink-0 transition-transform", sigOpen && "rotate-90")} aria-hidden />
                        <span className="truncate">{signatureName}</span>
                      </button>
                      {sigOpen && (
                        <div className="max-h-48 overflow-auto pb-1 pl-4 text-sm"
                          dangerouslySetInnerHTML={{ __html: sanitizeEmailHtml(signatureHtml) }} />
                      )}
                    </div>
                  )}
                  {quoteHtml && (
                    <div className="shrink-0 border-t border-line-soft px-4 py-1">
                      <button type="button" aria-expanded={quoteOpen} onClick={() => setQuoteOpen((v) => !v)}
                        title="Sent under your signature"
                        className="flex min-h-7 w-full min-w-0 items-center gap-1.5 text-left text-xs text-fg-muted transition-colors hover:text-fg">
                        <ChevronRight className={cn("size-3 shrink-0 transition-transform", quoteOpen && "rotate-90")} aria-hidden />
                        <span className="truncate">Forwarded message</span>
                      </button>
                      {quoteOpen && (
                        <div className="max-h-48 overflow-auto pb-1 pl-4 text-sm">
                          <EmailBody html={quoteHtml} title="Forwarded message" />
                        </div>
                      )}
                    </div>
                  )}

                  {/* Attachment preview */}
                  {replyFiles.length > 0 && (
                    <div className="flex shrink-0 flex-wrap gap-2 px-4 py-2">
                      {replyFiles.map((f, i) => (
                        <div key={i} className="flex items-center gap-1.5 rounded-lg bg-surface-2 px-3 py-1.5 text-xs">
                          <Paperclip className="size-3 text-fg-muted" aria-hidden />
                          <span className="max-w-[120px] truncate text-fg">{f.name}</span>
                          <button type="button" aria-label={`Remove ${f.name}`} onClick={() => setReplyFiles((fs) => fs.filter((_, j) => j !== i))}
                            className="text-fg-subtle transition-colors hover:text-fg">
                            <X className="size-3" aria-hidden />
                          </button>
                        </div>
                      ))}
                    </div>
                  )}

                  <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-t border-line px-4 py-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <Button size="sm" variant="ghost" onClick={() => fileInputRef.current?.click()}>
                        <Paperclip aria-hidden />
                        Attach
                      </Button>
                      <ReplyPromptButton onGenerate={suggestReply} currentDraft={typedBody} />
                      <input
                        ref={fileInputRef} type="file" multiple className="hidden"
                        onChange={(e) => setReplyFiles((fs) => [...fs, ...Array.from(e.target.files || [])])}
                      />
                      {sendError && (
                        <span role="alert" className="flex items-center gap-1 text-xs text-tone-red">
                          <AlertCircle className="size-3" aria-hidden /> {sendError}
                        </span>
                      )}
                    </div>
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="hidden text-xs text-fg-subtle xl:inline">Ctrl+Enter</span>
                      <Button size="sm" onClick={() => { setEditingScheduled(null); setShowScheduleModal(true); }}
                        disabled={!canSend} title="Schedule send">
                        <Clock aria-hidden />
                        Schedule
                      </Button>
                      <Button size="sm" variant="primary" onClick={handleSend} loading={sending} disabled={!canSend}>
                        {!sending && <Send aria-hidden />}
                        {sending ? "Sending" : replyMode === "forward" ? "Send forward" : "Send reply"}
                      </Button>
                    </div>
                  </div>
                </div>
              </div>
            )}

            {activeThread.status === "resolved" && (
              <div className="flex shrink-0 items-center gap-2 border-t border-line bg-surface-2 px-6 py-3">
                <CheckCircle2 className="size-4 text-tone-blue" aria-hidden />
                <p className="text-sm font-medium text-fg">Thread resolved.</p>
                <Button size="sm" variant="ghost" className="ml-auto" onClick={() => changeStatus("open")}>Reopen</Button>
              </div>
            )}
          </>
        )}
      </div>

      {activeThread && (
        <CrmContextPanel
          channel="email"
          threadId={activeThread.id}
          contactId={activeThread.linked_contact_id}
          orgId={activeThread.linked_org_id}
          displayName={activeThread.last_sender_name}
          email={activeThread.last_sender_email}
          onChange={load}
          open={panelOpen}
          onClose={() => setPanelOpen(false)}
        />
      )}

      {showLinkModal && activeThread && (
        <LinkModal
          thread={activeThread}
          onClose={() => setShowLinkModal(false)}
          onLinked={(update) => {
            setActiveThread((prev) => prev ? { ...prev, ...update } : prev);
            setThreads((ts) => ts.map((t) => t.id === activeThread.id ? { ...t, ...update } : t));
          }}
        />
      )}

      {/* Schedule Send Modal */}
      {showScheduleModal && activeThread && accessToken && (
        <ScheduleSendModal
          channel="email"
          chatId={activeThread.id}
          recipient={splitAddresses(addressing.to)[0] || activeThread.last_sender_email || ""}
          body={editingScheduled ? editingScheduled.body : composedHtml}
          subject={editingScheduled ? (editingScheduled.subject || undefined) : (addressing.subject.trim() || `Re: ${activeThread.subject || "(No Subject)"}`)}
          editingId={editingScheduled?.id}
          accessToken={accessToken}
          onScheduled={(msg) => {
            if (editingScheduled) {
              setScheduledMessages((prev) => prev.map((m) => m.id === editingScheduled.id ? msg : m));
            } else {
              setScheduledMessages((prev) => [...prev, msg]);
              setReplyText("");
              setReplyFiles([]);
              setQuoteHtml("");
              setPopped(false);
              setComposerOpen(false);
              setReplyParam(null);
            }
          }}
          onClose={() => { setShowScheduleModal(false); setEditingScheduled(null); }}
        />
      )}
    </FullHeightPage>
  );
}
