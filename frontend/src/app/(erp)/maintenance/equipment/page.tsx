"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, Search, Server, Trash2, Pencil } from "lucide-react";
import api from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader,
  SavedViewTabs, Stat, StatGroup, StatusPill, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Equipment {
  id: string; name: string; code: string; equipment_type: string | null;
  brand: string | null; model: string | null; serial_number: string | null;
  location: string | null; status: string; asset_id: string | null; notes: string | null;
}

const STATUS_LABELS: Record<string, string> = {
  active: "Active", under_maintenance: "In maintenance", decommissioned: "Decommissioned",
};

const EMPTY_FORM = { name: "", code: "", equipment_type: "", brand: "", model: "", serial_number: "", location: "", notes: "", asset_id: "" };

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "under_maintenance", label: "In maintenance", filters: { status: "under_maintenance" } },
  { id: "decommissioned", label: "Decommissioned", filters: { status: "decommissioned" } },
];

export default function EquipmentPage() {
  const views = useSavedViews<Filters>("maintenance.equipment", VIEWS);
  const statusFilter = views.active?.filters.status ?? "";
  const [items, setItems] = useState<Equipment[]>([]);
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
      if (search) params.q = search;
      if (statusFilter) params.status = statusFilter;
      const { rows } = await fetchAllPages<Equipment>((p) => api.get("/maintenance/equipment", { params: p }), params);
      setItems(rows);
    } catch { setItems([]); } finally { setLoading(false); }
  }, [search, statusFilter]);

  useEffect(() => { load(); }, [load]);

  function openCreate() { setEditingId(null); setForm(EMPTY_FORM); setShowModal(true); }
  function openEdit(e: Equipment) {
    setEditingId(e.id);
    setForm({ name: e.name, code: e.code, equipment_type: e.equipment_type ?? "", brand: e.brand ?? "", model: e.model ?? "", serial_number: e.serial_number ?? "", location: e.location ?? "", notes: e.notes ?? "", asset_id: e.asset_id ?? "" });
    setShowModal(true);
  }

  async function save() {
    if (!form.name.trim() || !form.code.trim()) return toast.error("Name and code required");
    setSaving(true);
    try {
      const payload = { name: form.name.trim(), code: form.code.trim().toUpperCase(), equipment_type: form.equipment_type || null, brand: form.brand || null, model: form.model || null, serial_number: form.serial_number || null, location: form.location || null, notes: form.notes || null, asset_id: form.asset_id || null };
      if (editingId) { await api.patch(`/maintenance/equipment/${editingId}`, payload); toast.success("Updated"); }
      else { await api.post("/maintenance/equipment", payload); toast.success("Equipment added"); }
      setShowModal(false); load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to save");
    } finally { setSaving(false); }
  }

  async function deleteEq(id: string) {
    try { await api.delete(`/maintenance/equipment/${id}`); toast.success("Deleted"); setConfirmDeleteId(null); load(); }
    catch { toast.error("Failed to delete"); }
  }

  const counts = { total: items.length, active: items.filter(i => i.status === "active").length, maintenance: items.filter(i => i.status === "under_maintenance").length };

  const columns = useMemo<ColumnDef<Equipment>[]>(() => [
    {
      accessorKey: "name", header: "Equipment",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.row.original.name}</p>
          <p className="font-mono text-xs text-fg-muted">{c.row.original.code}</p>
        </div>
      ),
    },
    {
      accessorKey: "equipment_type", header: "Type", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() ?? "-"}</span>,
    },
    {
      id: "make", accessorFn: (e) => [e.brand, e.model].filter(Boolean).join(" "), header: "Brand and model", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string>() || "-"}</span>,
    },
    {
      accessorKey: "location", header: "Location", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() ?? "-"}</span>,
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="equipment" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} />,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const eq = c.row.original;
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <IconButton size="sm" variant="ghost" icon={Pencil} label={`Edit ${eq.name}`} onClick={() => openEdit(eq)} />
            {confirmDeleteId === eq.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deleteEq(eq.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </>
            ) : (
              <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${eq.name}`} onClick={() => setConfirmDeleteId(eq.id)} />
            )}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={counts.total} />
        <Stat label="Active" value={counts.active} />
        <Stat label="In maintenance" value={counts.maintenance} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Equipment registry" summary="Tracked equipment subject to maintenance."
          actions={<Button variant="primary" onClick={openCreate}><Plus aria-hidden /> Add equipment</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} />
        <FilterChips chips={[]} onToggle={() => {}}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={e => setSearch(e.target.value)} placeholder="Search name, code, serial" aria-label="Search equipment" className="w-64" />
        </FilterChips>
        <DataTable label="Equipment" columns={columns} data={items} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Server, title: "No equipment yet", description: "Add equipment to schedule its maintenance." }}
          renderDetails={(eq) => (
            <div className="space-y-2">
              <KeyValues cols={3} items={[
                { label: "Serial number", value: eq.serial_number ? <span className="font-mono">{eq.serial_number}</span> : "-" },
                { label: "Brand", value: eq.brand ?? "-" },
                { label: "Model", value: eq.model ?? "-" },
                { label: "Type", value: eq.equipment_type ?? "-" },
                { label: "Location", value: eq.location ?? "-" },
              ]} />
              {eq.notes && <p className="whitespace-pre-wrap text-sm text-fg-muted">{eq.notes}</p>}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title={editingId ? "Edit equipment" : "Add equipment"}
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving}>{saving ? "Saving" : editingId ? "Save" : "Add"}</Button>
        </>}>
        <div className="space-y-4">
          <FieldGrid cols={2}>
            <Field label="Name" required>
              <Input value={form.name} onChange={e => setForm({ ...form, name: e.target.value })} placeholder="Air Compressor #1" />
            </Field>
            <Field label="Code" required>
              <Input value={form.code} onChange={e => setForm({ ...form, code: e.target.value.toUpperCase() })} className="font-mono" placeholder="EQ-001" />
            </Field>
          </FieldGrid>
          <FieldGrid cols={3}>
            <Field label="Type">
              <Input value={form.equipment_type} onChange={e => setForm({ ...form, equipment_type: e.target.value })} placeholder="Server" />
            </Field>
            <Field label="Brand">
              <Input value={form.brand} onChange={e => setForm({ ...form, brand: e.target.value })} placeholder="Dell" />
            </Field>
            <Field label="Model">
              <Input value={form.model} onChange={e => setForm({ ...form, model: e.target.value })} placeholder="R750" />
            </Field>
          </FieldGrid>
          <FieldGrid cols={2}>
            <Field label="Serial number">
              <Input value={form.serial_number} onChange={e => setForm({ ...form, serial_number: e.target.value })} className="font-mono" />
            </Field>
            <Field label="Location">
              <Input value={form.location} onChange={e => setForm({ ...form, location: e.target.value })} placeholder="Server Room, Osaka" />
            </Field>
            <Field label="Notes" className="md:col-span-2">
              <Textarea value={form.notes} onChange={e => setForm({ ...form, notes: e.target.value })} rows={2} />
            </Field>
          </FieldGrid>
        </div>
      </Dialog>
    </div>
  );
}
