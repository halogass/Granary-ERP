"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { vendorApi } from "@/lib/api";
import { ClipboardList, Plus, Search, Trash2, Send } from "lucide-react";
import { useSendDocument } from "@/components/approvals/useSendDocument";
import { extractApiError } from "@/lib/apiError";
import { vendorSource } from "@/lib/entitySources";
import { formatCurrency, sumByCurrency } from "@/lib/currency";
import {
  Button, Combobox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame,
  PageHeader, SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, toast, useSavedViews,
  type ColumnDef, type SavedView,
} from "@/components/ui";

// Shape of GET /vendor/work-orders.
interface WorkOrder {
  id: string;
  number: string;
  vendor_id: string;
  title: string;
  status: string;
  agreed_value?: number | null;
  currency: string;
  start_date?: string | null;
  end_date?: string | null;
}

// Matches models/vendor.py WorkOrder.status: draft | active | paused | completed | cancelled
const STATUS_LABELS: Record<string, string> = {
  draft: "Draft", active: "Active", paused: "Paused", completed: "Completed", cancelled: "Cancelled",
};

const TRANSITIONS: Record<string, { to: string; label: string }[]> = {
  draft:  [{ to: "active", label: "Activate" }, { to: "cancelled", label: "Cancel" }],
  active: [{ to: "completed", label: "Complete" }, { to: "paused", label: "Pause" }, { to: "cancelled", label: "Cancel" }],
  paused: [{ to: "active", label: "Resume" }, { to: "cancelled", label: "Cancel" }],
};

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "completed", label: "Completed", filters: { status: "completed" } },
];

const fmtDate = (d?: string | null) =>
  d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-";

