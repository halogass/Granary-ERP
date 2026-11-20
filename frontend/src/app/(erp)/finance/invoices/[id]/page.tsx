"use client";

import { useEffect, useState, useCallback } from "react";
import { useParams, useRouter } from "next/navigation";
import { financeApi, systemApi } from "@/lib/api";
import { formatCurrency, getCurrencySymbol } from "@/lib/currency";
import { printInvoice } from "@/lib/print";
import { downloadVerifiedCopy } from "@/lib/verifiedDownload";
import {
  FileText, FileCheck, Send, AlertCircle, Download, CreditCard, Banknote, Copy, Pencil, Ban,
} from "lucide-react";
import Link from "next/link";
import {
  Button, Card, CardBody, CardHeader, DataTable, Dialog, EmptyState, Field, FieldGrid, Input, KeyValues, PageHeader,
  Pill, Select, Skeleton, StatusPill, Textarea, toast, type ColumnDef, type Tone,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { extractApiError } from "@/lib/apiError";
import ReasonModal from "@/components/ui/ReasonModal";
import ApprovalPanel from "@/components/approvals/ApprovalPanel";
import { useSendDocument } from "@/components/approvals/useSendDocument";
import BankAccountSelect from "@/components/finance/BankAccountSelect";
import JournalLink from "@/components/finance/JournalLink";
import InvoiceClientCard, { type InvoiceClientFields } from "@/components/finance/InvoiceClientCard";

interface Payment {
  id: string;
  amount: number;
  payment_date: string;
  payment_method: string;
  payment_reference?: string;
  notes?: string;
}

interface Refund {
  id: string;
  amount: number;
  currency: string;
  paid_date: string;
  reference?: string | null;
}

interface LineItem {
  id: string;
  name?: string;
  description: string;
  qty: number;
  unit_price: number;
  discount_pct: number;
  tax_pct: number;
  subtotal: number;
}

interface Invoice extends InvoiceClientFields {
  id: string;
  number: string;
  invoice_type: string;
  status: string;
  total: number;
  /** Sum of line nets (after line discounts). The gross is subtotal + discount_total. */
  subtotal: number;
  discount_total?: number;
  tax_amount: number;
  amount_paid: number;
  amount_due: number;
  /** Paid, then taken back by a credit note: owed to the customer until refunded. */
  refund_due?: number;
  refunds?: Refund[];
  issue_date: string;
  due_date?: string;
  notes?: string;
  currency?: string;
  exchange_rate?: number;
  quotation?: { id: string; number: string };
  items: LineItem[];
  payments?: Payment[];
  full_number?: string;
  revision?: number;
  revision_reason?: string | null;
  active_revision_id?: string | null;
  revisions?: InvRevisionRow[];
  credited_by?: { id: string; full_number: string; status: string } | null;
  converted_to?: { id: string; full_number: string } | null;
  credit_of_invoice_id?: string | null;
}

interface InvRevisionRow {
  id: string; full_number: string; revision: number; status: string;
  created_at?: string | null; revision_reason?: string | null; is_current?: boolean;
}

const STATUS_LABELS: Record<string, string> = {
  partial: "Partial", partially_paid: "Partial",
};

function Banner({ tone, icon: Icon, children, action }: { tone: Tone; icon: React.ElementType; children: React.ReactNode; action?: React.ReactNode }) {
  const cls: Record<Tone, string> = {
    neutral: "border-line bg-surface-2 text-fg", blue: "border-tone-blue/30 bg-tone-blue-soft text-fg",
    violet: "border-tone-violet/30 bg-tone-violet-soft text-fg", orange: "border-tone-orange/30 bg-tone-orange-soft text-fg",
    red: "border-tone-red/30 bg-tone-red-soft text-fg",
  };
  const ink: Record<Tone, string> = {
    neutral: "text-fg-muted", blue: "text-tone-blue", violet: "text-tone-violet", orange: "text-tone-orange", red: "text-tone-red",
  };
  return (
    <div role="status" className={cn("mb-4 flex flex-wrap items-center gap-3 rounded-lg border px-4 py-3", cls[tone])}>
      <Icon className={cn("size-4 shrink-0", ink[tone])} aria-hidden />
      <p className="min-w-0 flex-1 text-sm">{children}</p>
      {action}
    </div>
  );
}

const fmtLongDate = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

// Stored as the label itself; the backend's default payment_method is "Bank Transfer (T/T)".
// Legacy snake_case values still render through the lookup.
const PAYMENT_METHODS: Record<string, string> = {
  "Bank Transfer (T/T)": "Bank Transfer (T/T)",
  "Cash": "Cash",
  "Check": "Check",
  "Credit Card": "Credit Card",
  "Other": "Other",
  bank_transfer: "Bank Transfer",
  cash: "Cash",
  check: "Check",
  credit_card: "Credit Card",
  other: "Other",
};
const PAYMENT_METHOD_OPTIONS = ["Bank Transfer (T/T)", "Cash", "Check", "Credit Card", "Other"];

// Statuses that can still receive a payment ("partial" is a legacy spelling of partially_paid).
const PAYABLE_STATUSES = ["sent", "partial", "partially_paid", "overdue"];


export default function InvoiceDetailPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const [invoice, setInvoice] = useState<Invoice | null>(null);
  const [functionalCur, setFunctionalCur] = useState("USD");
  useEffect(() => {
    systemApi.getCurrencySettings().then((r) => { if (r.data?.functional) setFunctionalCur(r.data.functional); }).catch(() => {});
  }, []);
  const [loading, setLoading] = useState(true);
  const [showPayModal, setShowPayModal] = useState(false);
  const [showRefundModal, setShowRefundModal] = useState(false);
  const [refundForm, setRefundForm] = useState({
    amount: "", paid_date: new Date().toISOString().slice(0, 10), bank_account_id: "", reference: "",
  });
  const [showReviseModal, setShowReviseModal] = useState(false);
  const [showCreditModal, setShowCreditModal] = useState(false);
  const [actioning, setActioning] = useState(false);
  const [approvalKey, setApprovalKey] = useState(0);
  const [downloading, setDownloading] = useState(false);
  const [payForm, setPayForm] = useState({
    amount: "",
    payment_date: new Date().toISOString().slice(0, 10),
    method: "Bank Transfer (T/T)",
    reference: "",
    notes: "",
    bank_account_id: "",
    exchange_rate: "",
  });

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await financeApi.getInvoice(id);
      setInvoice(res.data);
    } catch {
      setInvoice(null);
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => { load(); }, [load]);

  // A foreign payment is booked at the payment date's rate: prefill it from the rates on record
  // (left empty when there is none; the user enters it, or the API says which rate is missing).
  const payCur = invoice?.currency ?? "USD";
  const payForeign = !!invoice && payCur !== functionalCur && !!invoice.exchange_rate;
  useEffect(() => {
    if (!showPayModal || !payForeign || !payForm.payment_date) return;
    let cancelled = false;
    financeApi.getSpotRate(payCur, functionalCur, payForm.payment_date)
      .then((r) => { if (!cancelled) setPayForm((f) => ({ ...f, exchange_rate: String(r.data?.rate ?? "") })); })
      .catch(() => { if (!cancelled) setPayForm((f) => ({ ...f, exchange_rate: "" })); });
    return () => { cancelled = true; };
  }, [showPayModal, payForeign, payCur, functionalCur, payForm.payment_date]);

  // Send: submits for Owner approval first; once approved, emails / WhatsApps the verified PDF.
  const sender = useSendDocument({
    entityType: "invoice", label: "Invoice",
    onChanged: () => { load(); setApprovalKey((k) => k + 1); },
    onMessage: (kind, message) =>
      kind === "success" ? toast.success(message) : kind === "info" ? toast.info(message) : toast.error(message),
  });

  function sendInvoice() {
    sender.start(id);
  }


  async function convertToCommercial() {
    setActioning(true);
    try {
      await financeApi.convertToCommercial(id, {});
      await load();
    } finally { setActioning(false); }
  }

  async function recordPayment() {
    if (!payForm.amount || !payForm.payment_date) return;
    setActioning(true);
    try {
      await financeApi.recordPayment({
        invoice_id: id,
        amount: parseFloat(payForm.amount),
        payment_date: payForm.payment_date,
        payment_method: payForm.method,
        payment_reference: payForm.reference || undefined,
        notes: payForm.notes || undefined,
        bank_account_id: payForm.bank_account_id || undefined,
        exchange_rate: payForm.exchange_rate ? payForm.exchange_rate : undefined,
      });
      setShowPayModal(false);
      setPayForm({ amount: "", payment_date: new Date().toISOString().slice(0, 10), method: "Bank Transfer (T/T)", reference: "", notes: "", bank_account_id: "", exchange_rate: "" });
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to record payment"));
    } finally { setActioning(false); }
  }

  async function recordRefund() {
    if (!refundForm.amount || !refundForm.paid_date) return;
    setActioning(true);
    try {
      await financeApi.recordRefund(id, {
        amount: refundForm.amount,
        paid_date: refundForm.paid_date,
        bank_account_id: refundForm.bank_account_id || undefined,
        reference: refundForm.reference || undefined,
      });
      setShowRefundModal(false);
      setRefundForm({ amount: "", paid_date: new Date().toISOString().slice(0, 10), bank_account_id: "", reference: "" });
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to record the refund"));
    } finally { setActioning(false); }
  }

  if (loading) return (
    <div className="mx-auto max-w-5xl space-y-4" aria-busy>
      <Skeleton className="h-6 w-56" />
      <Skeleton className="h-40 w-full" />
      <Skeleton className="h-64 w-full" />
    </div>
  );

  if (!invoice) return (
    <EmptyState icon={AlertCircle} title="Invoice not found."
      action={<Button asChild><Link href="/finance/invoices">Back to invoices</Link></Button>} />
  );

  const cur = invoice.currency ?? "USD";
  const isForeignInv = cur !== functionalCur && !!invoice.exchange_rate;
  const isCreditNote = invoice.invoice_type === "credit_note";
  // A credit note in progress (draft, awaiting approval) or sent: only one per invoice.
  const liveCreditNote = invoice.credited_by && invoice.credited_by.status !== "cancelled" ? invoice.credited_by : null;
  const canPay = PAYABLE_STATUSES.includes(invoice.status) && !isCreditNote;
  const fmt = (n: number) => formatCurrency(n, cur);
  const paidPct = invoice.total > 0 ? Math.min(100, (invoice.amount_paid / invoice.total) * 100) : 0;
  const isOverdue = invoice.due_date && invoice.status !== "paid" && new Date(invoice.due_date) < new Date();

  const openPay = () => {
    setPayForm((f) => ({ ...f, amount: invoice.amount_due.toString() }));
    setShowPayModal(true);
  };

  async function downloadPdf() {
    if (!invoice) return;
    setDownloading(true);
    try {
      const result = await downloadVerifiedCopy("invoice", id, () => printInvoice({
        ...invoice,
        // The print's totals read Subtotal, Discount, Tax, so start from the gross.
        subtotal: invoice.subtotal + (invoice.discount_total ?? 0),
        outstanding: invoice.amount_due,
        payments: invoice.payments?.map((p) => ({
          amount: p.amount, payment_date: p.payment_date,
          method: p.payment_method, reference: p.payment_reference,
        })),
      }), { requireApproval: true, label: "invoice" });
      if (result.outcome === "refused") toast.error(result.message);
    } finally {
      setDownloading(false);
      setApprovalKey((k) => k + 1);
    }
  }

  const progress = (
    <div className="h-2 w-full overflow-hidden rounded-full bg-surface-2" role="progressbar" aria-valuenow={Math.round(paidPct)} aria-valuemin={0} aria-valuemax={100} aria-label="Paid">
      <div className={cn("h-full rounded-full", paidPct >= 100 ? "bg-tone-blue" : "bg-tone-orange")} style={{ width: `${paidPct}%` }} />
    </div>
  );

  const itemColumns: ColumnDef<LineItem>[] = [
    {
      id: "item", header: "Item", enableSorting: false,
      cell: (c) => (
        <div>
          <p className="font-medium text-fg">{c.row.original.name || c.row.original.description || "-"}</p>
          {c.row.original.name && c.row.original.description && <p className="mt-0.5 whitespace-pre-line text-xs text-fg-muted">{c.row.original.description}</p>}
        </div>
      ),
    },
    { accessorKey: "qty", header: "Qty", enableSorting: false, meta: { numeric: true } },
    { accessorKey: "unit_price", header: "Unit price", enableSorting: false, meta: { numeric: true }, cell: (c) => fmt(c.row.original.unit_price) },
    {
      accessorKey: "discount_pct", header: "Disc", enableSorting: false, meta: { numeric: true, hideOnMobile: true },
      cell: (c) => c.row.original.discount_pct > 0 ? `${c.row.original.discount_pct}%` : <span className="text-fg-subtle">-</span>,
    },
    {
      accessorKey: "tax_pct", header: "Tax", enableSorting: false, meta: { numeric: true, hideOnMobile: true },
      cell: (c) => c.row.original.tax_pct > 0 ? `${c.row.original.tax_pct}%` : <span className="text-fg-subtle">-</span>,
    },
    { accessorKey: "subtotal", header: "Subtotal", enableSorting: false, meta: { numeric: true }, cell: (c) => <span className="font-medium">{fmt(c.row.original.subtotal)}</span> },
  ];

  const busy = actioning || !!sender.preparing;

  return (
    <div className="mx-auto max-w-5xl">
      <PageHeader back={{ href: "/finance/invoices", label: "Invoices" }}
        title={<span className="font-mono">{invoice.number}</span>}
        status={
          <span className="inline-flex items-center gap-2">
            <StatusPill family="invoice" status={invoice.status} label={STATUS_LABELS[invoice.status]} />
            {invoice.invoice_type === "proforma" && <Pill tone="violet">Proforma</Pill>}
            {isCreditNote && <Pill tone="violet">Credit note</Pill>}
          </span>
        }
        summary={<span className="inline-flex flex-wrap items-center gap-x-3 gap-y-1">
          <span>{fmt(invoice.total)}</span>
          <JournalLink source="invoice" extraSources={["advance_application", "credit_note"]} sourceId={invoice.id}
            refreshKey={`${invoice.status}:${invoice.amount_paid}`} />
        </span>}
        actions={<>
          <Button loading={downloading} onClick={downloadPdf}>{!downloading && <Download aria-hidden />} PDF</Button>
          {invoice.status === "draft" && !isCreditNote && (
            <Button asChild><Link href={`/finance/invoices/${invoice.id}/edit`}><Pencil aria-hidden /> Edit</Link></Button>
          )}
          {["sent", "partially_paid", "overdue"].includes(invoice.status) && (
            <Button onClick={sendInvoice} disabled={busy}><Send aria-hidden /> Resend</Button>
          )}
          {invoice.invoice_type === "proforma" && invoice.status === "sent" && (
            <Button onClick={() => setShowReviseModal(true)} disabled={actioning}><Copy aria-hidden /> Revise</Button>
          )}
          {invoice.invoice_type === "commercial" && invoice.status !== "cancelled" && liveCreditNote && (
            <Button asChild>
              <Link href={`/finance/invoices/${liveCreditNote.id}`}>
                <Ban aria-hidden /> Credit note {liveCreditNote.full_number}
                {liveCreditNote.status === "draft" && <span className="text-xs font-normal text-fg-muted">(draft, pending)</span>}
              </Link>
            </Button>
          )}
          {invoice.invoice_type === "commercial" && invoice.status !== "cancelled" && !liveCreditNote && (
            <Button onClick={() => setShowCreditModal(true)} disabled={actioning}
              title="Commercial invoices can't be edited; issue a credit note to amend.">
              <Ban aria-hidden /> Issue credit note
            </Button>
          )}
          {invoice.invoice_type === "proforma" && ["sent", "draft"].includes(invoice.status) && (
            <Button onClick={convertToCommercial} disabled={actioning}><FileCheck aria-hidden /> Convert to commercial</Button>
          )}
          {invoice.status === "draft" && (
            <Button variant="primary" onClick={sendInvoice} disabled={busy}><Send aria-hidden /> Send invoice</Button>
          )}
          {canPay && (
            <Button variant="primary" onClick={openPay}><CreditCard aria-hidden /> Record payment</Button>
          )}
        </>} />

      <ApprovalPanel entityType="invoice" entityId={id} refreshKey={approvalKey} onChange={load} />
      {sender.modal}

      {invoice.status === "superseded" && (
        <Banner tone="neutral" icon={Copy}
          action={invoice.active_revision_id && (
            <Button size="sm" asChild><Link href={`/finance/invoices/${invoice.active_revision_id}`}>Go to current revision</Link></Button>
          )}>
          This is an older revision and has been <strong>superseded</strong>. It is read-only.
        </Banner>
      )}

      {invoice.status === "converted" && (
        <Banner tone="violet" icon={FileCheck}
          action={invoice.converted_to && (
            <Button size="sm" asChild><Link href={`/finance/invoices/${invoice.converted_to.id}`}>View {invoice.converted_to.full_number}</Link></Button>
          )}>
          This proforma was <strong>converted</strong> to a commercial invoice, which now carries the balance. It is read-only.
        </Banner>
      )}

      {invoice.status === "cancelled" && invoice.credited_by && (
        <Banner tone="orange" icon={Ban}
          action={<Button size="sm" asChild><Link href={`/finance/invoices/${invoice.credited_by.id}`}>View {invoice.credited_by.full_number}</Link></Button>}>
          This invoice was <strong>cancelled</strong> by a credit note.
        </Banner>
      )}

      {/* Paid, then credited: the money is owed back to the customer */}
      {(invoice.refund_due ?? 0) > 0 && (
        <Banner tone="red" icon={Banknote}
          action={<Button size="sm" variant="primary" onClick={() => {
            setRefundForm((f) => ({ ...f, amount: String(invoice.refund_due ?? "") }));
            setShowRefundModal(true);
          }}>Record refund</Button>}>
          <Pill tone="red" className="mr-2">Refund due</Pill>
          <span className="tabular">{fmt(invoice.refund_due ?? 0)}</span> the customer paid is owed back to them after the credit note.
        </Banner>
      )}

      {invoice.invoice_type === "credit_note" && invoice.credit_of_invoice_id && (
        <Banner tone="violet" icon={Ban}
          action={<Button size="sm" asChild><Link href={`/finance/invoices/${invoice.credit_of_invoice_id}`}>View original invoice</Link></Button>}>
          This is a <strong>credit note</strong>{invoice.revision_reason ? `: ${invoice.revision_reason}` : ""}.
        </Banner>
      )}

      {isOverdue && (
        <Banner tone="red" icon={AlertCircle}>
          This invoice was due on <strong>{new Date(invoice.due_date!).toLocaleDateString("en-GB")}</strong>; payment is overdue.
        </Banner>
      )}

      {invoice.revisions && invoice.revisions.length > 1 && (
        <Card className="mb-4">
          <CardHeader title="Revisions" />
          <ul className="divide-y divide-line-soft">
            {invoice.revisions.map((r) => (
              <li key={r.id}>
                <Link href={`/finance/invoices/${r.id}`} aria-current={r.is_current ? "page" : undefined}
                  className={cn("flex min-h-11 items-center gap-3 px-4 py-2 text-sm md:min-h-9", r.is_current ? "bg-brand/[0.07]" : "hover:bg-surface-2")}>
                  <span className="font-mono font-medium text-fg">{r.full_number}</span>
                  <StatusPill family="invoice" status={r.status} label={STATUS_LABELS[r.status]} />
                  {r.revision_reason && <span className="min-w-0 flex-1 truncate text-xs text-fg-muted">{r.revision_reason}</span>}
                  <span className="ml-auto shrink-0 text-xs tabular text-fg-muted">{r.created_at ? new Date(r.created_at).toLocaleDateString("en-GB") : ""}</span>
                </Link>
              </li>
            ))}
          </ul>
        </Card>
      )}

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <div className="space-y-4 lg:col-span-2">
          <Card>
            <CardBody className="space-y-4">
              <div className="flex flex-wrap items-start justify-between gap-4">
                <KeyValues className="min-w-0 flex-1" items={[
                  { label: "Issue date", value: fmtLongDate(invoice.issue_date) },
                  {
                    label: "Due date",
                    value: invoice.due_date
                      ? <span className={cn(isOverdue && "font-semibold text-tone-red")}>{fmtLongDate(invoice.due_date)}</span>
                      : "-",
                  },
                  ...(invoice.quotation ? [{
                    label: "Quotation",
                    value: <Link href={`/sales/quotations/${invoice.quotation.id}`} className="inline-flex items-center gap-1 text-brand-ink hover:underline">
                      <FileText className="size-3" aria-hidden /> From {invoice.quotation.number}
                    </Link>,
                  }] : []),
                ]} />
                <div className="text-right">
                  <p className="text-xs text-fg-muted">Total</p>
                  <p className="text-2xl font-semibold tabular text-fg">{fmt(invoice.total)}</p>
                  {isForeignInv && (
                    <p className="mt-0.5 text-xs tabular text-fg-muted">
                      1 {cur} = {getCurrencySymbol(functionalCur)} {Number(invoice.exchange_rate).toLocaleString("en-US", { maximumFractionDigits: 6 })}
                    </p>
                  )}
                </div>
              </div>

              {invoice.status !== "draft" && invoice.status !== "cancelled" && (
                <div className="border-t border-line-soft pt-4">
                  <div className="mb-2 flex justify-between text-xs text-fg-muted">
                    <span>Payment progress</span>
                    <span className="tabular">{Math.round(paidPct)}%</span>
                  </div>
                  {progress}
                  <div className="mt-2 flex justify-between text-xs tabular">
                    <span className="text-fg-muted">Paid: <span className="font-medium text-fg">{fmt(invoice.amount_paid)}</span></span>
                    <span className={invoice.amount_due > 0 ? "font-medium text-tone-orange" : "text-fg-muted"}>
                      Outstanding: {fmt(invoice.amount_due)}
                    </span>
                  </div>
                </div>
              )}
            </CardBody>
          </Card>

          <Card className="overflow-hidden">
            <CardHeader title="Line items" />
            <DataTable label="Line items" columns={itemColumns} data={invoice.items} getRowId={(r) => r.id} maxHeight="none"
              empty={{ title: "No line items", compact: true }}
              footer={
                <dl className="ml-auto w-64 space-y-1.5 text-sm">
                  <div className="flex justify-between"><dt>Subtotal</dt><dd className="tabular text-fg">{fmt(invoice.subtotal + (invoice.discount_total ?? 0))}</dd></div>
                  {(invoice.discount_total ?? 0) > 0 && (
                    <div className="flex justify-between"><dt>Discount</dt><dd className="tabular text-fg">-{fmt(invoice.discount_total ?? 0)}</dd></div>
                  )}
                  {invoice.tax_amount > 0 && (
                    <div className="flex justify-between"><dt>Tax</dt><dd className="tabular text-fg">{fmt(invoice.tax_amount)}</dd></div>
                  )}
                  <div className="flex justify-between border-t border-line pt-2">
                    <dt className="font-semibold text-fg">Total</dt>
                    <dd className="text-base font-semibold tabular text-fg">{fmt(invoice.total)}</dd>
                  </div>
                </dl>
              } />
          </Card>

          {invoice.payments && invoice.payments.length > 0 && (
            <Card>
              <CardHeader title="Payment history"
                actions={<span className="text-xs text-fg-muted">{invoice.payments.length} payment{invoice.payments.length !== 1 ? "s" : ""}</span>} />
              <ul className="divide-y divide-line-soft">
                {invoice.payments.map((pay) => (
                  <li key={pay.id} className="flex items-center justify-between gap-3 px-4 py-3">
                    <div className="min-w-0">
                      <p className="text-sm font-medium text-fg">
                        {PAYMENT_METHODS[pay.payment_method] ?? pay.payment_method}
                        {pay.payment_reference && <span className="ml-2 font-mono text-xs text-fg-muted">{pay.payment_reference}</span>}
                      </p>
                      <p className="text-xs text-fg-muted">{fmtLongDate(pay.payment_date)}</p>
                      <JournalLink source="payment" sourceId={pay.id} />
                    </div>
                    <span className="font-semibold tabular text-fg">{fmt(pay.amount)}</span>
                  </li>
                ))}
              </ul>
            </Card>
          )}

          {invoice.refunds && invoice.refunds.length > 0 && (
            <Card>
              <CardHeader title="Refunds"
                actions={<span className="text-xs text-fg-muted">{invoice.refunds.length} refund{invoice.refunds.length !== 1 ? "s" : ""}</span>} />
              <ul className="divide-y divide-line-soft">
                {invoice.refunds.map((r) => (
                  <li key={r.id} className="flex items-center justify-between gap-3 px-4 py-3">
                    <div className="min-w-0">
                      <p className="text-sm font-medium text-fg">
                        Refund{r.reference && <span className="ml-2 font-mono text-xs text-fg-muted">{r.reference}</span>}
                      </p>
                      <p className="text-xs text-fg-muted">{fmtLongDate(r.paid_date)}</p>
                      <JournalLink source="refund" sourceId={r.id} />
                    </div>
                    <span className="font-semibold tabular text-fg">-{formatCurrency(r.amount, r.currency || cur)}</span>
                  </li>
                ))}
              </ul>
            </Card>
          )}

          {invoice.notes && (
            <Card>
              <CardHeader title="Notes" />
              <CardBody><p className="whitespace-pre-line text-sm leading-relaxed text-fg">{invoice.notes}</p></CardBody>
            </Card>
          )}
        </div>

        <div className="space-y-4">
          <InvoiceClientCard invoice={invoice} />

          <Card>
            <CardHeader title="Summary" />
            <CardBody>
              <dl className="space-y-2 text-sm">
                <div className="flex justify-between"><dt className="text-fg-muted">Total</dt><dd className="font-semibold tabular text-fg">{fmt(invoice.total)}</dd></div>
                <div className="flex justify-between"><dt className="text-fg-muted">Paid</dt><dd className="font-medium tabular text-fg">{fmt(invoice.amount_paid)}</dd></div>
                <div className="flex justify-between">
                  <dt className="text-fg-muted">Outstanding</dt>
                  <dd className={cn("font-medium tabular", invoice.amount_due > 0 ? "text-tone-orange" : "text-fg-muted")}>{fmt(invoice.amount_due)}</dd>
                </div>
              </dl>
              {invoice.total > 0 && (
                <div className="mt-4 border-t border-line-soft pt-4">
                  <div className="mb-1.5 flex justify-between text-xs text-fg-muted">
                    <span>Payment completion</span>
                    <span className="tabular">{Math.round(paidPct)}%</span>
                  </div>
                  {progress}
                </div>
              )}
            </CardBody>
          </Card>

          {canPay && (
            <Button variant="primary" size="lg" className="w-full" onClick={openPay}>
              <CreditCard aria-hidden /> Record payment
            </Button>
          )}
        </div>
      </div>

      {showReviseModal && (
        <ReasonModal
          title={`Revise ${invoice.full_number || invoice.number}`}
          description="This supersedes the current proforma and opens an editable draft with the next revision number."
          label="Reason for revision *"
          placeholder="e.g. Updated amounts, corrected line items..."
          confirmText="Create revision"
          onClose={() => setShowReviseModal(false)}
          onConfirm={async (reason) => {
            const res = await financeApi.reviseInvoice(id, reason);
            router.push(`/finance/invoices/${res.data.id}/edit`);
          }}
        />
      )}

      {showCreditModal && (
        <ReasonModal
          title={`Issue credit note for ${invoice.full_number || invoice.number}`}
          description="Creates a draft credit note. The original is cancelled when the credit note is sent (after approval)."
          label="Reason for credit note *"
          placeholder="e.g. Order cancelled, billing error, goods returned..."
          confirmText="Issue credit note"
          onClose={() => setShowCreditModal(false)}
          onConfirm={async (reason) => {
            const res = await financeApi.issueCreditNote(id, reason);
            router.push(`/finance/invoices/${res.data.id}`);
          }}
        />
      )}

      <Dialog open={showRefundModal} onOpenChange={setShowRefundModal} title="Record refund"
        description={<>Refund due: <strong className="tabular text-tone-red">{fmt(invoice.refund_due ?? 0)}</strong></>}
        footer={<>
          <Button onClick={() => setShowRefundModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={recordRefund} loading={actioning}
            disabled={!refundForm.amount || Number(refundForm.amount) <= 0 || Number(refundForm.amount) > (invoice.refund_due ?? 0) || !refundForm.paid_date}>
            Record refund
          </Button>
        </>}>
        <FieldGrid cols={1}>
          <Field label={`Amount (${cur})`} required>
            <Input type="number" numeric min="0" max={invoice.refund_due ?? undefined} value={refundForm.amount}
              onChange={(e) => setRefundForm({ ...refundForm, amount: e.target.value })} />
          </Field>
          <Field label="Paid on" required>
            <Input type="date" value={refundForm.paid_date} onChange={(e) => setRefundForm({ ...refundForm, paid_date: e.target.value })} />
          </Field>
          <Field label="Paid from">
            <BankAccountSelect value={refundForm.bank_account_id} currency={cur}
              onChange={(v) => setRefundForm({ ...refundForm, bank_account_id: v })} />
          </Field>
          <Field label="Reference">
            <Input value={refundForm.reference} onChange={(e) => setRefundForm({ ...refundForm, reference: e.target.value })}
              placeholder="Transfer ref..." />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={showPayModal} onOpenChange={setShowPayModal} title="Record payment"
        description={<>Outstanding: <strong className="tabular text-tone-orange">{fmt(invoice.amount_due)}</strong>{isForeignInv && <span className="ml-1 text-xs">({cur})</span>}</>}
        footer={<>
          <Button onClick={() => setShowPayModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={recordPayment} loading={actioning} disabled={!payForm.amount || !payForm.payment_date}>
            Record payment
          </Button>
        </>}>
        <FieldGrid cols={1}>
          <Field label="Amount" required>
            <Input type="number" numeric value={payForm.amount} onChange={(e) => setPayForm({ ...payForm, amount: e.target.value })} />
          </Field>
          <Field label="Payment date" required>
            <Input type="date" value={payForm.payment_date} onChange={(e) => setPayForm({ ...payForm, payment_date: e.target.value })} />
          </Field>
          {isForeignInv && (
            <Field label={`Exchange rate (1 ${cur} = ? ${functionalCur})`}>
              <Input type="number" numeric min="0" step="any" value={payForm.exchange_rate}
                onChange={(e) => setPayForm({ ...payForm, exchange_rate: e.target.value })}
                placeholder="No rate on record for this date" />
            </Field>
          )}
          <Field label="Method">
            <Select value={payForm.method} onChange={(e) => setPayForm({ ...payForm, method: e.target.value })}
              options={PAYMENT_METHOD_OPTIONS.map((m) => ({ value: m, label: m }))} />
          </Field>
          <Field label="Received into">
            <BankAccountSelect value={payForm.bank_account_id} currency={cur}
              onChange={(v) => setPayForm({ ...payForm, bank_account_id: v })} />
          </Field>
          <Field label="Reference">
            <Input value={payForm.reference} onChange={(e) => setPayForm({ ...payForm, reference: e.target.value })}
              placeholder="Transfer ref, check #..." />
          </Field>
          <Field label="Notes">
            <Textarea value={payForm.notes} onChange={(e) => setPayForm({ ...payForm, notes: e.target.value })}
              rows={2} placeholder="Optional notes..." className="resize-none" />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
