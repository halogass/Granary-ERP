"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "next/navigation";
import { Inbox, Search } from "lucide-react";
import { approvalsApi } from "@/lib/api";
import { formatCurrency } from "@/lib/currency";
import { extractApiError } from "@/lib/apiError";
import ApprovalTrail, { ApprovalStatusBadge } from "@/components/approvals/ApprovalTrail";
import DocumentActions from "@/components/approvals/DocumentActions";
import { formatWhen, type ApprovalRequest } from "@/components/approvals/types";
import {
  Card, CardBody, CardHeader, DataTable, FilterChips, Input, ListFrame, PageHeader, Pill, SavedViewTabs,
  useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

type Filters = { scope: string };
const VIEWS: SavedView<Filters>[] = [
  { id: "awaiting_me", label: "Awaiting me", filters: { scope: "awaiting_me" } },
  { id: "submitted_by_me", label: "Submitted by me", filters: { scope: "submitted_by_me" } },
  { id: "involved", label: "Everything I'm involved in", filters: { scope: "involved" } },
];
const CHIP_STATUSES = [
  { id: "pending", label: "Pending" }, { id: "approved", label: "Approved" }, { id: "rejected", label: "Rejected" },
];

/** The trail and the approved document's actions, shown under a row (or on top for a linked request). */
function RequestDetails({ request, onChanged }: { request: ApprovalRequest; onChanged: (u?: ApprovalRequest) => void }) {
  return (
    <div className="max-w-3xl space-y-4">
      <ApprovalTrail key={request.id} request={request} onChanged={onChanged} showDocumentLink />
      {request.status === "approved" && (
        <DocumentActions key={`actions-${request.id}`} entityType={request.entity_type} entityId={request.entity_id} />
      )}
    </div>
  );
}

function ApprovalsInbox() {
  const params = useSearchParams();
  const views = useSavedViews<Filters>("approvals", VIEWS);
  const [rows, setRows] = useState<ApprovalRequest[]>([]);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<ApprovalRequest | null>(null);
  const [error, setError] = useState("");
  const [chipStatus, setChipStatus] = useState("");
  const [search, setSearch] = useState("");

  // ?scope= in the link wins over the remembered view
  const paramScope = params.get("scope");
  const selectView = views.select;
  useEffect(() => {
    if (paramScope && VIEWS.some((v) => v.id === paramScope)) selectView(paramScope);
    // only when the link changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paramScope]);
  const tab = views.active?.filters.scope ?? "awaiting_me";

  const load = useCallback(async () => {
    setLoading(true);
    try {
      // The list caps at 500 rows: ask for that much a page.
      const { rows } = await fetchAllPages<ApprovalRequest>((p) => approvalsApi.list(p), { scope: tab }, 500);
      setRows(rows);
      setError("");
    } catch (e) {
      setRows([]);
      setError(extractApiError(e, "Could not load approvals."));
    } finally {
      setLoading(false);
    }
  }, [tab]);

  useEffect(() => { load(); }, [load]);

  useEffect(() => {
    const id = params.get("request");
    if (!id) return;
    approvalsApi.get(id)
      .then((res) => setSelected(res.data))
      .catch((e) => setError(extractApiError(e, "That approval request is not available to you.")));
  }, [params]);

  function changed(updated?: ApprovalRequest) {
    if (updated) setSelected(updated);
    else if (selected) approvalsApi.get(selected.id).then((res) => setSelected(res.data)).catch(() => setSelected(null));
    load();
    window.dispatchEvent(new Event("approvals:changed"));
  }

  const q = search.trim().toLowerCase();
  const visible = rows.filter((r) => (!chipStatus || r.status === chipStatus) &&
    (!q || r.document_label.toLowerCase().includes(q) || r.entity_label.toLowerCase().includes(q) || (r.submitted_by.name ?? "").toLowerCase().includes(q)));
  const selectedInList = !!selected && visible.some((r) => r.id === selected.id);

  const columns = useMemo<ColumnDef<ApprovalRequest>[]>(() => [
    {
      id: "document", accessorFn: (r) => r.document_label, header: "Document",
      cell: (c) => {
        const r = c.row.original;
        const step = r.steps.find((s) => s.seq === r.current_step_seq);
        return (
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <Pill>{r.entity_label}</Pill>
              <span className="break-words font-medium text-fg">{r.document_label}</span>
            </div>
            <p className="mt-0.5 text-xs text-fg-muted">
              {r.submitted_by.name ?? "-"} · {formatWhen(r.submitted_at)}
              {step && r.status === "pending" && ` · ${step.name}`}
            </p>
          </div>
        );
      },
    },
    {
      id: "amount", accessorFn: (r) => r.amount_base ?? r.amount ?? 0, header: "Amount", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => {
        const r = c.row.original;
        return r.amount != null && r.currency ? <span className="font-medium">{formatCurrency(r.amount, r.currency)}</span> : <span className="text-fg-subtle">-</span>;
      },
    },
    { accessorKey: "status", header: "Status", meta: { align: "right" }, cell: (c) => <ApprovalStatusBadge status={c.row.original.status} /> },
  ], []);

  return (
    <div className="space-y-4">
      {error && <p className="text-sm text-tone-red" role="alert">{error}</p>}

      {selected && !selectedInList && (
        <Card>
          <CardHeader title="Linked approval request" actions={<ApprovalStatusBadge status={selected.status} />} />
          <CardBody><RequestDetails request={selected} onChanged={changed} /></CardBody>
        </Card>
      )}

      <ListFrame>
        <PageHeader flush title="Approvals"
          summary={`${visible.length} request${visible.length !== 1 ? "s" : ""}. Documents waiting for a decision, routed by the SOP approval policies.`} />
        <SavedViewTabs state={views} onSelect={() => { setChipStatus(""); setSelected(null); }} alertIds={["awaiting_me"]}
          counts={tab === "awaiting_me" && !loading ? { awaiting_me: rows.length } : undefined} />
        <FilterChips chips={CHIP_STATUSES.map((s) => ({ ...s, active: chipStatus === s.id }))}
          onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))} onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search approvals" aria-label="Search approvals" className="w-56" />
        </FilterChips>
        <DataTable label="Approval requests" columns={columns} data={visible} getRowId={(r) => r.id} loading={loading}
          expandedId={selectedInList ? selected!.id : null}
          onExpandedChange={(id) => setSelected(id ? visible.find((r) => r.id === id) ?? null : null)}
          empty={{ icon: Inbox, title: tab === "awaiting_me" ? "Nothing is waiting for you" : "No approval requests" }}
          renderDetails={(r) => <RequestDetails request={selected?.id === r.id ? selected : r} onChanged={changed} />} />
      </ListFrame>
    </div>
  );
}

export default function ApprovalsPage() {
  return (
    <Suspense fallback={null}>
      <ApprovalsInbox />
    </Suspense>
  );
}
