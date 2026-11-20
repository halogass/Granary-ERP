"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Check, Search, Timer, X } from "lucide-react";
import { vendorApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import {
  Button, DataTable, Dialog, Field, Input, KeyValues, ListFrame, PageHeader, SavedViewTabs, Stat, StatGroup,
  StatusPill, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

// Shape of GET /vendor/timesheets.
interface Timesheet {
  id: string;
  work_order_id: string;
  work_order_number: string;
  work_order_title: string;
  vendor_id: string;
  vendor_name: string | null;
  period_start: string;
  period_end: string;
  total_hours: number;
  status: string | null;
  notes: string | null;
  rejection_reason: string | null;
  reviewed_at: string | null;
  created_at: string;
}

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "submitted", label: "To review", filters: { status: "submitted" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "rejected", label: "Rejected", filters: { status: "rejected" } },
  { id: "all", label: "All", filters: {} },
];

const fmtDay = (d: string, year = false) =>
  new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", ...(year ? { year: "numeric" } : {}) });

const hours = (n: number) => `${n.toLocaleString("en-US", { maximumFractionDigits: 2 })}h`;

/** Timesheets vendors submitted against work orders, for the work order approvers to approve or reject. */
export default function VendorTimesheetsReviewPage() {
  const views = useSavedViews<Filters>("vendor.timesheets", VIEWS);
  const status = views.active?.filters.status ?? "";
  const [rows, setRows] = useState<Timesheet[]>([]);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);
  const [rejecting, setRejecting] = useState<Timesheet | null>(null);
  const [reason, setReason] = useState("");
  const { holds } = usePermissions();
  // The API lets the work order approvers review (vendor_mgmt:approve_work_order).
  const canReview = holds("vendor_mgmt:approve_work_order");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<Timesheet>(vendorApi.listTimesheets, status ? { status } : {});
      setRows(rows);
    } catch { setRows([]); } finally { setLoading(false); }
  }, [status]);

  useEffect(() => { load(); }, [load]);

  async function review(t: Timesheet, decision: "approved" | "rejected", why?: string) {
    setBusyId(t.id);
    try {
      await vendorApi.reviewTimesheet(t.id, decision === "rejected" ? { status: decision, rejection_reason: why } : { status: decision });
      toast.success(decision === "approved" ? "Timesheet approved" : "Timesheet rejected");
      setRejecting(null);
      setReason("");
      load();
    } catch (err) {
      toast.error(extractApiError(err, "The timesheet could not be reviewed"));
    } finally { setBusyId(null); }
  }

  const term = q.trim().toLowerCase();
  const visible = term
    ? rows.filter((t) => [t.vendor_name, t.work_order_number, t.work_order_title].some((v) => v?.toLowerCase().includes(term)))
    : rows;
  const pending = rows.filter((t) => t.status === "submitted");
  const pendingHours = pending.reduce((s, t) => s + t.total_hours, 0);

  const columns = useMemo<ColumnDef<Timesheet>[]>(() => [
    {
      id: "vendor", header: "Vendor", accessorFn: (t) => t.vendor_name ?? "",
      cell: ({ row }) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{row.original.vendor_name ?? "-"}</p>
          <p className="truncate text-xs text-fg-muted"><span className="font-mono">{row.original.work_order_number}</span> {row.original.work_order_title}</p>
        </div>
      ),
    },
    {
      id: "period", header: "Period", accessorFn: (t) => t.period_start,
      cell: ({ row }) => <span className="tabular">{fmtDay(row.original.period_start)} to {fmtDay(row.original.period_end, true)}</span>,
    },
    { accessorKey: "total_hours", header: "Hours", meta: { numeric: true }, cell: (c) => hours(c.getValue<number>()) },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill status={c.getValue<string>()} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: ({ row }) => {
        const t = row.original;
        if (!canReview || t.status !== "submitted") return null;
        return (
          <div className="flex items-center justify-end gap-1 whitespace-nowrap">
            <Button size="sm" loading={busyId === t.id} onClick={() => review(t, "approved")}
              aria-label={`Approve timesheet of ${t.vendor_name ?? "vendor"}`}><Check aria-hidden /> Approve</Button>
            <Button size="sm" variant="danger" disabled={busyId === t.id} onClick={() => { setReason(""); setRejecting(t); }}
              aria-label={`Reject timesheet of ${t.vendor_name ?? "vendor"}`}><X aria-hidden /> Reject</Button>
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [canReview, busyId]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="To review" value={pending.length} alert={pending.length > 0} />
        <Stat label="Hours to review" value={hours(pendingHours)} />
      </StatGroup>
      <ListFrame>
        <PageHeader flush title="Vendor timesheets"
          summary="Hours vendors log against their work orders. Approve them before the vendor invoices the time." />
        <SavedViewTabs state={views} />
        <div className="flex flex-wrap items-center gap-2 px-1">
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search vendor or work order"
            aria-label="Search timesheets" className="w-64" />
        </div>
        <DataTable label="Vendor timesheets" columns={columns} data={visible} getRowId={(t) => t.id} loading={loading}
          empty={{ icon: Timer, title: "No timesheets in this view", description: "Vendors submit timesheets from the vendor portal." }}
          renderDetails={(t) => (
            <KeyValues cols={3} items={[
              { label: "Work order", value: <span><span className="font-mono">{t.work_order_number}</span> {t.work_order_title}</span> },
              { label: "Submitted", value: fmtDay(t.created_at, true) },
              { label: "Reviewed", value: t.reviewed_at ? fmtDay(t.reviewed_at, true) : "-" },
              { label: "Notes", value: t.notes || "-" },
              ...(t.rejection_reason ? [{ label: "Rejection reason", value: t.rejection_reason }] : []),
            ]} />
          )} />
      </ListFrame>

      <Dialog open={!!rejecting} onOpenChange={(o) => { if (!o) setRejecting(null); }} size="sm" title="Reject timesheet"
        footer={<>
          <Button onClick={() => setRejecting(null)}>Cancel</Button>
          <Button variant="danger" disabled={!reason.trim()} loading={!!rejecting && busyId === rejecting.id}
            onClick={() => rejecting && review(rejecting, "rejected", reason.trim())}>Reject</Button>
        </>}>
        {rejecting && (
          <div className="space-y-3">
            <p className="text-[13px] text-fg-muted">
              {rejecting.vendor_name ?? "The vendor"}, {hours(rejecting.total_hours)} for {fmtDay(rejecting.period_start)} to {fmtDay(rejecting.period_end, true)}.
              The vendor sees the reason.
            </p>
            <Field label="Reason" required>
              <Textarea rows={3} value={reason} onChange={(e) => setReason(e.target.value)} />
            </Field>
          </div>
        )}
      </Dialog>
    </div>
  );
}
