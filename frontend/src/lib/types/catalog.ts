/**
 * Shapes of the sales catalog and the pricing service (backend/app/routers/sales.py,
 * services/pricing.py). Prices, tiers, rates and terms are all data from the API: the UI never
 * computes a list price, a tier or an FX conversion itself.
 */

export type ProductType = "physical" | "software" | "saas" | "service" | "bundle";
export type SaleModel = "one_time" | "recurring";
export type PricingMode = "list" | "percent_of_parent" | "manual" | "quote_only";
export type PriceRule =
  | "list" | "converted" | "volume_tier" | "percent_of_parent" | "whitelabel_free" | "manual";

export interface CatalogPriceRow {
  id: string;
  currency: string;
  amount: number;
  valid_from: string;
  valid_to: string | null;
}

export interface VolumeTier {
  id?: string;
  min_qty: number;
  max_qty: number | null;
  discount_pct: number;
}

export interface WhitelabelRule {
  mode: "percent" | "fixed";
  value?: number | null;
}

export interface CatalogItem {
  id: string;
  sku: string;
  name: string;
  short_name?: string | null;
  description?: string | null;
  category?: string | null;
  family?: string | null;
  product_type: ProductType | string;
  unit?: string | null;
  sale_model: SaleModel;
  price_metric: string;
  rate_period_months?: number | null;
  billing_interval_months?: number | null;
  min_quantity?: number | null;
  min_term_months?: number | null;
  unit_equivalents?: number | null;
  modules?: string[] | null;
  included_support_months?: number | null;
  pricing_mode: PricingMode;
  percent_rate?: number | null;
  addon_for?: string[] | null;
  is_whitelabel?: boolean;
  whitelabel_rule?: WhitelabelRule | null;
  whitelabel_free_min_qty?: number | null;
  terms_note?: string | null;
  cost_price?: number | null;
  /** The item's segment (read and write it through lib/segments.ts). */
  segment?: string | null;
  /** A proposed price is shown as such on quotes; a quotation with one goes to the Owner. */
  price_status?: PriceStatus | null;
  is_active: boolean;
  is_orderable: boolean;
  product_line_id?: string | null;
  price_rows?: CatalogPriceRow[];
  volume_tiers?: VolumeTier[];
  /** Today's own list price per currency (no conversions). */
  current_prices?: Record<string, number>;
}

/** A line sent to POST /sales/pricing/preview (services/pricing.LineIn). */
export interface LineIn {
  key: string;
  product_id?: string | null;
  name?: string | null;
  description?: string | null;
  unit?: string | null;
  quantity: number;
  /** Omitted/null = the expected (list) price. */
  unit_price?: number | null;
  discount_pct?: number | null;
  discount_type?: string | null;
  is_optional?: boolean;
  parent_key?: string | null;
  term_months?: number | null;
  tax_id?: string | null;
  /** Custom rate: tax_id null and tax_pct set. */
  tax_pct?: number | null;
}

/** One priced line from the server (services/pricing.PricedLine). */
export interface PricedLine {
  key: string;
  product_id: string | null;
  parent_key: string | null;
  sku: string | null;
  name: string;
  description: string | null;
  unit: string | null;
  sale_model: SaleModel | null;
  price_metric: string | null;
  rate_period_months: number | null;
  billing_interval_months: number | null;
  term_months: number | null;
  periods: number;
  /** unit price × periods, rounded by the server (exact); absent from an older backend */
  term_unit_price?: number;
  quantity: number;
  list_unit_price: number | null;
  unit_price: number;
  discount_pct: number;
  discount_type: string | null;
  price_rule: PriceRule | string;
  is_list: boolean;
  amount: number;
  /** The volume tier this SKU qualifies for: an internal negotiation hint, never applied by itself. */
  tier_available_pct?: number;
  tax_id: string | null;
  tax_code: string | null;
  tax_pct: number;
  tax_amount: number;
  terms_note: string | null;
  notes: string[];
  is_optional: boolean;
  unit_equivalents?: number | null;
  product_type?: string | null;
  cost_price?: number | null;
}

export interface TaxLine {
  tax_id?: string | null;
  code?: string | null;
  name?: string | null;
  pct?: number;
  rate?: number;
  base?: number;
  amount: number;
}

export interface DefaultTax { id: string | null; code: string | null; pct: number }

export interface PaymentTermsSuggestion {
  text: string;
  schedule: { label: string; percentage: number; due_trigger: string }[];
}

export interface PricingPreview {
  lines: PricedLine[];
  subtotal: number;
  /** Present when the server applies a header discount. */
  discount_amount?: number;
  tax_lines: TaxLine[];
  tax_amount: number;
  total: number;
  all_list: boolean;
  default_tax: DefaultTax | null;
  payment_terms?: PaymentTermsSuggestion | null;
  /** Optional lines are not in subtotal/tax/total: their own figures (4 Nov 2026). */
  optional_subtotal?: number;
  optional_tax?: number;
  optional_total?: number;
  total_with_optional?: number;
}

export type PriceStatus = "locked" | "proposed";
