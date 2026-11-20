"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { ArrowRight, ArrowDown, ArrowUp, RefreshCw, Shuffle, Plus, Search } from "lucide-react";
import api from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, Input, KeyValues, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, Stat, StatGroup, toast, useSavedViews, type ColumnDef, type SavedView, type Tone,
} from "@/components/ui";

interface StockMove {
  id: string;
  component_sku: string;
  component_name: string | null;
  from_location_id: string | null;
  from_location_name: string | null;
  to_location_id: string | null;
  to_location_name: string | null;
  quantity: number;
  move_type: string;
  reference: string | null;
  notes: string | null;
  moved_at: string | null;
}

interface Location {
  id: string;
  name: string;
  full_path: string;
  location_type: string;
}

const MOVE_META: Record<string, { label: string; tone: Tone; Icon: React.ElementType }> = {
  receipt:    { label: "Receipt",    tone: "blue",    Icon: ArrowDown  },
  issue:      { label: "Issue",      tone: "orange",  Icon: ArrowUp    },
  internal:   { label: "Transfer",   tone: "violet",  Icon: Shuffle    },
  adjustment: { label: "Adjustment", tone: "neutral", Icon: RefreshCw  },
};

const EMPTY_FORM = {
  component_sku: "", component_name: "", from_location_id: "", to_location_id: "",
  quantity: "", move_type: "receipt", reference: "", notes: "",
};

type Filters = { move_type?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All moves", filters: {} },
  { id: "receipt", label: "Receipts", filters: { move_type: "receipt" } },
  { id: "issue", label: "Issues", filters: { move_type: "issue" } },
  { id: "internal", label: "Transfers", filters: { move_type: "internal" } },
  { id: "adjustment", label: "Adjustments", filters: { move_type: "adjustment" } },
];

function fmt(d: string | null) {
  if (!d) return "-";
  return new Date(d).toLocaleString("en-GB", { day: "2-digit", month: "short", hour: "2-digit", minute: "2-digit" });
}

