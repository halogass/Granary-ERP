/**
 * The catalog item editor's form state (strings, as typed) and its conversion to the API payload
 * of POST/PATCH /sales/catalog. Prices and tiers have their own endpoints and are not part of it.
 */
import type { CatalogItem } from "@/lib/types/catalog";
import { segmentOf, segmentPatch } from "@/lib/segments";

export const PRODUCT_TYPES = [
  { value: "physical", label: "Physical" },
  { value: "software", label: "Software" },
  { value: "saas", label: "SaaS" },
  { value: "service", label: "Service" },
  { value: "bundle", label: "Bundle" },
];

export const SALE_MODELS = [
  { value: "one_time", label: "One-time" },
  { value: "recurring", label: "Recurring" },
];

export const PRICE_METRICS = ["unit", "user", "device", "site", "project", "hour", "day", "fixed"];

export const PRICING_MODES = [
  { value: "list", label: "List price" },
  { value: "percent_of_parent", label: "% of parent line" },
  { value: "manual", label: "Manual (staff type the price)" },
  { value: "quote_only", label: "Quote only" },
];

export interface ProductFormState {
  sku: string; name: string; short_name: string; description: string; category: string; family: string;
  product_type: string; unit: string; product_line_id: string;
  sale_model: string; price_metric: string;
  rate_period_months: string; billing_interval_months: string; min_term_months: string;
  min_quantity: string;
  unit_equivalents: string;
  modules: string;               // comma-separated
  included_support_months: string;
  pricing_mode: string; percent_rate: string;
  addon_for: string[];
  is_whitelabel: boolean; whitelabel_mode: "" | "percent" | "fixed"; whitelabel_value: string;
  whitelabel_free_min_qty: string;
  terms_note: string; cost_price: string;
  segment: string; price_status: string;
  is_active: boolean; is_orderable: boolean;
}

const s = (v: unknown) => (v == null ? "" : String(v));

export function emptyProductForm(): ProductFormState {
  return {
    sku: "", name: "", short_name: "", description: "", category: "", family: "",
    product_type: "physical", unit: "Unit", product_line_id: "",
    sale_model: "one_time", price_metric: "unit",
    rate_period_months: "", billing_interval_months: "", min_term_months: "", min_quantity: "",
    unit_equivalents: "",
    modules: "", included_support_months: "",
    pricing_mode: "list", percent_rate: "", addon_for: [],
    is_whitelabel: false, whitelabel_mode: "", whitelabel_value: "", whitelabel_free_min_qty: "",
    terms_note: "", cost_price: "", is_active: true, is_orderable: true,
    segment: "", price_status: "locked",
  };
}

export function productToForm(p: CatalogItem): ProductFormState {
  return {
    sku: s(p.sku), name: s(p.name), short_name: s(p.short_name), description: s(p.description),
    category: s(p.category), family: s(p.family), product_type: s(p.product_type) || "physical",
    unit: s(p.unit), product_line_id: s(p.product_line_id),
    sale_model: p.sale_model || "one_time", price_metric: p.price_metric || "unit",
    rate_period_months: s(p.rate_period_months), billing_interval_months: s(p.billing_interval_months),
    min_term_months: s(p.min_term_months), min_quantity: s(p.min_quantity),
    unit_equivalents: s(p.unit_equivalents), modules: (p.modules ?? []).join(", "),
    included_support_months: s(p.included_support_months),
    pricing_mode: p.pricing_mode || "list", percent_rate: s(p.percent_rate),
    addon_for: [...(p.addon_for ?? [])],
    is_whitelabel: !!p.is_whitelabel,
    whitelabel_mode: p.whitelabel_rule?.mode ?? "",
    whitelabel_value: s(p.whitelabel_rule?.value),
    whitelabel_free_min_qty: s(p.whitelabel_free_min_qty),
    terms_note: s(p.terms_note), cost_price: s(p.cost_price),
    segment: segmentOf(p), price_status: p.price_status || "locked",
    is_active: p.is_active !== false, is_orderable: p.is_orderable !== false,
  };
}

const num = (v: string): number | null => {
  if (v.trim() === "") return null;
  const n = Number(v);
  return Number.isFinite(n) ? n : null;
};
const int = (v: string): number | null => {
  const n = num(v);
  return n == null ? null : Math.trunc(n);
};
const txt = (v: string): string | null => (v.trim() === "" ? null : v.trim());

/** The first thing missing or inconsistent in the form, or null. The API validates again. */
export function productFormProblem(f: ProductFormState): string | null {
  if (!f.sku.trim()) return "SKU is required.";
  if (!f.name.trim()) return "Name is required.";
  if (f.sale_model === "recurring" && !int(f.rate_period_months)) return "A recurring item needs the months its price is quoted per.";
  if (f.pricing_mode === "percent_of_parent") {
    if (num(f.percent_rate) == null) return "Enter the percentage of the parent price.";
    if (!f.addon_for.length) return "A % of parent item must name the SKUs it attaches to.";
  }
  if (f.is_whitelabel && f.whitelabel_mode === "percent" && num(f.whitelabel_value) == null)
    return "Enter the white-label percentage.";
  return null;
}

/** API payload. Fields that do not apply to the chosen model/mode are sent as null. */
export function formToPayload(f: ProductFormState, opts: { includeCost: boolean }): Record<string, unknown> {
  const recurring = f.sale_model === "recurring";
  const percent = f.pricing_mode === "percent_of_parent";
  const payload: Record<string, unknown> = {
    sku: f.sku.trim(), name: f.name.trim(), short_name: txt(f.short_name), description: txt(f.description),
    category: txt(f.category), family: txt(f.family), product_type: f.product_type,
    unit: txt(f.unit) ?? "Unit", product_line_id: f.product_line_id || null,
    sale_model: f.sale_model, price_metric: f.price_metric,
    rate_period_months: recurring ? int(f.rate_period_months) : null,
    billing_interval_months: recurring ? int(f.billing_interval_months) : null,
    min_term_months: recurring ? int(f.min_term_months) : null,
    min_quantity: num(f.min_quantity),
    unit_equivalents: num(f.unit_equivalents),
    modules: f.modules.split(",").map((m) => m.trim()).filter(Boolean),
    included_support_months: int(f.included_support_months),
    pricing_mode: f.pricing_mode,
    percent_rate: percent ? num(f.percent_rate) : null,
    addon_for: f.addon_for,
    is_whitelabel: f.is_whitelabel,
    whitelabel_rule: f.is_whitelabel && f.whitelabel_mode
      ? { mode: f.whitelabel_mode, ...(f.whitelabel_mode === "percent" ? { value: num(f.whitelabel_value) } : {}) }
      : null,
    whitelabel_free_min_qty: int(f.whitelabel_free_min_qty),
    terms_note: txt(f.terms_note),
    ...segmentPatch(f.segment), price_status: f.price_status || "locked",
    is_active: f.is_active, is_orderable: f.is_orderable,
  };
  if (opts.includeCost) {
    payload.cost_price = num(f.cost_price);
  }
  return payload;
}
