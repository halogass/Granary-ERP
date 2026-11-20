"use client";

import { useState, useEffect, useCallback, useMemo } from "react";
import { localSource } from "@/lib/entitySources";
import { Plus, Warehouse, MapPin, ChevronRight, Trash2, Pencil, FolderOpen, Box, ArrowLeft } from "lucide-react";
import api from "@/lib/api";
import { cn } from "@/lib/utils";
import { useUrlParam } from "@/hooks/useUrlParam";
import {
  Button, Card, CardHeader, Checkbox, Combobox, Dialog, EmptyState, Field, FieldGrid, IconButton, Input,
  PageHeader, Pill, Select, Skeleton, toast, type Tone,
} from "@/components/ui";

interface WarehouseItem {
  id: string;
  name: string;
  code: string;
  address: string | null;
  is_default: boolean;
  is_active: boolean;
}

interface LocationNode {
  id: string;
  warehouse_id: string;
  parent_id: string | null;
  name: string;
  code: string;
  location_type: string;
  full_path: string;
  is_active: boolean;
  children: LocationNode[];
}

const TYPE_META: Record<string, { icon: React.ElementType; tone: Tone; label: string }> = {
  zone:  { icon: FolderOpen,   tone: "blue",    label: "Zone"  },
  aisle: { icon: ChevronRight, tone: "violet",  label: "Aisle" },
  rack:  { icon: Box,          tone: "orange",  label: "Rack"  },
  bin:   { icon: MapPin,       tone: "neutral", label: "Bin"   },
};

const EMPTY_WH_FORM = { name: "", code: "", address: "", is_default: false };
const EMPTY_LOC_FORM = { name: "", code: "", location_type: "zone", parent_id: "" };

