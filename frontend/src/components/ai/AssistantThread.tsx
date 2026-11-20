"use client";

import { useEffect, useRef, useState } from "react";
import Link from "next/link";
import { Sparkles, Send, BookOpen, Loader2, Check, Pencil, Trash2, Plus, Square, Paperclip } from "lucide-react";
import ProposalCard from "./ProposalCard";
import RecordsCard from "./RecordsCard";
import MarkdownMessage from "./MarkdownMessage";
import type { ChatMessage, Conversation, ProposalOutcome } from "./types";
import { Button, IconButton, controlClass } from "@/components/ui";
import { cn } from "@/lib/utils";

/** Scrolling message list, shared by the Ctrl J panel and the Assistant page. */
export function MessageList({
  messages, streaming, status, empty, onAskMore, onResolveProposal,
}: {
  messages: ChatMessage[];
  streaming: boolean;
  status: string;
  empty?: React.ReactNode;
  /** "Show next 25" under a record table sends this as the next question. */
  onAskMore?: (text: string) => void;
  /** A confirm card was applied or discarded. */
  onResolveProposal?: (proposalId: string, status: ProposalOutcome, result?: { id?: string; label?: string }) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, status]);

  return (
    <div ref={scrollRef} className="flex-1 space-y-4 overflow-y-auto bg-surface p-4">
      {messages.length === 0 && empty}
      {messages.map((m) => (
        <div key={m.id} className={m.role === "user" ? "flex justify-end" : "flex justify-start"}>
          <div className={
            m.role === "user"
              ? "max-w-[85%] whitespace-pre-wrap rounded-lg rounded-br-sm bg-fg px-3.5 py-2 text-sm text-surface"
              : "min-w-0 max-w-[90%] rounded-lg rounded-bl-sm bg-surface-2 px-3.5 py-2 text-sm text-fg"
          }>
            {m.role === "assistant" ? (
              <div className="min-w-0">
                {m.content && <MarkdownMessage content={m.content} />}
                {m.citations && m.citations.length > 0 && (
                  <div className="mt-2 flex flex-wrap gap-1.5">
                    {m.citations.map((c, ci) => (
                      <Link key={ci} href={c.id ? `/knowledge/articles/${c.id}` : "#"}
                        className="inline-flex items-center gap-1 rounded-full bg-tone-violet-soft px-2 py-0.5 text-[11px] font-medium text-tone-violet hover:underline">
                        <BookOpen size={11} aria-hidden />{c.title || "SOP"}
                      </Link>
                    ))}
                  </div>
                )}
                {(m.cards || []).map((card, ci) => (
                  <RecordsCard key={ci} card={card} onAskMore={onAskMore} />
                ))}
                {(m.proposals || []).map((p, pi) => (
                  <ProposalCard key={p.proposal_id || pi} proposal={p}
                    onResolved={p.proposal_id && onResolveProposal
                      ? (status, result) => onResolveProposal(p.proposal_id as string, status, result)
                      : undefined} />
                ))}
              </div>
            ) : m.content}
          </div>
        </div>
      ))}
      {streaming && status && (
        <div role="status" className="flex items-center gap-2 text-xs text-fg-muted">
          <Loader2 size={13} className="animate-spin" aria-hidden />{status}
        </div>
      )}
    </div>
  );
}

