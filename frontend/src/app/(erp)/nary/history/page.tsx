"use client";

import { useCallback, useEffect, useState } from "react";
import { Pause, Play, RefreshCw } from "lucide-react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import NaryTabs from "@/components/nary/NaryTabs";
import HistoryReview, { type ImportChat } from "@/components/nary/HistoryReview";
import HistoryProgress from "@/components/nary/HistoryProgress";
import { Button, Card, CardBody, CardHeader, Checkbox, Pill, Skeleton, StatusPill } from "@/components/ui";

interface ImportState { status: string; started_at?: string; finished_at?: string; style_guide?: boolean }

export default function NaryHistoryPage() {
  const { can } = usePermissions();
  const [state, setState] = useState<ImportState | null>(null);
  const [rows, setRows] = useState<ImportChat[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const { data } = await naryApi.history();
      setState(data.state);
      setRows(data.chats);
    } catch (e) {
      setNotice(extractApiError(e));
      setRows([]);
    }
  }, []);
  useEffect(() => { load(); }, [load]);

  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setNotice(null);
    try { await fn(); await load(); } catch (e) { setNotice(extractApiError(e)); } finally { setBusy(false); }
  };

  if (!can("nary:manage")) {
    return <div className="max-w-5xl"><NaryTabs /><p className="text-sm text-fg-muted">Only Nary&apos;s managers run the history import.</p></div>;
  }
  const included = (rows || []).filter((r) => r.decision === "include");
  const readDone = included.filter((r) => r.read_status === "done");
  const missed = readDone.filter((r) => r.missed);
  const pipeline = readDone.filter((r) => r.opportunity && r.opportunity.status !== "none" && !r.created);

  return (
    <div className="max-w-5xl space-y-4">
      <NaryTabs subtitle="A one-time import of the company WhatsApp: you tick the personal chats first, then Nary reads the business ones slowly in the background." />
      {notice && <p role="alert" className="text-sm text-tone-red">{notice}</p>}

      <Card>
        <CardBody className="flex flex-wrap items-center gap-3">
          <div className="min-w-0 flex-1">
            <p className="inline-flex items-center gap-2 text-sm font-semibold text-fg">
              Import <StatusPill status={state?.status ?? ""} label={state?.status ?? "..."} />
            </p>
            <p className="mt-0.5 text-xs text-fg-muted tabular">
              {included.length} chats to import · {included.filter((r) => r.import_status === "done").length} imported ·
              {" "}{readDone.length} read{state?.finished_at ? ` · finished ${new Date(state.finished_at).toLocaleString()}` : ""}
            </p>
          </div>
          <Button size="sm" disabled={busy} onClick={() => run(() => naryApi.historyRefresh())}>
            <RefreshCw aria-hidden /> Load chats from the phone
          </Button>
          {state?.status === "running" ? (
            <Button size="sm" disabled={busy} onClick={() => run(() => naryApi.historyRun(false))}>
              <Pause aria-hidden /> Pause
            </Button>
          ) : (
            <Button size="sm" variant="primary" disabled={busy || included.length === 0} onClick={() => run(() => naryApi.historyRun(true))}>
              <Play aria-hidden /> Start the import
            </Button>
          )}
        </CardBody>
      </Card>

      {rows === null ? <Skeleton className="h-48 w-full" /> : (
        <>
          <HistoryReview rows={rows} busy={busy} onSubmit={(d) => run(() => naryApi.historyDecide(d))} />

          {pipeline.length > 0 && (
            <Card>
              <CardHeader title="Reconstructed pipeline"
                description="Pick the ones to create as contacts and leads (re-engagement drafts are in Approvals)."
                actions={
                  <Button size="sm" variant="primary" disabled={busy || picked.size === 0}
                    onClick={() => run(async () => { await naryApi.historyCreate([...picked]); setPicked(new Set()); })}>
                    Create {picked.size || ""}
                  </Button>
                } />
              <ul>
                {pipeline.map((r) => (
                  <li key={r.chat_id} className="border-b border-line-soft last:border-0 even:bg-surface-2/60">
                    <label className="flex cursor-pointer gap-3 px-4 py-3 hover:bg-surface-2">
                      <span className="mt-0.5">
                        <Checkbox checked={picked.has(r.chat_id)} aria-label={`Create ${r.person?.company || r.name || r.chat_id}`}
                          onChange={() => { const n = new Set(picked); if (n.has(r.chat_id)) n.delete(r.chat_id); else n.add(r.chat_id); setPicked(n); }} />
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="flex flex-wrap items-center gap-2 text-sm font-medium text-fg">
                          {r.person?.company || r.name || r.chat_id} · {r.opportunity?.title || "opportunity"}
                          {r.opportunity?.status && <StatusPill status={r.opportunity.status} />}
                        </span>
                        <span className="block text-xs text-fg-muted">{r.summary}</span>
                      </span>
                    </label>
                  </li>
                ))}
              </ul>
            </Card>
          )}

          {missed.length > 0 && (
            <Card>
              <CardHeader title={<span className="inline-flex items-center gap-2">Missed opportunities <Pill tone="orange">{missed.length}</Pill></span>} />
              <ul>
                {missed.map((r) => (
                  <li key={r.chat_id} className="border-b border-line-soft px-4 py-2.5 last:border-0 even:bg-surface-2/60">
                    <p className="text-sm font-medium text-fg">{r.person?.company || r.name || r.chat_id}</p>
                    <p className="text-xs text-fg-muted">{r.summary}</p>
                  </li>
                ))}
              </ul>
            </Card>
          )}

          {included.length > 0 && (
            <HistoryProgress rows={included} onImportNow={async (id) => {
              setNotice(null);
              try { await naryApi.historyImportNow(id); } catch (e) { setNotice(extractApiError(e)); }
              await load();
            }} />
          )}
        </>
      )}
    </div>
  );
}
