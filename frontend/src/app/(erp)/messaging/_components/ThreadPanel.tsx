"use client";

import { useEffect, useState, useRef } from "react";
import { X } from "lucide-react";
import Avatar from "@/components/common/Avatar";
import { EmptyState, IconButton, Skeleton } from "@/components/ui";
import { cn } from "@/lib/utils";
import { messagingApi } from "@/lib/api";
import { Message } from "./MessageBubble";
import Composer from "./Composer";
import { MentionItem } from "./MentionDropdown";

interface Props {
  rootMessage: Message;
  myId: string;
  onClose: () => void;
  onSend: (payload: {
    content: string; message_type: string;
    file_url?: string; file_name?: string; file_size?: number; mime_type?: string;
    mentions: MentionItem[];
  }) => Promise<void>;
}

function formatTime(date: string) {
  return new Date(date).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
}

export default function ThreadPanel({ rootMessage, myId, onClose, onSend }: Props) {
  const [replies, setReplies] = useState<Message[]>([]);
  const [loading, setLoading] = useState(true);
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setLoading(true);
    messagingApi.getThread(rootMessage.id, rootMessage.source).then((r) => {
      setReplies(r.data);
    }).catch(() => setReplies([])).finally(() => setLoading(false));
  }, [rootMessage.id, rootMessage.source]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [replies]);

  const handleSend = async (payload: {
    content: string; message_type: string;
    file_url?: string; file_name?: string; file_size?: number; mime_type?: string;
    mentions: MentionItem[];
  }) => {
    await onSend(payload);
    // Refresh thread
    const r = await messagingApi.getThread(rootMessage.id, rootMessage.source);
    setReplies(r.data);
  };

  return (
    <aside aria-label="Thread"
      className="fixed inset-0 z-40 flex w-full flex-col bg-canvas pb-[env(safe-area-inset-bottom)] md:static md:z-auto md:w-80 md:shrink-0 md:border-l md:border-line md:pb-0">
      <div className="flex items-center justify-between border-b border-line bg-surface px-4 py-2.5">
        <h2 className="text-sm font-semibold text-fg">Thread</h2>
        <IconButton icon={X} label="Close thread" onClick={onClose} />
      </div>

      {/* Root message */}
      <div className="border-b border-line bg-surface px-4 py-3">
        <div className="flex items-start gap-2">
          <Avatar name={rootMessage.sender_name} size="sm" />
          <div className="min-w-0">
            <p className="text-xs font-medium text-fg">{rootMessage.sender_name}</p>
            <p className="mt-0.5 text-[13px] leading-relaxed text-fg">{rootMessage.content}</p>
            <p className="mt-1 text-xs text-fg-subtle tabular">{formatTime(rootMessage.created_at)}</p>
          </div>
        </div>
      </div>

      {/* Replies */}
      <div className="flex-1 space-y-3 overflow-y-auto px-4 py-3">
        {loading ? (
          <div className="space-y-3" aria-busy aria-label="Loading replies">
            {[0, 1].map((i) => (
              <div key={i} className="flex gap-2">
                <Skeleton className="size-7 rounded-full" />
                <Skeleton className="h-8 flex-1" />
              </div>
            ))}
          </div>
        ) : replies.length === 0 ? (
          <EmptyState compact title="No replies yet" description="Start the thread." />
        ) : (
          replies.map((r) => {
            const isMe = r.sender_id === myId;
            return (
              <div key={r.id} className={cn("flex items-start gap-2", isMe && "flex-row-reverse")}>
                <Avatar name={r.sender_name} size="sm" />
                <div className={cn("flex max-w-[85%] flex-col", isMe ? "items-end" : "items-start")}>
                  {!isMe && <p className="mb-0.5 text-xs font-medium text-fg-muted">{r.sender_name}</p>}
                  <div className={cn("rounded-lg border px-3 py-2 text-[13px] leading-relaxed text-fg",
                    isMe ? "border-brand/20 bg-brand/[0.07]" : "border-line bg-surface")}>
                    {r.content}
                  </div>
                  <span className="mt-0.5 text-xs text-fg-subtle tabular">{formatTime(r.created_at)}</span>
                </div>
              </div>
            );
          })
        )}
        <div ref={bottomRef} />
      </div>

      <Composer
        placeholder="Reply in thread"
        onSend={handleSend}
      />
    </aside>
  );
}
