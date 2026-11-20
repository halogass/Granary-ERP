"use client";

import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, Car, AlertTriangle, Wrench, Trash2, Pencil, Bike, Truck, Bus, CheckCircle2, Search } from "lucide-react";
import api from "@/lib/api";
import { formatCurrency } from "@/lib/currency";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader,
  SavedViewTabs, Select, Stat, StatGroup, StatusPill, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Vehicle {
  id: string; plate_number: string; make: string; model: string; year: number; color: string | null;
  vehicle_type: string; fuel_type: string; status: string;
  purchase_price: number | null; currency: string;
  insurance_expiry: string | null; insurance_expiry_alert: boolean;
  registration_expiry: string | null; registration_expiry_alert: boolean;
  odometer_current: number;
  current_assignment: { employee_name: string | null; purpose: string | null } | null;
}

interface Summary {
  total_vehicles: number; active: number; maintenance: number; expiring_soon: number;
  fuel_cost_this_month: number; fuel_currency: string;
  fuel_cost_this_month_by_currency: Record<string, number>;
}

// One figure per currency: fuel bought in different currencies is never added together.
function fuelByCurrency(s: Summary): string {
  const entries = Object.entries(s.fuel_cost_this_month_by_currency ?? {}).filter(([, v]) => v);
  if (!entries.length) return formatCurrency(0, s.fuel_currency || "USD");
  return entries.map(([cur, v]) => formatCurrency(v, cur)).join(" · ");
}

const TYPE_ICONS: Record<string, React.ElementType> = {
  car: Car, motorcycle: Bike, truck: Truck, van: Bus, other: Car,
};

const EMPTY_FORM = {
  plate_number: "", make: "", model: "", year: new Date().getFullYear().toString(),
  color: "", vehicle_type: "car", fuel_type: "petrol", currency: "USD",
  purchase_price: "", insurance_expiry: "", registration_expiry: "", odometer_current: "0", notes: "",
};

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "maintenance", label: "Maintenance", filters: { status: "maintenance" } },
  { id: "inactive", label: "Inactive", filters: { status: "inactive" } },
];

const opts = (xs: string[]) => xs.map((x) => ({ value: x, label: x }));

