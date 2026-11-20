"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "next/navigation";
import { naryApi } from "@/lib/api";
import NaryTabs from "@/components/nary/NaryTabs";
import { DataTable, KeyValues, Skeleton, StatusPill, type ColumnDef } from "@/components/ui";

interface Run {
  id: string; trigger_type: string; trigger_ref: string | null; job: string; context: string; sop_ref: string | null;
  model: string | null; status: string; outcome: string | null; error: string | null; prompt_tokens: number;
  completion_tokens: number; cache_read_tokens: number; est_cost: number; started_at: string;
}
interface Step { seq: number; kind: string; name: string | null; input: unknown; output: unknown }

const fmtTime = (iso: string) => new Date(iso).toLocaleString("en-GB");

/** A run's steps, fetched once when its row is first unfolded. */
function RunSteps({ run, cache, onLoaded }: { run: Run; cache: Record<string, Step[]>; onLoaded: (id: string, s: Step[]) => void }) {
  const steps = cache[run.id];
  useEffect(() => {
    if (steps) return;
    naryApi.run(run.id).then((r) => onLoaded(run.id, r.data.steps)).catch(() => onLoaded(run.id, []));
  }, [run.id, steps, onLoaded]);
  return (
    <div className="space-y-3">
      <KeyValues cols={3} items={[
        { label: "Trigger", value: `${run.trigger_type}${run.trigger_ref ? ` (${run.trigger_ref})` : ""}` },
        { label: "Tokens in / out / cached", value: `${run.prompt_tokens} / ${run.completion_tokens} / ${run.cache_read_tokens}`, numeric: true },
        { label: run.error ? "Error" : "Outcome", value: run.error || run.outcome || "-" },
      ]} />
      {!steps ? <Skeleton className="h-16 w-full" /> : steps.length === 0 ? (
        <p className="text-[13px] text-fg-muted">No steps recorded.</p>
      ) : (
        <ol className="space-y-1.5">
          {steps.map((s) => (
            <li key={s.seq} className="text-xs">
              <span className="font-medium text-fg">{s.seq}. {s.kind}{s.name ? `: ${s.name}` : ""}</span>
              <pre className="mt-0.5 whitespace-pre-wrap break-words rounded-lg bg-surface-2 p-2 font-mono text-[11px] text-fg-muted">
                {JSON.stringify({ input: s.input, output: s.output }, null, 2)}
              </pre>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

function Runs() {
  const params = useSearchParams();
  const status = params.get("status") || undefined;
  const [runs, setRuns] = useState<Run[] | null>(null);
  const [steps, setSteps] = useState<Record<string, Step[]>>({});
  const load = useCallback(async () => {
    try { setRuns((await fetchAllPages<Run>(naryApi.runs, { status })).rows); } catch { setRuns([]); }
  }, [status]);
  useEffect(() => { load(); }, [load]);
  const onLoaded = useCallback((id: string, s: Step[]) => setSteps((m) => ({ ...m, [id]: s })), []);

  const columns = useMemo<ColumnDef<Run>[]>(() => [
    {
      accessorKey: "job", header: "Job",
      cell: (c) => (
        <div className="min-w-0">
          <p className="font-medium text-fg">{c.row.original.job}</p>
          <p className="max-w-md truncate text-xs text-fg-muted">{c.row.original.error || c.row.original.outcome}</p>
        </div>
      ),
    },
    { accessorKey: "trigger_type", header: "Trigger", meta: { hideOnMobile: true } },
    {
      accessorKey: "sop_ref", header: "SOP", meta: { hideOnMobile: true },
      cell: (c) => c.getValue<string | null>() ? <span className="font-mono text-xs">{c.getValue<string>()}</span> : <span className="text-fg-subtle">-</span>,
    },
    { accessorKey: "model", header: "Model", meta: { hideOnMobile: true }, cell: (c) => c.getValue<string | null>() || "-" },
    { accessorKey: "est_cost", header: "Cost", meta: { numeric: true }, cell: (c) => (c.getValue<number>() ? `$${c.getValue<number>().toFixed(4)}` : "-") },
    { accessorKey: "started_at", header: "Started", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmtTime(c.getValue<string>()) },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill status={c.getValue<string>()} /> },
  ], []);

  return (
    <div className="max-w-6xl">
      <NaryTabs subtitle="Everything Nary did: trigger, SOP, model, steps, cost." />
      <div className="overflow-hidden rounded-lg border border-line bg-surface">
        <DataTable label="Nary runs" columns={columns} data={runs ?? []} getRowId={(r) => r.id} loading={runs === null}
          empty={{ title: status ? "No runs with this status" : "No runs yet", description: "Nary's runs appear here as it works." }}
          renderDetails={(r) => <RunSteps run={r} cache={steps} onLoaded={onLoaded} />} />
      </div>
    </div>
  );
}

export default function NaryRunsPage() {
  return <Suspense fallback={null}><Runs /></Suspense>;
}
