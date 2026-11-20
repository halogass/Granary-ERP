"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { MessageCircle, X } from "lucide-react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Card, CardBody, CardHeader, IconButton, Pill, Skeleton } from "@/components/ui";

export interface OwnerTopic {
  id: string; kind: string; tag: string; status: string; text: string; urgent: boolean;
  sent_at: string | null; card_id: string | null; ref_type: string | null; ref_id: string | null;
}

const KIND: Record<string, string> = {
  approval: "Approval", quotation_consult: "Quotation", proposal_review: "Proposal", question: "Question",
  alert: "Alert", brief: "Brief",
};
const STATUS: Record<string, string> = { queued: "not sent yet", open: "sent", waiting: "waiting for you" };

/** A topic's WhatsApp text for a two-line preview: bold and italic marks (*x*, **x**, _x_) dropped. */
export function plainTopicText(text: string): string {
  return (text || "").replace(/\*\*(.+?)\*\*/g, "$1").replace(/\*(\S(?:.*?\S)?)\*/g, "$1").replace(/(^|\s)_(\S(?:.*?\S)?)_(?=\s|$)/g, "$1$2");
}

/** Nary overview: what Nary has raised with the Owner on WhatsApp and is still open. */
export default function OwnerTopics() {
  const [topics, setTopics] = useState<OwnerTopic[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try { setTopics((await naryApi.ownerTopics("open")).data.topics); } catch (e) { setError(extractApiError(e)); }
  }, []);
  useEffect(() => { load(); }, [load]);

  const close = async (id: string) => {
    try { await naryApi.closeOwnerTopic(id); await load(); } catch (e) { setError(extractApiError(e)); }
  };

  return (
    <Card>
      <CardHeader title={<span className="inline-flex items-center gap-2"><MessageCircle className="size-4 text-fg-muted" aria-hidden />Open with you on WhatsApp</span>} />
      <CardBody className="py-2">
        {topics === null ? <Skeleton className="my-2 h-10 w-full" /> : topics.length === 0 ? (
          <p className="py-2 text-[13px] text-fg-muted">Nothing open: everything Nary raised with you is settled.</p>
        ) : (
          <ul className="divide-y divide-line-soft">
            {topics.map((t) => (
              <li key={t.id} className="flex items-start gap-3 py-2">
                <Pill tone={t.urgent ? "red" : "neutral"} className="mt-0.5 shrink-0">{KIND[t.kind] ?? t.kind}</Pill>
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium text-fg">{t.tag}</p>
                  <p className="line-clamp-2 whitespace-pre-line text-[13px] text-fg-muted">{plainTopicText(t.text)}</p>
                  <p className="mt-0.5 text-xs text-fg-subtle">
                    {STATUS[t.status] ?? t.status}
                    {t.card_id && <> · <Link href={`/nary/approvals?card=${t.card_id}`} className="text-brand-ink underline">open the card</Link></>}
                  </p>
                </div>
                <IconButton icon={X} size="sm" variant="ghost" label={`Close ${t.tag}`} onClick={() => close(t.id)} />
              </li>
            ))}
          </ul>
        )}
        {error && <p className="mt-2 text-xs text-tone-red">{error}</p>}
      </CardBody>
    </Card>
  );
}
