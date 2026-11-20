"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { vendorApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import BankAccountSelect from "@/components/finance/BankAccountSelect";
import JournalLink from "@/components/finance/JournalLink";
import { formatCurrency } from "@/lib/currency";
import { Receipt, Search, Trash2 } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader,
  SavedViewTabs, Stat, StatGroup, StatusPill, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Vendor {
  id: string;
  name: string;
  vendor_type: string;
}

interface VendorInvoice {
  id: string;
  vendor_id: string;
  vendor_invoice_number: string;
  invoice_date: string;
  due_date?: string;
  amount: number;
  withholding_amount: number;
  net_payable: number;
  currency: string;
  status: string;
  work_order_id?: string;
  purchase_order_id?: string;
  notes?: string;
  created_at: string;
}

interface CurrencySummary {
  currency: string;
  payable: number;
  payable_count: number;
  withholding_withheld: number;
}

/** One figure per currency; amounts in different currencies are never added together. */
function PerCurrency({ rows, pick }: { rows: CurrencySummary[]; pick: (r: CurrencySummary) => number }) {
  const shown = rows.filter((r) => pick(r) !== 0);
  if (shown.length === 0) return <>-</>;
  return (
    <span className="flex flex-col gap-1 text-base">
      {shown.map((r) => <span key={r.currency}>{formatCurrency(pick(r), r.currency)}</span>)}
    </span>
  );
}

const STATUS_LABELS: Record<string, string> = {
  submitted: "Submitted", under_review: "Under review", approved: "Approved", rejected: "Rejected",
  paid: "Paid", partially_paid: "Partial",
};

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "submitted", label: "Submitted", filters: { status: "submitted" } },
  { id: "under_review", label: "Under review", filters: { status: "under_review" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "paid", label: "Paid", filters: { status: "paid" } },
];

const fmtDate = (d?: string | null, withYear = true) =>
  d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", ...(withYear ? { year: "numeric" } : {}) }) : "-";

