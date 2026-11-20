"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { manufacturingApi } from "@/lib/api";
import { ShieldAlert, Plus, Wrench, CheckCircle2 } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, Input, KeyValues, ListFrame, PageHeader, Pill, SavedViewTabs,
  Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface WarrantyRecord {
  id: string;
  serial_number: string;
  product_id: string;
  product?: { name: string; product_code: string };
  customer_name?: string;
  customer_email?: string;
  purchase_date?: string;
  warranty_expiry?: string;
  status: string;
  rma_number?: string;
  issue_description?: string;
  resolution_notes?: string;
  created_at: string;
}

interface HardwareProduct {
  id: string;
  product_code: string;
  name: string;
}

const STATUS_LABELS: Record<string, string> = {
  active: "Active", expired: "Expired", rma_open: "RMA open", rma_closed: "RMA closed", voided: "Voided",
};

const NEXT_STATUS: Record<string, string> = {
  active: "rma_open",
  rma_open: "rma_closed",
};

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "rma_open", label: "RMA open", filters: { status: "rma_open" } },
  { id: "rma_closed", label: "RMA closed", filters: { status: "rma_closed" } },
  { id: "expired", label: "Expired", filters: { status: "expired" } },
];

const EMPTY_FORM = { serial_number: "", product_id: "", customer_name: "", customer_email: "", purchase_date: "", warranty_expiry: "" };
const DAY = 24 * 60 * 60 * 1000;
const fmtDate = (d?: string) => (d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-");

export default function WarrantyPage() {
  const views = useSavedViews<Filters>("manufacturing.warranty", VIEWS);
  const [records, setRecords] = useState<WarrantyRecord[]>([]);
  const [products, setProducts] = useState<HardwareProduct[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [rmaModal, setRmaModal] = useState<WarrantyRecord | null>(null);
  const [form, setForm] = useState(EMPTY_FORM);
  const [rmaForm, setRmaForm] = useState({ issue_description: "", resolution_notes: "" });
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [wRes, pRes] = await Promise.all([
        fetchAllPages<WarrantyRecord>(manufacturingApi.listWarranty),
        fetchAllPages<HardwareProduct>(manufacturingApi.listProducts),
      ]);
      setRecords(wRes.rows);
      setProducts(pRes.rows);
    } catch { setRecords([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function create() {
    if (!form.serial_number.trim() || !form.product_id) return;
    setSaving(true);
    try {
      await manufacturingApi.createWarranty({
        serial_number: form.serial_number.trim(),
        product_id: form.product_id,
        customer_name: form.customer_name.trim() || undefined,
        customer_email: form.customer_email.trim() || undefined,
        purchase_date: form.purchase_date || undefined,
        warranty_expiry: form.warranty_expiry || undefined,
      });
      setShowModal(false);
      setForm(EMPTY_FORM);
      await load();
    } finally { setSaving(false); }
  }

  async function advanceStatus(id: string, nextStatus: string, extras?: Record<string, string>) {
    setSaving(true);
    try {
      await manufacturingApi.updateWarrantyStatus(id, { status: nextStatus, ...extras });
      setRmaModal(null);
      await load();
    } finally { setSaving(false); }
  }

  const now = Date.now();
  const isExpiringSoon = (r: WarrantyRecord) => {
    if (r.status !== "active" || !r.warranty_expiry) return false;
    const diff = new Date(r.warranty_expiry).getTime() - now;
    return diff > 0 && diff < 30 * DAY;
  };
  const expiringSoon = records.filter(isExpiringSoon).length;

  const filterStatus = views.active?.filters.status ?? "";
  const filters: Filters = filterStatus ? { status: filterStatus } : {};
  const filtered = !filterStatus ? records : records.filter((r) => r.status === filterStatus);

  function actions(record: WarrantyRecord) {
    const nextStatus = NEXT_STATUS[record.status];
    if (nextStatus === "rma_open") {
      return (
        <Button size="sm" onClick={() => { setRmaModal(record); setRmaForm({ issue_description: "", resolution_notes: "" }); }}>
          <Wrench aria-hidden /> Open RMA
        </Button>
      );
    }
    if (nextStatus === "rma_closed") {
      return (
        <Button size="sm" disabled={saving} onClick={() => advanceStatus(record.id, "rma_closed", {})}>
          <CheckCircle2 aria-hidden /> Close RMA
        </Button>
      );
    }
    return null;
  }

  const columns = useMemo<ColumnDef<WarrantyRecord>[]>(() => [
    {
      accessorKey: "serial_number", header: "Serial",
      cell: (c) => <span className="font-mono text-[13px] font-medium text-fg">{c.getValue<string>()}</span>,
    },
    { id: "product", accessorFn: (r) => r.product?.name ?? "", header: "Product", cell: (c) => c.getValue<string>() || "-" },
    { accessorKey: "customer_name", header: "Customer", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<string>() ?? "-"}</span> },
    {
      accessorKey: "warranty_expiry", header: "Expires", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => isExpiringSoon(c.row.original)
        ? <span className="font-medium text-tone-orange">{fmtDate(c.getValue<string>())}</span>
        : <span className="text-fg-muted">{fmtDate(c.getValue<string>())}</span>,
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => {
        const r = c.row.original;
        return (
          <div className="flex flex-wrap items-center gap-1.5">
            <StatusPill family="warranty" status={r.status} label={STATUS_LABELS[r.status]} />
            {r.rma_number && <Pill tone="orange" className="font-mono">{r.rma_number}</Pill>}
            {isExpiringSoon(r) && <Pill tone="orange">Expiring soon</Pill>}
          </div>
        );
      },
    },
    { id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" }, cell: (c) => actions(c.row.original) },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [saving]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={records.length} />
        <Stat label="Active" value={records.filter((r) => r.status === "active").length} />
        <Stat label="RMA open" value={records.filter((r) => r.status === "rma_open").length} />
        <Stat label="Expiring" value={expiringSoon} note={expiringSoon > 0 ? "within 30 days" : undefined} alert={expiringSoon > 0} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Warranty & RMA" summary="Hardware warranty tracking and return merchandise authorizations"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Register warranty</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} />
        <DataTable label="Warranty records" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: ShieldAlert, title: "No warranty records", description: "Register a warranty when a unit ships." }}
          renderDetails={(r) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Product code", value: r.product?.product_code ?? "-" },
                { label: "Customer email", value: r.customer_email ?? "-" },
                { label: "Purchased", value: fmtDate(r.purchase_date) },
                { label: "RMA number", value: r.rma_number ?? "-" },
                { label: "Issue", value: r.issue_description ?? "-" },
                { label: "Resolution", value: r.resolution_notes ?? "-" },
              ]} />
              {actions(r)}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Register warranty"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving} disabled={!form.serial_number.trim() || !form.product_id}>
            {saving ? "Registering..." : "Register"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Serial number" required>
            <Input value={form.serial_number} onChange={(e) => setForm({ ...form, serial_number: e.target.value })} placeholder="e.g. DSK-2026-0001" className="font-mono" />
          </Field>
          <Field label="Product" required>
            <Select value={form.product_id} onChange={(e) => setForm({ ...form, product_id: e.target.value })} placeholder="Select a product"
              options={products.map((p) => ({ value: p.id, label: p.name }))} />
          </Field>
          <Field label="Customer name">
            <Input value={form.customer_name} onChange={(e) => setForm({ ...form, customer_name: e.target.value })} />
          </Field>
          <Field label="Customer email">
            <Input type="email" value={form.customer_email} onChange={(e) => setForm({ ...form, customer_email: e.target.value })} />
          </Field>
          <Field label="Purchase date">
            <Input type="date" value={form.purchase_date} onChange={(e) => setForm({ ...form, purchase_date: e.target.value })} />
          </Field>
          <Field label="Warranty expiry">
            <Input type="date" value={form.warranty_expiry} onChange={(e) => setForm({ ...form, warranty_expiry: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={!!rmaModal} onOpenChange={(o) => { if (!o) setRmaModal(null); }} size="sm" title="Open RMA"
        description={rmaModal?.serial_number}
        footer={<>
          <Button onClick={() => setRmaModal(null)}>Cancel</Button>
          <Button variant="primary" loading={saving} disabled={!rmaForm.issue_description.trim()}
            onClick={() => rmaModal && advanceStatus(rmaModal.id, "rma_open", { issue_description: rmaForm.issue_description })}>
            {saving ? "Opening..." : "Open RMA"}
          </Button>
        </>}>
        <Field label="Issue description" required>
          <Textarea value={rmaForm.issue_description} onChange={(e) => setRmaForm({ ...rmaForm, issue_description: e.target.value })}
            rows={3} placeholder="Describe the issue reported by the customer" />
        </Field>
      </Dialog>
    </div>
  );
}
