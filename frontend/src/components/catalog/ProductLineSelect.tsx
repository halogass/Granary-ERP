"use client";
import { useEffect, useState } from "react";
import { catalogApi } from "@/lib/api";
import { Select } from "@/components/ui";
import { cn } from "@/lib/utils";

export interface ProductLine { id: string; code: string; name: string; color: string; category?: string }

// Module-level cache so many selects on a page share one fetch.
let _cache: ProductLine[] | null = null;
let _inflight: Promise<ProductLine[]> | null = null;

export function useProductLines(): ProductLine[] {
  const [lines, setLines] = useState<ProductLine[]>(_cache ?? []);
  useEffect(() => {
    if (_cache) { setLines(_cache); return; }
    if (!_inflight) {
      _inflight = catalogApi.list()
        .then((r) => { const d: ProductLine[] = r.data ?? []; _cache = d; return d; })
        .catch(() => { const d: ProductLine[] = []; _cache = d; return d; });
    }
    _inflight.then((d) => setLines(d));
  }, []);
  return lines;
}

/** Resolve a product line by id or code (for badges elsewhere). */
export function lineLookup(lines: ProductLine[]) {
  const byId = new Map(lines.map((l) => [l.id, l] as [string, ProductLine]));
  const byCode = new Map(lines.map((l) => [l.code, l] as [string, ProductLine]));
  return (key?: string | null) => (key ? byId.get(key) ?? byCode.get(key) ?? null : null);
}

/** Single-select bound to an id (default) or code. */
export function ProductLineSelect({ value, onChange, valueKey = "id", placeholder = "None", className }: {
  value: string | null | undefined;
  onChange: (v: string | null) => void;
  valueKey?: "id" | "code";
  placeholder?: string;
  className?: string;
}) {
  const lines = useProductLines();
  const sel = lines.find((l) => l[valueKey] === value) || null;
  return (
    <div className="flex items-center gap-2">
      {/* the line's own color (data, set per product line) */}
      {sel && <span className="size-2.5 shrink-0 rounded-full" style={{ background: sel.color }} aria-hidden />}
      <div className="min-w-0 flex-1">
        <Select value={value ?? ""} onChange={(e) => onChange(e.target.value || null)} className={className}
          placeholder={placeholder} options={lines.map((l) => ({ value: l[valueKey], label: l.name }))} />
      </div>
    </div>
  );
}

/** Multi-select as toggle chips, bound to an array of ids (default) or codes. */
export function ProductLineMultiSelect({ value, onChange, valueKey = "code" }: {
  value: string[];
  onChange: (v: string[]) => void;
  valueKey?: "id" | "code";
}) {
  const lines = useProductLines();
  const set = new Set(value);
  const toggle = (k: string) => {
    const next = new Set(set);
    next.has(k) ? next.delete(k) : next.add(k);
    onChange([...next]);
  };
  return (
    <div className="flex flex-wrap gap-1.5">
      {lines.map((l) => {
        const k = l[valueKey];
        const on = set.has(k);
        return (
          <button key={l.id} type="button" onClick={() => toggle(k)} aria-pressed={on}
            className={cn("inline-flex min-h-9 items-center rounded-full border px-2.5 text-xs font-medium transition-colors md:min-h-7",
              !on && "border-line bg-surface text-fg-muted hover:bg-surface-2")}
            style={on ? { background: l.color + "1a", color: l.color, borderColor: l.color + "55" } : undefined}>
            {l.name}
          </button>
        );
      })}
      {lines.length === 0 && <span className="text-xs text-fg-muted">No product lines defined.</span>}
    </div>
  );
}
