"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { manufacturingApi } from "@/lib/api";
import { usePermissions } from "@/hooks/usePermissions";
import { Cpu, Search, Plus, Tag, Truck } from "lucide-react";
import GeneratedSerial from "@/components/manufacturing/GeneratedSerial";
import { apiErrorMessage, normalizeSerial } from "@/components/manufacturing/units";
import {
  Button, Checkbox, DataTable, Dialog, Field, FilterChips, Input, KeyValues, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Unit {
  id: string;
  serial_number: string;
  product_id: string;
  product?: { sku: string; name: string };
  production_order_id: string;
  status: string;
  hardware_revision?: string;
  firmware_version?: string;
  software_version?: string;
  produced_at?: string;
  flashed_at?: string;
  shipped_at?: string;
  delivery_date?: string;
  warranty_expiry?: string;
  tracking_number?: string;
  invoice_id?: string;
  notes?: string;
  created_at?: string;
  is_test?: boolean;
}

interface Product {
  id: string;
  sku: string;
  name: string;
}

const STATUS_LABELS: Record<string, string> = {
  in_production: "In production",
  qc_pending:    "QC pending",
  qc_passed:     "QC passed",
  qc_failed:     "QC failed",
  rework:        "Rework",
  in_stock:      "In stock",
  reserved:      "Reserved",
  shipped:       "Shipped",
  returned:      "Returned",
};

const STATUS_TRANSITIONS: Record<string, string[]> = {
  in_production: ["qc_pending"],
  qc_pending:    ["qc_passed", "qc_failed"],
  qc_failed:     ["rework", "qc_pending"],
  rework:        ["qc_pending"],
  qc_passed:     ["in_stock"],
  in_stock:      ["reserved", "shipped"],
  reserved:      ["shipped", "in_stock"],
  shipped:       ["returned"],
};

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All units", filters: {} },
  { id: "in_production", label: "In production", filters: { status: "in_production" } },
  { id: "qc_pending", label: "QC pending", filters: { status: "qc_pending" } },
  { id: "in_stock", label: "In stock", filters: { status: "in_stock" } },
  { id: "shipped", label: "Shipped", filters: { status: "shipped" } },
];

const CHIP_STATUSES = ["qc_passed", "qc_failed", "rework", "reserved", "returned"];

// 3 Nov 2026: no serial field: the ERP generates it from the product's serial prefix
const EMPTY_FORM = { production_order_id: "", product_id: "", hardware_revision: "", notes: "", is_test: false };
const fmt = (d?: string) => d ? new Date(d).toLocaleDateString("en-GB") : "-";

