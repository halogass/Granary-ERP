"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { salesApi, systemApi } from "@/lib/api";
import { Package, Plus, Search, Edit3, Trash2, Eye } from "lucide-react";
import { formatCurrency } from "@/lib/currency";
import { extractApiError } from "@/lib/apiError";
import { recurringHint } from "@/lib/catalogPricing";
import type { CatalogItem } from "@/lib/types/catalog";
import { segmentLabel, segmentOf, useSegments } from "@/lib/segments";
import ProductEditor from "@/components/sales/catalog/ProductEditor";
import ProductDocumentsPanel from "@/components/documents/ProductDocumentsPanel";
import { PRODUCT_TYPES, PRICING_MODES } from "@/components/sales/catalog/productForm";
import {
  Button, DataTable, FilterChips, Input, KeyValues, ListFrame, PageHeader, Pill, SavedViewTabs, Select,
  Stat, StatGroup, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";

type Kind = "recurring" | "one_time" | "addon";
type Filters = { kind?: Kind };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All items", filters: {} },
  { id: "one_time", label: "One-time", filters: { kind: "one_time" } },
  { id: "recurring", label: "Recurring", filters: { kind: "recurring" } },
  { id: "addon", label: "Add-ons", filters: { kind: "addon" } },
];

function matchesKind(p: CatalogItem, kind?: Kind) {
  if (!kind) return true;
  if (kind === "addon") return (p.addon_for ?? []).length > 0;
  return p.sale_model === kind;
}

function soldAs(p: CatalogItem) {
  return p.sale_model === "recurring" ? recurringHint(p) : (p.price_metric && p.price_metric !== "unit" ? `One-time, per ${p.price_metric}` : "One-time");
}

const NO_FAMILY = "Other";

const typeLabel = (t: string) => PRODUCT_TYPES.find((x) => x.value === t)?.label ?? t;
const modeLabel = (m: string) => PRICING_MODES.find((x) => x.value === m)?.label ?? m;

/** Group items by family (sorted), items by SKU inside each group. */
function groupByFamily(items: CatalogItem[]): [string, CatalogItem[]][] {
  const groups = new Map<string, CatalogItem[]>();
  for (const it of items) {
    const k = it.family || NO_FAMILY;
    groups.set(k, [...(groups.get(k) ?? []), it]);
  }
  return Array.from(groups.entries())
    .sort(([a], [b]) => (a === NO_FAMILY ? 1 : b === NO_FAMILY ? -1 : a.localeCompare(b)))
    .map(([k, v]) => [k, v.sort((x, y) => x.sku.localeCompare(y.sku))]);
}