export default function TransfersPage() {
  const views = useSavedViews<Filters>("manufacturing.transfers", VIEWS);
  const [moves, setMoves] = useState<StockMove[]>([]);
  const [loading, setLoading] = useState(true);
  const [locations, setLocations] = useState<Location[]>([]);
  const [skuFilter, setSkuFilter] = useState("");
  const typeFilter = views.active?.filters.move_type ?? "";
  const filters: Filters = typeFilter ? { move_type: typeFilter } : {};

  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (typeFilter) params.move_type = typeFilter;
      if (skuFilter) params.component_sku = skuFilter;
      const { rows } = await fetchAllPages<StockMove>((p) => api.get("/inventory/stock/moves", { params: p }), params);
      setMoves(rows);
    } catch { setMoves([]); } finally { setLoading(false); }
  }, [typeFilter, skuFilter]);

  useEffect(() => { load(); }, [load]);

  useEffect(() => {
    api.get("/inventory/locations").then((r) => setLocations(r.data ?? [])).catch(() => {});
  }, []);

  async function save() {
    if (!form.component_sku.trim() || !form.quantity) return toast.error("SKU and quantity are required");
    const qty = parseFloat(form.quantity);
    if (isNaN(qty) || qty <= 0) return toast.error("Quantity must be a positive number");

    setSaving(true);
    try {
      const payload = {
        component_sku: form.component_sku.trim(),
        component_name: form.component_name || null,
        from_location_id: form.from_location_id || null,
        to_location_id: form.to_location_id || null,
        quantity: qty,
        move_type: form.move_type,
        reference: form.reference || null,
        notes: form.notes || null,
      };

      if (form.move_type === "internal") {
        if (!form.from_location_id || !form.to_location_id) {
          setSaving(false);
          return toast.error("Internal transfer requires both from and to locations");
        }
        await api.post("/inventory/stock/transfer", payload);
      } else {
        await api.post("/inventory/stock/moves", payload);
      }

      toast.success("Stock move recorded");
      setShowModal(false);
      setForm(EMPTY_FORM);
      load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to record move");
    } finally {
      setSaving(false);
    }
  }

  const counts = {
    total: moves.length,
    receipts: moves.filter((m) => m.move_type === "receipt").length,
    issues: moves.filter((m) => m.move_type === "issue").length,
    transfers: moves.filter((m) => m.move_type === "internal").length,
  };

  const locationOptions = locations.map((l) => ({ value: l.id, label: l.full_path }));

  const columns = useMemo<ColumnDef<StockMove>[]>(() => [
    {
      accessorKey: "move_type", header: "Type",
      cell: (c) => { const meta = MOVE_META[c.getValue<string>()] ?? MOVE_META.adjustment; return <Pill tone={meta.tone} icon={meta.Icon}>{meta.label}</Pill>; },
    },
    {
      accessorKey: "component_sku", header: "SKU / component",
      cell: (c) => (
        <div>
          <p className="font-mono text-[13px] font-medium text-fg">{c.row.original.component_sku}</p>
          {c.row.original.component_name && <p className="text-xs text-fg-muted">{c.row.original.component_name}</p>}
        </div>
      ),
    },
    {
      id: "route", header: "From / to", enableSorting: false, meta: { hideOnMobile: true },
      cell: (c) => (
        <span className="inline-flex items-center gap-2 text-[13px] text-fg-muted">
          {c.row.original.from_location_name ?? <span className="text-fg-subtle">-</span>}
          <ArrowRight className="size-3.5 text-fg-subtle" aria-label="to" />
          {c.row.original.to_location_name ?? <span className="text-fg-subtle">-</span>}
        </span>
      ),
    },
    { accessorKey: "quantity", header: "Qty", meta: { numeric: true }, cell: (c) => <span className="font-medium">{c.getValue<number>()}</span> },
    { accessorKey: "reference", header: "Reference", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<string>() ?? "-"}</span> },
    { accessorKey: "moved_at", header: "Date", meta: { numeric: true }, cell: (c) => <span className="text-fg-muted">{fmt(c.getValue<string | null>())}</span> },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total moves" value={counts.total} />
        <Stat label="Receipts" value={counts.receipts} />
        <Stat label="Issues" value={counts.issues} />
        <Stat label="Transfers" value={counts.transfers} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Stock movements" summary="Receipts, issues, transfers and adjustments"
          actions={<Button variant="primary" onClick={() => { setForm(EMPTY_FORM); setShowModal(true); }}><Plus aria-hidden /> Record move</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} />
        <FilterChips chips={[]} onToggle={() => {}}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={skuFilter} onChange={(e) => setSkuFilter(e.target.value)} placeholder="Filter by SKU" aria-label="Filter by SKU" className="w-56" />
        </FilterChips>
        <DataTable label="Stock movements" columns={columns} data={moves} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Shuffle, title: "No stock movements yet", description: "Record a receipt, issue or transfer." }}
          renderDetails={(m) => (
            <KeyValues cols={3} items={[
              { label: "From", value: m.from_location_name ?? "-" },
              { label: "To", value: m.to_location_name ?? "-" },
              { label: "Quantity", value: m.quantity, numeric: true },
              { label: "Reference", value: m.reference ?? "-" },
              { label: "Notes", value: m.notes ?? "-" },
              { label: "Moved", value: fmt(m.moved_at) },
            ]} />
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Record stock movement"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={save} loading={saving}>{saving ? "Saving..." : "Record move"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Movement type" className="md:col-span-2">
            <Select value={form.move_type} onChange={(e) => setForm({ ...form, move_type: e.target.value })} options={[
              { value: "receipt", label: "Receipt (stock in)" },
              { value: "issue", label: "Issue (stock out)" },
              { value: "internal", label: "Internal transfer (location to location)" },
              { value: "adjustment", label: "Adjustment (manual correction)" },
            ]} />
          </Field>
          <Field label="Component SKU" required>
            <Input value={form.component_sku} onChange={(e) => setForm({ ...form, component_sku: e.target.value })} className="font-mono" placeholder="e.g. CHR-ERGO-01" />
          </Field>
          <Field label="Quantity" required>
            <Input numeric type="number" min="0.001" step="0.001" value={form.quantity} onChange={(e) => setForm({ ...form, quantity: e.target.value })} placeholder="0" />
          </Field>
          <Field label="Component name" hint="Optional, filled from inventory when empty." className="md:col-span-2">
            <Input value={form.component_name} onChange={(e) => setForm({ ...form, component_name: e.target.value })} />
          </Field>
          {(form.move_type === "issue" || form.move_type === "internal") && (
            <Field label="From location" className="md:col-span-2">
              <Select value={form.from_location_id} onChange={(e) => setForm({ ...form, from_location_id: e.target.value })}
                placeholder="Select location" options={locationOptions} />
            </Field>
          )}
          {(form.move_type === "receipt" || form.move_type === "internal" || form.move_type === "adjustment") && (
            <Field label="To location" className="md:col-span-2">
              <Select value={form.to_location_id} onChange={(e) => setForm({ ...form, to_location_id: e.target.value })}
                placeholder="Select location" options={locationOptions} />
            </Field>
          )}
          <Field label="Reference">
            <Input value={form.reference} onChange={(e) => setForm({ ...form, reference: e.target.value })} placeholder="PO number, WO number" />
          </Field>
          <Field label="Notes">
            <Input value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
