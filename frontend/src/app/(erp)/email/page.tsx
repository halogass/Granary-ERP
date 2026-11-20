"use client";

import { useEffect, useState, useCallback } from "react";
import EmailBody from "@/components/email/EmailBody";
import { emailSignatureApi, emailApi } from "@/lib/api";
import RichTextEditor from "@/components/ui/RichTextEditor";
import { extractApiError } from "@/lib/apiError";
import {
  Mail, Inbox, Send, Star, Trash2, Plus, Search,
  RefreshCw, Paperclip, ChevronLeft, X, ArrowUpRight, Download,
  Settings, AtSign, Archive, ShieldAlert, Pencil, MailOpen, Contact, ArrowLeft, ChevronDown,
} from "lucide-react";
import CrmContextPanel from "@/components/crm/CrmContextPanel";
import BulkActionBar from "@/components/common/BulkActionBar";
import {
  Button, Checkbox, CountBadge, Dialog, EmptyState, Field, FieldGrid, IconButton, Input, Skeleton,
  FullHeightPage,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { openOrSaveFile, saveFile } from "@/lib/safeFile";
import { useUrlParam } from "@/hooks/useUrlParam";

interface EmailAccount {
  id: string; email_address: string; display_name?: string; provider?: string;
  last_synced_at?: string; last_sync_error?: string | null;
  username?: string; imap_host?: string; imap_port?: number; imap_use_ssl?: boolean;
  smtp_host?: string; smtp_port?: number; smtp_use_tls?: boolean;
}

const FOLDER_META: Record<string, { label: string; icon: typeof Inbox }> = {
  INBOX:   { label: "Inbox",   icon: Inbox },
  SENT:    { label: "Sent",    icon: Send },
  DRAFTS:  { label: "Drafts",  icon: Mail },
  ARCHIVE: { label: "Archive", icon: Archive },
  SPAM:    { label: "Spam",    icon: ShieldAlert },
  TRASH:   { label: "Trash",   icon: Trash2 },
};
interface EmailMessage {
  id: string;
  account_id: string;
  subject?: string;
  from_address: string;
  from_name?: string;
  to_address: string;
  body_text?: string;
  body_html?: string;
  is_read: boolean;
  is_starred: boolean;
  has_attachments: boolean;
  attachments?: EmailAttachment[];
  date_received: string;
  linked_contact_id?: string | null;
}

interface EmailAttachment {
  id: string;
  filename: string;
  content_type?: string;
  size_bytes?: number;
}

function timeLabel(date: string) {
  const d = new Date(date);
  const now = new Date();
  const diff = now.getTime() - d.getTime();
  if (diff < 60000) return "just now";
  if (diff < 3600000) return `${Math.floor(diff / 60000)}m`;
  if (d.toDateString() === now.toDateString()) return d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
  if (diff < 7 * 86400000) return d.toLocaleDateString("en-GB", { weekday: "short" });
  return d.toLocaleDateString("en-GB", { day: "numeric", month: "short" });
}

const PAGE_SIZE = 50;

export default function EmailPage() {
  const [accounts, setAccounts] = useState<EmailAccount[]>([]);
  const [activeAccount, setActiveAccount] = useState<EmailAccount | null>(null);
  const [messages, setMessages] = useState<EmailMessage[]>([]);
  const [activeMsg, setActiveMsg] = useState<EmailMessage | null>(null);
  const [crmPanelOpen, setCrmPanelOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [canLoadMore, setCanLoadMore] = useState(true);
  const [syncing, setSyncing] = useState(false);
  const [search, setSearch] = useState("");
  const [folder, setFolder] = useState("INBOX");
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [folders, setFolders] = useState<string[]>(["INBOX"]);
  const [editingId, setEditingId] = useState<string | null>(null);

  const [showAddAccount, setShowAddAccount] = useState(false);
  const [showCompose, setShowCompose] = useState(false);
  const [accountForm, setAccountForm] = useState({
    email_address: "", display_name: "", username: "", password: "",
    imap_host: "", imap_port: "993", imap_use_ssl: true,
    smtp_host: "", smtp_port: "587", smtp_use_tls: true,
  });
  const [addError, setAddError] = useState("");
  const [composeForm, setComposeForm] = useState({ to: "", subject: "", body: "", account_id: "" });
  const [addingSaving, setAddingSaving] = useState(false);
  const [sendingSaving, setSendingSaving] = useState(false);
  // Phones show one pane at a time (folders, list, reader) and compose full screen; the open
  // message, the folders pane and the composer live in the URL so the phone Back button closes them.
  const [msgParam, setMsgParam] = useUrlParam("msg");
  const [panelParam, setPanelParam] = useUrlParam("panel");
  const [composeParam, setComposeParam] = useUrlParam("compose");
  const foldersOpen = panelParam === "folders";
  const closeMsg = useCallback(() => { setActiveMsg(null); setMsgParam(null); }, [setMsgParam]);
  const closeCompose = useCallback(() => { setShowCompose(false); setComposeParam(null); }, [setComposeParam]);

  useEffect(() => {
    emailApi.listAccounts().then((r) => {
      const accs: EmailAccount[] = r.data;
      setAccounts(accs);
      if (accs.length > 0) setActiveAccount(accs[0]);
    }).catch(() => {});
  }, []);

  const loadInbox = useCallback(async (account: EmailAccount, fldr: string) => {
    setLoading(true);
    setActiveMsg(null);
    setMsgParam(null);
    setCanLoadMore(true);
    try {
      const res = await emailApi.getInbox(account.id, {
        folder: fldr, unread_only: unreadOnly || undefined, limit: PAGE_SIZE, offset: 0 });
      const items = res.data.items ?? res.data;
      setMessages(items);
      if (items.length < PAGE_SIZE) setCanLoadMore(true); // may still have older history on the server
    } catch { setMessages([]); } finally { setLoading(false); }
  }, [unreadOnly, setMsgParam]);

  // Page further back: first drain any not-yet-shown rows from the DB, and when the
  // DB is exhausted reach into IMAP for older history (predating the first sync).
  const loadMore = useCallback(async () => {
    if (!activeAccount || loadingMore) return;
    setLoadingMore(true);
    try {
      const params = { folder, unread_only: unreadOnly || undefined, limit: PAGE_SIZE, offset: messages.length };
      let res = await emailApi.getInbox(activeAccount.id, params);
      let items: EmailMessage[] = res.data.items ?? res.data;
      if (items.length === 0) {
        const b = await emailApi.syncOlder(activeAccount.id, folder);
        if ((b.data?.stored ?? 0) > 0) {
          res = await emailApi.getInbox(activeAccount.id, params);
          items = res.data.items ?? res.data;
        }
        if (!b.data?.has_more) setCanLoadMore(items.length > 0);
      }
      if (items.length > 0) setMessages((prev) => [...prev, ...items]);
    } catch { /* keep the button available to retry */ } finally { setLoadingMore(false); }
  }, [activeAccount, folder, unreadOnly, messages.length, loadingMore]);

  // Load the available folders for the active account.
  useEffect(() => {
    if (!activeAccount) return;
    setFolder("INBOX");
    emailApi.getFolders(activeAccount.id)
      .then((r) => setFolders(r.data?.length ? r.data : ["INBOX"]))
      .catch(() => setFolders(["INBOX"]));
  }, [activeAccount]);

  // Show what we have at once, then sync this folder with the server (new mail and read/unread
  // changes made in Thunderbird or webmail) and show the result, so Sent is never left empty
  // just because nobody pressed Sync while it was open.
  useEffect(() => {
    if (!activeAccount) return;
    let alive = true;
    loadInbox(activeAccount, folder);
    emailApi.syncAccount(activeAccount.id, folder)
      .then(() => setTimeout(async () => {
        if (!alive) return;
        try {   // quiet refresh: keep the open message and the scroll position
          const res = await emailApi.getInbox(activeAccount.id, {
            folder, unread_only: unreadOnly || undefined, limit: PAGE_SIZE, offset: 0 });
          const fresh: EmailMessage[] = res.data.items ?? res.data;
          if (!alive) return;
          setMessages((prev) => {
            const ids = new Set(fresh.map((m) => m.id));
            return [...fresh, ...prev.filter((m) => !ids.has(m.id))];
          });
        } catch { /* keep what is shown */ }
      }, 3000))
      .catch(() => {});
    return () => { alive = false; };
  }, [activeAccount, folder, unreadOnly, loadInbox]);

  async function syncAccount() {
    if (!activeAccount) return;
    setSyncing(true);
    try { await emailApi.syncAccount(activeAccount.id, folder); setTimeout(() => loadInbox(activeAccount, folder), 2500); }
    catch { } finally { setSyncing(false); }
  }

  // Back/forward: the URL says which message is open.
  useEffect(() => {
    if (!msgParam) { if (activeMsg) setActiveMsg(null); return; }
    if (msgParam === activeMsg?.id) return;
    const known = messages.find((m) => m.id === msgParam);
    if (known) openMessage(known);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [msgParam]);
  useEffect(() => {
    if (!composeParam && showCompose) setShowCompose(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [composeParam]);

  async function openMessage(msg: EmailMessage) {
    setMsgParam(msg.id);
    setConfirmDelete(false);
    setDeleteError("");
    setActiveMsg(msg);
    if (!msg.is_read) setMessages((prev) => prev.map((m) => m.id === msg.id ? { ...m, is_read: true } : m));
    try {
      const res = await emailApi.getMessage(msg.id);   // full body + marks read + \Seen on server
      setActiveMsg(res.data);
    } catch { /* keep preview */ }
  }

  // Email attachments stream through the authenticated API proxy (MinIO-backed).
  async function downloadAttachment(att: EmailAttachment, open = false) {
    if (!activeMsg) return;
    try {
      const res = await emailApi.downloadAttachment(activeMsg.id, att.id);
      // Received files are untrusted: only safe types open in a tab, the rest is saved.
      if (open) openOrSaveFile(res.data, att.filename, att.content_type);
      else saveFile(res.data, att.filename, att.content_type);
    } catch { /* ignore */ }
  }

  async function toggleRead(msg: EmailMessage) {
    const next = !msg.is_read;
    setMessages((prev) => prev.map((m) => m.id === msg.id ? { ...m, is_read: next } : m));
    if (activeMsg?.id === msg.id) setActiveMsg({ ...activeMsg, is_read: next });
    try { await emailApi.markRead(msg.id, next); } catch { /* ignore */ }
  }

  function startEditAccount(acc: EmailAccount) {
    setEditingId(acc.id);
    setAccountForm({
      email_address: acc.email_address, display_name: acc.display_name || "",
      username: acc.username || "", password: "",
      imap_host: acc.imap_host || "", imap_port: String(acc.imap_port || 993), imap_use_ssl: acc.imap_use_ssl ?? true,
      smtp_host: acc.smtp_host || "", smtp_port: String(acc.smtp_port || 587), smtp_use_tls: acc.smtp_use_tls ?? true,
    });
    setAddError("");
    setShowAddAccount(true);
  }

  async function deleteAccount(acc: EmailAccount) {
    if (!confirm(`Remove ${acc.email_address}? Its synced messages will be deleted.`)) return;
    try {
      await emailApi.deleteAccount(acc.id);
      const r = await emailApi.listAccounts();
      setAccounts(r.data);
      if (activeAccount?.id === acc.id) { setActiveAccount(r.data[0] ?? null); setMessages([]); closeMsg(); }
    } catch { /* ignore */ }
  }

  function closeAccountModal() {
    setShowAddAccount(false);
    setEditingId(null);
    setAccountForm({
      email_address: "", display_name: "", username: "", password: "",
      imap_host: "", imap_port: "993", imap_use_ssl: true,
      smtp_host: "", smtp_port: "587", smtp_use_tls: true,
    });
  }

  async function addAccount(e: React.FormEvent) {
    e.preventDefault();
    setAddingSaving(true);
    setAddError("");
    const payload = {
      email_address: accountForm.email_address.trim(),
      display_name: accountForm.display_name.trim() || undefined,
      // Most providers use the full email address as the login username.
      username: accountForm.username.trim() || accountForm.email_address.trim(),
      imap_host: accountForm.imap_host.trim(),
      imap_port: parseInt(accountForm.imap_port) || 993,
      imap_use_ssl: accountForm.imap_use_ssl,
      smtp_host: accountForm.smtp_host.trim(),
      smtp_port: parseInt(accountForm.smtp_port) || 587,
      smtp_use_tls: accountForm.smtp_use_tls,
    };
    try {
      if (editingId) {
        // Only send the password if the user typed a new one (blank = unchanged).
        await emailApi.updateAccount(editingId, accountForm.password ? { ...payload, password: accountForm.password } : payload);
      } else {
        await emailApi.addAccount({ ...payload, password: accountForm.password });
      }
      const r = await emailApi.listAccounts();
      setAccounts(r.data);
      if (editingId && activeAccount?.id === editingId) setActiveAccount(r.data.find((a: EmailAccount) => a.id === editingId) ?? activeAccount);
      closeAccountModal();
    } catch (err) {
      setAddError(extractApiError(err, "Could not save the account. Check the fields and try again."));
    } finally { setAddingSaving(false); }
  }

  /** Open the composer with the user's signature already in the body, so what they see
   *  is what goes out, and they can delete it for a one-line reply. */
  const openCompose = useCallback(async (prefill?: { to?: string; subject?: string }) => {
    setComposeForm({
      to: prefill?.to ?? "", subject: prefill?.subject ?? "",
      body: "", account_id: activeAccount?.id ?? "",
    });
    setShowCompose(true);
    setComposeParam("1");
    try {
      const res = await emailSignatureApi.render();
      const sig = res.data?.html || "";
      if (sig) setComposeForm((f) => ({ ...f, body: `<p><br></p>${sig}` }));
    } catch { /* no signature configured, compose normally */ }
  }, [activeAccount?.id, setComposeParam]);

  async function sendEmail(e: React.FormEvent) {
    e.preventDefault();
    setSendingSaving(true);
    try {
      // The API takes a recipient LIST and HTML; the form holds a string and rich text.
      const recipients = composeForm.to.split(",").map((a) => a.trim()).filter(Boolean);
      const plain = composeForm.body
        .replace(/<br\s*\/?>(?=)/gi, "\n")
        .replace(/<\/(p|div|tr|table)>/gi, "\n")
        .replace(/<[^>]+>/g, "")
        .replace(/\n{3,}/g, "\n\n")
        .trim();
      await emailApi.sendEmail({
        account_id: composeForm.account_id || activeAccount?.id,
        to: recipients,
        subject: composeForm.subject,
        body_html: composeForm.body,
        body_text: plain,
      });
      closeCompose();
      setComposeForm({ to: "", subject: "", body: "", account_id: "" });
    } finally { setSendingSaving(false); }
  }

  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deletingMsg, setDeletingMsg] = useState(false);
  const [deleteError, setDeleteError] = useState("");

  /** Delete like a mail client: to the server's Trash, or, from Trash, gone for good. */
  async function deleteMessage(msg: EmailMessage) {
    setDeletingMsg(true);
    setDeleteError("");
    try {
      await emailApi.deleteMessage(msg.id);
      setMessages((prev) => prev.filter((m) => m.id !== msg.id));
      closeMsg();
      setConfirmDelete(false);
    } catch (e) {
      setDeleteError((e as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Could not delete this email.");
    } finally {
      setDeletingMsg(false);
    }
  }

  async function toggleStar(msg: EmailMessage) {
    setMessages((prev) => prev.map((m) => m.id === msg.id ? { ...m, is_starred: !m.is_starred } : m));
    try { await emailApi.starMessage(msg.id); } catch { /* revert */ }
  }

  // Bulk selection
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [allMatching, setAllMatching] = useState(false);
  const [matchingTotal, setMatchingTotal] = useState<number | undefined>();
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkNotice, setBulkNotice] = useState<string | null>(null);

  const clearSelection = useCallback(() => { setSelected(new Set()); setAllMatching(false); }, []);
  // A different view is a different selection.
  useEffect(() => { clearSelection(); }, [activeAccount, folder, unreadOnly, clearSelection]);
  // How many match this view on the server, for "select all N" (not while a local search is typed:
  // that filter only exists in the browser).
  useEffect(() => {
    if (!activeAccount || selected.size === 0 || search) { setMatchingTotal(undefined); return; }
    emailApi.countInbox(activeAccount.id, { folder, unread_only: unreadOnly || undefined })
      .then((r) => setMatchingTotal(r.data.total)).catch(() => setMatchingTotal(undefined));
  }, [activeAccount, folder, unreadOnly, search, selected.size > 0]); // eslint-disable-line react-hooks/exhaustive-deps

  function toggleSelected(id: string) {
    setAllMatching(false);
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }

  async function runBulk(action: string) {
    if (!activeAccount) return;
    setBulkBusy(true);
    setBulkNotice(null);
    const count = allMatching ? (matchingTotal ?? selected.size) : selected.size;
    try {
      const res = await emailApi.bulk({
        action: action as "delete" | "read" | "unread" | "star" | "unstar",
        ...(allMatching
          ? { filter: { account_id: activeAccount.id, folder, unread_only: unreadOnly || undefined } }
          : { ids: [...selected] }),
      });
      const failed = (res.data.failed ?? []).length;
      const done = res.data.affected ?? count;
      const verb = { delete: folder === "TRASH" ? "deleted forever" : "moved to Trash", read: "marked read",
                     unread: "marked unread", star: "starred", unstar: "unstarred" }[action] ?? "updated";
      setBulkNotice(`${done} ${done === 1 ? "email" : "emails"} ${verb}${failed ? `, ${failed} failed` : ""}.`);
      clearSelection();
      await loadInbox(activeAccount, folder);
    } catch (e) {
      setBulkNotice(extractApiError(e, "The action could not be completed"));
    } finally {
      setBulkBusy(false);
      setTimeout(() => setBulkNotice(null), 5000);
    }
  }

  const filteredMessages = messages.filter((m) =>
    !search || m.subject?.toLowerCase().includes(search.toLowerCase()) ||
    m.from_address.toLowerCase().includes(search.toLowerCase()) ||
    m.from_name?.toLowerCase().includes(search.toLowerCase())
  );

  const chip = (on: boolean) => cn(
    "inline-flex min-h-9 items-center gap-1.5 whitespace-nowrap rounded-lg border px-2.5 text-xs transition-colors disabled:opacity-40 md:min-h-7",
    "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-[rgb(var(--focus)/0.7)]",
    on ? "border-fg bg-fg text-surface" : "border-line bg-surface text-fg hover:bg-surface-2");
  const setAcc = (k: keyof typeof accountForm) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setAccountForm({ ...accountForm, [k]: e.target.value });

  return (
    <FullHeightPage className="bg-canvas">

      {/* Folders and accounts */}
      <div className={cn("w-full shrink-0 flex-col border-r border-line bg-surface md:flex md:w-56",
        foldersOpen && !activeMsg ? "flex" : "hidden")}>
        <div className="flex items-center justify-between px-4 py-3">
          <div className="flex items-center gap-2">
            <IconButton icon={ArrowLeft} label="Back to messages" onClick={() => setPanelParam(null)} className="-ml-2 md:hidden" />
            <Mail className="size-4 text-fg-muted" aria-hidden />
            <h1 className="text-base font-semibold text-fg">Email</h1>
          </div>
          <IconButton size="sm" variant="primary" icon={Plus} label="Compose" onClick={() => openCompose()} />
        </div>

        <nav className="flex-1 overflow-y-auto px-2" aria-label="Folders">
          {folders.map((key) => {
            const meta = FOLDER_META[key] ?? { label: key, icon: Mail };
            const Icon = meta.icon;
            const active = folder === key;
            const count = key === "INBOX" ? messages.filter(m => !m.is_read).length : 0;
            return (
              <button key={key} type="button" onClick={() => { setFolder(key); setPanelParam(null); }} aria-current={active || undefined}
                className={cn("mb-0.5 flex min-h-11 w-full items-center gap-2.5 rounded-lg px-3 text-sm transition-colors md:min-h-8",
                  active ? "bg-brand/[0.07] font-medium text-fg shadow-[inset_2px_0_0_rgb(var(--brand))]" : "text-fg-muted hover:bg-surface-2 hover:text-fg")}>
                <Icon className={cn("size-3.5", active && "text-brand-ink")} aria-hidden />
                <span className="flex-1 text-left text-[13px]">{meta.label}</span>
                {active && count > 0 && <CountBadge count={count} />}
              </button>
            );
          })}

          {accounts.length > 0 && (
            <div className="mt-4">
              <p className="mb-1 px-3 text-xs font-medium text-fg-muted">Accounts</p>
              {accounts.map((acc) => {
                const on = activeAccount?.id === acc.id;
                return (
                <div key={acc.id}
                  className={cn("group mb-0.5 flex w-full items-center gap-1 rounded-lg pl-3 pr-1 text-[13px] transition-colors",
                    on ? "bg-brand/[0.07] text-fg" : "text-fg-muted hover:bg-surface-2 hover:text-fg")}>
                  <button type="button" onClick={() => { setActiveAccount(acc); setPanelParam(null); }} aria-current={on || undefined}
                    className="flex min-h-11 min-w-0 flex-1 items-center gap-2.5 md:min-h-8">
                    <span className={cn("flex size-5 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold",
                      on ? "bg-brand text-brand-on" : "bg-surface-2 text-fg-muted")} aria-hidden>
                      {(acc.email_address || "?").charAt(0).toUpperCase()}
                    </span>
                    <span className="flex-1 truncate text-left">{acc.display_name || acc.email_address}</span>
                  </button>
                  <span className="flex shrink-0 items-center transition-opacity focus-within:opacity-100 group-hover:opacity-100 [@media(hover:hover)]:opacity-0">
                    <IconButton size="sm" icon={Pencil} label="Edit account" onClick={() => startEditAccount(acc)} className="[&_svg]:size-3.5" />
                    <IconButton size="sm" icon={Trash2} label="Remove account" onClick={() => deleteAccount(acc)}
                      className="hover:text-tone-red [&_svg]:size-3.5" />
                  </span>
                </div>
                );
              })}
            </div>
          )}
        </nav>

        <div className="border-t border-line px-2 py-2">
          <Button variant="ghost" size="sm" className="w-full justify-start text-fg-muted" onClick={() => setShowAddAccount(true)}>
            <AtSign aria-hidden /> Add account
          </Button>
        </div>
      </div>

      {/* Message list */}
      <div className={cn("w-full shrink-0 flex-col border-r border-line bg-surface md:flex md:w-80",
        !foldersOpen && !activeMsg ? "flex" : "hidden")}>
        <div className="border-b border-line px-4 py-3">
          {/* Phones: the folder and account pane opens from here */}
          <div className="mb-2 flex items-center gap-2 md:hidden">
            <Button variant="ghost" className="-ml-2 min-w-0 flex-1 justify-start" onClick={() => setPanelParam("folders")}
              aria-label="Folders and accounts">
              <Mail aria-hidden />
              <span className="truncate font-semibold">{(FOLDER_META[folder] ?? { label: folder }).label}</span>
              <ChevronDown aria-hidden />
            </Button>
            <Button variant="primary" onClick={() => openCompose()}><Plus aria-hidden /> Compose</Button>
          </div>
          <div className="mb-2 flex items-center gap-2">
            <div className="flex-1">
              <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
                value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search email" aria-label="Search email" />
            </div>
            <IconButton icon={RefreshCw} label="Sync inbox" onClick={syncAccount} disabled={syncing}
              variant="secondary" className={syncing ? "[&_svg]:animate-spin" : undefined} />
          </div>
          {selected.size > 0 ? (
            <BulkActionBar
              count={allMatching ? (matchingTotal ?? selected.size) : selected.size}
              matchingTotal={matchingTotal}
              allMatching={allMatching}
              onSelectAllMatching={() => setAllMatching(true)}
              onClear={clearSelection}
              busy={bulkBusy}
              onAction={runBulk}
              actions={[
                { key: "read", label: "Read", icon: MailOpen },
                { key: "unread", label: "Unread", icon: Mail },
                { key: "star", label: "Star", icon: Star },
                { key: "unstar", label: "Unstar" },
                { key: "delete", label: folder === "TRASH" ? "Delete forever" : "Delete", icon: Trash2,
                  confirm: (n) => folder === "TRASH"
                    ? `Delete ${n} ${n === 1 ? "email" : "emails"} forever? This can't be undone.`
                    : `Delete ${n} ${n === 1 ? "email" : "emails"}? They move to Trash.` },
              ]}
            />
          ) : (
          <div className="flex items-center justify-between gap-2">
            <Checkbox aria-label="Select all loaded emails" disabled={!filteredMessages.length}
              checked={false} onChange={() => setSelected(new Set(filteredMessages.map((m) => m.id)))} />
            <p className="flex-1 truncate text-xs text-fg-muted">
              {activeAccount?.email_address ?? "Select an account"} · <span className="tabular">{filteredMessages.length}</span> {unreadOnly ? "unread" : "messages"}
            </p>
            <button type="button" onClick={() => setUnreadOnly((v) => !v)} aria-pressed={unreadOnly} disabled={!activeAccount}
              title={unreadOnly ? "Showing unread email only" : "Show unread email only"}
              className={cn(chip(unreadOnly), "shrink-0")}>
              <span className={cn("size-1.5 rounded-full", unreadOnly ? "bg-surface" : "bg-brand")} aria-hidden />
              Unread
            </button>
          </div>
          )}
          {bulkNotice && <p role="status" className="mt-1.5 text-xs text-fg-muted">{bulkNotice}</p>}
        </div>

        <div className="flex-1 overflow-y-auto pb-[var(--bulk-bar-h,0px)] md:pb-0">
          {!activeAccount ? (
            <EmptyState icon={Mail} title="No email account yet" description="Add an email account to get started."
              action={<Button size="sm" onClick={() => setShowAddAccount(true)}><AtSign aria-hidden /> Add account</Button>} />
          ) : loading ? (
            <div className="space-y-3 p-4" aria-busy aria-label="Loading inbox">
              {[0, 1, 2, 3, 4].map((i) => (
                <div key={i} className="space-y-1.5">
                  <Skeleton className="h-3.5 w-1/2" />
                  <Skeleton className="h-3 w-5/6" />
                </div>
              ))}
            </div>
          ) : filteredMessages.length === 0 ? (
            <EmptyState icon={Inbox}
              title={unreadOnly ? "No unread email here" : "Your inbox is empty"}
              description={unreadOnly ? "You're all caught up." : undefined} />
          ) : (
            filteredMessages.map((msg, i) => {
              const isActive = activeMsg?.id === msg.id;
              const isSel = selected.has(msg.id) || allMatching;
              return (
              <div key={msg.id} className={cn("group flex items-start border-b border-line-soft transition-colors",
                isActive ? "bg-brand/[0.07] shadow-[inset_2px_0_0_rgb(var(--brand))]"
                  : isSel ? "bg-brand/[0.035]" : i % 2 === 1 ? "bg-surface-2/70 hover:bg-brand/[0.035]" : "hover:bg-brand/[0.035]")}>
              <Checkbox aria-label="Select email" checked={allMatching || selected.has(msg.id)}
                onChange={() => toggleSelected(msg.id)}
                className={cn("ml-3 mt-4 transition-opacity",
                  selected.size > 0 ? "opacity-100" : "focus-within:opacity-100 group-hover:opacity-100 [@media(hover:hover)]:opacity-0")} />
              <button type="button" onClick={() => openMessage(msg)} aria-current={isActive || undefined}
                className="min-w-0 flex-1 py-3 pl-2 pr-4 text-left">
                <div className="mb-1 flex items-start justify-between gap-2">
                  <div className="flex min-w-0 items-center gap-2">
                    {!msg.is_read && <span className="mt-0.5 size-1.5 shrink-0 rounded-full bg-brand" aria-label="Unread" />}
                    <p className={cn("truncate text-[13px]", !msg.is_read ? "font-semibold text-fg" : "text-fg")}>
                      {msg.from_name ?? msg.from_address}
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    {msg.has_attachments && <Paperclip className="size-3 text-fg-subtle" aria-label="Has attachments" />}
                    {msg.is_starred && <Star className="size-3 fill-tone-orange text-tone-orange" aria-label="Starred" />}
                    <span className="text-xs text-fg-muted tabular">{timeLabel(msg.date_received)}</span>
                  </div>
                </div>
                <p className={cn("mb-0.5 truncate text-[13px]", !msg.is_read ? "font-medium text-fg" : "text-fg-muted")}>
                  {msg.subject ?? "(no subject)"}
                </p>
                <p className="truncate text-xs leading-relaxed text-fg-muted">{msg.body_text?.slice(0, 80) ?? ""}</p>
              </button>
              </div>
              );
            })
          )}
          {!loading && !search && messages.length > 0 && canLoadMore && (
            <button type="button" onClick={loadMore} disabled={loadingMore}
              className="min-h-11 w-full text-xs font-medium text-brand-ink transition-colors hover:bg-brand/[0.05] disabled:opacity-50">
              {loadingMore ? "Loading older emails" : "Load older emails"}
            </button>
          )}
        </div>
      </div>

      {/* Reader */}
      <div className={cn("min-w-0 flex-1 flex-col bg-surface md:flex", activeMsg ? "flex" : "hidden")}>
        {!activeMsg ? (
          <EmptyState icon={Mail} className="flex-1" title="Select an email" description="Choose a message from the list to read it." />
        ) : (
          <>
            <div className="flex shrink-0 flex-col-reverse gap-2 border-b border-line px-3 py-3 md:flex-row md:items-start md:justify-between md:gap-0 md:px-6 md:py-4">
              <div className="min-w-0 flex-1">
                <h2 className="mb-2 text-lg font-semibold leading-tight text-fg">{activeMsg.subject ?? "(no subject)"}</h2>
                <div className="flex items-center gap-3">
                  <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-surface-2 text-sm font-semibold text-fg-muted" aria-hidden>
                    {(activeMsg.from_name || activeMsg.from_address || "?").charAt(0).toUpperCase()}
                  </span>
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium text-fg">{activeMsg.from_name ?? activeMsg.from_address}</p>
                    <p className="truncate text-xs text-fg-muted">{activeMsg.from_address} · <span className="tabular">{new Date(activeMsg.date_received).toLocaleString("en-GB")}</span></p>
                  </div>
                </div>
              </div>
              <div className="flex flex-wrap items-center gap-0.5 md:ml-4">
                <IconButton icon={ArrowLeft} label="Back to messages" onClick={closeMsg} className="mr-auto md:hidden" />
                <IconButton icon={Contact} label="CRM context" onClick={() => setCrmPanelOpen((v) => !v)} aria-pressed={crmPanelOpen}
                  className={crmPanelOpen || activeMsg.linked_contact_id ? "bg-brand/[0.07] text-brand-ink" : undefined} />
                <IconButton icon={activeMsg.is_read ? Mail : MailOpen} label={activeMsg.is_read ? "Mark as unread" : "Mark as read"}
                  onClick={() => toggleRead(activeMsg)} />
                <IconButton icon={Star} label={activeMsg.is_starred ? "Unstar" : "Star"} aria-pressed={activeMsg.is_starred}
                  onClick={() => toggleStar(activeMsg)}
                  className={activeMsg.is_starred ? "text-tone-orange [&_svg]:fill-tone-orange" : undefined} />
                {confirmDelete ? (
                  <span className="flex items-center gap-1">
                    <Button size="sm" variant="danger" loading={deletingMsg} onClick={() => deleteMessage(activeMsg)}>
                      {folder === "TRASH" ? "Delete forever" : "Move to Trash"}
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => setConfirmDelete(false)}>Cancel</Button>
                  </span>
                ) : (
                  <IconButton icon={Trash2} label={folder === "TRASH" ? "Delete forever" : "Delete (moves to Trash)"}
                    onClick={() => { setConfirmDelete(true); setDeleteError(""); }} className="hover:text-tone-red" />
                )}
                <IconButton icon={ArrowUpRight} label="Open" />
                <IconButton icon={X} label="Close" onClick={closeMsg} className="max-md:hidden" />
              </div>
            </div>
            {deleteError && (
              <p role="alert" className="mx-3 mt-2 rounded-lg md:mx-6 bg-tone-red-soft px-3 py-2 text-xs text-tone-red">{deleteError}</p>
            )}

            <div className="flex-1 overflow-y-auto px-3 py-4 md:px-6 md:py-5">
              {activeMsg.body_html ? (
                <EmailBody key={activeMsg.id} html={activeMsg.body_html} sender={activeMsg.from_address}
                  trusted={folder === "SENT" || folder === "DRAFTS"} title={activeMsg.subject || "Email message"} />
              ) : (
                <pre className="whitespace-pre-wrap font-sans text-sm leading-relaxed text-fg">
                  {activeMsg.body_text ?? "Empty message"}
                </pre>
              )}

              {activeMsg.attachments && activeMsg.attachments.length > 0 && (
                <div className="mt-6 border-t border-line pt-4">
                  <p className="mb-2 flex items-center gap-1.5 text-xs font-medium text-fg-muted">
                    <Paperclip className="size-3" aria-hidden /> <span className="tabular">{activeMsg.attachments.length}</span> attachment{activeMsg.attachments.length > 1 ? "s" : ""}
                  </p>
                  <div className="space-y-1.5">
                    {activeMsg.attachments.map((att) => (
                      <div key={att.id} className="group flex items-center gap-3 rounded-lg border border-line bg-surface-2 px-3 py-2 transition-colors hover:bg-surface">
                        <button type="button" onClick={() => downloadAttachment(att, true)} title="Open" aria-label={`Open ${att.filename}`}
                          className="flex size-8 shrink-0 items-center justify-center rounded-lg border border-line bg-surface text-fg-muted">
                          <Paperclip className="size-4" aria-hidden />
                        </button>
                        <button type="button" onClick={() => downloadAttachment(att, true)} className="min-w-0 flex-1 text-left" title="Open">
                          <p className="truncate text-[13px] font-medium text-fg">{att.filename}</p>
                          {att.size_bytes ? <p className="text-xs text-fg-muted tabular">{(att.size_bytes / 1024).toFixed(1)} KB</p> : null}
                        </button>
                        <IconButton size="sm" icon={Download} label="Download" onClick={() => downloadAttachment(att, false)}
                          className="transition-opacity focus:opacity-100 group-hover:opacity-100 [@media(hover:hover)]:opacity-0" />
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>

            <div className="shrink-0 border-t border-line px-3 py-3 md:px-6">
              <Button className="max-md:w-full" onClick={() => { openCompose({ to: activeMsg.from_address, subject: `Re: ${activeMsg.subject}` }); }}>
                <Send aria-hidden /> Reply to {activeMsg.from_name ?? activeMsg.from_address}
              </Button>
            </div>
          </>
        )}
      </div>

      {/* CRM context panel (link a contact when a customer moves to a staff email) */}
      {activeMsg && (
        <CrmContextPanel
          channel="email"
          collapsible
          contactId={activeMsg.linked_contact_id ?? null}
          email={activeMsg.from_address}
          displayName={activeMsg.from_name ?? activeMsg.from_address}
          open={crmPanelOpen}
          onClose={() => setCrmPanelOpen(false)}
          onLinkContact={async (contactId) => {
            await emailApi.linkMessage(activeMsg.id, contactId);
            setActiveMsg((m) => (m ? { ...m, linked_contact_id: contactId } : m));
            setMessages((ms) => ms.map((x) => x.from_address === activeMsg.from_address ? { ...x, linked_contact_id: contactId } : x));
          }}
        />
      )}

      {/* Add / edit account */}
      {showAddAccount && (
        <Dialog open onOpenChange={(o) => { if (!o) { closeAccountModal(); setAddError(""); } }}
          title={editingId ? "Edit email account" : "Add email account"} description="Connect your IMAP/SMTP inbox."
          footer={<>
            <Button type="button" onClick={() => { closeAccountModal(); setAddError(""); }}>Cancel</Button>
            <Button type="submit" form="email-account-form" variant="primary" loading={addingSaving}>
              {editingId ? "Save changes" : "Add account"}
            </Button>
          </>}>
          <form id="email-account-form" onSubmit={addAccount} className="space-y-4">
            <FieldGrid cols={2}>
              <Field label="Email address" required>
                <Input type="email" disabled={!!editingId} value={accountForm.email_address} onChange={setAcc("email_address")} placeholder="you@example.com" />
              </Field>
              <Field label="Display name">
                <Input value={accountForm.display_name} onChange={setAcc("display_name")} placeholder="Support inbox" />
              </Field>
              <Field label="Username" hint="Defaults to the email address.">
                <Input value={accountForm.username} onChange={setAcc("username")} />
              </Field>
              <Field label="Password or app password" required={!editingId}>
                <Input type="password" value={accountForm.password} placeholder={editingId ? "Leave blank to keep current" : ""} onChange={setAcc("password")} />
              </Field>
              <Field label="IMAP host" required>
                <Input value={accountForm.imap_host} onChange={setAcc("imap_host")} placeholder="imap.gmail.com" />
              </Field>
              <div className="grid grid-cols-2 items-end gap-2">
                <Field label="IMAP port">
                  <Input numeric value={accountForm.imap_port} onChange={setAcc("imap_port")} />
                </Field>
                <Checkbox label="SSL" checked={accountForm.imap_use_ssl}
                  onChange={(e) => setAccountForm({ ...accountForm, imap_use_ssl: e.target.checked, imap_port: e.target.checked ? "993" : "143" })} />
              </div>
              <Field label="SMTP host" required>
                <Input value={accountForm.smtp_host} onChange={setAcc("smtp_host")} placeholder="smtp.gmail.com" />
              </Field>
              <div className="grid grid-cols-2 items-end gap-2">
                <Field label="SMTP port">
                  <Input numeric value={accountForm.smtp_port} onChange={setAcc("smtp_port")} />
                </Field>
                <Checkbox label="TLS" checked={accountForm.smtp_use_tls}
                  onChange={(e) => setAccountForm({ ...accountForm, smtp_use_tls: e.target.checked, smtp_port: e.target.checked ? "587" : "25" })} />
              </div>
            </FieldGrid>
            {addError && (
              <p role="alert" className="rounded-lg bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{addError}</p>
            )}
          </form>
        </Dialog>
      )}

      {/* Compose */}
      {showCompose && (
        <div role="dialog" aria-label="New message"
          className={cn("fixed z-50 flex flex-col overflow-hidden bg-surface-raised",
            "inset-0 pb-[env(safe-area-inset-bottom)]",
            "md:inset-auto md:bottom-6 md:right-6 md:w-[540px] md:max-w-[calc(100vw-2rem)] md:rounded-lg md:border md:border-line md:pb-0 md:shadow-pop")}>
          <div className="flex items-center justify-between border-b border-line px-4 py-2.5">
            <h2 className="text-sm font-semibold text-fg">New message</h2>
            <IconButton size="sm" icon={X} label="Close" onClick={closeCompose} />
          </div>
          <form onSubmit={sendEmail} className="flex min-h-0 flex-1 flex-col gap-3 p-4">
            <Input required type="email" value={composeForm.to} onChange={(e) => setComposeForm({ ...composeForm, to: e.target.value })}
              placeholder="To" aria-label="To" />
            <Input value={composeForm.subject} onChange={(e) => setComposeForm({ ...composeForm, subject: e.target.value })}
              placeholder="Subject" aria-label="Subject" />
            <div className="min-h-0 flex-1 overflow-y-auto md:max-h-[320px] md:flex-none">
              <RichTextEditor value={composeForm.body}
                onChange={(html) => setComposeForm((f) => ({ ...f, body: html }))}
                placeholder="Write your message" minHeight={160} />
            </div>
            <div className="flex items-center justify-between border-t border-line pt-3">
              <Button type="button" variant="ghost" size="sm" className="text-fg-muted">
                <Paperclip aria-hidden /> Attach
              </Button>
              <div className="flex items-center gap-2">
                <Button type="button" variant="ghost" size="sm" onClick={closeCompose}>Cancel</Button>
                <Button type="submit" variant="primary" size="sm" loading={sendingSaving}>
                  <Send aria-hidden /> Send
                </Button>
              </div>
            </div>
          </form>
        </div>
      )}
    </FullHeightPage>
  );
}
