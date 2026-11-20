"use client";

import { useEffect, useRef, useState } from "react";
import { useSearchParams } from "next/navigation";
import { Sparkles, Plus, PanelLeft, BookOpen, TrendingUp, FileText, FolderKanban } from "lucide-react";
import { useAssistantChat } from "@/components/ai/useAssistantChat";
import { MessageList, Composer, ConversationList } from "@/components/ai/AssistantThread";
import NaryTabs from "@/components/nary/NaryTabs";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import { useUrlParam } from "@/hooks/useUrlParam";
import { Button, FullHeightPage, IconButton } from "@/components/ui";

const STARTERS = [
  { icon: BookOpen,    text: "What does our SOP say about approving a quotation discount?" },
  { icon: TrendingUp,  text: "Summarise my open deals and which ones look stalled." },
  { icon: FileText,    text: "Which invoices are overdue right now?" },
  { icon: FolderKanban, text: "What are my tasks due this week?" },
];

/** Nary's chat (formerly the Granary Assistant): the first Nary tab, the only one most staff see. */
export default function NaryChatPage() {
  const searchParams = useSearchParams();
  const chat = useAssistantChat();
  const { loadConversations, openConversation } = chat;
  // Phones: the chat list is its own pane, in the URL so the phone Back button returns to the chat.
  const [listParam, setListParam] = useUrlParam("chats");
  const showList = listParam === "1";
  const setShowList = (on: boolean) => setListParam(on ? "1" : null);
  const { can } = usePermissions();
  const [attaching, setAttaching] = useState<string | null>(null);

  // A file: stored first, then sent with a line naming it. Meeting notes and recordings go to their meeting
  // (attach_meeting_notes); any other file (a customer's PDF, a photo, a spreadsheet) is read with read_attachment.
  const attach = async (file: File) => {
    setAttaching(`Uploading ${file.name}...`);
    try {
      const r = await naryApi.chatFileUpload(file, (p) => setAttaching(`Uploading ${file.name}... ${p}%`));
      const said = chat.input.trim();
      chat.send(said ? `${said}\n${r.data.chat_line}` : r.data.chat_line);
    } catch (e) {
      setAttaching(null);
      window.alert(extractApiError(e));
      return;
    }
    setAttaching(null);
  };

  useEffect(() => { loadConversations(); }, [loadConversations]);

  // Deep link from the Ctrl J panel's "open full page" button (and old /assistant?c= links).
  const openedFromUrl = useRef<string | null>(null);
  useEffect(() => {
    const cid = searchParams.get("c");
    if (cid && cid !== openedFromUrl.current) {
      openedFromUrl.current = cid;
      openConversation(cid);
    }
  }, [searchParams, openConversation]);

  return (
    <FullHeightPage className="flex-col bg-surface">
      <div className="shrink-0 px-4 pt-4 md:px-6 md:pt-5"><NaryTabs /></div>
      <div className="-mt-4 flex min-h-0 flex-1 overflow-hidden">
      {/* Conversations */}
      <aside className={`${showList ? "flex" : "hidden"} w-full shrink-0 flex-col border-r border-line bg-surface-2 md:flex md:w-64`}>
        <div className="border-b border-line px-3 py-2.5">
          <p className="text-xs font-semibold text-fg-muted">Your chats</p>
        </div>
        <ConversationList
          conversations={chat.conversations}
          activeId={chat.conversationId}
          onOpen={(id) => { openConversation(id); setShowList(false); }}
          onRename={chat.renameConversation}
          onDelete={chat.deleteConversation}
          onNew={() => { chat.newChat(); setShowList(false); }}
        />
      </aside>

      {/* Thread */}
      <section className={`${showList ? "hidden" : "flex"} min-w-0 flex-1 flex-col md:flex`}>
        <header className="flex items-center justify-between gap-2 border-b border-line bg-surface px-4 py-2">
          <div className="flex min-w-0 items-center gap-2">
            <IconButton icon={PanelLeft} label="Your chats" className="md:hidden" onClick={() => setShowList(true)} />
            <Sparkles size={18} className="shrink-0 text-tone-violet" aria-hidden />
            <div className="min-w-0">
              <p className="truncate text-sm font-semibold text-fg">Nary</p>
              <p className="truncate text-xs text-fg-muted">
                Reads only what your permissions allow · suggests, never sends
              </p>
            </div>
          </div>
          <Button size="sm" onClick={chat.newChat} title="New chat">
            <Plus aria-hidden /> New chat
          </Button>
        </header>

        <MessageList
          messages={chat.messages}
          streaming={chat.streaming}
          status={chat.status}
          onAskMore={chat.send}
          onResolveProposal={chat.resolveProposal}
          empty={
            <div className="mx-auto mt-10 max-w-2xl px-2 md:mt-16">
              <div className="text-center">
                <Sparkles size={36} className="mx-auto mb-3 text-tone-violet" aria-hidden />
                <p className="text-base font-semibold text-fg">How can I help?</p>
                <p className="mt-2 text-xs text-fg-muted">
                  Ask about SOPs, your pipeline, projects, or invoices. I only see what your
                  permissions allow, and I suggest; I never send or change anything.
                </p>
              </div>
              <div className="mt-6 grid gap-2 sm:grid-cols-2">
                {STARTERS.map(({ icon: Icon, text }) => (
                  <button key={text} type="button" onClick={() => chat.send(text)}
                    className="flex min-h-11 items-start gap-2 rounded-lg border border-line px-3 py-2.5 text-left text-[13px] text-fg hover:border-line-strong hover:bg-surface-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-[rgb(var(--focus)/0.7)]">
                    <Icon size={14} className="mt-0.5 shrink-0 text-brand-ink" aria-hidden />
                    {text}
                  </button>
                ))}
              </div>
            </div>
          }
        />

        <Composer value={chat.input} onChange={chat.setInput} onSend={() => chat.send()}
          onStop={chat.stop} streaming={chat.streaming} autoFocus
          onAttach={attach} attaching={attaching}
          attachAccept={can("nary:approve")
            ? ".pdf,.docx,.xlsx,.txt,.csv,.md,.srt,.vtt,image/*,audio/*,video/mp4,video/webm,.mp3,.m4a,.wav,.ogg,.opus,.webm,.mp4"
            : ".pdf,.docx,.xlsx,.txt,.csv,.md,image/*"}
          attachTitle={can("nary:approve")
            ? "Attach a file for Nary to read (PDF, Word, Excel, photo) or meeting notes / a recording"
            : "Attach a file for Nary to read (PDF, Word, Excel, photo)"} />
      </section>
      </div>
    </FullHeightPage>
  );
}
