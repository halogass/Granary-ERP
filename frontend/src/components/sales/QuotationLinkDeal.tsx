"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState } from "react";
import { crmApi, salesApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Checkbox, Combobox, Dialog, Field, type ComboboxSource } from "@/components/ui";

interface DealRow { id: string; title: string; deal_number?: string | null; stage?: string | null }

/**
 * 5 Nov 2026: "Link to deal" for a quotation that is no longer an editable draft (sent, approved, accepted...).
 * The quotation and its revisions are linked to the deal without a revision; by default the deal's value
 * becomes the quotation's total. Only the customer's own deals are offered (the API refuses another
 * organization's deal).
 */
export default function QuotationLinkDeal({
  open, onClose, onLinked, quotationId, number, orgId, currentDealId,
}: {
  open: boolean;
  onClose: () => void;
  onLinked: () => void | Promise<void>;
  quotationId: string;
  number: string;
  orgId?: string | null;
  currentDealId?: string | null;
}) {
  const [deals, setDeals] = useState<DealRow[]>([]);
  const [dealId, setDealId] = useState("");
  const [setValue, setSetValue] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!open) return;
    setError("");
    setDealId("");
    setSetValue(true);
    let live = true;
    fetchAllPages<DealRow>(crmApi.listDeals, orgId ? { organization_id: orgId } : {})
      .then(({ rows }) => { if (live) setDeals(rows); })
      .catch(() => { if (live) setDeals([]); });
    return () => { live = false; };
  }, [open, orgId]);

  // the deals already in hand, filtered in the browser (the deals list has no text search)
  const source = useMemo<ComboboxSource>(() => {
    const options = deals.filter((d) => d.id !== currentDealId).map((d) => ({
      id: d.id, label: d.title, sublabel: [d.deal_number, d.stage].filter(Boolean).join(" · ") || undefined,
    }));
    return {
      fetch: (query) => {
        const q = query.trim().toLowerCase();
        return Promise.resolve(q ? options.filter((o) => `${o.label} ${o.sublabel ?? ""}`.toLowerCase().includes(q)) : options);
      },
      resolve: (id) => Promise.resolve(options.find((o) => o.id === id) ?? null),
    };
  }, [deals, currentDealId]);

  async function link() {
    if (!dealId) return;
    setSaving(true);
    setError("");
    try {
      await salesApi.linkQuotationDeal(quotationId, dealId, setValue);
      await onLinked();
      onClose();
    } catch (e) {
      setError(extractApiError(e, "Could not link the quotation to the deal."));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={(o) => { if (!o) onClose(); }} title={`Link ${number} to a deal`}
      description="No revision is made: the PDF, lines, terms, approval and status stay as they are. Its revisions are linked too."
      footer={<>
        <Button onClick={onClose}>Back</Button>
        <Button variant="primary" onClick={link} disabled={!dealId} loading={saving}>Link to deal</Button>
      </>}>
      <div className="space-y-4">
        <Field label="Deal" hint={orgId ? "The customer's deals" : "This quotation has no customer: recent deals"}>
          <Combobox value={dealId} onChange={(id) => setDealId(id)} source={source} placeholder="Choose a deal" />
        </Field>
        <Checkbox label="Set the deal's value to this quotation's total" checked={setValue}
          onChange={(e) => setSetValue(e.target.checked)} />
        {error && (
          <p role="alert" className="rounded-lg border border-tone-red/30 bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{error}</p>
        )}
      </div>
    </Dialog>
  );
}
