/**
 * Pure helpers for catalog lines in the quotation and invoice forms. None of these produce a
 * price: prices, tiers and tax come from POST /sales/pricing/preview. They only order lines,
 * describe terms and check what an editor typed before it goes to the API.
 */
import type { CatalogItem, PricedLine, VolumeTier } from "@/lib/types/catalog";

// ─── Line tree ────────────────────────────────────────────────────────────────

export interface TreeLine { key: string; parent_key?: string | null }

/**
 * Lines in display order: each top-level line followed by its add-ons (depth-first), keeping the
 * original order among siblings. A line whose parent is missing is shown at the top level.
 */
export function orderLineTree<T extends TreeLine>(lines: T[]): { line: T; depth: number }[] {
  const keys = new Set(lines.map((l) => l.key));
  const children = new Map<string, T[]>();
  const roots: T[] = [];
  for (const l of lines) {
    if (l.parent_key && keys.has(l.parent_key) && l.parent_key !== l.key) {
      const arr = children.get(l.parent_key) ?? [];
      arr.push(l);
      children.set(l.parent_key, arr);
    } else {
      roots.push(l);
    }
  }
  const out: { line: T; depth: number }[] = [];
  const seen = new Set<string>();
  const walk = (l: T, depth: number) => {
    if (seen.has(l.key)) return;      // a parent cycle never loops forever
    seen.add(l.key);
    out.push({ line: l, depth });
    for (const c of children.get(l.key) ?? []) walk(c, depth + 1);
  };
  roots.forEach((r) => walk(r, 0));
  // Lines only reachable through a cycle: show them flat rather than drop them.
  for (const l of lines) if (!seen.has(l.key)) walk(l, 0);
  return out;
}

/** The key and every descendant key (for removing a line with its add-ons). */
export function subtreeKeys(lines: TreeLine[], key: string): Set<string> {
  const out = new Set<string>([key]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const l of lines) {
      if (l.parent_key && out.has(l.parent_key) && !out.has(l.key)) { out.add(l.key); grew = true; }
    }
  }
  return out;
}

/** Index after which a new add-on of `parentKey` goes: after the parent's last descendant. */
export function insertIndexForChild(lines: TreeLine[], parentKey: string): number {
  const sub = subtreeKeys(lines, parentKey);
  let last = -1;
  lines.forEach((l, i) => { if (sub.has(l.key)) last = i; });
  return last + 1;
}

// ─── Terms ────────────────────────────────────────────────────────────────────

type TermFields = Pick<CatalogItem, "billing_interval_months" | "min_term_months" | "rate_period_months">;

/** How many term choices the selector offers (a UI length, not a business rule). */
export const TERM_CHOICES = 5;

/** Months in a calendar year, for labelling only. */
const YEAR = 12;

/**
 * Term options for a recurring line: multiples of the billing interval, starting at the first one
 * that meets the minimum term. `current` is always included so a saved term is never lost.
 */
export function termOptions(item: TermFields, current?: number | null): number[] {
  const step = item.billing_interval_months || item.rate_period_months || YEAR;
  const min = item.min_term_months || step;
  const first = Math.ceil(min / step) * step;
  const opts: number[] = [];
  for (let i = 0; i < TERM_CHOICES; i++) opts.push(first + i * step);
  if (current && !opts.includes(current)) opts.push(current);
  return opts.sort((a, b) => a - b);
}

/** "12 months" → "1 year", "24" → "2 years", other months stay months. */
export function termLabel(months: number): string {
  if (months % YEAR === 0) {
    const y = months / YEAR;
    return `${y} year${y === 1 ? "" : "s"}`;
  }
  return `${months} month${months === 1 ? "" : "s"}`;
}

function periodWord(months: number | null | undefined): string {
  if (!months) return "";
  if (months === 1) return "month";
  if (months === YEAR) return "year";
  if (months % YEAR === 0) return `${months / YEAR} years`;
  return `${months} months`;
}

function billedWord(months: number | null | undefined): string {
  if (!months) return "";
  if (months === 1) return "billed monthly";
  if (months === 3) return "billed quarterly";
  if (months === YEAR) return "billed annually";
  return `billed every ${termLabel(months)}`;
}

/** e.g. "per user / month, billed annually, min 12 months", built only from the item's fields. */
export function recurringHint(item: Partial<TermFields> & { price_metric?: string | null; sale_model?: string | null }): string {
  if (item.sale_model !== "recurring") return "";
  const parts: string[] = [];
  const metric = item.price_metric && item.price_metric !== "unit" && item.price_metric !== "fixed" ? item.price_metric : "";
  const period = periodWord(item.rate_period_months);
  if (metric || period) parts.push(`per ${[metric, period].filter(Boolean).join(" / ")}`);
  const billed = billedWord(item.billing_interval_months);
  if (billed) parts.push(billed);
  if (item.min_term_months) parts.push(`min ${item.min_term_months} month${item.min_term_months === 1 ? "" : "s"}`);
  return parts.join(", ");
}

