"use client";

import { useState, useEffect, useCallback } from "react";
import { useRouter } from "next/navigation";
import { financeApi, systemApi, salesApi } from "@/lib/api";
import { Plus, Trash2, Building2, Receipt, AlertCircle, RefreshCw } from "lucide-react";
import Link from "next/link";
import RichTextEditor from "@/components/ui/RichTextEditor";
import NoteTemplatePicker from "@/components/notes/NoteTemplatePicker";
import { formatCurrency, getCurrencySymbol, CURRENCY_META } from "@/lib/currency";
import { extractApiError } from "@/lib/apiError";
import {
  Button, Card, CardBody, CardFooter, CardHeader, Combobox, Field, FieldGrid, IconButton, Input, PageHeader, Select, Switch,
} from "@/components/ui";
import { DEFAULT_TAX_NOTE, hasTaxNote, taxConflict, withTaxNote, withoutTaxNote, type TaxNotes } from "@/lib/taxNotes";
import { cn } from "@/lib/utils";
import OrgContactPicker from "@/components/crm/OrgContactPicker";
import { catalogSource } from "@/lib/entitySources";
import { newLineKey, recurringHint, termLabel } from "@/lib/catalogPricing";

export interface InvoiceLine {
  name: string; description: string; product_id: string;
  qty: number; unit_price: number; discount_pct: number;
  /** Tax row from Finance > Taxes; the server snapshots its rate. tax_pct is for display only,
   *  except on a custom-rate line (tax_custom), where it is the typed rate sent to the server. */
  tax_id: string; tax_pct: number;
  tax_custom?: boolean;
  /** Client key (the saved item id when editing) and the parent line's key, for add-ons. */
  key?: string; parent_key?: string | null;
  /** Catalog snapshot carried from the pricing preview or the quotation. */
  sku?: string | null; term_months?: number | null; price_rule?: string | null;
  /** UI only: the price source or a pricing error. */
  price_note?: string; price_warning?: boolean;
}
export interface InvoiceFormValues {
  invoice_type: string; issue_date: string; due_date: string;
  customer_org_id: string; customer_contact_id: string; notes: string;
  payment_terms: string; currency: string; exchange_rate: number | null;
  /** Where "Send" delivers the invoice : used before the CRM contact's details. */
  billing_email: string; billing_whatsapp: string;
}

interface TaxRule { id: string; name: string; rate: number; code?: string }
interface TaxConfig { id?: string; name: string; rules: TaxRule[] }
export const emptyInvoiceLine = (): InvoiceLine => ({
  key: newLineKey(), name: "", description: "", product_id: "", qty: 1, unit_price: 0, discount_pct: 0, tax_id: "", tax_pct: 0,
});

/** A saved line without a tax row but with a rate is a custom-rate line. */
const normaliseLine = (l: InvoiceLine): InvoiceLine => ({
  ...l,
  key: l.key || newLineKey(),
  tax_custom: l.tax_custom ?? (!l.tax_id && Number(l.tax_pct) > 0),
});

/** Round half away from zero to `places` decimals ; the server's rule (app/core/money.py).
 *  `places` comes from the currency.decimals setting; unknown means no rounding (the server decides). */
function roundMoney(x: number, places: number | undefined): number {
  if (places === undefined) return x;
  const f = 10 ** places;
  return Math.sign(x) * Math.round(Math.abs(x) * f + 1e-9) / f;
}

/** A line's amounts exactly as the server will compute them: each step rounded for the currency. */
function lineAmounts(l: InvoiceLine, places: number | undefined) {
  const gross = roundMoney(l.qty * l.unit_price, places);
  const discount = roundMoney(gross * l.discount_pct / 100, places);
  const net = gross - discount;
  const tax = roundMoney(net * l.tax_pct / 100, places);
  return { gross, discount, net, tax };
}

interface Props {
  mode: "create" | "edit";
  invoiceId?: string;
  initialForm?: Partial<InvoiceFormValues>;
  initialLines?: InvoiceLine[];
}

