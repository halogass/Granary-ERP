"use client";

import { useState } from "react";
import { Plus, Save, Trash2 } from "lucide-react";
import { salesApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { formatCurrency } from "@/lib/currency";
import type { CatalogPriceRow } from "@/lib/types/catalog";
import { Button, IconButton, Input, Pill, Select } from "@/components/ui";
import { cn } from "@/lib/utils";
import { stack } from "@/lib/stackTable";

interface Props {
  productId: string;
  rows: CatalogPriceRow[];
  currencies: string[];
  canEdit: boolean;
  /** Called after any change so the caller reloads the item. */
  onChanged: () => void | Promise<void>;
}

type Draft = { amount: string; valid_from: string; valid_to: string };

const today = () => new Date().toISOString().slice(0, 10);

/**
 * One list price per currency and effective date. A price review adds a row with a later
 * valid_from instead of overwriting, so a quotation is checked against the price of its issue date.
 */
export default function PriceListEditor({ productId, rows, currencies, canEdit, onChanged }: Props) {
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [adding, setAdding] = useState({ currency: currencies[0] ?? "", amount: "", valid_from: today(), valid_to: "" });
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");

  const allCurrencies = Array.from(new Set([...currencies, ...rows.map((r) => r.currency)]));
  const sorted = [...rows].sort((a, b) => a.currency.localeCompare(b.currency) || b.valid_from.localeCompare(a.valid_from));
  const todayStr = today();
  const isCurrent = (r: CatalogPriceRow) => {
    if (r.valid_from > todayStr || (r.valid_to && r.valid_to < todayStr)) return false;
    // The latest row already in effect for its currency.
    return !rows.some((o) => o.currency === r.currency && o.id !== r.id && o.valid_from > r.valid_from && o.valid_from <= todayStr
      && (!o.valid_to || o.valid_to >= todayStr));
  };

  const draftOf = (r: CatalogPriceRow): Draft =>
    drafts[r.id] ?? { amount: String(r.amount), valid_from: r.valid_from, valid_to: r.valid_to ?? "" };
  const dirty = (r: CatalogPriceRow) => {
    const d = drafts[r.id];
    return !!d && (Number(d.amount) !== Number(r.amount) || d.valid_from !== r.valid_from || (d.valid_to || null) !== (r.valid_to || null));
  };

  async function run(id: string, fn: () => Promise<unknown>) {
    setBusy(id); setError("");
    try {
      await fn();
      setDrafts((p) => { const n = { ...p }; delete n[id]; return n; });
      await onChanged();
    } catch (e) {
      setError(extractApiError(e, "Could not save the price."));
    } finally { setBusy(null); }
  }

  async function add() {
    const amount = parseFloat(adding.amount);
    if (!adding.currency || !Number.isFinite(amount) || amount < 0 || !adding.valid_from) {
      setError("Enter a currency, an amount and a start date."); return;
    }
    await run("new", () => salesApi.addCatalogPrice(productId, {
      currency: adding.currency, amount, valid_from: adding.valid_from, valid_to: adding.valid_to || null,
    }));
    setAdding((a) => ({ ...a, amount: "", valid_to: "" }));
  }

  const cell = "h-11 max-md:text-base md:h-8 md:text-[13px]";

  return (
    <div>
      {sorted.length === 0 ? (
        <p className="mb-3 text-xs text-fg-muted">No list prices yet. Currencies without their own list are converted from the base currency (Settings, Sales).</p>
      ) : (
        <div className="mb-3 overflow-x-auto rounded-lg border border-line max-md:overflow-visible max-md:border-0">
          <table className={cn("w-full text-[13px]", stack.table)}>
            <thead className={cn("bg-surface-2 text-xs text-fg-muted", stack.thead)}>
              <tr>
                <th className="w-20 px-3 py-2 text-left font-medium">Currency</th>
                <th className="px-3 py-2 text-right font-medium">Amount</th>
                <th className="px-3 py-2 text-left font-medium">Valid from</th>
                <th className="px-3 py-2 text-left font-medium">Valid to</th>
                <th className="w-20" />
              </tr>
            </thead>
            <tbody className={cn("divide-y divide-line-soft", stack.tbody, "max-md:p-0")}>
              {sorted.map((r) => {
                const d = draftOf(r);
                return (
                  <tr key={r.id} className={cn("even:bg-surface-2/50", stack.tr)}>
                    <td className={cn("px-3 py-1.5 font-mono font-medium text-fg", stack.title)}>
                      {r.currency}
                      {isCurrent(r) && <Pill tone="blue" className="ml-1.5 h-[18px] px-1.5 font-sans text-[10px]">now</Pill>}
                    </td>
                    <td data-label="Amount" className={cn("px-3 py-1.5 text-right tabular", canEdit ? stack.field : stack.td)}>
                      {canEdit ? (
                        <Input type="number" numeric min={0} step="any" value={d.amount} aria-label={`${r.currency} amount`}
                          onChange={(e) => setDrafts((p) => ({ ...p, [r.id]: { ...d, amount: e.target.value } }))}
                          className={`${cell} ml-auto md:w-32`} />
                      ) : formatCurrency(r.amount, r.currency)}
                    </td>
                    <td data-label="Valid from" className={cn("px-3 py-1.5 tabular", canEdit ? stack.field : stack.td)}>
                      {canEdit ? (
                        <Input type="date" value={d.valid_from} aria-label="Valid from"
                          onChange={(e) => setDrafts((p) => ({ ...p, [r.id]: { ...d, valid_from: e.target.value } }))} className={`${cell} md:w-40`} />
                      ) : r.valid_from}
                    </td>
                    <td data-label="Valid to" className={cn("px-3 py-1.5 tabular", canEdit ? stack.field : stack.td)}>
                      {canEdit ? (
                        <Input type="date" value={d.valid_to} aria-label="Valid to"
                          onChange={(e) => setDrafts((p) => ({ ...p, [r.id]: { ...d, valid_to: e.target.value } }))} className={`${cell} md:w-40`} />
                      ) : (r.valid_to ?? "-")}
                    </td>
                    <td className={cn("whitespace-nowrap px-2 py-1.5 text-right", canEdit ? stack.actions : stack.hide)}>
                      {canEdit && (
                        <>
                          <IconButton size="sm" icon={Save} label={`Save ${r.currency} price`} disabled={!dirty(r) || busy === r.id}
                            onClick={() => run(r.id, () => salesApi.updateCatalogPrice(r.id, {
                              amount: parseFloat(d.amount), valid_from: d.valid_from, valid_to: d.valid_to || null,
                            }))} />
                          <IconButton size="sm" icon={Trash2} label={`Delete ${r.currency} price`} disabled={busy === r.id}
                            className="hover:text-tone-red" onClick={() => run(r.id, () => salesApi.deleteCatalogPrice(r.id))} />
                        </>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {canEdit && (
        <div className="flex flex-wrap items-end gap-2 rounded-lg bg-surface-2 p-2.5">
          <div className="w-24">
            <Select value={adding.currency} onChange={(e) => setAdding({ ...adding, currency: e.target.value })}
              aria-label="Currency" className="font-mono" options={allCurrencies.map((c) => ({ value: c, label: c }))} />
          </div>
          <Input type="number" numeric min={0} step="any" placeholder="Amount" value={adding.amount} aria-label="New amount"
            onChange={(e) => setAdding({ ...adding, amount: e.target.value })} className="w-32 max-md:flex-1" />
          <label className="flex items-center gap-1.5 text-xs text-fg-muted max-md:w-full">from
            <Input type="date" value={adding.valid_from} onChange={(e) => setAdding({ ...adding, valid_from: e.target.value })} className="w-40 max-md:flex-1" />
          </label>
          <label className="flex items-center gap-1.5 text-xs text-fg-muted max-md:w-full">to
            <Input type="date" value={adding.valid_to} onChange={(e) => setAdding({ ...adding, valid_to: e.target.value })} className="w-40 max-md:flex-1" />
          </label>
          <Button variant="primary" size="sm" onClick={add} loading={busy === "new"}>
            <Plus aria-hidden /> Add price
          </Button>
        </div>
      )}
      {error && <p role="alert" className="mt-2 text-xs text-tone-red">{error}</p>}
    </div>
  );
}
