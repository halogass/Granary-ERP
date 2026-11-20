"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { manufacturingApi } from "@/lib/api";
import { Package, Plus, Trash2, X } from "lucide-react";
import ApprovalPanel from "@/components/approvals/ApprovalPanel";
import { ProductLineSelect, useProductLines, lineLookup } from "@/components/catalog/ProductLineSelect";
import {
  Button, DataTable, Dialog, Field, FieldGrid, IconButton, Input, ListFrame, PageHeader, Pill, SavedViewTabs,
  Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView, type Tone,
} from "@/components/ui";

// Shape of GET /manufacturing/bom/{product_id} items.
interface BOMItem {
  component_name: string;
  sku?: string | null;
  quantity: number;
  unit: string;
}

interface HardwareProduct {
  id: string;
  product_code: string;
  name: string;
  category: string;
  description?: string;
  unit_cost?: number;
  currency: string;
  is_active: boolean;
  product_line_id?: string | null;
  bom?: BOMItem[];
  /** 3 Nov 2026: the prefix of its units' generated serials (DSK, SOV); none = units cannot be registered. */
  serial_prefix?: string | null;
}

/** "DSK": 2 to 6 letters, upper case; empty is no prefix. */
function cleanPrefix(raw: string): string {
  return raw.toUpperCase().replace(/[^A-Z]/g, "").slice(0, 6);
}

/** The product's serial prefix, edited in the list (saved on blur or Enter). */
function SerialPrefixInput({ product, onSaved }: { product: HardwareProduct; onSaved: (p: string | null) => void }) {
  const [value, setValue] = useState(product.serial_prefix ?? "");
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { setValue(product.serial_prefix ?? ""); }, [product.serial_prefix]);
  async function save() {
    const next = cleanPrefix(value);
    if (next === (product.serial_prefix ?? "")) return;
    if (next.length === 1) { setError("2 to 6 letters"); return; }
    setError(null);
    try {
      const res = await manufacturingApi.updateProduct(product.id, { serial_prefix: next || null });
      onSaved((res.data?.serial_prefix as string | null) ?? (next || null));
    } catch (err) {
      const detail = (err as { response?: { data?: { detail?: unknown } } })?.response?.data?.detail;
      setError(typeof detail === "string" ? detail : "Not saved");
    }
  }
  return (
    <div>
      <Input aria-label={`Serial prefix of ${product.name}`} className="w-20 font-mono uppercase" placeholder="None"
        value={value} maxLength={6} onChange={(e) => setValue(cleanPrefix(e.target.value))} onBlur={save}
        onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }} />
      {error && <span role="alert" className="mt-0.5 block text-xs text-tone-red">{error}</span>}
    </div>
  );
}

const CATEGORY_META: Record<string, { label: string; tone: Tone }> = {
  finished_good: { label: "Finished good", tone: "blue" },
  assembly:   { label: "Assembly",    tone: "violet" },
  peripheral: { label: "Peripheral",  tone: "neutral" },
  component:  { label: "Component",   tone: "neutral" },
  other:      { label: "Other",       tone: "neutral" },
};

type Filters = { active?: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All products", filters: {} },
  { id: "active", label: "Active", filters: { active: true } },
  { id: "inactive", label: "Inactive", filters: { active: false } },
];

const EMPTY_FORM = { product_code: "", name: "", category: "finished_good", description: "", unit_cost: "", currency: "USD", product_line_id: "", serial_prefix: "" };
const EMPTY_BOM_ROW = { component_name: "", component_sku: "", quantity: "1", unit: "pcs" };

