"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { BookOpen } from "lucide-react";
import { financeApi } from "@/lib/api";

interface SourceEntry {
  id: string;
  entry_number: string;
  entry_date: string;
  status: string;
  reversed: boolean;
}

interface Props {
  /** Journal source, e.g. "invoice", "payment", "payroll". */
  source: string;
  sourceId: string;
  /** More sources of the same document id, e.g. ["advance_application", "credit_note"]. */
  extraSources?: string[];
  className?: string;
  /** Bump to refetch after an action that posts to the ledger. */
  refreshKey?: unknown;
}

/** "Journal JE-..." links to the automatic ledger entries of a document. Renders nothing when
 *  there are none or the user can't view the journal. */
export default function JournalLink({ source, sourceId, extraSources, className, refreshKey }: Props) {
  const [entries, setEntries] = useState<SourceEntry[]>([]);
  const sourcesKey = [source, ...(extraSources ?? [])].join(",");

  useEffect(() => {
    if (!sourceId) return;
    let alive = true;
    Promise.all(
      sourcesKey.split(",").map(s =>
        financeApi.journalBySource(s, sourceId)
          .then(res => (res.data ?? []) as SourceEntry[])
          .catch(() => [] as SourceEntry[]),
      ),
    ).then(lists => {
      if (!alive) return;
      const seen = new Set<string>();
      setEntries(lists.flat().filter(e => (seen.has(e.id) ? false : (seen.add(e.id), true))));
    });
    return () => { alive = false; };
  }, [sourcesKey, sourceId, refreshKey]);

  if (entries.length === 0) return null;
  return (
    <span className={`inline-flex flex-wrap items-center gap-x-2 gap-y-0.5 ${className ?? ""}`}>
      {entries.map(e => (
        <Link
          key={e.id}
          href={`/finance/journal?entry=${e.id}`}
          onClick={ev => ev.stopPropagation()}
          title={`${e.status}${e.reversed ? ", reversed" : ""}, ${e.entry_date}`}
          className={`inline-flex items-center gap-1 text-xs font-medium hover:underline ${e.reversed ? "text-fg-subtle line-through" : "text-brand-ink"}`}
        >
          <BookOpen className="size-3" aria-hidden />
          Journal {e.entry_number}
        </Link>
      ))}
    </span>
  );
}
