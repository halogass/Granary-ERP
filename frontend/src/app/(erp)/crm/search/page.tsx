"use client";

import { Suspense, useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Filter, Loader2, Sparkles, X } from "lucide-react";
import { crmRecordsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, CountBadge, DataTable, ListFrame, PageHeader, type ColumnDef } from "@/components/ui";

interface Row {
  id: string; label: string;
  industry?: string | null; city?: string | null; country?: string | null;
  email?: string | null; position?: string | null;
  status?: string | null; priority?: string | null;
  stage?: string | null; value?: number | null; currency?: string | null;
}
interface FilterSpec { field: string; op: string; value?: unknown }

const PATH: Record<string, string> = {
  organization: "/crm/organizations",
  contact: "/crm/contacts",
  lead: "/crm/leads",
  deal: "/crm/deals",
};

/** Decode the `q` the assistant put in the link: base64 of {object_type, filters}. */
function decode(q: string | null): { object_type: string; filters: FilterSpec[] } {
  if (!q) return { object_type: "organization", filters: [] };
  try {
    const json = decodeURIComponent(escape(window.atob(q)));
    const parsed = JSON.parse(json);
    return { object_type: parsed.object_type || "organization", filters: parsed.filters || [] };
  } catch {
    return { object_type: "organization", filters: [] };
  }
}

/**
 * The filtered list behind an assistant answer: the same filters, shown and editable.
 * Ask questions in Nary's chat; this page is where its results open in full.
 */
function CrmSearchInner() {
  const params = useSearchParams();
  const [objectType, setObjectType] = useState("organization");
  const [filters, setFilters] = useState<FilterSpec[]>([]);
  const [items, setItems] = useState<Row[]>([]);
  const [count, setCount] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const run = useCallback(async (type: string, specs: FilterSpec[]) => {
    setLoading(true);
    setError("");
    try {
      const r = await crmRecordsApi.searchFilters({ object_type: type, filters: specs, limit: 100 });
      setItems(r.data.items ?? []);
      setCount(r.data.count ?? (r.data.items ?? []).length);
    } catch (e) {
      setError(extractApiError(e, "Could not run those filters."));
      setItems([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    const { object_type, filters: specs } = decode(params.get("q"));
    setObjectType(object_type);
    setFilters(specs);
    run(object_type, specs);
  }, [params, run]);

  function removeFilter(index: number) {
    const next = filters.filter((_, i) => i !== index);
    setFilters(next);
    run(objectType, next);
  }

  const columns = useMemo<ColumnDef<Row>[]>(() => [
    {
      accessorKey: "label", header: "Record",
      cell: (c) => {
        const r = c.row.original;
        return (
          <div className="min-w-0">
            <Link href={`${PATH[objectType] ?? "/crm"}/${r.id}`} className="font-medium text-fg hover:text-brand-ink">{r.label || "-"}</Link>
            <p className="truncate text-xs text-fg-muted">
              {[r.industry, r.city, r.country, r.email, r.position, r.status, r.priority, r.stage]
                .filter(Boolean).join(", ") || "-"}
            </p>
          </div>
        );
      },
    },
    {
      accessorKey: "value", header: "Value", meta: { numeric: true },
      cell: (c) => {
        const r = c.row.original;
        return r.value != null ? `${r.currency || "USD"} ${r.value.toLocaleString()}` : "";
      },
    },
  ], [objectType]);

  return (
    <div className="max-w-4xl">
      <ListFrame>
        <PageHeader flush title="CRM list"
          summary="The records behind an assistant answer. Remove a filter to widen the list."
          actions={<Button asChild><Link href="/nary/chat"><Sparkles aria-hidden className="text-brand-ink" /> Ask the assistant</Link></Button>} />
        <div className="flex flex-wrap items-center gap-1.5 border-b border-line bg-surface px-4 py-2" role="group" aria-label="Filters">
          <span className="mr-1 inline-flex items-center gap-1.5 text-xs text-fg-muted">
            <Filter className="size-3.5" aria-hidden /> Filters, <span className="capitalize">{objectType}</span>
          </span>
          {filters.length === 0 && <span className="text-xs text-fg-muted">No filters: showing everything.</span>}
          {filters.map((f, i) => (
            <span key={i} className="inline-flex min-h-9 items-center gap-1 rounded-lg border border-line bg-surface-2 pl-2.5 font-mono text-[11px] text-fg md:min-h-7">
              {f.field} {f.op} {String(f.value ?? "")}
              <button type="button" aria-label={`Remove filter ${f.field}`} title="Remove this filter" onClick={() => removeFilter(i)}
                className="flex size-7 items-center justify-center rounded-lg text-fg-subtle hover:text-tone-red">
                <X className="size-3" aria-hidden />
              </button>
            </span>
          ))}
          <span className="ml-auto inline-flex items-center gap-2 text-[13px] text-fg-muted">
            Results <CountBadge count={count} tone="muted" />
            {loading && <Loader2 className="size-3.5 animate-spin" aria-hidden />}
          </span>
        </div>
        {error && <div role="alert" className="border-b border-line bg-tone-red-soft px-4 py-3 text-sm text-tone-red">{error}</div>}
        <DataTable label="Results" columns={columns} data={items} getRowId={(r) => r.id} loading={loading}
          rowHref={(r) => `${PATH[objectType] ?? "/crm"}/${r.id}`}
          empty={{ title: "Nothing matched", description: "Try removing a filter above." }} />
      </ListFrame>
    </div>
  );
}

export default function CrmSearchPage() {
  return (
    <Suspense fallback={<div className="py-16 flex justify-center"><Loader2 className="size-6 animate-spin text-brand-ink" aria-label="Loading" /></div>}>
      <CrmSearchInner />
    </Suspense>
  );
}
