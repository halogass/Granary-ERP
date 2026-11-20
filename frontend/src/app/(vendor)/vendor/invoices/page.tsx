"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import api, { portalVendorApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Receipt, Plus } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, Input, KeyValues, ListFrame, PageHeader, Select, Stat, StatGroup,
  StatusPill, Textarea, toast, type ColumnDef,
} from "@/components/ui";
import { formatCurrency, sumByCurrency } from "@/lib/currency";

interface VendorInvoice {
  id: string;
  vendor_invoice_number: string | null;
  work_order_id?: string | null;
  purchase_order_id?: string | null;
  work_order_title?: string;
  amount: number;          // before tax
  tax_amount?: number;     // sales tax
  withholding_amount?: number;   // withheld by the company
  net_payable?: number | null;
  currency: string;
  status: string;
  invoice_date: string;
  due_date?: string;
  paid_at?: string;
  notes?: string;
  created_at?: string;
}

/** A purchase order or work order the invoice can be billed against. */
interface BillableOrder { key: string; kind: "po" | "wo"; id: string; number: string; label: string; currency: string; }

const fmtDate = (d?: string | null) => d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-";

export default function VendorInvoicesPage() {
  const [invoices, setInvoices] = useState<VendorInvoice[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [orders, setOrders] = useState<BillableOrder[]>([]);
  const EMPTY_FORM = {
    order_key: "", vendor_invoice_number: "", amount: "", tax_amount: "",
    invoice_date: "", due_date: "", notes: "",
  };
  const [form, setForm] = useState(EMPTY_FORM);
  const selectedOrder = orders.find((o) => o.key === form.order_key);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await portalVendorApi.listMyInvoices();
      setInvoices(res.data.items ?? res.data ?? []);
    } catch { setInvoices([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  useEffect(() => {
    Promise.all([
      api.get("/vendor/portal/purchase-orders").then((r) => r.data ?? []).catch(() => []),
      portalVendorApi.listMyWorkOrders().then((r) => r.data ?? []).catch(() => []),
    ]).then(([pos, wos]: [{ id: string; number: string; status: string; currency: string }[], { id: string; number: string; title: string; status: string; currency: string }[]]) => {
      setOrders([
        ...pos.map((p) => ({ key: `po:${p.id}`, kind: "po" as const, id: p.id, number: p.number, label: `PO ${p.number}`, currency: p.currency })),
        ...wos.filter((w) => !["draft", "cancelled"].includes(w.status))
          .map((w) => ({ key: `wo:${w.id}`, kind: "wo" as const, id: w.id, number: w.number, label: `WO ${w.number} · ${w.title}`, currency: w.currency })),
      ]);
    });
  }, []);

  const orderNumber = (inv: VendorInvoice) =>
    orders.find((o) => o.id === inv.purchase_order_id || o.id === inv.work_order_id)?.label;

  async function submit() {
    if (!canSubmit || !selectedOrder) return;
    setSaving(true);
    try {
      await portalVendorApi.submitInvoice({
        purchase_order_id: selectedOrder.kind === "po" ? selectedOrder.id : undefined,
        work_order_id: selectedOrder.kind === "wo" ? selectedOrder.id : undefined,
        vendor_invoice_number: form.vendor_invoice_number.trim(),
        amount: form.amount,
        tax_amount: form.tax_amount || "0",
        currency: selectedOrder.currency,
        invoice_date: form.invoice_date,
        due_date: form.due_date || undefined,
        notes: form.notes.trim() || undefined,
      });
      setShowModal(false);
      setForm(EMPTY_FORM);
      toast.success("Invoice submitted");
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Could not submit the invoice"));
    } finally { setSaving(false); }
  }

  const canSubmit = !!selectedOrder && !!form.vendor_invoice_number.trim() && !!form.amount && !!form.invoice_date;
  // Invoices carry their own currency: grouped by currency, never added across them.
  const totalPaid = sumByCurrency(invoices.filter((i) => i.status === "paid"), (i) => i.amount, (i) => i.currency, "USD");
  const totalPending = sumByCurrency(invoices.filter((i) => ["submitted", "under_review", "approved", "partially_paid"].includes(i.status)),
    (i) => i.amount, (i) => i.currency, "USD");
  const mixedNote = (t: { mixed: boolean; currency: string }) => t.mixed ? `Mixed currencies: ${t.currency} total shown` : undefined;

  const columns = useMemo<ColumnDef<VendorInvoice>[]>(() => [
    { accessorKey: "vendor_invoice_number", header: "Invoice", cell: (c) => <span className="font-mono text-[13px]">{c.getValue<string>() ?? "-"}</span> },
    { id: "order", header: "Order", enableSorting: false, meta: { hideOnMobile: true },
      cell: ({ row }) => <span className="text-fg-muted">{row.original.work_order_title ?? orderNumber(row.original) ?? "-"}</span> },
    { accessorKey: "invoice_date", header: "Date", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmtDate(c.getValue<string>()) },
    { accessorKey: "amount", header: "Amount before tax", meta: { numeric: true },
      cell: ({ row }) => <>{row.original.amount.toLocaleString()} <span className="text-xs text-fg-muted">{row.original.currency}</span></> },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="invoice" status={c.getValue<string>()} /> },
  // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [orders]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={invoices.length} />
        <Stat label="Pending" value={formatCurrency(totalPending.total, totalPending.currency)} note={mixedNote(totalPending)} />
        <Stat label="Paid" value={formatCurrency(totalPaid.total, totalPaid.currency)} note={mixedNote(totalPaid)} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="My invoices" summary="Submit and track payment for your work."
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Submit invoice</Button>} />
        <DataTable label="Invoices" columns={columns} data={invoices} getRowId={(i) => i.id} loading={loading}
          empty={{ icon: Receipt, title: "No invoices submitted yet", description: "Submit an invoice against a purchase order or work order.",
            action: <Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Submit invoice</Button> }}
          renderDetails={(inv) => (
            <div className="space-y-3">
              <KeyValues cols={3} items={[
                { label: "Order", value: inv.work_order_title ?? orderNumber(inv) ?? "-" },
                { label: "Invoice date", value: fmtDate(inv.invoice_date) },
                { label: "Due", value: fmtDate(inv.due_date) },
                { label: "Amount before tax", value: `${inv.amount.toLocaleString()} ${inv.currency}`, numeric: true },
                ...(inv.tax_amount ? [{ label: "Sales tax", value: `+${inv.tax_amount.toLocaleString()}`, numeric: true }] : []),
                ...(inv.withholding_amount ? [{ label: "Withholding tax", value: `-${inv.withholding_amount.toLocaleString()}`, numeric: true }] : []),
                ...(inv.net_payable != null ? [{ label: "Net payable", value: `${inv.net_payable.toLocaleString()} ${inv.currency}`, numeric: true }] : []),
                ...(inv.paid_at ? [{ label: "Paid", value: fmtDate(inv.paid_at) }] : []),
              ]} />
              {inv.notes && <p className="whitespace-pre-wrap text-[13px] text-fg">{inv.notes}</p>}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Submit invoice" dismissable={!saving}
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={submit} loading={saving} disabled={!canSubmit}>{saving ? "Submitting..." : "Submit invoice"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Purchase order or work order" required className="md:col-span-2"
            hint={orders.length === 0 ? "No open purchase orders or work orders to invoice." : undefined}>
            <Select value={form.order_key} onChange={(e) => setForm({ ...form, order_key: e.target.value })}
              placeholder="Select the order you are invoicing"
              options={orders.map((o) => ({ value: o.key, label: `${o.label} (${o.currency})` }))} />
          </Field>
          <Field label="Invoice number" required className="md:col-span-2">
            <Input value={form.vendor_invoice_number} onChange={(e) => setForm({ ...form, vendor_invoice_number: e.target.value })}
              placeholder="Your own invoice number" />
          </Field>
          <Field label="Amount before tax" required>
            <Input type="number" numeric value={form.amount} onChange={(e) => setForm({ ...form, amount: e.target.value })}
              trailing={selectedOrder?.currency} />
          </Field>
          <Field label="Sales tax" hint="Withholding tax, where it applies, is withheld by us and calculated for you.">
            <Input type="number" numeric value={form.tax_amount} onChange={(e) => setForm({ ...form, tax_amount: e.target.value })}
              placeholder="Blank if no sales tax is charged" />
          </Field>
          <Field label="Invoice date" required>
            <Input type="date" value={form.invoice_date} onChange={(e) => setForm({ ...form, invoice_date: e.target.value })} />
          </Field>
          <Field label="Due date">
            <Input type="date" value={form.due_date} onChange={(e) => setForm({ ...form, due_date: e.target.value })} />
          </Field>
          <Field label="Notes" className="md:col-span-2">
            <Textarea rows={2} value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
