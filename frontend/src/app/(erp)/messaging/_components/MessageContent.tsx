"use client";

// The one renderer for messaging text: mention tokens become chips, the rest gets the small safe
// markdown set. The channel view (MessageBubble) and the thread panel both use it, so a message
// looks the same wherever it is shown.

import { useRouter } from "next/navigation";
import { cn } from "@/lib/utils";
import { MENTION_TOKEN_RE, mentionKey, mentionLabel, parseMentionToken, type MessageMention } from "@/lib/messageMentions";

const ENTITY_ROUTE: Record<string, (id: string) => string> = {
  user: (id) => `/team/${id}`,
  project: (id) => `/projects/${id}`,
  deal: (id) => `/crm/deals/${id}`,
  sop: (id) => `/knowledge/${id}`,
  product: (id) => `/products/${id}`,
};

const MENTION_CHIP_COLORS: Record<string, string> = {
  user: "bg-tone-violet-soft text-tone-violet",
  project: "bg-tone-blue-soft text-tone-blue",
  deal: "bg-tone-blue-soft text-tone-blue",
  sop: "bg-tone-orange-soft text-tone-orange",
  product: "bg-tone-neutral-soft text-tone-neutral",
};

export function MentionChip({ type, id, name }: { type: string; id: string; name?: string }) {
  const router = useRouter();
  const route = ENTITY_ROUTE[type]?.(id);
  const colorClass = MENTION_CHIP_COLORS[type] ?? "bg-tone-neutral-soft text-tone-neutral";
  const cls = cn("inline-flex items-center rounded px-1.5 py-0.5 text-xs font-medium", colorClass);
  const label = mentionLabel(type, name);
  if (!route) return <span className={cls} data-mention={type}>{label}</span>;
  return (
    <button type="button" data-mention={type} onClick={() => router.push(route)} className={cn(cls, "hover:opacity-80")}>
      {label}
    </button>
  );
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

function MarkdownText({ text }: { text: string }) {
  // Escape HTML first, then apply safe markdown substitutions
  const rendered = escapeHtml(text)
    .replace(/```([\s\S]*?)```/g, '<pre class="bg-surface-2 text-fg border border-line rounded-lg px-3 py-2 my-1 text-xs overflow-x-auto font-mono whitespace-pre-wrap">$1</pre>')
    .replace(/`([^`]+)`/g, '<code class="bg-surface-2 text-fg rounded px-1 py-0.5 text-xs font-mono">$1</code>')
    .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
    .replace(/_(.+?)_/g, "<em>$1</em>")
    .replace(/\n/g, "<br>");

  return <span dangerouslySetInnerHTML={{ __html: rendered }} />;
}

export default function MessageContent({ content, mentions }: { content: string | null | undefined; mentions?: MessageMention[] | null }) {
  if (!content) return null;
  const byKey = new Map<string, MessageMention>();
  for (const m of mentions ?? []) byKey.set(mentionKey(m), m);

  const parts: React.ReactNode[] = [];
  const segments = content.replace(MENTION_TOKEN_RE, (match) => `\x00${match}\x00`).split("\x00");
  segments.forEach((seg, i) => {
    if (!seg) return;
    const parsed = parseMentionToken(seg);
    if (parsed) {
      const known = byKey.get(seg);
      parts.push(<MentionChip key={`m-${i}`} type={parsed.type} id={parsed.id} name={known?.name} />);
    } else {
      parts.push(<MarkdownText key={`t-${i}`} text={seg} />);
    }
  });
  return <>{parts}</>;
}
