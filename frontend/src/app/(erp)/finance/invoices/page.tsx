"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { financeApi } from "@/lib/api";
import { formatCurrency } from "@/lib/currency";
import { invoiceTypeLabel } from "@/lib/invoiceTypes";
import { AlertTriangle, Clock, Download, FileText, Plus, Search, Trash2 } from "lucide-react";
import Link from "next/link";
import { csvName, downloadCsv } from "@/lib/csvExport";
import {
  Button, DataTable, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader, SavedViewTabs,
  Stat, StatGroup, StatusPill, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";

interface Invoice {
  id: string;
  number: string;
  invoice_type: string;
  status: string;
  subtotal: number | null;
  total: number | null;
  amount_paid: number;
  amount_due?: number;
  currency?: string;
  due_date?: string;
  issue_date?: string | null;
  customer_name?: string | null;
}

interface CurrencySummary {
  currency: string;
  outstanding: number;
  overdue: number;
  paid_this_month: number;
}

/** One figure per currency; amounts in different currencies are never added together. */
function PerCurrency({ rows, pick }: { rows: CurrencySummary[]; pick: (r: CurrencySummary) => number }) {
  const shown = rows.filter((r) => pick(r) !== 0);
  if (shown.length === 0) return <>-</>;
  return (
    <span className="flex flex-col gap-1">
      {shown.map((r) => <span key={r.currency}>{formatCurrency(pick(r), r.currency)}</span>)}
    </span>
  );
}

const STATUS_LABELS: Record<string, string> = {
  draft: "Draft", sent: "Sent", partial: "Partial", paid: "Paid", overdue: "Overdue", cancelled: "Cancelled",
  partially_paid: "Partial", superseded: "Superseded", converted: "Converted",
};

const fmt = (n: number, cur = "USD") => formatCurrency(n, cur);
const fmtDate = (d?: string | null) => (d ? new Date(d).toLocaleDateString("en-GB") : "-");

function DueDateChip({ dueDate, status }: { dueDate?: string; status: string }) {
  if (!dueDate || status === "paid" || status === "cancelled") return <span className="text-fg-subtle">-</span>;
  const diff = Math.ceil((new Date(dueDate).getTime() - Date.now()) / 86400000);
  if (status === "overdue" || diff < 0)
    return <span className="inline-flex items-center gap-1 text-xs font-semibold text-tone-red"><AlertTriangle className="size-3" aria-hidden />{Math.abs(diff)}d overdue</span>;
  if (diff <= 3)
    return <span className="inline-flex items-center gap-1 text-xs font-semibold text-tone-orange"><Clock className="size-3" aria-hidden />Due in {diff}d</span>;
  return <span className="text-xs text-fg-muted">{fmtDate(dueDate)}</span>;
}

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "sent", label: "Sent", filters: { status: "sent" } },
  { id: "partial", label: "Partly paid", filters: { status: "partial" } },
  { id: "overdue", label: "Overdue", filters: { status: "overdue" } },
  { id: "paid", label: "Paid", filters: { status: "paid" } },
];

// Statuses not covered by a built-in view stay one click away as chips.
const CHIP_STATUSES = ["partially_paid", "cancelled", "superseded", "converted"];

