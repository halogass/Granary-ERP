"use client";

import { useState, useEffect } from "react";
import { MessageSquare, Download, Film, FileText, ScrollText, CheckCheck, RotateCcw, Clock, ChevronDown, ChevronUp } from "lucide-react";
import { messagingApi } from "@/lib/api";
import { safeHref } from "@/lib/sanitize";
import Avatar from "@/components/common/Avatar";
import { Button, Pill, Skeleton, StatusPill, type Tone } from "@/components/ui";
import { cn } from "@/lib/utils";
import { mentionsToPlainText } from "@/lib/messageMentions";
import MessageContent from "./MessageContent";

const COMMON_EMOJIS = ["👍", "❤️", "😂", "🎉", "🔥", "👏", "😍", "🚀", "💯", "✅"];

export interface Reaction { emoji: string; count: number; user_ids: string[] }

export interface MemoData {
  id: string;
  memo_number: string;
  subject: string;
  memo_type: string;
  body: string;
  sender_id: string;
  sender_name: string;
  sender_position?: string;
  sender_department?: string;
  sender_signed_at: string;
  recipient_id?: string;
  channel_id?: string;
  status: string;
  recalled_at?: string;
  acknowledgements: { user_id: string; name: string; position?: string; acknowledged_at: string }[];
  created_at: string;
}

export interface Message {
  id: string;
  sender_id: string;
  sender_name: string;
  sender_avatar?: string;
  content: string;
  message_type: string;
  file_url?: string;
  file_name?: string;
  file_size?: number;
  mime_type?: string;
  mentions?: { type: string; id: string; name: string }[];
  thread_count?: number;
  thread_root_id?: string;
  created_at: string;
  source: "dm" | "channel";
  channel_id?: string;
  receiver_id?: string;
  memo_id?: string;
  memo?: MemoData;
}

// Attachment renderers
function ImageAttachment({ url, name }: { url: string; name?: string }) {
  const [enlarged, setEnlarged] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setEnlarged(true)} aria-label={`Enlarge ${name ?? "image"}`} className="mt-1 block cursor-zoom-in">
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={url} alt={name ?? "image"} className="max-h-48 max-w-xs rounded-lg border border-line object-cover" />
      </button>
      {enlarged && (
        <div
          role="dialog"
          aria-label={name ?? "image"}
          className="fixed inset-0 z-[200] flex items-center justify-center bg-black/80"
          onClick={() => setEnlarged(false)}
        >
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={url} alt={name} className="max-h-[90vh] max-w-[90vw] rounded-lg" />
        </div>
      )}
    </>
  );
}

function VideoAttachment({ url, name }: { url: string; name?: string }) {
  return (
    <video
      src={url}
      controls
      className="mt-1 max-w-xs rounded-lg border border-line"
      preload="metadata"
    >
      {name}
    </video>
  );
}

function FileAttachment({ url, name, size, mime }: { url: string; name?: string; size?: number; mime?: string }) {
  const sizeStr = size ? size > 1024 * 1024 ? `${(size / 1024 / 1024).toFixed(1)} MB` : `${Math.round(size / 1024)} KB` : "";
  return (
    <a
      href={url}
      download={name}
      target="_blank"
      rel="noopener noreferrer"
      className="group mt-1 flex max-w-xs items-center gap-3 rounded-lg border border-line bg-surface px-3 py-2 transition-colors hover:bg-surface-2"
    >
      <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-surface-2">
        <FileText className="size-4 text-fg-muted" aria-hidden />
      </div>
      <div className="min-w-0 flex-1">
        <p className="truncate text-xs font-medium text-fg">{name ?? "File"}</p>
        {sizeStr && <p className="text-xs text-fg-muted tabular">{sizeStr}</p>}
      </div>
      <Download className="size-3.5 shrink-0 text-fg-subtle transition-colors group-hover:text-fg" aria-label="Download" />
    </a>
  );
}

