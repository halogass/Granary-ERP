import { describe, it, expect } from "vitest";
import {
  addonChoices, insertIndexForChild, lineTermText, orderLineTree, priceRuleBadge, recurringHint, rekeyLines,
  subtreeKeys, termLabel, termOptions, tierProblem, TERM_CHOICES,
} from "./catalogPricing";
import type { CatalogItem } from "@/lib/types/catalog";

const item = (over: Partial<CatalogItem>): CatalogItem => ({
  id: over.sku ?? "x", sku: "X", name: "X", product_type: "physical", sale_model: "one_time", price_metric: "unit",
  pricing_mode: "list", is_active: true, is_orderable: true, ...over,
});

describe("orderLineTree", () => {
  it("puts add-ons right after their parent, depth-first, keeping sibling order", () => {
    const lines = [
      { key: "a" }, { key: "b" }, { key: "a1", parent_key: "a" }, { key: "b1", parent_key: "b" },
      { key: "a2", parent_key: "a" }, { key: "a1x", parent_key: "a1" },
    ];
    expect(orderLineTree(lines).map(({ line, depth }) => `${line.key}:${depth}`))
      .toEqual(["a:0", "a1:1", "a1x:2", "a2:1", "b:0", "b1:1"]);
  });

  it("shows a line whose parent is missing at the top level", () => {
    expect(orderLineTree([{ key: "c", parent_key: "gone" }]).map((x) => x.depth)).toEqual([0]);
  });

  it("never loops on a parent cycle and drops no line", () => {
    const out = orderLineTree([{ key: "p", parent_key: "q" }, { key: "q", parent_key: "p" }]);
    expect(out.map((x) => x.line.key).sort()).toEqual(["p", "q"]);
  });
});

describe("subtree helpers", () => {
  const lines = [{ key: "a" }, { key: "a1", parent_key: "a" }, { key: "b" }, { key: "a1x", parent_key: "a1" }];
  it("collects a line and all its descendants", () => {
    expect([...subtreeKeys(lines, "a")].sort()).toEqual(["a", "a1", "a1x"]);
  });
  it("inserts a new add-on after the parent's last descendant", () => {
    expect(insertIndexForChild(lines, "a")).toBe(4);
    expect(insertIndexForChild(lines, "b")).toBe(3);
  });
});

describe("termOptions", () => {
  it("offers multiples of the billing interval from the minimum term", () => {
    const opts = termOptions({ billing_interval_months: 12, min_term_months: 12, rate_period_months: 1 });
    expect(opts).toHaveLength(TERM_CHOICES);
    expect(opts.slice(0, 3)).toEqual([12, 24, 36]);
  });
  it("rounds the minimum up to a whole billing interval", () => {
    expect(termOptions({ billing_interval_months: 12, min_term_months: 18, rate_period_months: 1 })[0]).toBe(24);
  });
  it("falls back to the rate period when there is no billing interval", () => {
    expect(termOptions({ billing_interval_months: null, min_term_months: null, rate_period_months: 12 })[0]).toBe(12);
  });
  it("keeps a saved term that is not one of the options", () => {
    expect(termOptions({ billing_interval_months: 12, min_term_months: 12, rate_period_months: 1 }, 6)).toContain(6);
  });
});

describe("term text", () => {
  it("labels whole years as years", () => {
    expect(termLabel(12)).toBe("1 year");
    expect(termLabel(36)).toBe("3 years");
    expect(termLabel(6)).toBe("6 months");
  });
  it("builds the recurring hint from the item's fields", () => {
    expect(recurringHint({ sale_model: "recurring", price_metric: "user", rate_period_months: 1, billing_interval_months: 12, min_term_months: 12 }))
      .toBe("per user / month, billed annually, min 12 months");
    expect(recurringHint({ sale_model: "one_time", price_metric: "user" })).toBe("");
  });
  it("describes a saved line's term and metric", () => {
    expect(lineTermText({ sale_model: "recurring", price_metric: "user", rate_period_months: 1, billing_interval_months: 12, term_months: 24 }))
      .toBe("2 years · per user / month, billed annually");
    expect(lineTermText({ sale_model: "one_time", price_metric: "lane" })).toBe("per lane");
    expect(lineTermText({ sale_model: "one_time", price_metric: "unit" })).toBe("");
  });
});

describe("priceRuleBadge", () => {
  it("shows the tier discount the server applied", () => {
    expect(priceRuleBadge({ price_rule: "volume_tier", discount_pct: 5 }).label).toBe("Volume −5%");
    expect(priceRuleBadge({ price_rule: "manual", discount_pct: 0 }).label).toBe("Manual");
    expect(priceRuleBadge({ price_rule: "converted", discount_pct: 0 }).label).toBe("Converted");
  });
});

describe("addonChoices", () => {
  const catalog = [
    item({ sku: "CARE", addon_for: ["BOX", "SRV"] }),
    item({ sku: "MOD2", addon_for: ["SRV"] }),
    item({ sku: "WL", is_whitelabel: true, addon_for: [] }),
    item({ sku: "WL-SRV", is_whitelabel: true, addon_for: ["SRV"] }),
    item({ sku: "OLD", addon_for: ["BOX"], is_orderable: false }),
    item({ sku: "BOX" }),
  ];
  it("lists add-ons for the parent SKU plus general white-label, never retired items", () => {
    expect(addonChoices(catalog, "BOX").map((c) => c.sku)).toEqual(["CARE", "WL"]);
    expect(addonChoices(catalog, "SRV").map((c) => c.sku)).toEqual(["CARE", "MOD2", "WL", "WL-SRV"]);
    expect(addonChoices(catalog, null)).toEqual([]);
  });
});

describe("tierProblem", () => {
  it("accepts contiguous tiers ending open", () => {
    expect(tierProblem([
      { min_qty: 21, max_qty: 100, discount_pct: 5 },
      { min_qty: 101, max_qty: null, discount_pct: 7 },
    ])).toBeNull();
  });
  it("finds overlapping ranges in any order", () => {
    expect(tierProblem([
      { min_qty: 50, max_qty: null, discount_pct: 7 },
      { min_qty: 21, max_qty: 60, discount_pct: 5 },
    ])).toMatch(/overlap/);
  });
  it("rejects an open tier followed by another", () => {
    expect(tierProblem([
      { min_qty: 1, max_qty: null, discount_pct: 5 },
      { min_qty: 10, max_qty: null, discount_pct: 7 },
    ])).toMatch(/overlap/);
  });
  it("rejects a max below the min and a discount over 100%", () => {
    expect(tierProblem([{ min_qty: 10, max_qty: 5, discount_pct: 5 }])).toMatch(/maximum/);
    expect(tierProblem([{ min_qty: 1, max_qty: null, discount_pct: 120 }])).toMatch(/discount/);
    expect(tierProblem([{ min_qty: NaN, max_qty: null, discount_pct: 1 }])).toMatch(/minimum/);
    expect(tierProblem([{ min_qty: 0, max_qty: 5, discount_pct: 1 }])).toMatch(/minimum/);
  });
});

describe("rekeyLines", () => {
  it("gives configurator lines fresh keys and keeps parent links", () => {
    let n = 0;
    const out = rekeyLines([{ key: "1" }, { key: "2", parent_key: "1" }], () => `k${++n}`);
    expect(out).toEqual([{ key: "k1", parent_key: null }, { key: "k2", parent_key: "k1" }]);
  });
});
