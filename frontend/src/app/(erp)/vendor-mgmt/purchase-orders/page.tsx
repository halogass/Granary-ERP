"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { vendorApi } from "@/lib/api";
import { ShoppingCart, Plus, Search, Trash2, Send, X } from "lucide-react";
import { useSendDocument } from "@/components/approvals/useSendDocument";
import { vendorSource } from "@/lib/entitySources";
import { formatCurrency, sumByCurrency } from "@/lib/currency";
import { useCurrencySettings, currencyOptions } from "@/hooks/useCurrencySettings";
import ThreeWayMatch from "@/components/vendor/ThreeWayMatch";
import {
  Button, Combobox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame,
  PageHeader, SavedViewTabs, Select, Stat, StatGroup, StatusPill, Tabs, TabsContent, TabsList, TabsTrigger, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

// Shape of GET /vendor/purchase-orders.
interface PurchaseOrder {
  id: string;
  number: string;
  vendor_id: string;
  vendor_name?: string | null;
  status: string;
  total: number;
  currency: string;
  expected_delivery?: string | null;
  created_at: string;
}

const STATUS_LABELS: Record<string, string> = {
  draft: "Draft", sent: "Sent", approved: "Approved", confirmed: "Confirmed", received: "Received",
  partial: "Partial", partially_received: "Partial", cancelled: "Cancelled",
};

const AWAITING = ["sent", "confirmed", "partially_received"];

type Filters = { group?: "draft" | "awaiting" | "received" | "cancelled" };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Drafts", filters: { group: "draft" } },
  { id: "awaiting", label: "Awaiting delivery", filters: { group: "awaiting" } },
  { id: "received", label: "Received", filters: { group: "received" } },
];

const fmtDate = (d?: string | null, withYear = true) =>
  d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", ...(withYear ? { year: "numeric" } : {}) }) : "-";

function inGroup(o: PurchaseOrder, group?: Filters["group"]) {
  if (!group) return true;
  if (group === "awaiting") return AWAITING.includes(o.status);
  return o.status === group;
}