export default function CatalogPage() {
  const views = useSavedViews<Filters>("sales.catalog", VIEWS);
  const [products, setProducts] = useState<CatalogItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [search, setSearch] = useState("");
  const [family, setFamily] = useState("");
  const [includeRetired, setIncludeRetired] = useState(false);
  const [editing, setEditing] = useState<CatalogItem | null | undefined>(undefined);   // undefined = closed, null = new
  const [currencies, setCurrencies] = useState<string[]>([]);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [canEdit, setCanEdit] = useState(false);
  const segments = useSegments();

  useEffect(() => {
    systemApi.getCurrencySettings()
      .then((r) => {
        const list = r.data?.active ?? r.data?.active_currencies ?? r.data?.currencies;
        if (Array.isArray(list)) setCurrencies(list);
      })
      .catch(() => {});
    // Only catalog managers edit (list prices drive quotation auto-approval).
    salesApi.catalogAccess().then((r) => setCanEdit(!!r.data.can_edit)).catch(() => setCanEdit(false));
  }, []);

  const load = useCallback(async () => {
    setLoading(true); setLoadError("");
    try {
      const res = await salesApi.listCatalog({ include_retired: includeRetired, limit: 500 });
      setProducts((res.data.items ?? res.data ?? []) as CatalogItem[]);
    } catch (e) {
      setProducts([]);
      setLoadError(extractApiError(e, "Could not load the catalog."));
    } finally { setLoading(false); }
  }, [includeRetired]);

  useEffect(() => { load(); }, [load]);

  const families = useMemo(() => Array.from(new Set(products.map((p) => p.family || NO_FAMILY))).sort(), [products]);

  const filtered = products.filter((p) => {
    const q = search.trim().toLowerCase();
    const matchSearch = !q || p.name.toLowerCase().includes(q) || (p.sku ?? "").toLowerCase().includes(q);
    const matchFamily = !family || (p.family || NO_FAMILY) === family;
    return matchSearch && matchFamily && matchesKind(p, views.active?.filters.kind);
  });
  // Family order (Other last), SKU order inside a family.
  const rows = groupByFamily(filtered).flatMap(([, items]) => items);

  async function deleteProduct(id: string) {
    setDeleting(true);
    try {
      await salesApi.deleteProduct(id);
      setConfirmDeleteId(null);
      await load();
    } catch (e) {
      setLoadError(extractApiError(e, "Could not delete the item."));
    } finally { setDeleting(false); }
  }

  function onSaved(item: CatalogItem) {
    setProducts((prev) => (prev.some((p) => p.id === item.id) ? prev.map((p) => (p.id === item.id ? item : p)) : [...prev, item]));
  }

  const orderable = products.filter((p) => p.is_active && p.is_orderable).length;
  const recurring = products.filter((p) => p.sale_model === "recurring").length;
  const addons = products.filter((p) => (p.addon_for ?? []).length > 0).length;

  const columns: ColumnDef<CatalogItem>[] = [
    {
      accessorKey: "name", header: "Item",
      cell: (c) => {
        const p = c.row.original;
        return (
          <div className="min-w-0">
            <p className="max-w-80 truncate font-medium text-fg">{p.name}</p>
            <p className="font-mono text-xs text-fg-muted">{p.sku}</p>
          </div>
        );
      },
    },
    {
      id: "family", accessorFn: (p) => p.family || NO_FAMILY, header: "Family", meta: { hideOnMobile: true },
      cell: (c) => <span className="font-mono text-xs text-fg-muted">{c.getValue<string>()}</span>,
    },
    {
      id: "tags", header: "Type", enableSorting: false,
      cell: (c) => {
        const p = c.row.original;
        const retired = !p.is_active || !p.is_orderable;
        return (
          <div className="flex flex-wrap items-center gap-1">
            <Pill>{typeLabel(p.product_type)}</Pill>
            {p.pricing_mode !== "list" && (
              <Pill tone="orange">{p.pricing_mode === "percent_of_parent" && p.percent_rate != null ? `${Number(p.percent_rate)}% of parent` : modeLabel(p.pricing_mode)}</Pill>
            )}
            {(p.addon_for ?? []).length > 0 && <Pill tone="blue" title={(p.addon_for ?? []).join(", ")}>Add-on ({(p.addon_for ?? []).length})</Pill>}
            {p.is_whitelabel && <Pill tone="violet">White-label</Pill>}
            {(p.volume_tiers ?? []).length > 0 && <Pill tone="blue">{(p.volume_tiers ?? []).length} tiers</Pill>}
            {retired && <Pill>{!p.is_active ? "Retired" : "Not orderable"}</Pill>}
          </div>
        );
      },
    },
    {
      // the segment, and a proposed (not yet locked) price
      id: "segment", accessorFn: (p) => segmentLabel(segments, segmentOf(p)), header: "Segment", meta: { hideOnMobile: true },
      cell: (c) => {
        const p = c.row.original;
        return (
          <div className="flex flex-wrap items-center gap-1">
            <span className="text-[13px] text-fg-muted">{c.getValue<string>()}</span>
            {p.price_status === "proposed" && <Pill tone="orange" title="Proposed price: the Owner approves every quotation with it">Proposed</Pill>}
          </div>
        );
      },
    },
    {
      id: "sold", header: "Sold as", enableSorting: false, meta: { hideOnMobile: true },
      cell: (c) => <span className="text-[13px] text-fg-muted">{soldAs(c.row.original)}</span>,
    },
    {
      id: "price", header: "List price", enableSorting: false, meta: { numeric: true },
      cell: (c) => {
        const p = c.row.original;
        const prices = Object.entries(p.current_prices ?? {});
        return prices.length ? (
          <div className="flex flex-col items-end">{prices.map(([cur, amt]) => <span key={cur} className="font-medium">{formatCurrency(amt, cur)}</span>)}</div>
        ) : <span className="text-fg-subtle">{p.pricing_mode === "list" ? "No list price" : "-"}</span>;
      },
    },
  ];

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Shown items" value={products.length} />
        <Stat label="Orderable" value={orderable} />
        <Stat label="Recurring" value={recurring} />
        <Stat label="Add-ons" value={addons} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Product catalog"
          summary="SKUs, price lists, volume tiers and add-ons. The single source of every quoted price."
          actions={
            <div className="flex flex-wrap items-center justify-end gap-2">
              {canEdit ? (
                <Button variant="primary" onClick={() => setEditing(null)}><Plus aria-hidden /> New item</Button>
              ) : (
                <p className="max-w-[16rem] text-right text-xs text-fg-muted">Only catalog managers can edit the catalog.</p>
              )}
            </div>
          } />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} />
        <FilterChips chips={[{ id: "retired", label: "Show retired", active: includeRetired }]}
          onToggle={() => setIncludeRetired((v) => !v)} onClear={() => setIncludeRetired(false)}>
          <div className="w-44">
            <Select value={family} onChange={(e) => setFamily(e.target.value)} aria-label="Family" placeholder="All families"
              options={families.map((f) => ({ value: f, label: f }))} />
          </div>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other" value={search}
            onChange={(e) => setSearch(e.target.value)} placeholder="Search name or SKU" aria-label="Search name or SKU" className="w-60" />
        </FilterChips>
        {loadError && <p role="alert" className="border-b border-line bg-tone-red-soft px-4 py-2 text-sm text-tone-red">{loadError}</p>}
        <DataTable label="Catalog items" columns={columns} data={rows} getRowId={(p) => p.id} loading={loading}
          empty={{
            icon: Package, title: "No items", description: canEdit ? "Add the first item to the catalog." : "Nothing in this view.",
            action: canEdit ? <Button variant="primary" onClick={() => setEditing(null)}><Plus aria-hidden /> Add an item</Button> : undefined,
          }}
          renderDetails={(p) => {
            const isConfirming = confirmDeleteId === p.id;
            return (
              <div className="space-y-4">
              <div className={cn("flex flex-wrap items-start gap-6", (!p.is_active || !p.is_orderable) && "opacity-80")}>
                <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                  { label: "Family", value: p.family || NO_FAMILY },
                  { label: "Type", value: typeLabel(p.product_type) },
                  { label: "Pricing", value: modeLabel(p.pricing_mode) },
                  { label: "Sold as", value: soldAs(p) },
                  { label: "Add-on for", value: (p.addon_for ?? []).length ? <span className="font-mono text-xs">{(p.addon_for ?? []).join(", ")}</span> : "-" },
                  { label: "Volume tiers", value: (p.volume_tiers ?? []).length || "-", numeric: true },
                ]} />
                <div className="flex items-center gap-2">
                  {isConfirming ? (
                    <>
                      <span className="text-xs font-semibold text-tone-red">Delete this item?</span>
                      <Button size="sm" variant="danger" loading={deleting} onClick={() => deleteProduct(p.id)}>Delete</Button>
                      <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>Cancel</Button>
                    </>
                  ) : (
                    <>
                      <Button size="sm" onClick={() => setEditing(p)}>
                        {canEdit ? <><Edit3 aria-hidden /> Edit item</> : <><Eye aria-hidden /> View item</>}
                      </Button>
                      {canEdit && (
                        <Button size="sm" variant="danger" onClick={() => setConfirmDeleteId(p.id)}><Trash2 aria-hidden /> Delete</Button>
                      )}
                    </>
                  )}
                </div>
              </div>
              {/* Library documents linked to the item or its family (Nary sends these). */}
              <ProductDocumentsPanel product={p} />
              </div>
            );
          }} />
      </ListFrame>

      {editing !== undefined && (
        <ProductEditor product={editing} catalog={products} currencies={currencies} canEdit={canEdit}
          onClose={() => setEditing(undefined)} onSaved={onSaved} />
      )}
    </div>
  );
}
