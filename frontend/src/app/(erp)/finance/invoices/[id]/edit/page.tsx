"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { ArrowLeft, AlertTriangle } from "lucide-react";
import { Button, EmptyState, Skeleton } from "@/components/ui";
import { financeApi } from "@/lib/api";
import InvoiceForm, { type InvoiceFormValues, type InvoiceLine } from "@/components/finance/InvoiceForm";

interface RawItem {
  name: string; description?: string | null; product_id?: string | null;
  quantity: number; unit_price: number; discount_pct?: number; tax_pct?: number; tax_id?: string | null;
  id?: string; parent_item_id?: string | null; sku?: string | null; term_months?: number | null; price_rule?: string | null;
}

export default function EditInvoicePage({ params }: { params: { id: string } }) {
  const id = params.id;
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [initialForm, setInitialForm] = useState<Partial<InvoiceFormValues> | null>(null);
  const [initialLines, setInitialLines] = useState<InvoiceLine[]>([]);

  useEffect(() => {
    let cancelled = false;
    financeApi.getInvoice(id)
      .then((r) => {
        if (cancelled) return;
        const v = r.data;
        setStatus(v.status);
        const terms = /(\d+)/.exec(v.payment_terms_text || "")?.[1] ?? "30";
        setInitialForm({
          invoice_type: v.invoice_type ?? "commercial",
          issue_date: v.issue_date ?? new Date().toISOString().slice(0, 10),
          due_date: v.due_date ?? "",
          customer_org_id: v.customer_org_id ?? "",
          customer_contact_id: v.customer_contact_id ?? "",
          billing_email: v.customer_email ?? "",
          billing_whatsapp: v.customer_tel ?? "",
          notes: v.notes ?? "",
          payment_terms: terms,
          currency: v.currency ?? "USD",
          exchange_rate: null,
        });
        setInitialLines((v.items ?? []).map((i: RawItem) => ({
          name: i.name ?? "",
          description: i.description ?? "",
          product_id: i.product_id ?? "",
          qty: i.quantity == null ? 1 : Number(i.quantity),   // keep the server's value; 0 stays 0
          unit_price: Number(i.unit_price) || 0,
          discount_pct: Number(i.discount_pct) || 0,
          tax_pct: Number(i.tax_pct) || 0,
          tax_id: i.tax_id ?? "",
          // Catalog snapshot, kept on save (add-on parent, recurring term, price rule).
          key: i.id, parent_key: i.parent_item_id ?? null,
          sku: i.sku ?? null, term_months: i.term_months ?? null, price_rule: i.price_rule ?? null,
        })));
      })
      .catch(() => { if (!cancelled) setError("Invoice not found."); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [id]);

  if (loading) return (
    <div className="mx-auto max-w-4xl space-y-4 py-6" aria-busy>
      <Skeleton className="h-6 w-48" />
      <Skeleton className="h-48 w-full" />
    </div>
  );

  if (error || (status && status !== "draft")) return (
    <div className="mx-auto max-w-xl py-10">
      <EmptyState icon={AlertTriangle} title={error || "Only draft invoices can be edited"}
        description={!error ? <>This invoice is <strong className="text-fg">{status}</strong> and is locked. Revise a sent proforma, or issue a credit note for a commercial invoice.</> : undefined}
        action={<Button asChild><Link href={`/finance/invoices/${id}`}><ArrowLeft aria-hidden /> Back to invoice</Link></Button>} />
    </div>
  );

  if (!initialForm) return null;
  return <InvoiceForm mode="edit" invoiceId={id} initialForm={initialForm} initialLines={initialLines} />;
}
