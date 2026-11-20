"use client";
import ScheduleSendModal, { ScheduledMsg } from "@/components/crm/ScheduleSendModal";
import FollowUpButton from "@/components/crm/FollowUpButton";

import { useEffect, useLayoutEffect, useRef, useState, useCallback } from "react";
import { useSearchParams } from "next/navigation";
import dynamic from "next/dynamic";
import type { EmojiClickData, Theme } from "emoji-picker-react";
import { useSession } from "next-auth/react";
import { whatsappApi, aiApi } from "@/lib/api";
import { openTicketedSocket } from "@/lib/wsTicket";
import CrmContextPanel from "@/components/crm/CrmContextPanel";
import { previewLabel } from "@/lib/whatsappPreview";
import ReplyPromptButton from "@/components/ai/ReplyPromptButton";
import { useDropToInput } from "@/hooks/useFileDrop";
import { usePermissions } from "@/hooks/usePermissions";
import { useUrlParam } from "@/hooks/useUrlParam";
import BulkActionBar from "@/components/common/BulkActionBar";
import { useTheme } from "@/lib/theme";
import {
  Button, Checkbox, CountBadge, Dialog, EmptyState, Field, FieldGrid, IconButton, Input, Pill, Skeleton, Textarea, toast,
  FullHeightPage,
} from "@/components/ui";
import {
  applyAck, hasMoreFromHeaders, mergeLatestMessages, mergeOlderMessages, sameMessages, tickState,
  chatRetryDelay, failedChatList, staleChatList, type ChatListProblem,
} from "./messages";
import ChatListNotice from "./ChatListNotice";
import { isSafeImageType, isSafeVideoType, safeObjectUrl, saveFile } from "@/lib/safeFile";
import {
  Search, Send, RefreshCw, Wifi, WifiOff, QrCode,
  FileText, Image as ImageIcon, Download, MessageSquare,
  Check, CheckCheck, Users, X, Volume2, Clock, AlertTriangle, User as UserIcon,
  Smile, Paperclip, Reply, Forward, ArrowLeft, Building2, Trash2, Mic, ChevronDown, ArrowUp, Loader2,
} from "lucide-react";

// Client-only: the emoji picker touches `window`.
const EmojiPicker = dynamic(() => import("emoji-picker-react"), { ssr: false });

// WhatsApp media size limits (warn/block above these).
const MAX_MEDIA_BYTES = 16 * 1024 * 1024;   // image / video / audio
const MAX_DOC_BYTES = 100 * 1024 * 1024;    // documents
const WA_PAGE_SIZE = 50;                     // messages per page; "Load older" pages back by this
const CHAT_PAGE_SIZE = 50;                   // chats per page; grows on "Load older chats"

// ─── Types ────────────────────────────────────────────────────────────────────

interface WAStatus {
  status: string;
  number?: string;
  name?: string;
  is_connected: boolean;
  qr_code?: string;
}

interface WaChat {
  id: string;
  name: string;
  phone?: string;
  lastMessage: string;
  lastMessageFromMe: boolean;
  lastMessageType: string;
  timestamp: number;
  unreadCount: number;
  isGroup: boolean;
  avatarUrl?: string;
  crm_contact_id?: string;
  crm_contact_name?: string;
  crm_org_id?: string | null;
  crm_org_name?: string | null;
  /** One of our own staff (linked on their profile, or their employee number): never a customer. */
  staff_user_id?: string | null;
  staff_user_name?: string | null;
}

interface WaMessage {
  id: string;
  body: string;
  fromMe: boolean;
  timestamp: number;
  type: string;
  author?: string;
  hasMedia: boolean;
  media?: {
    mimetype: string;
    filename?: string;
    data: string; // base64 (only when the API inlines it)
  };
  // What the attachment is, without its bytes: the bytes come from /whatsapp/media.
  mediaMeta?: { mimetype?: string | null; filename?: string | null; size?: number | null } | null;
  // False when WhatsApp gave us no usable message id, so its media can't be fetched.
  addressable?: boolean;
  quoted?: { id: string; body: string; fromMe: boolean; author?: string | null } | null;
  // Our messages sent from the ERP: which colleague sent them (the "sent by" log).
  sentBy?: string | null;
  // Own messages: 0 pending, 1 sent, 2 delivered, 3 read, 4 played.
  ack?: number | null;
}

interface AttachDraft {
  data: string;        // base64 (no data: prefix)
  mimetype: string;
  filename: string;
  caption: string;
  previewUrl: string;  // full data URL for preview
  kind: "image" | "video" | "file";
}

