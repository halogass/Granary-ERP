"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import { manufacturingApi } from "@/lib/api";
import { Boxes, Plus, AlertTriangle, ArrowUpDown, Search } from "lucide-react";
import { extractApiError } from "@/lib/apiError";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, Input, KeyValues, ListFrame, PageHeader,
  SavedViewTabs, Select, Stat, StatGroup, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface InventoryItem {
  id: string;
  component_name: string;
  sku: string;
  category?: string;
  quantity_on_hand: number;
  quantity_reserved: number;
  reorder_point?: number;
  unit: string;
  unit_cost?: number;
  currency: string;
  location?: string;
  primary_supplier?: string;
  updated_at?: string | null;
}

type Filters = { low?: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All components", filters: {} },
  { id: "low", label: "Low stock", filters: { low: true } },
];

const EMPTY_FORM = {
  component_name: "", component_sku: "", category: "", quantity_on_hand: "0",
  reorder_point: "", unit: "pcs", unit_cost: "", currency: "USD", location: "", supplier: "",
};

const isLow = (i: InventoryItem) => i.reorder_point != null && i.quantity_on_hand <= i.reorder_point;

export default function InventoryPage() {
  const views = useSavedViews<Filters>("manufacturing.inventory", VIEWS);
  const [items, setItems] = useState<InventoryItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [adjustModal, setAdjustModal] = useState<InventoryItem | null>(null);
  const [form, setForm] = useState(EMPTY_FORM);
  const [adjustForm, setAdjustForm] = useState({ delta: "", reason: "" });
  const [saving, setSaving] = useState(false);
  const [search, setSearch] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await manufacturingApi.listInventory();
      setItems(res.data.items ?? res.data ?? []);
    } catch { setItems([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function create() {
    if (!form.component_name.trim() || !form.component_sku.trim()) return;
    setSaving(true);
    try {
      await manufacturingApi.createInventoryItem({
        component_name: form.component_name.trim(),
        component_sku: form.component_sku.trim(),
        category: form.category.trim() || undefined,
        quantity_on_hand: parseInt(form.quantity_on_hand) || 0,
        reorder_point: form.reorder_point ? parseInt(form.reorder_point) : undefined,
        unit: form.unit || "pcs",
        unit_cost: form.unit_cost ? parseFloat(form.unit_cost) : undefined,
        currency: form.currency,
        location: form.location.trim() || undefined,
        supplier: form.supplier.trim() || undefined,
      });
      setShowModal(false);
      setForm(EMPTY_FORM);
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to add component"));
    } finally { setSaving(false); }
  }

  async function adjust() {
    if (!adjustModal || !adjustForm.delta) return;
    setSaving(true);
    try {
      await manufacturingApi.adjustInventory(adjustModal.id, {
        quantity_delta: parseFloat(adjustForm.delta),
        notes: adjustForm.reason.trim() || undefined,
      });
      setAdjustModal(null);
      setAdjustForm({ delta: "", reason: "" });
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to adjust stock"));
    } finally { setSaving(false); }
  }

  const lowOnly = !!views.active?.filters.low;
  const filters: Filters = lowOnly ? { low: true } : {};
  const q = search.toLowerCase();
  const filtered = items.filter((i) =>
    (!lowOnly || isLow(i)) &&
    (!search || i.component_name.toLowerCase().includes(q) || (i.sku ?? "").toLowerCase().includes(q)),
  );

  const lowStock = items.filter(isLow).length;
  const totalValue = items.reduce((s, i) => s + ((i.unit_cost ?? 0) * i.quantity_on_hand), 0);
  const skuCount = new Set(items.map((i) => i.sku).filter(Boolean)).size;

  const columns = useMemo<ColumnDef<InventoryItem>[]>(() => [
    {
      accessorKey: "component_name", header: "Component",
      cell: (c) => {
        const item = c.row.original;
        return (
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <span className="truncate font-medium text-fg">{item.component_name}</span>
              {isLow(item) && <AlertTriangle className="size-3.5 shrink-0 text-tone-orange" aria-label="Low stock" />}
            </div>
            {item.sku && <p className="font-mono text-xs text-fg-muted">{item.sku}</p>}
          </div>
        );
      },
    },
    {
      accessorKey: "quantity_on_hand", header: "On hand", meta: { numeric: true },
      cell: (c) => {
        const item = c.row.original;
        return (
          <span className={isLow(item) ? "font-semibold text-tone-orange" : "font-medium text-fg"}>
            {item.quantity_on_hand} <span className="text-xs font-normal text-fg-muted">{item.unit}</span>
          </span>
        );
      },
    },
    { accessorKey: "quantity_reserved", header: "Reserved", meta: { numeric: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<number>()}</span> },
    { accessorKey: "reorder_point", header: "Reorder", meta: { numeric: true, hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<number>() ?? "-"}</span> },
    { accessorKey: "location", header: "Location", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<string>() ?? "-"}</span> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => (
        <Button size="sm" variant="ghost" onClick={() => { setAdjustModal(c.row.original); setAdjustForm({ delta: "", reason: "" }); }}>
          <ArrowUpDown aria-hidden /> Adjust
        </Button>
      ),
    },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Components" value={items.length} />
        <Stat label="Low stock" value={lowStock} note={lowStock > 0 ? "at or below reorder point" : undefined} alert={lowStock > 0} />
        <Stat label="Total SKUs" value={skuCount} />
        <Stat label="Stock value" value={`$${totalValue.toLocaleString()}`} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Component inventory" summary="Stock levels, reorder alerts and adjustments"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Add component</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} counts={{ low: lowStock || undefined }} alertIds={["low"]} />
        <FilterChips chips={[]} onToggle={() => {}}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search components" aria-label="Search components" className="w-64" />
        </FilterChips>
        <DataTable label="Component inventory" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Boxes, title: "No inventory items", description: "Add a component to start tracking stock." }}
          renderDetails={(item) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Category", value: item.category ?? "-" },
                { label: "Available", value: `${item.quantity_on_hand - item.quantity_reserved} ${item.unit}`, numeric: true },
                { label: "Unit cost", value: item.unit_cost != null ? `${item.currency} ${item.unit_cost.toLocaleString()}` : "-", numeric: true },
                { label: "Supplier", value: item.primary_supplier ?? "-" },
                { label: "Location", value: item.location ?? "-" },
                { label: "Updated", value: item.updated_at ? new Date(item.updated_at).toLocaleDateString("en-GB") : "-" },
              ]} />
              <Button size="sm" onClick={() => { setAdjustModal(item); setAdjustForm({ delta: "", reason: "" }); }}>
                <ArrowUpDown aria-hidden /> Adjust stock
              </Button>
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Add component"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving}
            disabled={!form.component_name.trim() || !form.component_sku.trim()}>
            {saving ? "Adding..." : "Add component"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Name" required>
            <Input value={form.component_name} onChange={(e) => setForm({ ...form, component_name: e.target.value })} placeholder="e.g. Ergonomic Chair" />
          </Field>
          <Field label="SKU" required>
            <Input value={form.component_sku} onChange={(e) => setForm({ ...form, component_sku: e.target.value })} className="font-mono" />
          </Field>
          <Field label="Qty on hand">
            <Input numeric type="number" value={form.quantity_on_hand} onChange={(e) => setForm({ ...form, quantity_on_hand: e.target.value })} />
          </Field>
          <Field label="Reorder point">
            <Input numeric type="number" value={form.reorder_point} onChange={(e) => setForm({ ...form, reorder_point: e.target.value })} />
          </Field>
          <Field label="Unit">
            <Input value={form.unit} onChange={(e) => setForm({ ...form, unit: e.target.value })} placeholder="pcs" />
          </Field>
          <Field label="Category">
            <Input value={form.category} onChange={(e) => setForm({ ...form, category: e.target.value })} />
          </Field>
          <Field label="Unit cost">
            <Input numeric type="number" value={form.unit_cost} onChange={(e) => setForm({ ...form, unit_cost: e.target.value })} />
          </Field>
          <Field label="Currency">
            <Select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}
              options={["USD", "EUR", "GBP", "JPY"].map((c) => ({ value: c, label: c }))} />
          </Field>
          <Field label="Location">
            <Input value={form.location} onChange={(e) => setForm({ ...form, location: e.target.value })} placeholder="e.g. Warehouse A / Shelf B3" />
          </Field>
          <Field label="Supplier">
            <Input value={form.supplier} onChange={(e) => setForm({ ...form, supplier: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={!!adjustModal} onOpenChange={(o) => { if (!o) setAdjustModal(null); }} size="sm" title="Adjust stock"
        description={adjustModal ? `${adjustModal.component_name}, currently ${adjustModal.quantity_on_hand} ${adjustModal.unit}` : undefined}
        footer={<>
          <Button onClick={() => setAdjustModal(null)}>Cancel</Button>
          <Button variant="primary" onClick={adjust} loading={saving} disabled={!adjustForm.delta}>
            {saving ? "Saving..." : "Apply adjustment"}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Adjustment (+/-)">
            <Input numeric type="number" value={adjustForm.delta} onChange={(e) => setAdjustForm({ ...adjustForm, delta: e.target.value })} placeholder="e.g. +50 or -10" />
          </Field>
          <Field label="Reason">
            <Input value={adjustForm.reason} onChange={(e) => setAdjustForm({ ...adjustForm, reason: e.target.value })} placeholder="e.g. Received PO, used in production" />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
