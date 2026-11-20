"use client";

import { useState } from "react";
import Link from "next/link";
import { EmptyState, StatusPill } from "@/components/ui";
import MarkdownMessage from "@/components/ai/MarkdownMessage";
import { crmApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Loader2, Mail, MessageCircle, Phone, FileText, Receipt, StickyNote, Users, Video } from "lucide-react";

export interface TimelineEntry {
  kind: string;
  sub_kind?: string | null;
  id: string;
  title: string;
  preview?: string | null;
  direction?: string | null;
  outcome?: string | null;
  at?: string | null;
  href?: string | null;
}

// AI and notes violet/neutral, conversations blue, money and documents orange.
const ICONS: Record<string, { icon: typeof Mail; cls: string }> = {
  email:     { icon: Mail,          cls: "bg-tone-blue-soft text-tone-blue" },
  whatsapp:  { icon: MessageCircle, cls: "bg-tone-blue-soft text-tone-blue" },
  call:      { icon: Phone,         cls: "bg-tone-violet-soft text-tone-violet" },
  meeting:   { icon: Users,         cls: "bg-tone-violet-soft text-tone-violet" },
  demo:      { icon: Video,         cls: "bg-tone-violet-soft text-tone-violet" },
  note:      { icon: StickyNote,    cls: "bg-surface-2 text-fg-muted" },
  quotation: { icon: FileText,      cls: "bg-tone-orange-soft text-tone-orange" },
  invoice:   { icon: Receipt,       cls: "bg-tone-orange-soft text-tone-orange" },
};

function meta(entry: TimelineEntry) {
  // Activities carry the real channel in sub_kind (call/email/whatsapp/...); other sources
  // are identified by kind alone.
  return ICONS[entry.sub_kind ?? ""] ?? ICONS[entry.kind] ?? ICONS.note;
}

function relative(iso?: string | null): string {
  if (!iso) return "";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const days = Math.floor((Date.now() - then) / 86400000);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days}d ago`;
  if (days < 365) return `${Math.floor(days / 30)}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

// The server sends an activity's first 280 characters as its preview (crm/timeline.rs); a preview that long may
// be cut, so it gets "Show more", which loads the whole activity.
export const PREVIEW_CHARS = 280;

export function mayBeLonger(e: TimelineEntry): boolean {
  return e.kind === "activity" && !!e.preview && [...e.preview].length >= PREVIEW_CHARS;
}

type Full = { loading?: boolean; content?: string; error?: string; open?: boolean };

/** 23 Oct: an activity's preview, or its whole text (Markdown) after "Show more". */
function Preview({ entry }: { entry: TimelineEntry }) {
  const [full, setFull] = useState<Full>({});
  if (!entry.preview) return null;
  const longer = mayBeLonger(entry);
  const showMore = async () => {
    if (full.content !== undefined) { setFull({ ...full, open: true }); return; }
    setFull({ loading: true });
    try {
      const { data } = await crmApi.getActivity(entry.id);
      setFull({ content: String(data?.content ?? entry.preview), open: true });
    } catch (e) {
      setFull({ error: extractApiError(e, "The full note could not be loaded. Try again.") });
    }
  };
  if (full.open && full.content !== undefined) {
    return (
      <div className="mt-1 rounded-lg bg-surface-2 p-2 text-[13px]" data-testid="activity-full">
        <MarkdownMessage content={full.content} />
        <button type="button" onClick={() => setFull({ ...full, open: false })}
          className="mt-1.5 text-xs font-medium text-brand-ink hover:underline">Show less</button>
      </div>
    );
  }
  return (
    <>
      <p className="mt-0.5 line-clamp-2 break-words text-[13px] text-fg-muted">{entry.preview}</p>
      {longer && (
        <button type="button" onClick={showMore} disabled={full.loading}
          className="mt-0.5 inline-flex items-center gap-1 text-xs font-medium text-brand-ink hover:underline disabled:opacity-60">
          {full.loading && <Loader2 className="size-3 animate-spin" aria-hidden />}Show more
        </button>
      )}
      {full.error && <p className="text-[11px] text-tone-red mt-0.5">{full.error}</p>}
    </>
  );
}

export default function RecordTimeline({ entries }: { entries: TimelineEntry[] }) {
  if (entries.length === 0) {
    return (
      <EmptyState icon={StickyNote} title="No interactions recorded yet"
        description="Emails, WhatsApp messages, quotations and invoices appear here automatically." />
    );
  }

  return (
    <ol className="relative">
      {entries.map((e, i) => {
        const { icon: Icon, cls } = meta(e);
        const body = (
          <div className="flex gap-3 py-3">
            <div className="relative shrink-0">
              <div className={`flex size-7 items-center justify-center rounded-lg ${cls}`}>
                <Icon className="size-3.5" aria-hidden />
              </div>
              {i < entries.length - 1 && (
                <div className="absolute left-1/2 top-7 bottom-[-12px] w-px -translate-x-1/2 bg-line" />
              )}
            </div>
            <div className="flex-1 min-w-0">
              <div className="flex items-baseline gap-2">
                <p className="truncate text-sm font-medium text-fg">{e.title}</p>
                <span className="ml-auto shrink-0 text-xs text-fg-subtle tabular">{relative(e.at)}</span>
              </div>
              <Preview entry={e} />
              <div className="flex items-center gap-1.5 mt-1">
                <span className="text-[11px] capitalize text-fg-subtle">{e.kind}</span>
                {e.outcome && (
                  <StatusPill status={e.outcome} />
                )}
                {e.direction && e.direction !== "outbound" && (
                  <span className="text-[11px] capitalize text-fg-subtle">{e.direction}</span>
                )}
              </div>
            </div>
          </div>
        );

        return (
          <li key={`${e.kind}-${e.id}`} className="group">
            {e.href
              ? <Link href={e.href} className="-mx-2 block rounded-lg px-2 transition-colors hover:bg-surface-2">{body}</Link>
              : body}
          </li>
        );
      })}
    </ol>
  );
}
