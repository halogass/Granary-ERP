export const SUPPORTED_CURRENCIES = ["USD", "EUR", "SGD", "GBP", "JPY", "CNY"] as const;
export type CurrencyCode = (typeof SUPPORTED_CURRENCIES)[number];

export const CURRENCY_META: Record<CurrencyCode, { symbol: string; locale: string; decimals: number; name: string }> = {
  USD: { symbol: "$",   locale: "en-US", decimals: 2, name: "US Dollar" },
  EUR: { symbol: "€",   locale: "de-DE", decimals: 2, name: "Euro" },
  SGD: { symbol: "S$",  locale: "en-SG", decimals: 2, name: "Singapore Dollar" },
  GBP: { symbol: "£",   locale: "en-GB", decimals: 2, name: "British Pound" },
  JPY: { symbol: "¥",   locale: "ja-JP", decimals: 0, name: "Japanese Yen" },
  CNY: { symbol: "¥",   locale: "zh-CN", decimals: 2, name: "Chinese Yuan" },
};

/** One display style for every currency: the symbol before the amount, with no space,
 *  "," for thousands and "." for decimals ("$3,708.00", "€956.00", "¥240,240").
 *  Locale-specific layouts ("956,00 €", full-width "￥") made one list read in three
 *  different styles, so the per-currency locale is not used for display. Each currency
 *  keeps its own number of decimals. Only the display changes, never the stored value. */
export function formatCurrency(amount: number, currency: string): string {
  const code = (currency || "").toUpperCase();
  const meta = CURRENCY_META[code as CurrencyCode];
  if (meta) {
    const parts = new Intl.NumberFormat("en-US", {
      style: "currency",
      currency: code,
      maximumFractionDigits: meta.decimals,
      minimumFractionDigits: meta.decimals,
    }).formatToParts(amount);
    // A code shown as letters ("SGD") brings its own spacer; the symbol never needs one.
    return parts
      .filter((p) => !(p.type === "literal" && /^\s+$/.test(p.value)))
      .map((p) => (p.type === "currency" ? meta.symbol : p.value))
      .join("");
  }
  // Unlisted code: let Intl apply that currency's own decimals rather than guessing.
  // A malformed/empty code makes Intl throw a RangeError, which would take down a
  // whole printed document, so that falls back instead.
  const safe = /^[A-Z]{3}$/.test(code) ? code : "USD";
  return new Intl.NumberFormat("en-US", { style: "currency", currency: safe }).format(amount);
}

export function getCurrencySymbol(currency: string): string {
  const code = currency?.toUpperCase() as CurrencyCode;
  return CURRENCY_META[code]?.symbol ?? currency;
}

export async function fetchSpotRate(
  from: string,
  to: string,
  onDate?: string
): Promise<{ rate: number; effective_date: string; source: string } | null> {
  try {
    const params = new URLSearchParams({ from_currency: from, to_currency: to });
    if (onDate) params.set("on_date", onDate);
    const res = await fetch(`/api/v1/finance/fx/rate?${params}`);
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

/** Total a set of records that each carry their own currency.
 *
 *  Adding amounts across different currencies produces a meaningless number, so this
 *  groups by currency and reports the largest subtotal, flagging when other currencies
 *  were left out. The previous behaviour: summing everything and labelling the result
 *  with one code: silently mixed currencies into one figure. Converting instead would need an
 *  FX rate per record and a base-currency setting; see fetchSpotRate. */
export function sumByCurrency<T>(
  rows: T[],
  amount: (row: T) => number | null | undefined,
  currency: (row: T) => string | null | undefined,
  fallback = "USD",
): { total: number; currency: string; mixed: boolean } {
  const totals = new Map<string, number>();
  for (const row of rows) {
    const value = amount(row) ?? 0;
    if (!value) continue;
    const code = (currency(row) || fallback).toUpperCase();
    totals.set(code, (totals.get(code) ?? 0) + value);
  }
  if (totals.size === 0) return { total: 0, currency: fallback, mixed: false };
  const [[code, total]] = [...totals.entries()].sort((a, b) => b[1] - a[1]);
  return { total, currency: code, mixed: totals.size > 1 };
}
