import { describe, it, expect } from "vitest";
import { emptyProductForm, formToPayload, productFormProblem, productToForm } from "./productForm";
import type { CatalogItem } from "@/lib/types/catalog";

describe("catalog item form", () => {
  it("round-trips an item through the form", () => {
    const item: CatalogItem = {
      id: "1", sku: "SUPPORT", name: "Support", product_type: "service", sale_model: "recurring", price_metric: "user",
      rate_period_months: 12, billing_interval_months: 12, min_term_months: 12, pricing_mode: "percent_of_parent",
      percent_rate: 18, addon_for: ["BASE"], modules: ["reporting", "api_access"], is_whitelabel: false,
      is_active: true, is_orderable: true, cost_price: 3,
    };
    const p = formToPayload(productToForm(item), { includeCost: true });
    expect(p).toMatchObject({
      sku: "SUPPORT", sale_model: "recurring", rate_period_months: 12, billing_interval_months: 12, min_term_months: 12,
      pricing_mode: "percent_of_parent", percent_rate: 18, addon_for: ["BASE"],
      modules: ["reporting", "api_access"], cost_price: 3, whitelabel_rule: null,
    });
  });

  it("clears recurring and percent fields that do not apply", () => {
    const f = { ...emptyProductForm(), sku: "A", name: "A", rate_period_months: "12", percent_rate: "30" };
    const p = formToPayload(f, { includeCost: false });
    expect(p.rate_period_months).toBeNull();
    expect(p.percent_rate).toBeNull();
    expect("cost_price" in p).toBe(false);
  });

  it("sends the white-label rule only for a white-label item", () => {
    const f = { ...emptyProductForm(), sku: "WL", name: "WL", is_whitelabel: true, whitelabel_mode: "percent" as const, whitelabel_value: "10" };
    expect(formToPayload(f, { includeCost: false }).whitelabel_rule).toEqual({ mode: "percent", value: 10 });
    expect(formToPayload({ ...f, whitelabel_mode: "fixed" }, { includeCost: false }).whitelabel_rule).toEqual({ mode: "fixed" });
    expect(formToPayload({ ...f, is_whitelabel: false }, { includeCost: false }).whitelabel_rule).toBeNull();
  });

  it("names what is missing", () => {
    expect(productFormProblem(emptyProductForm())).toMatch(/SKU/);
    const base = { ...emptyProductForm(), sku: "A", name: "A" };
    expect(productFormProblem({ ...base, sale_model: "recurring" })).toMatch(/recurring/);
    expect(productFormProblem({ ...base, pricing_mode: "percent_of_parent", percent_rate: "18" })).toMatch(/SKUs/);
    expect(productFormProblem(base)).toBeNull();
  });

  it("carries the segment and the price status", () => {
    const item: CatalogItem = {
      id: "2", sku: "SRV-12", name: "Server 12", product_type: "physical", sale_model: "one_time",
      price_metric: "unit", pricing_mode: "list", is_active: true, is_orderable: true, cost_price: 4000,
      segment: "retail", price_status: "proposed",
    };
    const f = productToForm(item);
    expect(f).toMatchObject({ segment: "retail", price_status: "proposed" });
    expect(formToPayload(f, { includeCost: true })).toMatchObject({ segment: "retail", price_status: "proposed", cost_price: 4000 });
    const fresh = formToPayload({ ...emptyProductForm(), sku: "N", name: "N" }, { includeCost: false });
    expect(fresh).toMatchObject({ segment: null, price_status: "locked" });
  });
});
