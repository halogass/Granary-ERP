"use client";

import { Lock, RefreshCw, TriangleAlert } from "lucide-react";
import { Button, Card, CardBody, CardHeader } from "@/components/ui";
import { cn } from "@/lib/utils";
import { stack } from "@/lib/stackTable";

/** The internal cost sheet of a proposal (Nary Phase 4). Never shown to a customer, never in the PDF:
 *  cost per man-day from the rate card, non-labour from catalog costs, contingency, and the price at the
 *  target margin and floor. The Owner sets the final prices. */

interface Line { role: string; seniority: string; low: number; likely: number; high: number; day_rate: string | null; cost_likely: string | null }
interface Workstream { name: string; lines: Line[]; total_likely: string }
interface Point { labour: string; non_labour: string; contingency: string; cost: string; band: string | null; target_pct?: string; floor_pct?: string; price_at_target: string | null; price_at_floor: string | null }
export interface Sheet {
  labour: { workstreams: Workstream[]; missing_rates: string[] };
  non_labour: { items: { item: string; kind: string; quantity: number; cost_base: string | null; source: string }[]; to_confirm: string[] };
  contingency_pct: string; summary: Record<"low" | "likely" | "high", Point>; assumptions: string[]; currency: string;
}

const amount = (cur: string) => (v: string | null | undefined) =>
  (v == null ? "to confirm" : `${cur} ${Number(v).toLocaleString("en-US", { maximumFractionDigits: 2 })}`);

/** `onReestimate`: ask Nary for a fresh estimate (after a rate card or scope change). */
export default function CostSheet({ sheet, onReestimate, busy }: { sheet: Sheet; onReestimate?: () => void; busy?: boolean }) {
  const likely = sheet.summary.likely;
  const money = amount(sheet.currency || "USD");
  return (
    <Card aria-label="Internal cost sheet" className="border-tone-orange/40">
      <CardHeader title={<span className="inline-flex items-center gap-1.5 text-tone-orange"><Lock className="size-3.5" aria-hidden /> Internal cost sheet. Never sent, never in the PDF.</span>}
        actions={onReestimate && <Button size="sm" onClick={onReestimate} loading={busy}><RefreshCw aria-hidden /> Re-estimate</Button>} />
      <CardBody className="space-y-3">
        <table className={cn("w-full text-sm", stack.table)}>
          <thead className={stack.thead}><tr className="text-left text-xs text-fg-muted"><th className="font-medium" /><th className="text-right font-medium">Low</th><th className="text-right font-medium">Likely</th><th className="text-right font-medium">High</th></tr></thead>
          <tbody className={cn(stack.tbody, "max-md:p-0")}>
            {(["labour", "non_labour", "contingency", "cost", "price_at_target", "price_at_floor"] as const).map((k) => (
              <tr key={k} className={cn("border-t border-line-soft", (k === "cost" || k === "price_at_target") && "font-semibold", stack.tr)}>
                <td className={cn("py-1.5 text-fg", stack.title)}>{{ labour: "Labour", non_labour: "Non-labour", contingency: `Contingency (${sheet.contingency_pct}%)`, cost: "Cost",
                  price_at_target: `Price at target margin${likely.target_pct ? ` (${likely.target_pct}%)` : ""}`, price_at_floor: `Floor${likely.floor_pct ? ` (${likely.floor_pct}%)` : ""}` }[k]}</td>
                {(["low", "likely", "high"] as const).map((p) => <td key={p} data-label={{ low: "Low", likely: "Likely", high: "High" }[p]} className={cn("py-1.5 text-right tabular", stack.td)}>{money(sheet.summary[p][k])}</td>)}
              </tr>
            ))}
          </tbody>
        </table>
        {likely.band && <p className="text-xs text-fg-muted">Margin category: {likely.band}.</p>}
        {sheet.labour.missing_rates.length > 0 && (
          <p className="inline-flex items-center gap-1 text-xs text-tone-red"><TriangleAlert className="size-3.5" aria-hidden /> No rate for: {sheet.labour.missing_rates.join(", ")} (Settings, Proposals, Rate card).</p>
        )}
        {sheet.non_labour.to_confirm.length > 0 && <p className="text-xs text-tone-orange">To confirm: {sheet.non_labour.to_confirm.join(", ")}.</p>}
        <details className="text-xs">
          <summary className="cursor-pointer font-medium text-fg-muted">Workstreams and man-days</summary>
          {sheet.labour.workstreams.map((w) => (
            <div key={w.name} className="mt-2">
              <p className="font-semibold text-fg">{w.name} · {money(w.total_likely)}</p>
              <ul className="ml-3 text-fg-muted">{w.lines.map((l, i) => <li key={i}>{l.role} ({l.seniority}): {l.low} / {l.likely} / {l.high} days × {money(l.day_rate)}</li>)}</ul>
            </div>
          ))}
          {sheet.assumptions.length > 0 && <><p className="mt-2 font-semibold text-fg">Assumptions</p><ul className="ml-3 list-disc text-fg-muted">{sheet.assumptions.map((a, i) => <li key={i}>{a}</li>)}</ul></>}
        </details>
      </CardBody>
    </Card>
  );
}
