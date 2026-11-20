"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { usePathname } from "next/navigation";
import { aiApi } from "@/lib/api";
import type { ChatMessage, Citation, Conversation, Proposal, ProposalOutcome, RecordsCard } from "./types";

// Map a CRM/PM/finance route to an entity context the assistant can resolve.
const ENTITY_SEGMENTS: Record<string, string> = {
  deals: "deal", leads: "lead", contacts: "contact", projects: "project",
  invoices: "invoice", quotations: "quotation", organizations: "organization",
};
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

let seq = 0;
/** A stable React key for a message (crypto.randomUUID needs a secure context, so fall back). */
const newId = () =>
  (typeof crypto !== "undefined" && "randomUUID" in crypto) ? crypto.randomUUID() : `m${Date.now()}-${++seq}`;

export function deriveContext(pathname: string): Record<string, string> {
  const ctx: Record<string, string> = { route: pathname };
  const segs = pathname.split("/").filter(Boolean);
  for (let i = 0; i < segs.length - 1; i++) {
    if (ENTITY_SEGMENTS[segs[i]] && UUID_RE.test(segs[i + 1])) {
      ctx.entity_type = ENTITY_SEGMENTS[segs[i]];
      ctx.entity_id = segs[i + 1];
    }
  }
  return ctx;
}

// fix-walist: a dropped stream (Cloudflare's 100 s limit on a long turn) no longer ends in "Connection
// error": the server finishes and saves the turn anyway, so the hook fetches it, polling while it runs.
type ServerMessage = Omit<ChatMessage, "id">;
export const RECOVER_POLL_MS = 5000;
export const RECOVER_FOR_MS = 3 * 60 * 1000;
export const DROPPED_STILL_WORKING = "The connection dropped; Nary is still working";
export const DROPPED_NO_ID = "The connection dropped. Reload the chat list to see the answer.";
export const DROPPED_GAVE_UP = "The connection dropped and the answer is not saved yet. Open this chat again in a while to see it.";

/** A conversation's saved messages as the thread shows them (records and proposals included). */
export function toChatMessages(id: string, list: ServerMessage[]): ChatMessage[] {
  return list.map((m, i) => ({
    id: `server-${id}-${i}`,
    role: m.role, content: m.content || "", citations: m.citations || undefined,
    cards: m.cards || undefined, proposals: m.proposals || undefined,
  }));
}

/** Whether the server holds the reply to `asked`: the conversation grew past the `before` messages
 *  it had, ends with the assistant, and its last question is the one just asked. */
export function replySaved(list: ServerMessage[], asked: string, before: number): boolean {
  if (list.length < before + 2 || list[list.length - 1]?.role !== "assistant") return false;
  for (let i = list.length - 2; i >= 0; i--) {
    if (list[i].role === "user") return (list[i].content || "").trim() === asked.trim();
  }
  return false;
}

const sleep = (ms: number, signal: AbortSignal) => new Promise<void>((resolve) => {
  const t = setTimeout(resolve, ms);
  signal.addEventListener("abort", () => { clearTimeout(t); resolve(); }, { once: true });
});

/**
 * Everything the assistant needs to hold a conversation: history, streaming turn,
 * and session management. Shared by the ⌘K panel and the full Assistant page so the
 * two never drift apart.
 */
