"use client";

import { useState } from "react";
import { CheckCircle2, Plus, Trash2 } from "lucide-react";
import { Button, Card, CardBody, CardFooter, CardHeader, IconButton, Input } from "@/components/ui";

/** Confirming a proposal (Nary Phase 4): the Owner's final prices become the quotation's manual lines (the
 *  cost sheet suggests the price at the target margin; it stays internal). Catalog items can be added by
 *  SKU. Nary then drafts the quotation, linked to the proposal, for approval. */
export interface ConfirmLine { kind: "manual" | "catalog"; name: string; product_id?: string; quantity: string; unit_price: string }

export default function ConfirmPanel({ suggested, floor, currency, onConfirm }: {
  suggested: string | null; floor: string | null; currency: string; onConfirm: (lines: ConfirmLine[]) => void;
}) {
  const [lines, setLines] = useState<ConfirmLine[]>([
    { kind: "manual", name: "Implementation services", quantity: "1", unit_price: suggested ?? "" }]);
  const set = (i: number, k: keyof ConfirmLine, v: string) => setLines(lines.map((l, j) => (j === i ? { ...l, [k]: v } : l)));
  const total = lines.reduce((s, l) => s + (Number(l.unit_price) || 0) * (Number(l.quantity) || 0), 0);
  const underFloor = floor != null && lines.some((l) => l.kind === "manual") && total < Number(floor);
  const ok = lines.length > 0 && lines.every((l) => l.name.trim() && Number(l.quantity) > 0 && (l.kind === "catalog" ? l.product_id : Number(l.unit_price) > 0));
  return (
    <Card aria-label="Confirm the proposal">
      <CardHeader title="Confirm: your final prices" description="The quotation Nary drafts from them." />
      <CardBody className="space-y-2">
        {lines.map((l, i) => (
          <div key={i} className="flex items-center gap-2">
            <Input aria-label={`Line ${i + 1} name`} value={l.name} onChange={(e) => set(i, "name", e.target.value)} autoComplete="off" className="flex-1" />
            <Input aria-label={`Line ${i + 1} quantity`} value={l.quantity} onChange={(e) => set(i, "quantity", e.target.value)} inputMode="decimal" autoComplete="off" numeric className="w-20" />
            {l.kind === "manual"
              ? <Input aria-label={`Line ${i + 1} unit price (${currency})`} value={l.unit_price} onChange={(e) => set(i, "unit_price", e.target.value)} inputMode="decimal" autoComplete="off" numeric className="w-40" />
              : <span className="w-40 text-xs text-fg-muted">catalog list price</span>}
            <IconButton icon={Trash2} label={`Remove line ${i + 1}`} size="sm" onClick={() => setLines(lines.filter((_, j) => j !== i))} />
          </div>
        ))}
        <Button size="sm" variant="ghost" onClick={() => setLines([...lines, { kind: "manual", name: "", quantity: "1", unit_price: "" }])}><Plus aria-hidden /> Line</Button>
        {underFloor && <p className="text-xs text-tone-red">Under the floor price ({currency} {Number(floor).toLocaleString()}) for the likely cost.</p>}
      </CardBody>
      <CardFooter>
        <Button variant="primary" disabled={!ok} onClick={() => onConfirm(lines)}>
          <CheckCircle2 aria-hidden /> Confirm and draft the quotation
        </Button>
      </CardFooter>
    </Card>
  );
}
