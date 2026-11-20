"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, CalendarClock, CheckCircle2, Trash2 } from "lucide-react";
import api from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, Stat, StatGroup, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Plan {
  id: string; equipment_id: string; equipment_name: string | null;
  name: string; description: string | null;
  frequency_type: string; frequency_value: number;
  last_performed_at: string | null; next_due_at: string | null;
  next_due_days: number | null; overdue: boolean; due_soon: boolean; due_soon_days?: number; is_active: boolean;
}
interface Equipment { id: string; name: string; code: string; }

const EMPTY_FORM = { equipment_id: "", name: "", description: "", frequency_type: "months", frequency_value: "3", next_due_at: "" };

type Filters = { activeOnly: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "active", label: "Active plans", filters: { activeOnly: true } },
  { id: "all", label: "All plans", filters: { activeOnly: false } },
];

export default function PlansPage() {
  const views = useSavedViews<Filters>("maintenance.plans", VIEWS);
  const activeOnly = views.active?.filters.activeOnly ?? true;
  const [chip, setChip] = useState<"" | "overdue" | "due_soon">("");
  const [plans, setPlans] = useState<Plan[]>([]);
  const [equipment, setEquipment] = useState<Equipment[]>([]);
  const [loading, setLoading] = useState(true);
  const [showCreate, setShowCreate] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [creating, setCreating] = useState(false);
  const [markingId, setMarkingId] = useState<string | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [pRes, eRes] = await Promise.all([
        fetchAllPages<Plan>((p) => api.get("/maintenance/plans", { params: p }), { active_only: activeOnly ? "true" : "false" }),
        fetchAllPages<Equipment>((p) => api.get("/maintenance/equipment", { params: p })),
      ]);
      setPlans(pRes.rows);
      setEquipment(eRes.rows);
    } catch { setPlans([]); } finally { setLoading(false); }
  }, [activeOnly]);

  useEffect(() => { load(); }, [load]);

  async function createPlan() {
    if (!form.equipment_id || !form.name.trim() || !form.frequency_value) return toast.error("Equipment, name, and frequency required");
    setCreating(true);
    try {
      await api.post("/maintenance/plans", {
        equipment_id: form.equipment_id, name: form.name.trim(),
        description: form.description || null,
        frequency_type: form.frequency_type,
        frequency_value: parseInt(form.frequency_value),
        next_due_at: form.next_due_at ? new Date(form.next_due_at).toISOString() : null,
      });
      toast.success("Maintenance plan created");
      setShowCreate(false);
      setForm(EMPTY_FORM);
      load();
    } catch { toast.error("Failed to create plan"); } finally { setCreating(false); }
  }

  async function markCompleted(id: string) {
    setMarkingId(id);
    try {
      const res = await api.post(`/maintenance/plans/${id}/complete`);
      toast.success(`Marked done. Next due: ${res.data.next_due_at?.slice(0, 10) ?? "manual"}`);
      load();
    } catch { toast.error("Failed"); } finally { setMarkingId(null); }
  }

  async function deletePlan(id: string) {
    try { await api.delete(`/maintenance/plans/${id}`); toast.success("Deleted"); setConfirmDeleteId(null); load(); }
    catch { toast.error("Failed"); }
  }

  const overdue = plans.filter(p => p.overdue).length;
  const dueSoon = plans.filter(p => p.due_soon).length;
  const soonDays = plans.find(p => typeof p.due_soon_days === "number")?.due_soon_days;
  const dueSoonLabel = soonDays == null ? "Due soon" : `Due within ${soonDays} ${soonDays === 1 ? "day" : "days"}`;
  const rows = chip === "overdue" ? plans.filter(p => p.overdue) : chip === "due_soon" ? plans.filter(p => p.due_soon) : plans;

  const columns = useMemo<ColumnDef<Plan>[]>(() => [
    {
      accessorKey: "name", header: "Plan",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.row.original.name}</p>
          {c.row.original.description && <p className="line-clamp-1 text-xs text-fg-muted">{c.row.original.description}</p>}
        </div>
      ),
    },
    { id: "equipment", accessorFn: (p) => p.equipment_name ?? "-", header: "Equipment" },
    {
      id: "frequency", accessorFn: (p) => `Every ${p.frequency_value} ${p.frequency_type}`, header: "Frequency", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string>()}</span>,
    },
    {
      accessorKey: "last_performed_at", header: "Last done", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted tabular">{c.getValue<string | null>()?.slice(0, 10) ?? "-"}</span>,
    },
    {
      accessorKey: "next_due_at", header: "Next due",
      cell: (c) => {
        const p = c.row.original;
        if (!p.next_due_at) return <span className="text-xs text-fg-subtle">No due date</span>;
        if (p.overdue) return <Pill tone="red">Overdue by {Math.abs(p.next_due_days!)}d</Pill>;
        if (p.due_soon) return <Pill tone="orange">Due in {p.next_due_days}d</Pill>;
        return <span className="text-fg-muted tabular">{p.next_due_at.slice(0, 10)}</span>;
      },
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const p = c.row.original;
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <Button size="sm" onClick={() => markCompleted(p.id)} loading={markingId === p.id}><CheckCircle2 aria-hidden /> Done</Button>
            {confirmDeleteId === p.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deletePlan(p.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </>
            ) : (
              <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${p.name}`} onClick={() => setConfirmDeleteId(p.id)} />
            )}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, markingId]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Plans" value={plans.length} />
        <Stat label="Overdue" value={overdue} alert={overdue > 0} note={overdue > 0 ? "needs maintenance now" : undefined} />
        <Stat label={dueSoonLabel} value={dueSoon} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Maintenance schedules" summary="Preventive maintenance plans and recurring schedules."
          actions={<Button variant="primary" onClick={() => { setForm(EMPTY_FORM); setShowCreate(true); }}><Plus aria-hidden /> New plan</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? { activeOnly }} onSelect={() => setChip("")} />
        <FilterChips
          chips={[
            { id: "overdue", label: "Overdue", active: chip === "overdue", count: overdue },
            { id: "due_soon", label: "Due soon", active: chip === "due_soon", count: dueSoon },
          ]}
          onToggle={(id) => setChip((cur) => (cur === id ? "" : id as "overdue" | "due_soon"))} onClear={() => setChip("")} />
        <DataTable label="Maintenance plans" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: CalendarClock, title: "No maintenance plans yet", description: "Create a plan to schedule recurring maintenance." }} />
      </ListFrame>

      <Dialog open={showCreate} onOpenChange={setShowCreate} title="New maintenance plan"
        footer={<>
          <Button onClick={() => setShowCreate(false)}>Cancel</Button>
          <Button variant="primary" onClick={createPlan} loading={creating}>{creating ? "Creating" : "Create plan"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Equipment" required className="md:col-span-2">
            <Select value={form.equipment_id} onChange={e => setForm({ ...form, equipment_id: e.target.value })} placeholder="Select"
              options={equipment.map(e => ({ value: e.id, label: `${e.name} (${e.code})` }))} />
          </Field>
          <Field label="Plan name" required className="md:col-span-2">
            <Input value={form.name} onChange={e => setForm({ ...form, name: e.target.value })} placeholder="Quarterly Oil Change, Annual Inspection..." />
          </Field>
          <Field label="Every" required>
            <Input numeric type="number" min="1" value={form.frequency_value} onChange={e => setForm({ ...form, frequency_value: e.target.value })} placeholder="3" />
          </Field>
          <Field label="Frequency unit" required>
            <Select value={form.frequency_type} onChange={e => setForm({ ...form, frequency_type: e.target.value })}
              options={[{ value: "days", label: "Days" }, { value: "months", label: "Months" }, { value: "hours", label: "Hours (usage)" }]} />
          </Field>
          <Field label="First due date" className="md:col-span-2">
            <Input type="date" value={form.next_due_at} onChange={e => setForm({ ...form, next_due_at: e.target.value })} />
          </Field>
          <Field label="Description" className="md:col-span-2">
            <Textarea value={form.description} onChange={e => setForm({ ...form, description: e.target.value })} rows={2} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
