"use client";

import { OptionalTotals } from "@/components/sales/QuotationOptionalTotals";
import { useEffect, useState, useCallback } from "react";
import { useParams, useRouter } from "next/navigation";
import { approvalsApi, salesApi, financeApi } from "@/lib/api";
import { formatCurrency } from "@/lib/currency";
import { sanitizeHtml } from "@/lib/sanitize";
import { printQuotation } from "@/lib/print";
import { downloadVerifiedCopy } from "@/lib/verifiedDownload";
import {
  AlertTriangle, Ban, Building2, CalendarClock, CheckCircle2, Circle, CircleDashed, Copy, CornerDownRight, Download,
  FileCheck, Hash, Link2, Package, Pencil, Plus, RefreshCw, Send, Undo2, User, XCircle,
} from "lucide-react";
import Link from "next/link";
import ReasonModal from "@/components/ui/ReasonModal";
import ApprovalPanel from "@/components/approvals/ApprovalPanel";
import QuotationPricingCheck, { type PricingCheck } from "@/components/sales/QuotationPricingCheck";
import FollowUpButton from "@/components/crm/FollowUpButton";
import QuotationLinkDeal from "@/components/sales/QuotationLinkDeal";
import { usePermissions } from "@/hooks/usePermissions";
import { useSendDocument } from "@/components/approvals/useSendDocument";
import { toast } from "sonner";
import {
  Button, Card, CardBody, CardHeader, DataTable, Dialog, EmptyState, Field, IconButton, Input, KeyValues,
  PageHeader, Pill, Select, Skeleton, StatusPill, type ColumnDef, type Tone,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { extractApiError } from "@/lib/apiError";
import { lineTermText, orderLineTree } from "@/lib/catalogPricing";
import type { TaxLine } from "@/lib/types/catalog";

interface LineItem {
  id: string;
  parent_item_id?: string | null;
  product_id?: string;
  /** Snapshot SKU of the line (the product's SKU is the fallback for older lines). */
  sku?: string | null;
  name?: string;
  description: string;
  qty: number;
  unit?: string | null;
  unit_price: number;
  discount_pct: number;
  discount_type?: string | null;
  is_optional?: boolean;
  sale_model?: string | null;
  price_metric?: string | null;
  rate_period_months?: number | null;
  billing_interval_months?: number | null;
  term_months?: number | null;
  price_rule?: string | null;
  tax_id?: string | null;
  tax_code?: string | null;
  tax_pct: number;
  tax_amount?: number | null;
  subtotal: number;
  product?: { name: string; sku?: string; terms_note?: string | null; price_status?: string | null };
}

interface Quotation {
  id: string;
  number: string;
  full_number: string;
  revision: number;
  subject?: string;
  status: string;
  /** 17 Nov 2026: "approved" for a draft whose approval still matches it; otherwise the status. */
  display_status?: string;
  total: number;
  subtotal: number;
  tax_amount: number;
  discount_amount?: number;
  currency?: string;
  exchange_rate?: number;
  functional_currency?: string;
  issue_date: string;
  valid_until?: string;
  notes?: string;
  terms?: string;
  customer_po_number?: string | null;
  customer_po_date?: string | null;
  prepared_by_id?: string;
  customer_org_id?: string;
  customer_contact_id?: string;
  /** 5 Nov 2026: the linked deal (Link to deal links a sent quotation without a revision). */
  deal_id?: string | null;
  customer_org?: { id: string; name: string; email?: string };
  customer_contact?: { id: string; full_name: string; email?: string };
  prepared_by?: { id: string; full_name?: string; email: string };
  items: LineItem[];
  tax_lines?: TaxLine[] | null;
  payment_terms?: { text?: string; schedule?: { label: string; percentage: number; due_trigger: string }[] } | null;
  revision_group?: string;
  active_revision_id?: string | null;
  revision_reason?: string | null;
  revisions?: RevisionRow[];
  /** 1 Nov 2026: why the header discount was given; pdf = a Discount line, notes_only = stated in the notes. */
  discount_note?: string | null;
  discount_display?: "pdf" | "notes_only" | null;
  cancelled_at?: string | null;
  cancel_reason?: string | null;
  /** 4 Nov 2026: optional lines are not in the total; their own figures. optional_in_total = an older
   *  quotation whose stored total still counts them. */
  optional_subtotal?: number;
  optional_tax?: number;
  optional_total?: number;
  total_with_optional?: number;
  optional_in_total?: boolean;
  /** 5 Nov 2026 (E7): the pricing rules (blocking, warnings, the Owner's override). */
  pricing_check?: PricingCheck | null;
}

interface RevisionRow {
  id: string; full_number: string; revision: number; status: string;
  created_at?: string | null; revision_reason?: string | null; is_current?: boolean;
}

interface Schedule {
  id: string; name: string; sequence: number;
  percentage: number | null; amount: number | null; status: string;
  invoice_id?: string | null; proforma_invoice_id?: string | null;
}

const STATUS_LABELS: Record<string, string> = {
  draft: "Draft", approved: "Approved", sent: "Sent", accepted: "Accepted", signed: "PO signed", rejected: "Rejected",
  expired: "Expired", superseded: "Superseded", cancelled: "Cancelled",
};

const fmtLongDate = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

const BANNER: Record<Tone, string> = {
  neutral: "border-line bg-surface-2", blue: "border-tone-blue/30 bg-tone-blue-soft",
  violet: "border-tone-violet/30 bg-tone-violet-soft", orange: "border-tone-orange/30 bg-tone-orange-soft",
  red: "border-tone-red/30 bg-tone-red-soft",
};
const BANNER_INK: Record<Tone, string> = {
  neutral: "text-fg-muted", blue: "text-tone-blue", violet: "text-tone-violet", orange: "text-tone-orange", red: "text-tone-red",
};

function Banner({ tone, icon: Icon, children, action }: { tone: Tone; icon: React.ElementType; children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div role="status" className={cn("mb-4 flex flex-wrap items-center gap-3 rounded-lg border px-4 py-3 text-fg", BANNER[tone])}>
      <Icon className={cn("size-4 shrink-0", BANNER_INK[tone])} aria-hidden />
      <p className="min-w-0 flex-1 text-sm">{children}</p>
      {action}
    </div>
  );
}

export default function QuotationDetailPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const [quote, setQuote] = useState<Quotation | null>(null);
  const [loading, setLoading] = useState(true);
  const [actioning, setActioning] = useState(false);
  const [showPoModal, setShowPoModal] = useState(false);
  const [poNumber, setPoNumber] = useState("");
  const [showConvertModal, setShowConvertModal] = useState(false);
  const [convertDueDate, setConvertDueDate] = useState("");
  const [convertError, setConvertError] = useState("");
  const [error, setError] = useState("");
  const [actionError, setActionError] = useState("");
  const [approvalKey, setApprovalKey] = useState(0);
  const [downloading, setDownloading] = useState(false);
  const [showReviseModal, setShowReviseModal] = useState(false);
  // 1 Nov 2026: withdraw from approval, cancel, reprice to today
  const [approvalPending, setApprovalPending] = useState(false);
  const [showCancelModal, setShowCancelModal] = useState(false);
  const [showRepriceModal, setShowRepriceModal] = useState(false);
  const [repriceChanges, setRepriceChanges] = useState<string[] | null>(null);
  // 5 Nov 2026: link a quotation that is no longer an editable draft to a deal, without a revision
  const [showLinkDeal, setShowLinkDeal] = useState(false);
  const { can } = usePermissions();
  const [schedules, setSchedules] = useState<Schedule[]>([]);
  const [showSchedEditor, setShowSchedEditor] = useState(false);
  const [schedRows, setSchedRows] = useState<{ name: string; percentage: string; due_trigger: string }[]>([]);

  /** Start from the saved schedule, else the payment terms suggested on the quotation, else one blank row. */
  function openSchedEditor() {
    if (schedules.length) {
      setSchedRows(schedules.map((s) => ({ name: s.name, percentage: s.percentage != null ? String(s.percentage) : "", due_trigger: "on_milestone" })));
    } else if (quote?.payment_terms?.schedule?.length) {
      setSchedRows(quote.payment_terms.schedule.map((s) => ({ name: s.label, percentage: String(s.percentage), due_trigger: s.due_trigger })));
    } else {
      setSchedRows([{ name: "", percentage: "", due_trigger: "on_order" }]);
    }
    setShowSchedEditor(true);
  }

  function openConvertModal() {
    setConvertError("");
    setShowConvertModal(true);
  }

  const loadSchedules = useCallback(() => {
    financeApi.listSchedules({ quotation_id: id })
      .then((r) => setSchedules(r.data ?? [])).catch(() => setSchedules([]));
  }, [id]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await salesApi.getQuotation(id);
      setQuote(res.data);
      loadSchedules();
    } catch {
      setError("Failed to load quotation.");
    } finally {
      setLoading(false);
    }
  }, [id, loadSchedules]);

  useEffect(() => { load(); }, [load]);

  // Whether an approval request waits (Withdraw is offered then); the panel below shows the rest.
  useEffect(() => {
    let live = true;
    approvalsApi.forEntity("quotation", id)
      .then((r) => { if (live) setApprovalPending(r.data?.request?.status === "pending"); })
      .catch(() => { if (live) setApprovalPending(false); });
    return () => { live = false; };
  }, [id, approvalKey, quote?.status, quote?.issue_date]);

  async function withdrawFromApproval() {
    setActioning(true);
    setActionError("");
    try {
      const res = await salesApi.withdrawQuotation(id);
      toast.success(res.data?.label || "Withdrawn from approval.");
      await load();
    } catch (e) {
      setActionError(extractApiError(e, "Could not withdraw it from approval."));
    } finally {
      setActioning(false);
      setApprovalKey((k) => k + 1);
    }
  }

  async function repriceToToday() {
    setActioning(true);
    setActionError("");
    try {
      const res = await salesApi.repriceQuotation(id);
      setShowRepriceModal(false);
      setRepriceChanges(Array.isArray(res.data?.changes) ? res.data.changes : []);
      await load();
    } catch (e) {
      setShowRepriceModal(false);
      setActionError(extractApiError(e, "Could not reprice the quotation."));
    } finally {
      setActioning(false);
      setApprovalKey((k) => k + 1);
    }
  }

  async function updateStatus(status: string) {
    if (!quote) return;
    setActioning(true);
    setActionError("");
    try {
      await salesApi.updateStatus(id, status);
      await load();
    } catch (e) {
      setActionError(extractApiError(e, "Could not update the quotation."));
    } finally {
      setActioning(false);
      setApprovalKey((k) => k + 1);
    }
  }

  async function downloadPdf() {
    if (!quote) return;
    setDownloading(true);
    try {
      setActionError("");
      const result = await downloadVerifiedCopy("quotation", quote.id, () => printQuotation(quote),
        { requireApproval: true, label: "quotation" });
      if (result.outcome === "refused") setActionError(result.message);
    } finally {
      setDownloading(false);
      setApprovalKey((k) => k + 1);   // a copy may have just been issued
    }
  }

  // Send: submits for approval when needed; once approved, emails / WhatsApps the verified PDF.
  const sender = useSendDocument({
    entityType: "quotation", label: "Quotation",
    onChanged: () => { load(); setApprovalKey((k) => k + 1); },
    onMessage: (kind, message) => {
      if (kind === "success") { setActionError(""); toast.success(message); }
      else setActionError(message);
    },
  });

  function sendQuotation() {
    if (!quote) return;
    setActionError("");
    sender.start(quote.id);
  }


  async function recordPO() {
    if (!poNumber.trim()) return;
    setActioning(true);
    try {
      await salesApi.recordPO(id, { po_number: poNumber.trim() });
      setShowPoModal(false);
      setPoNumber("");
      await load();
    } finally {
      setActioning(false);
    }
  }

  async function generateInvoice(mode: "full" | "milestone", scheduleId?: string) {
    setActioning(true);
    try {
      const res = await salesApi.generateInvoiceFromQuotation(id, {
        invoice_type: "commercial",
        due_date: convertDueDate || undefined,
        mode,
        schedule_id: scheduleId,
      });
      setShowConvertModal(false);
      router.push(`/finance/invoices/${res.data.id}`);
    } catch (e) {
      // Inline, not the page-level error: that one replaces the whole page.
      setConvertError(extractApiError(e, "Could not generate the invoice."));
    } finally {
      setActioning(false);
    }
  }

  async function saveSchedule() {
    const milestones = schedRows
      .filter((r) => r.name.trim() && parseFloat(r.percentage) > 0)
      .map((r, i) => ({ name: r.name.trim(), percentage: parseFloat(r.percentage), due_trigger: r.due_trigger, sequence: i + 1 }));
    if (milestones.length === 0) return;
    setActioning(true);
    try {
      await financeApi.createSchedule({ quotation_id: id, milestones });
      setShowSchedEditor(false);
      loadSchedules();
    } finally {
      setActioning(false);
    }
  }

  if (loading) return (
    <div className="mx-auto max-w-5xl space-y-3" aria-busy="true">
      <Skeleton className="h-8 w-72" />
      <Skeleton className="h-40 w-full" />
      <Skeleton className="h-64 w-full" />
    </div>
  );

  if (error || !quote) return (
    <Card className="mx-auto max-w-xl">
      <EmptyState icon={AlertTriangle} title={error || "Quotation not found."}
        action={<Button asChild><Link href="/sales/quotations">Back to quotations</Link></Button>} />
    </Card>
  );

  const cur = quote.currency ?? "USD";
  const functionalCur = quote.functional_currency ?? "USD";
  const isForeignQ = cur !== functionalCur && !!quote.exchange_rate;
  const fmt = (n: number) => formatCurrency(n, cur);
  const isExpiring = quote.valid_until && quote.status === "sent" &&
    new Date(quote.valid_until) < new Date(Date.now() + 3 * 86400000);
  const lines = orderLineTree(quote.items.map((it) => ({ ...it, key: it.id, parent_key: it.parent_item_id ?? null })));
  const billed = schedules.filter((s) => s.status === "invoiced" || s.invoice_id || s.proforma_invoice_id)
    .reduce((sum, s) => sum + (s.amount ?? (s.percentage != null ? quote.total * s.percentage / 100 : 0)), 0);
  const setRow = (i: number, patch: Partial<{ name: string; percentage: string; due_trigger: string }>) =>
    setSchedRows((p) => p.map((x, idx) => (idx === i ? { ...x, ...patch } : x)));
  const flowMain = ["draft", "sent", "accepted", "signed"];

  const lineColumns: ColumnDef<(typeof lines)[number]>[] = [
    {
      id: "item", header: "Item", enableSorting: false,
      cell: (c) => {
        const { line: item, depth } = c.row.original;
        const sku = item.sku || item.product?.sku;
        const term = lineTermText(item);
        const note = item.product?.terms_note;
        return (
          <div style={{ paddingLeft: depth * 20 }} className="py-1">
            <p className="font-medium text-fg">
              {depth > 0 && <CornerDownRight className="mr-1 inline size-3.5 text-fg-subtle" aria-hidden />}
              {item.name || item.product?.name || "-"}
              {item.is_optional && <Pill className="ml-2">Optional</Pill>}
              {item.product?.price_status === "proposed" && (
                <Pill tone="orange" className="ml-2" title="The catalog price is proposed, not locked: the Owner approves this quotation">Proposed price</Pill>
              )}
            </p>
            {sku && (
              <p className="mt-0.5 flex items-center gap-1.5 text-xs text-fg-muted">
                <Package className="size-3.5 shrink-0 text-fg-subtle" aria-hidden />
                <span className="font-mono">{sku}</span>
              </p>
            )}
            {term && <p className="mt-0.5 text-xs text-fg-muted">{term}</p>}
            {item.description && <p className="mt-0.5 whitespace-pre-line text-xs text-fg-muted">{item.description}</p>}
            {note && <p className="mt-1 whitespace-pre-line text-[11px] italic text-fg-subtle">{note}</p>}
          </div>
        );
      },
    },
    { id: "qty", header: "Qty", enableSorting: false, meta: { numeric: true }, cell: (c) => c.row.original.line.qty },
    { id: "unit_price", header: "Unit price", enableSorting: false, meta: { numeric: true }, cell: (c) => fmt(c.row.original.line.unit_price) },
    {
      id: "disc", header: "Disc", enableSorting: false, meta: { numeric: true, hideOnMobile: true },
      cell: (c) => c.row.original.line.discount_pct > 0
        ? <span className="font-medium text-tone-orange">{c.row.original.line.discount_pct}%</span>
        : <span className="text-fg-subtle">-</span>,
    },
    {
      id: "tax", header: "Tax", enableSorting: false, meta: { numeric: true, hideOnMobile: true },
      cell: (c) => {
        const item = c.row.original.line;
        return item.tax_pct > 0 ? (
          <span title={item.tax_amount ? fmt(item.tax_amount) : undefined}>
            {item.tax_code && item.tax_code !== "CUSTOM" ? `${item.tax_code} ` : ""}{item.tax_pct}%
          </span>
        ) : <span className="text-fg-subtle">-</span>;
      },
    },
    { id: "subtotal", header: "Subtotal", enableSorting: false, meta: { numeric: true }, cell: (c) => (
      <span className="font-medium">
        {fmt(c.row.original.line.subtotal)}
        {c.row.original.line.is_optional && !quote.optional_in_total && <span className="block text-[11px] font-normal text-fg-muted">not in the total</span>}
      </span>
    ) },
  ];

  const busy = actioning || !!sender.preparing;

  return (
    <div className="mx-auto max-w-5xl">
      <PageHeader back={{ href: "/sales/quotations", label: "Quotations" }}
        title={<span className="whitespace-nowrap font-mono">{quote.full_number || quote.number}</span>}
        status={<StatusPill family="quotation" status={quote.display_status ?? quote.status} label={STATUS_LABELS[quote.display_status ?? quote.status]} tone={quote.status === "signed" ? "blue" : undefined} />}
        summary={<span>{quote.subject ? `${quote.subject}, ` : ""}{fmt(quote.total)}</span>}
        actions={<>
          {quote.status === "draft" && (
            <>
              {approvalPending ? (
                <Button onClick={withdrawFromApproval} disabled={busy}
                  title="Take it back from the approver to change it (the person who submitted it or the Owner)">
                  <Undo2 aria-hidden /> Withdraw from approval
                </Button>
              ) : (
                <>
                  <Button asChild><Link href={`/sales/quotations/${quote.id}/edit`}><Pencil aria-hidden /> Edit</Link></Button>
                  <Button onClick={() => setShowRepriceModal(true)} disabled={busy}><CalendarClock aria-hidden /> Reprice to today</Button>
                </>
              )}
              <Button onClick={() => setShowCancelModal(true)} disabled={busy}><Ban aria-hidden /> Cancel quotation</Button>
              <Button variant="primary" onClick={sendQuotation} disabled={busy}><Send aria-hidden /> Send to client</Button>
            </>
          )}
          {quote.status === "sent" && (
            <>
              <Button onClick={sendQuotation} disabled={busy}><RefreshCw aria-hidden /> Resend</Button>
              <Button onClick={() => setShowReviseModal(true)} disabled={actioning}><Copy aria-hidden /> Revise</Button>
              <Button onClick={() => updateStatus("rejected")} disabled={actioning}><XCircle aria-hidden /> Mark rejected</Button>
              <Button variant="primary" onClick={() => updateStatus("accepted")} disabled={actioning}><CheckCircle2 aria-hidden /> Mark accepted</Button>
            </>
          )}
          {(quote.status === "accepted" || quote.status === "signed") && (
            <>
              {!quote.customer_po_number && (
                <Button onClick={() => setShowPoModal(true)}><Hash aria-hidden /> Record PO</Button>
              )}
              <Button variant="primary" onClick={openConvertModal}><FileCheck aria-hidden /> Convert to invoice</Button>
            </>
          )}
          {(quote.status === "expired" || quote.status === "rejected") && (
            <Button onClick={() => setShowReviseModal(true)} disabled={actioning}><Copy aria-hidden /> Revise</Button>
          )}
          {quote.status !== "draft" && quote.status !== "cancelled" && quote.status !== "superseded"
            && can("sales:create_quotation") && (
            <Button onClick={() => setShowLinkDeal(true)} disabled={busy}
              title="Link it (and its revisions) to a deal without a revision; the customer document does not change">
              <Link2 aria-hidden /> {quote.deal_id ? "Change deal" : "Link to deal"}
            </Button>
          )}
          <FollowUpButton compact entityType="quotation" entityId={quote.id}
            subject={quote.full_number || quote.number} />
          <Button onClick={downloadPdf} loading={downloading}>{!downloading && <Download aria-hidden />} PDF</Button>
        </>} />

      {actionError && (
        <Banner tone="orange" icon={AlertTriangle}
          action={<Button size="sm" variant="ghost" onClick={() => setActionError("")}>Dismiss</Button>}>
          <span className="font-medium">{actionError}</span>
        </Banner>
      )}

      <ApprovalPanel entityType="quotation" entityId={quote.id} refreshKey={approvalKey} onChange={load} />
      <QuotationPricingCheck quotationId={quote.id} number={quote.full_number || quote.number}
        status={quote.status} check={quote.pricing_check} onChange={load} />
      {sender.modal}

      {quote.status === "cancelled" && (
        <Banner tone="neutral" icon={Ban}>
          This quotation was <strong>cancelled</strong>
          {quote.cancelled_at ? ` on ${fmtLongDate(quote.cancelled_at)}` : ""}
          {quote.cancel_reason ? `: ${quote.cancel_reason}` : ""}. It is kept for history and can&apos;t be sent.
        </Banner>
      )}

      {repriceChanges && (
        <Banner tone="blue" icon={CalendarClock}
          action={<Button size="sm" variant="ghost" onClick={() => setRepriceChanges(null)}>Dismiss</Button>}>
          <span className="font-medium">Repriced to today; it goes to approval again.</span>
          {repriceChanges.length > 0 && (
            <span className="mt-1 block whitespace-pre-wrap text-xs text-fg-muted">{repriceChanges.join("\n")}</span>
          )}
        </Banner>
      )}

      {quote.status === "superseded" && (
        <Banner tone="violet" icon={Copy}
          action={quote.active_revision_id && (
            <Button size="sm" asChild><Link href={`/sales/quotations/${quote.active_revision_id}`}>Go to current revision</Link></Button>
          )}>
          This is an older revision and has been <strong>superseded</strong>. It is read-only.
        </Banner>
      )}

      {isExpiring && (
        <Banner tone="orange" icon={AlertTriangle}>
          This quotation expires on <strong>{new Date(quote.valid_until!).toLocaleDateString("en-GB")}</strong>. Follow up with the client soon.
        </Banner>
      )}

      {quote.revisions && quote.revisions.length > 1 && (
        <Card className="mb-4">
          <CardHeader title="Revisions" />
          <ul className="divide-y divide-line-soft">
            {quote.revisions.map((r) => (
              <li key={r.id}>
                <Link href={`/sales/quotations/${r.id}`}
                  className={cn("flex min-h-11 items-center gap-3 px-4 py-2 text-sm transition-colors hover:bg-surface-2",
                    r.is_current && "bg-brand/[0.07]")}>
                  <span className="font-mono font-medium text-fg">{r.full_number}</span>
                  <StatusPill family="quotation" status={r.status} label={STATUS_LABELS[r.status]} tone={r.status === "signed" ? "blue" : undefined} />
                  {r.revision_reason && <span className="min-w-0 flex-1 truncate text-xs text-fg-muted">{r.revision_reason}</span>}
                  <span className="ml-auto shrink-0 text-xs text-fg-muted tabular">{r.created_at ? new Date(r.created_at).toLocaleDateString("en-GB") : ""}</span>
                </Link>
              </li>
            ))}
          </ul>
        </Card>
      )}

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <div className="space-y-4 lg:col-span-2">
          <Card>
            <CardHeader title={quote.subject || "Quotation"}
              actions={<div className="text-right">
                <p className="text-xs text-fg-muted">Total amount</p>
                <p className="text-lg font-semibold text-fg tabular">{fmt(quote.total)}</p>
                {isForeignQ && <p className="text-xs text-fg-muted tabular">1 {cur} = {formatCurrency(quote.exchange_rate!, functionalCur)}</p>}
              </div>} />
            <CardBody>
              <KeyValues cols={3} items={[
                { label: "Issue date", value: fmtLongDate(quote.issue_date) },
                ...(quote.valid_until ? [{
                  label: "Valid until",
                  value: <span className={cn(isExpiring && "font-medium text-tone-orange")}>{fmtLongDate(quote.valid_until)}</span>,
                }] : []),
                ...(quote.customer_po_number ? [{ label: "Customer PO", value: <span className="font-mono">{quote.customer_po_number}</span> }] : []),
              ]} />
            </CardBody>
          </Card>

          <Card>
            <CardHeader title="Line items" />
            <DataTable label="Line items" columns={lineColumns} data={lines} getRowId={(r) => r.line.id} maxHeight="none"
              empty={{ title: "No line items", compact: true }}
              footer={
                <div className="ml-auto w-full max-w-xs space-y-2 px-4 py-3 text-sm">
                  <div className="flex justify-between text-fg-muted">
                    <span>Subtotal</span><span className="font-medium text-fg tabular">{fmt(quote.subtotal)}</span>
                  </div>
                  {(quote.discount_amount ?? 0) > 0 && (
                    <div className="text-tone-orange">
                      <div className="flex justify-between">
                        <span>Discount</span><span className="font-medium tabular">-{fmt(quote.discount_amount!)}</span>
                      </div>
                      {quote.discount_note && quote.discount_display !== "notes_only" && (
                        <p className="text-right text-xs text-fg-muted">{quote.discount_note}</p>
                      )}
                    </div>
                  )}
                  {quote.discount_note && quote.discount_display === "notes_only" && (
                    <p className="text-xs text-fg-muted">Discount stated in the notes only (totals stay full): {quote.discount_note}</p>
                  )}
                  {(quote.tax_lines ?? []).length > 0 ? (quote.tax_lines ?? []).map((t, i) => (
                    <div key={`${t.tax_id ?? t.code}-${i}`} className="flex justify-between text-fg-muted">
                      <span>{t.name || t.code || "Tax"}{(t.pct ?? t.rate) != null ? ` (${t.pct ?? t.rate}%)` : ""}</span>
                      <span className="font-medium text-fg tabular">{fmt(t.amount)}</span>
                    </div>
                  )) : quote.tax_amount > 0 && (
                    <div className="flex justify-between text-fg-muted">
                      <span>Tax</span><span className="font-medium text-fg tabular">{fmt(quote.tax_amount)}</span>
                    </div>
                  )}
                  <div className="flex justify-between border-t border-line pt-2 font-semibold text-fg">
                    <span>Total</span><span className="text-base tabular">{fmt(quote.total)}</span>
                  </div>
                  <OptionalTotals quote={quote} fmt={fmt} />
                </div>
              } />
          </Card>

          <Card>
            <CardHeader title="Payment schedule"
              actions={!showSchedEditor && (
                <Button size="sm" variant="ghost" onClick={openSchedEditor}>
                  {schedules.length > 0 ? "Edit splits" : <><Plus aria-hidden /> Add milestones</>}
                </Button>
              )} />
            <CardBody>
              {showSchedEditor ? (
                <div className="space-y-2">
                  {schedRows.map((r, i) => (
                    <div key={i} className="flex flex-wrap items-center gap-2">
                      <Input value={r.name} onChange={(e) => setRow(i, { name: e.target.value })}
                        placeholder="Milestone name" aria-label="Milestone name" className="min-w-40 flex-1" />
                      <div className="w-24">
                        <Input type="number" numeric value={r.percentage} onChange={(e) => setRow(i, { percentage: e.target.value })}
                          placeholder="%" aria-label="Percentage" trailing="%" />
                      </div>
                      <div className="w-40">
                        <Select value={r.due_trigger} onChange={(e) => setRow(i, { due_trigger: e.target.value })} aria-label="Due trigger"
                          options={[
                            { value: "on_order", label: "On order" },
                            { value: "on_delivery", label: "On delivery" },
                            { value: "on_milestone", label: "On milestone" },
                            { value: "on_date", label: "On date" },
                          ]} />
                      </div>
                      <IconButton icon={XCircle} label="Remove milestone" size="sm" onClick={() => setSchedRows((p) => p.filter((_, idx) => idx !== i))} />
                    </div>
                  ))}
                  <Button size="sm" variant="ghost" onClick={() => setSchedRows((p) => [...p, { name: "", percentage: "", due_trigger: "on_milestone" }])}>
                    <Plus aria-hidden /> Add row
                  </Button>
                  <div className="flex items-center gap-2 pt-2">
                    <span className="text-xs text-fg-muted tabular">Total: {schedRows.reduce((s, r) => s + (parseFloat(r.percentage) || 0), 0)}%</span>
                    <span className="flex-1" />
                    <Button size="sm" onClick={() => setShowSchedEditor(false)}>Cancel</Button>
                    <Button size="sm" variant="primary" onClick={saveSchedule} loading={actioning}>Save schedule</Button>
                  </div>
                </div>
              ) : schedules.length === 0 ? (
                <p className="text-sm text-fg-muted">No payment schedule. Add milestones to bill this quotation in installments.</p>
              ) : (
                <div className="space-y-1.5">
                  {schedules.map((s) => {
                    const amt = s.amount ?? (s.percentage != null ? quote.total * s.percentage / 100 : 0);
                    const invoiced = s.status === "invoiced" || s.invoice_id || s.proforma_invoice_id;
                    return (
                      <div key={s.id} className="flex items-center justify-between gap-3 py-1.5">
                        <div className="min-w-0">
                          <p className="truncate text-sm font-medium text-fg">{s.name} {s.percentage != null && <span className="text-fg-muted">, {s.percentage}%</span>}</p>
                          <p className="text-xs text-fg-muted tabular">{fmt(amt)}</p>
                        </div>
                        {invoiced ? (
                          <StatusPill family="schedule" status="invoiced" label="Invoiced" />
                        ) : (
                          <Button size="sm" onClick={openConvertModal} disabled={actioning}>Generate invoice</Button>
                        )}
                      </div>
                    );
                  })}
                  <div className="mt-1 flex items-center justify-between border-t border-line pt-2 text-xs">
                    <span className="text-fg-muted">Billed</span>
                    <span className="font-medium text-fg tabular">{fmt(billed)} <span className="text-fg-muted">/ {fmt(quote.total)}</span></span>
                  </div>
                </div>
              )}
            </CardBody>
          </Card>

          {(quote.notes || quote.terms) && (
            <Card>
              <CardBody className="space-y-4">
                {quote.notes && (
                  <div>
                    <h3 className="mb-2 text-sm font-semibold text-fg">Notes</h3>
                    <div className="text-sm leading-relaxed text-fg [&_a]:text-brand-ink [&_a]:underline [&_ol]:list-decimal [&_ol]:pl-5 [&_ul]:list-disc [&_ul]:pl-5"
                      dangerouslySetInnerHTML={{ __html: sanitizeHtml(quote.notes) }} />
                  </div>
                )}
                {quote.terms && (
                  <div>
                    <h3 className="mb-2 text-sm font-semibold text-fg">Terms and conditions</h3>
                    <p className="whitespace-pre-line text-sm leading-relaxed text-fg">{quote.terms}</p>
                  </div>
                )}
              </CardBody>
            </Card>
          )}
        </div>

        <div className="space-y-4">
          <Card>
            <CardHeader title="Client" />
            <CardBody className="space-y-3">
              {quote.customer_org && (
                <div className="flex items-start gap-3">
                  <Building2 className="mt-0.5 size-4 shrink-0 text-fg-muted" aria-hidden />
                  <div className="min-w-0">
                    <p className="text-sm font-medium text-fg">{quote.customer_org.name}</p>
                    {quote.customer_org.email && <p className="text-xs text-fg-muted">{quote.customer_org.email}</p>}
                  </div>
                </div>
              )}
              {quote.customer_contact && (
                <div className="flex items-start gap-3">
                  <User className="mt-0.5 size-4 shrink-0 text-fg-muted" aria-hidden />
                  <div className="min-w-0">
                    <p className="text-sm font-medium text-fg">{quote.customer_contact.full_name}</p>
                    {quote.customer_contact.email && <p className="text-xs text-fg-muted">{quote.customer_contact.email}</p>}
                  </div>
                </div>
              )}
              {!quote.customer_org && !quote.customer_contact && <p className="text-sm text-fg-muted">No client assigned</p>}
            </CardBody>
          </Card>

          {quote.prepared_by && (
            <Card>
              <CardHeader title="Prepared by" />
              <CardBody className="flex items-center gap-3">
                <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-surface-2 text-xs font-semibold text-fg">
                  {(quote.prepared_by.full_name ?? quote.prepared_by.email).charAt(0).toUpperCase()}
                </span>
                <div className="min-w-0">
                  <p className="text-sm font-medium text-fg">{quote.prepared_by.full_name ?? quote.prepared_by.email}</p>
                  <p className="truncate text-xs text-fg-muted">{quote.prepared_by.email}</p>
                </div>
              </CardBody>
            </Card>
          )}

          <Card>
            <CardHeader title="Summary" />
            <CardBody>
              <dl className="space-y-2.5 text-sm">
                {[
                  { label: "Currency", value: cur + (isForeignQ ? ` (1 ${cur} = ${formatCurrency(quote.exchange_rate!, functionalCur)})` : "") },
                  { label: "Items", value: quote.items.length.toString() },
                  { label: "Subtotal", value: fmt(quote.subtotal) },
                  { label: "Tax", value: fmt(quote.tax_amount) },
                  { label: "Total", value: fmt(quote.total), highlight: true },
                  ...(quote.items.some((i) => i.is_optional) && !quote.optional_in_total
                    ? [{ label: "With optional items", value: fmt(quote.total_with_optional ?? quote.total), highlight: false }]
                    : []),
                ].map((row) => (
                  <div key={row.label} className={cn("flex items-center justify-between gap-3", row.highlight && "border-t border-line pt-2.5 font-semibold")}>
                    <dt className={row.highlight ? "text-fg" : "text-fg-muted"}>{row.label}</dt>
                    <dd className="text-right text-fg tabular">{row.value}</dd>
                  </div>
                ))}
              </dl>
            </CardBody>
          </Card>

          <Card>
            <CardHeader title="Status flow" />
            <ol className="space-y-1 p-2">
              {(["draft", "sent", "accepted", "signed", "rejected", "expired"] as const).map((s) => {
                const isCurrent = quote.status === s;
                const isPast = flowMain.includes(s) && flowMain.indexOf(quote.status) > flowMain.indexOf(s);
                const Icon = isPast ? CheckCircle2 : isCurrent ? Circle : CircleDashed;
                return (
                  <li key={s} aria-current={isCurrent ? "step" : undefined}
                    className={cn("flex items-center gap-2.5 rounded-lg px-3 py-2 text-sm",
                      isCurrent ? "bg-brand/[0.07] font-semibold text-fg" : isPast ? "text-fg" : "text-fg-subtle")}>
                    <Icon className={cn("size-3.5 shrink-0", isCurrent ? "text-brand-ink" : isPast ? "text-tone-blue" : "text-fg-subtle")} aria-hidden />
                    {STATUS_LABELS[s]}
                  </li>
                );
              })}
            </ol>
          </Card>
        </div>
      </div>

      {showReviseModal && (
        <ReasonModal
          title={`Revise ${quote.full_number || quote.number}`}
          description="This supersedes the current quotation and opens an editable draft with the next revision number."
          label="Reason for revision *"
          placeholder="e.g. Updated pricing, added item, corrected quantities"
          confirmText="Create revision"
          onClose={() => setShowReviseModal(false)}
          onConfirm={async (reason) => {
            const res = await salesApi.reviseQuotation(id, reason);
            router.push(`/sales/quotations/${res.data.id}/edit`);
          }}
        />
      )}

      {showCancelModal && (
        <ReasonModal
          title={`Cancel ${quote.full_number || quote.number}`}
          description="Its approval request is withdrawn and it leaves proposals still being written. It is hidden from the list, can never be sent and is kept for history."
          label="Why is it cancelled? *"
          placeholder="e.g. Customer chose another option; replaced by a new quotation"
          confirmText="Cancel quotation"
          confirmVariant="danger"
          onClose={() => setShowCancelModal(false)}
          onConfirm={async (reason) => {
            await salesApi.cancelQuotation(id, reason);
            setShowCancelModal(false);
            setApprovalKey((k) => k + 1);
            await load();
          }}
        />
      )}

      <QuotationLinkDeal open={showLinkDeal} onClose={() => setShowLinkDeal(false)}
        quotationId={quote.id} number={quote.full_number || quote.number}
        orgId={quote.customer_org_id ?? quote.customer_org?.id ?? null} currentDealId={quote.deal_id ?? null}
        onLinked={async () => { toast.success("Linked to the deal."); await load(); }} />

      <Dialog open={showRepriceModal} onOpenChange={setShowRepriceModal} title="Reprice to today"
        description="The issue date becomes today and the validity counts from today. Catalog lines take today's catalog price; typed (manual or custom) lines keep their price. It then goes to approval again."
        footer={<>
          <Button onClick={() => setShowRepriceModal(false)}>Back</Button>
          <Button variant="primary" onClick={repriceToToday} loading={actioning}>Reprice</Button>
        </>}>
        <p className="text-sm text-fg-muted">Each line&apos;s price before and after is shown once it is done.</p>
      </Dialog>

      <Dialog open={showPoModal} onOpenChange={setShowPoModal} title="Record customer PO"
        footer={<>
          <Button onClick={() => setShowPoModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={recordPO} disabled={!poNumber.trim()} loading={actioning}>Save PO</Button>
        </>}>
        <Field label="PO number">
          <Input value={poNumber} onChange={(e) => setPoNumber(e.target.value)} placeholder="e.g. PO-2024-0042" className="font-mono" />
        </Field>
      </Dialog>

      <Dialog open={showConvertModal} onOpenChange={setShowConvertModal} title="Create invoice"
        description="Bill the full quotation, or a single payment milestone."
        footer={<Button onClick={() => setShowConvertModal(false)}>Cancel</Button>}>
        <div className="space-y-4">
          <Field label="Payment due date (optional)" hint="Each line keeps the tax set on the quotation; you can change it on the draft invoice.">
            <Input type="date" value={convertDueDate} onChange={(e) => setConvertDueDate(e.target.value)} />
          </Field>
          {convertError && (
            <p role="alert" className="rounded-lg border border-tone-red/30 bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{convertError}</p>
          )}
          <Button variant="primary" size="lg" onClick={() => generateInvoice("full")} disabled={actioning}
            className="h-auto w-full flex-col items-start gap-0 py-2.5 text-left md:h-auto">
            <span className="block">Full invoice</span>
            <span className="block text-xs font-normal opacity-80">All non-optional lines, with the quotation discount</span>
          </Button>
          {schedules.filter((s) => s.status !== "invoiced").length > 0 && (
            <div>
              <p className="mb-2 text-[13px] font-medium text-fg">Or bill a milestone</p>
              <div className="space-y-1.5">
                {schedules.filter((s) => s.status !== "invoiced").map((s) => (
                  <Button key={s.id} onClick={() => generateInvoice("milestone", s.id)} disabled={actioning}
                    className="w-full justify-between">
                    <span>{s.name}</span>
                    <span className="text-xs text-fg-muted tabular">{s.percentage != null ? `${s.percentage}%` : (s.amount != null ? fmt(s.amount) : "")}</span>
                  </Button>
                ))}
              </div>
            </div>
          )}
        </div>
      </Dialog>
    </div>
  );
}
