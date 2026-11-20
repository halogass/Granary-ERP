"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState, useCallback, useMemo, useRef } from "react";
import { crmApi, salesApi, systemApi, financeApi } from "@/lib/api";
import { useRouter } from "next/navigation";
import { CornerDownRight, Info, Loader2, Plus, RefreshCw, RotateCcw, Trash2, X } from "lucide-react";
import Link from "next/link";
import { formatCurrency, getCurrencySymbol, CURRENCY_META } from "@/lib/currency";
import { extractApiError } from "@/lib/apiError";
import RichTextEditor from "@/components/ui/RichTextEditor";
import NoteTemplatePicker from "@/components/notes/NoteTemplatePicker";
import QuotationNotesHelper, { type NotesContext } from "@/components/sales/QuotationNotesHelper";
import SearchSelect from "@/components/ui/SearchSelect";
import OrgContactPicker from "@/components/crm/OrgContactPicker";
import {
  Button, Card, CardBody, CardHeader, Checkbox, CountBadge, Dialog, Field, FieldGrid, IconButton, Input, PageHeader, Pill,
  Select, Switch, type Tone,
} from "@/components/ui";
import { DEFAULT_TAX_NOTE, hasTaxNote, taxConflict, withTaxNote, withoutTaxNote, type TaxNotes } from "@/lib/taxNotes";
import { catalogSource } from "@/lib/entitySources";
import { usePermissions } from "@/hooks/usePermissions";
import type { CatalogItem, DefaultTax, LineIn, PricedLine, PricingPreview } from "@/lib/types/catalog";
import {
  addonChoices, insertIndexForChild, newLineKey, orderLineTree, priceRuleBadge, recurringHint,
  subtreeKeys, termLabel, termOptions,
} from "@/lib/catalogPricing";
import { stack } from "@/lib/stackTable";
import { cn } from "@/lib/utils";

/** One line as the form holds it. Prices are never computed here: they come from the preview. */
export interface LineItem {
  /** Stable client id; the preview and the save resolve add-ons through it. */
  key: string;
  product_id: string;
  parent_key: string | null;
  name: string; description: string;
  quantity: number;
  /** Typed price, sent only for a manual line (custom, manual/quote-only item, or an override). */
  unit_price: number | null;
  manual: boolean;
  /** Typed discount %; null = none. Volume tiers are never applied by themselves (a private lever). */
  discount_pct: number | null;
  discount_type: string;
  is_optional: boolean;
  term_months: number | null;
  /** none = no tax; tax = a Finance → Taxes row; custom = a typed %. */
  tax_mode: "none" | "tax" | "custom";
  tax_id: string;
  tax_pct: number | null;
  /** Follows the customer's default tax until someone picks a tax for the line. */
  tax_auto: boolean;
  /** An add-on priced per parent unit keeps the parent's quantity until edited. */
  follow_parent_qty: boolean;
}

export interface QuotationHeader {
  customer_org_id: string; customer_contact_id: string; deal_id: string; title: string; notes: string;
  issue_date: string; valid_until: string;
  discount_type: "none" | "percentage" | "fixed"; discount_value: number;
  /** 1 Nov 2026: why the discount is given (under the PDF's Discount line, or in the notes with notes_only). */
  discount_note?: string;
  /** pdf: a Discount line; notes_only: nothing is taken off, the PDF notes state the discount. */
  discount_display?: "pdf" | "notes_only";
  payment_terms: string; currency: string; exchange_rate: number | null;
  procurement_type: string;
}

interface SavedItem {
  id: string; parent_item_id?: string | null; product_id?: string | null;
  name?: string | null; description?: string | null; quantity?: number | null; qty?: number | null;
  unit_price?: number | null; discount_percentage?: number | null; discount_type?: string | null;
  is_optional?: boolean | null; term_months?: number | null; price_rule?: string | null;
  tax_id?: string | null; tax_code?: string | null; tax_pct?: number | null;
}

/** A saved quotation item as an editable line. Lines at list are re-priced by the server. */
export function savedItemToLine(i: SavedItem): LineItem {
  const manual = i.price_rule === "manual" || !i.product_id;
  // A "volume" discount is one someone granted within the tier: it stays like any other discount.
  const autoDiscount = !i.discount_type || i.discount_type === "none";
  const pct = i.tax_pct == null ? null : Number(i.tax_pct);
  return {
    key: i.id,
    product_id: i.product_id ?? "",
    parent_key: i.parent_item_id ?? null,
    name: i.name ?? "",
    description: i.description ?? "",
    quantity: i.quantity != null ? Number(i.quantity) : i.qty != null ? Number(i.qty) : 1,
    unit_price: manual ? Number(i.unit_price) || 0 : null,
    manual,
    discount_pct: autoDiscount ? null : Number(i.discount_percentage) || 0,
    discount_type: autoDiscount ? "none" : (i.discount_type ?? "none"),
    is_optional: !!i.is_optional,
    term_months: i.term_months ?? null,
    tax_mode: i.tax_id ? "tax" : pct ? "custom" : "none",
    tax_id: i.tax_id ?? "",
    tax_pct: i.tax_id ? null : pct,
    tax_auto: false,
    follow_parent_qty: false,
  };
}

const emptyLine = (): LineItem => ({
  key: newLineKey(), product_id: "", parent_key: null, name: "", description: "", quantity: 1,
  unit_price: null, manual: false, discount_pct: null, discount_type: "none", is_optional: false,
  term_months: null, tax_mode: "none", tax_id: "", tax_pct: null, tax_auto: true, follow_parent_qty: false,
});

/** The line's tax set from the customer's default tax (from the preview). */
function withDefaultTax(l: LineItem, t: DefaultTax | null): LineItem {
  if (!t) return { ...l, tax_mode: "none", tax_id: "", tax_pct: null };
  if (t.id) return { ...l, tax_mode: "tax", tax_id: t.id, tax_pct: null };
  return { ...l, tax_mode: "custom", tax_id: "", tax_pct: Number(t.pct) };
}

const sameTax = (a: LineItem, b: LineItem) => a.tax_mode === b.tax_mode && a.tax_id === b.tax_id && a.tax_pct === b.tax_pct;

// A typed discount names its type. A volume discount up to the SKU's tier counts
// as list and is auto-approved; anything else goes to approval.
const DISCOUNT_TYPES = [
  { value: "volume", label: "Volume tier" },
  { value: "bundle", label: "Bundle" },
  { value: "loyalty", label: "Loyalty" },
  { value: "multi_year_saas", label: "Multi-year SaaS" },
  { value: "owner_exception", label: "Owner exception" },
];

