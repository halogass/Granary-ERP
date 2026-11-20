"use client";

import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Edit3, MapPin, Plus, Trash2 } from "lucide-react";
import { hrApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import {
  Button, Card, CardHeader, DataTable, Dialog, Field, FieldGrid, IconButton, Input, StatusPill, toast, type ColumnDef,
} from "@/components/ui";
import KiosksPanel from "./KiosksPanel";

export interface OfficeLocation {
  id: string;
  name: string;
  address?: string;
  latitude: number;
  longitude: number;
  radius_meters: number;
  is_active: boolean;
}

const KEY = ["settings", "office-locations"];

/** The office locations, shared by the offices and kiosks blocks. */
export function useOfficeLocations() {
  return useQuery({
    queryKey: KEY,
    queryFn: async () => {
      const res = await hrApi.listOfficeLocations();
      return (res.data.items ?? res.data ?? []) as OfficeLocation[];
    },
  });
}

const EMPTY = { name: "", address: "", latitude: "", longitude: "", radius_meters: "200" };

export default function OfficeLocationsBlock() {
  const qc = useQueryClient();
  const { data: offices = [], isLoading } = useOfficeLocations();
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<OfficeLocation | null>(null);
  const [form, setForm] = useState(EMPTY);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const reload = () => qc.invalidateQueries({ queryKey: KEY });

  const openNew = () => { setEditing(null); setForm(EMPTY); setOpen(true); };
  const openEdit = (loc: OfficeLocation) => {
    setEditing(loc);
    setForm({ name: loc.name, address: loc.address ?? "", latitude: String(loc.latitude), longitude: String(loc.longitude), radius_meters: String(loc.radius_meters) });
    setOpen(true);
  };

  async function save() {
    if (!form.name || !form.latitude || !form.longitude) return;
    setSaving(true);
    try {
      const payload = {
        name: form.name,
        address: form.address || undefined,
        latitude: parseFloat(form.latitude),
        longitude: parseFloat(form.longitude),
        radius_meters: parseInt(form.radius_meters) || 200,
      };
      if (editing) await hrApi.updateOfficeLocation(editing.id, payload);
      else await hrApi.createOfficeLocation(payload);
      toast.success(editing ? "Location updated" : "Location added");
      setOpen(false);
      setEditing(null);
      await reload();
    } catch (e) {
      toast.error(extractApiError(e));
    } finally {
      setSaving(false);
    }
  }

  async function remove(loc: OfficeLocation) {
    if (!window.confirm(`Delete the office location "${loc.name}"?`)) return;
    setDeleting(loc.id);
    try {
      await hrApi.deleteOfficeLocation(loc.id);
      toast.success("Location deleted");
      await reload();
    } catch (e) {
      toast.error(extractApiError(e));
    } finally {
      setDeleting(null);
    }
  }

  const columns = useMemo<ColumnDef<OfficeLocation>[]>(() => [
    { accessorKey: "name", header: "Name", cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
    { accessorKey: "address", header: "Address", meta: { hideOnMobile: true, className: "max-w-[220px] truncate text-fg-muted" }, cell: (c) => c.getValue<string>() || "-" },
    {
      id: "coords", header: "Coordinates", meta: { hideOnMobile: true },
      cell: (c) => <span className="font-mono text-xs">{Number(c.row.original.latitude).toFixed(5)}, {Number(c.row.original.longitude).toFixed(5)}</span>,
    },
    { accessorKey: "radius_meters", header: "Radius", meta: { numeric: true }, cell: (c) => `${c.getValue<number>()} m` },
    { accessorKey: "is_active", header: "Status", cell: (c) => <StatusPill status={c.getValue<boolean>() ? "active" : "inactive"} /> },
    {
      id: "actions", header: () => <span className="sr-only">Actions</span>, enableSorting: false, meta: { align: "right" },
      cell: (c) => (
        <div className="flex justify-end gap-1">
          <IconButton label={`Edit ${c.row.original.name}`} icon={Edit3} size="sm" onClick={() => openEdit(c.row.original)} />
          <IconButton label={`Delete ${c.row.original.name}`} icon={Trash2} size="sm" variant="ghost" className="text-tone-red"
            disabled={deleting === c.row.original.id} onClick={() => remove(c.row.original)} />
        </div>
      ),
    },
  // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [deleting]);

  return (
    <Card>
      <CardHeader title="Locations" description="Employees must be within the radius to clock in."
        actions={<Button size="sm" variant="primary" onClick={openNew}><Plus aria-hidden /> Add location</Button>} />
      <DataTable label="Office locations" columns={columns} data={offices} getRowId={(r) => r.id} loading={isLoading} maxHeight="none"
        empty={{ icon: MapPin, title: "No office locations", description: "Add at least one location to enable geofenced clock-in.", compact: true }} />
      <Dialog open={open} onOpenChange={setOpen} title={editing ? "Edit office location" : "New office location"}
        footer={<>
          <Button onClick={() => setOpen(false)}>Cancel</Button>
          <Button variant="primary" loading={saving} disabled={!form.name || !form.latitude || !form.longitude} onClick={save}>
            {editing ? "Update location" : "Add location"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Name" required className="md:col-span-2">
            <Input value={form.name} onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} placeholder="e.g. Head office" />
          </Field>
          <Field label="Address" className="md:col-span-2">
            <Input value={form.address} onChange={(e) => setForm((f) => ({ ...f, address: e.target.value }))} placeholder="1-1 Example Street, Tokyo" />
          </Field>
          <Field label="Latitude" required>
            <Input type="number" step="0.000001" value={form.latitude} onChange={(e) => setForm((f) => ({ ...f, latitude: e.target.value }))} placeholder="35.681236" className="font-mono" />
          </Field>
          <Field label="Longitude" required>
            <Input type="number" step="0.000001" value={form.longitude} onChange={(e) => setForm((f) => ({ ...f, longitude: e.target.value }))} placeholder="139.767125" className="font-mono" />
          </Field>
          <Field label="Geofence radius" hint="Employees must be within it to clock in; 100 to 300 m works well." className="md:col-span-2">
            <Input type="number" min="50" max="5000" numeric trailing="m" value={form.radius_meters} onChange={(e) => setForm((f) => ({ ...f, radius_meters: e.target.value }))} placeholder="200" />
          </Field>
        </FieldGrid>
      </Dialog>
    </Card>
  );
}

/** The kiosks block with the active offices it needs for its hint. */
export function KiosksBlock() {
  const { data: offices = [] } = useOfficeLocations();
  return <KiosksPanel offices={offices.filter((l) => l.is_active !== false)} />;
}
