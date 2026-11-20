"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState, useCallback, useMemo } from "react";
import { localSource } from "@/lib/entitySources";
import { financeApi, salesApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { CalendarClock, Plus, Trash2, Search, FileText } from "lucide-react";
import Link from "next/link";
import { usePermissions } from "@/hooks/usePermissions";
import {
  Button, Combobox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame,
  PageHeader, SavedViewTabs, Stat, StatGroup, StatusPill, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { formatCurrency } from "@/lib/currency";

/** One total per currency ("USD 1,000.00 · JPY 150,000"): amounts in different currencies can't be added. */
function perCurrency<T>(rows: T[], amount: (r: T) => number | null | undefined, currency: (r: T) => string | null | undefined): string {
  const totals = new Map<string, number>();
  for (const r of rows) {
    const v = amount(r) ?? 0;
    if (!v) continue;
    const code = (currency(r) || "").toUpperCase();
    totals.set(code, (totals.get(code) ?? 0) + v);
  }
  if (totals.size === 0) return "0";
  return [...totals.entries()].map(([code, total]) => (code ? formatCurrency(total, code) : total.toLocaleString())).join(" · ");
}

// Shape of GET /finance/payment-schedules.
interface PaymentSchedule {
  id: string;
  name: string;
  sequence: number;
  percentage: number | null;
  amount: number | null;
  currency: string;
  due_trigger: string;
  due_date: string | null;
  status: string;
  quotation_id?: string | null;
  quotation_number?: string | null;
  invoice_id?: string | null;
  proforma_invoice_id?: string | null;
}

interface Quotation {
  id: string;
  number: string;
  full_number?: string | null;
  subject?: string | null;
  total: number | null;
  currency: string;
}

// Milestones without a due date sort last and never count as overdue/due-soon.
const dueTime = (s: PaymentSchedule) => (s.due_date ? new Date(s.due_date).getTime() : Number.POSITIVE_INFINITY);

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "pending", label: "Pending", filters: { status: "pending" } },
  { id: "overdue", label: "Overdue", filters: { status: "overdue" } },
  { id: "paid", label: "Paid", filters: { status: "paid" } },
];

const fmtDue = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