export default function ManufacturingProductsPage() {
  const views = useSavedViews<Filters>("manufacturing.products", VIEWS);
  const [products, setProducts] = useState<HardwareProduct[]>([]);
  const [loading, setLoading] = useState(true);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [bomData, setBomData] = useState<Record<string, BOMItem[]>>({});
  const [showModal, setShowModal] = useState(false);
  const [showBomModal, setShowBomModal] = useState<string | null>(null);
  const [form, setForm] = useState(EMPTY_FORM);
  const [bomForm, setBomForm] = useState([EMPTY_BOM_ROW]);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const lines = useProductLines();
  const lookup = lineLookup(lines);

  async function setLine(id: string, product_line_id: string | null) {
    setProducts((prev) => prev.map((p) => p.id === id ? { ...p, product_line_id } : p));
    try { await manufacturingApi.updateProduct(id, { product_line_id }); } catch { load(); }
  }

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<HardwareProduct>(manufacturingApi.listProducts);
      setProducts(rows);
    } catch { setProducts([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  // The shown BOM's id, version and status (a new BOM goes to approval; production uses an approved one).
  const [bomMeta, setBomMeta] = useState<Record<string, { id: string; version: string; status: string }>>({});

  async function fetchBOM(productId: string) {
    try {
      const res = await manufacturingApi.getBOM(productId);
      setBomData((prev) => ({ ...prev, [productId]: res.data.items ?? [] }));
      if (res.data.id) setBomMeta((prev) => ({ ...prev, [productId]: { id: res.data.id, version: res.data.version, status: res.data.status } }));
    } catch { setBomData((prev) => ({ ...prev, [productId]: [] })); }
  }

  async function toggleBOM(productId: string | null) {
    setExpandedId(productId);
    if (productId && !bomData[productId]) await fetchBOM(productId);
  }

  async function create() {
    if (!form.product_code.trim() || !form.name.trim()) return;
    setSaving(true);
    try {
      await manufacturingApi.createProduct({
        product_code: form.product_code.trim(),
        name: form.name.trim(),
        category: form.category,
        description: form.description.trim() || undefined,
        unit_cost: form.unit_cost ? parseFloat(form.unit_cost) : undefined,
        currency: form.currency,
        product_line_id: form.product_line_id || undefined,
        serial_prefix: form.serial_prefix || undefined,
      });
      setShowModal(false);
      setForm(EMPTY_FORM);
      await load();
    } finally { setSaving(false); }
  }

  async function deleteProduct(id: string) {
    try {
      await manufacturingApi.deleteProduct(id);
      setConfirmDeleteId(null);
      setProducts((prev) => prev.filter((p) => p.id !== id));
    } catch { /* keep */ }
  }

  async function saveBOM(productId: string) {
    setSaving(true);
    try {
      await manufacturingApi.createBOM({
        product_id: productId,
        items: bomForm.filter((b) => b.component_name.trim()).map((b) => ({
          component_name: b.component_name.trim(),
          component_sku: b.component_sku.trim() || undefined,
          quantity: parseFloat(b.quantity) || 1,
          unit: b.unit || "pcs",
        })),
      });
      setShowBomModal(null);
      setBomForm([EMPTY_BOM_ROW]);
      await fetchBOM(productId);
    } finally { setSaving(false); }
  }

  const active = products.filter((p) => p.is_active).length;
  const activeFilter = views.active?.filters.active;
  const filters: Filters = activeFilter === undefined ? {} : { active: activeFilter };
  const rows = activeFilter === undefined ? products : products.filter((p) => p.is_active === activeFilter);

  const columns = useMemo<ColumnDef<HardwareProduct>[]>(() => [
    {
      accessorKey: "name", header: "Product",
      cell: (c) => {
        const p = c.row.original;
        return (
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <span className="truncate font-medium text-fg">{p.name}</span>
              <span className="font-mono text-xs text-fg-muted">{p.product_code}</span>
            </div>
            {p.description && <p className="max-w-md truncate text-xs text-fg-muted">{p.description}</p>}
          </div>
        );
      },
    },
    {
      accessorKey: "category", header: "Category",
      cell: (c) => { const cat = CATEGORY_META[c.getValue<string>()] ?? CATEGORY_META.other; return <Pill tone={cat.tone}>{cat.label}</Pill>; },
    },
    {
      id: "line", header: "Product line", enableSorting: false, meta: { hideOnMobile: true, width: "200px" },
      cell: (c) => {
        const p = c.row.original;
        return (
          <div className="w-44">
            <ProductLineSelect value={p.product_line_id ?? null} onChange={(v) => setLine(p.id, v)} placeholder="Set line..." />
          </div>
        );
      },
    },
    {
      id: "serial_prefix", header: "Serial prefix", enableSorting: false, meta: { hideOnMobile: true, width: "110px" },
      cell: (c) => {
        const p = c.row.original;
        return (
          <SerialPrefixInput product={p}
            onSaved={(serial_prefix) => setProducts((prev) => prev.map((x) => x.id === p.id ? { ...x, serial_prefix } : x))} />
        );
      },
    },
    {
      accessorKey: "unit_cost", header: "Unit cost", meta: { numeric: true },
      cell: (c) => {
        const p = c.row.original;
        return p.unit_cost ? <span className="font-medium">{p.unit_cost.toLocaleString()} <span className="text-xs font-normal text-fg-muted">{p.currency}</span></span> : <span className="text-fg-subtle">-</span>;
      },
    },
    {
      accessorKey: "is_active", header: "Status",
      cell: (c) => <StatusPill status={c.getValue<boolean>() ? "active" : "inactive"} />,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const p = c.row.original;
        if (confirmDeleteId === p.id) {
          return (
            <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
              <span className="text-xs font-semibold text-tone-red">Delete?</span>
              <Button size="sm" variant="danger" onClick={() => deleteProduct(p.id)}>Yes</Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
            </div>
          );
        }
        return (
          <div className="flex items-center justify-end gap-1 whitespace-nowrap">
            <Button size="sm" variant="ghost" onClick={() => { setShowBomModal(p.id); setBomForm([EMPTY_BOM_ROW]); }}>
              <Plus aria-hidden /> BOM
            </Button>
            <IconButton size="sm" icon={Trash2} label={`Delete ${p.name}`} onClick={() => setConfirmDeleteId(p.id)} />
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, lines]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={products.length} />
        <Stat label="Active" value={active} />
        <Stat label="Categories" value={new Set(products.map((p) => p.category)).size} />
        <Stat label="Inactive" value={products.length - active} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Hardware products" summary="Products the company manufactures and assembles"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New product</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} />
        <DataTable label="Hardware products" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          expandedId={expandedId} onExpandedChange={toggleBOM}
          empty={{ icon: Package, title: "No hardware products", description: "Create a product to define its bill of materials." }}
          renderDetails={(product) => {
            const bom = bomData[product.id];
            const meta = bomMeta[product.id];
            const pl = lookup(product.product_line_id);
            return (
              <div className="space-y-3">
                <div className="flex flex-wrap items-center gap-2">
                  <h3 className="text-sm font-semibold text-fg">Bill of materials</h3>
                  {meta && <span className="text-xs text-fg-muted">v{meta.version}</span>}
                  {meta && <StatusPill status={meta.status} />}
                  {pl && (
                    <span className="inline-flex items-center gap-1.5 text-xs text-fg-muted">
                      {/* the line's own color (data, set per product line) */}
                      <span className="size-2 rounded-full" style={{ background: pl.color }} aria-hidden />{pl.name}
                    </span>
                  )}
                </div>
                {!bom ? (
                  <p className="text-xs text-fg-muted">Loading...</p>
                ) : bom.length === 0 ? (
                  <p className="text-xs text-fg-muted">No BOM defined yet.</p>
                ) : (
                  <ul className="max-w-2xl divide-y divide-line-soft rounded-lg border border-line bg-surface">
                    {bom.map((item, i) => (
                      <li key={i} className="flex items-center gap-3 px-3 py-2 text-sm">
                        <span className="font-medium text-fg">{item.component_name}</span>
                        {item.sku && <span className="font-mono text-xs text-fg-muted">{item.sku}</span>}
                        <span className="ml-auto text-xs text-fg-muted tabular">{item.quantity} {item.unit}</span>
                      </li>
                    ))}
                  </ul>
                )}
                {meta && meta.status !== "archived" && (
                  <ApprovalPanel entityType="bom" entityId={meta.id} onChange={() => fetchBOM(product.id)} />
                )}
              </div>
            );
          }} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="New hardware product"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving} disabled={!form.product_code.trim() || !form.name.trim()}>
            {saving ? "Creating..." : "Create product"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Product code" required>
            <Input value={form.product_code} onChange={(e) => setForm({ ...form, product_code: e.target.value })} placeholder="e.g. DSK-001" className="font-mono" />
          </Field>
          <Field label="Category">
            <Select value={form.category} onChange={(e) => setForm({ ...form, category: e.target.value })}
              options={Object.entries(CATEGORY_META).map(([k, v]) => ({ value: k, label: v.label }))} />
          </Field>
          <Field label="Name" required className="md:col-span-2">
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Standing Desk Pro" />
          </Field>
          <Field label="Product line" className="md:col-span-2">
            <ProductLineSelect value={form.product_line_id || null} onChange={(v) => setForm({ ...form, product_line_id: v ?? "" })} />
          </Field>
          <Field label="Serial prefix" hint="2 to 6 letters, e.g. DSK: its units' serials are generated as DSK-XXXXX-XXXXX.">
            <Input className="font-mono uppercase" placeholder="e.g. DSK" maxLength={6} value={form.serial_prefix}
              onChange={(e) => setForm({ ...form, serial_prefix: cleanPrefix(e.target.value) })} />
          </Field>
          <Field label="Description" className="md:col-span-2">
            <Textarea value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} rows={2} />
          </Field>
          <Field label="Unit cost">
            <Input numeric type="number" value={form.unit_cost} onChange={(e) => setForm({ ...form, unit_cost: e.target.value })} />
          </Field>
          <Field label="Currency">
            <Select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}
              options={["USD", "EUR", "GBP", "JPY"].map((c) => ({ value: c, label: c }))} />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={!!showBomModal} onOpenChange={(o) => { if (!o) setShowBomModal(null); }} size="lg" title="Define bill of materials"
        description={products.find((p) => p.id === showBomModal)?.name}
        footer={<>
          <Button onClick={() => setShowBomModal(null)}>Cancel</Button>
          <Button variant="primary" onClick={() => showBomModal && saveBOM(showBomModal)} loading={saving}>
            {saving ? "Saving..." : "Save BOM"}
          </Button>
        </>}>
        <div className="space-y-2">
          <div className="hidden grid-cols-[1fr_96px_80px_72px_32px] gap-2 px-1 text-xs font-medium text-fg-muted md:grid">
            <span>Component</span><span>SKU</span><span>Qty</span><span>Unit</span><span />
          </div>
          {bomForm.map((row, i) => (
            <div key={i} className="grid grid-cols-2 items-center gap-2 md:grid-cols-[1fr_96px_80px_72px_32px]">
              <Input aria-label="Component name" value={row.component_name} className="col-span-2 md:col-span-1"
                onChange={(e) => setBomForm(bomForm.map((r, idx) => idx === i ? { ...r, component_name: e.target.value } : r))} placeholder="Component name" />
              <Input aria-label="SKU" value={row.component_sku} className="font-mono"
                onChange={(e) => setBomForm(bomForm.map((r, idx) => idx === i ? { ...r, component_sku: e.target.value } : r))} placeholder="SKU" />
              <Input aria-label="Quantity" numeric type="number" value={row.quantity}
                onChange={(e) => setBomForm(bomForm.map((r, idx) => idx === i ? { ...r, quantity: e.target.value } : r))} />
              <Input aria-label="Unit" value={row.unit}
                onChange={(e) => setBomForm(bomForm.map((r, idx) => idx === i ? { ...r, unit: e.target.value } : r))} placeholder="pcs" />
              {bomForm.length > 1 ? (
                <IconButton size="sm" icon={X} label="Remove component" onClick={() => setBomForm(bomForm.filter((_, idx) => idx !== i))} />
              ) : <span />}
            </div>
          ))}
          <Button size="sm" variant="ghost" onClick={() => setBomForm([...bomForm, EMPTY_BOM_ROW])}>
            <Plus aria-hidden /> Add component
          </Button>
        </div>
      </Dialog>
    </div>
  );
}