export default function FleetVehiclesPage() {
  const views = useSavedViews<Filters>("fleet.vehicles", VIEWS);
  const statusFilter = views.active?.filters.status ?? "";
  const [chips, setChips] = useState<Record<string, boolean>>({});
  const [vehicles, setVehicles] = useState<Vehicle[]>([]);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");

  const [showModal, setShowModal] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (statusFilter) params.status = statusFilter;
      if (search) params.q = search;
      const [vRes, sRes] = await Promise.all([
        api.get("/fleet/vehicles", { params }),
        api.get("/fleet/summary"),
      ]);
      setVehicles(vRes.data ?? []);
      setSummary(sRes.data);
    } catch { setVehicles([]); } finally { setLoading(false); }
  }, [statusFilter, search]);

  useEffect(() => { load(); }, [load]);

  function openCreate() {
    setEditingId(null);
    setForm(EMPTY_FORM);
    setShowModal(true);
  }

  function openEdit(v: Vehicle) {
    setEditingId(v.id);
    setForm({
      plate_number: v.plate_number, make: v.make, model: v.model,
      year: v.year.toString(), color: v.color ?? "",
      vehicle_type: v.vehicle_type, fuel_type: v.fuel_type,
      currency: v.currency, purchase_price: v.purchase_price?.toString() ?? "",
      insurance_expiry: v.insurance_expiry ?? "", registration_expiry: v.registration_expiry ?? "",
      odometer_current: v.odometer_current.toString(), notes: "",
    });
    setShowModal(true);
  }

  async function save() {
    if (!form.plate_number.trim() || !form.make.trim() || !form.model.trim()) {
      return toast.error("Plate number, make and model are required");
    }
    setSaving(true);
    try {
      const payload = {
        plate_number: form.plate_number.trim().toUpperCase(),
        make: form.make.trim(), model: form.model.trim(),
        year: parseInt(form.year), color: form.color || null,
        vehicle_type: form.vehicle_type, fuel_type: form.fuel_type,
        currency: form.currency,
        purchase_price: form.purchase_price ? parseFloat(form.purchase_price) : null,
        insurance_expiry: form.insurance_expiry || null,
        registration_expiry: form.registration_expiry || null,
        odometer_current: parseFloat(form.odometer_current) || 0,
      };
      if (editingId) {
        await api.patch(`/fleet/vehicles/${editingId}`, payload);
        toast.success("Vehicle updated");
      } else {
        await api.post("/fleet/vehicles", payload);
        toast.success("Vehicle added");
      }
      setShowModal(false);
      load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to save vehicle");
    } finally { setSaving(false); }
  }

  async function deleteVehicle(id: string) {
    try {
      await api.delete(`/fleet/vehicles/${id}`);
      toast.success("Vehicle removed");
      setConfirmDeleteId(null);
      load();
    } catch { toast.error("Failed to delete vehicle"); }
  }

  async function setStatus(v: Vehicle, status: string) {
    await api.patch(`/fleet/vehicles/${v.id}`, { status });
    load();
  }

  const rows = chips.expiring ? vehicles.filter((v) => v.insurance_expiry_alert || v.registration_expiry_alert) : vehicles;

  const columns = useMemo<ColumnDef<Vehicle>[]>(() => [
    {
      accessorKey: "plate_number", header: "Vehicle",
      cell: (c) => {
        const v = c.row.original;
        const Icon = TYPE_ICONS[v.vehicle_type] ?? Car;
        return (
          <div className="flex min-w-0 items-center gap-3">
            <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-surface-2">
              <Icon className="size-4 text-fg-muted" aria-label={v.vehicle_type} />
            </span>
            <div className="min-w-0">
              <p className="font-mono text-[13px] font-medium text-fg">{v.plate_number}</p>
              <p className="truncate text-xs text-fg-muted">{v.year} {v.make} {v.model}{v.color ? ` · ${v.color}` : ""}</p>
            </div>
          </div>
        );
      },
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="vehicle" status={c.getValue<string>()} /> },
    {
      id: "assigned", accessorFn: (v) => v.current_assignment?.employee_name ?? "", header: "Assigned to", meta: { hideOnMobile: true },
      cell: (c) => c.row.original.current_assignment
        ? <span className="text-fg">{c.row.original.current_assignment.employee_name ?? "-"}</span>
        : <span className="text-fg-subtle">Unassigned</span>,
    },
    {
      accessorKey: "odometer_current", header: "Odometer", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => `${c.getValue<number>().toLocaleString()} km`,
    },
    {
      id: "expiry", header: "Expiry", enableSorting: false, meta: { hideOnMobile: true },
      cell: (c) => {
        const v = c.row.original;
        const alert = v.insurance_expiry_alert || v.registration_expiry_alert;
        if (!v.insurance_expiry && !v.registration_expiry) return <span className="text-fg-subtle">-</span>;
        return alert
          ? <span className="inline-flex items-center gap-1 text-xs font-medium text-tone-orange"><AlertTriangle className="size-3" aria-hidden /> Expiring soon</span>
          : <span className="text-xs text-fg-muted tabular">{v.insurance_expiry ?? v.registration_expiry}</span>;
      },
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const v = c.row.original;
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <IconButton size="sm" variant="ghost" icon={Pencil} label={`Edit ${v.plate_number}`} onClick={() => openEdit(v)} />
            {confirmDeleteId === v.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deleteVehicle(v.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </>
            ) : (
              <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${v.plate_number}`} onClick={() => setConfirmDeleteId(v.id)} />
            )}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  return (
    <div className="space-y-4">
      {summary && (
        <StatGroup>
          <Stat label="Total fleet" value={summary.total_vehicles} />
          <Stat label="Active" value={summary.active} />
          <Stat label="In maintenance" value={summary.maintenance} />
          <Stat label="Expiring soon" value={summary.expiring_soon} alert={summary.expiring_soon > 0}
            note={summary.expiring_soon > 0 ? "insurance or registration" : undefined} />
          <Stat label="Fuel this month" value={fuelByCurrency(summary)} />
        </StatGroup>
      )}

      <ListFrame>
        <PageHeader flush title="Fleet vehicles" summary="Company vehicle registry and status."
          actions={<Button variant="primary" onClick={openCreate}><Plus aria-hidden /> Add vehicle</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} onSelect={() => setChips({})} />
        <FilterChips chips={[{ id: "expiring", label: "Expiring soon", active: !!chips.expiring }]}
          onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id] }))} onClear={() => setChips({})}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={e => setSearch(e.target.value)} placeholder="Search plate, make, model" aria-label="Search vehicles" className="w-64" />
        </FilterChips>
        <DataTable label="Vehicles" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Car, title: "No vehicles found", description: "Pick another view, or add a vehicle." }}
          renderDetails={(v) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Type", value: <span className="capitalize">{v.vehicle_type}</span> },
                { label: "Fuel", value: <span className="capitalize">{v.fuel_type}</span> },
                { label: "Odometer", value: `${v.odometer_current.toLocaleString()} km`, numeric: true },
                { label: "Assigned to", value: v.current_assignment?.employee_name ?? "Unassigned" },
                { label: "Purpose", value: v.current_assignment?.purpose ?? "-" },
                { label: "Purchase price", value: v.purchase_price != null ? formatCurrency(v.purchase_price, v.currency) : "-", numeric: true },
                {
                  label: "Insurance expiry",
                  value: <span className={v.insurance_expiry_alert ? "font-medium text-tone-orange" : undefined}>{v.insurance_expiry ?? "-"}</span>,
                },
                {
                  label: "Registration expiry",
                  value: <span className={v.registration_expiry_alert ? "font-medium text-tone-orange" : undefined}>{v.registration_expiry ?? "-"}</span>,
                },
              ]} />
              <div className="flex flex-wrap gap-2">
                <Button size="sm" onClick={() => openEdit(v)}><Pencil aria-hidden /> Edit</Button>
                {v.status !== "maintenance" ? (
                  <Button size="sm" onClick={() => setStatus(v, "maintenance")}><Wrench aria-hidden /> Maintenance</Button>
                ) : (
                  <Button size="sm" onClick={() => setStatus(v, "active")}><CheckCircle2 aria-hidden /> Mark active</Button>
                )}
              </div>
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title={editingId ? "Edit vehicle" : "Add vehicle"} size="lg"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving}>{saving ? "Saving" : editingId ? "Save changes" : "Add vehicle"}</Button>
        </>}>
        <FieldGrid cols={3}>
          <Field label="Plate number" required>
            <Input value={form.plate_number} onChange={e => setForm({ ...form, plate_number: e.target.value.toUpperCase() })} className="font-mono" placeholder="ABC-1234" />
          </Field>
          <Field label="Make" required>
            <Input value={form.make} onChange={e => setForm({ ...form, make: e.target.value })} placeholder="Toyota" />
          </Field>
          <Field label="Model" required>
            <Input value={form.model} onChange={e => setForm({ ...form, model: e.target.value })} placeholder="Corolla" />
          </Field>
          <Field label="Year">
            <Input numeric type="number" min="1990" max="2030" value={form.year} onChange={e => setForm({ ...form, year: e.target.value })} />
          </Field>
          <Field label="Color">
            <Input value={form.color} onChange={e => setForm({ ...form, color: e.target.value })} placeholder="Silver" />
          </Field>
          <Field label="Odometer (km)">
            <Input numeric type="number" min="0" value={form.odometer_current} onChange={e => setForm({ ...form, odometer_current: e.target.value })} />
          </Field>
        </FieldGrid>
        <FieldGrid cols={2} className="mt-4">
          <Field label="Vehicle type">
            <Select value={form.vehicle_type} onChange={e => setForm({ ...form, vehicle_type: e.target.value })} options={opts(["car", "motorcycle", "truck", "van", "other"])} />
          </Field>
          <Field label="Fuel type">
            <Select value={form.fuel_type} onChange={e => setForm({ ...form, fuel_type: e.target.value })} options={opts(["petrol", "diesel", "electric", "hybrid"])} />
          </Field>
          <Field label="Insurance expiry">
            <Input type="date" value={form.insurance_expiry} onChange={e => setForm({ ...form, insurance_expiry: e.target.value })} />
          </Field>
          <Field label="Registration expiry">
            <Input type="date" value={form.registration_expiry} onChange={e => setForm({ ...form, registration_expiry: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
