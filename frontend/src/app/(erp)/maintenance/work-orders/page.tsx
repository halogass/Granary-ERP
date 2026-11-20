"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, Wrench, Play, CheckCircle2, XCircle, Trash2 } from "lucide-react";
import api from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, IconButton, Input, KeyValues, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, StatusPill, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface WorkOrder {
  id: string; number: string; equipment_id: string; equipment_name: string | null;
  work_type: string; description: string; status: string;
  assigned_to_id: string | null; scheduled_date: string | null;
  started_at: string | null; completed_at: string | null;
  downtime_hours: number | null; cost: number | null; currency: string; notes: string | null;
}
interface Equipment { id: string; name: string; code: string; }

const STATUS_LABELS: Record<string, string> = {
  planned: "Planned", in_progress: "In progress", completed: "Completed", cancelled: "Cancelled",
};

const EMPTY_FORM = { equipment_id: "", work_type: "corrective", description: "", scheduled_date: "", notes: "" };
const EMPTY_COMPLETE = { downtime_hours: "", cost: "", currency: "USD", notes: "" };

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "planned", label: "Planned", filters: { status: "planned" } },
  { id: "in_progress", label: "In progress", filters: { status: "in_progress" } },
  { id: "completed", label: "Completed", filters: { status: "completed" } },
  { id: "cancelled", label: "Cancelled", filters: { status: "cancelled" } },
];