// Reaction bar
function ReactionBar({
  messageId, source, myId, initialReactions,
}: {
  messageId: string; source: string; myId: string; initialReactions: Reaction[];
}) {
  const [reactions, setReactions] = useState<Reaction[]>(initialReactions);
  const [showPicker, setShowPicker] = useState(false);

  const toggle = async (emoji: string) => {
    try {
      const res = await messagingApi.toggleReaction(messageId, emoji, source);
      setReactions(res.data.reactions);
    } catch { /* ignore */ }
    setShowPicker(false);
  };

  return (
    <div className="relative mt-1 flex flex-wrap items-center gap-1">
      {reactions.map((r) => {
        const mine = r.user_ids.includes(myId);
        return (
          <button
            key={r.emoji}
            type="button"
            aria-pressed={mine}
            onClick={() => toggle(r.emoji)}
            className={cn("flex items-center gap-1 rounded-full border px-1.5 py-0.5 text-xs transition-colors",
              mine ? "border-brand/30 bg-brand/[0.07] text-brand-ink" : "border-line bg-surface text-fg-muted hover:bg-surface-2")}
          >
            <span>{r.emoji}</span>
            <span className="font-medium tabular">{r.count}</span>
          </button>
        );
      })}
      <div className="relative">
        <button
          type="button"
          aria-label="Add reaction"
          aria-expanded={showPicker}
          onClick={() => setShowPicker((v) => !v)}
          className="rounded-full border border-line px-1.5 py-0.5 text-xs text-fg-subtle transition-colors hover:bg-surface-2 hover:text-fg"
        >
          +
        </button>
        {showPicker && (
          <div className="absolute bottom-full left-0 z-50 mb-1 w-44 rounded-lg border border-line bg-surface-raised p-2 shadow-pop">
            <div className="grid grid-cols-5 gap-1">
              {COMMON_EMOJIS.map((e) => (
                <button
                  key={e}
                  type="button"
                  aria-label={`React with ${e}`}
                  onClick={() => toggle(e)}
                  className="flex size-7 items-center justify-center rounded text-base hover:bg-surface-2"
                >
                  {e}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function formatTime(date: string) {
  return new Date(date).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
}

// Formal Memo Card

const MEMO_TYPE_META: Record<string, { label: string; tone: Tone }> = {
  directive:   { label: "Directive",           tone: "orange" },
  information: { label: "Information",         tone: "blue" },
  request:     { label: "Request",             tone: "violet" },
  approval:    { label: "Approval / decision", tone: "neutral" },
};

const STATUS_META: Record<string, { label: string; icon: React.ElementType }> = {
  sent:                    { label: "Pending acknowledgement", icon: Clock },
  acknowledged:            { label: "Acknowledged",            icon: CheckCheck },
  partially_acknowledged:  { label: "Partially acknowledged",  icon: Clock },
  recalled:                { label: "Recalled",                icon: RotateCcw },
};

function fmtDate(iso: string) {
  const d = new Date(iso);
  return d.toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" })
    + ", " + d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
}

/** The API's recall rule: the sender, until everyone has acknowledged it (a channel memo some members
 *  acknowledged can still be recalled). */
export const RECALLABLE_MEMO_STATUSES = ["sent", "partially_acknowledged"];

export function memoCanRecall(memo: Pick<MemoData, "sender_id" | "status">, myId: string): boolean {
  return memo.sender_id === myId && RECALLABLE_MEMO_STATUSES.includes(memo.status);
}

function MemoCard({ msg, myId, onMemoChange }: {
  msg: Message; myId: string; onMemoChange?: (memo: MemoData) => void;
}) {
  const [memo, setMemo] = useState<MemoData | null>(msg.memo ?? null);
  const [expanded, setExpanded] = useState(false);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!memo && msg.memo_id) {
      messagingApi.getMemo(msg.memo_id).then((r) => setMemo(r.data)).catch(() => {});
    }
  }, [msg.memo_id]); // eslint-disable-line

  // Keep in sync when parent updates (via WS)
  useEffect(() => { if (msg.memo) setMemo(msg.memo); }, [msg.memo]);

  if (!memo) {
    return (
      <div className="w-full max-w-xl rounded-lg border border-line bg-surface p-4" aria-busy aria-label="Loading memo">
        <Skeleton className="mb-3 h-3 w-1/3" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    );
  }

  const typeMeta = MEMO_TYPE_META[memo.memo_type] ?? { label: memo.memo_type, tone: "neutral" as Tone };
  const statusMeta = STATUS_META[memo.status] ?? { label: memo.status, icon: Clock };
  const StatusIcon = statusMeta.icon;

  const isSender = memo.sender_id === myId;
  const isRecipient = memo.recipient_id === myId;
  const isChannelMember = !!memo.channel_id;
  const alreadyAcked = memo.acknowledgements.some((a) => a.user_id === myId);
  const canAck = !isSender && (isRecipient || isChannelMember) && memo.status !== "recalled" && !alreadyAcked;
  const canRecall = memoCanRecall(memo, myId);

  const BODY_PREVIEW_LIMIT = 280;
  const bodyTruncated = memo.body.length > BODY_PREVIEW_LIMIT && !expanded;
  const bodyDisplay = bodyTruncated ? memo.body.slice(0, BODY_PREVIEW_LIMIT) + "..." : memo.body;

  async function handleAck() {
    setLoading(true);
    try {
      const res = await messagingApi.acknowledgeMemo(memo!.id);
      setMemo(res.data);
      onMemoChange?.(res.data);
    } catch { /* ignore */ } finally { setLoading(false); }
  }

  async function handleRecall() {
    const ask = memo!.status === "partially_acknowledged"
      ? "Recall this memo? Some members have acknowledged it already; everyone will see it as recalled."
      : "Recall this memo? The recipient will see it as recalled.";
    if (!confirm(ask)) return;
    setLoading(true);
    try {
      const res = await messagingApi.recallMemo(memo!.id);
      setMemo(res.data);
      onMemoChange?.(res.data);
    } catch { /* ignore */ } finally { setLoading(false); }
  }

  async function handlePdf() {
    try {
      const res = await messagingApi.downloadMemoPdf(memo!.id);
      const url = URL.createObjectURL(res.data as Blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${memo!.memo_number.replace(/\//g, "-")}.pdf`;
      a.click();
      URL.revokeObjectURL(url);
    } catch { /* ignore */ }
  }

  const label = "text-xs font-medium text-fg-muted";
  return (
    <div className={cn("w-full max-w-xl overflow-hidden rounded-lg border border-line bg-surface shadow-card",
      memo.status === "recalled" && "opacity-60")}>
      {/* Header band */}
      <div className="flex items-center justify-between border-b border-line bg-surface-2 px-4 py-2">
        <div className="flex items-center gap-2">
          <ScrollText className="size-3.5 text-fg-muted" aria-hidden />
          <span className="text-xs font-semibold text-fg">Formal memo</span>
        </div>
        <span className="font-mono text-xs text-fg-muted">{memo.memo_number}</span>
      </div>

      <div className="space-y-3 px-4 py-3">
        <div className="flex items-center justify-between gap-3">
          <Pill tone={typeMeta.tone}>{typeMeta.label}</Pill>
          <span className="shrink-0 text-xs text-fg-muted tabular">{fmtDate(memo.sender_signed_at)}</span>
        </div>

        <div>
          <p className={cn(label, "mb-0.5")}>Subject</p>
          <p className="text-sm font-semibold leading-snug text-fg">{memo.subject}</p>
        </div>

        <div className="border-t border-line-soft" />

        <div>
          <p className={cn(label, "mb-1")}>Content</p>
          <p className="whitespace-pre-wrap text-sm leading-relaxed text-fg">{bodyDisplay}</p>
          {memo.body.length > BODY_PREVIEW_LIMIT && (
            <button type="button" onClick={() => setExpanded((v) => !v)} aria-expanded={expanded}
              className="mt-1 flex items-center gap-1 text-xs text-brand-ink hover:underline">
              {expanded ? <><ChevronUp className="size-3" aria-hidden /> Show less</> : <><ChevronDown className="size-3" aria-hidden /> Show full memo</>}
            </button>
          )}
        </div>

        <div className="border-t border-line-soft" />

        {/* Sender signature */}
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <p className={cn(label, "mb-0.5")}>Signed by</p>
            <p className="text-xs font-semibold text-fg">{memo.sender_name}</p>
            {memo.sender_position && (
              <p className="text-xs text-fg-muted">{memo.sender_position}{memo.sender_department ? `, ${memo.sender_department}` : ""}</p>
            )}
            <p className="text-xs text-fg-muted"><span className="tabular">{fmtDate(memo.sender_signed_at)}</span> · <span className="text-fg">Digitally signed</span></p>
          </div>
          {memo.status === "acknowledged" && memo.acknowledgements[0] && !memo.channel_id && (
            <div className="shrink-0 text-right">
              <p className={cn(label, "mb-0.5")}>Acknowledged by</p>
              <p className="text-xs font-semibold text-fg">{memo.acknowledgements[0].name}</p>
              <p className="text-xs text-fg-muted tabular">{fmtDate(memo.acknowledgements[0].acknowledged_at)}</p>
            </div>
          )}
        </div>

        {/* Channel acknowledgement progress */}
        {memo.channel_id && memo.acknowledgements.length > 0 && (
          <div className="rounded-lg bg-surface-2 px-3 py-2">
            <p className="mb-1.5 text-xs font-medium text-fg-muted">
              Acknowledged by <span className="tabular">{memo.acknowledgements.length}</span> member{memo.acknowledgements.length !== 1 ? "s" : ""}
            </p>
            <div className="flex flex-wrap gap-1">
              {memo.acknowledgements.map((a) => (
                <Pill key={a.user_id} tone="blue">{a.name}</Pill>
              ))}
            </div>
          </div>
        )}

        {/* Status + actions */}
        <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
          <StatusPill family="memo" status={memo.status} label={statusMeta.label} icon={StatusIcon} />
          <div className="flex items-center gap-1.5">
            {canAck && (
              <Button size="sm" variant="primary" loading={loading} onClick={handleAck}>
                <CheckCheck aria-hidden /> Acknowledge
              </Button>
            )}
            {canRecall && (
              <Button size="sm" variant="danger" disabled={loading} onClick={handleRecall}>
                <RotateCcw aria-hidden /> Recall
              </Button>
            )}
            <Button size="sm" onClick={handlePdf}>
              <Download aria-hidden /> PDF
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

/** The attachment under a message (image, video or file), shared by the channel view and the thread panel. */
export function MessageAttachment({ msg }: { msg: Pick<Message, "message_type" | "file_url" | "file_name" | "file_size" | "mime_type"> }) {
  const url = safeHref(msg.file_url);
  if (!url) return null;
  if (msg.message_type === "image") return <ImageAttachment url={url} name={msg.file_name} />;
  if (msg.message_type === "video") return <VideoAttachment url={url} name={msg.file_name} />;
  if (msg.message_type === "file") return <FileAttachment url={url} name={msg.file_name} size={msg.file_size} mime={msg.mime_type} />;
  return null;
}

// Main MessageBubble
interface BubbleProps {
  msg: Message;
  isMe: boolean;
  showAvatar: boolean;
  myId: string;
  reactions?: Reaction[];
  onReplyInThread?: (msg: Message) => void;
  onMemoChange?: (memoId: string, memo: MemoData) => void;
  searchQuery?: string;
}

export default function MessageBubble({ msg, isMe, showAvatar, myId, reactions = [], onReplyInThread, onMemoChange, searchQuery }: BubbleProps) {
  const [hover, setHover] = useState(false);

  const isHighlighted = searchQuery
    ? mentionsToPlainText(msg.content, msg.mentions).toLowerCase().includes(searchQuery.toLowerCase())
    : false;

  // Formal memos render as a special full-width card, not a chat bubble
  if (msg.message_type === "memo") {
    return (
      <div className={cn("py-1", isHighlighted && "-mx-2 rounded-lg bg-tone-orange-soft px-2")}>
        {showAvatar && (
          <div className="mb-2 ml-1 flex items-center gap-2">
            <Avatar name={msg.sender_name} size="sm" />
            <span className="text-xs font-medium text-fg">{msg.sender_name}</span>
            <span className="text-xs text-fg-subtle tabular">{formatTime(msg.created_at)}</span>
          </div>
        )}
        <MemoCard
          msg={msg}
          myId={myId}
          onMemoChange={(memo) => onMemoChange?.(msg.memo_id ?? memo.id, memo)}
        />
      </div>
    );
  }

  return (
    <div
      className={cn("flex items-end gap-2", isMe && "flex-row-reverse", isHighlighted && "-mx-2 rounded-lg bg-tone-orange-soft px-2")}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      {/* Avatar slot */}
      {!isMe && (
        <div className="w-7 shrink-0">
          {showAvatar && <Avatar name={msg.sender_name} size="sm" />}
        </div>
      )}

      <div className={cn("flex max-w-[70%] flex-col", isMe ? "items-end" : "items-start")}>
        {showAvatar && !isMe && (
          <span className="mb-1 ml-1 text-xs font-medium text-fg">{msg.sender_name}</span>
        )}

        {/* Bubble */}
        <div className={cn("rounded-lg border px-3.5 py-2 text-sm leading-relaxed text-fg",
          isMe ? "rounded-br-sm border-brand/20 bg-brand/[0.07]" : "rounded-bl-sm border-line bg-surface")}>
          <MessageContent content={msg.content} mentions={msg.mentions} />

          <MessageAttachment msg={msg} />
        </div>

        {/* Metadata row */}
        <div className={cn("mx-1 mt-0.5 flex items-center gap-2", isMe && "flex-row-reverse")}>
          <span className="text-xs text-fg-subtle tabular">{formatTime(msg.created_at)}</span>

          {/* Hover actions */}
          {onReplyInThread && (
            <button
              type="button"
              onClick={() => onReplyInThread(msg)}
              className={cn("flex min-h-8 items-center gap-0.5 px-1 text-xs text-fg-muted transition-colors hover:text-fg md:min-h-0 md:px-0",
                !hover && "[@media(hover:hover)]:hidden")}
            >
              <MessageSquare className="size-3" aria-hidden /> Thread
            </button>
          )}
        </div>

        {/* Reactions */}
        <div className={cn("w-full", isMe ? "items-end" : "items-start")}>
          <ReactionBar
            messageId={msg.id}
            source={msg.source}
            myId={myId}
            initialReactions={reactions}
          />
        </div>

        {/* Thread count */}
        {(msg.thread_count ?? 0) > 0 && onReplyInThread && (
          <button
            type="button"
            onClick={() => onReplyInThread(msg)}
            className="ml-1 mt-1 flex items-center gap-1 text-xs font-medium text-brand-ink hover:underline"
          >
            <MessageSquare className="size-3" aria-hidden />
            {msg.thread_count} {msg.thread_count === 1 ? "reply" : "replies"}
          </button>
        )}
      </div>
    </div>
  );
}
