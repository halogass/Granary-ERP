import { describe, expect, it } from "vitest";
import { formatCurrency } from "./currency";
import { formatCurrency as formatCurrencyOrDash } from "./utils";

describe("formatCurrency: one display style for every currency", () => {
  it("puts the symbol first with no space, comma thousands and dot decimals", () => {
    expect(formatCurrency(3708, "USD")).toBe("$3,708.00");
    expect(formatCurrency(956, "EUR")).toBe("€956.00");
    expect(formatCurrency(1234.5, "GBP")).toBe("£1,234.50");
    expect(formatCurrency(1234.5, "SGD")).toBe("S$1,234.50");
    expect(formatCurrency(1234.5, "CNY")).toBe("¥1,234.50");
  });

  it("keeps each currency's own decimals (JPY has none) and a plain half-width yen", () => {
    expect(formatCurrency(240240, "JPY")).toBe("¥240,240");
    expect(formatCurrency(240240, "jpy")).toBe("¥240,240");
  });

  it("places the minus sign before the symbol", () => {
    expect(formatCurrency(-956, "EUR")).toBe("-€956.00");
  });

  it("falls back safely for unlisted or malformed codes", () => {
    expect(formatCurrency(10, "CHF")).toMatch(/^CHF\s10\.00$/);
    expect(formatCurrency(10, "")).toBe("$10.00");
  });

  it("the utils helper uses the same style and shows a dash for no amount", () => {
    expect(formatCurrencyOrDash(956, "EUR")).toBe("€956.00");
    expect(formatCurrencyOrDash(null, "EUR")).toBe("-");
  });
});