export default function WorkOrdersPage() {
  const views = useSavedViews<Filters>("maintenance.work-orders", VIEWS);
  const statusFilter = views.active?.filters.status ?? "";
  const [wos, setWos] = useState<WorkOrder[]>([]);
  const [equipment, setEquipment] = useState<Equipment[]>([]);
  const [loading, setLoading] = useState(true);
  const [showCreate, setShowCreate] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [creating, setCreating] = useState(false);
  const [completingId, setCompletingId] = useState<string | null>(null);
  const [completeForm, setCompleteForm] = useState(EMPTY_COMPLETE);
  const [completing, setCompleting] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (statusFilter) params.status = statusFilter;
      const [wRes, eRes] = await Promise.all([
        fetchAllPages<WorkOrder>((p) => api.get("/maintenance/work-orders", { params: p }), params),
        fetchAllPages<Equipment>((p) => api.get("/maintenance/equipment", { params: p })),
      ]);
      setWos(wRes.rows);
      setEquipment(eRes.rows);
    } catch { setWos([]); } finally { setLoading(false); }
  }, [statusFilter]);

  useEffect(() => { load(); }, [load]);

  async function createWO() {
    if (!form.equipment_id || !form.description.trim()) return toast.error("Equipment and description required");
    setCreating(true);
    try {
      await api.post("/maintenance/work-orders", { equipment_id: form.equipment_id, work_type: form.work_type, description: form.description.trim(), scheduled_date: form.scheduled_date || null, notes: form.notes || null });
      toast.success("Work order created");
      setShowCreate(false);
      setForm(EMPTY_FORM);
      load();
    } catch { toast.error("Failed to create work order"); } finally { setCreating(false); }
  }

  async function updateStatus(id: string, status: string, extra?: object) {
    try {
      await api.patch(`/maintenance/work-orders/${id}/status`, { status, ...extra });
      toast.success(`Status updated to ${status}`);
      setCompletingId(null);
      load();
    } catch { toast.error("Failed to update status"); }
  }

  async function submitComplete() {
    if (!completingId) return;
    setCompleting(true);
    try {
      await api.patch(`/maintenance/work-orders/${completingId}/status`, {
        status: "completed",
        downtime_hours: completeForm.downtime_hours ? parseFloat(completeForm.downtime_hours) : null,
        cost: completeForm.cost ? parseFloat(completeForm.cost) : null,
        currency: completeForm.currency,
        notes: completeForm.notes || null,
      });
      toast.success("Work order completed");
      setCompletingId(null);
      load();
    } catch { toast.error("Failed"); } finally { setCompleting(false); }
  }

  async function deleteWO(id: string) {
    try { await api.delete(`/maintenance/work-orders/${id}`); toast.success("Deleted"); setConfirmDeleteId(null); load(); }
    catch { toast.error("Failed to delete"); }
  }

  const columns = useMemo<ColumnDef<WorkOrder>[]>(() => [
    { accessorKey: "number", header: "Work order", cell: (c) => <span className="font-mono text-[13px] font-medium">{c.getValue<string>()}</span> },
    {
      id: "equipment", accessorFn: (w) => w.equipment_name ?? "-", header: "Equipment",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.getValue<string>()}</p>
          <p className="line-clamp-1 max-w-md text-xs text-fg-muted">{c.row.original.description}</p>
        </div>
      ),
    },
    { accessorKey: "work_type", header: "Type", meta: { hideOnMobile: true }, cell: (c) => <Pill className="capitalize">{c.getValue<string>()}</Pill> },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="work_order" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} />,
    },
    {
      accessorKey: "scheduled_date", header: "Scheduled", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted tabular">{c.getValue<string | null>() ?? "-"}</span>,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const w = c.row.original;
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            {w.status === "planned" && <Button size="sm" onClick={() => updateStatus(w.id, "in_progress")}><Play aria-hidden /> Start</Button>}
            {w.status === "in_progress" && (
              <Button size="sm" variant="primary" onClick={() => { setCompletingId(w.id); setCompleteForm(EMPTY_COMPLETE); }}>
                <CheckCircle2 aria-hidden /> Complete
              </Button>
            )}
            {["planned", "in_progress"].includes(w.status) && (
              <Button size="sm" variant="ghost" onClick={() => updateStatus(w.id, "cancelled")}><XCircle aria-hidden /> Cancel</Button>
            )}
            {confirmDeleteId === w.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deleteWO(w.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </>
            ) : (
              <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${w.number}`} onClick={() => setConfirmDeleteId(w.id)} />
            )}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Maintenance work orders" summary="Corrective and preventive maintenance jobs."
          actions={<Button variant="primary" onClick={() => { setForm(EMPTY_FORM); setShowCreate(true); }}><Plus aria-hidden /> New work order</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} />
        <DataTable label="Maintenance work orders" columns={columns} data={wos} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Wrench, title: "No work orders", description: "Pick another view, or create a work order." }}
          renderDetails={(w) => (
            <div className="space-y-2">
              <p className="whitespace-pre-wrap text-sm text-fg">{w.description}</p>
              <KeyValues cols={3} items={[
                { label: "Scheduled", value: w.scheduled_date ?? "-" },
                { label: "Started", value: w.started_at?.slice(0, 10) ?? "-" },
                { label: "Completed", value: w.completed_at?.slice(0, 10) ?? "-" },
                { label: "Downtime", value: w.downtime_hours ? `${w.downtime_hours}h` : "-", numeric: true },
                { label: "Cost", value: w.cost ? `${w.currency} ${w.cost.toLocaleString()}` : "-", numeric: true },
              ]} />
              {w.notes && <p className="whitespace-pre-wrap text-sm text-fg-muted">{w.notes}</p>}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showCreate} onOpenChange={setShowCreate} title="New work order"
        footer={<>
          <Button onClick={() => setShowCreate(false)}>Cancel</Button>
          <Button variant="primary" onClick={createWO} loading={creating}>{creating ? "Creating" : "Create"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Equipment" required className="md:col-span-2">
            <Select value={form.equipment_id} onChange={e => setForm({ ...form, equipment_id: e.target.value })} placeholder="Select"
              options={equipment.map(e => ({ value: e.id, label: `${e.name} (${e.code})` }))} />
          </Field>
          <Field label="Type">
            <Select value={form.work_type} onChange={e => setForm({ ...form, work_type: e.target.value })}
              options={[{ value: "corrective", label: "Corrective" }, { value: "preventive", label: "Preventive" }]} />
          </Field>
          <Field label="Scheduled date">
            <Input type="date" value={form.scheduled_date} onChange={e => setForm({ ...form, scheduled_date: e.target.value })} />
          </Field>
          <Field label="Description" required className="md:col-span-2">
            <Textarea value={form.description} onChange={e => setForm({ ...form, description: e.target.value })} rows={2} />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={!!completingId} onOpenChange={(o) => { if (!o) setCompletingId(null); }} title="Complete work order"
        footer={<>
          <Button onClick={() => setCompletingId(null)}>Cancel</Button>
          <Button variant="primary" onClick={submitComplete} loading={completing}>{completing ? "Saving" : "Mark completed"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Downtime (hours)">
            <Input numeric type="number" min="0" step="0.5" value={completeForm.downtime_hours} onChange={e => setCompleteForm({ ...completeForm, downtime_hours: e.target.value })} placeholder="0" />
          </Field>
          <Field label="Total cost">
            <Input numeric type="number" min="0" value={completeForm.cost} onChange={e => setCompleteForm({ ...completeForm, cost: e.target.value })} placeholder="0" />
          </Field>
          <Field label="Completion notes" className="md:col-span-2">
            <Textarea value={completeForm.notes} onChange={e => setCompleteForm({ ...completeForm, notes: e.target.value })} rows={2} />
          </Field>
        </FieldGrid>
      </Dialog>
    </>
  );
}
