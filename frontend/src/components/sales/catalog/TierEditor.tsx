"use client";

import { useEffect, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { salesApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { tierProblem } from "@/lib/catalogPricing";
import type { VolumeTier } from "@/lib/types/catalog";
import { Button, IconButton, Input } from "@/components/ui";

interface Props {
  productId: string;
  tiers: VolumeTier[];
  canEdit: boolean;
  onChanged: () => void | Promise<void>;
}

type Row = { min_qty: string; max_qty: string; discount_pct: string };

const toRows = (t: VolumeTier[]): Row[] =>
  [...t].sort((a, b) => Number(a.min_qty) - Number(b.min_qty)).map((x) => ({
    min_qty: String(x.min_qty), max_qty: x.max_qty == null ? "" : String(x.max_qty), discount_pct: String(x.discount_pct),
  }));

/** Volume tiers of one SKU: the discount applied when a quotation holds that many of it. */
export default function TierEditor({ productId, tiers, canEdit, onChanged }: Props) {
  const [rows, setRows] = useState<Row[]>(() => toRows(tiers));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  useEffect(() => { setRows(toRows(tiers)); }, [tiers]);

  const parsed = rows.map((r) => ({
    min_qty: parseFloat(r.min_qty), max_qty: r.max_qty.trim() === "" ? null : parseFloat(r.max_qty), discount_pct: parseFloat(r.discount_pct),
  }));
  const problem = tierProblem(parsed);

  async function save() {
    if (problem) { setError(problem); return; }
    setSaving(true); setError(""); setSaved(false);
    try {
      await salesApi.putCatalogTiers(productId, parsed);
      setSaved(true);
      await onChanged();
    } catch (e) {
      setError(extractApiError(e, "Could not save the tiers."));
    } finally { setSaving(false); }
  }

  const input = "h-9 w-24 text-[13px] md:h-8";

  if (!canEdit && !tiers.length) return <p className="text-xs text-fg-muted">No volume tiers.</p>;

  return (
    <div>
      {rows.length === 0 && <p className="mb-2 text-xs text-fg-muted">No volume tiers: the list price applies at every quantity.</p>}
      <div className="space-y-1.5">
        {rows.map((r, i) => (
          <div key={i} className="flex flex-wrap items-center gap-2 text-[13px] text-fg-muted">
            <span>From</span>
            <Input type="number" numeric min={0} step="any" value={r.min_qty} disabled={!canEdit} aria-label={`Tier ${i + 1} from`}
              onChange={(e) => setRows((p) => p.map((x, j) => (j === i ? { ...x, min_qty: e.target.value } : x)))} className={input} />
            <span>to</span>
            <Input type="number" numeric min={0} step="any" value={r.max_qty} placeholder="∞" disabled={!canEdit} aria-label={`Tier ${i + 1} to`}
              onChange={(e) => setRows((p) => p.map((x, j) => (j === i ? { ...x, max_qty: e.target.value } : x)))} className={input} />
            <span>units:</span>
            <Input type="number" numeric min={0} max={100} step="any" value={r.discount_pct} disabled={!canEdit} aria-label={`Tier ${i + 1} discount`}
              onChange={(e) => setRows((p) => p.map((x, j) => (j === i ? { ...x, discount_pct: e.target.value } : x)))} className={input} />
            <span>% off</span>
            {canEdit && (
              <IconButton size="sm" icon={Trash2} label={`Remove tier ${i + 1}`} className="hover:text-tone-red"
                onClick={() => setRows((p) => p.filter((_, j) => j !== i))} />
            )}
          </div>
        ))}
      </div>
      {canEdit && (
        <div className="mt-2.5 flex flex-wrap items-center gap-3">
          <Button variant="ghost" size="sm" onClick={() => setRows((p) => [...p, { min_qty: "", max_qty: "", discount_pct: "" }])}>
            <Plus aria-hidden /> Add tier
          </Button>
          <div className="flex-1" />
          {rows.length > 0 && problem && <span className="text-xs text-tone-orange">{problem}</span>}
          {saved && !error && <span className="text-xs text-tone-blue">Saved</span>}
          <Button variant="primary" size="sm" onClick={save} loading={saving} disabled={rows.length > 0 && !!problem}>
            Save tiers
          </Button>
        </div>
      )}
      {error && <p role="alert" className="mt-2 text-xs text-tone-red">{error}</p>}
    </div>
  );
}
