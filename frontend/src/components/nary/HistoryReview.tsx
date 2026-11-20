"use client";

import { useMemo, useState } from "react";
import { Lock, Search } from "lucide-react";
import { Button, Card, CardFooter, CardHeader, Checkbox, Input, Pill } from "@/components/ui";

/** The history import's review screen: every 1:1 chat (name, messages, last date). The Owner ticks the
 *  personal ones BEFORE anything is read; they are excluded for good. Nothing else is shown here. */
export interface ImportChat {
  chat_id: string; name: string | null; message_count: number | null; last_message_at: string | null;
  /** The staff member this chat is linked to: never personal, not imported (Nary answers them there). */
  staff?: string | null;
  saved_on_phone: boolean | null; decision: "pending" | "include" | "exclude"; import_status: string;
  imported_count: number; read_status: string; classification: string | null; summary: string | null;
  missed: boolean | null; created: boolean | null;
  person: { name: string | null; company: string | null } | null;
  opportunity: { status: string; title: string | null; products: string[] } | null; error: string | null;
}

export function pendingDecisions(rows: ImportChat[], personal: Set<string>): Record<string, "include" | "exclude"> {
  const out: Record<string, "include" | "exclude"> = {};
  for (const r of rows) {
    if (r.decision === "pending") out[r.chat_id] = personal.has(r.chat_id) && !r.staff ? "exclude" : "include";
  }
  return out;
}

export default function HistoryReview({ rows, onSubmit, busy }: {
  rows: ImportChat[]; onSubmit: (decisions: Record<string, "include" | "exclude">) => void; busy: boolean;
}) {
  const pending = useMemo(() => rows.filter((r) => r.decision === "pending"), [rows]);
  const [personal, setPersonal] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState("");
  const shown = pending.filter((r) => !filter || (r.name || r.chat_id).toLowerCase().includes(filter.toLowerCase()));
  const toggle = (id: string) => {
    const next = new Set(personal);
    if (next.has(id)) next.delete(id); else next.add(id);
    setPersonal(next);
  };

  if (pending.length === 0) return null;
  return (
    <Card>
      <CardHeader title="Tick the personal chats"
        description={<span className="inline-flex items-center gap-1"><Lock className="size-3.5 shrink-0" aria-hidden /> Ticked chats are never stored, never read by the AI and never answered. The rest are imported.
          Nothing has been read yet.</span>} />
      <div className="flex flex-wrap items-center gap-2 border-b border-line px-4 py-2">
        <Input type="search" autoComplete="off" leading={Search} value={filter} onChange={(e) => setFilter(e.target.value)}
          placeholder="Search a name" aria-label="Search a name" className="sm:w-72" />
        <span className="ml-auto text-xs text-fg-muted tabular">{personal.size} personal · {pending.length - personal.size} to import</span>
      </div>
      <ul className="max-h-[28rem] overflow-y-auto">
        {shown.map((r) => (
          <li key={r.chat_id} className="border-b border-line-soft last:border-0 even:bg-surface-2/60">
            <label className="flex cursor-pointer items-center gap-3 px-4 py-2 hover:bg-surface-2">
              <Checkbox checked={!r.staff && personal.has(r.chat_id)} disabled={!!r.staff}
                onChange={() => toggle(r.chat_id)} aria-label={`${r.name || r.chat_id} is personal`} />
              <span className="min-w-0 flex-1 truncate text-sm text-fg">{r.name || r.chat_id}</span>
              {r.staff && (
                <span title="Linked to staff: Nary answers them here, so it is never personal and not imported">
                  <Pill tone="violet">Staff: {r.staff}</Pill>
                </span>
              )}
              {r.saved_on_phone && <span className="text-xs text-fg-subtle">saved on phone</span>}
              <span className="w-24 text-right text-xs text-fg-muted tabular">{r.message_count ?? "?"} msgs</span>
              <span className="hidden w-24 text-right text-xs text-fg-muted tabular sm:inline">
                {r.last_message_at ? new Date(r.last_message_at).toLocaleDateString() : ""}
              </span>
            </label>
          </li>
        ))}
      </ul>
      <CardFooter className="justify-start">
        <Button variant="primary" loading={busy} onClick={() => onSubmit(pendingDecisions(rows, personal))}>
          Save: {personal.size} personal, {pending.length - personal.size} to import
        </Button>
      </CardFooter>
    </Card>
  );
}