export default function InvoicesPage() {
  const views = useSavedViews<Filters>("finance.invoices", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [invoices, setInvoices] = useState<Invoice[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [summary, setSummary] = useState<CurrencySummary[]>([]);

  const statusFilter = chipStatus || views.active?.filters.status || "";
  const filters: Filters = statusFilter ? { status: statusFilter } : {};

  const load = useCallback(async () => {
    setLoading(true);
    try {
      // The list endpoint has no text search; search is filtered client-side below.
      const params: Record<string, string> = {};
      if (statusFilter) params.status = statusFilter;
      // KPI figures come from the server over every invoice, not from this page of rows.
      const [res, sumRes] = await Promise.all([
        fetchAllPages<Invoice>(financeApi.listInvoices, params),
        financeApi.invoiceSummary().catch(() => null),
      ]);
      setInvoices(res.rows);
      setSummary(sumRes?.data?.currencies ?? []);
    } catch {
      setInvoices([]);
    } finally {
      setLoading(false);
    }
  }, [statusFilter]);

  useEffect(() => { load(); }, [load]);

  async function deleteInvoice(id: string) {
    setDeleting(true);
    try {
      await financeApi.deleteInvoice(id);
      setConfirmDeleteId(null);
      setInvoices((prev) => prev.filter((i) => i.id !== id));
    } catch {
      toast.error("Failed to delete invoice. It may be in a sent or paid state.");
    } finally { setDeleting(false); }
  }

  const q = search.trim().toLowerCase();
  const visible = q
    ? invoices.filter((i) => i.number.toLowerCase().includes(q) || (i.customer_name ?? "").toLowerCase().includes(q))
    : invoices;
  const hasOverdue = summary.some((r) => r.overdue > 0);
  const draftCount = invoices.filter((i) => i.status === "draft").length;
  const outstandingLine = summary.filter((r) => r.outstanding !== 0).map((r) => formatCurrency(r.outstanding, r.currency)).join(", ");

  const columns = useMemo<ColumnDef<Invoice>[]>(() => [
    {
      accessorKey: "number", header: "Invoice",
      cell: (c) => (
        <div>
          <p className="font-mono text-[13px] font-medium text-fg">{c.row.original.number}</p>
          <p className="text-xs text-fg-muted">{fmtDate(c.row.original.issue_date)}</p>
        </div>
      ),
    },
    {
      accessorKey: "customer_name", header: "Client",
      cell: (c) => (
        <div className="min-w-0">
          <p className="max-w-44 truncate font-medium text-fg">{c.row.original.customer_name ?? "-"}</p>
          <p className="text-xs text-fg-muted">{invoiceTypeLabel(c.row.original.invoice_type)}</p>
        </div>
      ),
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="invoice" status={c.row.original.status} label={STATUS_LABELS[c.row.original.status]} />,
    },
    {
      id: "total", accessorFn: (r) => r.total ?? 0, header: "Amount", meta: { numeric: true },
      cell: (c) => <span className="font-medium">{fmt(c.row.original.total ?? 0, c.row.original.currency)}</span>,
    },
    {
      accessorKey: "amount_paid", header: "Paid", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => {
        const inv = c.row.original;
        const paidPct = (inv.total ?? 0) > 0 ? (inv.amount_paid / (inv.total as number)) * 100 : 0;
        return (
          <div className="flex flex-col items-end gap-1">
            <span className="text-[13px]">{fmt(inv.amount_paid, inv.currency)}</span>
            <div className="h-1 w-16 overflow-hidden rounded-full bg-surface-2" aria-hidden>
              <div className={cn("h-full rounded-full", paidPct >= 100 ? "bg-tone-blue" : paidPct > 0 ? "bg-tone-orange" : "bg-line")}
                style={{ width: `${Math.min(100, paidPct)}%` }} />
            </div>
          </div>
        );
      },
    },
    {
      accessorKey: "due_date", header: "Due date", meta: { hideOnMobile: true },
      cell: (c) => <DueDateChip dueDate={c.row.original.due_date} status={c.row.original.status} />,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const inv = c.row.original;
        if (confirmDeleteId === inv.id) {
          return (
            <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
              <span className="text-xs font-semibold text-tone-red">Delete?</span>
              <Button size="sm" variant="danger" loading={deleting} onClick={() => deleteInvoice(inv.id)}>Yes</Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
            </div>
          );
        }
        return inv.status === "draft"
          ? <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${inv.number}`} onClick={() => setConfirmDeleteId(inv.id)} />
          : null;
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, deleting]);

  /** The list as shown (current view, chip and search) as a CSV file. */
  function exportCsv() {
    downloadCsv(csvName("invoices"), visible, [
      { header: "Number", value: (i) => i.number },
      { header: "Type", value: (i) => invoiceTypeLabel(i.invoice_type) },
      { header: "Customer", value: (i) => i.customer_name },
      { header: "Status", value: (i) => STATUS_LABELS[i.status] ?? i.status },
      { header: "Total", value: (i) => i.total },
      { header: "Paid", value: (i) => i.amount_paid },
      { header: "Due", value: (i) => i.amount_due },
      { header: "Currency", value: (i) => i.currency },
      { header: "Issued", value: (i) => i.issue_date },
      { header: "Due date", value: (i) => i.due_date },
    ]);
  }

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Outstanding" value={<PerCurrency rows={summary} pick={(r) => r.outstanding} />} note="open commercial invoices" />
        <Stat label="Paid this month" value={<PerCurrency rows={summary} pick={(r) => r.paid_this_month} />} note="confirmed payments" />
        <Stat label="Overdue" value={<PerCurrency rows={summary} pick={(r) => r.overdue} />} note="past due, unpaid" alert={hasOverdue} />
        <Stat label="Draft" value={draftCount} note="not sent yet" />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Invoices"
          summary={`${visible.length} invoice${visible.length !== 1 ? "s" : ""}${outstandingLine ? `, ${outstandingLine} outstanding` : ""}`}
          actions={<>
            <Button disabled={loading || visible.length === 0} onClick={exportCsv}><Download aria-hidden /> Export</Button>
            <Button variant="primary" asChild><Link href="/finance/invoices/new"><Plus aria-hidden /> New invoice</Link></Button>
          </>} />
        <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChipStatus("")} alertIds={["overdue"]} />
        <FilterChips
          chips={CHIP_STATUSES.map((s) => ({ id: s, label: STATUS_LABELS[s] === "Partial" ? "Partially paid" : STATUS_LABELS[s], active: chipStatus === s }))}
          onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))}
          onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search invoices" aria-label="Search invoices" className="w-56" />
        </FilterChips>
        <DataTable label="Invoices" columns={columns} data={visible} getRowId={(r) => r.id} loading={loading}
          empty={{
            icon: FileText, title: "No invoices found", description: "Create your first invoice to start tracking payments.",
            action: <Button variant="primary" asChild><Link href="/finance/invoices/new">Create invoice</Link></Button>,
          }}
          renderDetails={(inv) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Client", value: inv.customer_name ?? "-" },
                { label: "Type", value: invoiceTypeLabel(inv.invoice_type) },
                { label: "Issued", value: fmtDate(inv.issue_date) },
                { label: "Total", value: fmt(inv.total ?? 0, inv.currency), numeric: true },
                { label: "Paid", value: fmt(inv.amount_paid, inv.currency), numeric: true },
                { label: "Amount due", value: fmt(inv.amount_due ?? (inv.total ?? 0) - inv.amount_paid, inv.currency), numeric: true },
                { label: "Due date", value: fmtDate(inv.due_date) },
              ]} />
              <Button size="sm" asChild><Link href={`/finance/invoices/${inv.id}`}>Open invoice</Link></Button>
            </div>
          )} />
      </ListFrame>
    </div>
  );
}