export default function InvoiceForm({ mode, invoiceId, initialForm, initialLines }: Props) {
  const router = useRouter();
  const isEdit = mode === "edit";
  const [form, setForm] = useState<InvoiceFormValues>({
    invoice_type: "commercial",
    issue_date: new Date().toISOString().slice(0, 10),
    due_date: "", customer_org_id: "", customer_contact_id: "", notes: "",
    payment_terms: "30", currency: "USD", exchange_rate: null,
    billing_email: "", billing_whatsapp: "",
    ...(initialForm || {}),
  });
  const [lines, setLines] = useState<InvoiceLine[]>(initialLines && initialLines.length ? initialLines.map(normaliseLine) : [emptyInvoiceLine()]);
  const [taxConfig, setTaxConfig] = useState<TaxConfig | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [activeCurrencies, setActiveCurrencies] = useState<string[]>(["USD", "EUR"]);
  const [functionalCurrency, setFunctionalCurrency] = useState("USD");
  const [currencyDecimals, setCurrencyDecimals] = useState<Record<string, number>>({});
  const [fetchingRate, setFetchingRate] = useState(false);

  const loadData = useCallback(async () => {
    try {
      const [taxRes, currRes] = await Promise.all([
        financeApi.listTaxes({ active_only: true }),
        systemApi.getCurrencySettings().catch(() => null),
      ]);
      const taxes: { id: string; name: string; code: string; rate: number; applies_to_sales: boolean }[] =
        (taxRes.data.items ?? taxRes.data ?? []).filter((t: { applies_to_sales?: boolean }) => t.applies_to_sales !== false);
      setTaxConfig(taxes.length
        ? { name: "Finance Taxes", rules: taxes.map((t) => ({ id: t.id, name: t.name, code: t.code, rate: t.rate })) }
        : null);
      if (currRes?.data) {
        const active: string[] = currRes.data.active ?? ["USD", "EUR"];
        const functional: string = currRes.data.functional ?? "USD";
        setActiveCurrencies(active);
        setFunctionalCurrency(functional);
        setCurrencyDecimals(currRes.data.decimals ?? {});
        if (!isEdit) setForm(f => ({ ...f, currency: functional }));
      }
    } catch {}
  }, [isEdit]);

  useEffect(() => { loadData(); }, [loadData]);

  // 2 Nov 2026: "Prices exclude tax": every line no tax and the notes say so (the sentence from Settings).
  // A customer quoted without tax gets no tax from the pricing preview already; a line can still be set by hand.
  const [taxCfg, setTaxCfg] = useState<TaxNotes | null>(null);
  useEffect(() => {
    salesApi.taxNotes()
      .then((r) => setTaxCfg(r.data))
      .catch(() => setTaxCfg({ note: DEFAULT_TAX_NOTE, exclusive_phrases: [], inclusive_phrases: [] }));
  }, []);
  const taxNote = taxCfg?.note ?? DEFAULT_TAX_NOTE;
  const pricesExcludeTax = hasTaxNote(form.notes, taxNote);
  function setPricesExcludeTax(on: boolean) {
    if (on) setLines((prev) => prev.map((l) => ({ ...l, tax_id: "", tax_pct: 0, tax_custom: false })));
    setForm((f) => ({ ...f, notes: on ? withTaxNote(f.notes, taxNote) : withoutTaxNote(f.notes, taxNote) }));
  }

  const fetchRate = useCallback(async (currency: string, issueDate: string, funcCur: string) => {
    if (currency === funcCur) { setForm(f => ({ ...f, exchange_rate: null })); return; }
    setFetchingRate(true);
    try {
      const res = await financeApi.getSpotRate(currency, funcCur, issueDate);
      setForm(f => ({ ...f, exchange_rate: res.data?.rate ?? null }));
    } catch {
      setForm(f => ({ ...f, exchange_rate: null }));
    } finally { setFetchingRate(false); }
  }, []);

  useEffect(() => {
    if (form.currency !== functionalCurrency) fetchRate(form.currency, form.issue_date, functionalCurrency);
  }, [form.currency, form.issue_date, functionalCurrency, fetchRate]);

  function updateLine(i: number, patch: Partial<InvoiceLine>) {
    setLines((prev) => prev.map((l, idx) => idx === i ? { ...l, ...patch } : l));
  }

  /** Price a catalog line through the pricing service (list price, tier, tax for this customer).
   *  A recurring line is invoiced for its whole term: unit price x periods, with the term in the
   *  description. An add-on is priced with its parent line, when the parent is a catalog line. */
  async function pickProduct(i: number, productId: string) {
    if (!productId) { updateLine(i, { product_id: "", sku: null, price_rule: null, term_months: null, price_note: undefined, price_warning: false }); return; }
    const line = lines[i];
    const key = line.key || newLineKey();
    const parent = line.parent_key ? lines.find((l) => l.key === line.parent_key && l.product_id) : undefined;
    try {
      const r = await salesApi.pricingPreview({
        currency: form.currency, issue_date: form.issue_date || undefined,
        customer_org_id: form.customer_org_id || undefined,
        lines: [
          ...(parent ? [{ key: parent.key!, product_id: parent.product_id, quantity: parent.qty, term_months: parent.term_months ?? null }] : []),
          { key, product_id: productId, quantity: line.qty || 1, parent_key: parent ? parent.key : null,
            term_months: line.product_id === productId ? (line.term_months ?? null) : null },
        ],
      });
      const p = r.data.lines.find((l) => l.key === key);
      if (!p) return;
      const periods = Number(p.periods) || 1;
      const recurring = p.sale_model === "recurring";
      const hint = recurring ? [p.term_months ? termLabel(p.term_months) : "", recurringHint({ ...p, min_term_months: null })].filter(Boolean).join(", ") : "";
      updateLine(i, {
        key, product_id: productId, sku: p.sku, name: p.name,
        description: hint || line.description,
        qty: Number(p.quantity) || line.qty,
        // the whole term's price as the server rounded it (exact); the JS product only for an older backend
        unit_price: recurring
          ? (p.term_unit_price != null ? Number(p.term_unit_price) : roundMoney(Number(p.unit_price) * periods, currencyDecimals[form.currency]))
          : Number(p.unit_price),
        discount_pct: Number(p.discount_pct) || 0,
        ...(pricesExcludeTax
          ? { tax_id: "", tax_pct: 0, tax_custom: false }
          : { tax_id: p.tax_id ?? "", tax_pct: Number(p.tax_pct) || 0, tax_custom: !p.tax_id && Number(p.tax_pct) > 0 }),
        term_months: p.term_months, price_rule: p.price_rule,
        price_note: p.price_rule === "converted" ? "Converted from the base price list" : (p.notes ?? []).join(" ") || undefined,
        price_warning: false,
      });
    } catch (e) {
      updateLine(i, { product_id: productId, price_warning: true, price_note: extractApiError(e, "Could not price that product. Enter the price.") });
    }
  }

  const amounts = lines.map((l) => lineAmounts(l, currencyDecimals[form.currency]));
  const grossTotal = amounts.reduce((s, a) => s + a.gross, 0);
  const discountTotal = amounts.reduce((s, a) => s + a.discount, 0);
  const subtotal = amounts.reduce((s, a) => s + a.net, 0);
  const taxAmount = amounts.reduce((s, a) => s + a.tax, 0);
  const total = subtotal + taxAmount;
  const isForeign = form.currency !== functionalCurrency;
  const rate = form.exchange_rate ?? 1;
  const currencySymbol = getCurrencySymbol(form.currency);
  const backHref = isEdit && invoiceId ? `/finance/invoices/${invoiceId}` : "/finance/invoices";

  async function handleSave() {
    if (!form.due_date) { setError("Due date is required."); return; }
    if (lines.every((l) => !(l.name || l.description).trim())) { setError("Add at least one line item."); return; }
    if (form.billing_email.trim() && !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(form.billing_email.trim())) {
      setError("Billing email is not a valid email address."); return;
    }
    setError(""); setSaving(true);
    // An add-on whose parent line was removed is saved as a plain line.
    const savedKeys = new Set(lines.filter((l) => (l.name || l.description).trim()).map((l) => l.key));
    const payload = {
      invoice_type: form.invoice_type,
      issue_date: form.issue_date, due_date: form.due_date,
      customer_org_id: form.customer_org_id || undefined,
      customer_contact_id: form.customer_contact_id || undefined,
      customer_email: form.billing_email.trim() || undefined,
      customer_tel: form.billing_whatsapp.trim() || undefined,
      notes: form.notes || undefined,
      payment_terms_text: `Net ${parseInt(form.payment_terms) || 30}`,
      currency: form.currency,
      exchange_rate: isForeign ? (form.exchange_rate ?? undefined) : undefined,
      items: lines.filter((l) => (l.name || l.description).trim()).map((l) => ({
        name: l.name || l.description,
        description: l.name ? l.description : "",
        product_id: l.product_id || undefined,
        sku: l.sku || undefined,
        quantity: l.qty, unit_price: l.unit_price,
        discount_pct: l.discount_pct || 0,
        // A tax row, or a typed rate (no row) for a custom tax.
        tax_id: l.tax_custom ? undefined : (l.tax_id || undefined),
        tax_pct: l.tax_custom ? (Number(l.tax_pct) || 0) : undefined,
        key: l.key, parent_key: l.parent_key && savedKeys.has(l.parent_key) ? l.parent_key : undefined,
        term_months: l.term_months ?? undefined,
        price_rule: l.price_rule || undefined,
      })),
    };
    try {
      if (isEdit && invoiceId) {
        await financeApi.updateInvoice(invoiceId, payload);
        router.push(`/finance/invoices/${invoiceId}`);
      } else {
        const res = await financeApi.createInvoice(payload);
        router.push(`/finance/invoices/${res.data.id}`);
      }
    } catch (err) {
      setError(extractApiError(err, isEdit ? "Failed to update invoice." : "Failed to create invoice. Please check required fields."));
    } finally { setSaving(false); }
  }


  const filled = lines.filter((l) => l.name.trim() || l.product_id);
  const taxWarning = taxConflict(form.notes, filled.filter((l) => Number(l.tax_pct) > 0).length, filled.length, taxCfg);

  const totals = (
    <dl className="space-y-1.5 text-sm">
      <div className="flex justify-between text-fg-muted"><dt>Subtotal</dt><dd className="tabular text-fg">{formatCurrency(grossTotal, form.currency)}</dd></div>
      {discountTotal > 0 && <div className="flex justify-between text-fg-muted"><dt>Discount</dt><dd className="tabular text-fg">-{formatCurrency(discountTotal, form.currency)}</dd></div>}
      {taxAmount > 0 && <div className="flex justify-between text-fg-muted"><dt>Tax</dt><dd className="tabular text-fg">{formatCurrency(taxAmount, form.currency)}</dd></div>}
      <div className="flex justify-between border-t border-line pt-2">
        <dt className="font-semibold text-fg">Total</dt>
        <dd className="text-right">
          <div className="tabular text-base font-semibold text-fg">{formatCurrency(total, form.currency)}</div>
          {isForeign && form.exchange_rate && <div className="mt-0.5 text-xs tabular text-fg-muted">approx. {formatCurrency(total * rate, functionalCurrency)}</div>}
        </dd>
      </div>
    </dl>
  );

  return (
    <div className="mx-auto max-w-5xl">
      <PageHeader title={isEdit ? "Edit invoice" : "New invoice"} back={{ href: backHref, label: isEdit ? "Invoice" : "Invoices" }}
        actions={<>
          <Button asChild><Link href={backHref}>Cancel</Link></Button>
          <Button variant="primary" loading={saving} onClick={handleSave}>
            {!saving && <Receipt aria-hidden />}
            {saving ? (isEdit ? "Saving..." : "Creating invoice...") : (isEdit ? "Save changes" : "Create invoice")}
          </Button>
        </>} />

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <div className="space-y-4 lg:col-span-2">
          <Card>
            <CardHeader title="Invoice details" />
            <CardBody>
              <FieldGrid cols={2}>
                <Field label="Type">
                  <div className="flex gap-2" role="group" aria-label="Invoice type">
                    {[{ value: "commercial", label: "Commercial" }, { value: "proforma", label: "Proforma" }].map((t) => (
                      <Button key={t.value} className="flex-1" aria-pressed={form.invoice_type === t.value}
                        variant={form.invoice_type === t.value ? "primary" : "secondary"}
                        disabled={isEdit && form.invoice_type !== t.value}
                        title={isEdit ? "The type can't change after the invoice is created" : undefined}
                        onClick={() => setForm({ ...form, invoice_type: t.value })}>
                        {t.label}
                      </Button>
                    ))}
                  </div>
                </Field>
                <Field label="Payment terms">
                  <Select value={form.payment_terms} onChange={(e) => setForm({ ...form, payment_terms: e.target.value })}
                    options={[
                      { value: "0", label: "Due immediately" }, { value: "7", label: "Net 7" }, { value: "14", label: "Net 14" },
                      { value: "30", label: "Net 30" }, { value: "45", label: "Net 45" }, { value: "60", label: "Net 60" },
                    ]} />
                </Field>
                <Field label="Issue date">
                  <Input type="date" value={form.issue_date} onChange={(e) => setForm({ ...form, issue_date: e.target.value })} />
                </Field>
                <Field label="Due date" required>
                  <Input type="date" value={form.due_date} onChange={(e) => setForm({ ...form, due_date: e.target.value })} />
                </Field>
                <Field label="Currency">
                  <Select value={form.currency} disabled={isEdit}
                    title={isEdit ? "The currency can't change after the invoice is created" : undefined}
                    onChange={(e) => setForm(f => ({ ...f, currency: e.target.value, exchange_rate: null }))}>
                    {activeCurrencies.map(c => (
                      <option key={c} value={c}>{c}: {CURRENCY_META[c as keyof typeof CURRENCY_META]?.name ?? c}</option>
                    ))}
                  </Select>
                </Field>
                {isForeign && (
                  <Field label="Rate" hint={`1 ${form.currency} = ? ${functionalCurrency}`}>
                    <div className="flex items-center gap-2">
                      <div className="flex-1">
                        <Input type="number" numeric step="0.000001" min="0" value={form.exchange_rate ?? ""}
                          onChange={(e) => setForm(f => ({ ...f, exchange_rate: e.target.value ? Number(e.target.value) : null }))}
                          placeholder="e.g. 15900" />
                      </div>
                      <IconButton icon={RefreshCw} label="Fetch the spot rate" disabled={fetchingRate}
                        className={fetchingRate ? "[&_svg]:animate-spin" : undefined}
                        onClick={() => fetchRate(form.currency, form.issue_date, functionalCurrency)} />
                    </div>
                  </Field>
                )}
              </FieldGrid>
            </CardBody>
          </Card>

          <Card>
            <CardHeader title="Line items"
              actions={<span className="flex items-center gap-3">
                <Switch checked={pricesExcludeTax} onCheckedChange={setPricesExcludeTax} label="Prices exclude tax" />
                {taxConfig && <span className="text-xs text-fg-muted">Tax config: <strong className="font-medium text-fg">{taxConfig.name}</strong></span>}
              </span>} />
            <CardBody className="space-y-3">
              {taxWarning && (
                <p role="alert" className="rounded-md border border-tone-orange/30 bg-tone-orange-soft px-3 py-2 text-xs font-medium text-fg">{taxWarning}</p>
              )}
              {lines.map((line, i) => (
                <div key={line.key ?? i} className={cn("rounded-lg border border-line bg-surface-2/60 p-3",
                  line.parent_key && lines.some((l) => l.key === line.parent_key) && "ml-6 border-l-2 border-l-line-strong")}>
                  <Field label="Catalog product (optional)" className="mb-3"
                    hint={(line.sku || line.term_months) ? (
                      <>
                        {line.sku && <span className="font-mono">{line.sku}</span>}
                        {line.sku && line.term_months ? ", " : ""}
                        {line.term_months ? `Term ${termLabel(line.term_months)}` : ""}
                      </>
                    ) : undefined}>
                    <Combobox value={line.product_id} source={catalogSource}
                      placeholder="Custom line, or pick a product" emptyLabel="No products found"
                      onChange={(id) => pickProduct(i, id)} />
                    {line.price_note && (
                      <p className={cn("text-xs", line.price_warning ? "text-tone-orange" : "text-fg-muted")}>{line.price_note}</p>
                    )}
                  </Field>
                  <div className="mb-3 flex items-end gap-3">
                    <div className="flex flex-1 flex-col gap-2">
                      <Field label="Item name" required>
                        <Input value={line.name} onChange={(e) => updateLine(i, { name: e.target.value })} placeholder="Item or service name" />
                      </Field>
                      <Input value={line.description} onChange={(e) => updateLine(i, { description: e.target.value })} placeholder="Description (optional)"
                        aria-label="Description" />
                    </div>
                    <IconButton icon={Trash2} variant="danger" label="Remove line" disabled={lines.length === 1}
                      onClick={() => setLines((prev) => prev.filter((_, idx) => idx !== i))} />
                  </div>
                  <div className="grid grid-cols-2 gap-3 md:grid-cols-5">
                    <Field label="Qty">
                      <Input type="number" numeric value={line.qty} min={0} step="any" onChange={(e) => updateLine(i, { qty: parseFloat(e.target.value) || 0 })} />
                    </Field>
                    <Field label={`Unit price (${currencySymbol})`} className="md:col-span-2">
                      <Input type="number" numeric value={line.unit_price} min={0} step="any" onChange={(e) => updateLine(i, { unit_price: parseFloat(e.target.value) || 0 })} />
                    </Field>
                    <Field label="Disc %">
                      <Input type="number" numeric value={line.discount_pct} min={0} max={100} step="any" onChange={(e) => updateLine(i, { discount_pct: parseFloat(e.target.value) || 0 })} />
                    </Field>
                    <div className="flex flex-col gap-1">
                    <Field label="Tax">
                      <Select value={line.tax_custom ? "__custom" : line.tax_id}
                        onChange={(e) => {
                          if (e.target.value === "__custom") { updateLine(i, { tax_custom: true, tax_id: "" }); return; }
                          const rule = taxConfig?.rules.find((r) => r.id === e.target.value);
                          updateLine(i, { tax_custom: false, tax_id: rule?.id ?? "", tax_pct: rule ? Number(rule.rate) : 0 });
                        }}>
                        <option value="">No tax</option>
                        <option value="__custom">Custom %</option>
                        {/* A line saved with a tax that is now inactive keeps showing it. */}
                        {line.tax_id && !taxConfig?.rules.some((r) => r.id === line.tax_id) && (
                          <option value={line.tax_id}>{line.tax_pct}% (inactive)</option>
                        )}
                        {taxConfig?.rules.map((r) => <option key={r.id} value={r.id}>{r.name} ({r.rate}%)</option>)}
                      </Select>
                    </Field>
                    {line.tax_custom && (
                      <Input type="number" numeric min={0} max={100} step="any" value={line.tax_pct} aria-label="Custom tax %"
                        onChange={(e) => updateLine(i, { tax_pct: parseFloat(e.target.value) || 0 })} />
                    )}
                    </div>
                  </div>
                  <div className="mt-3 flex justify-end">
                    <span className="text-sm font-semibold tabular text-fg">{formatCurrency(amounts[i].net + amounts[i].tax, form.currency)}</span>
                  </div>
                </div>
              ))}
              <Button className="w-full border-dashed" onClick={() => setLines((prev) => [...prev, emptyInvoiceLine()])}>
                <Plus aria-hidden /> Add line item
              </Button>
            </CardBody>
            <CardFooter className="justify-end bg-surface-2/60">
              <div className="w-full md:w-64">{totals}</div>
            </CardFooter>
          </Card>

          <Card>
            <CardHeader title="Notes"
              actions={<NoteTemplatePicker docType="invoice" currentValue={form.notes}
                onInsert={(html) => setForm({ ...form, notes: form.notes ? form.notes + html : html })} />} />
            <CardBody>
              <RichTextEditor value={form.notes} onChange={(html) => setForm({ ...form, notes: html })}
                placeholder="Payment instructions, bank details, or additional notes..." />
            </CardBody>
          </Card>

          {error && (
            <div role="alert" className="flex items-center gap-3 rounded-lg border border-tone-red/30 bg-tone-red-soft px-4 py-3">
              <AlertCircle className="size-4 shrink-0 text-tone-red" aria-hidden /><p className="text-sm text-tone-red">{error}</p>
            </div>
          )}

          {/* Phones: the total stays in view while the lines are edited. */}
          <div className="sticky bottom-[calc(var(--shell-bottom)-6rem)] md:bottom-[-1.5rem] z-10 -mx-4 flex items-center justify-between gap-3 border-t border-line bg-surface px-4 py-2.5 md:hidden"
            aria-hidden>
            <span className="text-xs text-fg-muted">{lines.filter((l) => (l.name || l.description).trim()).length} items</span>
            <span className="text-sm font-semibold text-fg tabular">Total {formatCurrency(total, form.currency)}</span>
          </div>

          <div className="flex justify-end gap-2 pb-8">
            <Button asChild><Link href={backHref}>Cancel</Link></Button>
            <Button variant="primary" loading={saving} onClick={handleSave}>
              {!saving && <Receipt aria-hidden />}
              {saving ? (isEdit ? "Saving..." : "Creating invoice...") : (isEdit ? "Save changes" : "Create invoice")}
            </Button>
          </div>
        </div>

        <div className="space-y-4 max-lg:order-first">
          <Card>
            <CardHeader title={<span className="inline-flex items-center gap-2"><Building2 className="size-4 text-fg-muted" aria-hidden /> Client</span>} />
            <CardBody className="space-y-3">
              <OrgContactPicker
                organizationId={form.customer_org_id}
                contactId={form.customer_contact_id}
                onChange={({ organization_id, contact_id }) =>
                  setForm((f) => ({ ...f, customer_org_id: organization_id, customer_contact_id: contact_id }))}
                labelClass="mb-1.5 block text-[13px] font-medium text-fg"
                contactLabel="Contact person"
              />
              <Field label={<>Billing email <span className="font-normal text-fg-muted">(optional)</span></>} id="billing-email">
                <Input type="email" value={form.billing_email}
                  onChange={(e) => setForm((f) => ({ ...f, billing_email: e.target.value }))}
                  placeholder="finance@customer.com" />
              </Field>
              <Field label={<>Billing WhatsApp <span className="font-normal text-fg-muted">(optional)</span></>} id="billing-whatsapp"
                hint="Used first when sending this invoice; leave empty to use the contact's details.">
                <Input type="tel" value={form.billing_whatsapp}
                  onChange={(e) => setForm((f) => ({ ...f, billing_whatsapp: e.target.value }))}
                  placeholder="+1 555 010 0199" />
              </Field>
            </CardBody>
          </Card>

          <Card className="max-lg:hidden">
            <CardHeader title="Summary" />
            <CardBody className="space-y-1.5">
              <div className="flex justify-between text-sm"><span className="text-fg-muted">Items</span><span className="tabular text-fg">{lines.filter((l) => (l.name || l.description).trim()).length}</span></div>
              {totals}
            </CardBody>
          </Card>

          {!taxConfig && (
            <div className="rounded-lg border border-tone-orange/30 bg-tone-orange-soft p-4">
              <div className="flex items-start gap-2">
                <AlertCircle className="mt-0.5 size-4 shrink-0 text-tone-orange" aria-hidden />
                <div>
                  <p className="text-xs font-semibold text-tone-orange">No tax rates set up</p>
                  <p className="mt-0.5 text-xs text-fg">Lines can&apos;t carry tax until a sales tax is set up.{" "}
                    <Link href="/finance/taxes" className="font-medium text-brand-ink underline">Configure in Finance &gt; Taxes</Link>
                  </p>
                </div>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
