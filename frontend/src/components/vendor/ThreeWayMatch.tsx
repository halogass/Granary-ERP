"use client";

import { useEffect, useState } from "react";
import { AlertTriangle } from "lucide-react";
import { p2pApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { formatCurrency } from "@/lib/currency";
import { KeyValues, Pill, Skeleton, type Tone } from "@/components/ui";

// Shape of GET /vendor/purchase-orders/{id}/matching.
interface MatchLine {
  po_item_id: string;
  component_sku: string | null;
  description: string;
  quantity_ordered: number;
  quantity_received: number;
  quantity_invoiced: number;
  unit_price: number;
  currency: string | null;
  match_status: "matched" | "partial" | "unmatched" | "over";
  over_received?: boolean;
  over_invoiced?: boolean;
  receipt_pct: number;
  invoice_pct: number;
}
export interface MatchResult {
  po_id: string;
  po_number: string;
  po_total: number;
  po_currency: string | null;
  total_invoiced: Record<string, number>;
  overall_match: "matched" | "partial" | "over";
  invoice_count: number;
  receipt_count: number;
  lines: MatchLine[];
}

const STATUS: Record<string, { label: string; tone: Tone }> = {
  matched: { label: "Matched", tone: "blue" },
  partial: { label: "Partial", tone: "orange" },
  unmatched: { label: "Not started", tone: "neutral" },
  over: { label: "Over", tone: "red" },
};

const qty = (n: number) => n.toLocaleString("en-US", { maximumFractionDigits: 3 });

/** Three-way match of one purchase order: ordered vs received (goods receipts) vs invoiced (approved vendor
 *  invoices), per line. A line received or invoiced beyond the ordered quantity is flagged, even when the
 *  receipt tolerance allowed it. */
export default function ThreeWayMatch({ poId }: { poId: string }) {
  const [data, setData] = useState<MatchResult | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let live = true;
    setData(null);
    setError("");
    p2pApi.getMatching(poId)
      .then((r) => { if (live) setData(r.data as MatchResult); })
      .catch((e) => { if (live) setError(extractApiError(e, "Could not load the match")); });
    return () => { live = false; };
  }, [poId]);

  if (error) return <p className="text-[13px] text-tone-red">{error}</p>;
  if (!data) return <Skeleton className="h-24" />;

  const overall = STATUS[data.overall_match] ?? STATUS.partial;
  const invoiced = Object.entries(data.total_invoiced ?? {});
  const overLines = data.lines.filter((l) => l.over_received || l.over_invoiced);

  return (
    <div className="space-y-3">
      <KeyValues cols={2} items={[
        { label: "Overall", value: <Pill tone={overall.tone}>{overall.label}</Pill> },
        { label: "Goods receipts", value: data.receipt_count, numeric: true },
        { label: "Approved invoices", value: data.invoice_count, numeric: true },
        {
          label: "Invoiced", numeric: true,
          value: invoiced.length ? invoiced.map(([c, a]) => formatCurrency(a, c)).join(", ") : "-",
        },
      ]} />
      {overLines.length > 0 && (
        <p role="alert" className="inline-flex items-center gap-1.5 text-[13px] font-medium text-tone-red">
          <AlertTriangle className="size-4" aria-hidden />
          {overLines.length} line{overLines.length !== 1 ? "s" : ""} received or invoiced beyond the ordered quantity
        </p>
      )}
      {data.lines.length === 0 ? (
        <p className="text-[13px] text-fg-muted">This order has no lines.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-sm" aria-label={`Three-way match for ${data.po_number}`}>
            <thead>
              <tr className="text-left text-xs text-fg-muted">
                <th className="py-1.5 pr-3 font-medium">Line</th>
                <th className="py-1.5 pr-3 text-right font-medium">Ordered</th>
                <th className="py-1.5 pr-3 text-right font-medium">Received</th>
                <th className="py-1.5 pr-3 text-right font-medium">Invoiced</th>
                <th className="py-1.5 font-medium">Match</th>
              </tr>
            </thead>
            <tbody>
              {data.lines.map((l) => {
                const s = STATUS[l.match_status] ?? STATUS.partial;
                return (
                  <tr key={l.po_item_id} className="border-t border-line">
                    <td className="py-1.5 pr-3">
                      <p className="text-fg">{l.description}</p>
                      {l.component_sku && <p className="font-mono text-xs text-fg-muted">{l.component_sku}</p>}
                    </td>
                    <td className="py-1.5 pr-3 text-right tabular">{qty(l.quantity_ordered)}</td>
                    <td className={`py-1.5 pr-3 text-right tabular ${l.over_received ? "font-semibold text-tone-red" : ""}`}>
                      {qty(l.quantity_received)}
                    </td>
                    <td className={`py-1.5 pr-3 text-right tabular ${l.over_invoiced ? "font-semibold text-tone-red" : ""}`}>
                      {qty(l.quantity_invoiced)}
                    </td>
                    <td className="py-1.5">
                      <span className="flex flex-wrap items-center gap-1">
                        <Pill tone={s.tone}>{s.label}</Pill>
                        {l.over_received && <Pill tone="red">Over-received</Pill>}
                        {l.over_invoiced && <Pill tone="red">Over-invoiced</Pill>}
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
