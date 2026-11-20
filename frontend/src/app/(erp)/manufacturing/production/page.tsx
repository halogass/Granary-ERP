"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { manufacturingApi } from "@/lib/api";
import { Factory, Plus, DollarSign, Trash2, ArrowRight } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, IconButton, Input, KeyValues, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface ProductionOrder {
  id: string;
  order_number: string;
  product_id?: string;
  product?: { name: string; product_code: string };
  quantity: number;
  status: string;
  due_date?: string;
  notes?: string;
  created_at?: string;
  total_bom_cost?: number | null;
  actual_material_cost?: number | null;
  cost_currency?: string;
  variance_pct?: number | null;
}

interface HardwareProduct {
  id: string;
  product_code: string;
  name: string;
}

const STATUS_LABELS: Record<string, string> = {
  planned: "Planned", component_check: "Component check", in_production: "In production",
  qc: "QC", completed: "Completed", cancelled: "Cancelled",
};

const NEXT_STATUS: Record<string, string> = {
  planned: "component_check",
  component_check: "in_production",
  in_production: "qc",
  qc: "completed",
};

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All orders", filters: {} },
  { id: "planned", label: "Planned", filters: { status: "planned" } },
  { id: "in_production", label: "In production", filters: { status: "in_production" } },
  { id: "qc", label: "QC", filters: { status: "qc" } },
  { id: "completed", label: "Completed", filters: { status: "completed" } },
];

function VarianceBadge({ pct }: { pct: number | null | undefined }) {
  if (pct == null) return <span className="text-fg-subtle">-</span>;
  const abs = Math.abs(pct);
  return (
    <Pill tone={abs <= 5 ? "neutral" : abs <= 15 ? "orange" : "red"} className="tabular">
      {pct >= 0 ? "+" : ""}{pct.toFixed(1)}%
    </Pill>
  );
}

function fmtCost(val: number | null | undefined, currency = "USD") {
  if (val == null) return "-";
  return new Intl.NumberFormat("en-US", { style: "currency", currency, maximumFractionDigits: 2 }).format(val);
}

const EMPTY_FORM = { product_id: "", bom_id: "", quantity: "1", due_date: "", notes: "" };