export function useAssistantChat() {
  const pathname = usePathname() || "/";
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [conversationId, setConversationId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [status, setStatus] = useState("");
  const abortRef = useRef<AbortController | null>(null);

  const loadConversations = useCallback(async () => {
    try {
      const res = await aiApi.listConversations();
      setConversations(res.data || []);
    } catch { /* ignore */ }
  }, []);

  // Drop the in-flight stream when the component using this hook goes away.
  useEffect(() => () => abortRef.current?.abort(), []);

  const newChat = useCallback(() => {
    abortRef.current?.abort();
    setConversationId(null);
    setMessages([]);
    setStatus("");
  }, []);

  const openConversation = useCallback(async (id: string) => {
    try {
      const res = await aiApi.getConversation(id);
      setConversationId(id);
      setMessages(toChatMessages(id, res.data?.messages || []));
    } catch { /* ignore */ }
  }, []);

  const deleteConversation = useCallback(async (id: string) => {
    try {
      await aiApi.deleteConversation(id);
      if (id === conversationId) newChat();
      loadConversations();
    } catch { /* ignore */ }
  }, [conversationId, newChat, loadConversations]);

  const renameConversation = useCallback(async (id: string, title: string) => {
    const clean = title.trim();
    if (!clean) return;
    try {
      await aiApi.renameConversation(id, clean);
      loadConversations();
    } catch { /* ignore */ }
  }, [loadConversations]);

  const send = useCallback(async (text?: string) => {
    const body = (text ?? input).trim();
    if (!body || streaming) return;
    setInput("");
    // fix-walist: messages known to be on the server (loaded from it), to tell this turn's reply from an
    // older answer to the same question. Never more than the server has, so a recovery cannot wait forever.
    const before = conversationId ? messages.filter((m) => m.id.startsWith("server-")).length : 0;
    setMessages((m) => [...m, { id: newId(), role: "user", content: body },
                              { id: newId(), role: "assistant", content: "" }]);
    setStreaming(true);
    setStatus("Thinking…");
    const ctrl = new AbortController();
    abortRef.current = ctrl;

    // Each of these replaces the trailing assistant message as events stream in. Never mutate:
    // StrictMode runs updaters twice in dev, and an in-place `+=` would double every chunk.
    const patchLast = (patch: (m: ChatMessage) => Partial<ChatMessage>) =>
      setMessages((m) => {
        const last = m[m.length - 1];
        if (last?.role !== "assistant") return m;
        return [...m.slice(0, -1), { ...last, ...patch(last) }];
      });
    let convId = conversationId;
    let finished = false;

    // fix-walist: the stream dropped. Show the saved reply, or poll for it while the server finishes.
    const recover = async () => {
      if (!convId) {
        patchLast((m) => ({ content: `${m.content}${m.content ? "\n\n" : ""}⚠️ ${DROPPED_NO_ID}` }));
        loadConversations();
        return;
      }
      const cid = convId;
      const note = `*${DROPPED_STILL_WORKING}…*`;
      const withNote = (text: string, extra: string) => `${text}${text ? "\n\n" : ""}${extra}`;
      patchLast((m) => ({ content: withNote(m.content, note) }));
      setStatus(DROPPED_STILL_WORKING);
      const until = Date.now() + RECOVER_FOR_MS;
      for (let first = true; ; first = false) {
        if (!first) await sleep(RECOVER_POLL_MS, ctrl.signal);
        if (ctrl.signal.aborted) return;
        try {
          const res = await aiApi.getConversation(cid);
          const list: ServerMessage[] = res.data?.messages || [];
          if (replySaved(list, body, before)) {
            if (ctrl.signal.aborted) return;
            setMessages(toChatMessages(cid, list));
            loadConversations();
            return;
          }
        } catch { /* keep polling */ }
        if (Date.now() >= until) break;
      }
      patchLast((m) => {
        const text = m.content.endsWith(note) ? m.content.slice(0, -note.length).trimEnd() : m.content;
        return { content: withNote(text, `⚠️ ${DROPPED_GAVE_UP}`) };
      });
    };

    try {
      await aiApi.chatStream(
        { message: body, conversation_id: conversationId, context: deriveContext(pathname) },
        {
          signal: ctrl.signal,
          onEvent: (ev) => {
            const t = ev.type as string;
            // fix-walist: keep the first conversation id the server names, for a recovery before `done`.
            if (ev.conversation_id && !convId) {
              convId = String(ev.conversation_id);
              setConversationId(convId);
            }
            if (t === "status") setStatus(String(ev.text || ""));
            else if (t === "text") { setStatus(""); patchLast((m) => ({ content: m.content + String(ev.text || "") })); }
            else if (t === "proposal") {
              const { type: _t, ...proposal } = ev;
              patchLast((m) => ({ proposals: [...(m.proposals || []), proposal as unknown as Proposal] }));
            }
            else if (t === "citations") patchLast(() => ({ citations: (ev.items as Citation[]) || [] }));
            else if (t === "records") patchLast((m) => ({ cards: [...(m.cards || []), ev as unknown as RecordsCard] }));
            else if (t === "error") {
              setStatus("");
              patchLast((m) => ({ content: `${m.content}${m.content ? "\n\n" : ""}⚠️ ${ev.text}` }));
            }
            else if (t === "done") {
              finished = true;
              loadConversations();
            }
          },
        },
      );
      // A stream that ends without `done` was cut (a proxy closing it cleanly).
      if (!finished && !ctrl.signal.aborted) await recover();
    } catch (err) {
      // An aborted stream is a deliberate cancel (Stop / new chat / unmount), not a failure.
      if (ctrl.signal.aborted) patchLast((m) => ({ content: m.content ? `${m.content} *(stopped)*` : "*(stopped)*" }));
      // The server refused the turn (no stream was started): nothing to recover.
      else if (err instanceof Error && err.message.startsWith("AI chat failed (")) {
        const { status, detail } = err as Error & { status?: number; detail?: string };
        const why = status === 409 && detail ? detail : "Connection error. Please try again.";
        patchLast((m) => ({ content: `${m.content}\n\n⚠️ ${why}` }));
      }
      else await recover();
    } finally {
      setStreaming(false);
      setStatus("");
    }
  }, [input, streaming, conversationId, messages, pathname, loadConversations]);

  const stop = useCallback(() => abortRef.current?.abort(), []);

  /** A confirm card was applied or discarded: show it here and tell the server (best-effort), so a
   *  reopened chat and the assistant's next turn both know the outcome. */
  const resolveProposal = useCallback(
    (proposalId: string, status: ProposalOutcome, result?: { id?: string; label?: string }) => {
      setMessages((ms) => ms.map((m) => (m.proposals?.some((p) => p.proposal_id === proposalId)
        ? { ...m, proposals: m.proposals.map((p) => (p.proposal_id === proposalId ? { ...p, status, result } : p)) }
        : m)));
      if (conversationId) {
        aiApi.setProposalStatus(conversationId, proposalId, { status, result }).catch(() => { /* ignore */ });
      }
    }, [conversationId]);

  return {
    conversations, conversationId, messages, input, setInput, streaming, status,
    loadConversations, newChat, openConversation, deleteConversation, renameConversation, send, stop,
    resolveProposal,
  };
}
