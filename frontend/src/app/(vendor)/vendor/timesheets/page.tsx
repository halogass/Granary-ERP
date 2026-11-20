"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import { portalVendorApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Timer, Plus } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, Input, ListFrame, PageHeader, Select, Stat, StatGroup, StatusPill, Textarea,
  type ColumnDef,
} from "@/components/ui";

/** A timesheet as GET /vendor/portal/timesheets returns it. */
interface Timesheet {
  id: string;
  work_order_id: string;
  period_start: string;
  period_end: string;
  total_hours: number;
  notes?: string | null;
  status: string;
  rejection_reason?: string | null;
}

/** One of the vendor's work orders (GET /vendor/portal/work-orders). */
interface WorkOrder {
  id: string;
  number?: string | null;
  title: string;
  status: string;
}

const EMPTY_FORM = { work_order_id: "", period_start: "", period_end: "", total_hours: "", notes: "" };

const woLabel = (w: WorkOrder) => (w.number ? `${w.number} · ${w.title}` : w.title);

export default function VendorTimesheetsPage() {
  const [timesheets, setTimesheets] = useState<Timesheet[]>([]);
  const [orders, setOrders] = useState<WorkOrder[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [ts, wo] = await Promise.all([
        portalVendorApi.listMyTimesheets().catch(() => ({ data: [] })),
        portalVendorApi.listMyWorkOrders().catch(() => ({ data: [] })),
      ]);
      setTimesheets(ts.data?.items ?? ts.data ?? []);
      setOrders(wo.data?.items ?? wo.data ?? []);
    } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  // hours are logged against a work order that is running
  const activeOrders = useMemo(() => orders.filter((o) => o.status === "active"), [orders]);
  const orderById = useMemo(() => new Map(orders.map((o) => [o.id, o])), [orders]);

  const hours = Number(form.total_hours);
  const periodOk = !form.period_start || !form.period_end || form.period_end >= form.period_start;
  const canSubmit = !!form.work_order_id && !!form.period_start && !!form.period_end && periodOk
    && Number.isFinite(hours) && hours > 0 && hours <= 9999.99;

  function open() {
    setForm({ ...EMPTY_FORM, work_order_id: activeOrders.length === 1 ? activeOrders[0].id : "" });
    setError("");
    setShowModal(true);
  }

  async function submit() {
    if (!canSubmit) return;
    setSaving(true); setError("");
    try {
      await portalVendorApi.submitTimesheet({
        work_order_id: form.work_order_id,
        period_start: form.period_start,
        period_end: form.period_end,
        total_hours: hours,
        notes: form.notes.trim() || undefined,
      });
      setShowModal(false);
      setForm(EMPTY_FORM);
      await load();
    } catch (e) {
      setError(extractApiError(e, "The timesheet could not be submitted."));
    } finally { setSaving(false); }
  }

  const totalHours = timesheets.reduce((s, t) => s + (Number(t.total_hours) || 0), 0);
  const approvedHours = timesheets.filter((t) => t.status === "approved").reduce((s, t) => s + (Number(t.total_hours) || 0), 0);

  const columns = useMemo<ColumnDef<Timesheet>[]>(() => [
    { id: "period", header: "Period", accessorFn: (t) => t.period_start,
      cell: ({ row }) => (
        <span className="font-medium tabular">
          {new Date(row.original.period_start).toLocaleDateString("en-GB", { day: "numeric", month: "short" })}
          {" to "}
          {new Date(row.original.period_end).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" })}
        </span>
      ) },
    { id: "summary", header: "Work", enableSorting: false, meta: { hideOnMobile: true },
      cell: ({ row }) => {
        const wo = orderById.get(row.original.work_order_id);
        return (
          <span className="line-clamp-1 text-fg-muted">
            {wo && woLabel(wo)}
            {wo && row.original.notes && " · "}
            {row.original.notes}
            {row.original.status === "rejected" && row.original.rejection_reason && (
              <span className="text-tone-red"> Rejected: {row.original.rejection_reason}</span>
            )}
          </span>
        );
      } },
    { accessorKey: "total_hours", header: "Hours", meta: { numeric: true }, cell: (c) => `${c.getValue<number>()}h` },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill status={c.getValue<string>()} /> },
  ], [orderById]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Submissions" value={timesheets.length} />
        <Stat label="Total hours" value={`${totalHours}h`} />
        <Stat label="Approved hours" value={`${approvedHours}h`} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Timesheets" summary="Log your hours for review and payment processing."
          actions={<Button variant="primary" onClick={open}><Plus aria-hidden /> Log hours</Button>} />
        <DataTable label="Timesheets" columns={columns} data={timesheets} getRowId={(t) => t.id} loading={loading}
          empty={{ icon: Timer, title: "No timesheets submitted", description: "Log your hours to send them for review.",
            action: <Button variant="primary" onClick={open}><Plus aria-hidden /> Log hours</Button> }} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Log hours" dismissable={!saving}
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={submit} loading={saving} disabled={!canSubmit}>
            {saving ? "Submitting..." : "Submit"}
          </Button>
        </>}>
        {activeOrders.length === 0 && !loading && (
          <p className="mb-3 text-sm text-fg-muted">
            You have no active work orders. Hours are logged against an active work order; ask your contact to activate yours.
          </p>
        )}
        <FieldGrid cols={2}>
          <Field label="Work order" required className="md:col-span-2">
            <Select value={form.work_order_id} onChange={(e) => setForm({ ...form, work_order_id: e.target.value })}
              placeholder="Choose a work order" options={activeOrders.map((o) => ({ value: o.id, label: woLabel(o) }))} />
          </Field>
          <Field label="Period start" required>
            <Input type="date" value={form.period_start} onChange={(e) => setForm({ ...form, period_start: e.target.value })} />
          </Field>
          <Field label="Period end" required error={periodOk ? undefined : "The period ends before it starts."}>
            <Input type="date" value={form.period_end} onChange={(e) => setForm({ ...form, period_end: e.target.value })} />
          </Field>
          <Field label="Total hours" required className="md:col-span-2">
            <Input type="number" step="0.5" min="0.5" max="9999.99" numeric value={form.total_hours}
              onChange={(e) => setForm({ ...form, total_hours: e.target.value })} placeholder="e.g. 40" />
          </Field>
          <Field label="Notes" className="md:col-span-2">
            <Textarea rows={3} value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} placeholder="What you worked on this period" />
          </Field>
        </FieldGrid>
        {error && <p role="alert" className="mt-3 text-sm text-tone-red">{error}</p>}
      </Dialog>
    </div>
  );
}
