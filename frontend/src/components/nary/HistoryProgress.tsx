"use client";

import { useState } from "react";
import { AlertTriangle, Search, Zap } from "lucide-react";
import { Button, Card, CardHeader, Input } from "@/components/ui";
import type { ImportChat } from "@/components/nary/HistoryReview";

const STEP: Record<string, string> = {
  waiting: "waiting", running: "importing", done: "imported", failed: "failed", skipped: "skipped",
  classifying: "sorting", classified: "to read", reading: "reading",
};

/** The import's progress per chat, with a search and "Import now": that chat's newest messages at once (so Nary can
 *  write to it), the rest first in line. */
export default function HistoryProgress({ rows, onImportNow }: {
  rows: ImportChat[]; onImportNow: (chatId: string) => Promise<void>;
}) {
  const [filter, setFilter] = useState("");
  const [working, setWorking] = useState<string | null>(null);
  const q = filter.trim().toLowerCase();
  const shown = rows.filter((r) => !q || (r.name || "").toLowerCase().includes(q) || r.chat_id.includes(q));
  const now = async (id: string) => {
    setWorking(id);
    try { await onImportNow(id); } finally { setWorking(null); }
  };

  return (
    <Card>
      <CardHeader title="Progress" actions={
        <Input type="search" autoComplete="off" leading={Search} value={filter} onChange={(e) => setFilter(e.target.value)}
          placeholder="Search a name or number" aria-label="Search the import" className="w-full sm:w-64" />
      } />
      <ul className="max-h-96 overflow-y-auto">
        {shown.map((r) => (
          <li key={r.chat_id} className="flex items-center gap-3 border-b border-line-soft px-4 py-2 text-sm last:border-0 even:bg-surface-2/60">
            <span className="min-w-0 flex-1 truncate text-fg">{r.name || r.chat_id}</span>
            <span className="text-xs text-fg-muted tabular">{r.imported_count} msgs</span>
            <span className="w-40 text-right text-xs text-fg-muted">
              {STEP[r.import_status] ?? r.import_status}{r.import_status === "done" ? ` · ${STEP[r.read_status] ?? r.read_status}` : ""}
              {r.classification === "not_business" ? " (not business)" : ""}
            </span>
            {r.error && <span title={r.error} className="text-tone-red"><AlertTriangle className="size-4" aria-label={`Error: ${r.error}`} /></span>}
            {["waiting", "running", "failed"].includes(r.import_status) ? (
              <Button size="sm" disabled={working !== null && working !== r.chat_id} loading={working === r.chat_id} onClick={() => now(r.chat_id)}
                title="Import this chat's newest messages now; the rest comes first in line">
                {working !== r.chat_id && <Zap aria-hidden />} Import now
              </Button>
            ) : <span className="w-[100px]" aria-hidden />}
          </li>
        ))}
        {shown.length === 0 && <li className="px-4 py-3 text-xs text-fg-muted">No chat matches.</li>}
      </ul>
    </Card>
  );
}
