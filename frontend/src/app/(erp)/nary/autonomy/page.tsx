"use client";

import { useCallback, useEffect, useState } from "react";
import { ArrowUpCircle, Lock } from "lucide-react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import NaryTabs from "@/components/nary/NaryTabs";
import { LEVEL_LABELS } from "@/components/nary/types";
import { Button, DataTable, Select, type ColumnDef } from "@/components/ui";

interface Row {
  action: string; label: string; customer_facing: boolean; level: number; never_promote: boolean; streak: number;
  candidate_since: string | null; approvals: number; edits: number; rejects: number; last_change: string | null;
}
interface Autonomy { actions: Row[]; never_promote: string[]; promotion_count: number }

export default function NaryAutonomyPage() {
  const { can } = usePermissions();
  const manage = can("nary:manage");
  const [data, setData] = useState<Autonomy | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    try { setData((await naryApi.autonomy()).data); } catch (e) { setNotice(extractApiError(e)); }
  }, []);
  useEffect(() => { load(); }, [load]);

  const run = async (fn: () => Promise<unknown>) => {
    setNotice(null);
    try { await fn(); await load(); } catch (e) { setNotice(extractApiError(e)); }
  };

  const columns: ColumnDef<Row>[] = [
    {
      accessorKey: "label", header: "Action",
      cell: (c) => {
        const r = c.row.original;
        return (
          <div className="min-w-0 py-1">
            <p className="font-medium text-fg">{r.label}</p>
            <p className="font-mono text-[11px] text-fg-subtle">{r.action}{r.customer_facing ? " · customer-facing" : ""}</p>
            {r.last_change && <p className="text-[11px] text-fg-muted">{r.last_change}</p>}
          </div>
        );
      },
    },
    {
      accessorKey: "level", header: "Level",
      cell: (c) => {
        const r = c.row.original;
        return (
          <div className="flex flex-wrap items-center gap-1.5">
            {manage ? (
              <Select value={r.level} aria-label={`Level for ${r.action}`} className="w-40"
                onChange={(e) => run(() => naryApi.setLevel(r.action, Number(e.target.value)))}>
                {[0, 1, 2, 3].map((l) => (
                  <option key={l} value={l} disabled={r.never_promote && l > 1}>{LEVEL_LABELS[l]}</option>
                ))}
              </Select>
            ) : LEVEL_LABELS[r.level]}
            {r.never_promote && <span className="inline-flex items-center text-[11px] text-fg-muted"><Lock className="mr-0.5 size-3" aria-hidden />never promoted</span>}
          </div>
        );
      },
    },
    { accessorKey: "streak", header: "Streak", meta: { numeric: true } },
    {
      id: "decisions", header: "Approved / edited / rejected", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.row.original.approvals} / {c.row.original.edits} / {c.row.original.rejects}</span>,
    },
    {
      id: "promote", header: "", enableSorting: false,
      cell: (c) => c.row.original.candidate_since && manage ? (
        <Button size="sm" variant="primary" onClick={() => run(() => naryApi.promote(c.row.original.action))}>
          <ArrowUpCircle aria-hidden /> Promote
        </Button>
      ) : null,
    },
  ];

  return (
    <div className="max-w-6xl">
      <NaryTabs subtitle={data ? `An action earns trust after ${data.promotion_count} approvals in a row without a meaningful edit. You decide every promotion; an edit or reject demotes it.` : undefined} />
      {notice && <p role="alert" className="mb-3 text-sm text-tone-red">{notice}</p>}
      <div className="overflow-hidden rounded-lg border border-line bg-surface">
        <DataTable label="Autonomy per action" columns={columns} data={data?.actions ?? []} getRowId={(r) => r.action}
          loading={!data} maxHeight="none" empty={{ title: "No actions yet" }} />
      </div>
    </div>
  );
}
