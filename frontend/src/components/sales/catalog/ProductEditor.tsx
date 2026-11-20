"use client";

import { useMemo, useState } from "react";
import { Button, Checkbox, Dialog, Field, FieldGrid, Input, Select, Textarea } from "@/components/ui";
import { ProductLineSelect } from "@/components/catalog/ProductLineSelect";
import { salesApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { recurringHint } from "@/lib/catalogPricing";
import type { CatalogItem } from "@/lib/types/catalog";
import { useSegments } from "@/lib/segments";
import SegmentInput from "@/components/crm/SegmentInput";
import PriceListEditor from "./PriceListEditor";
import TierEditor from "./TierEditor";
import {
  PRICE_METRICS, PRICING_MODES, PRODUCT_TYPES, SALE_MODELS, emptyProductForm, formToPayload, productFormProblem, productToForm,
  type ProductFormState,
} from "./productForm";

interface Props {
  /** null = a new item. */
  product: CatalogItem | null;
  /** Every item (retired too), for the add-on SKU list and family suggestions. */
  catalog: CatalogItem[];
  currencies: string[];
  canEdit: boolean;
  onClose: () => void;
  /** After a save; the editor stays open on the saved item so prices and tiers can be added. */
  onSaved: (item: CatalogItem) => void;
}

function Section({ title, hint, children }: { title: string; hint?: string; children: React.ReactNode }) {
  return (
    <section className="mt-5 border-t border-line pt-4">
      <h3 className="text-sm font-semibold text-fg">{title}</h3>
      {hint ? <p className="mb-3 mt-0.5 text-xs text-fg-muted">{hint}</p> : <div className="mb-3" />}
      {children}
    </section>
  );
}

export default function ProductEditor({ product, catalog, currencies, canEdit, onClose, onSaved }: Props) {
  const [item, setItem] = useState<CatalogItem | null>(product);
  const [f, setF] = useState<ProductFormState>(() => (product ? productToForm(product) : emptyProductForm()));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [addonFilter, setAddonFilter] = useState("");
  const set = <K extends keyof ProductFormState>(k: K, v: ProductFormState[K]) => setF((p) => ({ ...p, [k]: v }));
  const segments = useSegments();

  const families = useMemo(() => Array.from(new Set(catalog.map((c) => c.family).filter(Boolean))) as string[], [catalog]);
  const skus = useMemo(() => catalog.filter((c) => c.id !== item?.id).map((c) => ({ sku: c.sku, name: c.name, retired: !c.is_active || !c.is_orderable }))
    .sort((a, b) => a.sku.localeCompare(b.sku)), [catalog, item]);
  // cost_price is only returned to catalog managers; only they send it back.
  const includeCost = !product || product.cost_price !== undefined;

  async function reload(id: string) {
    const r = await salesApi.getProduct(id);
    setItem(r.data as CatalogItem);
    onSaved(r.data as CatalogItem);
  }

  async function save() {
    const problem = productFormProblem(f);
    if (problem) { setError(problem); return; }
    setSaving(true); setError("");
    try {
      const payload = formToPayload(f, { includeCost });
      const r = item ? await salesApi.updateProduct(item.id, payload) : await salesApi.createProduct(payload);
      const saved = r.data as CatalogItem;
      if (saved?.id) await reload(saved.id);
      else if (item) await reload(item.id);
    } catch (e) {
      setError(extractApiError(e, "Could not save the item."));
    } finally { setSaving(false); }
  }

  const recurring = f.sale_model === "recurring";
  const ro = !canEdit;

  const num = (k: keyof ProductFormState, step: number | "any" = "any", extra: Partial<React.InputHTMLAttributes<HTMLInputElement>> = {}) => (
    <Input type="number" numeric min={0} step={step} value={f[k] as string} onChange={(e) => set(k, e.target.value as never)} {...extra} />
  );

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} size="xl"
      title={<span className="flex items-baseline gap-3">{item ? (canEdit ? "Edit item" : item.name) : "New catalog item"}
        {item && <span className="font-mono text-xs font-normal text-fg-muted">{item.sku}</span>}</span>}
      description={recurring ? recurringHint({
        sale_model: "recurring", price_metric: f.price_metric,
        rate_period_months: Number(f.rate_period_months) || null,
        billing_interval_months: Number(f.billing_interval_months) || null,
        min_term_months: Number(f.min_term_months) || null,
      }) : undefined}
      footer={<>
        <Button onClick={onClose}>Close</Button>
        {canEdit && <Button variant="primary" loading={saving} onClick={save}>{item ? "Save item" : "Create item"}</Button>}
      </>}>
      <fieldset disabled={ro} className="min-w-0">
        <FieldGrid cols={3}>
          <Field label="SKU" required><Input value={f.sku} onChange={(e) => set("sku", e.target.value)} className="font-mono" /></Field>
          <Field label="Name" required className="md:col-span-2"><Input value={f.name} onChange={(e) => set("name", e.target.value)} /></Field>
          <Field label="Short name"><Input value={f.short_name} onChange={(e) => set("short_name", e.target.value)} /></Field>
          <Field label="Family">
            <Input value={f.family} list="catalog-families" onChange={(e) => set("family", e.target.value)} placeholder="e.g. laptop_pro" />
          </Field>
          <Field label="Category"><Input value={f.category} onChange={(e) => set("category", e.target.value)} /></Field>
          <Field label="Type">
            <Select value={f.product_type} onChange={(e) => set("product_type", e.target.value)} options={PRODUCT_TYPES.map((t) => ({ value: t.value, label: t.label }))} />
          </Field>
          <Field label="Unit label"><Input value={f.unit} onChange={(e) => set("unit", e.target.value)} placeholder="Unit" /></Field>
          <Field label="Product line">
            <ProductLineSelect value={f.product_line_id || null} onChange={(v) => set("product_line_id", v ?? "")} placeholder="None" />
          </Field>
          <Field label="Description" className="md:col-span-3">
            <Textarea value={f.description} rows={2} onChange={(e) => set("description", e.target.value)} className="resize-none" />
          </Field>
        </FieldGrid>
        <datalist id="catalog-families">{families.map((x) => <option key={x} value={x} />)}</datalist>

        <Section title="How it is sold" hint="A recurring price is quoted per period and invoiced every billing interval; a longer term is a multi-year prepay.">
          <FieldGrid cols={4}>
            <Field label="Sale model">
              <Select value={f.sale_model} onChange={(e) => set("sale_model", e.target.value)} options={SALE_MODELS.map((m) => ({ value: m.value, label: m.label }))} />
            </Field>
            <Field label="Price per">
              <Select value={f.price_metric} onChange={(e) => set("price_metric", e.target.value)} options={PRICE_METRICS.map((m) => ({ value: m, label: m }))} />
            </Field>
            <Field label="Min quantity">{num("min_quantity")}</Field>
            <Field label="Care included (months)">{num("included_support_months", 1)}</Field>
            {recurring && (
              <>
                <Field label="Price period (months)">{num("rate_period_months", 1, { min: 1 })}</Field>
                <Field label="Billed every (months)">{num("billing_interval_months", 1, { min: 1 })}</Field>
                <Field label="Min term (months)">{num("min_term_months", 1)}</Field>
              </>
            )}
          </FieldGrid>
        </Section>

        <Section title="Pricing">
          <FieldGrid cols={4}>
            <Field label="Pricing mode" className="md:col-span-2">
              <Select value={f.pricing_mode} onChange={(e) => set("pricing_mode", e.target.value)} options={PRICING_MODES.map((m) => ({ value: m.value, label: m.label }))} />
            </Field>
            {f.pricing_mode === "percent_of_parent" && <Field label="% of parent net price">{num("percent_rate")}</Field>}
            {includeCost && <Field label="Cost price">{num("cost_price")}</Field>}
            <Field label="Price status" hint="A proposed price shows on quotes as proposed; the Owner approves every quotation with one.">
              <Select value={f.price_status} onChange={(e) => set("price_status", e.target.value)}
                options={[{ value: "locked", label: "Locked" }, { value: "proposed", label: "Proposed" }]} />
            </Field>
            <Field label="Segment">
              <SegmentInput segments={segments} value={f.segment} onChange={(v) => set("segment", v)} />
            </Field>
          </FieldGrid>
          {(f.pricing_mode === "manual" || f.pricing_mode === "quote_only") && (
            <p className="mt-2 text-xs text-tone-orange">Staff with the manual-price permission type this price on each quotation; it goes to approval.</p>
          )}
        </Section>

        <Section title="Add-on" hint="An add-on attaches to a line of one of these SKUs (Care, extra modules, white-label). Leave empty for a standalone item.">
          <Input value={addonFilter} onChange={(e) => setAddonFilter(e.target.value)} placeholder="Filter SKUs" aria-label="Filter SKUs" className="mb-2" />
          <div className="grid max-h-40 grid-cols-1 gap-x-3 overflow-y-auto rounded-lg border border-line p-2 md:grid-cols-2">
            {skus.filter((x) => !addonFilter || `${x.sku} ${x.name}`.toLowerCase().includes(addonFilter.toLowerCase())).map((x) => (
              <Checkbox key={x.sku} className={x.retired ? "text-fg-subtle" : undefined}
                checked={f.addon_for.includes(x.sku)}
                onChange={(e) => set("addon_for", e.target.checked ? [...f.addon_for, x.sku] : f.addon_for.filter((y) => y !== x.sku))}
                label={<span className="flex min-w-0 gap-2 text-xs"><span className="font-mono">{x.sku}</span><span className="truncate">{x.name}</span></span>} />
            ))}
          </div>
          {f.addon_for.length > 0 && <p className="mt-1 text-xs text-fg-muted">{f.addon_for.length} parent SKU{f.addon_for.length === 1 ? "" : "s"} selected</p>}
        </Section>

        <Section title="White-label">
          <FieldGrid cols={4} className="items-end">
            <Checkbox className="md:col-span-2" checked={f.is_whitelabel} onChange={(e) => set("is_whitelabel", e.target.checked)}
              label="This item is the white-label option" />
            <Field label="Free for partners from qty (as parent)" className="md:col-span-2">
              {num("whitelabel_free_min_qty", 1, { placeholder: "never free" })}
            </Field>
            {f.is_whitelabel && (
              <>
                <Field label="White-label price" className="md:col-span-2">
                  <Select value={f.whitelabel_mode} onChange={(e) => set("whitelabel_mode", e.target.value as ProductFormState["whitelabel_mode"])}
                    options={[
                      { value: "", label: "Not set (priced manually)" },
                      { value: "percent", label: "% of the parent price" },
                      { value: "fixed", label: "Fixed (from this item's price list)" },
                    ]} />
                </Field>
                {f.whitelabel_mode === "percent" && <Field label="% of parent">{num("whitelabel_value")}</Field>}
              </>
            )}
          </FieldGrid>
        </Section>

        <Section title="Sizing" hint="How many units one item counts as for the payment-term split, and the modules it includes.">
          <FieldGrid cols={4}>
            <Field label="Unit equivalents">{num("unit_equivalents")}</Field>
            <Field label="Modules (comma-separated)" className="md:col-span-4"
              hint="Any names; quotations and proposals list them as they are.">
              <Input value={f.modules} onChange={(e) => set("modules", e.target.value)} placeholder="reporting, api_access" className="font-mono" />
            </Field>
          </FieldGrid>
        </Section>

        <Section title="Quotation text and status">
          <Field label="Terms note (printed under the line)">
            <Textarea value={f.terms_note} rows={2} onChange={(e) => set("terms_note", e.target.value)} className="resize-none" />
          </Field>
          <div className="mt-3 flex gap-5">
            <Checkbox checked={f.is_active} onChange={(e) => set("is_active", e.target.checked)} label="Active" />
            <Checkbox checked={f.is_orderable} onChange={(e) => set("is_orderable", e.target.checked)} label="Orderable" />
          </div>
        </Section>
      </fieldset>

      {error && <p role="alert" className="mt-4 rounded-lg border border-tone-red/30 bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{error}</p>}

      {item ? (
        <>
          <Section title="Price list" hint="One list per currency, effective-dated. A price review adds a row; old quotations keep the price of their date.">
            <PriceListEditor productId={item.id} rows={item.price_rows ?? []} currencies={currencies}
              canEdit={canEdit} onChanged={() => reload(item.id)} />
          </Section>
          <Section title="Volume tiers" hint="Counted per SKU across the quotation's non-optional lines.">
            <TierEditor productId={item.id} tiers={item.volume_tiers ?? []} canEdit={canEdit} onChanged={() => reload(item.id)} />
          </Section>
        </>
      ) : (
        <p className="mt-4 text-xs text-fg-muted">Create the item first, then add its prices and volume tiers.</p>
      )}
    </Dialog>
  );
}
