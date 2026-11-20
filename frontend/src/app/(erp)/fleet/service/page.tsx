"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, Wrench, AlertTriangle, Trash2 } from "lucide-react";
import api from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader, Pill,
  Select, Textarea, toast, type ColumnDef,
} from "@/components/ui";

interface ServiceRecord {
  id: string; vehicle_id: string; plate_number: string | null; service_date: string;
  service_type: string; description: string | null; cost: number | null; currency: string;
  vendor_name: string | null; odometer_reading: number | null;
  next_service_date: string | null; next_service_days: number | null; next_service_alert: boolean;
}
interface Vehicle { id: string; plate_number: string; make: string; model: string; }

const SERVICE_LABELS: Record<string, string> = {
  oil_change: "Oil Change", tire: "Tire", brake: "Brake",
  inspection: "Inspection", repair: "Repair", other: "Other",
};

const EMPTY_FORM = {
  vehicle_id: "", service_date: new Date().toISOString().slice(0, 10),
  service_type: "oil_change", description: "", cost: "", currency: "USD",
  vendor_name: "", odometer_reading: "", next_service_date: "", next_service_odometer: "",
};

export default function ServicePage() {
  const [records, setRecords] = useState<ServiceRecord[]>([]);
  const [vehicles, setVehicles] = useState<Vehicle[]>([]);
  const [loading, setLoading] = useState(true);
  const [vehicleFilter, setVehicleFilter] = useState("");
  const [dueOnly, setDueOnly] = useState(false);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (vehicleFilter) params.vehicle_id = vehicleFilter;
      const [sRes, vRes] = await Promise.all([
        fetchAllPages<ServiceRecord>((p) => api.get("/fleet/service-records", { params: p }), params),
        api.get("/fleet/vehicles"),
      ]);
      setRecords(sRes.rows);
      setVehicles(vRes.data ?? []);
    } catch { setRecords([]); } finally { setLoading(false); }
  }, [vehicleFilter]);

  useEffect(() => { load(); }, [load]);

  async function save() {
    if (!form.vehicle_id || !form.service_date) return toast.error("Vehicle and date required");
    setSaving(true);
    try {
      await api.post("/fleet/service-records", {
        vehicle_id: form.vehicle_id,
        service_date: form.service_date,
        service_type: form.service_type,
        description: form.description || null,
        cost: form.cost ? parseFloat(form.cost) : null,
        currency: form.currency,
        vendor_name: form.vendor_name || null,
        odometer_reading: form.odometer_reading ? parseFloat(form.odometer_reading) : null,
        next_service_date: form.next_service_date || null,
        next_service_odometer: form.next_service_odometer ? parseFloat(form.next_service_odometer) : null,
      });
      toast.success("Service record added");
      setShowModal(false);
      setForm(EMPTY_FORM);
      load();
    } catch { toast.error("Failed to save"); } finally { setSaving(false); }
  }

  async function deleteRecord(id: string) {
    try {
      await api.delete(`/fleet/service-records/${id}`);
      toast.success("Deleted");
      setConfirmDeleteId(null);
      load();
    } catch { toast.error("Failed to delete"); }
  }

  const upcoming = records.filter(r => r.next_service_alert).length;
  const rows = dueOnly ? records.filter(r => r.next_service_alert) : records;
  const vehicleOptions = vehicles.map(v => ({ value: v.id, label: `${v.plate_number}, ${v.make} ${v.model}` }));

  const columns = useMemo<ColumnDef<ServiceRecord>[]>(() => [
    {
      accessorKey: "plate_number", header: "Vehicle",
      cell: (c) => <span className="font-mono text-[13px] font-medium">{c.getValue<string | null>() ?? "-"}</span>,
    },
    {
      accessorKey: "service_type", header: "Service",
      cell: (c) => <Pill>{SERVICE_LABELS[c.getValue<string>()] ?? c.getValue<string>()}</Pill>,
    },
    { accessorKey: "service_date", header: "Date", cell: (c) => <span className="text-fg-muted tabular">{c.getValue<string>()}</span> },
    {
      accessorKey: "vendor_name", header: "Workshop", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() ?? "-"}</span>,
    },
    {
      accessorKey: "cost", header: "Cost", meta: { numeric: true },
      cell: (c) => c.row.original.cost ? <span className="font-medium">{c.row.original.currency} {c.row.original.cost.toLocaleString()}</span> : <span className="text-fg-subtle">-</span>,
    },
    {
      accessorKey: "next_service_date", header: "Next service", meta: { hideOnMobile: true },
      cell: (c) => {
        const r = c.row.original;
        if (!r.next_service_date) return <span className="text-fg-subtle">-</span>;
        return r.next_service_alert
          ? <span className="inline-flex items-center gap-1 font-medium text-tone-orange tabular"><AlertTriangle className="size-3" aria-label="Due soon" />{r.next_service_date}</span>
          : <span className="text-fg-muted tabular">{r.next_service_date}</span>;
      },
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const r = c.row.original;
        return confirmDeleteId === r.id ? (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <span className="text-xs font-semibold text-tone-red">Delete?</span>
            <Button size="sm" variant="danger" onClick={() => deleteRecord(r.id)}>Yes</Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
          </div>
        ) : (
          <IconButton size="sm" variant="ghost" icon={Trash2} label="Delete service record" onClick={() => setConfirmDeleteId(r.id)} />
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  return (
    <div className="space-y-4">
      {upcoming > 0 && (
        <div className="flex items-center gap-3 rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-4 py-3" role="status">
          <AlertTriangle className="size-4 shrink-0 text-tone-orange" aria-hidden />
          <p className="text-sm font-medium text-tone-orange">{upcoming} vehicle{upcoming > 1 ? "s" : ""} due for service within 30 days</p>
        </div>
      )}

      <ListFrame>
        <PageHeader flush title="Service records" summary="Maintenance history and upcoming service alerts."
          actions={<Button variant="primary" onClick={() => { setForm(EMPTY_FORM); setShowModal(true); }}><Plus aria-hidden /> Add service record</Button>} />
        <FilterChips chips={[{ id: "due", label: "Due soon", active: dueOnly, count: upcoming }]}
          onToggle={() => setDueOnly((d) => !d)} onClear={() => setDueOnly(false)}>
          <Select value={vehicleFilter} onChange={e => setVehicleFilter(e.target.value)} aria-label="Filter by vehicle"
            placeholder="All vehicles" options={vehicleOptions} className="w-64" />
        </FilterChips>
        <DataTable label="Service records" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Wrench, title: "No service records yet", description: "Add a record after each service." }}
          renderDetails={(r) => (
            <div className="space-y-2">
              <KeyValues cols={3} items={[
                { label: "Odometer", value: r.odometer_reading ? `${r.odometer_reading.toLocaleString()} km` : "-", numeric: true },
                { label: "Workshop", value: r.vendor_name ?? "-" },
                { label: "Next service", value: r.next_service_date ?? "-" },
              ]} />
              {r.description && <p className="whitespace-pre-wrap text-sm text-fg-muted">{r.description}</p>}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Add service record"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving}>{saving ? "Saving" : "Add record"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Vehicle" required className="md:col-span-2">
            <Select value={form.vehicle_id} onChange={e => setForm({ ...form, vehicle_id: e.target.value })} placeholder="Select vehicle" options={vehicleOptions} />
          </Field>
          <Field label="Service date" required>
            <Input type="date" value={form.service_date} onChange={e => setForm({ ...form, service_date: e.target.value })} />
          </Field>
          <Field label="Service type">
            <Select value={form.service_type} onChange={e => setForm({ ...form, service_type: e.target.value })}
              options={Object.entries(SERVICE_LABELS).map(([value, label]) => ({ value, label }))} />
          </Field>
          <Field label="Description" className="md:col-span-2">
            <Textarea value={form.description} onChange={e => setForm({ ...form, description: e.target.value })} rows={2} />
          </Field>
          <Field label="Cost">
            <Input numeric type="number" min="0" value={form.cost} onChange={e => setForm({ ...form, cost: e.target.value })} placeholder="0" />
          </Field>
          <Field label="Workshop">
            <Input value={form.vendor_name} onChange={e => setForm({ ...form, vendor_name: e.target.value })} placeholder="Auto service name" />
          </Field>
          <Field label="Odometer (km)">
            <Input numeric type="number" min="0" value={form.odometer_reading} onChange={e => setForm({ ...form, odometer_reading: e.target.value })} />
          </Field>
          <Field label="Next service date">
            <Input type="date" value={form.next_service_date} onChange={e => setForm({ ...form, next_service_date: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