// Non-standard procurement always goes to the Owner.
const PROCUREMENT_TYPES = [
  { value: "standard", label: "Standard" },
  { value: "government", label: "Government" },
  { value: "white_label", label: "White label" },
  { value: "oem", label: "OEM" },
];

const BADGE_TONE: Record<string, Tone> = { neutral: "neutral", good: "blue", warn: "orange", info: "violet" };

const needsTypedPrice = (item?: CatalogItem) => item?.pricing_mode === "manual" || item?.pricing_mode === "quote_only";

interface Tax { id: string; name: string; code: string; rate: number }

interface Props {
  mode: "create" | "edit";
  quotationId?: string;
  initialHeader?: Partial<QuotationHeader>;
  initialLines?: LineItem[];
  /** "New quotation" from a deal: link it and pre-fill the customer and currency from the deal. */
  initialDealId?: string;
  /** 2 Nov 2026: why it can't be saved now (waiting for approval); shown by the save button, which is off. */
  lockedReason?: string;
}

interface DealOption { id: string; title: string; organization_id?: string | null }

export default function QuotationForm({ mode, quotationId, initialHeader, initialLines, initialDealId, lockedReason }: Props) {
  const router = useRouter();
  const isEdit = mode === "edit";
  const { can } = usePermissions();
  const canManual = can("sales:price_manual");
  const canDiscount = can("sales:apply_discount");
  const [catalog, setCatalog] = useState<CatalogItem[]>([]);
  const [taxes, setTaxes] = useState<Tax[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [activeCurrencies, setActiveCurrencies] = useState<string[]>([]);
  const [functionalCurrency, setFunctionalCurrency] = useState("");
  const [fetchingRate, setFetchingRate] = useState(false);
  const [addonFor, setAddonFor] = useState<string | null>(null);

  const today = new Date().toISOString().slice(0, 10);

  // Validity and payment terms start from Settings (GET /sales/quotation-defaults) on a new quotation.
  const [header, setHeader] = useState<QuotationHeader>({
    customer_org_id: "", customer_contact_id: "", deal_id: initialDealId ?? "", title: "", notes: "",
    issue_date: today, valid_until: "",
    discount_type: "none", discount_value: 0, payment_terms: "",
    currency: "", exchange_rate: null, procurement_type: "standard",
    discount_note: "", discount_display: "pdf",
    ...(initialHeader || {}),
  });

  const [lines, setLines] = useState<LineItem[]>(
    initialLines && initialLines.length ? initialLines : [emptyLine()],
  );

  // ── Deal ────────────────────────────────────────────────────────────────────
  const [deals, setDeals] = useState<DealOption[]>([]);

  useEffect(() => {
    if (!initialDealId || isEdit) return;
    crmApi.getDeal(initialDealId).then((r) => {
      const d = r.data;
      setHeader((h) => ({
        ...h, deal_id: d.id,
        customer_org_id: h.customer_org_id || d.organization_id || "",
        customer_contact_id: h.customer_contact_id || d.contact_id || "",
        currency: d.currency || h.currency,
        title: h.title || d.title || "",
      }));
    }).catch(() => setHeader((h) => ({ ...h, deal_id: "" })));
  }, [initialDealId, isEdit]);

  // The customer's deals (or the most recent ones when no customer is chosen yet).
  useEffect(() => {
    (header.customer_org_id
      ? fetchAllPages<DealOption>(crmApi.listDeals, { organization_id: header.customer_org_id }).then(({ rows }) => rows)
      : crmApi.listDeals({ limit: 50 }).then((r) => (r.data.items ?? r.data) as DealOption[]))
      .then((rows) => setDeals(rows))
      .catch(() => setDeals([]));
  }, [header.customer_org_id]);

  // ── Reference data ──────────────────────────────────────────────────────────

  const loadCurrencySettings = useCallback(async () => {
    try {
      const res = await systemApi.getCurrencySettings();
      const active: string[] = res.data.active ?? [];
      const functional: string = res.data.functional ?? "";
      setActiveCurrencies(active);
      setFunctionalCurrency(functional);
      // Only seed the currency from the functional default when creating; never clobber an
      // existing quotation's currency in edit mode.
      if (!isEdit) setHeader(h => ({ ...h, currency: h.currency || functional }));   // a deal may have set it
    } catch { /* keep defaults */ }
  }, [isEdit]);

  useEffect(() => {
    if (isEdit) return;
    salesApi.quotationDefaults()
      .then((r) => {
        const { validity_days, payment_terms_days } = r.data as { validity_days: number; payment_terms_days: number };
        setHeader((h) => ({
          ...h,
          valid_until: h.valid_until || new Date(Date.now() + validity_days * 86400000).toISOString().slice(0, 10),
          payment_terms: h.payment_terms || `NET${payment_terms_days}`,
        }));
      })
      .catch((e) => setError(extractApiError(e, "Could not load the quotation defaults.")));
  }, [isEdit]);

  // 2 Nov 2026: the "Prices exclude tax" sentence and the phrases the approval's notes/tax check reads
  const [taxCfg, setTaxCfg] = useState<TaxNotes | null>(null);
  useEffect(() => {
    salesApi.taxNotes()
      .then((r) => setTaxCfg(r.data))
      .catch(() => setTaxCfg({ note: DEFAULT_TAX_NOTE, exclusive_phrases: [], inclusive_phrases: [] }));
  }, []);

  useEffect(() => {
    // Orderable items, for the add-on choices and the term/metric hints of each line.
    salesApi.listCatalog({ limit: 200 })
      .then((r) => setCatalog(((r.data.items ?? r.data) as CatalogItem[]).filter((c) => c.is_active && c.is_orderable)))
      .catch(() => {});
    financeApi.listTaxes({ active_only: true })
      .then((r) => setTaxes((r.data.items ?? r.data ?? []).filter((t: { applies_to_sales?: boolean }) => t.applies_to_sales !== false)))
      .catch(() => setTaxes([]));
    loadCurrencySettings();
  }, [loadCurrencySettings]);

  const catalogById = useMemo(() => new Map(catalog.map((c) => [c.id, c])), [catalog]);

  /** Load items a line points at that the orderable list does not hold (retired, or past 200). */
  const requested = useRef(new Set<string>());
  const ensureItems = useCallback(async (ids: string[]) => {
    // Each id is fetched once per form, however often the lines change.
    const missing = Array.from(new Set(ids.filter((id) => id && !catalogById.has(id) && !requested.current.has(id))));
    if (!missing.length) return;
    missing.forEach((id) => requested.current.add(id));
    const got = await Promise.all(missing.map((id) =>
      salesApi.getProduct(id).then((r) => r.data as CatalogItem).catch(() => null)));
    const add = got.filter((g): g is CatalogItem => !!g);
    if (add.length) setCatalog((prev) => [...prev, ...add.filter((a) => !prev.some((p) => p.id === a.id))]);
  }, [catalogById]);

  useEffect(() => { ensureItems(lines.map((l) => l.product_id)); }, [lines, ensureItems]);

  // ── Exchange rate (quotation → functional currency, for reporting) ──────────

  const fetchRate = useCallback(async (currency: string, issueDate: string) => {
    if (currency === functionalCurrency) { setHeader(h => ({ ...h, exchange_rate: null })); return; }
    setFetchingRate(true);
    try {
      const res = await financeApi.getSpotRate(currency, functionalCurrency, issueDate);
      setHeader(h => ({ ...h, exchange_rate: res.data?.rate ?? null }));
    } catch {
      setHeader(h => ({ ...h, exchange_rate: null }));
    } finally { setFetchingRate(false); }
  }, [functionalCurrency]);

  // In edit mode keep the quotation's stored rate until the currency or issue date changes;
  // only then fetch the recorded rate for the new combination.
  const [storedRate] = useState(() => ({
    keep: isEdit && initialHeader?.exchange_rate != null,
    currency: initialHeader?.currency, issueDate: initialHeader?.issue_date,
  }));

  useEffect(() => {
    if (storedRate.keep) {
      if (header.currency === storedRate.currency && header.issue_date === storedRate.issueDate) return;
      storedRate.keep = false;
    }
    if (header.currency !== functionalCurrency) fetchRate(header.currency, header.issue_date);
  }, [header.currency, header.issue_date, functionalCurrency, fetchRate, storedRate]);

  // ── Server pricing ──────────────────────────────────────────────────────────

  // A line without a product is a custom (manual) line, and a manual/quote-only item needs a typed
  // price: only staff with sales:price_manual price either, so other users' previews leave them out.
  const isFilled = useCallback((l: LineItem) => {
    if (!l.product_id) return canManual && !!l.name.trim();
    return canManual || !needsTypedPrice(catalogById.get(l.product_id));
  }, [canManual, catalogById]);

  const toLineIn = useCallback((l: LineItem, filledKeys: Set<string>): LineIn => {
    const manual = l.manual || !l.product_id;
    return {
      key: l.key,
      product_id: l.product_id || null,
      name: l.name.trim() || null,
      description: l.description.trim() || null,
      quantity: l.quantity,
      unit_price: manual ? (l.unit_price ?? 0) : null,
      discount_pct: l.discount_pct,
      discount_type: l.discount_pct ? l.discount_type : null,
      is_optional: l.is_optional,
      parent_key: l.parent_key && filledKeys.has(l.parent_key) ? l.parent_key : null,
      term_months: l.term_months,
      // Both null = the customer's default tax (the server decides). "No tax" is an explicit 0%.
      tax_id: l.tax_auto ? null : l.tax_mode === "tax" ? (l.tax_id || null) : null,
      tax_pct: l.tax_auto ? null : l.tax_mode === "custom" ? (l.tax_pct ?? 0) : l.tax_mode === "none" ? 0 : null,
    };
  }, []);

  const filledLines = useMemo(() => lines.filter(isFilled), [lines, isFilled]);
  const previewBody = useMemo(() => {
    const keys = new Set(filledLines.map((l) => l.key));
    return {
      currency: header.currency,
      issue_date: header.issue_date || undefined,
      customer_org_id: header.customer_org_id || undefined,
      discount_type: header.discount_type !== "none" && header.discount_display !== "notes_only" ? header.discount_type : undefined,
      discount_value: header.discount_type !== "none" && header.discount_display !== "notes_only" ? header.discount_value : undefined,
      lines: filledLines.map((l) => toLineIn(l, keys)),
    };
  }, [filledLines, header.currency, header.issue_date, header.customer_org_id, header.discount_type, header.discount_value, header.discount_display, toLineIn]);
  const previewKey = JSON.stringify(previewBody);

  const [preview, setPreview] = useState<PricingPreview | null>(null);
  const [previewError, setPreviewError] = useState("");
  const [pricing, setPricing] = useState(false);
  const [defaultTax, setDefaultTax] = useState<DefaultTax | null | undefined>(undefined);
  const seq = useRef(0);

  useEffect(() => {
    const body = JSON.parse(previewKey);
    if (!body.currency) return;          // the currency default is still loading: nothing to price yet
    const mine = ++seq.current;
    setPricing(true);
    const t = setTimeout(async () => {
      try {
        const r = await salesApi.pricingPreview(body);
        if (mine !== seq.current) return;
        setPreview(r.data);
        setPreviewError("");
        setDefaultTax(r.data.default_tax ?? null);
      } catch (e) {
        if (mine !== seq.current) return;
        setPreviewError(extractApiError(e, "Could not price the lines."));
      } finally {
        if (mine === seq.current) setPricing(false);
      }
    }, 350);
    return () => clearTimeout(t);
  }, [previewKey]);

  // Lines still on the automatic tax follow the customer's default (it changes with the org).
  useEffect(() => {
    if (defaultTax === undefined) return;
    setLines((prev) => {
      let changed = false;
      const next = prev.map((l) => {
        if (!l.tax_auto) return l;
        const d = withDefaultTax(l, defaultTax);
        if (sameTax(d, l)) return l;
        changed = true;
        return d;
      });
      return changed ? next : prev;
    });
  }, [defaultTax]);

  const pricedByKey = useMemo(() => new Map((preview?.lines ?? []).map((p) => [p.key, p] as const)), [preview]);

  // ── Prices exclude tax (2 Nov 2026) ────────────────────────────────────────
  // On: every line "No tax" and the notes carry the sentence; off: the sentence goes and the lines follow the
  // customer's default tax again. Each line can still be set by hand.
  const taxNote = taxCfg?.note ?? DEFAULT_TAX_NOTE;
  const pricesExcludeTax = hasTaxNote(header.notes, taxNote);
  const noTax = (l: LineItem): LineItem => ({ ...l, tax_mode: "none", tax_id: "", tax_pct: null, tax_auto: false });
  function setPricesExcludeTax(on: boolean) {
    if (on) {
      setLines((prev) => prev.map(noTax));
      setHeader((h) => ({ ...h, notes: withTaxNote(h.notes, taxNote) }));
    } else {
      setLines((prev) => prev.map((l) => {
        const auto = { ...l, tax_auto: true };
        return defaultTax !== undefined ? withDefaultTax(auto, defaultTax) : auto;
      }));
      setHeader((h) => ({ ...h, notes: withoutTaxNote(h.notes, taxNote) }));
    }
  }
  /** A new line's tax: none while prices exclude tax, else the customer's default. */
  const startTax = (l: LineItem): LineItem =>
    pricesExcludeTax ? noTax(l) : l.tax_auto && defaultTax !== undefined ? withDefaultTax(l, defaultTax) : l;
  const taxedLines = filledLines.filter((l) => l.tax_mode === "tax" ? !!l.tax_id : l.tax_mode === "custom" && (l.tax_pct ?? 0) > 0).length;
  const taxWarning = taxConflict(header.notes, taxedLines, filledLines.length, taxCfg);

  // ── Line edits ──────────────────────────────────────────────────────────────

  function updateLine(key: string, patch: Partial<LineItem>) {
    setLines((prev) => {
      const next = prev.map((l) => (l.key === key ? { ...l, ...patch } : l));
      // Add-ons priced per parent unit keep the parent's quantity.
      if (patch.quantity !== undefined) {
        return next.map((l) => (l.parent_key === key && l.follow_parent_qty ? { ...l, quantity: patch.quantity! } : l));
      }
      return next;
    });
  }

  /** A new line for a catalog item, with its default term, minimum quantity and default tax. */
  function lineFor(item: CatalogItem, base: LineItem): LineItem {
    const term = item.sale_model === "recurring" ? termOptions(item)[0] : null;
    const minQ = item.min_quantity ? Number(item.min_quantity) : 0;
    const l: LineItem = {
      ...base,
      product_id: item.id,
      name: item.name,
      quantity: Math.max(base.quantity || 1, minQ),
      manual: needsTypedPrice(item),
      unit_price: needsTypedPrice(item) ? (base.unit_price ?? 0) : null,
      term_months: term,
    };
    return startTax(l);
  }

  async function pickProduct(key: string, productId: string) {
    if (!productId) {
      updateLine(key, { product_id: "", manual: true, unit_price: 0, term_months: null });
      return;
    }
    let item = catalogById.get(productId);
    if (!item) {
      try {
        item = (await salesApi.getProduct(productId)).data as CatalogItem;
        setCatalog((prev) => (prev.some((p) => p.id === item!.id) ? prev : [...prev, item!]));
      } catch (e) {
        setError(extractApiError(e, "Could not load that product."));
        return;
      }
    }
    setLines((prev) => prev.map((l) => (l.key === key ? lineFor(item!, { ...l, description: l.product_id ? "" : l.description }) : l)));
  }

  function addChild(parentKey: string, item: CatalogItem) {
    setLines((prev) => {
      const parent = prev.find((l) => l.key === parentKey);
      if (!parent) return prev;
      const follow = item.pricing_mode === "percent_of_parent" || !!item.is_whitelabel;
      const child = lineFor(item, {
        ...emptyLine(), parent_key: parentKey, follow_parent_qty: follow,
        quantity: follow ? parent.quantity : 1, is_optional: parent.is_optional,
      });
      const at = insertIndexForChild(prev, parentKey);
      return [...prev.slice(0, at), child, ...prev.slice(at)];
    });
    setAddonFor(null);
  }

  function removeLine(key: string) {
    setLines((prev) => {
      const drop = subtreeKeys(prev, key);
      const next = prev.filter((l) => !drop.has(l.key));
      return next.length ? next : [emptyLine()];
    });
  }

  // ── Totals (all from the server) ────────────────────────────────────────────

  const ordered = orderLineTree(lines);
  const subtotal = preview?.subtotal ?? 0;
  const serverDiscount = preview?.discount_amount;
  const total = preview?.total ?? 0;
  // Optional lines are offered, not sold: the server keeps them out of the total and sums them apart.
  const hasOptional = !!preview && preview.lines.some((p) => p.is_optional);
  const optionalSubtotal = preview?.optional_subtotal ?? 0;
  const optionalTax = preview?.optional_tax ?? 0;
  const totalWithOptional = preview?.total_with_optional ?? total;
  const isForeign = header.currency !== functionalCurrency;
  const rate = header.exchange_rate ?? 1;
  const backHref = isEdit && quotationId ? `/sales/quotations/${quotationId}` : "/sales/quotations";
  const suggestedTerms = preview?.payment_terms ?? null;

  // 4 Nov 2026: the house notes template and the notes check read the form as it stands
  const notesCtx = useMemo<NotesContext>(() => {
    const keys = new Set(filledLines.map((l) => l.key));
    return {
      quotation_id: isEdit && quotationId ? quotationId : undefined,
      notes: header.notes,
      currency: header.currency,
      subject: header.title || undefined,
      customer_org_id: header.customer_org_id || undefined,
      validity_days: header.issue_date && header.valid_until
        ? Math.round((Date.parse(header.valid_until) - Date.parse(header.issue_date)) / 86400000) || undefined
        : undefined,
      discount_type: header.discount_type,
      discount_value: header.discount_value,
      discount_display: header.discount_display,
      items: orderLineTree(filledLines).map(({ line }) => {
        const li = toLineIn(line, keys);
        return {
          product_id: li.product_id ?? null, name: li.name ?? "", description: li.description ?? "",
          quantity: li.quantity, unit_price: li.unit_price ?? null, is_optional: li.is_optional,
          tax_id: li.tax_id ?? null, tax_pct: li.tax_pct ?? null,
        };
      }),
    };
  }, [filledLines, header.notes, header.currency, header.title, header.customer_org_id, header.issue_date, header.valid_until,
    header.discount_type, header.discount_value, header.discount_display, toLineIn, isEdit, quotationId]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!filledLines.length) { setError("Add at least one line item."); return; }
    const untyped = filledLines.find((l) => (l.discount_pct ?? 0) > 0 && (!l.discount_type || l.discount_type === "none"));
    if (untyped) { setError(`Choose a discount type for "${untyped.name || "a line"}".`); return; }
    setSaving(true); setError("");
    const keys = new Set(filledLines.map((l) => l.key));
    const items = orderLineTree(filledLines).map(({ line }, i) => {
      const li = toLineIn(line, keys);
      return {
        key: li.key, product_id: li.product_id ?? undefined, sort_order: i + 1,
        name: li.name ?? "", description: li.description ?? "",
        quantity: li.quantity,
        ...(li.unit_price != null ? { unit_price: li.unit_price } : {}),
        ...(li.discount_pct != null ? { discount_percentage: li.discount_pct, discount_type: li.discount_type ?? "none" } : {}),
        is_optional: li.is_optional,
        parent_key: li.parent_key ?? undefined,
        term_months: li.term_months ?? undefined,
        tax_id: li.tax_id ?? undefined,
        tax_pct: li.tax_pct ?? undefined,
      };
    });
    const useSchedule = suggestedTerms && header.payment_terms === suggestedTerms.text;
    const payload = {
      ...header,
      title: undefined,
      subject: header.title || undefined,
      payment_terms: header.payment_terms
        ? { text: header.payment_terms, ...(useSchedule ? { schedule: suggestedTerms!.schedule } : {}) }
        : undefined,
      customer_org_id: header.customer_org_id || undefined,
      customer_contact_id: header.customer_contact_id || undefined,
      deal_id: header.deal_id || null,
      discount_value: header.discount_type !== "none" ? header.discount_value : undefined,
      discount_note: header.discount_note?.trim() || null,
      discount_display: header.discount_display || "pdf",
      currency: header.currency,
      exchange_rate: isForeign ? (header.exchange_rate ?? undefined) : undefined,
      items,
    };
    try {
      if (isEdit && quotationId) {
        await salesApi.updateQuotation(quotationId, payload);
        router.push(`/sales/quotations/${quotationId}`);
      } else {
        await salesApi.createQuotation(payload);
        router.push("/sales/quotations");
      }
    } catch (err: unknown) {
      setError(extractApiError(err, isEdit ? "Failed to update quotation." : "Failed to create quotation."));
    } finally { setSaving(false); }
  }

  const currencySymbol = getCurrencySymbol(header.currency);
  const addonParent = addonFor ? lines.find((l) => l.key === addonFor) : undefined;
  const addonParentItem = addonParent ? catalogById.get(addonParent.product_id) : undefined;
  const th = "px-3 py-2 text-xs font-medium text-fg-muted";

  return (
    <div className="mx-auto max-w-6xl">
      <PageHeader back={{ href: backHref, label: isEdit ? "Quotation" : "Quotations" }}
        title={isEdit ? "Edit quotation" : "New quotation"}
        summary={isEdit ? "Update this draft proposal" : "Create a price proposal for a client"} />

      <form onSubmit={handleSubmit} className="space-y-4">
        <Card>
          <CardHeader title="Client and details" />
          <CardBody>
            <FieldGrid cols={2}>
              <OrgContactPicker
                organizationId={header.customer_org_id}
                contactId={header.customer_contact_id}
                onChange={({ organization_id, contact_id }) =>
                  setHeader((h) => ({ ...h, customer_org_id: organization_id, customer_contact_id: contact_id }))}
                contactLabel="Contact person"
              />
              <Field label="Deal" className="md:col-span-2">
                <Select value={header.deal_id} onChange={(e) => setHeader({ ...header, deal_id: e.target.value })}>
                  <option value="">No deal</option>
                  {header.deal_id && !deals.some((d) => d.id === header.deal_id) && (
                    <option value={header.deal_id}>Linked deal</option>
                  )}
                  {deals.map((d) => <option key={d.id} value={d.id}>{d.title}</option>)}
                </Select>
              </Field>
              <Field label="Subject or title" className="md:col-span-2">
                <Input value={header.title} onChange={(e) => setHeader({ ...header, title: e.target.value })}
                  placeholder="e.g. Annual support, Enterprise package" />
              </Field>
              <Field label="Issue date" required>
                <Input type="date" required value={header.issue_date} onChange={(e) => setHeader({ ...header, issue_date: e.target.value })} />
              </Field>
              <Field label="Valid until">
                <Input type="date" value={header.valid_until} onChange={(e) => setHeader({ ...header, valid_until: e.target.value })} />
              </Field>
              <Field label="Currency">
                <Select value={header.currency} className="font-mono"
                  onChange={(e) => setHeader(h => ({ ...h, currency: e.target.value, exchange_rate: null }))}>
                  {activeCurrencies.map(c => (
                    <option key={c} value={c}>{c}, {CURRENCY_META[c as keyof typeof CURRENCY_META]?.name ?? c}</option>
                  ))}
                </Select>
              </Field>
              <Field label="Payment terms"
                hint={suggestedTerms?.text && suggestedTerms.text !== header.payment_terms ? (
                  <span className="flex items-start gap-2">
                    <span className="flex-1">Suggested: {suggestedTerms.text}</span>
                    <button type="button" onClick={() => setHeader((h) => ({ ...h, payment_terms: suggestedTerms.text }))}
                      className="shrink-0 font-semibold text-brand-ink hover:underline">Use</button>
                  </span>
                ) : undefined}>
                <Input value={header.payment_terms} onChange={(e) => setHeader({ ...header, payment_terms: e.target.value })} placeholder="NET30" />
              </Field>
              <Field label="Procurement type">
                <Select value={header.procurement_type} onChange={(e) => setHeader({ ...header, procurement_type: e.target.value })}
                  options={PROCUREMENT_TYPES} />
              </Field>
              {isForeign && (
                <Field label={<>Exchange rate <span className="font-normal text-fg-muted">(1 {header.currency} = ? {functionalCurrency})</span></>}
                  hint={header.exchange_rate ? `${formatCurrency(total, header.currency)} ≈ ${formatCurrency(total * rate, functionalCurrency)}` : undefined}>
                  <div className="flex items-center gap-2">
                    <Input type="number" step="0.000001" min="0" numeric
                      value={header.exchange_rate ?? ""}
                      onChange={(e) => setHeader(h => ({ ...h, exchange_rate: e.target.value ? Number(e.target.value) : null }))}
                      placeholder="e.g. 15900" className="flex-1" />
                    <IconButton icon={RefreshCw} label="Refresh rate" variant="secondary" disabled={fetchingRate}
                      className={fetchingRate ? "[&_svg]:animate-spin" : undefined}
                      onClick={() => fetchRate(header.currency, header.issue_date)} />
                  </div>
                </Field>
              )}
            </FieldGrid>
          </CardBody>
        </Card>

        <Card>
          <CardHeader
            title={<span className="inline-flex items-center gap-2">
              Line items <CountBadge count={filledLines.length} tone="muted" />
              {pricing && <Loader2 className="size-3.5 animate-spin text-fg-muted" aria-label="Pricing" />}
            </span>}
            actions={<>
              <Button size="sm" onClick={() => setLines((p) => [...p, startTax(emptyLine())])}>
                <Plus aria-hidden /> Add line
              </Button>
            </>} />

          {previewError && (
            <div role="alert" className="mx-4 mt-4 rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-4 py-2.5 text-xs text-fg">{previewError}</div>
          )}

          <div className="md:overflow-x-auto">
            <table className={cn("w-full text-sm md:min-w-[1060px]", stack.table)}>
              <thead className={stack.thead}>
                <tr className="border-b border-line bg-surface-2 text-left">
                  <th className={th}>Item</th>
                  <th className={`${th} w-20 text-right`}>Qty</th>
                  <th className={`${th} w-28`}>Term</th>
                  <th className={`${th} w-36 text-right`}>Unit price ({currencySymbol})</th>
                  <th className={`${th} w-28 text-right`}>Disc %</th>
                  <th className={`${th} w-36`}>Tax</th>
                  <th className={`${th} w-32 text-right`}>Amount</th>
                  <th className="w-10"><span className="sr-only">Remove</span></th>
                </tr>
              </thead>
              <tbody className={cn("divide-y divide-line-soft", stack.tbody)}>
                {ordered.map(({ line, depth }) => (
                  <LineRow key={line.key} line={line} depth={depth}
                    item={catalogById.get(line.product_id)}
                    priced={pricedByKey.get(line.key)}
                    catalog={catalog} taxes={taxes} currency={header.currency}
                    canManual={canManual} canDiscount={canDiscount}
                    onPick={(id) => pickProduct(line.key, id)}
                    onChange={(patch) => updateLine(line.key, patch)}
                    onAddon={() => setAddonFor(line.key)}
                    onRemove={() => removeLine(line.key)} />
                ))}
              </tbody>
            </table>
          </div>

          <div className="flex justify-end border-t border-line bg-surface-2 px-4 py-4">
            <div className="w-full space-y-2.5 text-sm md:w-80">
              <div className="flex justify-between">
                <span className="text-fg-muted">Subtotal</span>
                <span className="font-medium text-fg tabular">{formatCurrency(subtotal, header.currency)}</span>
              </div>
              <div className="flex items-center justify-between gap-2">
                <span className="text-fg-muted">Discount</span>
                <div className="flex items-center gap-2">
                  <div className="w-24">
                    <Select value={header.discount_type} aria-label="Discount type"
                      onChange={(e) => setHeader({ ...header, discount_type: e.target.value as "none"|"percentage"|"fixed" })}
                      options={[{ value: "none", label: "None" }, { value: "percentage", label: "%" }, { value: "fixed", label: header.currency }]} />
                  </div>
                  {header.discount_type !== "none" && (
                    <Input type="number" min="0" numeric value={header.discount_value} aria-label="Discount value" className="w-24"
                      onChange={(e) => setHeader({ ...header, discount_value: Number(e.target.value) })} />
                  )}
                  {serverDiscount != null && (
                    <span className="min-w-[80px] text-right font-medium text-tone-orange tabular">-{formatCurrency(serverDiscount, header.currency)}</span>
                  )}
                </div>
              </div>
              {(header.discount_type !== "none" || !!header.discount_note) && (
                <div className="space-y-1.5">
                  <Input value={header.discount_note ?? ""} aria-label="Discount note" disabled={!canDiscount}
                    placeholder="Why (printed with the discount)"
                    title={canDiscount ? undefined : "Needs the discount permission"}
                    onChange={(e) => setHeader({ ...header, discount_note: e.target.value })} />
                  <Select value={header.discount_display ?? "pdf"} aria-label="How the PDF shows the discount" disabled={!canDiscount}
                    onChange={(e) => setHeader({ ...header, discount_display: e.target.value as "pdf" | "notes_only" })}
                    options={[{ value: "pdf", label: "PDF: a Discount line with the note" },
                              { value: "notes_only", label: "Notes only: totals stay full" }]} />
                </div>
              )}
              <div className="flex items-center justify-between gap-2">
                <Switch checked={pricesExcludeTax} onCheckedChange={setPricesExcludeTax} label="Prices exclude tax" />
                <span className="text-right text-[11px] text-fg-muted">Every line no tax; the notes say so</span>
              </div>
              {taxWarning && (
                <p role="alert" className="rounded-md border border-tone-orange/30 bg-tone-orange-soft px-3 py-2 text-xs font-medium text-fg">{taxWarning}</p>
              )}
              {(preview?.tax_lines ?? []).map((t, i) => (
                <div key={`${t.tax_id ?? t.code}-${i}`} className="flex justify-between text-fg-muted">
                  <span>{t.name || t.code || "Tax"}{(t.pct ?? t.rate) != null ? ` (${t.pct ?? t.rate}%)` : ""}</span>
                  <span className="font-medium text-fg tabular">{formatCurrency(t.amount, header.currency)}</span>
                </div>
              ))}
              {!(preview?.tax_lines ?? []).length && (preview?.tax_amount ?? 0) > 0 && (
                <div className="flex justify-between text-fg-muted">
                  <span>Tax</span><span className="font-medium text-fg tabular">{formatCurrency(preview!.tax_amount, header.currency)}</span>
                </div>
              )}
              <div className="flex justify-between border-t border-line pt-2.5">
                <span className="font-semibold text-fg">Total</span>
                <div className="text-right">
                  <div className="text-lg font-semibold text-fg tabular">{formatCurrency(total, header.currency)}</div>
                  {isForeign && header.exchange_rate && (
                    <div className="mt-0.5 text-xs text-fg-muted tabular">≈ {formatCurrency(total * rate, functionalCurrency)}</div>
                  )}
                </div>
              </div>
              {hasOptional && (
                <div className="space-y-1.5 border-t border-dashed border-line pt-2.5 text-fg-muted" data-testid="optional-totals">
                  <div className="flex justify-between">
                    <span>Optional items (not in the total)</span>
                    <span className="font-medium text-fg tabular">{formatCurrency(optionalSubtotal, header.currency)}</span>
                  </div>
                  {optionalTax > 0 && (
                    <div className="flex justify-between text-xs">
                      <span>Their tax</span><span className="tabular">{formatCurrency(optionalTax, header.currency)}</span>
                    </div>
                  )}
                  <div className="flex justify-between text-xs">
                    <span>Total with optional items</span>
                    <span className="tabular">{formatCurrency(totalWithOptional, header.currency)}</span>
                  </div>
                </div>
              )}
              {preview && !preview.all_list && (
                <p className="text-right text-xs font-medium text-tone-orange">Manual prices or discounts: this quotation goes to approval.</p>
              )}
            </div>
          </div>
        </Card>

        <Card>
          <CardHeader title="Notes, terms and conditions"
            actions={<NoteTemplatePicker docType="quotation" currentValue={header.notes}
              onInsert={(html) => setHeader({ ...header, notes: header.notes ? header.notes + html : html })} />} />
          <CardBody>
            <div className="mb-3">
              <QuotationNotesHelper ctx={notesCtx}
                onFill={(html, terms) => setHeader((h) => ({ ...h, notes: html, payment_terms: terms || h.payment_terms }))} />
            </div>
            <RichTextEditor value={header.notes} onChange={(html) => setHeader({ ...header, notes: html })}
              placeholder="Payment terms, delivery schedule, special conditions, validity notes" />
          </CardBody>
        </Card>

        {error && (
          <div role="alert" className="rounded-lg border border-tone-red/30 bg-tone-red-soft px-4 py-3 text-sm font-medium text-tone-red">{error}</div>
        )}
        {lockedReason && (
          <div role="status" className="rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-4 py-3 text-sm font-medium text-fg">{lockedReason}</div>
        )}

        {/* Phones: the total stays in view while the lines are edited. */}
        <div className="sticky bottom-[calc(var(--shell-bottom)-6rem)] md:bottom-[-1.5rem] z-10 -mx-4 flex items-center justify-between gap-3 border-t border-line bg-surface px-4 py-2.5 md:hidden"
          aria-hidden>
          <span className="text-xs text-fg-muted">{filledLines.length} {filledLines.length === 1 ? "line" : "lines"}{pricing ? ", pricing" : ""}</span>
          <span className="text-sm font-semibold text-fg tabular">Total {formatCurrency(total, header.currency)}</span>
        </div>

        <div className="flex justify-end gap-2 pb-10">
          <Button asChild><Link href={backHref}>Cancel</Link></Button>
          <Button type="submit" variant="primary" loading={saving} disabled={!!lockedReason}>
            {saving ? (isEdit ? "Saving" : "Creating") : (isEdit ? "Save changes" : "Create quotation")}
          </Button>
        </div>
      </form>

      <Dialog open={!!addonParent} onOpenChange={(o) => { if (!o) setAddonFor(null); }}
        title={`Add to ${addonParent?.name || "line"}`}
        description="Care, extra modules and white-label attach to this line and are priced from it."
        footer={<Button onClick={() => setAddonFor(null)}>Cancel</Button>}>
        {addonParent && (
          <div className="max-h-80 space-y-1.5 overflow-y-auto">
            {addonChoices(catalog, addonParentItem?.sku).map((c) => (
              <button key={c.id} type="button" onClick={() => addChild(addonParent.key, c)}
                className="w-full rounded-lg border border-line px-4 py-2.5 text-left transition-colors hover:border-brand/40 hover:bg-brand/[0.07]">
                <span className="block text-sm font-medium text-fg">{c.name}</span>
                <span className="block text-xs text-fg-muted">
                  <span className="font-mono">{c.sku}</span>
                  {c.pricing_mode === "percent_of_parent" && c.percent_rate != null ? `, ${Number(c.percent_rate)}% of the parent price` : ""}
                  {c.sale_model === "recurring" ? `, ${recurringHint(c)}` : ""}
                </span>
              </button>
            ))}
          </div>
        )}
      </Dialog>

    </div>
  );
}