interface WsEvent {
  type: "new_message" | "status_change" | "qr_update" | "pong" | "scheduled_message_sent"
    | "unread_count" | "message_ack";
  chatId?: string;
  body?: string;
  fromMe?: boolean;
  timestamp?: number;
  msgType?: string;
  status?: string;
  qr?: string;
  chatName?: string;
  fromName?: string;
  messageId?: string;
  scheduledMessageId?: string;
  unreadCount?: number;
  ack?: number | null;
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

function formatTimestamp(ts: number): string {
  if (!ts) return "";
  const d = new Date(ts * 1000);
  const now = new Date();
  const diff = now.getTime() - d.getTime();
  if (diff < 86400000) {
    return d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
  }
  if (diff < 604800000) {
    return d.toLocaleDateString("en-GB", { weekday: "short" });
  }
  return d.toLocaleDateString("en-GB", { day: "numeric", month: "short" });
}

function formatMessageTime(ts: number): string {
  return new Date(ts * 1000).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
}

function formatDateHeader(ts: number): string {
  const d = new Date(ts * 1000);
  const now = new Date();
  const diff = now.getTime() - d.getTime();
  if (diff < 86400000) return "Today";
  if (diff < 172800000) return "Yesterday";
  return d.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" });
}

const AVATAR_TONES = [
  "bg-tone-blue-soft text-tone-blue",
  "bg-tone-violet-soft text-tone-violet",
  "bg-tone-orange-soft text-tone-orange",
  "bg-tone-neutral-soft text-tone-neutral",
];

function ChatAvatar({ name, avatarUrl, size = "md", isGroup = false }: {
  name: string; avatarUrl?: string; size?: "sm" | "md" | "lg"; isGroup?: boolean;
}) {
  const [imgError, setImgError] = useState(false);
  const color = AVATAR_TONES[(name.charCodeAt(0) || 0) % AVATAR_TONES.length];
  const initials = name.split(" ").map((w) => w[0]).join("").slice(0, 2).toUpperCase();
  const sz = size === "sm" ? "w-8 h-8 text-xs" : size === "lg" ? "w-12 h-12 text-base" : "w-10 h-10 text-sm";

  if (avatarUrl && !imgError) {
    return (
      <div className={`${sz} rounded-full overflow-hidden shrink-0`}>
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={avatarUrl} alt={name} className="w-full h-full object-cover" onError={() => setImgError(true)} />
      </div>
    );
  }
  return (
    <div className={`${sz} ${color} rounded-full flex items-center justify-center font-semibold shrink-0`} aria-hidden>
      {isGroup ? <Users className="w-4 h-4" /> : initials}
    </div>
  );
}

function humanSize(bytes?: number | null) {
  if (!bytes) return "";
  const mb = bytes / 1024 / 1024;
  return mb >= 1 ? `${mb.toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/**
 * An attachment. The message list no longer carries base64 (it made every fetch huge and
 * WhatsApp usually can't resolve media on the first ask anyway); bytes are pulled per
 * item from /whatsapp/media, which retries until WhatsApp materialises them.
 */
function MediaBubble({ msg }: { msg: WaMessage }) {
  const meta = msg.mediaMeta || msg.media || null;
  const mimetype = meta?.mimetype || "";
  const filename = meta?.filename || "";
  // Received media is untrusted: only plain raster images and common video render inline.
  // SVG (script-capable) and everything else is a file to download.
  const isImage = isSafeImageType(mimetype);
  const isVideo = isSafeVideoType(mimetype);

  // Inlined base64 (transcript-style payloads) still renders directly, safe image types only.
  const inlineSrc = msg.media?.data && isSafeImageType(msg.media.mimetype)
    ? `data:${msg.media.mimetype.split(";")[0].trim().toLowerCase()};base64,${msg.media.data}` : "";

  const [objectUrl, setObjectUrl] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  // WhatsApp couldn't give this message an addressable id, so /media can't serve it.
  const unavailable = msg.addressable === false;

  const fetchBlob = useCallback(async () => {
    if (unavailable || objectUrl || loading) return objectUrl;
    setLoading(true);
    setError("");
    try {
      const res = await whatsappApi.getMedia(msg.id);
      const url = safeObjectUrl(res.data as Blob, mimetype);
      setObjectUrl(url);
      return url;
    } catch (e) {
      const blob = (e as { response?: { data?: Blob } })?.response?.data;
      let detail = "";
      try { detail = blob ? JSON.parse(await blob.text())?.detail : ""; } catch { /* not JSON */ }
      setError(detail || "Could not load this attachment");
      return "";
    } finally {
      setLoading(false);
    }
  }, [msg.id, objectUrl, loading, unavailable, mimetype]);

  // Images load when their bubble scrolls into view: opening a chat with 30 attachments
  // must not fire 30 downloads at once. Documents wait for a click.
  const holderRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!(isImage || isVideo) || inlineSrc || unavailable) return;
    const node = holderRef.current;
    if (!node || typeof IntersectionObserver === "undefined") { fetchBlob(); return; }
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        io.disconnect();
        fetchBlob();
      }
    }, { rootMargin: "200px" });
    io.observe(node);
    return () => io.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [msg.id, isImage, isVideo, inlineSrc, unavailable]);

  useEffect(() => () => { if (objectUrl) URL.revokeObjectURL(objectUrl); }, [objectUrl]);

  const src = inlineSrc || objectUrl;

  if (isImage || isVideo) {
    if (src) {
      return (
        <div className="max-w-[240px]" ref={holderRef}>
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={src} alt={filename || "media"} className="rounded-lg w-full object-cover max-h-48 cursor-pointer"
            onClick={() => { if (objectUrl) window.open(objectUrl, "_blank", "noopener"); }} />
          {filename && <p className="text-[11px] text-fg-muted mt-1 truncate">{filename}</p>}
        </div>
      );
    }
    return (
      <div ref={holderRef}
        className="flex items-center gap-2 text-xs text-fg-muted bg-surface-2 border border-line rounded-lg px-4 py-3 min-w-[160px]">
        {loading ? <RefreshCw className="w-4 h-4 animate-spin" aria-hidden /> : <ImageIcon className="w-4 h-4" aria-hidden />}
        <span>{loading ? "Loading image…" : unavailable ? "Image unavailable" : (error || "Image")}</span>
        {!loading && error && !unavailable && (
          <button type="button" onClick={fetchBlob} className="underline shrink-0 hover:text-fg">Retry</button>
        )}
      </div>
    );
  }

  const download = async () => {
    const url = objectUrl || (await fetchBlob());
    if (!url) return;
    const a = document.createElement("a");
    a.href = url;
    a.download = filename || "attachment";
    a.click();
  };

  return (
    <button type="button" onClick={download} disabled={loading || unavailable}
      className="flex items-center gap-3 bg-surface-2 border border-line enabled:hover:border-line-strong rounded-lg px-4 py-3 min-w-[180px] text-left w-full transition-colors disabled:opacity-70">
      {loading ? <RefreshCw className="w-5 h-5 shrink-0 animate-spin text-fg-muted" aria-hidden /> : <FileText className="w-5 h-5 shrink-0 text-fg-muted" aria-hidden />}
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-fg truncate">{filename || msg.body || "Document"}</p>
        <p className="text-xs text-fg-muted truncate">
          {unavailable
            ? "Unavailable: re-sync this chat on your phone"
            : error || [mimetype.split("/").pop()?.toUpperCase(),
                        humanSize(meta && "size" in meta ? meta.size : null)]
                .filter(Boolean).join(" · ") || "Tap to download"}
        </p>
      </div>
      <Download className="w-4 h-4 shrink-0 text-fg-muted" aria-hidden />
    </button>
  );
}

/** Delivery ticks under our own messages: clock, one tick sent, two delivered, blue two read. */
function Ticks({ msg }: { msg: WaMessage }) {
  const state = tickState(msg);
  if (state === "none") return null;
  if (state === "pending") return <Clock className="w-3 h-3 text-fg-subtle" aria-label="Sending" />;
  if (state === "sent") return <Check className="w-3 h-3 text-fg-subtle" aria-label="Sent" />;
  return (
    <CheckCheck className={`w-3 h-3 ${state === "read" ? "text-tone-blue" : "text-fg-subtle"}`}
      aria-label={state === "read" ? "Read" : "Delivered"} />
  );
}

/** The sign-off the server adds to what this user sends. Read-only: it can't be removed. */
function SignatureHint({ signature }: { signature: string }) {
  // WhatsApp shows `_text_` in italics; mirror that for the standard sign-off.
  const italic = /^_[^_][\s\S]*_$/.test(signature);
  const text = italic ? signature.slice(1, -1) : signature;
  return (
    <p className="mt-1.5 px-1 text-[11px] text-fg-subtle truncate" title="Added to every message you send">
      Will be signed: <span className={`text-fg-muted ${italic ? "italic" : ""}`}>{text}</span>
    </p>
  );
}

/** A small icon for a chat whose last message was not text. */
function LastTypeIcon({ type }: { type: string }) {
  const Icon = type === "image" ? ImageIcon : type === "document" ? FileText : type === "audio" || type === "ptt" ? Mic : Paperclip;
  return <Icon className="w-3 h-3 text-fg-subtle shrink-0" aria-hidden />;
}

/** Paging hints from a message page's headers (older known / ask the phone). A service that
 *  predates them sends neither, so fall back to "a full page means there may be more". */
function pagingFrom(res: { headers?: Record<string, unknown> }, count: number) {
  const more = res.headers?.["x-has-more"];
  const end = res.headers?.["x-history-end"];
  if (more === undefined && end === undefined) return { more: count >= WA_PAGE_SIZE, end: false };
  return { more: more === "1", end: end === "1" };
}

// ─── Main Component ───────────────────────────────────────────────────────────

export default function WhatsAppPage() {
  const { data: session } = useSession();
  const isAdmin = (session?.user as { is_superadmin?: boolean; is_system_admin?: boolean })?.is_superadmin
    || (session?.user as { is_superadmin?: boolean; is_system_admin?: boolean })?.is_system_admin;
  const accessToken = (session as { access_token?: string })?.access_token;

  const [status, setStatus] = useState<WAStatus | null>(null);
  // The sign-off the server adds to everything this user sends (read-only here).
  const [signature, setSignature] = useState<{ signature: string; name: string } | null>(null);
  useEffect(() => {
    whatsappApi.getSignature().then((r) => setSignature(r.data)).catch(() => setSignature(null));
  }, []);
  const [statusLoading, setStatusLoading] = useState(true);
  const [chats, setChats] = useState<WaChat[]>([]);
  const [chatsLoading, setChatsLoading] = useState(false);
  const [hasMoreChats, setHasMoreChats] = useState(false);
  const [loadingMoreChats, setLoadingMoreChats] = useState(false);
  const chatLimitRef = useRef(CHAT_PAGE_SIZE);   // grows as the user pages back through the inbox
  // fix-walist: a stale or failed list is said out loud and retried (5 s, 15 s, then every 30 s).
  const [chatsProblem, setChatsProblem] = useState<ChatListProblem | null>(null);
  const chatRetryRef = useRef<{ timer: ReturnType<typeof setTimeout> | null; attempt: number; closed: boolean }>({ timer: null, attempt: 0, closed: false });
  const loadChatsRef = useRef<() => void>(() => {});
  const [activeChat, setActiveChat] = useState<WaChat | null>(null);
  // Phones show one pane at a time; the open chat lives in the URL so the phone Back button closes it.
  const [chatParam, setChatParam] = useUrlParam("chat");
  const perms = usePermissions();
  // Hiding chats from the shared inbox is for the Owner (superadmin) and system admins only.
  const canDelete = perms.isAdmin;
  const [confirmHide, setConfirmHide] = useState(false);
  const [hiding, setHiding] = useState(false);
  const [hideError, setHideError] = useState("");
  const [messages, setMessages] = useState<WaMessage[]>([]);
  const [messagesLoading, setMessagesLoading] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [hasMoreWa, setHasMoreWa] = useState(true);
  // WhatsApp Web's local history is exhausted: older messages have to come from the phone.
  const [historyEnd, setHistoryEnd] = useState(false);
  const [historyNote, setHistoryNote] = useState("");
  const messagesRef = useRef<WaMessage[]>([]);
  const scrollBoxRef = useRef<HTMLDivElement>(null);
  const scrollRestoreRef = useRef<number | null>(null);   // distance from the bottom to keep
  const markReadTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);

  /** Draft a reply with the AI, following the user's direction (if any). Errors show in the panel. */
  async function suggestReply(instruction: string) {
    if (!activeChat) return;
    try {
      const res = await aiApi.suggestReply({ channel: "whatsapp", chat_id: activeChat.id,
                                             instruction: instruction || undefined });
      if (res.data?.draft) {
        setInput(res.data.draft);
        requestAnimationFrame(() => inputRef.current?.focus());
      }
    } catch (e) {
      const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      throw new Error(detail || "Couldn't suggest a reply. Check Settings → Nary & AI.");
    }
  }
  const [search, setSearch] = useState("");
  const [panelOpen, setPanelOpen] = useState(false);

  // Transcript export
  const [exportOpen, setExportOpen] = useState(false);
  const [exportFormat, setExportFormat] = useState<"pdf" | "txt">("pdf");
  const [exportFrom, setExportFrom] = useState("");
  const [exportTo, setExportTo] = useState("");
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState("");
  const [showQr, setShowQr] = useState(false);
  const [scheduledMessages, setScheduledMessages] = useState<ScheduledMsg[]>([]);
  const [showScheduleModal, setShowScheduleModal] = useState(false);
  const [editingScheduled, setEditingScheduled] = useState<ScheduledMsg | null>(null);

  // Emoji / attachments / reply / forward
  const [showEmoji, setShowEmoji] = useState(false);
  const [replyTo, setReplyTo] = useState<WaMessage | null>(null);
  const [attach, setAttach] = useState<AttachDraft | null>(null);
  const [attachSending, setAttachSending] = useState(false);
  const [attachError, setAttachError] = useState("");
  const [forwardMsg, setForwardMsg] = useState<WaMessage | null>(null);
  const [forwardSearch, setForwardSearch] = useState("");
  const [forwarding, setForwarding] = useState(false);
  const [notice, setNotice] = useState("");

  const wsRef = useRef<WebSocket | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  // Grow the message box with its text, whoever changed it: typing, an AI draft, an emoji, or a
  // message restored after a failed send (up to ~10 lines, then it scrolls).
  useLayoutEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 240)}px`;
  }, [input]);
  const pollRef = useRef<NodeJS.Timeout | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const { dragActive: waDrag, dropHandlers: waDrop } = useDropToInput(fileInputRef);
  // Per-chat message cache so reopening a chat shows instantly (no spinner/refetch flash).
  const msgCacheRef = useRef<Map<string, WaMessage[]>>(new Map());
  const openChatIdRef = useRef<string | null>(null);
  const lastScrollKeyRef = useRef<string>("");

  // Short notices (sent, forwarded, failed) go to the app's toasts.
  useEffect(() => {
    if (!notice) return;
    toast(notice);
    setNotice("");
  }, [notice]);
  const { resolved: resolvedTheme } = useTheme();

  // ── Status polling (while disconnected) ─────────────────────────────────────
  const fetchStatus = useCallback(async () => {
    try {
      const res = await whatsappApi.getStatus();
      setStatus(res.data);
      setStatusLoading(false);
      return res.data as WAStatus;
    } catch {
      setStatusLoading(false);
      return null;
    }
  }, []);

  useEffect(() => {
    fetchStatus().then((s) => {
      if (s?.is_connected) {
        loadChats();
      }
    });
  }, [fetchStatus]); // eslint-disable-line react-hooks/exhaustive-deps

  // Poll QR while not connected
  useEffect(() => {
    if (status?.is_connected) {
      if (pollRef.current) clearInterval(pollRef.current);
      return;
    }
    pollRef.current = setInterval(async () => {
      const s = await fetchStatus();
      if (s?.is_connected) {
        clearInterval(pollRef.current!);
        loadChats();
      }
    }, 3000);
    return () => { if (pollRef.current) clearInterval(pollRef.current); };
  }, [status?.is_connected, fetchStatus]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── WebSocket ────────────────────────────────────────────────────────────────
  useEffect(() => {
    if (!accessToken) return;

    const wsBase = process.env.NEXT_PUBLIC_API_URL?.replace(/^http/, "ws") ?? "ws://localhost:8000";
    // Stage 8: a single-use ticket opens the socket (no access token in the URL)
    return openTicketedSocket(`${wsBase}/api/v1/whatsapp/ws`, (ws) => {
      wsRef.current = ws;

      ws.onopen = () => {
        const ping = setInterval(() => ws.readyState === WebSocket.OPEN && ws.send(JSON.stringify({ type: "ping" })), 30000);
        ws.onclose = () => clearInterval(ping);
      };

      ws.onmessage = (e) => {
        try {
          const event: WsEvent = JSON.parse(e.data);

          if (event.type === "new_message" && event.chatId) {
            // Update chat list: bump timestamp and last message
            setChats((prev) => {
              const idx = prev.findIndex((c) => c.id === event.chatId);
              if (idx === -1) {
                // New chat: reload the list
                loadChats();
                return prev;
              }
              const updated = [...prev];
              updated[idx] = {
                ...updated[idx],
                lastMessage: event.body || "",
                lastMessageFromMe: event.fromMe ?? false,
                timestamp: event.timestamp ?? Math.floor(Date.now() / 1000),
                // The chat on screen is being read: its badge doesn't count up.
                unreadCount: event.fromMe || openChatIdRef.current === event.chatId
                  ? updated[idx].unreadCount : updated[idx].unreadCount + 1,
              };
              // Bubble to top
              const chat = updated.splice(idx, 1)[0];
              return [chat, ...updated];
            });

            // Append to active conversation
            setActiveChat((current) => {
              if (current?.id === event.chatId) {
                setMessages((prev) => {
                  const exists = prev.some((m) => m.id === event.messageId);
                  if (exists) return prev;
                  const incoming: WaMessage = {
                    id: event.messageId ?? `tmp-${Date.now()}`,
                    body: event.body ?? "",
                    fromMe: event.fromMe ?? false,
                    timestamp: event.timestamp ?? Math.floor(Date.now() / 1000),
                    type: event.msgType ?? "chat",
                    hasMedia: false,
                    ack: event.ack ?? null,
                  };
                  // WhatsApp echoes our own outbound messages back over the socket. If this
                  // is the echo of a message we just sent optimistically, take over that
                  // bubble instead of adding a second one next to it.
                  if (incoming.fromMe) {
                    const pending = prev.findIndex(
                      (m) => m.id.startsWith("opt-") && m.body === incoming.body,
                    );
                    if (pending !== -1) {
                      const copy = [...prev];
                      copy[pending] = { ...copy[pending], ...incoming };
                      return copy;
                    }
                  }
                  return [...prev, incoming];
                });
              }
              return current;
            });
          }

          // A message arriving in the chat on screen has been seen: tell the phone (debounced, and
          // only while the tab is actually visible).
          if (event.type === "new_message" && event.chatId && !event.fromMe
              && openChatIdRef.current === event.chatId && document.visibilityState === "visible") {
            const chatId = event.chatId;
            if (markReadTimerRef.current) clearTimeout(markReadTimerRef.current);
            markReadTimerRef.current = setTimeout(() => { whatsappApi.markRead(chatId).catch(() => {}); }, 1000);
          }

          // Read (or marked unread) on the phone: follow it, except for the chat on screen.
          if (event.type === "unread_count" && event.chatId) {
            const chatId = event.chatId;
            const onScreen = openChatIdRef.current === chatId;
            setChats((prev) => prev.map((c) => (c.id === chatId
              ? { ...c, unreadCount: onScreen ? 0 : Math.max(0, event.unreadCount ?? 0) } : c)));
          }

          // Delivered / read ticks for our own messages.
          if (event.type === "message_ack" && event.messageId && typeof event.ack === "number") {
            const { messageId, ack } = event;
            setMessages((prev) => applyAck(prev, messageId, ack));
            if (event.chatId) {
              const cached = msgCacheRef.current.get(event.chatId);
              if (cached) msgCacheRef.current.set(event.chatId, applyAck(cached, messageId, ack));
            }
          }

          if (event.type === "scheduled_message_sent" && event.chatId) {
            // Remove the just-sent scheduled message from the banner
            setScheduledMessages((prev) => prev.filter((m) => m.chat_id !== event.chatId || m.id !== event.scheduledMessageId));
            // Reload messages to show the sent message
            setActiveChat((current) => {
              if (current && current.id === event.chatId) {
                whatsappApi.getMessages(current.id, WA_PAGE_SIZE)
                  .then((r) => setMessages((prev) => mergeLatestMessages(prev, r.data))).catch(() => {});
              }
              return current;
            });
          }

          if (event.type === "status_change") {
            fetchStatus().then((s) => {
              if (s?.is_connected) loadChats();
            });
          }

          if (event.type === "qr_update") {
            setStatus((prev) => prev ? { ...prev, qr_code: event.qr, status: "waiting_for_qr", is_connected: false } : prev);
            if (isAdmin) setShowQr(true);
          }
        } catch { /* ignore parse errors */ }
      };
    });
  }, [accessToken]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Auto-scroll ──────────────────────────────────────────────────────────────
  // Scroll only when the chat changes (jump instantly) or a genuinely new message
  // arrives (smooth). A background refetch that returns the same messages won't scroll.
  useEffect(() => {
    const chatId = activeChat?.id ?? "";
    const lastId = messages.length ? messages[messages.length - 1].id : "";
    const key = `${chatId}:${lastId}`;
    if (key === lastScrollKeyRef.current) return;
    const chatChanged = !lastScrollKeyRef.current.startsWith(`${chatId}:`);
    lastScrollKeyRef.current = key;
    bottomRef.current?.scrollIntoView({ behavior: chatChanged ? "auto" : "smooth" });
  }, [messages, activeChat]);

  // Keep the per-chat cache in sync with the live message list (WS appends, sends, etc.).
  useEffect(() => {
    if (activeChat && messages.length) msgCacheRef.current.set(activeChat.id, messages);
  }, [messages, activeChat]);

  // ── Load chats ───────────────────────────────────────────────────────────────
  const loadChats = useCallback(async () => {
    const retry = chatRetryRef.current;
    if (retry.timer) { clearTimeout(retry.timer); retry.timer = null; }
    setChatsLoading(true);
    let problem: ChatListProblem | null = null;
    try {
      // Refetches at whatever width the user has paged out to, so a background reload
      // (a new message, a reconnect) doesn't collapse the inbox back to one page.
      const res = await whatsappApi.getConversations(chatLimitRef.current);
      setChats(res.data);
      setHasMoreChats(hasMoreFromHeaders(res, res.data.length, chatLimitRef.current));
      problem = staleChatList(res);
    } catch (err) {
      // fix-walist: never a silent empty list; the chats already shown stay.
      problem = failedChatList(err);
    } finally {
      setChatsLoading(false);
    }
    setChatsProblem(problem);
    if (problem?.retry && !retry.closed) {
      retry.timer = setTimeout(() => loadChatsRef.current(), chatRetryDelay(retry.attempt));
      retry.attempt += 1;
    } else {
      retry.attempt = 0;
    }
  }, []);
  loadChatsRef.current = loadChats;
  useEffect(() => {
    const retry = chatRetryRef.current;
    retry.closed = false;
    return () => {
      retry.closed = true;
      if (retry.timer) clearTimeout(retry.timer);
      retry.timer = null;
    };
  }, []);

  // ── Load older chats: widen the inbox window ─────────────────────────────────
  const loadMoreChats = useCallback(async () => {
    if (loadingMoreChats) return;
    setLoadingMoreChats(true);
    const next = chatLimitRef.current + CHAT_PAGE_SIZE;
    try {
      const res = await whatsappApi.getConversations(next);
      chatLimitRef.current = next;
      setChats(res.data);
      setHasMoreChats(hasMoreFromHeaders(res, res.data.length, next));
    } catch { /* keep current */ }
    finally { setLoadingMoreChats(false); }
  }, [loadingMoreChats]);

  // ── Scheduled messages ───────────────────────────────────────────────────────
  const fetchScheduled = useCallback(async (chatId: string) => {
    if (!accessToken) return;
    try {
      const res = await fetch(
        `/api/v1/crm/scheduled?channel=whatsapp&chat_id=${encodeURIComponent(chatId)}&status=pending`,
        { headers: { Authorization: `Bearer ${accessToken}` } },
      );
      if (res.ok) setScheduledMessages(await res.json());
    } catch { /* silent */ }
  }, [accessToken]);

  async function cancelScheduled(id: string) {
    if (!accessToken) return;
    await fetch(`/api/v1/crm/scheduled/${id}`, {
      method: "DELETE",
      headers: { Authorization: `Bearer ${accessToken}` },
    });
    setScheduledMessages((prev) => prev.filter((m) => m.id !== id));
  }

  // ── Open chat ────────────────────────────────────────────────────────────────
  const openChat = useCallback(async (chat: WaChat) => {
    setChatParam(chat.id);
    setConfirmHide(false);
    setHideError("");
    setActiveChat(chat);
    openChatIdRef.current = chat.id;
    setScheduledMessages([]);
    setHasMoreWa(false);
    setHistoryEnd(false);
    setHistoryNote("");

    // Clear unread badge
    setChats((prev) => prev.map((c) => c.id === chat.id ? { ...c, unreadCount: 0 } : c));

    // Show cached messages immediately (no spinner / flash); only show the loader the
    // first time a chat is opened.
    const cached = msgCacheRef.current.get(chat.id);
    if (cached && cached.length) {
      setMessages(cached);
      setMessagesLoading(false);
    } else {
      setMessages([]);
      setMessagesLoading(true);
    }
    // Not on phones: the keyboard would cover the conversation the moment it opens.
    if (window.matchMedia("(min-width: 768px)").matches) inputRef.current?.focus();

    // Background refresh: apply only if still on this chat and the data actually changed.
    try {
      const res = await whatsappApi.getMessages(chat.id, WA_PAGE_SIZE);
      const fresh: WaMessage[] = res.data;
      msgCacheRef.current.set(chat.id, fresh);
      if (openChatIdRef.current === chat.id) {
        const { more, end } = pagingFrom(res, fresh.length);
        setMessages((prev) => (sameMessages(prev, fresh) ? prev : fresh));
        setHasMoreWa(more);
        setHistoryEnd(end && !more);
      }
      whatsappApi.markRead(chat.id).catch(() => {});
      fetchScheduled(chat.id);
    } catch {
      // Don't blindly clear: a live WS message may have already landed in `messages` while
      // this fetch was in flight (common for contacts WhatsApp hasn't finished syncing yet,
      // whose history fetch reliably 502s). Only reset to empty if there's truly nothing to show.
      if (openChatIdRef.current === chat.id) {
        setMessages((prev) => (prev.length ? prev : []));
      }
    } finally {
      if (openChatIdRef.current === chat.id) setMessagesLoading(false);
    }
  }, [fetchScheduled, setChatParam]);

  const closeChat = useCallback(() => {
    setActiveChat(null);
    openChatIdRef.current = null;
    setChatParam(null);
  }, [setChatParam]);

  // The URL keeps the open chat: the phone Back button closes it, forward reopens it.
  useEffect(() => {
    if (!chatParam) {
      if (activeChat) { setActiveChat(null); openChatIdRef.current = null; }
      return;
    }
    if (chatParam === activeChat?.id) return;
    const chat = chats.find((c) => c.id === chatParam);
    if (chat) openChat(chat);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chatParam, chats.length > 0]);

  // ── Deep-link: open a chat from ?contact=<id> / ?phone=<number> (e.g. from a Lead card) ──
  const searchParams = useSearchParams();
  const autoOpenedRef = useRef(false);
  useEffect(() => {
    if (autoOpenedRef.current || !chats.length) return;
    const contactId = searchParams.get("contact");
    const phone = searchParams.get("phone");
    if (!contactId && !phone) return;
    const digits = (s?: string) => (s || "").replace(/\D/g, "");
    const phoneMatch = (a?: string, b?: string) => {
      const x = digits(a), y = digits(b);
      if (x.length < 8 || y.length < 8) return false;
      const n = Math.min(x.length, y.length, 10);
      return x.slice(-n) === y.slice(-n);   // suffix match (handles 08xx vs 62xx prefixes)
    };
    let match: WaChat | undefined;
    if (contactId) match = chats.find((c) => c.crm_contact_id === contactId);
    if (!match && phone) match = chats.find((c) => phoneMatch(c.phone, phone) || phoneMatch(c.id, phone));
    autoOpenedRef.current = true;
    if (match) openChat(match);
    else if (phone) setSearch(phone);   // no exact chat: narrow the list to help them find it
  }, [chats, searchParams, openChat]);

  // ── Refetch current chat messages (after media/forward) ──────────────────────
  const reloadMessages = useCallback(async () => {
    if (!activeChat) return;
    try {
      const res = await whatsappApi.getMessages(activeChat.id, WA_PAGE_SIZE);
      // Refresh the newest page; older pages the user loaded stay.
      setMessages((prev) => mergeLatestMessages(prev, res.data));
    } catch { /* keep current */ }
  }, [activeChat]);

  // ── Load older history: one page older than the oldest message on screen ─────
  useEffect(() => { messagesRef.current = messages; }, [messages]);

  // Prepending older messages must not move what the user is looking at.
  useLayoutEffect(() => {
    const box = scrollBoxRef.current;
    if (box && scrollRestoreRef.current !== null) {
      box.scrollTop = box.scrollHeight - scrollRestoreRef.current;
      scrollRestoreRef.current = null;
    }
  }, [messages]);

  /** Fetch the page before the oldest message shown; returns how many messages it added. */
  const fetchOlderPage = useCallback(async (chatId: string) => {
    const real = messagesRef.current.filter((m) => !m.id.startsWith("opt-"));
    const oldest = real.length ? Math.min(...real.map((m) => m.timestamp)) : Math.floor(Date.now() / 1000);
    // +1: include messages from the same second as the oldest one; duplicates are dropped.
    const res = await whatsappApi.getMessages(chatId, WA_PAGE_SIZE, oldest + 1);
    const older: WaMessage[] = res.data;
    if (openChatIdRef.current !== chatId) return 0;
    const { more, end } = pagingFrom(res, older.length);
    const added = mergeOlderMessages(messagesRef.current, older).length - messagesRef.current.length;
    if (added > 0) {
      const box = scrollBoxRef.current;
      if (box) scrollRestoreRef.current = box.scrollHeight - box.scrollTop;
      setMessages((prev) => mergeOlderMessages(prev, older));
    }
    setHasMoreWa(more);
    setHistoryEnd(end && !more);
    return added;
  }, []);

  const loadOlderMessages = useCallback(async () => {
    if (!activeChat || loadingOlder) return;
    setLoadingOlder(true);
    setHistoryNote("");
    try { await fetchOlderPage(activeChat.id); } catch { /* keep current */ }
    finally { setLoadingOlder(false); }
  }, [activeChat, loadingOlder, fetchOlderPage]);

  /** The linked phone holds history WhatsApp Web doesn't: ask it, then pick up what arrives. */
  const requestOlderFromPhone = useCallback(async () => {
    if (!activeChat || loadingOlder) return;
    const chatId = activeChat.id;
    setLoadingOlder(true);
    setHistoryNote("");
    try {
      const res = await whatsappApi.syncHistory(chatId);
      if (!res.data?.requested) {
        setHistoryEnd(false);
        setHistoryNote("No older messages on the phone.");
        return;
      }
      setHistoryNote("Requested from the phone. Older messages arrive shortly (the phone must be online).");
      for (const wait of [4000, 6000, 10000]) {
        await new Promise((r) => setTimeout(r, wait));
        if (openChatIdRef.current !== chatId) return;
        if (await fetchOlderPage(chatId) > 0) {
          setHistoryNote("");
          return;
        }
      }
      setHistoryNote("Nothing has arrived yet. Keep the phone online and try again in a minute.");
    } catch {
      setHistoryNote("Couldn't reach the phone. Try again in a moment.");
    } finally {
      setLoadingOlder(false);
    }
  }, [activeChat, loadingOlder, fetchOlderPage]);

  // ── Emoji insertion at the input caret ───────────────────────────────────────
  function insertEmoji(emoji: string) {
    const el = inputRef.current;
    if (!el) { setInput((v) => v + emoji); return; }
    const start = el.selectionStart ?? input.length;
    const end = el.selectionEnd ?? input.length;
    const next = input.slice(0, start) + emoji + input.slice(end);
    setInput(next);
    requestAnimationFrame(() => {
      el.focus();
      const pos = start + emoji.length;
      el.setSelectionRange(pos, pos);
    });
  }

  // ── Send message ─────────────────────────────────────────────────────────────
  async function handleSend(e: React.FormEvent) {
    e.preventDefault();
    if (!input.trim() || !activeChat || sending) return;
    const text = input.trim();
    const quoted = replyTo;
    setInput("");
    setReplyTo(null);
    setSending(true);

    // Optimistic
    const optimistic: WaMessage = {
      id: `opt-${Date.now()}`,
      body: text,
      fromMe: true,
      timestamp: Math.floor(Date.now() / 1000),
      type: "chat",
      hasMedia: false,
      quoted: quoted ? { id: quoted.id, body: quoted.body, fromMe: quoted.fromMe, author: quoted.author } : null,
    };
    setMessages((prev) => [...prev, optimistic]);

    try {
      const res = await whatsappApi.send({
        chat_id: activeChat.id,
        message: text,
        quoted_message_id: quoted?.id,
      });
      // Adopt the real message id so the socket echo is recognised as this message.
      // If the echo beat the HTTP response, it already took over the bubble: drop ours.
      const realId = res?.data?.messageId as string | undefined;
      // Show what was actually sent: the text with the server-added sign-off, and who sent it.
      const signedBody = (res?.data?.body as string | undefined) ?? text;
      const sentBy = (res?.data?.sentBy as string | undefined) ?? null;
      if (realId) {
        setMessages((prev) => {
          const already = prev.some((m) => m.id === realId);
          return prev.flatMap((m) =>
            m.id !== optimistic.id ? [m]
              : already ? []
              : [{ ...m, id: realId, body: signedBody, sentBy }],
          );
        });
      }
    } catch (e) {
      // The request can fail (proxy timeout) after WhatsApp actually sent it: the socket
      // echo will have claimed the bubble. Only treat it as failed if nothing arrived.
      let delivered = false;
      setMessages((prev) => {
        delivered = !prev.some((m) => m.id === optimistic.id);
        return delivered ? prev : prev.filter((m) => m.id !== optimistic.id);
      });
      if (!delivered) {
        setInput(text);        // restore on failure
        setReplyTo(quoted);
        // Say why: the message vanishing with no explanation is worse than the failure.
        const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
        setNotice(detail || "Message could not be sent");
      }
    } finally {
      setSending(false);
    }
  }

  // ── Attachments ──────────────────────────────────────────────────────────────
  function onPickFile(file: File) {
    setAttachError("");
    const mt = file.type || "application/octet-stream";
    const isMedia = mt.startsWith("image/") || mt.startsWith("video/") || mt.startsWith("audio/");
    const limit = isMedia ? MAX_MEDIA_BYTES : MAX_DOC_BYTES;
    if (file.size > limit) {
      setNotice(`File too large, max ${isMedia ? "16 MB" : "100 MB"}`);
      return;
    }
    const kind: AttachDraft["kind"] = mt.startsWith("image/") ? "image" : mt.startsWith("video/") ? "video" : "file";
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = String(reader.result || "");
      const base64 = dataUrl.includes(",") ? dataUrl.slice(dataUrl.indexOf(",") + 1) : dataUrl;
      setAttach({ data: base64, mimetype: mt, filename: file.name, caption: "", previewUrl: dataUrl, kind });
    };
    reader.onerror = () => setNotice("Could not read the file");
    reader.readAsDataURL(file);
  }

  async function runExport() {
    if (!activeChat || exporting) return;
    setExporting(true);
    setExportError("");
    try {
      const res = await whatsappApi.exportConversation({
        chat_id: activeChat.id,
        format: exportFormat,
        date_from: exportFrom || undefined,
        date_to: exportTo || undefined,
        chat_name: activeChat.crm_contact_name || activeChat.name,
      });
      // Filename comes from the backend's Content-Disposition.
      const disposition = String(res.headers?.["content-disposition"] || "");
      const match = disposition.match(/filename="?([^"]+)"?/);
      const url = URL.createObjectURL(res.data as Blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = match?.[1] || `whatsapp-export.${exportFormat}`;
      a.click();
      URL.revokeObjectURL(url);
      setExportOpen(false);
    } catch (e) {
      // The error body is a Blob here (responseType: "blob"): read the detail out of it.
      const blob = (e as { response?: { data?: Blob } })?.response?.data;
      let detail = "";
      try { detail = blob ? JSON.parse(await blob.text())?.detail : ""; } catch { /* not JSON */ }
      setExportError(detail || "Export failed. Check that WhatsApp is connected and try again.");
    } finally {
      setExporting(false);
    }
  }

  async function sendAttachment() {
    if (!attach || !activeChat || attachSending) return;
    setAttachSending(true);
    setAttachError("");
    const quoted = replyTo;
    try {
      await whatsappApi.sendMedia({
        chat_id: activeChat.id,
        data: attach.data,
        mimetype: attach.mimetype,
        filename: attach.filename,
        caption: attach.caption || undefined,
        quoted_message_id: quoted?.id,
      });
      setAttach(null);
      setReplyTo(null);
      await reloadMessages();
    } catch {
      setAttachError("Could not send the attachment. Try a smaller file or check the connection.");
    } finally {
      setAttachSending(false);
    }
  }

  // ── Forward ──────────────────────────────────────────────────────────────────
  async function doForward(chat: WaChat) {
    if (!forwardMsg || forwarding) return;
    setForwarding(true);
    try {
      await whatsappApi.forwardMessage({ message_id: forwardMsg.id, to_chat_id: chat.id });
      setForwardMsg(null);
      setForwardSearch("");
      setNotice("Message forwarded");
    } catch {
      setNotice("Forward failed");
    } finally {
      setForwarding(false);
    }
  }

  /** Hide the open chat from the list (WhatsApp itself is untouched). */
  async function hideChat() {
    if (!activeChat) return;
    const chat = activeChat;
    setHiding(true);
    setHideError("");
    try {
      await whatsappApi.hideChat(chat.id, chat.timestamp || 0);
      setChats((prev) => prev.filter((c) => c.id !== chat.id));
      closeChat();
      setConfirmHide(false);
    } catch (e) {
      setHideError((e as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Could not delete the chat.");
    } finally {
      setHiding(false);
    }
  }

  // ── Filtered chats ───────────────────────────────────────────────────────────
  // ── Bulk selection (loaded chats only: the list comes from WhatsApp page by page) ──
  const [selectedChats, setSelectedChats] = useState<Set<string>>(new Set());
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkNotice, setBulkNotice] = useState<string | null>(null);
  const clearChatSelection = useCallback(() => setSelectedChats(new Set()), []);

  function toggleChat(id: string) {
    setSelectedChats((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }

  async function runChatBulk(action: string) {
    const picked = chats.filter((c) => selectedChats.has(c.id));
    setBulkBusy(true);
    setBulkNotice(null);
    try {
      const res = await whatsappApi.bulk({
        action: action as "hide" | "read",
        chats: picked.map((c) => ({ id: c.id, through: c.timestamp })),
      });
      const failed: { id: string }[] = res.data.failed ?? [];
      const done = new Set(picked.map((c) => c.id).filter((id) => !failed.some((f) => f.id === id)));
      if (action === "hide") {
        setChats((prev) => prev.filter((c) => !done.has(c.id)));
        if (activeChat && done.has(activeChat.id)) closeChat();
      } else {
        setChats((prev) => prev.map((c) => done.has(c.id) ? { ...c, unreadCount: 0 } : c));
      }
      const n = done.size;
      setBulkNotice(`${n} ${n === 1 ? "chat" : "chats"} ${action === "hide" ? "deleted from the list" : "marked read"}` +
        `${failed.length ? `, ${failed.length} failed` : ""}.`);
      clearChatSelection();
    } catch (e) {
      const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      setBulkNotice(detail || "The action could not be completed");
    } finally {
      setBulkBusy(false);
      setTimeout(() => setBulkNotice(null), 6000);
    }
  }

  const filteredChats = chats.filter((c) => {
    const q = search.toLowerCase();
    return (
      c.name.toLowerCase().includes(q) ||
      (c.crm_contact_name?.toLowerCase().includes(q) ?? false) ||
      (c.crm_org_name?.toLowerCase().includes(q) ?? false) ||
      (c.phone?.includes(q) ?? false)
    );
  });

  // ── Group messages by date ───────────────────────────────────────────────────
  const groupedMessages = messages.reduce<{ date: string; items: WaMessage[] }[]>((acc, msg) => {
    const d = formatDateHeader(msg.timestamp);
    const last = acc[acc.length - 1];
    if (last?.date === d) { last.items.push(msg); }
    else { acc.push({ date: d, items: [msg] }); }
    return acc;
  }, []);

  const displayName = activeChat?.crm_contact_name || activeChat?.name || "";
  const totalUnread = chats.reduce((s, c) => s + c.unreadCount, 0);

  // ─── Render ────────────────────────────────────────────────────────────────
  return (
    <FullHeightPage className="bg-canvas">

      {/* ── Left: Chat list ─────────────────────────────────────────────── */}
      <div className={`w-full md:w-[320px] bg-surface md:flex flex-col shrink-0 border-r border-line ${activeChat ? "hidden" : "flex"}`}>

        {/* Header */}
        <div className="px-4 pt-4 pb-3 border-b border-line">
          <div className="flex items-center justify-between gap-2 mb-3">
            <div className="flex items-center gap-2 min-w-0">
              <div className="w-8 h-8 rounded-lg bg-brand/10 text-brand-ink flex items-center justify-center shrink-0">
                <MessageSquare className="w-4 h-4" aria-hidden />
              </div>
              <div className="min-w-0">
                <h1 className="text-sm font-semibold text-fg leading-tight">WhatsApp</h1>
                {totalUnread > 0 && (
                  <span className="text-[11px] font-medium text-brand-ink tabular">{totalUnread} unread</span>
                )}
              </div>
            </div>
            <div className="flex items-center gap-1">
              {/* Connection status */}
              <Pill tone={status?.is_connected ? "blue" : statusLoading ? "neutral" : "red"}
                icon={status?.is_connected ? Wifi : WifiOff}>
                {statusLoading ? "Checking" : status?.is_connected ? "Live" : "Off"}
              </Pill>
              {/* QR setup button (admins only) */}
              {isAdmin && !status?.is_connected && (
                <IconButton size="sm" icon={QrCode} label="Scan QR to connect" onClick={() => setShowQr(true)} />
              )}
              <IconButton size="sm" icon={RefreshCw} label="Refresh"
                className={chatsLoading ? "[&_svg]:animate-spin" : undefined}
                onClick={() => { fetchStatus(); loadChats(); }} />
            </div>
          </div>

          {/* Search */}
          <Input type="search" autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            leading={Search} value={search} onChange={(e) => setSearch(e.target.value)}
            placeholder="Search chats" aria-label="Search chats" />
          {selectedChats.size > 0 ? (
            <div className="mt-2">
              <BulkActionBar
                count={selectedChats.size}
                allMatching={false}
                onClear={clearChatSelection}
                busy={bulkBusy}
                onAction={runChatBulk}
                actions={[
                  { key: "read", label: "Mark read", icon: CheckCheck },
                  ...(canDelete ? [{ key: "hide", label: "Delete", icon: Trash2,
                    confirm: (n: number) => `Delete ${n} ${n === 1 ? "chat" : "chats"} from the list? They stay on the phone and come back with a new message.` }] : []),
                ]}
              />
            </div>
          ) : filteredChats.length > 0 && (
            <Checkbox className="mt-1 text-xs text-fg-muted" label="Select all shown" checked={false}
              onChange={() => setSelectedChats(new Set(filteredChats.map((c) => c.id)))} />
          )}
          {bulkNotice && <p className="mt-1.5 text-[11px] text-fg-muted" role="status">{bulkNotice}</p>}
        </div>

        {/* Chat list (room at the bottom for the docked bulk bar on phones) */}
        <div className="flex-1 overflow-y-auto pb-[var(--bulk-bar-h,0px)] md:pb-0">
          {!status?.is_connected && !statusLoading && (
            <EmptyState compact icon={WifiOff} title="WhatsApp not connected"
              action={isAdmin ? (
                <Button size="sm" variant="ghost" onClick={() => setShowQr(true)}>
                  <QrCode aria-hidden /> Scan QR to connect
                </Button>
              ) : undefined} />
          )}

          {status?.is_connected && (
            <ChatListNotice problem={chatsProblem} busy={chatsLoading}
              onRetry={() => { chatRetryRef.current.attempt = 0; loadChats(); }} />
          )}

          {chatsLoading && chats.length === 0 && (
            <div className="space-y-3 px-4 py-4" aria-busy>
              {[0, 1, 2, 3].map((i) => (
                <div key={i} className="flex items-center gap-3">
                  <Skeleton className="w-10 h-10 rounded-full" />
                  <div className="flex-1 space-y-1.5">
                    <Skeleton className="h-3.5 w-2/3" />
                    <Skeleton className="h-3 w-1/2" />
                  </div>
                </div>
              ))}
            </div>
          )}

          {filteredChats.map((chat) => {
            const isActive = activeChat?.id === chat.id;
            const displayChatName = chat.crm_contact_name || chat.name;
            return (
              <div key={chat.id} className={`group flex items-center border-b border-line-soft transition-colors ${
                  isActive ? "bg-brand/[0.07]" : selectedChats.has(chat.id) ? "bg-surface-2" : "hover:bg-surface-2"}`}>
              <span className={`ml-3 flex shrink-0 transition-opacity ${
                  selectedChats.size > 0 ? "opacity-100" : "[@media(hover:hover)]:opacity-0 [@media(hover:hover)]:group-hover:opacity-100 [@media(hover:hover)]:focus-within:opacity-100"}`}>
                <Checkbox aria-label={`Select chat with ${displayChatName}`} checked={selectedChats.has(chat.id)}
                  onChange={() => toggleChat(chat.id)} />
              </span>
              <button
                type="button"
                onClick={() => openChat(chat)}
                aria-current={isActive ? "true" : undefined}
                className="flex-1 min-w-0 flex items-center gap-3 pl-2 pr-4 py-3 text-left"
              >
                <ChatAvatar name={displayChatName} avatarUrl={chat.avatarUrl} isGroup={chat.isGroup} />
                <div className="flex-1 min-w-0">
                  <div className="flex items-center justify-between">
                    <span className={`text-sm truncate text-fg ${chat.unreadCount > 0 ? "font-semibold" : "font-medium"}`}>
                      {displayChatName}
                    </span>
                    {chat.timestamp > 0 && (
                      <span suppressHydrationWarning className={`text-[11px] shrink-0 ml-2 tabular ${chat.unreadCount > 0 ? "text-brand-ink font-medium" : "text-fg-subtle"}`}>
                        {formatTimestamp(chat.timestamp)}
                      </span>
                    )}
                  </div>
                  {chat.staff_user_id && (
                    <p className="flex items-center gap-1 text-[11px] text-fg-muted truncate" title="Staff: an internal chat for Nary">
                      <Pill tone="violet" className="h-[18px] px-1.5 text-[10px]">Staff</Pill>
                      <span className="truncate">{chat.staff_user_name}</span>
                    </p>
                  )}
                  {chat.crm_org_name && (
                    <p className="flex items-center gap-1 text-[11px] text-fg-muted truncate" title={chat.crm_org_name}>
                      <Building2 className="w-3 h-3 shrink-0" aria-hidden />
                      <span className="truncate">{chat.crm_org_name}</span>
                    </p>
                  )}
                  <div className="flex items-center justify-between mt-0.5">
                    <div className="flex items-center gap-1 min-w-0">
                      {chat.lastMessageFromMe && <CheckCheck className="w-3.5 h-3.5 text-fg-subtle shrink-0" aria-label="Sent by you" />}
                      {chat.lastMessageType !== "chat" && chat.lastMessageType !== "text" && (
                        <LastTypeIcon type={chat.lastMessageType} />
                      )}
                      <p className="text-xs text-fg-muted truncate">
                        {chat.lastMessage || previewLabel(chat.lastMessageType)}
                      </p>
                    </div>
                    <CountBadge count={chat.unreadCount} className="ml-2 shrink-0" />
                  </div>
                </div>
              </button>
              </div>
            );
          })}

          {/* The inbox is paged, and search only filters what has been loaded, so this
              stays reachable (and says so) while a search is active. */}
          {hasMoreChats && (
            <Button variant="ghost" size="sm" className="w-full rounded-none" onClick={loadMoreChats} loading={loadingMoreChats}>
              {!loadingMoreChats && <ChevronDown aria-hidden />}
              {loadingMoreChats
                ? "Loading older chats…"
                : search
                  ? "Load older chats to widen this search"
                  : "Load older chats"}
            </Button>
          )}
        </div>

        {/* Bottom: connected number */}
        {status?.is_connected && status.number && (
          <div className="px-4 py-3 border-t border-line">
            <p className="text-[11px] text-fg-muted">Connected as</p>
            <p className="text-xs font-medium text-fg tabular">+{status.number}{status.name ? ` · ${status.name}` : ""}</p>
          </div>
        )}
      </div>

      {/* ── Right: Chat area ────────────────────────────────────────────────── */}
      {!activeChat ? (
        <div className="hidden flex-1 flex-col items-center justify-center bg-canvas md:flex">
          <EmptyState icon={MessageSquare} title="WhatsApp inbox"
            description={status?.is_connected
              ? "Select a conversation on the left to start messaging."
              : isAdmin
                ? "WhatsApp is not connected. Use the QR button to link your account."
                : "WhatsApp is not connected. Ask an admin to set it up."}
            action={isAdmin && !status?.is_connected ? (
              <Button variant="primary" onClick={() => setShowQr(true)}><QrCode aria-hidden /> Connect WhatsApp</Button>
            ) : undefined} />
        </div>
      ) : (
        <>
        <div className="flex-1 flex flex-col bg-canvas min-w-0">
          {/* Chat header */}
          <div className="bg-surface px-2 md:px-4 py-2.5 flex flex-wrap md:flex-nowrap items-center gap-2 shrink-0 border-b border-line">
            <IconButton icon={ArrowLeft} label="Back to chats" onClick={closeChat} className="md:hidden" />
            <ChatAvatar name={displayName} avatarUrl={activeChat.avatarUrl} isGroup={activeChat.isGroup} />
            <div className="flex-1 min-w-[45%] md:min-w-0 ml-1">
              <h2 className="text-sm font-semibold text-fg leading-tight truncate">{displayName}</h2>
              <p className="text-xs text-fg-muted truncate">
                {activeChat.isGroup
                  ? "Group"
                  : activeChat.crm_contact_name
                    ? [activeChat.crm_org_name, activeChat.name, activeChat.phone ? `+${activeChat.phone}` : ""]
                        .filter(Boolean).join(" · ")
                    : activeChat.phone
                      ? `+${activeChat.phone}`
                      : activeChat.id.replace("@c.us", "")
                }
              </p>
            </div>
            {activeChat.crm_contact_id && <Pill tone="blue">CRM</Pill>}
            <FollowUpButton entityType="whatsapp_chat" chatId={activeChat.id} subject={displayName} />
            {canDelete && (confirmHide ? (
              <div className="flex items-center gap-1.5 rounded-lg border border-line bg-surface-2 px-2 py-1">
                <span className="text-[13px] text-fg">Remove from list?</span>
                <Button size="sm" variant="danger" onClick={hideChat} loading={hiding}>Delete</Button>
                <Button size="sm" variant="ghost" onClick={() => { setConfirmHide(false); setHideError(""); }}>Cancel</Button>
              </div>
            ) : (
              <IconButton icon={Trash2} label="Delete chat" onClick={() => setConfirmHide(true)}
                title="Delete chat: removed from this list only; it stays on the WhatsApp phone and comes back if they write again" />
            ))}
            <IconButton icon={Download} label="Export conversation" onClick={() => { setExportOpen(true); setExportError(""); }} />
            <IconButton icon={UserIcon} label="CRM context" className="lg:hidden" onClick={() => setPanelOpen(true)} />
          </div>
          {hideError && (
            <p className="bg-tone-red-soft px-4 py-2 text-xs text-tone-red" role="alert">{hideError}</p>
          )}

          {/* Messages */}
          <div ref={scrollBoxRef} className="flex-1 overflow-y-auto px-3 md:px-4 py-3 space-y-1">
            {messagesLoading ? (
              <div className="space-y-3 py-4" aria-busy>
                {[0, 1, 2, 3].map((i) => (
                  <div key={i} className={`flex ${i % 2 ? "justify-end" : "justify-start"}`}>
                    <Skeleton className={`h-10 rounded-lg ${i % 2 ? "w-1/3" : "w-2/5"}`} />
                  </div>
                ))}
              </div>
            ) : messages.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-full text-center">
                <EmptyState compact icon={MessageSquare} title="No messages yet"
                  action={historyEnd ? (
                    <Button size="sm" onClick={requestOlderFromPhone} loading={loadingOlder}>
                      {loadingOlder ? "Asking the phone…" : "Load history from phone"}
                    </Button>
                  ) : undefined} />
                {historyNote && <p className="max-w-sm text-xs text-fg-muted" role="status">{historyNote}</p>}
              </div>
            ) : (
              <>
              {(hasMoreWa || historyEnd || historyNote) && (
                <div className="sticky top-0 z-10 flex flex-col items-center gap-1 py-2 pointer-events-none">
                  {(hasMoreWa || historyEnd) && (
                    <button type="button" onClick={hasMoreWa ? loadOlderMessages : requestOlderFromPhone} disabled={loadingOlder}
                      className="pointer-events-auto inline-flex items-center gap-1 text-xs font-medium text-fg-muted hover:text-fg bg-surface-raised border border-line shadow-pop disabled:opacity-50 px-3 py-1.5 rounded-full transition-colors">
                      {!loadingOlder && <ArrowUp className="w-3 h-3" aria-hidden />}
                      {loadingOlder
                        ? (hasMoreWa ? "Loading older messages…" : "Asking the phone…")
                        : (hasMoreWa ? "Load older messages" : "Load older from phone")}
                    </button>
                  )}
                  {historyNote && (
                    <p className="pointer-events-auto max-w-sm text-center text-xs text-fg-muted bg-surface-raised border border-line rounded-lg px-3 py-1.5" role="status">
                      {historyNote}
                    </p>
                  )}
                </div>
              )}
              {groupedMessages.map(({ date, items }) => (
                <div key={date}>
                  <div className="flex items-center justify-center my-4">
                    <span suppressHydrationWarning className="bg-surface-2 text-fg-muted text-[11px] font-medium px-3 py-1 rounded-lg">
                      {date}
                    </span>
                  </div>
                  <div className="space-y-1">
                    {items.map((msg) => {
                      const isMe = msg.fromMe;
                      return (
                        <div key={msg.id} className={`flex ${isMe ? "justify-end" : "justify-start"}`}>
                          <div className={`group max-w-[70%] px-3 py-2 rounded-lg text-sm text-fg border relative ${
                            isMe
                              ? "bg-brand/[0.07] border-brand/15 rounded-br-sm"
                              : "bg-surface border-line rounded-bl-sm"
                          }`}>
                            {/* Hover actions: reply / forward */}
                            {!msg.id?.startsWith("opt-") && (
                              <div className={`absolute -top-3 ${isMe ? "right-1 [@media(hover:none)]:right-full [@media(hover:none)]:mr-1" : "left-1 [@media(hover:none)]:left-full [@media(hover:none)]:ml-1"} z-10 hidden group-hover:flex group-focus-within:flex gap-0.5 bg-surface-raised border border-line rounded-lg px-1 py-0.5 shadow-pop [@media(hover:none)]:top-1/2 [@media(hover:none)]:flex [@media(hover:none)]:-translate-y-1/2`}>
                                <button type="button" title="Reply" aria-label="Reply" onClick={() => { setReplyTo(msg); inputRef.current?.focus(); }}
                                  className="p-1 rounded text-fg-muted hover:bg-surface-2 hover:text-fg [@media(hover:none)]:p-3"><Reply className="w-3.5 h-3.5" aria-hidden /></button>
                                <button type="button" title="Forward" aria-label="Forward" onClick={() => { setForwardSearch(""); setForwardMsg(msg); }}
                                  className="p-1 rounded text-fg-muted hover:bg-surface-2 hover:text-fg [@media(hover:none)]:p-3"><Forward className="w-3.5 h-3.5" aria-hidden /></button>
                              </div>
                            )}
                            {/* Group: show author name */}
                            {activeChat.isGroup && !isMe && msg.author && (
                              <p className="text-[11px] font-semibold text-tone-blue mb-1">
                                {msg.author.replace("@c.us", "")}
                              </p>
                            )}
                            {/* Quoted message (reply context) */}
                            {msg.quoted && (
                              <div className="mb-1.5 border-l-2 border-brand bg-surface-2 rounded px-2 py-1">
                                <p className="text-[11px] font-semibold text-brand-ink">
                                  {msg.quoted.fromMe ? "You" : (msg.quoted.author?.replace("@c.us", "") || "Contact")}
                                </p>
                                <p className="text-xs text-fg-muted truncate">{msg.quoted.body || "Media"}</p>
                              </div>
                            )}
                            {/* Message content */}
                            {msg.hasMedia ? (
                              <MediaBubble msg={msg} />
                            ) : msg.type === "audio" || msg.type === "ptt" ? (
                              <div className="flex items-center gap-2 text-xs text-fg-muted">
                                <Volume2 className="w-4 h-4" aria-hidden />
                                <span>Voice message</span>
                              </div>
                            ) : msg.type !== "chat" && msg.type !== "text" && !msg.body ? (
                              <div className="flex items-center gap-2 text-xs text-fg-muted">
                                {msg.type === "image" && <ImageIcon className="w-4 h-4" aria-hidden />}
                                {msg.type === "document" && <FileText className="w-4 h-4" aria-hidden />}
                                <span>{msg.type}</span>
                              </div>
                            ) : (
                              <p className="leading-relaxed whitespace-pre-wrap break-words">{msg.body}</p>
                            )}
                            {/* Timestamp + tick */}
                            <div className="flex items-center gap-1 mt-1 justify-end">
                              {isMe && msg.sentBy && (
                                <span className="text-[10px] text-fg-muted font-medium truncate max-w-[140px]" title={`Sent by ${msg.sentBy}`}>
                                  {msg.sentBy} ·
                                </span>
                              )}
                              <span suppressHydrationWarning className="text-[10px] text-fg-subtle tabular">
                                {formatMessageTime(msg.timestamp)}
                              </span>
                              {isMe && <Ticks msg={msg} />}
                            </div>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </div>
              ))}
              </>
            )}
            <div ref={bottomRef} />
          </div>

          {/* Scheduled messages banner */}
          {scheduledMessages.filter((m) => m.status === "pending").length > 0 && (
            <div className="bg-tone-orange-soft border-t border-line px-4 py-2 shrink-0 space-y-1.5">
              {scheduledMessages.filter((m) => m.status === "pending").map((m) => (
                <div key={m.id} className="flex items-center gap-2 bg-surface border border-line rounded-lg px-3 py-1.5">
                  <Clock className="w-3.5 h-3.5 text-tone-orange shrink-0" aria-hidden />
                  <p className="text-xs text-fg flex-1 min-w-0 truncate">
                    Scheduled for {new Date(m.scheduled_at).toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}
                    <span className="text-fg-muted ml-1.5">· {m.body.slice(0, 40)}{m.body.length > 40 ? "…" : ""}</span>
                  </p>
                  {m.retry_count > 0 && (
                    <span title={`${m.retry_count} retries · ${m.last_error}`}>
                      <AlertTriangle className="w-3.5 h-3.5 text-tone-red shrink-0" aria-label={`${m.retry_count} retries`} />
                    </span>
                  )}
                  <Button size="sm" variant="ghost" onClick={() => { setEditingScheduled(m); setShowScheduleModal(true); }}>Edit</Button>
                  <IconButton size="sm" icon={X} label="Cancel scheduled message" onClick={() => cancelScheduled(m.id)} />
                </div>
              ))}
            </div>
          )}

          {/* Input bar */}
          <div className="bg-surface border-t border-line px-2 md:px-4 py-2 md:py-3 shrink-0">
            {/* Reply preview */}
            {replyTo && (
              <div className="flex items-center gap-2 mb-2 bg-surface-2 border-l-2 border-brand rounded-lg px-3 py-2">
                <div className="flex-1 min-w-0">
                  <p className="text-[11px] font-semibold text-brand-ink">
                    Replying to {replyTo.fromMe ? "yourself" : (activeChat.crm_contact_name || activeChat.name)}
                  </p>
                  <p className="text-xs text-fg-muted truncate">{replyTo.body || "Media"}</p>
                </div>
                <IconButton size="sm" icon={X} label="Cancel reply" onClick={() => setReplyTo(null)} />
              </div>
            )}
            {/* Click-away backdrop for the emoji popover */}
            {showEmoji && <div className="fixed inset-0 z-20" onClick={() => setShowEmoji(false)} />}
            <form onSubmit={handleSend} className="flex flex-wrap md:flex-nowrap items-end gap-1.5">
              {/* Emoji */}
              <div className="relative">
                <IconButton icon={Smile} label="Emoji" onClick={() => setShowEmoji((v) => !v)}
                  disabled={!status?.is_connected} />
                {showEmoji && (
                  <div className="absolute bottom-12 left-0 z-30">
                    <EmojiPicker
                      onEmojiClick={(e: EmojiClickData) => insertEmoji(e.emoji)}
                      theme={resolvedTheme as unknown as Theme}
                      width={320}
                      height={400}
                      lazyLoadEmojis
                    />
                  </div>
                )}
              </div>
              {/* Attach */}
              <IconButton icon={Paperclip} label="Attach file" onClick={() => fileInputRef.current?.click()}
                disabled={!status?.is_connected} />
              <input
                ref={fileInputRef}
                type="file"
                accept="image/*,video/*,.pdf,.doc,.docx,.xls,.xlsx,.ppt,.pptx,.txt,.csv,.zip"
                className="hidden"
                onChange={(e) => { const f = e.target.files?.[0]; if (f) onPickFile(f); e.target.value = ""; }}
              />
              <div className={`flex-1 relative rounded-lg max-md:order-first max-md:basis-full ${waDrag ? "ring-2 ring-brand" : ""}`} {...waDrop}>
                <Textarea
                  ref={inputRef}
                  value={input}
                  rows={1}
                  aria-label="Message"
                  onChange={(e) => setInput(e.target.value)}
                  onKeyDown={(e) => {
                    // Enter sends; Shift+Enter inserts a newline.
                    if (e.key === "Enter" && !e.shiftKey) {
                      e.preventDefault();
                      if (!sending) handleSend(e);
                    }
                  }}
                  placeholder="Type a message"
                  disabled={!status?.is_connected}
                  className="min-h-0 resize-none max-h-[240px] overflow-y-auto"
                />
              </div>
              <ReplyPromptButton onGenerate={suggestReply} currentDraft={input}
                disabled={!status?.is_connected} />
              <IconButton icon={Clock} label="Schedule send"
                onClick={() => { setEditingScheduled(null); setShowScheduleModal(true); }}
                disabled={!input.trim() || !status?.is_connected} />
              <IconButton type="submit" variant="primary" icon={sending ? RefreshCw : Send} label="Send"
                className={`max-md:ml-auto ${sending ? "[&_svg]:animate-spin" : ""}`}
                disabled={!input.trim() || sending || !status?.is_connected} />
            </form>
            {signature && status?.is_connected && <SignatureHint signature={signature.signature} />}
          </div>
        </div>
        <CrmContextPanel
          channel="whatsapp"
          phone={activeChat.phone}
          chatId={activeChat.id}
          contactId={activeChat.crm_contact_id ?? null}
          displayName={activeChat.crm_contact_name || activeChat.name}
          onChange={loadChats}
          open={panelOpen}
          onClose={() => setPanelOpen(false)}
        />
        </>
      )}

      {/* ── Schedule Send Modal ─────────────────────────────────────────────── */}
      {showScheduleModal && activeChat && accessToken && (
        <ScheduleSendModal
          channel="whatsapp"
          chatId={activeChat.id}
          recipient={activeChat.phone ? `+${activeChat.phone}` : activeChat.id.replace("@c.us", "")}
          body={editingScheduled ? editingScheduled.body : input}
          editingId={editingScheduled?.id}
          accessToken={accessToken}
          onScheduled={(msg) => {
            if (editingScheduled) {
              setScheduledMessages((prev) => prev.map((m) => m.id === editingScheduled.id ? msg : m));
            } else {
              setScheduledMessages((prev) => [...prev, msg]);
              setInput("");
            }
          }}
          onClose={() => { setShowScheduleModal(false); setEditingScheduled(null); }}
        />
      )}

      {/* ── Export transcript ───────────────────────────────────────────────── */}
      <Dialog open={exportOpen && !!activeChat} onOpenChange={setExportOpen} title="Export conversation"
        description={<>Transcript of the chat with <span className="font-medium text-fg">{displayName}</span>. Leave the dates empty to export everything WhatsApp still has for this chat.</>}
        dismissable={!exporting}
        footer={<>
          <Button variant="ghost" onClick={() => setExportOpen(false)}>Cancel</Button>
          <Button variant="primary" onClick={runExport} loading={exporting}>
            {!exporting && <Download aria-hidden />}
            {exporting ? "Exporting…" : "Export"}
          </Button>
        </>}>
        <div className="space-y-4">
          <div role="group" aria-label="Format" className="space-y-1.5">
            <p className="text-[13px] font-medium text-fg">Format</p>
            <div className="grid grid-cols-2 gap-2">
              {([
                { key: "pdf", label: "PDF", hint: "Branded, images inline" },
                { key: "txt", label: "Text", hint: "Plain .txt, no media" },
              ] as const).map((f) => (
                <button key={f.key} type="button" onClick={() => setExportFormat(f.key)} aria-pressed={exportFormat === f.key}
                  className={`rounded-lg px-3 py-2.5 text-left border transition-colors ${
                    exportFormat === f.key
                      ? "bg-brand/[0.07] border-brand text-fg"
                      : "bg-surface border-line text-fg hover:bg-surface-2"
                  }`}>
                  <span className="block text-sm font-semibold">{f.label}</span>
                  <span className="block text-xs text-fg-muted mt-0.5">{f.hint}</span>
                </button>
              ))}
            </div>
          </div>

          <FieldGrid cols={2}>
            <Field label="From">
              <Input type="date" value={exportFrom} max={exportTo || undefined}
                onChange={(e) => setExportFrom(e.target.value)} />
            </Field>
            <Field label="To">
              <Input type="date" value={exportTo} min={exportFrom || undefined}
                onChange={(e) => setExportTo(e.target.value)} />
            </Field>
          </FieldGrid>

          {exportError && <p className="text-xs text-tone-red" role="alert">{exportError}</p>}
          {exporting && (
            <p className="text-xs text-fg-muted">
              Reading the conversation from WhatsApp, a long history can take a minute.
            </p>
          )}
        </div>
      </Dialog>

      {/* ── Attachment preview + caption ────────────────────────────────────── */}
      <Dialog open={!!attach} onOpenChange={(o) => { if (!o) { setAttach(null); setAttachError(""); } }}
        title="Send attachment" dismissable={!attachSending}
        footer={<>
          <Button variant="ghost" onClick={() => { setAttach(null); setAttachError(""); }}>Cancel</Button>
          <Button variant="primary" onClick={sendAttachment} loading={attachSending}>
            {!attachSending && <Send aria-hidden />}
            {attachSending ? "Sending…" : "Send"}
          </Button>
        </>}>
        {attach && (
          <div className="space-y-3">
            {attach.kind === "image" ? (
              // eslint-disable-next-line @next/next/no-img-element
              <img src={attach.previewUrl} alt={attach.filename} className="max-h-64 rounded-lg mx-auto" />
            ) : attach.kind === "video" ? (
              <video src={attach.previewUrl} controls className="max-h-64 rounded-lg mx-auto w-full" />
            ) : (
              <div className="flex items-center gap-3 bg-surface-2 border border-line rounded-lg px-4 py-3">
                <FileText className="w-6 h-6 text-fg-muted shrink-0" aria-hidden />
                <span className="text-sm text-fg truncate">{attach.filename}</span>
              </div>
            )}
            <Field label="Caption">
              <Input
                value={attach.caption}
                onChange={(e) => setAttach({ ...attach, caption: e.target.value })}
                placeholder="Add a caption"
              />
            </Field>
            {signature && <SignatureHint signature={signature.signature} />}
            {attachError && <p className="text-xs text-tone-red" role="alert">{attachError}</p>}
          </div>
        )}
      </Dialog>

      {/* ── Forward picker ──────────────────────────────────────────────────── */}
      <Dialog open={!!forwardMsg} onOpenChange={(o) => { if (!o) { setForwardMsg(null); setForwardSearch(""); } }}
        title="Forward to" size="sm">
        <div className="space-y-2">
          <Input type="search" autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            leading={Search} value={forwardSearch} onChange={(e) => setForwardSearch(e.target.value)}
            placeholder="Search chats" aria-label="Search chats to forward to" />
          <div className="-mx-2">
            {chats
              .filter((c) => !forwardSearch || c.name.toLowerCase().includes(forwardSearch.toLowerCase()))
              .map((c) => (
                <button key={c.id} type="button" onClick={() => doForward(c)} disabled={forwarding}
                  className="w-full flex items-center gap-3 px-2 py-2 rounded-lg hover:bg-surface-2 text-left disabled:opacity-50 transition-colors">
                  <ChatAvatar name={c.name} size="sm" isGroup={c.isGroup} />
                  <span className="text-sm text-fg truncate">{c.name}</span>
                </button>
              ))}
          </div>
        </div>
      </Dialog>

      {/* ── Connect (QR) ────────────────────────────────────────────────────── */}
      <Dialog open={showQr && !!isAdmin} onOpenChange={setShowQr} title="Connect WhatsApp"
        description="Scan with your phone to link" size="sm">
        <div className="text-center">
          {status?.is_connected ? (
            <div className="py-4">
              <div className="w-14 h-14 rounded-full bg-tone-blue-soft text-tone-blue flex items-center justify-center mx-auto mb-3">
                <Wifi className="w-6 h-6" aria-hidden />
              </div>
              <p className="font-semibold text-fg">Connected</p>
              <p className="text-sm text-fg-muted mt-1 tabular">+{status.number}{status.name ? ` · ${status.name}` : ""}</p>
              <Button variant="primary" className="mt-4" onClick={() => setShowQr(false)}>Done</Button>
            </div>
          ) : status?.qr_code ? (
            <div>
              <div className="p-3 rounded-lg border border-line bg-surface inline-block mb-4">
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img src={status.qr_code} alt="WhatsApp QR" className="w-52 h-52" />
              </div>
              <div className="space-y-2 text-left text-sm text-fg-muted">
                <p className="font-medium text-fg text-center mb-1">How to scan</p>
                {["Open WhatsApp on your phone", "Tap Menu (⋮) or Settings, then Linked Devices", "Tap Link a Device and scan this QR"].map((step, i) => (
                  <div key={i} className="flex items-start gap-2">
                    <span className="w-5 h-5 rounded-full bg-brand text-brand-on text-[11px] font-semibold flex items-center justify-center shrink-0 mt-0.5">{i + 1}</span>
                    <span>{step}</span>
                  </div>
                ))}
              </div>
              <p className="text-xs text-fg-muted mt-4">The QR refreshes automatically about every 20 seconds</p>
            </div>
          ) : (
            <div className="py-4">
              <Loader2 className="w-6 h-6 animate-spin text-fg-muted mx-auto mb-3" aria-hidden />
              <p className="text-sm text-fg">Generating QR code…</p>
              <p className="text-xs text-fg-muted mt-1">This may take a few seconds on first launch</p>
            </div>
          )}
        </div>
      </Dialog>
    </FullHeightPage>
  );
}