export default function PurchaseOrdersPage() {
  const views = useSavedViews<Filters>("vendor.purchase-orders", VIEWS);
  const [chips, setChips] = useState<Record<string, boolean>>({});
  const [search, setSearch] = useState("");
  const [orders, setOrders] = useState<PurchaseOrder[]>([]);
  const [vendors, setVendors] = useState<{ id: string; name: string }[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const { functional: functionalCurrency, active: activeCurrencies } = useCurrencySettings();
  const [form, setForm] = useState({ vendor_id: "", currency: "", expected_delivery: "", notes: "" });
  const [lineItems, setLineItems] = useState([{ description: "", qty: "1", unit_price: "" }]);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [poRes, vRes] = await Promise.all([
        fetchAllPages<PurchaseOrder>(vendorApi.listPOs),
        fetchAllPages<{ id: string; name: string }>(vendorApi.listVendors),
      ]);
      setOrders(poRes.rows);
      setVendors(vRes.rows);
    } catch { setOrders([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  // Send the approved purchase order (QR-verified PDF) to the vendor; an unapproved one is submitted first.
  const sender = useSendDocument({
    entityType: "purchase_order", label: "Purchase order", onChanged: load,
    onMessage: (kind, message) =>
      kind === "success" ? toast.success(message) : kind === "info" ? toast.info(message) : toast.error(message),
  });

  const vendorName = useCallback(
    (o: PurchaseOrder) => o.vendor_name ?? vendors.find((v) => v.id === o.vendor_id)?.name ?? "-",
    [vendors],
  );

  async function deletePO(id: string) {
    try {
      await vendorApi.deletePO(id);
      setConfirmDeleteId(null);
      setOrders((prev) => prev.filter((o) => o.id !== id));
    } catch { /* keep */ }
  }

  // KPIs count committed orders only: a draft isn't spend yet and a cancelled one never will be.
  // POs carry their own currency, so spend is grouped by currency rather than added up.
  const committed = orders.filter((o) => !["draft", "cancelled"].includes(o.status));
  const spend = sumByCurrency(committed, (o) => o.total, (o) => o.currency, "USD");
  const awaitingCount = committed.filter((o) => AWAITING.includes(o.status)).length;
  const receivedCount = committed.filter((o) => o.status === "received").length;

  const group = chips.cancelled ? "cancelled" : views.active?.filters.group;
  const filters: Filters = group ? { group } : {};
  const q = search.trim().toLowerCase();
  const rows = orders.filter((o) => inGroup(o, group) &&
    (!q || o.number.toLowerCase().includes(q) || vendorName(o).toLowerCase().includes(q)));

  function addLine() { setLineItems([...lineItems, { description: "", qty: "1", unit_price: "" }]); }
  function removeLine(i: number) { setLineItems(lineItems.filter((_, idx) => idx !== i)); }
  function updateLine(i: number, field: string, val: string) {
    setLineItems(lineItems.map((l, idx) => idx === i ? { ...l, [field]: val } : l));
  }

  const subtotal = lineItems.reduce((s, l) => s + (parseFloat(l.qty || "0") * parseFloat(l.unit_price || "0")), 0);

  async function create() {
    if (!form.vendor_id || lineItems.every((l) => !l.description.trim())) return;
    setSaving(true);
    try {
      await vendorApi.createPO({
        vendor_id: form.vendor_id,
        currency: form.currency,
        expected_delivery: form.expected_delivery || undefined,
        notes: form.notes.trim() || undefined,
        items: lineItems.filter((l) => l.description.trim()).map((l) => ({
          description: l.description.trim(),
          quantity: parseFloat(l.qty) || 1,
          unit_price: parseFloat(l.unit_price) || 0,
        })),
      });
      setShowModal(false);
      setForm({ vendor_id: "", currency: functionalCurrency, expected_delivery: "", notes: "" });
      setLineItems([{ description: "", qty: "1", unit_price: "" }]);
      await load();
    } finally { setSaving(false); }
  }

  const columns = useMemo<ColumnDef<PurchaseOrder>[]>(() => [
    {
      accessorKey: "number", header: "PO",
      cell: (c) => <span className="font-mono text-[13px] font-medium">{c.getValue<string>()}</span>,
    },
    {
      id: "vendor", accessorFn: (o) => vendorName(o), header: "Vendor",
      cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span>,
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="purchase" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} />,
    },
    {
      accessorKey: "created_at", header: "Created", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{fmtDate(c.getValue<string>())}</span>,
    },
    {
      accessorKey: "expected_delivery", header: "Delivery", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{fmtDate(c.getValue<string | null>(), false)}</span>,
    },
    {
      accessorKey: "total", header: "Total", meta: { numeric: true },
      cell: (c) => <span className="font-medium">{formatCurrency(c.row.original.total ?? 0, c.row.original.currency)}</span>,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const order = c.row.original;
        const canDelete = ["draft", "cancelled"].includes(order.status);
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            {order.status !== "cancelled" && (
              <Button size="sm" onClick={() => sender.start(order.id)} loading={sender.preparing === order.id}
                title="Send the approved PO to the vendor (email or WhatsApp)">
                <Send aria-hidden /> Send
              </Button>
            )}
            {canDelete && (confirmDeleteId === order.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deletePO(order.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </>
            ) : (
              <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${order.number}`} onClick={() => setConfirmDeleteId(order.id)} />
            ))}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, sender.preparing, vendorName]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Committed" value={committed.length} note="sent or later" />
        <Stat label="Awaiting delivery" value={awaitingCount} />
        <Stat label="Received" value={receivedCount} />
        <Stat label="Total spend" value={formatCurrency(spend.total, spend.currency)}
          note={spend.mixed ? `Mixed currencies: ${spend.currency} total shown` : undefined} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Purchase orders"
          summary={`${orders.length} order${orders.length !== 1 ? "s" : ""}, ${awaitingCount} awaiting delivery`}
          actions={<Button variant="primary" onClick={() => { setForm((f) => ({ ...f, currency: f.currency || functionalCurrency })); setShowModal(true); }}><Plus aria-hidden /> New PO</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChips({})} />
        <FilterChips chips={[{ id: "cancelled", label: "Cancelled", active: !!chips.cancelled }]}
          onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id] }))} onClear={() => setChips({})}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search orders" aria-label="Search purchase orders" className="w-56" />
        </FilterChips>
        <DataTable label="Purchase orders" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: ShoppingCart, title: "No purchase orders", description: "Pick another view, or create a PO." }}
          renderDetails={(o) => (
            <Tabs defaultValue="details">
              <TabsList aria-label={`Purchase order ${o.number}`}>
                <TabsTrigger value="details">Details</TabsTrigger>
                <TabsTrigger value="matching">Matching</TabsTrigger>
              </TabsList>
              <TabsContent value="details">
                <KeyValues cols={3} items={[
                  { label: "Vendor", value: vendorName(o) },
                  { label: "Number", value: <span className="font-mono">{o.number}</span> },
                  { label: "Status", value: STATUS_LABELS[o.status] ?? o.status },
                  { label: "Created", value: fmtDate(o.created_at) },
                  { label: "Expected delivery", value: fmtDate(o.expected_delivery) },
                  { label: "Total", value: formatCurrency(o.total ?? 0, o.currency), numeric: true },
                ]} />
              </TabsContent>
              <TabsContent value="matching">
                <ThreeWayMatch poId={o.id} />
              </TabsContent>
            </Tabs>
          )} />
      </ListFrame>

      {sender.modal}
      <Dialog open={showModal} onOpenChange={setShowModal} title="New purchase order" size="lg"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!form.vendor_id} loading={saving}>{saving ? "Creating" : "Create PO"}</Button>
        </>}>
        <div className="space-y-4">
          <FieldGrid cols={3}>
            <Field label="Vendor" required className="md:col-span-2">
              <Combobox value={form.vendor_id} source={vendorSource}
                placeholder="Select vendor" emptyLabel="No vendors found"
                onChange={(id) => setForm({ ...form, vendor_id: id })} />
            </Field>
            <Field label="Currency">
              <Select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}
                options={currencyOptions(activeCurrencies, form.currency)} />
            </Field>
          </FieldGrid>

          <div>
            <div className="mb-2 flex items-center justify-between">
              <span className="text-sm font-medium text-fg">Items <span className="text-brand-ink">*</span></span>
              <Button size="sm" variant="ghost" onClick={addLine}><Plus aria-hidden /> Add line</Button>
            </div>
            <div className="space-y-2">
              {lineItems.map((line, i) => (
                <div key={i} className="grid grid-cols-[1fr_72px_120px_32px] items-center gap-2 max-md:grid-cols-[1fr_1fr_auto] max-md:rounded-lg max-md:border max-md:border-line max-md:p-3">
                  <Input value={line.description} onChange={(e) => updateLine(i, "description", e.target.value)}
                    placeholder="Description" aria-label={`Line ${i + 1} description`} className="max-md:col-span-3" />
                  <div>
                    <span className="mb-1 block text-xs text-fg-muted md:hidden" aria-hidden>Qty</span>
                    <Input numeric type="number" min="0" step="any" value={line.qty} onChange={(e) => updateLine(i, "qty", e.target.value)}
                      placeholder="Qty" aria-label={`Line ${i + 1} quantity`} />
                  </div>
                  <div>
                    <span className="mb-1 block text-xs text-fg-muted md:hidden" aria-hidden>Unit price</span>
                    <Input numeric type="number" min="0" step="any" value={line.unit_price} onChange={(e) => updateLine(i, "unit_price", e.target.value)}
                      placeholder="Unit price" aria-label={`Line ${i + 1} unit price`} />
                  </div>
                  {lineItems.length > 1
                    ? <IconButton size="sm" variant="ghost" icon={X} label={`Remove line ${i + 1}`} onClick={() => removeLine(i)} className="max-md:self-end" />
                    : <span />}
                </div>
              ))}
            </div>
            <p className="mt-2 text-right text-sm font-semibold text-fg tabular">Subtotal: {form.currency} {subtotal.toLocaleString()}</p>
          </div>

          <Field label="Expected delivery">
            <Input type="date" value={form.expected_delivery} onChange={(e) => setForm({ ...form, expected_delivery: e.target.value })} />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