export default function VendorInvoicesPage() {
  const views = useSavedViews<Filters>("vendor.invoices", VIEWS);
  const [chips, setChips] = useState<Record<string, boolean>>({});
  const [search, setSearch] = useState("");
  const [invoices, setInvoices] = useState<VendorInvoice[]>([]);
  const [summary, setSummary] = useState<CurrencySummary[]>([]);
  const [vendors, setVendors] = useState<Vendor[]>([]);
  const [loading, setLoading] = useState(true);
  const [reviewModal, setReviewModal] = useState<{ invoice: VendorInvoice; action: "approve" | "reject" | "paid" } | null>(null);
  const todayIso = () => new Date().toISOString().slice(0, 10);
  const emptyReview = () => ({ rejection_reason: "", payment_reference: "", bank_account_id: "", paid_date: todayIso() });
  const [reviewData, setReviewData] = useState(emptyReview);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      // KPI totals come from the server over every invoice, not from this page of rows.
      const [invRes, vendRes, sumRes] = await Promise.all([
        fetchAllPages<VendorInvoice>(vendorApi.listVendorInvoices),
        fetchAllPages<Vendor>(vendorApi.listVendors),
        vendorApi.vendorInvoiceSummary().catch(() => null),
      ]);
      setInvoices(invRes.rows);
      setSummary(sumRes?.data?.currencies ?? []);
      setVendors(vendRes.rows);
    } catch { setInvoices([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function deleteInvoice(id: string) {
    try {
      await vendorApi.deleteVendorInvoice(id);
      setConfirmDeleteId(null);
      await load();
    } catch (e) { toast.error(extractApiError(e, "Could not delete the invoice.")); }
  }

  // submitted -> under_review needs no input; approve/reject/paid go through the modal.
  async function startReview(invoice: { id: string }) {
    try {
      await vendorApi.updateVendorInvoiceStatus(invoice.id, { status: "under_review" });
      await load();
      window.dispatchEvent(new Event("approvals:changed"));
    } catch (e) { toast.error(extractApiError(e, "Failed to start review")); }
  }

  async function processAction() {
    if (!reviewModal) return;
    setSaving(true);
    try {
      const { invoice, action } = reviewModal;
      const statusMap = { approve: "approved", reject: "rejected", paid: "paid" } as const;
      await vendorApi.updateVendorInvoiceStatus(invoice.id, {
        status: statusMap[action],
        rejection_reason: action === "reject" ? reviewData.rejection_reason : undefined,
        payment_reference: action === "paid" ? reviewData.payment_reference : undefined,
        bank_account_id: action === "paid" ? reviewData.bank_account_id || undefined : undefined,
        paid_date: action === "paid" ? reviewData.paid_date || undefined : undefined,
      });
      setReviewModal(null);
      setReviewData(emptyReview());
      window.dispatchEvent(new Event("approvals:changed"));
      await load();
    } catch (e) {
      toast.error(extractApiError(e, "Could not update the invoice."));
    } finally { setSaving(false); }
  }

  const vendorMap = useMemo(() => Object.fromEntries(vendors.map((v) => [v.id, v.name])) as Record<string, string>, [vendors]);
  const status = chips.rejected ? "rejected" : views.active?.filters.status;
  const q = search.trim().toLowerCase();
  const filtered = invoices.filter((i) => (!status || i.status === status) &&
    (!q || i.vendor_invoice_number.toLowerCase().includes(q) || (vendorMap[i.vendor_id] ?? "").toLowerCase().includes(q)));

  // Payable = not yet paid or rejected (submitted, under review, approved, partially paid).
  const payableCount = summary.reduce((n, r) => n + r.payable_count, 0);
  const pendingCount = invoices.filter((i) => ["submitted", "under_review"].includes(i.status)).length;

  const columns = useMemo<ColumnDef<VendorInvoice>[]>(() => {
    const now = new Date();
    return [
      {
        id: "vendor", accessorFn: (i) => vendorMap[i.vendor_id] ?? "-", header: "Vendor",
        cell: (c) => (
          <div className="min-w-0">
            <p className="truncate font-medium text-fg">{c.getValue<string>()}</p>
            <p className="font-mono text-xs text-fg-muted">{c.row.original.vendor_invoice_number}</p>
          </div>
        ),
      },
      {
        accessorKey: "invoice_date", header: "Date", meta: { hideOnMobile: true },
        cell: (c) => {
          const inv = c.row.original;
          const isOverdue = inv.due_date && inv.status !== "paid" && new Date(inv.due_date) < now;
          return (
            <div>
              <p className="text-fg">{fmtDate(inv.invoice_date)}</p>
              {inv.due_date && (
                <p className={isOverdue ? "text-xs font-semibold text-tone-red" : "text-xs text-fg-muted"}>due {fmtDate(inv.due_date, false)}</p>
              )}
            </div>
          );
        },
      },
      {
        accessorKey: "amount", header: "Amount", meta: { numeric: true },
        cell: (c) => <span className="font-medium">{formatCurrency(c.row.original.amount, c.row.original.currency)}</span>,
      },
      {
        accessorKey: "withholding_amount", header: "Withholding", meta: { numeric: true, hideOnMobile: true },
        cell: (c) => <span className="text-fg-muted">{c.row.original.withholding_amount > 0 ? c.row.original.withholding_amount.toLocaleString() : "-"}</span>,
      },
      {
        accessorKey: "net_payable", header: "Net payable", meta: { numeric: true, hideOnMobile: true },
        cell: (c) => <span className="font-medium">{c.row.original.net_payable.toLocaleString()}</span>,
      },
      {
        accessorKey: "status", header: "Status",
        cell: (c) => <StatusPill family="vendor_invoice" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} />,
      },
      {
        id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
        cell: (c) => {
          const inv = c.row.original;
          const canDelete = ["submitted", "rejected"].includes(inv.status);
          return (
            <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
              {inv.status === "submitted" && <Button size="sm" onClick={() => startReview(inv)}>Review</Button>}
              {inv.status === "under_review" && (
                <>
                  <Button size="sm" variant="primary" onClick={() => setReviewModal({ invoice: inv, action: "approve" })}>Approve</Button>
                  <Button size="sm" variant="danger" onClick={() => { setReviewModal({ invoice: inv, action: "reject" }); setReviewData((d) => ({ ...d, rejection_reason: "" })); }}>Reject</Button>
                </>
              )}
              {inv.status === "approved" && (
                <Button size="sm" variant="primary" onClick={() => { setReviewModal({ invoice: inv, action: "paid" }); setReviewData(emptyReview()); }}>Mark paid</Button>
              )}
              {canDelete && (confirmDeleteId === inv.id ? (
                <>
                  <span className="text-xs font-semibold text-tone-red">Delete?</span>
                  <Button size="sm" variant="danger" onClick={() => deleteInvoice(inv.id)}>Yes</Button>
                  <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
                </>
              ) : (
                <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${inv.vendor_invoice_number}`} onClick={() => setConfirmDeleteId(inv.id)} />
              ))}
            </div>
          );
        },
      },
    ];
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [confirmDeleteId, vendorMap]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={invoices.length} />
        <Stat label="Pending" value={pendingCount} note="submitted or under review" alert={pendingCount > 0} />
        <Stat label="Payable" value={<PerCurrency rows={summary} pick={(r) => r.payable} />} note={`${payableCount} unpaid`} />
        <Stat label="Withholding tax" value={<PerCurrency rows={summary} pick={(r) => r.withholding_withheld} />} note="excludes rejected" />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Vendor invoices"
          summary={`${invoices.length} invoice${invoices.length !== 1 ? "s" : ""}, ${pendingCount} pending. Incoming invoices from vendors and contractors.`} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} onSelect={() => setChips({})} />
        <FilterChips chips={[{ id: "rejected", label: "Rejected", active: !!chips.rejected }]}
          onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id] }))} onClear={() => setChips({})}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search invoices" aria-label="Search vendor invoices" className="w-56" />
        </FilterChips>
        <DataTable label="Vendor invoices" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Receipt, title: "No vendor invoices", description: "Vendors submit invoices from the vendor portal." }}
          renderDetails={(inv) => (
            <div className="space-y-3">
              <KeyValues cols={3} items={[
                { label: "Vendor", value: vendorMap[inv.vendor_id] ?? "-" },
                { label: "Invoice number", value: <span className="font-mono">{inv.vendor_invoice_number}</span> },
                { label: "Invoice date", value: fmtDate(inv.invoice_date) },
                { label: "Due date", value: fmtDate(inv.due_date) },
                { label: "Amount", value: formatCurrency(inv.amount, inv.currency), numeric: true },
                { label: "Withholding", value: inv.withholding_amount > 0 ? formatCurrency(inv.withholding_amount, inv.currency) : "-", numeric: true },
                { label: "Net payable", value: formatCurrency(inv.net_payable, inv.currency), numeric: true },
              ]} />
              {inv.notes && <p className="text-sm text-fg-muted">{inv.notes}</p>}
              {["approved", "paid"].includes(inv.status) && (
                <JournalLink source="vendor_invoice" extraSources={["vendor_payment"]} sourceId={inv.id} refreshKey={inv.status} />
              )}
            </div>
          )} />
      </ListFrame>

      <Dialog open={!!reviewModal} onOpenChange={(o) => { if (!o) setReviewModal(null); }}
        title={reviewModal?.action === "approve" ? "Approve invoice" : reviewModal?.action === "reject" ? "Reject invoice" : "Mark as paid"}
        description={reviewModal && (
          <span className="tabular">
            {vendorMap[reviewModal.invoice.vendor_id] ?? "-"} · {reviewModal.invoice.vendor_invoice_number}
            {" · "}{reviewModal.invoice.net_payable.toLocaleString()} {reviewModal.invoice.currency}
          </span>
        )}
        footer={reviewModal && <>
          <Button onClick={() => setReviewModal(null)}>Cancel</Button>
          <Button variant={reviewModal.action === "reject" ? "danger" : "primary"} onClick={processAction} loading={saving}
            disabled={reviewModal.action === "reject" && !reviewData.rejection_reason.trim()}>
            {saving ? "Processing" : reviewModal.action === "approve" ? "Approve" : reviewModal.action === "reject" ? "Reject" : "Mark paid"}
          </Button>
        </>}>
        {reviewModal && (
          <div className="space-y-4">
            {reviewModal.action === "reject" && (
              <Field label="Rejection reason" required>
                <Textarea value={reviewData.rejection_reason} onChange={(e) => setReviewData({ ...reviewData, rejection_reason: e.target.value })} rows={3} />
              </Field>
            )}
            {reviewModal.action === "paid" && (
              <>
                <Field label="Paid from">
                  <BankAccountSelect value={reviewData.bank_account_id} currency={reviewModal.invoice.currency}
                    onChange={(v) => setReviewData({ ...reviewData, bank_account_id: v })} />
                </Field>
                <Field label="Paid date">
                  <Input type="date" value={reviewData.paid_date} onChange={(e) => setReviewData({ ...reviewData, paid_date: e.target.value })} />
                </Field>
                <Field label="Payment reference">
                  <Input value={reviewData.payment_reference} onChange={(e) => setReviewData({ ...reviewData, payment_reference: e.target.value })}
                    placeholder="e.g. TRF-20260519-001" />
                </Field>
              </>
            )}
            {reviewModal.action === "approve" && (
              <p className="rounded-lg bg-tone-blue-soft px-4 py-3 text-sm text-tone-blue">
                Approving will make this invoice eligible for payment processing.
              </p>
            )}
          </div>
        )}
      </Dialog>
    </div>
  );
}
