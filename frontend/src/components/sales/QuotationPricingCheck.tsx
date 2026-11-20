"use client";

/**
 * 5 Nov 2026 (offering expansion E7): the pricing rules of a saved quotation (GET /sales/quotations/{id}
 * pricing_check). Blocking issues stop the approval of a draft unless the Owner gave an override with a reason
 * for these lines (POST /sales/quotations/{id}/pricing-override, Owner only, audited); warnings never block.
 */
import { useState } from "react";
import { AlertTriangle, ShieldCheck } from "lucide-react";
import { Button } from "@/components/ui";
import ReasonModal from "@/components/ui/ReasonModal";
import { salesApi } from "@/lib/api";
import { cn } from "@/lib/utils";

export interface PricingIssue { code: string; message: string }
export interface PricingCheck {
  blocking: PricingIssue[];
  warnings: PricingIssue[];
  override?: { reason?: string; by?: string | null; at?: string | null; valid: boolean } | null;
  refused_at_approval?: boolean;
}

interface Props {
  quotationId: string;
  number: string;
  status: string;
  check?: PricingCheck | null;
  onChange: () => void;
}

export default function QuotationPricingCheck({ quotationId, number, status, check, onChange }: Props) {
  const [open, setOpen] = useState(false);
  if (!check) return null;
  const draft = status === "draft" || status === "rejected";
  const blocking = check.blocking ?? [];
  const warnings = check.warnings ?? [];
  const ov = check.override;
  if (!blocking.length && !warnings.length && !ov) return null;
  const overridden = !!ov?.valid;
  const tone = blocking.length && !overridden ? "red" : "orange";
  return (
    <div role="status" aria-label="Pricing check"
      className={cn("mb-4 rounded-lg border px-4 py-3 text-sm text-fg",
        tone === "red" ? "border-tone-red/30 bg-tone-red-soft" : "border-tone-orange/30 bg-tone-orange-soft")}>
      <div className="flex flex-wrap items-center gap-3">
        <AlertTriangle className={cn("size-4 shrink-0", tone === "red" ? "text-tone-red" : "text-tone-orange")} aria-hidden />
        <p className="min-w-0 flex-1 font-medium">
          {blocking.length
            ? overridden
              ? "Pricing rules: overridden by the Owner for these lines."
              : draft
                ? "Pricing rules: the approval refuses these prices."
                : "Pricing rules: these prices break a blocking rule."
            : "Pricing check"}
        </p>
        {draft && (blocking.length > 0 || ov) && (
          <Button size="sm" onClick={() => setOpen(true)}>
            <ShieldCheck aria-hidden /> {ov ? "Change override (Owner)" : "Override (Owner)"}
          </Button>
        )}
      </div>
      {blocking.length > 0 && (
        <ul className="mt-2 list-disc space-y-0.5 pl-6">
          {blocking.map((b, i) => <li key={`b${i}`}>{b.message}</li>)}
        </ul>
      )}
      {warnings.length > 0 && (
        <ul className="mt-2 list-disc space-y-0.5 pl-6 text-fg-muted">
          {warnings.map((w, i) => <li key={`w${i}`}>{w.message}</li>)}
        </ul>
      )}
      {ov && (
        <p className="mt-2 text-xs text-fg-muted">
          Override{ov.by ? ` by ${ov.by}` : ""}{ov.reason ? `: ${ov.reason}` : ""}
          {!ov.valid && " (given for earlier lines: the prices changed since, so it no longer applies)"}
        </p>
      )}
      {open && (
        <ReasonModal
          title={`Pricing override for ${number}`}
          description="Only the Owner can let this draft through the pricing rules. The override holds for the lines as they are now and is recorded in the audit log. An empty reason removes it."
          label="Why are these prices accepted? *"
          placeholder="e.g. Strategic first deal in the region; priced to win the account"
          confirmText="Save override"
          onClose={() => setOpen(false)}
          onConfirm={async (reason) => {
            await salesApi.pricingOverride(quotationId, reason);
            setOpen(false);
            onChange();
          }}
        />
      )}
    </div>
  );
}
