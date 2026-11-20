"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, Fuel, Trash2 } from "lucide-react";
import api from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, ListFrame, PageHeader, Select,
  Stat, StatGroup, toast, type ColumnDef,
} from "@/components/ui";

interface FuelLog {
  id: string; vehicle_id: string; log_date: string; odometer_reading: number | null;
  fuel_liters: number; fuel_cost: number; currency: string;
  fuel_station: string | null; cost_per_liter: number | null;
}
interface Vehicle { id: string; plate_number: string; make: string; model: string; }

const EMPTY_FORM = { vehicle_id: "", log_date: new Date().toISOString().slice(0, 10), odometer_reading: "", fuel_liters: "", fuel_cost: "", currency: "USD", fuel_station: "" };

export default function FuelLogsPage() {
  const [logs, setLogs] = useState<FuelLog[]>([]);
  const [vehicles, setVehicles] = useState<Vehicle[]>([]);
  const [loading, setLoading] = useState(true);
  const [vehicleFilter, setVehicleFilter] = useState("");
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (vehicleFilter) params.vehicle_id = vehicleFilter;
      const [lRes, vRes] = await Promise.all([
        fetchAllPages<FuelLog>((p) => api.get("/fleet/fuel-logs", { params: p }), params),
        api.get("/fleet/vehicles"),
      ]);
      setLogs(lRes.rows);
      setVehicles(vRes.data ?? []);
    } catch { setLogs([]); } finally { setLoading(false); }
  }, [vehicleFilter]);

  useEffect(() => { load(); }, [load]);

  const vehicleMap = useMemo(() => Object.fromEntries(vehicles.map(v => [v.id, v])) as Record<string, Vehicle>, [vehicles]);
  const vehicleOptions = vehicles.map(v => ({ value: v.id, label: `${v.plate_number}, ${v.make} ${v.model}` }));

  async function save() {
    if (!form.vehicle_id || !form.fuel_liters || !form.fuel_cost) return toast.error("Vehicle, liters and cost are required");
    setSaving(true);
    try {
      await api.post("/fleet/fuel-logs", {
        vehicle_id: form.vehicle_id,
        log_date: form.log_date,
        odometer_reading: form.odometer_reading ? parseFloat(form.odometer_reading) : null,
        fuel_liters: parseFloat(form.fuel_liters),
        fuel_cost: parseFloat(form.fuel_cost),
        currency: form.currency,
        fuel_station: form.fuel_station || null,
      });
      toast.success("Fuel log added");
      setShowModal(false);
      setForm(EMPTY_FORM);
      load();
    } catch { toast.error("Failed to add fuel log"); } finally { setSaving(false); }
  }

  async function deletLog(id: string) {
    try {
      await api.delete(`/fleet/fuel-logs/${id}`);
      toast.success("Deleted");
      setConfirmDeleteId(null);
      load();
    } catch { toast.error("Failed to delete"); }
  }

  const totalCost = logs.reduce((s, l) => s + l.fuel_cost, 0);
  const totalLiters = logs.reduce((s, l) => s + l.fuel_liters, 0);

  const columns = useMemo<ColumnDef<FuelLog>[]>(() => [
    {
      id: "vehicle", accessorFn: (l) => vehicleMap[l.vehicle_id]?.plate_number ?? "-", header: "Vehicle",
      cell: (c) => <span className="font-mono text-[13px] font-medium">{c.getValue<string>()}</span>,
    },
    { accessorKey: "log_date", header: "Date", cell: (c) => <span className="text-fg-muted tabular">{c.getValue<string>()}</span> },
    { accessorKey: "fuel_liters", header: "Liters", meta: { numeric: true }, cell: (c) => `${c.getValue<number>().toFixed(1)} L` },
    {
      accessorKey: "fuel_cost", header: "Cost", meta: { numeric: true },
      cell: (c) => <span className="font-medium">{c.row.original.currency} {c.getValue<number>().toLocaleString()}</span>,
    },
    {
      accessorKey: "cost_per_liter", header: "Per liter", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<number | null>()?.toLocaleString(undefined, { maximumFractionDigits: 0 }) ?? "-"}</span>,
    },
    {
      accessorKey: "fuel_station", header: "Station", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() ?? "-"}</span>,
    },
    {
      accessorKey: "odometer_reading", header: "Odometer", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<number | null>()?.toLocaleString() ?? "-"} km</span>,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const l = c.row.original;
        return confirmDeleteId === l.id ? (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <span className="text-xs font-semibold text-tone-red">Delete?</span>
            <Button size="sm" variant="danger" onClick={() => deletLog(l.id)}>Yes</Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
          </div>
        ) : (
          <IconButton size="sm" variant="ghost" icon={Trash2} label="Delete fuel log" onClick={() => setConfirmDeleteId(l.id)} />
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, vehicleMap]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total records" value={logs.length} />
        <Stat label="Total liters" value={`${totalLiters.toLocaleString(undefined, { maximumFractionDigits: 1 })} L`} />
        <Stat label="Total cost" value={`${(totalCost / 1000).toFixed(0)}K`} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Fuel logs" summary="Track fuel consumption and costs per vehicle."
          actions={<Button variant="primary" onClick={() => { setForm(EMPTY_FORM); setShowModal(true); }}><Plus aria-hidden /> Add fuel log</Button>} />
        <FilterChips chips={[]} onToggle={() => {}}>
          <Select value={vehicleFilter} onChange={e => setVehicleFilter(e.target.value)} aria-label="Filter by vehicle"
            placeholder="All vehicles" options={vehicleOptions} className="w-64" />
        </FilterChips>
        <DataTable label="Fuel logs" columns={columns} data={logs} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Fuel, title: "No fuel logs yet", description: "Add a fuel log after each refill." }} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Add fuel log"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving}>{saving ? "Saving" : "Add log"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Vehicle" required className="md:col-span-2">
            <Select value={form.vehicle_id} onChange={e => setForm({ ...form, vehicle_id: e.target.value })} placeholder="Select vehicle" options={vehicleOptions} />
          </Field>
          <Field label="Date" required>
            <Input type="date" value={form.log_date} onChange={e => setForm({ ...form, log_date: e.target.value })} />
          </Field>
          <Field label="Odometer (km)">
            <Input numeric type="number" min="0" value={form.odometer_reading} onChange={e => setForm({ ...form, odometer_reading: e.target.value })} placeholder="Optional" />
          </Field>
          <Field label="Liters" required>
            <Input numeric type="number" min="0.1" step="0.1" value={form.fuel_liters} onChange={e => setForm({ ...form, fuel_liters: e.target.value })} placeholder="45.5" />
          </Field>
          <Field label="Cost" required>
            <Input numeric type="number" min="0" value={form.fuel_cost} onChange={e => setForm({ ...form, fuel_cost: e.target.value })} placeholder="8000" />
          </Field>
          <Field label="Fuel station" className="md:col-span-2">
            <Input value={form.fuel_station} onChange={e => setForm({ ...form, fuel_station: e.target.value })} placeholder="Main Street station" />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