export default function WarehousesPage() {
  const [warehouses, setWarehouses] = useState<WarehouseItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [activeWh, setActiveWh] = useState<WarehouseItem | null>(null);
  // Phones show the list, then a warehouse's locations full width; the chosen one is in the URL so
  // the phone Back button returns to the list (desktop still opens the first warehouse by itself).
  const [whParam, setWhParam] = useUrlParam("wh");
  const [locations, setLocations] = useState<LocationNode[]>([]);
  const [locLoading, setLocLoading] = useState(false);

  const [showWhModal, setShowWhModal] = useState(false);
  const [editingWh, setEditingWh] = useState<WarehouseItem | null>(null);
  const [whForm, setWhForm] = useState(EMPTY_WH_FORM);
  const [whSaving, setWhSaving] = useState(false);

  const [showLocModal, setShowLocModal] = useState(false);
  const [locForm, setLocForm] = useState(EMPTY_LOC_FORM);
  const [locSaving, setLocSaving] = useState(false);

  const [confirmDeleteWh, setConfirmDeleteWh] = useState<string | null>(null);
  const [flatLocations, setFlatLocations] = useState<LocationNode[]>([]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await api.get("/inventory/warehouses");
      setWarehouses(res.data ?? []);
      if (!activeWh && res.data?.length) {
        setActiveWh(res.data[0]);
      }
    } catch { setWarehouses([]); } finally { setLoading(false); }
  }, [activeWh]);

  useEffect(() => { load(); }, [load]);

  const locationOptions = useMemo(() => localSource(flatLocations.map((l) => ({
    id: l.id, label: l.full_path, sublabel: l.location_type,
  }))), [flatLocations]);

  const loadLocations = useCallback(async (whId: string) => {
    setLocLoading(true);
    try {
      const [treeRes, flatRes] = await Promise.all([
        api.get(`/inventory/warehouses/${whId}/locations`),
        api.get("/inventory/locations", { params: { warehouse_id: whId } }),
      ]);
      setLocations(treeRes.data ?? []);
      setFlatLocations(flatRes.data ?? []);
    } catch { setLocations([]); } finally { setLocLoading(false); }
  }, []);

  useEffect(() => {
    if (activeWh) loadLocations(activeWh.id);
  }, [activeWh, loadLocations]);

  useEffect(() => {
    if (!whParam || whParam === activeWh?.id) return;
    const wh = warehouses.find((w) => w.id === whParam);
    if (wh) setActiveWh(wh);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [whParam, warehouses]);

  function openCreateWh() {
    setEditingWh(null);
    setWhForm(EMPTY_WH_FORM);
    setShowWhModal(true);
  }

  function openEditWh(wh: WarehouseItem) {
    setEditingWh(wh);
    setWhForm({ name: wh.name, code: wh.code, address: wh.address ?? "", is_default: wh.is_default });
    setShowWhModal(true);
  }

  async function saveWarehouse() {
    if (!whForm.name.trim() || !whForm.code.trim()) return toast.error("Name and code are required");
    setWhSaving(true);
    try {
      const payload = { name: whForm.name.trim(), code: whForm.code.trim(), address: whForm.address || null, is_default: whForm.is_default };
      if (editingWh) {
        await api.patch(`/inventory/warehouses/${editingWh.id}`, payload);
        toast.success("Warehouse updated");
      } else {
        await api.post("/inventory/warehouses", payload);
        toast.success("Warehouse created");
      }
      setShowWhModal(false);
      load();
    } catch { toast.error("Failed to save warehouse"); } finally { setWhSaving(false); }
  }

  async function deleteWarehouse(id: string) {
    try {
      await api.delete(`/inventory/warehouses/${id}`);
      toast.success("Warehouse deactivated");
      setConfirmDeleteWh(null);
      if (activeWh?.id === id) { setActiveWh(null); setWhParam(null); }
      load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Cannot delete default warehouse");
    }
  }

  async function saveLocation() {
    if (!locForm.name.trim() || !locForm.code.trim() || !activeWh) return toast.error("Name and code are required");
    setLocSaving(true);
    try {
      await api.post("/inventory/locations", {
        warehouse_id: activeWh.id,
        parent_id: locForm.parent_id || null,
        name: locForm.name.trim(),
        code: locForm.code.trim(),
        location_type: locForm.location_type,
      });
      toast.success("Location created");
      setShowLocModal(false);
      setLocForm(EMPTY_LOC_FORM);
      loadLocations(activeWh.id);
    } catch { toast.error("Failed to create location"); } finally { setLocSaving(false); }
  }

  function renderLocationTree(nodes: LocationNode[], depth = 0): React.ReactNode {
    return nodes.map((node) => {
      const meta = TYPE_META[node.location_type] ?? TYPE_META.bin;
      const Icon = meta.icon;
      return (
        <li key={node.id}>
          <div className="flex items-center gap-2 rounded-lg px-3 py-2 hover:bg-surface-2" style={{ paddingLeft: `${12 + depth * 20}px` }}>
            <Icon className="size-3.5 shrink-0 text-fg-muted" aria-hidden />
            <span className="flex-1 text-sm font-medium text-fg">{node.name}</span>
            <span className="font-mono text-xs text-fg-muted">{node.code}</span>
            <Pill tone={meta.tone}>{meta.label}</Pill>
          </div>
          {node.children.length > 0 && <ul>{renderLocationTree(node.children, depth + 1)}</ul>}
        </li>
      );
    });
  }

  return (
    <div>
      <PageHeader title="Warehouses" summary="Storage locations and zones"
        actions={<Button variant="primary" onClick={openCreateWh}><Plus aria-hidden /> New warehouse</Button>} />

      <div className="flex flex-col gap-4 md:h-[calc(100dvh-11rem)] md:flex-row">
        {/* Warehouse list */}
        <Card className={cn("flex shrink-0 flex-col overflow-hidden md:w-[300px]", whParam && "max-md:hidden")}>
          <CardHeader title="Warehouses" />
          <div className="flex-1 overflow-y-auto">
            {loading ? (
              <div className="space-y-2 p-4">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-10 w-full" />)}</div>
            ) : warehouses.length === 0 ? (
              <EmptyState compact icon={Warehouse} title="No warehouses yet" description="Create a warehouse to add its locations." />
            ) : (
              <ul className="divide-y divide-line-soft">
                {warehouses.map((wh) => {
                  const on = activeWh?.id === wh.id;
                  return (
                    <li key={wh.id} className={cn("group flex items-center gap-2 pr-2", on ? "bg-brand/[0.07] shadow-[inset_2px_0_0_rgb(var(--brand))]" : "hover:bg-surface-2")}>
                      <button type="button" onClick={() => { setActiveWh(wh); setWhParam(wh.id); }} aria-current={on || undefined}
                        className="flex min-w-0 flex-1 items-center gap-3 px-4 py-3 text-left">
                        <Warehouse className="size-4 shrink-0 text-fg-muted" aria-hidden />
                        <span className="min-w-0 flex-1">
                          <span className="flex items-center gap-1.5">
                            <span className="truncate text-sm font-medium text-fg">{wh.name}</span>
                            {wh.is_default && <Pill tone="blue">Default</Pill>}
                          </span>
                          <span className="font-mono text-xs text-fg-muted">{wh.code}</span>
                        </span>
                      </button>
                      {confirmDeleteWh === wh.id ? (
                        <div className="flex shrink-0 items-center gap-1">
                          <Button size="sm" variant="danger" onClick={() => deleteWarehouse(wh.id)}>Yes</Button>
                          <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteWh(null)}>No</Button>
                        </div>
                      ) : (
                        <div className="flex shrink-0 items-center">
                          <IconButton size="sm" icon={Pencil} label={`Edit ${wh.name}`} onClick={() => openEditWh(wh)} />
                          <IconButton size="sm" icon={Trash2} label={`Delete ${wh.name}`} onClick={() => setConfirmDeleteWh(wh.id)} />
                        </div>
                      )}
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        </Card>

        {/* Location tree */}
        <Card className={cn("flex min-w-0 flex-1 flex-col overflow-hidden", !whParam && "max-md:hidden")}>
          <div className="border-b border-line px-2 py-1 md:hidden">
            <Button variant="ghost" onClick={() => setWhParam(null)}><ArrowLeft aria-hidden /> Warehouses</Button>
          </div>
          {!activeWh ? (
            <EmptyState icon={MapPin} title="Select a warehouse" description="Pick a warehouse to manage its locations." className="flex-1" />
          ) : (
            <>
              <CardHeader title={activeWh.name}
                description={`Location hierarchy, ${activeWh.code}${activeWh.address ? `, ${activeWh.address}` : ""}`}
                actions={
                  <Button variant="primary" size="sm"
                    onClick={() => { setLocForm({ ...EMPTY_LOC_FORM, location_type: locations.length === 0 ? "zone" : "bin" }); setShowLocModal(true); }}>
                    <Plus aria-hidden /> Add location
                  </Button>
                } />
              <div className="flex-1 overflow-y-auto p-3">
                {locLoading ? (
                  <div className="space-y-2 p-1">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-8 w-full" />)}</div>
                ) : locations.length === 0 ? (
                  <EmptyState icon={MapPin} title="No locations yet"
                    description='Start with a zone (e.g. "Zone A"), then add aisles, racks and bins inside it.' />
                ) : (
                  <ul>{renderLocationTree(locations)}</ul>
                )}
              </div>
            </>
          )}
        </Card>
      </div>

      <Dialog open={showWhModal} onOpenChange={setShowWhModal} title={editingWh ? "Edit warehouse" : "New warehouse"}
        footer={<>
          <Button onClick={() => setShowWhModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={saveWarehouse} loading={whSaving}>
            {whSaving ? "Saving..." : editingWh ? "Save changes" : "Create warehouse"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Name" required>
            <Input value={whForm.name} onChange={(e) => setWhForm({ ...whForm, name: e.target.value })} placeholder="Main Warehouse" />
          </Field>
          <Field label="Code" required>
            <Input value={whForm.code} onChange={(e) => setWhForm({ ...whForm, code: e.target.value.toUpperCase() })} className="font-mono" placeholder="WH01" />
          </Field>
          <Field label="Address" className="md:col-span-2">
            <Input value={whForm.address} onChange={(e) => setWhForm({ ...whForm, address: e.target.value })} placeholder="2-5 Harbor Road, Osaka" />
          </Field>
          <Checkbox className="md:col-span-2" label="Set as default warehouse" checked={whForm.is_default}
            onChange={(e) => setWhForm({ ...whForm, is_default: e.target.checked })} />
        </FieldGrid>
      </Dialog>

      <Dialog open={showLocModal} onOpenChange={setShowLocModal} title="Add location" description="Hierarchy: zone, aisle, rack, bin."
        footer={<>
          <Button onClick={() => setShowLocModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={saveLocation} loading={locSaving}>{locSaving ? "Saving..." : "Add location"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Name" required>
            <Input value={locForm.name} onChange={(e) => setLocForm({ ...locForm, name: e.target.value })} placeholder="Zone A" />
          </Field>
          <Field label="Code" required>
            <Input value={locForm.code} onChange={(e) => setLocForm({ ...locForm, code: e.target.value.toUpperCase() })} className="font-mono" placeholder="ZA" />
          </Field>
          <Field label="Type" className="md:col-span-2">
            <Select value={locForm.location_type} onChange={(e) => setLocForm({ ...locForm, location_type: e.target.value })} options={[
              { value: "zone", label: "Zone (top-level area)" },
              { value: "aisle", label: "Aisle" },
              { value: "rack", label: "Rack" },
              { value: "bin", label: "Bin (storage unit)" },
            ]} />
          </Field>
          <Field label="Parent location" hint="Optional. Leave empty for a top-level location." className="md:col-span-2">
            <Combobox value={locForm.parent_id} source={locationOptions}
              placeholder="Top level (no parent)" emptyLabel="No locations"
              onChange={(id) => setLocForm({ ...locForm, parent_id: id })} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