export default function ProductionPage() {
  const views = useSavedViews<Filters>("manufacturing.production", VIEWS);
  const [orders, setOrders] = useState<ProductionOrder[]>([]);
  const [products, setProducts] = useState<HardwareProduct[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [costModal, setCostModal] = useState<ProductionOrder | null>(null);
  const [actualCost, setActualCost] = useState("");
  const [form, setForm] = useState(EMPTY_FORM);
  const [productBom, setProductBom] = useState<{ id: string; version: string; status: string } | null>(null);
  const [bomLoading, setBomLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [oRes, pRes] = await Promise.all([
        fetchAllPages<ProductionOrder>(manufacturingApi.listProductionOrders),
        fetchAllPages<HardwareProduct>(manufacturingApi.listProducts),
      ]);
      setOrders(oRes.rows);
      setProducts(pRes.rows);
    } catch { setOrders([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  // The backend exposes one BOM per product (latest approved, else newest); preselect it.
  async function selectProduct(productId: string) {
    setForm((f) => ({ ...f, product_id: productId, bom_id: "" }));
    setProductBom(null);
    if (!productId) return;
    setBomLoading(true);
    try {
      const res = await manufacturingApi.getBOM(productId);
      setProductBom(res.data);
      setForm((f) => (f.product_id === productId ? { ...f, bom_id: res.data.id } : f));
    } catch { /* 404: product has no BOM yet */ } finally { setBomLoading(false); }
  }

  async function create() {
    if (!form.product_id || !form.quantity) return;
    setSaving(true);
    try {
      await manufacturingApi.createProductionOrder({
        product_id: form.product_id,
        bom_id: form.bom_id || undefined,
        quantity: parseInt(form.quantity) || 1,
        due_date: form.due_date || undefined,
        notes: form.notes.trim() || undefined,
      });
      setShowModal(false);
      setForm(EMPTY_FORM);
      setProductBom(null);
      await load();
    } finally { setSaving(false); }
  }

  async function advance(id: string, nextStatus: string) {
    await manufacturingApi.updateProductionOrderStatus(id, { status: nextStatus });
    await load();
  }

  async function deleteOrder(id: string) {
    try {
      await manufacturingApi.deleteProductionOrder(id);
      setConfirmDeleteId(null);
      setOrders((prev) => prev.filter((o) => o.id !== id));
    } catch { /* keep */ }
  }

  async function saveActualCost() {
    if (!costModal || !actualCost) return;
    setSaving(true);
    try {
      await manufacturingApi.updateProductionCost(costModal.id, { actual_material_cost: parseFloat(actualCost) });
      setCostModal(null);
      setActualCost("");
      await load();
    } finally { setSaving(false); }
  }

  const inProgress = orders.filter((o) => o.status === "in_production").length;
  const completed = orders.filter((o) => o.status === "completed").length;
  const statusFilter = views.active?.filters.status ?? "";
  const filters: Filters = statusFilter ? { status: statusFilter } : {};
  const rows = statusFilter ? orders.filter((o) => o.status === statusFilter) : orders;

  function rowActions(order: ProductionOrder) {
    const nextStatus = NEXT_STATUS[order.status];
    const canSetCost = ["in_production", "qc", "completed"].includes(order.status);
    const canDelete = ["planned", "cancelled"].includes(order.status);
    if (canDelete && confirmDeleteId === order.id) {
      return (
        <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
          <span className="text-xs font-semibold text-tone-red">Delete?</span>
          <Button size="sm" variant="danger" onClick={() => deleteOrder(order.id)}>Yes</Button>
          <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
        </div>
      );
    }
    return (
      <div className="flex items-center justify-end gap-1 whitespace-nowrap">
        {canSetCost && (
          <Button size="sm" variant="ghost" onClick={() => { setCostModal(order); setActualCost(String(order.actual_material_cost ?? "")); }}>
            <DollarSign aria-hidden /> Cost
          </Button>
        )}
        {nextStatus && (
          <Button size="sm" variant="ghost" onClick={() => advance(order.id, nextStatus)}>
            <ArrowRight aria-hidden /> {STATUS_LABELS[nextStatus]}
          </Button>
        )}
        {canDelete && <IconButton size="sm" icon={Trash2} label={`Delete ${order.order_number}`} onClick={() => setConfirmDeleteId(order.id)} />}
      </div>
    );
  }

  const columns = useMemo<ColumnDef<ProductionOrder>[]>(() => [
    {
      id: "product", accessorFn: (o) => o.product?.name ?? "", header: "Product",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.getValue<string>() || "-"}</p>
          <p className="font-mono text-xs text-fg-muted">{c.row.original.order_number}</p>
        </div>
      ),
    },
    { accessorKey: "quantity", header: "Qty", meta: { numeric: true } },
    {
      accessorKey: "due_date", header: "Due", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string>() ? new Date(c.getValue<string>()).toLocaleDateString("en-GB", { day: "numeric", month: "short" }) : "-"}</span>,
    },
    { accessorKey: "total_bom_cost", header: "BOM cost", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmtCost(c.getValue<number | null>(), c.row.original.cost_currency) },
    { accessorKey: "actual_material_cost", header: "Actual", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmtCost(c.getValue<number | null>(), c.row.original.cost_currency) },
    { accessorKey: "variance_pct", header: "Variance", meta: { numeric: true, hideOnMobile: true }, cell: (c) => <VarianceBadge pct={c.getValue<number | null>()} /> },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="production" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} /> },
    { id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" }, cell: (c) => rowActions(c.row.original) },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={orders.length} />
        <Stat label="In production" value={inProgress} />
        <Stat label="Completed" value={completed} />
        <Stat label="Planned" value={orders.filter((o) => o.status === "planned").length} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Production orders" summary="Hardware assembly and manufacturing runs"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New order</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} />
        <DataTable label="Production orders" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Factory, title: "No production orders", description: "Create an order to start a manufacturing run." }}
          renderDetails={(o) => (
            <KeyValues cols={3} items={[
              { label: "Order", value: o.order_number },
              { label: "Product code", value: o.product?.product_code ?? "-" },
              { label: "Quantity", value: o.quantity, numeric: true },
              { label: "BOM cost", value: fmtCost(o.total_bom_cost, o.cost_currency), numeric: true },
              { label: "Actual cost", value: fmtCost(o.actual_material_cost, o.cost_currency), numeric: true },
              { label: "Notes", value: o.notes ?? "-" },
            ]} />
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="New production order"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving} disabled={!form.product_id}>{saving ? "Creating..." : "Create order"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Product" required className="md:col-span-2">
            <Select value={form.product_id} onChange={(e) => selectProduct(e.target.value)} placeholder="Select product"
              options={products.map((p) => ({ value: p.id, label: `${p.name} (${p.product_code})` }))} />
          </Field>
          {form.product_id && (
            <Field label="Bill of materials" className="md:col-span-2">
              <Select value={form.bom_id} onChange={(e) => setForm({ ...form, bom_id: e.target.value })} disabled={bomLoading}
                placeholder={bomLoading ? "Loading..." : "No BOM (required before production starts)"}
                options={productBom ? [{ value: productBom.id, label: `v${productBom.version} (${productBom.status})` }] : []} />
            </Field>
          )}
          <Field label="Quantity" required>
            <Input numeric type="number" min="1" value={form.quantity} onChange={(e) => setForm({ ...form, quantity: e.target.value })} />
          </Field>
          <Field label="Due date">
            <Input type="date" value={form.due_date} onChange={(e) => setForm({ ...form, due_date: e.target.value })} />
          </Field>
          <Field label="Notes" className="md:col-span-2">
            <Textarea value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} rows={2} />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={!!costModal} onOpenChange={(o) => { if (!o) { setCostModal(null); setActualCost(""); } }} size="sm"
        title="Set actual material cost"
        description={costModal ? `${costModal.order_number}, BOM standard ${fmtCost(costModal.total_bom_cost, costModal.cost_currency)}` : undefined}
        footer={<>
          <Button onClick={() => { setCostModal(null); setActualCost(""); }}>Cancel</Button>
          <Button variant="primary" onClick={saveActualCost} loading={saving} disabled={!actualCost}>{saving ? "Saving..." : "Save"}</Button>
        </>}>
        <Field label={`Actual cost (${costModal?.cost_currency ?? "USD"})`}>
          <Input numeric type="number" step="0.01" min="0" value={actualCost} onChange={(e) => setActualCost(e.target.value)} placeholder="0.00" />
        </Field>
      </Dialog>
    </div>
  );
}