// ─── Price rule badge ─────────────────────────────────────────────────────────

export interface RuleBadge { label: string; tone: "neutral" | "good" | "warn" | "info" }

/** Badge for a priced line, from the server's price_rule (never re-derived on the client). */
export function priceRuleBadge(p: Pick<PricedLine, "price_rule" | "discount_pct">): RuleBadge {
  switch (p.price_rule) {
    case "volume_tier": return { label: `Volume −${Number(p.discount_pct)}%`, tone: "good" };
    case "manual": return { label: "Manual", tone: "warn" };
    case "converted": return { label: "Converted", tone: "info" };
    case "percent_of_parent": return { label: "% of parent", tone: "neutral" };
    case "whitelabel_free": return { label: "Free (partner)", tone: "good" };
    case "list": return { label: "List", tone: "neutral" };
    default: return { label: String(p.price_rule || ""), tone: "neutral" };
  }
}

// ─── Add-ons ──────────────────────────────────────────────────────────────────

/**
 * Catalog items that can be attached to a line of `parentSku`: add-ons whose addon_for names it,
 * plus white-label items (those with an addon_for list must name it too). Only orderable items.
 */
export function addonChoices(catalog: CatalogItem[], parentSku: string | null | undefined): CatalogItem[] {
  if (!parentSku) return [];
  return catalog.filter((c) => {
    if (!c.is_active || !c.is_orderable) return false;
    const forList = c.addon_for ?? [];
    if (forList.includes(parentSku)) return true;
    return !!c.is_whitelabel && forList.length === 0;
  });
}

// ─── Volume tiers ─────────────────────────────────────────────────────────────

/** The first problem with a tier set as typed, or null. The API checks again (400 on overlap). */
export function tierProblem(tiers: Pick<VolumeTier, "min_qty" | "max_qty" | "discount_pct">[]): string | null {
  for (const [i, t] of tiers.entries()) {
    const n = i + 1;
    if (!Number.isFinite(t.min_qty) || t.min_qty <= 0) return `Tier ${n}: minimum quantity must be more than 0.`;
    if (t.max_qty != null && (!Number.isFinite(t.max_qty) || t.max_qty < t.min_qty))
      return `Tier ${n}: maximum quantity must be at least the minimum.`;
    if (!Number.isFinite(t.discount_pct) || t.discount_pct < 0 || t.discount_pct > 100)
      return `Tier ${n}: discount must be between 0 and 100%.`;
  }
  const sorted = [...tiers].sort((a, b) => a.min_qty - b.min_qty);
  for (let i = 1; i < sorted.length; i++) {
    const prev = sorted[i - 1];
    const cur = sorted[i];
    if (prev.max_qty == null || prev.max_qty >= cur.min_qty) {
      const range = (t: typeof prev) => `${t.min_qty}–${t.max_qty ?? "∞"}`;
      return `Tiers ${range(prev)} and ${range(cur)} overlap.`;
    }
  }
  return null;
}

// ─── Keys ─────────────────────────────────────────────────────────────────────

let _seq = 0;
/** A client-side line key, unique within the page. */
export function newLineKey(): string {
  _seq += 1;
  return `l${Date.now().toString(36)}${_seq.toString(36)}`;
}

/** Re-key configurator lines so they never collide with lines already on the form. */
export function rekeyLines<T extends TreeLine>(lines: T[], mk: () => string = newLineKey): T[] {
  const map = new Map(lines.map((l) => [l.key, mk()] as const));
  return lines.map((l) => ({
    ...l,
    key: map.get(l.key)!,
    parent_key: l.parent_key ? (map.get(l.parent_key) ?? null) : null,
  }));
}

/** Metric and term text for a saved or priced line, e.g. "2 years · per user / month, billed annually". */
export function lineTermText(l: {
  sale_model?: string | null; price_metric?: string | null; rate_period_months?: number | null;
  billing_interval_months?: number | null; term_months?: number | null;
}): string {
  if (l.sale_model === "recurring") {
    return [l.term_months ? termLabel(l.term_months) : "", recurringHint({ ...l, min_term_months: null })]
      .filter(Boolean).join(" · ");
  }
  return l.price_metric && l.price_metric !== "unit" && l.price_metric !== "fixed" ? `per ${l.price_metric}` : "";
}
