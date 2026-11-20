"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { ArrowRight, Plus, UserCheck, X } from "lucide-react";
import api from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, Input, ListFrame, PageHeader, SavedViewTabs, Select, StatusPill,
  toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Assignment {
  id: string; vehicle_id: string; employee_id: string | null; employee_name: string | null;
  start_date: string; end_date: string | null; purpose: string | null; is_active: boolean;
}
interface Vehicle { id: string; plate_number: string; make: string; model: string; }
interface Employee { id: string; first_name: string; last_name: string | null; position: string | null; }

const EMPTY_FORM = { vehicle_id: "", employee_id: "", start_date: new Date().toISOString().slice(0, 10), end_date: "", purpose: "", notes: "" };

type Filters = { activeOnly: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "active", label: "Active", filters: { activeOnly: true } },
  { id: "all", label: "All", filters: { activeOnly: false } },
];

export default function AssignmentsPage() {
  const views = useSavedViews<Filters>("fleet.assignments", VIEWS);
  const activeOnly = views.active?.filters.activeOnly ?? true;
  const [assignments, setAssignments] = useState<Assignment[]>([]);
  const [vehicles, setVehicles] = useState<Vehicle[]>([]);
  const [employees, setEmployees] = useState<Employee[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [aRes, vRes, eRes] = await Promise.all([
        fetchAllPages<Assignment>((p) => api.get("/fleet/assignments", { params: p }), activeOnly ? { active_only: "true" } : {}),
        api.get("/fleet/vehicles"),
        fetchAllPages<Employee>((p) => api.get("/hr/employees", { params: p })),
      ]);
      setAssignments(aRes.rows);
      setVehicles(vRes.data ?? []);
      setEmployees(eRes.rows);
    } catch { setAssignments([]); } finally { setLoading(false); }
  }, [activeOnly]);

  useEffect(() => { load(); }, [load]);

  const vehicleMap = useMemo(() => Object.fromEntries(vehicles.map(v => [v.id, v])) as Record<string, Vehicle>, [vehicles]);

  async function save() {
    if (!form.vehicle_id || !form.start_date) return toast.error("Vehicle and start date required");
    setSaving(true);
    try {
      await api.post("/fleet/assignments", {
        vehicle_id: form.vehicle_id,
        employee_id: form.employee_id || null,
        start_date: form.start_date,
        end_date: form.end_date || null,
        purpose: form.purpose || null,
        notes: form.notes || null,
      });
      toast.success("Assignment created");
      setShowModal(false);
      setForm(EMPTY_FORM);
      load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to create assignment");
    } finally { setSaving(false); }
  }

  async function endAssignment(id: string) {
    try {
      await api.delete(`/fleet/assignments/${id}`);
      toast.success("Assignment ended");
      load();
    } catch { toast.error("Failed to end assignment"); }
  }

  const columns = useMemo<ColumnDef<Assignment>[]>(() => [
    {
      id: "vehicle", accessorFn: (a) => vehicleMap[a.vehicle_id]?.plate_number ?? a.vehicle_id.slice(0, 8), header: "Vehicle",
      cell: (c) => <span className="font-mono text-[13px] font-medium">{c.getValue<string>()}</span>,
    },
    {
      id: "employee", accessorFn: (a) => a.employee_name ?? "Unassigned", header: "Assigned to",
      cell: (c) => <span className={c.row.original.employee_name ? "font-medium text-fg" : "text-fg-subtle"}>{c.getValue<string>()}</span>,
    },
    {
      accessorKey: "start_date", header: "Period", meta: { hideOnMobile: true },
      cell: (c) => (
        <span className="inline-flex items-center gap-1 text-fg-muted tabular">
          {c.row.original.start_date} <ArrowRight className="size-3" aria-label="to" /> {c.row.original.end_date ?? "ongoing"}
        </span>
      ),
    },
    {
      accessorKey: "purpose", header: "Purpose", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() ?? "-"}</span>,
    },
    {
      accessorKey: "is_active", header: "Status",
      cell: (c) => <StatusPill status={c.getValue<boolean>() ? "active" : "closed"} label={c.getValue<boolean>() ? "Active" : "Ended"} />,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => c.row.original.is_active
        ? <Button size="sm" onClick={() => endAssignment(c.row.original.id)}><X aria-hidden /> End</Button>
        : null,
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [vehicleMap]);

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Vehicle assignments" summary="Which vehicle is assigned to whom."
          actions={<Button variant="primary" onClick={() => { setForm(EMPTY_FORM); setShowModal(true); }}><Plus aria-hidden /> Assign vehicle</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? { activeOnly }} />
        <DataTable label="Vehicle assignments" columns={columns} data={assignments} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: UserCheck, title: "No assignments found", description: "Assign a vehicle to an employee, or pick another view." }} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Assign vehicle"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving}>{saving ? "Saving" : "Assign"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Vehicle" required className="md:col-span-2">
            <Select value={form.vehicle_id} onChange={e => setForm({ ...form, vehicle_id: e.target.value })} placeholder="Select vehicle"
              options={vehicles.filter(v => v.id).map(v => ({ value: v.id, label: `${v.plate_number}, ${v.make} ${v.model}` }))} />
          </Field>
          <Field label="Employee" className="md:col-span-2">
            <Select value={form.employee_id} onChange={e => setForm({ ...form, employee_id: e.target.value })} placeholder="No specific employee"
              options={employees.map(e => ({ value: e.id, label: `${e.first_name} ${e.last_name ?? ""}${e.position ? ` (${e.position})` : ""}` }))} />
          </Field>
          <Field label="Start date" required>
            <Input type="date" value={form.start_date} onChange={e => setForm({ ...form, start_date: e.target.value })} />
          </Field>
          <Field label="End date" hint="Leave blank for ongoing.">
            <Input type="date" value={form.end_date} onChange={e => setForm({ ...form, end_date: e.target.value })} />
          </Field>
          <Field label="Purpose" className="md:col-span-2">
            <Input value={form.purpose} onChange={e => setForm({ ...form, purpose: e.target.value })} placeholder="e.g. Sales operations, Executive transport" />
          </Field>
        </FieldGrid>
      </Dialog>
    </>
  );
}