export default function UnitsPage() {
  const views = useSavedViews<Filters>("manufacturing.units", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [units, setUnits] = useState<Unit[]>([]);
  const [products, setProducts] = useState<Product[]>([]);
  const [productionOrders, setProductionOrders] = useState<{ id: string; order_number: string; product?: { name: string } }[]>([]);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [filterProduct, setFilterProduct] = useState("");
  const [showRegister, setShowRegister] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [updatingId, setUpdatingId] = useState<string | null>(null);
  const [registerError, setRegisterError] = useState<string | null>(null);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [registered, setRegistered] = useState<{ id: string; serial: string; is_test: boolean } | null>(null);
  const { holds } = usePermissions();
  // Registering, moving, relabelling and delivery details all need manufacturing:manage_production.
  const canManage = holds("manufacturing:manage_production");
  // Relabel: a new serial for a unit (a damaged or wrong label).
  const [relabel, setRelabel] = useState<{ unit: Unit; serial: string; error: string | null; saving: boolean } | null>(null);
  // Delivery: the shipment's tracking number and the date the customer received it.
  const [delivery, setDelivery] = useState<{ unit: Unit; tracking_number: string; delivery_date: string; error: string | null; saving: boolean } | null>(null);

  // the label's QR opens ?serial=<serial> (the unit, by its serial)
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const serial = params.get("serial");
    if (serial) setQ(serial);
  }, []);

  const filterStatus = chipStatus || views.active?.filters.status || "";
  const filters: Filters = filterStatus ? { status: filterStatus } : {};

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      // typed or scanned: case, dashes and spaces do not matter, I/L read as 1 and O as 0 (the server checks too)
      const serial = normalizeSerial(q);
      if (serial) params.q = serial;
      if (filterStatus) params.status = filterStatus;
      if (filterProduct) params.product_id = filterProduct;
      setSearchError(null);
      const [uRes, pRes, poRes] = await Promise.all([
        fetchAllPages<Unit>(manufacturingApi.listUnits, params).catch((err) => {
          // e.g. a mistyped generated serial: "This serial's check character does not match: probably a typo"
          setSearchError(apiErrorMessage(err, "The units could not be loaded."));
          return { rows: [] as Unit[], capped: false };
        }),
        fetchAllPages<Product>(manufacturingApi.listProducts),
        fetchAllPages<{ id: string; order_number: string; product?: { name: string } }>(manufacturingApi.listProductionOrders),
      ]);
      setUnits(uRes.rows);
      setProducts(pRes.rows);
      setProductionOrders(poRes.rows);
    } catch { setUnits([]); } finally { setLoading(false); }
  }, [q, filterStatus, filterProduct]);

  useEffect(() => { load(); }, [load]);

  async function register() {
    if (!form.production_order_id || !form.product_id) return;
    setSaving(true);
    setRegisterError(null);
    try {
      const res = await manufacturingApi.registerUnit({
        production_order_id: form.production_order_id,
        product_id: form.product_id,
        hardware_revision: form.hardware_revision || undefined,
        notes: form.notes || undefined,
        is_test: form.is_test,
      });
      setShowRegister(false);
      setForm(EMPTY_FORM);
      setRegistered({ id: res.data.id, serial: res.data.serial_number, is_test: !!res.data.is_test });
      load();
    } catch (err) {
      setRegisterError(apiErrorMessage(err, "The unit could not be registered."));
    } finally { setSaving(false); }
  }

  async function updateStatus(unitId: string, newStatus: string) {
    setUpdatingId(unitId);
    try {
      await manufacturingApi.updateUnitStatus(unitId, { status: newStatus });
      load();
    } catch { } finally { setUpdatingId(null); }
  }

  async function saveSerial() {
    if (!relabel || !relabel.serial.trim()) return;
    setRelabel({ ...relabel, saving: true, error: null });
    try {
      await manufacturingApi.setUnitSerial(relabel.unit.id, relabel.serial.trim());
      setRelabel(null);
      load();
    } catch (err) {
      setRelabel((r) => r && { ...r, saving: false, error: apiErrorMessage(err, "The serial could not be changed.") });
    }
  }

  async function saveDelivery() {
    if (!delivery) return;
    setDelivery({ ...delivery, saving: true, error: null });
    try {
      await manufacturingApi.updateUnitDelivery(delivery.unit.id, {
        tracking_number: delivery.tracking_number.trim() || null,
        delivery_date: delivery.delivery_date || null,
      });
      setDelivery(null);
      load();
    } catch (err) {
      setDelivery((d) => d && { ...d, saving: false, error: apiErrorMessage(err, "The delivery details could not be saved.") });
    }
  }

  const counts: Record<string, number> = {};
  units.forEach(u => { counts[u.status] = (counts[u.status] ?? 0) + 1; });

  const columns = useMemo<ColumnDef<Unit>[]>(() => [
    {
      accessorKey: "serial_number", header: "Serial number",
      cell: (c) => (
        <span className="inline-flex items-center gap-1.5">
          <span className="font-mono text-[13px] font-medium text-fg">{c.getValue<string>()}</span>
          {c.row.original.is_test && <Pill tone="orange">Test</Pill>}
        </span>
      ),
    },
    { id: "product", accessorFn: (u) => u.product?.name ?? "", header: "Product", cell: (c) => c.getValue<string>() || "-" },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="unit" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} /> },
    { accessorKey: "firmware_version", header: "Firmware", meta: { hideOnMobile: true }, cell: (c) => <span className="font-mono text-xs text-fg-muted">{c.getValue<string>() ?? "-"}</span> },
    {
      id: "qc", header: "QC", enableSorting: false, meta: { hideOnMobile: true },
      cell: (c) => {
        const s = c.row.original.status;
        if (s === "qc_passed") return <span className="text-fg">Pass</span>;
        if (s === "qc_failed") return <span className="font-medium text-tone-red">Fail</span>;
        return <span className="text-fg-subtle">-</span>;
      },
    },
    { accessorKey: "shipped_at", header: "Shipped", meta: { numeric: true, hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{fmt(c.getValue<string>())}</span> },
    { accessorKey: "warranty_expiry", header: "Warranty expiry", meta: { numeric: true }, cell: (c) => <span className="text-fg-muted">{fmt(c.getValue<string>())}</span> },
  ], []);

  return (
    <ListFrame>
      <PageHeader flush title="Unit serial registry" summary="Track every hardware unit by serial number through production to delivery"
        actions={canManage ? <Button variant="primary" onClick={() => setShowRegister(true)}><Plus aria-hidden /> Register unit</Button> : undefined} />
      <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChipStatus("")}
        counts={Object.fromEntries(VIEWS.filter((v) => v.filters.status).map((v) => [v.id, counts[v.filters.status!] ?? 0]))} />
      <FilterChips
        chips={CHIP_STATUSES.map((s) => ({ id: s, label: STATUS_LABELS[s], active: chipStatus === s, count: counts[s] ?? 0 }))}
        onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))}
        onClear={() => setChipStatus("")}>
        <Select aria-label="Product" value={filterProduct} onChange={(e) => setFilterProduct(e.target.value)} className="w-48"
          placeholder="All products" options={products.map((p) => ({ value: p.id, label: p.name }))} />
        <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
          value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search serial number" aria-label="Search serial number" className="w-56" />
      </FilterChips>
      {searchError && (
        <p role="alert" className="rounded-lg border border-tone-red/30 bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{searchError}</p>
      )}
      <DataTable label="Units" columns={columns} data={units} getRowId={(r) => r.id} loading={loading}
        empty={{ icon: Cpu, title: "No units registered yet", description: "Register a unit to start tracking it by serial number." }}
        renderDetails={(unit) => {
          const nextStatuses = STATUS_TRANSITIONS[unit.status] ?? [];
          return (
            <div className="space-y-4">
              <KeyValues cols={3} items={[
                { label: "Hardware rev", value: unit.hardware_revision ?? "-" },
                { label: "Software", value: unit.software_version ?? "-" },
                { label: "Flashed", value: fmt(unit.flashed_at) },
                { label: "Delivery date", value: fmt(unit.delivery_date) },
                { label: "Tracking no.", value: unit.tracking_number ?? "-" },
                { label: "Produced", value: fmt(unit.produced_at) },
                { label: "Notes", value: unit.notes ?? "-" },
              ]} />
              <GeneratedSerial serial={unit.serial_number} isTest={unit.is_test} unitId={unit.id} />
              {canManage && (
                <div className="flex flex-wrap items-center gap-2">
                  <Button size="sm" onClick={() => setRelabel({ unit, serial: unit.serial_number, error: null, saving: false })}>
                    <Tag aria-hidden /> Change serial
                  </Button>
                  <Button size="sm" onClick={() => setDelivery({
                    unit, tracking_number: unit.tracking_number ?? "", delivery_date: unit.delivery_date?.slice(0, 10) ?? "",
                    error: null, saving: false,
                  })}>
                    <Truck aria-hidden /> Delivery details
                  </Button>
                </div>
              )}
              {canManage && nextStatuses.length > 0 && (
                <div className="flex flex-wrap items-center gap-2">
                  <span className="mr-1 text-xs font-medium text-fg-muted">Move to:</span>
                  {nextStatuses.map(s => (
                    <Button key={s} size="sm" loading={updatingId === unit.id} onClick={() => updateStatus(unit.id, s)}>
                      {STATUS_LABELS[s] ?? s}
                    </Button>
                  ))}
                </div>
              )}
            </div>
          );
        }} />

      <Dialog open={showRegister} onOpenChange={setShowRegister} title="Register production unit"
        footer={<>
          <Button onClick={() => setShowRegister(false)}>Cancel</Button>
          <Button variant="primary" onClick={register} loading={saving}
            disabled={!form.production_order_id || !form.product_id}>
            {saving ? "Registering..." : "Register unit"}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Production order" required>
            <Select value={form.production_order_id} onChange={e => setForm(f => ({ ...f, production_order_id: e.target.value }))}
              placeholder="Select order" options={productionOrders.map((o) => ({ value: o.id, label: o.product?.name ? `${o.order_number}, ${o.product.name}` : o.order_number }))} />
          </Field>
          <Field label="Product" required>
            <Select value={form.product_id} onChange={e => setForm(f => ({ ...f, product_id: e.target.value }))}
              placeholder="Select product" options={products.map((p) => ({ value: p.id, label: `${p.name} (${p.sku})` }))} />
          </Field>
          <p className="text-xs text-fg-muted">The ERP generates the serial from the product&apos;s serial prefix (e.g. DSK-7K2M9-QXH48).</p>
          <Field label="Hardware revision">
            <Input placeholder="e.g. v1.2" value={form.hardware_revision} onChange={e => setForm(f => ({ ...f, hardware_revision: e.target.value }))} />
          </Field>
          <Field label="Notes">
            <Textarea rows={2} value={form.notes} onChange={e => setForm(f => ({ ...f, notes: e.target.value }))} />
          </Field>
          <Checkbox label="Test unit" hint="Not for a customer: its serial gets the product's prefix plus T (e.g. DSKT-...)."
            checked={form.is_test} onChange={e => setForm(f => ({ ...f, is_test: e.target.checked }))} />
          {registerError && <p role="alert" className="text-sm text-tone-red">{registerError}</p>}
        </div>
      </Dialog>

      <Dialog open={!!relabel} onOpenChange={(o) => { if (!o) setRelabel(null); }} size="sm" title="Change serial"
        footer={<>
          <Button onClick={() => setRelabel(null)}>Cancel</Button>
          <Button variant="primary" onClick={saveSerial} loading={relabel?.saving}
            disabled={!relabel?.serial.trim() || relabel.serial.trim() === relabel.unit.serial_number}>Save serial</Button>
        </>}>
        {relabel && (
          <div className="space-y-3">
            <p className="text-[13px] text-fg-muted">
              For a unit whose label is wrong or damaged. The change is recorded in the audit log; print the new label afterwards.
            </p>
            <Field label="New serial" required hint={`Now ${relabel.unit.serial_number}`}>
              <Input className="font-mono" value={relabel.serial} autoComplete="off"
                onChange={(e) => setRelabel({ ...relabel, serial: e.target.value })} />
            </Field>
            {relabel.error && <p role="alert" className="text-sm text-tone-red">{relabel.error}</p>}
          </div>
        )}
      </Dialog>

      <Dialog open={!!delivery} onOpenChange={(o) => { if (!o) setDelivery(null); }} size="sm" title="Delivery details"
        footer={<>
          <Button onClick={() => setDelivery(null)}>Cancel</Button>
          <Button variant="primary" onClick={saveDelivery} loading={delivery?.saving}>Save</Button>
        </>}>
        {delivery && (
          <div className="space-y-3">
            <Field label="Tracking number">
              <Input value={delivery.tracking_number} maxLength={100} autoComplete="off"
                onChange={(e) => setDelivery({ ...delivery, tracking_number: e.target.value })} />
            </Field>
            <Field label="Delivery date" hint="The day the customer received the unit.">
              <Input type="date" value={delivery.delivery_date}
                onChange={(e) => setDelivery({ ...delivery, delivery_date: e.target.value })} />
            </Field>
            {delivery.error && <p role="alert" className="text-sm text-tone-red">{delivery.error}</p>}
          </div>
        )}
      </Dialog>

      <Dialog open={!!registered} onOpenChange={(o) => { if (!o) setRegistered(null); }} size="sm" title="Unit registered"
        footer={<Button variant="primary" onClick={() => setRegistered(null)}>Done</Button>}>
        {registered && (
          <GeneratedSerial serial={registered.serial} isTest={registered.is_test} unitId={registered.id}
            hint="Generated by the ERP. Print the label and put it on the box." />
        )}
      </Dialog>
    </ListFrame>
  );
}