// One line

interface RowProps {
  line: LineItem; depth: number; item?: CatalogItem; priced?: PricedLine;
  catalog: CatalogItem[]; taxes: Tax[]; currency: string;
  canManual: boolean; canDiscount: boolean;
  onPick: (id: string) => void;
  onChange: (patch: Partial<LineItem>) => void;
  onAddon: () => void;
  onRemove: () => void;
}

const cell = "h-11 max-md:text-base md:h-8 md:text-xs";
// Phones: a line is a card, its fields two to a row.
const line_card = "max-md:grid max-md:grid-cols-2 max-md:gap-x-3";

function LineRow({ line, depth, item, priced, catalog, taxes, currency, canManual, canDiscount, onPick, onChange, onAddon, onRemove }: RowProps) {
  const isCustom = !line.product_id;
  const recurring = (item?.sale_model ?? priced?.sale_model) === "recurring";
  const termSource = item ?? (priced ? {
    billing_interval_months: priced.billing_interval_months, min_term_months: null, rate_period_months: priced.rate_period_months,
  } : null);
  const hint = item ? recurringHint(item) : priced ? recurringHint({ ...priced, min_term_months: null }) : "";
  const badge = priced ? priceRuleBadge(priced) : null;
  const priceEditable = canManual;
  const shownPrice = line.manual || isCustom ? (line.unit_price ?? "") : (priced?.unit_price ?? "");
  const canReset = line.manual && !isCustom && !needsTypedPrice(item);
  const hasAddons = !!item && addonChoices(catalog, item.sku).length > 0;
  const taxValue = line.tax_mode === "tax" ? line.tax_id : line.tax_mode === "custom" ? "__custom" : "";

  return (
    <tr className={cn(depth ? "bg-surface-2" : undefined, stack.tr, line_card, depth ? "max-md:ml-4 max-md:bg-surface-2 max-md:even:bg-surface-2" : "")}>
      <td className={cn("px-3 py-3 align-top", stack.title, "max-md:col-span-2")}>
        <div className="flex gap-1.5 max-md:!pl-0" style={{ paddingLeft: depth * 20 }}>
          {depth > 0 && <CornerDownRight className="mt-2 size-3.5 shrink-0 text-fg-subtle" aria-hidden />}
          <div className="min-w-0 flex-1 space-y-1">
            <SearchSelect size="sm" value={line.product_id} source={catalogSource}
              placeholder={canManual ? "Custom" : "Pick a catalog item"} emptyLabel="No products found"
              onChange={(id) => onPick(id)} />
            <Input value={line.name} onChange={(e) => onChange({ name: e.target.value })} aria-label="Item name"
              placeholder={isCustom && !canManual ? "Custom lines need the manual-price permission" : "Item name"}
              disabled={isCustom && !canManual} className={cell} />
            <Input value={line.description} onChange={(e) => onChange({ description: e.target.value })} aria-label="Description"
              placeholder="Description (optional)" className={`${cell} text-fg-muted`} />
            <div className="flex flex-wrap items-center gap-1.5 pt-0.5">
              {line.is_optional && <Pill tone="violet">Optional</Pill>}
              {badge && badge.label && <Pill tone={BADGE_TONE[badge.tone]}>{badge.label}</Pill>}
              {priced?.sku && <span className="font-mono text-[11px] text-fg-muted">{priced.sku}</span>}
              {hint && <span className="text-[11px] text-fg-muted">{hint}</span>}
              <Checkbox label={<span className="text-[11px] text-fg-muted">Optional</span>} checked={line.is_optional}
                onChange={(e) => onChange({ is_optional: e.target.checked })} />
              {hasAddons && (
                <Button size="sm" variant="ghost" onClick={onAddon} className="text-brand-ink"><Plus aria-hidden /> Add-on</Button>
              )}
            </div>
            {priced?.terms_note && <p className="whitespace-pre-line text-[11px] italic text-fg-muted">{priced.terms_note}</p>}
            {(priced?.notes ?? []).map((n, i) => <p key={i} className="text-[11px] text-fg-muted">{n}</p>)}
            {needsTypedPrice(item) && !canManual && (
              <p className="text-[11px] text-tone-orange">This item is priced by hand; someone with the manual-price permission must enter it.</p>
            )}
          </div>
        </div>
      </td>
      <td data-label="Qty" className={cn("px-3 py-3 align-top", stack.field)}>
        <Input type="number" min="0" step="any" numeric value={line.quantity} aria-label="Quantity" className={cell}
          onChange={(e) => onChange({ quantity: Number(e.target.value), follow_parent_qty: false })} />
        {item?.min_quantity ? <p className="mt-1 text-right text-[11px] text-fg-muted">min {Number(item.min_quantity)}</p> : null}
      </td>
      <td data-label="Term" className={cn("px-3 py-3 align-top", stack.field, !(recurring && termSource) && "max-md:hidden")}>
        {recurring && termSource ? (
          <Select value={line.term_months ?? ""} aria-label="Term" className={cell}
            onChange={(e) => onChange({ term_months: e.target.value ? Number(e.target.value) : null })}>
            {termOptions(termSource, line.term_months).map((m) => <option key={m} value={m}>{termLabel(m)}</option>)}
          </Select>
        ) : <span className="text-xs text-fg-subtle">-</span>}
      </td>
      <td data-label={`Unit price (${getCurrencySymbol(currency)})`} className={cn("px-3 py-3 align-top", stack.field)}>
        {priceEditable ? (
          <Input type="number" min="0" step="any" numeric value={shownPrice} aria-label="Unit price"
            onChange={(e) => onChange({ unit_price: e.target.value === "" ? 0 : Number(e.target.value), manual: true })}
            className={`${cell} ${line.manual && !isCustom ? "border-tone-orange" : ""}`} />
        ) : (
          <p className="py-2 text-right text-xs font-medium text-fg tabular">
            {priced ? formatCurrency(priced.unit_price, currency) : "-"}
          </p>
        )}
        {priced && priced.list_unit_price != null && Number(priced.list_unit_price) !== Number(priced.unit_price) && (
          <p className="mt-1 text-right text-[11px] text-fg-muted tabular">list {formatCurrency(priced.list_unit_price, currency)}</p>
        )}
        {canReset && (
          <Button size="sm" variant="ghost" className="ml-auto mt-1 flex" onClick={() => onChange({ manual: false, unit_price: null })}>
            <RotateCcw aria-hidden /> List price
          </Button>
        )}
      </td>
      <td data-label="Disc %" className={cn("px-3 py-3 align-top", stack.field)}>
        <Input type="number" min="0" max="100" step="any" numeric value={line.discount_pct ?? ""} disabled={!canDiscount}
          placeholder="0" aria-label="Discount %" className={cell}
          onChange={(e) => onChange({ discount_pct: e.target.value === "" ? null : Number(e.target.value) })}
          title={canDiscount ? "Leave blank for list price" : "Needs the discount permission"} />
        {/* Internal hint only: tiers are a negotiation lever, never shown to the customer. */}
        {priced && Number(priced.tier_available_pct ?? 0) > 0 && (
          <button type="button" disabled={!canDiscount}
            onClick={() => onChange({ discount_pct: Number(priced.tier_available_pct), discount_type: "volume" })}
            title="Internal: a discount up to this tier still counts as list and is auto-approved"
            className="mt-1 w-full text-right text-[11px] font-medium text-tone-blue hover:underline disabled:text-fg-subtle disabled:no-underline">
            Tier up to {Number(priced.tier_available_pct)}%
          </button>
        )}
        {(line.discount_pct ?? 0) > 0 && (
          <div className="mt-1">
            <Select value={line.discount_type} aria-label="Discount type" onChange={(e) => onChange({ discount_type: e.target.value })}
              aria-invalid={line.discount_type === "none" || undefined} className={cell}>
              <option value="none">Discount type</option>
              {DISCOUNT_TYPES.map((t) => <option key={t.value} value={t.value}>{t.label}</option>)}
            </Select>
          </div>
        )}
      </td>
      <td data-label="Tax" className={cn("px-3 py-3 align-top", stack.field)}>
        <Select value={taxValue} aria-label="Tax" className={cell}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "__custom") onChange({ tax_mode: "custom", tax_id: "", tax_pct: line.tax_pct ?? 0, tax_auto: false });
            else if (v) onChange({ tax_mode: "tax", tax_id: v, tax_pct: null, tax_auto: false });
            else onChange({ tax_mode: "none", tax_id: "", tax_pct: null, tax_auto: false });
          }}>
          <option value="">No tax</option>
          {line.tax_mode === "tax" && line.tax_id && !taxes.some((t) => t.id === line.tax_id) && (
            <option value={line.tax_id}>{priced?.tax_code ?? "Tax"} ({priced?.tax_pct ?? "?"}%)</option>
          )}
          {taxes.map((t) => <option key={t.id} value={t.id}>{t.name} ({t.rate}%)</option>)}
          <option value="__custom">Custom %</option>
        </Select>
        {line.tax_mode === "custom" && (
          <Input type="number" min="0" max="100" step="any" numeric value={line.tax_pct ?? ""} aria-label="Custom tax %"
            className={`${cell} mt-1`}
            onChange={(e) => onChange({ tax_pct: e.target.value === "" ? null : Number(e.target.value), tax_auto: false })} />
        )}
        {priced && Number(priced.tax_amount) > 0 && (
          <p className="mt-1 text-right text-[11px] text-fg-muted tabular">{formatCurrency(priced.tax_amount, currency)}</p>
        )}
      </td>
      <td data-label="Amount" className={cn("px-3 py-3 text-right align-top text-xs font-semibold text-fg tabular", stack.td, "max-md:col-span-2 max-md:text-sm")}>
        {priced ? formatCurrency(priced.amount, currency) : "-"}
        {priced && line.is_optional && <p className="mt-1 text-[11px] font-normal text-fg-muted">not in the total</p>}
        {priced && recurring && priced.term_months ? (
          <p className="mt-1 text-[11px] font-normal text-fg-muted">{termLabel(priced.term_months)}</p>
        ) : null}
      </td>
      <td className={cn("px-2 py-3 align-top", stack.actions, "max-md:col-span-2")}>
        <IconButton icon={Trash2} size="sm" label={depth ? "Remove add-on" : "Remove line and its add-ons"} onClick={onRemove}
          className="text-fg-muted hover:text-tone-red" />
      </td>
    </tr>
  );
}