export default function PaymentSchedulesPage() {
  const [schedules, setSchedules] = useState<PaymentSchedule[]>([]);
  const [quotations, setQuotations] = useState<Quotation[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState({ quotation_id: "", name: "", amount: "", due_date: "" });
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [raisingId, setRaisingId] = useState<string | null>(null);
  const { holds } = usePermissions();
  // Adding, deleting and raising a proforma all need finance:create_invoice on the API.
  const canInvoice = holds("finance:create_invoice");
  const views = useSavedViews<Filters>("finance.schedules", VIEWS);
  const [chips, setChips] = useState<Record<string, boolean>>({});
  const [q, setQ] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [schRes, qRes] = await Promise.all([
        fetchAllPages<PaymentSchedule>(financeApi.listSchedules),
        fetchAllPages<Quotation>(salesApi.listQuotations).catch(() => ({ rows: [] as Quotation[] })),
      ]);
      const raw = schRes.rows;
      // Auto-tag overdue
      const now = Date.now();
      setSchedules(raw.map((s: PaymentSchedule) => ({
        ...s,
        status: s.status === "pending" && dueTime(s) < now ? "overdue" : s.status,
      })));
      setQuotations(qRes.rows);
    } catch { setSchedules([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const quotationOptions = useMemo(() => localSource(quotations.map((q) => ({
    id: q.id,
    label: `${q.full_number ?? q.number}${q.subject ? `, ${q.subject}` : ""}`,
    sublabel: q.total != null ? `${q.total.toLocaleString()} ${q.currency}` : q.currency,
  }))), [quotations]);

  async function deleteSchedule(id: string) {
    try {
      await financeApi.deleteSchedule(id);
      setConfirmDeleteId(null);
      setSchedules((prev) => prev.filter((s) => s.id !== id));
    } catch { /* keep */ }
  }

  async function raiseProforma(s: PaymentSchedule) {
    setRaisingId(s.id);
    try {
      const res = await financeApi.raiseProforma(s.id);
      const number = res.data?.number;
      toast.success(number ? `Proforma ${number} raised` : "Proforma raised");
      setSchedules((prev) => prev.map((x) => (x.id === s.id ? { ...x, proforma_invoice_id: res.data?.id ?? "raised" } : x)));
      load();
    } catch (err) {
      toast.error(extractApiError(err, "The proforma could not be raised"));
    } finally { setRaisingId(null); }
  }

  async function create() {
    if (!form.quotation_id || !form.name.trim() || !form.amount || !form.due_date) return;
    setSaving(true);
    try {
      const quotation = quotations.find((q) => q.id === form.quotation_id);
      const sequence = schedules.filter((s) => s.quotation_id === form.quotation_id).length + 1;
      await financeApi.createSchedule({
        quotation_id: form.quotation_id,
        currency: quotation?.currency ?? "USD",
        milestones: [{
          name: form.name.trim(),
          amount: parseFloat(form.amount),
          due_date: form.due_date,
          sequence,
          due_trigger: "on_date",
        }],
      });
      setShowModal(false);
      setForm({ quotation_id: "", name: "", amount: "", due_date: "" });
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to add milestone"));
    } finally { setSaving(false); }
  }

  const now = new Date();
  const overdueItems = schedules.filter((s) => s.status === "overdue");
  const dueSoon = schedules.filter((s) => {
    if (s.status !== "pending") return false;
    const diff = dueTime(s) - now.getTime();
    return diff > 0 && diff < 14 * 86400000;
  });
  const totalPending = perCurrency(schedules.filter((s) => ["pending", "overdue"].includes(s.status)), (r) => r.amount, (r) => r.currency);
  const totalPaid = perCurrency(schedules.filter((s) => s.status === "paid"), (r) => r.amount, (r) => r.currency);

  const selectedQuotation = quotations.find((q) => q.id === form.quotation_id);

  const statusFilter = views.active?.filters.status;
  const term = q.trim().toLowerCase();
  const rows = [...schedules]
    .sort((a, b) => dueTime(a) - dueTime(b))
    .filter((s) => !statusFilter || s.status === statusFilter)
    .filter((s) => !chips.due_soon || dueSoon.includes(s))
    .filter((s) => !term || s.name.toLowerCase().includes(term) || (s.quotation_number ?? "").toLowerCase().includes(term));
  const viewCounts = {
    all: schedules.length,
    pending: schedules.filter((s) => s.status === "pending").length,
    overdue: overdueItems.length,
    paid: schedules.filter((s) => s.status === "paid").length,
  };

  const columns: ColumnDef<PaymentSchedule>[] = [
    { accessorKey: "name", header: "Milestone", cell: (c) => {
      const s = c.row.original;
      return (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{s.name}</p>
          <p className="truncate text-xs text-fg-muted">
            <span className="font-mono">{s.quotation_number ?? "-"}</span>
            {s.percentage != null && `, ${s.percentage}%`}
          </p>
        </div>
      );
    } },
    { id: "due", header: "Due date", accessorFn: (s) => dueTime(s), cell: (c) => {
      const s = c.row.original;
      const daysUntil = s.due_date ? Math.floor((dueTime(s) - now.getTime()) / 86400000) : null;
      return (
        <div className="tabular">
          {s.due_date ? fmtDue(s.due_date) : <span className="capitalize text-fg-muted">{s.due_trigger.replace(/_/g, " ")}</span>}
          {s.status === "pending" && daysUntil !== null && daysUntil >= 0 && daysUntil <= 14 && (
            <p className="text-xs font-medium text-tone-orange">in {daysUntil}d</p>
          )}
          {s.status === "overdue" && daysUntil !== null && (
            <p className="text-xs font-medium text-tone-red">{Math.abs(daysUntil)}d overdue</p>
          )}
        </div>
      );
    } },
    { accessorKey: "amount", header: "Amount", meta: { numeric: true }, cell: (c) => {
      const s = c.row.original;
      return <span className="font-medium">{s.amount != null ? s.amount.toLocaleString() : "-"} <span className="text-xs font-normal text-fg-muted">{s.currency}</span></span>;
    } },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="schedule" status={c.getValue<string>()} /> },
    { id: "actions", header: () => <span className="sr-only">Actions</span>, enableSorting: false, meta: { align: "right", width: "220px" },
      cell: (c) => {
        const s = c.row.original;
        if (!canInvoice) return null;
        const open = s.status === "pending" || s.status === "overdue";
        const raise = open && !s.proforma_invoice_id && !s.invoice_id ? (
          <Button size="sm" loading={raisingId === s.id} onClick={() => raiseProforma(s)}>
            <FileText aria-hidden /> Raise proforma
          </Button>
        ) : null;
        // Only a pending milestone can be deleted (the API refuses the rest); an overdue one is pending there.
        if (!open) return null;
        return confirmDeleteId === s.id ? (
          <div className="flex items-center justify-end gap-1.5">
            <span className="text-xs font-medium text-tone-red">Delete?</span>
            <Button size="sm" variant="danger" onClick={() => deleteSchedule(s.id)}>Yes</Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
          </div>
        ) : (
          <div className="flex items-center justify-end gap-1">
            {raise}
            <IconButton size="sm" label="Delete milestone" icon={Trash2} onClick={() => setConfirmDeleteId(s.id)} />
          </div>
        );
      } },
  ];

  const summary = [
    overdueItems.length > 0 ? `${overdueItems.length} milestone${overdueItems.length > 1 ? "s" : ""} overdue` : null,
    dueSoon.length > 0 ? `${dueSoon.length} due within 14 days` : null,
  ].filter(Boolean).join(", ") || "Instalment milestones and phased payment tracking";

  return (
    <div className="animate-fade-up space-y-4">
      <PageHeader title="Payment schedules" summary={summary} className="mb-0"
        actions={canInvoice ? <Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Add milestone</Button> : undefined} />

      <StatGroup>
        <Stat label="Total" value={schedules.length} />
        <Stat label="Overdue" value={overdueItems.length} alert={overdueItems.length > 0}
          note={overdueItems.length > 0 ? "Needs follow-up" : undefined} />
        <Stat label="Pending" value={<span className="break-words text-base">{totalPending}</span>}
          note={dueSoon.length > 0 ? `${dueSoon.length} due within 14 days` : undefined} />
        <Stat label="Collected" value={<span className="break-words text-base">{totalPaid}</span>} />
      </StatGroup>

      <ListFrame>
        <SavedViewTabs state={views} counts={viewCounts} alertIds={overdueItems.length > 0 ? ["overdue"] : []} />
        <FilterChips
          chips={[{ id: "due_soon", label: "Due within 14 days", active: !!chips.due_soon, count: dueSoon.length }]}
          onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id] }))} onClear={() => setChips({})}>
          <Input type="search" autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other" leading={Search} value={q} onChange={(e) => setQ(e.target.value)}
            placeholder="Search milestones" aria-label="Search milestones" className="w-56" />
        </FilterChips>
        <DataTable label="Payment milestones" columns={columns} data={rows} getRowId={(s) => s.id} loading={loading}
          empty={{ icon: CalendarClock, title: "No payment schedules", description: "Add milestones to break quotations into phased payments." }}
          renderDetails={(s) => (
            <KeyValues cols={3} items={[
              { label: "Quotation", value: <span className="font-mono">{s.quotation_number ?? "-"}</span> },
              { label: "Sequence", value: s.sequence, numeric: true },
              { label: "Share", value: s.percentage != null ? `${s.percentage}%` : "-", numeric: true },
              { label: "Due trigger", value: <span className="capitalize">{s.due_trigger.replace(/_/g, " ")}</span> },
              { label: "Due date", value: s.due_date ? fmtDue(s.due_date) : "-", numeric: true },
              { label: "Amount", value: s.amount != null ? `${s.amount.toLocaleString()} ${s.currency}` : "-", numeric: true },
              { label: "Invoiced", value: s.proforma_invoice_id && s.proforma_invoice_id !== "raised"
                ? <Link className="text-brand-ink underline-offset-2 hover:underline" href={`/finance/invoices/${s.proforma_invoice_id}`}>Proforma raised</Link>
                : s.invoice_id || s.proforma_invoice_id ? "Yes" : "No" },
            ]} />
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Add payment milestone"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving}
            disabled={!form.quotation_id || !form.name.trim() || !form.amount || !form.due_date}>Add milestone</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Quotation" required className="md:col-span-2">
            <Combobox value={form.quotation_id} source={quotationOptions}
              placeholder="Select quotation" emptyLabel="No quotations found"
              onChange={(id) => setForm({ ...form, quotation_id: id })} />
          </Field>
          <Field label="Milestone name" required className="md:col-span-2">
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="e.g. Deposit 30%, Progress 40%, Final 30%" />
          </Field>
          <Field label="Amount" required
            hint={selectedQuotation?.total != null ? `Quotation total: ${selectedQuotation.total.toLocaleString()} ${selectedQuotation.currency}` : undefined}>
            <Input type="number" numeric value={form.amount} onChange={(e) => setForm({ ...form, amount: e.target.value })} />
          </Field>
          <Field label="Due date" required>
            <Input type="date" value={form.due_date} onChange={(e) => setForm({ ...form, due_date: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