export default function WorkOrdersPage() {
  const views = useSavedViews<Filters>("vendor.work-orders", VIEWS);
  const [chips, setChips] = useState<Record<string, boolean>>({});
  const [search, setSearch] = useState("");
  const [orders, setOrders] = useState<WorkOrder[]>([]);
  const [vendors, setVendors] = useState<{ id: string; name: string }[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState({ vendor_id: "", title: "", scope: "", agreed_value: "", currency: "USD", start_date: "", end_date: "" });
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [woRes, vRes] = await Promise.all([
        fetchAllPages<WorkOrder>(vendorApi.listWorkOrders),
        fetchAllPages<{ id: string; name: string }>(vendorApi.listVendors),
      ]);
      setOrders(woRes.rows);
      setVendors(vRes.rows);
    } catch { setOrders([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  // Send the approved work order (QR-verified PDF) to the vendor; an unapproved one is submitted first.
  const sender = useSendDocument({
    entityType: "work_order", label: "Work order", onChanged: load,
    onMessage: (kind, message) =>
      kind === "success" ? toast.success(message) : kind === "info" ? toast.info(message) : toast.error(message),
  });

  // The list endpoint returns only vendor_id, so resolve the display name locally.
  const vendorName = useCallback((id: string) => vendors.find((v) => v.id === id)?.name ?? "-", [vendors]);

  async function create() {
    if (!form.vendor_id || !form.title.trim()) return;
    setSaving(true);
    try {
      await vendorApi.createWorkOrder({
        vendor_id: form.vendor_id,
        title: form.title.trim(),
        scope: form.scope.trim() || undefined,
        agreed_value: form.agreed_value ? parseFloat(form.agreed_value) : undefined,
        currency: form.currency,
        start_date: form.start_date || undefined,
        end_date: form.end_date || undefined,
      });
      setShowModal(false);
      setForm({ vendor_id: "", title: "", scope: "", agreed_value: "", currency: "USD", start_date: "", end_date: "" });
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to create work order"));
    } finally { setSaving(false); }
  }

  async function deleteOrder(id: string) {
    try {
      await vendorApi.deleteWorkOrder(id);
      setConfirmDeleteId(null);
      setOrders((prev) => prev.filter((o) => o.id !== id));
    } catch { /* keep */ }
  }

  async function updateStatus(id: string, status: string) {
    try {
      await vendorApi.updateWorkOrderStatus(id, { status });
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to update status"));
    }
  }

  const inProgress = orders.filter((o) => o.status === "active").length;
  const completed = orders.filter((o) => o.status === "completed").length;
  // Work orders carry their own currency: the value is grouped by currency (cancelled ones left out).
  const totalValue = sumByCurrency(orders.filter((o) => o.status !== "cancelled"), (o) => o.agreed_value, (o) => o.currency, "USD");

  const status = chips.paused ? "paused" : chips.cancelled ? "cancelled" : views.active?.filters.status;
  const q = search.trim().toLowerCase();
  const rows = orders.filter((o) => (!status || o.status === status) &&
    (!q || o.title.toLowerCase().includes(q) || o.number.toLowerCase().includes(q) || vendorName(o.vendor_id).toLowerCase().includes(q)));

  const columns = useMemo<ColumnDef<WorkOrder>[]>(() => [
    {
      accessorKey: "title", header: "Work order",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.row.original.title}</p>
          <p className="font-mono text-xs text-fg-muted">{c.row.original.number}</p>
        </div>
      ),
    },
    { id: "vendor", accessorFn: (o) => vendorName(o.vendor_id), header: "Vendor", meta: { hideOnMobile: true } },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="work_order" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} />,
    },
    {
      accessorKey: "end_date", header: "Due", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{fmtDate(c.getValue<string | null>())}</span>,
    },
    {
      accessorKey: "agreed_value", header: "Value", meta: { numeric: true },
      cell: (c) => c.row.original.agreed_value != null ? formatCurrency(c.row.original.agreed_value, c.row.original.currency) : "-",
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
                title="Send the approved work order to the vendor (email or WhatsApp)">
                <Send aria-hidden /> Send
              </Button>
            )}
            {canDelete && (confirmDeleteId === order.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deleteOrder(order.id)}>Yes</Button>
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
        <Stat label="Total" value={orders.length} />
        <Stat label="Active" value={inProgress} />
        <Stat label="Completed" value={completed} />
        <Stat label="Total value" value={formatCurrency(totalValue.total, totalValue.currency)}
          note={totalValue.mixed ? `Mixed currencies: ${totalValue.currency} total shown` : undefined} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Work orders"
          summary={`${orders.length} work order${orders.length !== 1 ? "s" : ""}, ${inProgress} active. Contractor and vendor work assignments.`}
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New work order</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} onSelect={() => setChips({})} />
        <FilterChips
          chips={[{ id: "paused", label: "Paused", active: !!chips.paused }, { id: "cancelled", label: "Cancelled", active: !!chips.cancelled }]}
          onToggle={(id) => setChips((c) => ({ [id]: !c[id] }))} onClear={() => setChips({})}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search work orders" aria-label="Search work orders" className="w-56" />
        </FilterChips>
        <DataTable label="Work orders" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: ClipboardList, title: "No work orders", description: "Pick another view, or create a work order." }}
          renderDetails={(o) => {
            const transitions = TRANSITIONS[o.status] ?? [];
            return (
              <div className="flex flex-wrap items-start gap-6">
                <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                  { label: "Vendor", value: vendorName(o.vendor_id) },
                  { label: "Start", value: fmtDate(o.start_date) },
                  { label: "End", value: fmtDate(o.end_date) },
                  { label: "Agreed value", value: o.agreed_value != null ? formatCurrency(o.agreed_value, o.currency) : "-", numeric: true },
                ]} />
                {transitions.length > 0 && (
                  <div className="flex flex-wrap gap-2">
                    {transitions.map((t) => (
                      <Button key={t.to} size="sm" variant={t.to === "cancelled" ? "danger" : "secondary"}
                        onClick={() => updateStatus(o.id, t.to)}>{t.label}</Button>
                    ))}
                  </div>
                )}
              </div>
            );
          }} />
      </ListFrame>

      {sender.modal}
      <Dialog open={showModal} onOpenChange={setShowModal} title="New work order"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!form.vendor_id || !form.title.trim()} loading={saving}>
            {saving ? "Creating" : "Create work order"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Vendor" required className="md:col-span-2">
            <Combobox value={form.vendor_id} source={vendorSource}
              placeholder="Select vendor" emptyLabel="No vendors found"
              onChange={(id) => setForm({ ...form, vendor_id: id })} />
          </Field>
          <Field label="Title" required className="md:col-span-2">
            <Input value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} placeholder="e.g. Build mobile dashboard UI" />
          </Field>
          <Field label="Scope" className="md:col-span-2">
            <Textarea value={form.scope} onChange={(e) => setForm({ ...form, scope: e.target.value })} rows={3} />
          </Field>
          <Field label="Agreed value">
            <Input numeric type="number" min="0" value={form.agreed_value} onChange={(e) => setForm({ ...form, agreed_value: e.target.value })} />
          </Field>
          <Field label="Currency">
            <Select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}
              options={["USD", "EUR", "GBP", "JPY"].map((c) => ({ value: c, label: c }))} />
          </Field>
          <Field label="Start date">
            <Input type="date" value={form.start_date} onChange={(e) => setForm({ ...form, start_date: e.target.value })} />
          </Field>
          <Field label="End date">
            <Input type="date" value={form.end_date} onChange={(e) => setForm({ ...form, end_date: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
