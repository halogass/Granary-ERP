"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { ArrowLeft, AlertTriangle, Undo2 } from "lucide-react";
import { approvalsApi, salesApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Card, EmptyState, Skeleton } from "@/components/ui";
import QuotationForm, { type QuotationHeader, type LineItem, savedItemToLine } from "@/components/sales/QuotationForm";

export default function EditQuotationPage({ params }: { params: { id: string } }) {
  const id = params.id;
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [initialHeader, setInitialHeader] = useState<Partial<QuotationHeader> | null>(null);
  const [initialLines, setInitialLines] = useState<LineItem[]>([]);
  // 2 Nov 2026: a draft waiting for approval can't be saved: say so up front, with Withdraw right here
  const [pending, setPending] = useState(false);
  const [number, setNumber] = useState("");
  const [withdrawing, setWithdrawing] = useState(false);
  const [withdrawError, setWithdrawError] = useState("");

  useEffect(() => {
    let live = true;
    approvalsApi.forEntity("quotation", id)
      .then((r) => { if (live) setPending(r.data?.request?.status === "pending"); })
      .catch(() => {});
    return () => { live = false; };
  }, [id]);

  async function withdraw() {
    setWithdrawing(true); setWithdrawError("");
    try {
      await salesApi.withdrawQuotation(id, "To change it in the editor");
      setPending(false);
    } catch (e) {
      setWithdrawError(extractApiError(e, "Could not withdraw it from approval."));
    } finally { setWithdrawing(false); }
  }

  useEffect(() => {
    let cancelled = false;
    salesApi.getQuotation(id)
      .then((r) => {
        if (cancelled) return;
        const q = r.data;
        setStatus(q.status);
        setNumber(q.full_number || q.number || "");
        setInitialHeader({
          customer_org_id: q.customer_org_id ?? "",
          customer_contact_id: q.customer_contact_id ?? "",
          deal_id: q.deal_id ?? "",
          title: q.subject ?? "",
          notes: q.notes ?? "",
          issue_date: q.issue_date ?? new Date().toISOString().slice(0, 10),
          valid_until: q.valid_until ?? new Date().toISOString().slice(0, 10),
          discount_type: (q.discount_type as QuotationHeader["discount_type"]) ?? "none",
          discount_value: q.discount_value ?? 0,
          discount_note: q.discount_note ?? "",
          discount_display: q.discount_display === "notes_only" ? "notes_only" : "pdf",
          payment_terms: q.payment_terms?.text ?? "",
          currency: q.currency ?? "USD",
          exchange_rate: q.exchange_rate != null ? Number(q.exchange_rate) : null,
          procurement_type: q.procurement_type ?? "standard",
        });
        // Lines keep their keys (the item ids) so add-ons stay under their parent; lines at list are
        // re-priced by the server, manual ones keep their typed price.
        setInitialLines((q.items ?? []).map(savedItemToLine));
      })
      .catch(() => { if (!cancelled) setError("Quotation not found."); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [id]);

  if (loading) return (
    <div className="mx-auto max-w-4xl space-y-3 py-6" aria-busy="true">
      <Skeleton className="h-8 w-64" />
      <Skeleton className="h-40 w-full" />
      <Skeleton className="h-64 w-full" />
    </div>
  );

  if (error || (status && status !== "draft")) return (
    <div className="mx-auto max-w-xl py-10">
      <Card>
        <EmptyState icon={AlertTriangle}
          title={error || "Only draft quotations can be edited"}
          description={!error ? <>This quotation is <strong>{status}</strong> and is locked. Create a new revision to change it.</> : undefined}
          action={<Button asChild><Link href={`/sales/quotations/${id}`}><ArrowLeft aria-hidden /> Back to quotation</Link></Button>} />
      </Card>
    </div>
  );

  if (!initialHeader) return null;
  const waiting = `${number || "This quotation"} is waiting for approval: withdraw it first, then save your changes.`;
  return (
    <>
      {pending && (
        <div role="alert" className="mx-auto mb-4 flex max-w-6xl flex-wrap items-center justify-between gap-3 rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-4 py-3">
          <div className="text-sm text-fg">
            <p className="font-semibold">Waiting for approval: withdraw it first</p>
            <p className="text-xs text-fg-muted">The approver has it, so it can&apos;t be changed. Withdraw it (the person who submitted it or the Owner), change it, and save: it goes to approval again.</p>
            {withdrawError && <p className="mt-1 text-xs font-medium text-tone-red">{withdrawError}</p>}
          </div>
          <Button onClick={withdraw} disabled={withdrawing}><Undo2 aria-hidden /> Withdraw from approval</Button>
        </div>
      )}
      <QuotationForm mode="edit" quotationId={id} initialHeader={initialHeader} initialLines={initialLines}
        lockedReason={pending ? waiting : undefined} />
    </>
  );
}
