"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { Sparkles, X, Plus, History, Maximize2 } from "lucide-react";
import { useAssistantChat } from "./useAssistantChat";
import { MessageList, Composer, ConversationList, EmptyState } from "./AssistantThread";
import { IconButton } from "@/components/ui";

export default function AssistantLauncher() {
  const pathname = usePathname() || "/";
  const [open, setOpen] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const chat = useAssistantChat();
  const { loadConversations, newChat, openConversation } = chat;

  // Ctrl J / Cmd J toggles the assistant (Ctrl K is the command palette, which can also open it).
  useEffect(() => {
    const onOpen = () => setOpen(true);
    window.addEventListener("assistant:open", onOpen);
    return () => window.removeEventListener("assistant:open", onOpen);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "j") {
        e.preventDefault();
        setOpen((v) => !v);
      } else if (e.key === "Escape" && open) {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  useEffect(() => {
    if (open) loadConversations();
  }, [open, loadConversations]);

  // Nary's chat page is the same chat; don't float a second one on top of it.
  if (pathname.startsWith("/nary/chat") || pathname.startsWith("/assistant")) return null;

  return (
    <>
      {/* Floating launcher */}
      {!open && (
        <button
          type="button"
          onClick={() => setOpen(true)}
          title="Ask AI (Ctrl J)"
          className="fixed bottom-20 right-4 z-40 flex min-h-11 items-center gap-2 rounded-full bg-fg py-3 pl-3 pr-4 text-surface shadow-pop transition-opacity hover:opacity-90 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)] md:bottom-6 md:right-6"
        >
          <Sparkles size={18} aria-hidden />
          <span className="hidden text-sm font-medium sm:inline">Ask AI</span>
          <span className="sr-only sm:hidden">Ask AI</span>
        </button>
      )}

      {/* Slide-over panel */}
      {open && (
        <div className="fixed inset-0 z-50 flex justify-end">
          <div className="absolute inset-0 bg-black/40" onClick={() => setOpen(false)} aria-hidden />
          <div role="dialog" aria-modal="true" aria-label="Nary"
            className="relative flex h-full w-full flex-col border-l border-line bg-surface-raised text-fg shadow-pop sm:w-[440px]">
            {/* Header */}
            <div className="flex items-center justify-between border-b border-line px-4 py-2">
              <div className="flex items-center gap-2">
                <Sparkles size={18} className="text-tone-violet" aria-hidden />
                <span className="text-sm font-semibold">Nary</span>
              </div>
              <div className="flex items-center gap-0.5">
                <Link href={chat.conversationId ? `/nary/chat?c=${chat.conversationId}` : "/nary/chat"}
                  onClick={() => setOpen(false)} title="Open full page" aria-label="Open full page"
                  className="flex size-11 items-center justify-center rounded-lg text-fg-muted hover:bg-surface-2 hover:text-fg md:size-8"><Maximize2 size={16} aria-hidden /></Link>
                <IconButton icon={History} label="History" onClick={() => setShowHistory((v) => !v)} aria-pressed={showHistory} />
                <IconButton icon={Plus} label="New chat" onClick={() => { newChat(); setShowHistory(false); }} />
                <IconButton icon={X} label="Close" onClick={() => setOpen(false)} />
              </div>
            </div>

            {showHistory ? (
              <ConversationList
                conversations={chat.conversations}
                activeId={chat.conversationId}
                onOpen={(id) => { openConversation(id); setShowHistory(false); }}
                onRename={chat.renameConversation}
                onDelete={chat.deleteConversation}
              />
            ) : (
              <MessageList messages={chat.messages} streaming={chat.streaming} status={chat.status}
                onAskMore={chat.send} onResolveProposal={chat.resolveProposal} empty={<EmptyState compact />} />
            )}

            {!showHistory && (
              <Composer value={chat.input} onChange={chat.setInput} onSend={() => chat.send()}
                onStop={chat.stop} streaming={chat.streaming} />
            )}
          </div>
        </div>
      )}
    </>
  );
}