export function Composer({
  value, onChange, onSend, onStop, streaming, autoFocus, onAttach, attachAccept, attachTitle, attaching,
}: {
  value: string;
  onChange: (v: string) => void;
  onSend: () => void;
  /** Cancels the reply in progress; shown in place of Send while streaming. */
  onStop?: () => void;
  streaming: boolean;
  autoFocus?: boolean;
  /** A paperclip before the text box (Nary chat: a meeting notes file or recording). */
  onAttach?: (file: File) => void;
  attachAccept?: string;
  attachTitle?: string;
  /** Upload progress text while a file goes up. */
  attaching?: string | null;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  // Grow with the text up to max-h-32, and shrink back once it is sent.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [value]);

  return (
    <div className="border-t border-line bg-surface p-3">
      {attaching && (
        <p role="status" className="mb-1.5 flex items-center gap-1.5 text-xs text-fg-muted">
          <Loader2 size={12} className="animate-spin" aria-hidden /> {attaching}
        </p>
      )}
      <div className="flex items-end gap-2">
        {onAttach && (
          <>
            <input ref={fileRef} type="file" className="hidden" accept={attachAccept} aria-label="Attach a file"
              onChange={(e) => { const f = e.target.files?.[0]; if (f) onAttach(f); e.target.value = ""; }} />
            <button type="button" onClick={() => fileRef.current?.click()} disabled={streaming || !!attaching}
              title={attachTitle ?? "Attach a file"} aria-label={attachTitle ?? "Attach a file"}
              className="flex size-11 shrink-0 items-center justify-center rounded-lg border border-line-strong text-fg-muted hover:bg-surface-2 hover:text-fg disabled:opacity-40 md:size-9">
              <Paperclip size={16} aria-hidden />
            </button>
          </>
        )}
        <textarea
          ref={ref}
          autoFocus={autoFocus}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            // Enter while an IME is composing (e.g. picking a candidate) must not send.
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); onSend(); }
          }}
          rows={1}
          placeholder="Ask anything (Enter to send)"
          aria-label="Message"
          className={cn(controlClass, "max-h-32 min-h-11 flex-1 resize-none overflow-y-auto py-2 md:min-h-9")}
        />
        {streaming && onStop ? (
          <button type="button" onClick={() => onStop()} title="Stop" aria-label="Stop"
            className="flex size-11 shrink-0 items-center justify-center rounded-lg bg-fg text-surface hover:opacity-90 md:size-9">
            <Square size={16} className="fill-current" aria-hidden />
          </button>
        ) : (
          // Wrapped so the click event is never passed on as the message text.
          <button type="button" onClick={() => onSend()} disabled={streaming || !value.trim()}
            title="Send" aria-label="Send"
            className="flex size-11 shrink-0 items-center justify-center rounded-lg bg-brand text-brand-on hover:bg-brand-hover disabled:opacity-40 md:size-9">
            {streaming ? <Loader2 size={16} className="animate-spin" aria-hidden /> : <Send size={16} aria-hidden />}
          </button>
        )}
      </div>
      <p className="mt-1.5 text-center text-[11px] text-fg-subtle">
        AI can be wrong. Verify important details. Suggestions only.
      </p>
    </div>
  );
}

/** The caller's own past sessions, with inline rename and delete. */
export function ConversationList({
  conversations, activeId, onOpen, onRename, onDelete, onNew,
}: {
  conversations: Conversation[];
  activeId?: string | null;
  onOpen: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onDelete: (id: string) => void;
  onNew?: () => void;
}) {
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");

  const submitRename = (id: string) => {
    const title = renameValue;
    setRenamingId(null);
    onRename(id, title);
  };

  return (
    <div className="flex-1 overflow-y-auto p-2">
      {onNew && (
        <Button variant="secondary" className="mb-2 w-full justify-start border-dashed" onClick={onNew}>
          <Plus aria-hidden /> New chat
        </Button>
      )}
      {conversations.length === 0 && (
        <p className="mt-8 text-center text-sm text-fg-muted">No previous chats.</p>
      )}
      {conversations.map((c) => (
        <div key={c.id}
          className={cn("group flex items-center gap-1 rounded-lg px-2 py-1.5 hover:bg-surface-2", c.id === activeId && "bg-brand/[0.07]")}>
          {renamingId === c.id ? (
            <>
              <input autoFocus value={renameValue} onChange={(e) => setRenameValue(e.target.value)} aria-label="Chat name"
                onKeyDown={(e) => e.key === "Enter" && submitRename(c.id)} autoComplete="off"
                className={cn(controlClass, "h-8 flex-1")} />
              <IconButton icon={Check} size="sm" label="Save name" onClick={() => submitRename(c.id)} />
            </>
          ) : (
            <>
              <button type="button" onClick={() => onOpen(c.id)} aria-current={c.id === activeId ? "true" : undefined}
                className="min-h-11 flex-1 truncate text-left text-sm text-fg md:min-h-7">
                {c.title}
              </button>
              <IconButton icon={Pencil} size="sm" label={`Rename ${c.title}`} className="text-fg-muted focus-visible:opacity-100 group-hover:opacity-100 [@media(hover:hover)]:opacity-0"
                onClick={() => { setRenamingId(c.id); setRenameValue(c.title); }} />
              <IconButton icon={Trash2} size="sm" label={`Delete ${c.title}`} className="text-fg-muted hover:text-tone-red focus-visible:opacity-100 group-hover:opacity-100 [@media(hover:hover)]:opacity-0"
                onClick={() => onDelete(c.id)} />
            </>
          )}
        </div>
      ))}
    </div>
  );
}

export function EmptyState({ compact }: { compact?: boolean }) {
  return (
    <div className={cn("px-4 text-center text-fg-muted", compact ? "mt-10" : "mt-16")}>
      <Sparkles size={compact ? 28 : 36} className="mx-auto mb-3 text-tone-violet" aria-hidden />
      <p className="text-sm font-medium text-fg">How can I help?</p>
      <p className="mx-auto mt-2 max-w-md text-xs">Ask about SOPs, your pipeline, projects, or invoices. I only
        see what your permissions allow, and I suggest; I never send or change anything.</p>
    </div>
  );
}
