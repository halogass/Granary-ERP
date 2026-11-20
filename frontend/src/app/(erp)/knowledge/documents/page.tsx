"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { CalendarClock, FileSpreadsheet, FileText, Plus, Search, Upload } from "lucide-react";
import {
  Button, Combobox, DataTable, FilterChips, Input, KeyValues, ListFrame, PageHeader, Pager, Pill, SavedViewTabs,
  StatusPill, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { usePermissions } from "@/hooks/usePermissions";
import { extractApiError } from "@/lib/apiError";
import { catalogSource } from "@/lib/entitySources";
import {
  AUDIENCES, DOC_VIEWS, audienceLabel, docLibApi, formatFileSize, languageLabel, listParams,
  productLabel, type DocFilters, type LibraryDocument,
} from "@/lib/documentLibrary";
import { NewDocumentDialog, UploadVersionDialog, useDocCategories } from "@/components/documents/DocumentDialogs";
import { DownloadButton } from "@/components/documents/DocumentCards";

const VIEWS: SavedView<DocFilters>[] = DOC_VIEWS.map((v) => ({ id: v.id, label: v.label, filters: { view: v.id } }));
const PAGE_SIZE = 50;

const fmtDate = (d?: string | null) => (d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-");
const audienceTone = (a: string) => (a === "internal" ? "neutral" : a === "partner" ? "violet" : "blue");

export default function DocumentsPage() {
  const router = useRouter();
  const { can } = usePermissions();
  const canCreate = can("documents:create");
  const categories = useDocCategories();
  const views = useSavedViews<DocFilters>("knowledge.documents", VIEWS);
  const [chips, setChips] = useState<DocFilters>({});
  const [q, setQ] = useState("");
  const [debounced, setDebounced] = useState("");
  const [page, setPage] = useState(1);
  const [rows, setRows] = useState<LibraryDocument[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const [uploadFor, setUploadFor] = useState<LibraryDocument | null>(null);
  const [generating, setGenerating] = useState(false);

  useEffect(() => { const t = setTimeout(() => setDebounced(q), 300); return () => clearTimeout(t); }, [q]);

  // ?product=<id> (from a catalog item's documents) starts filtered to that product.
  // ?version=<id> (the link on an approval request) opens the document that version belongs to.
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const product = params.get("product");
    if (product) setChips((c) => ({ ...c, product_id: product }));
    const version = params.get("version");
    if (version) {
      docLibApi.list({ version })
        .then((res) => {
          const doc = res.data.items?.[0];
          if (doc) router.replace(`/knowledge/documents/${doc.id}`);
          else toast.error("That document version is not available to you.");
        })
        .catch((e) => toast.error(extractApiError(e, "That document version could not be opened.")));
    }
  }, [router]);

  const filters: DocFilters = useMemo(() => ({
    ...views.active?.filters,
    ...Object.fromEntries(Object.entries(chips).filter(([, v]) => v)),
  }), [views.active?.filters, chips]);
  const filterKey = JSON.stringify(filters);

  useEffect(() => { setPage(1); }, [filterKey, debounced]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await docLibApi.list(listParams(JSON.parse(filterKey) as DocFilters, debounced, page, PAGE_SIZE));
      setRows(res.data.items ?? []);
      setTotal(res.data.total ?? 0);
    } catch (e) {
      setRows([]);
      setTotal(0);
      toast.error(extractApiError(e, "Documents could not be loaded."));
    } finally {
      setLoading(false);
    }
  }, [filterKey, debounced, page]);

  useEffect(() => { void load(); }, [load]);

  async function generatePriceList() {
    setGenerating(true);
    try {
      const res = await docLibApi.generatePriceList();
      toast.success("Price list generated");
      if (res.data?.id) router.push(`/knowledge/documents/${res.data.id}`);
      else void load();
    } catch (e) {
      toast.error(extractApiError(e, "The price list could not be generated."));
    } finally {
      setGenerating(false);
    }
  }

  const languages = useMemo(() => {
    const seen = new Set(["en", "id", ...rows.map((r) => r.language)]);
    if (filters.language) seen.add(filters.language);
    return Array.from(seen).filter(Boolean);
  }, [rows, filters.language]);

  const columns = useMemo<ColumnDef<LibraryDocument>[]>(() => [
    {
      accessorKey: "title", header: "Document",
      cell: (c) => {
        const d = c.row.original;
        return (
          <div className="min-w-0">
            <Link href={`/knowledge/documents/${d.id}`} className="block truncate font-medium text-fg hover:text-brand-ink">{d.title}</Link>
            <span className="text-xs text-fg-muted">{d.category?.name ?? "-"}</span>
          </div>
        );
      },
    },
    {
      id: "status", header: "Status", enableSorting: false,
      cell: (c) => {
        const d = c.row.original;
        if (d.pending_version) return <StatusPill family="doc_version" status="pending_approval" label={`${d.pending_version.label} waiting`} />;
        if (d.needs_review) return <StatusPill family="followup" status="overdue" label="Needs review" />;
        return d.current_version ? <StatusPill family="doc_version" status="live" /> : <StatusPill family="doc_version" status="draft" label="No live version" />;
      },
    },
    {
      id: "version", header: "Version", accessorFn: (d) => d.current_version?.label ?? "",
      cell: (c) => <span className="font-mono text-[13px]">{c.row.original.current_version?.label ?? "-"}</span>,
    },
    {
      id: "audiences", header: "For", enableSorting: false, meta: { hideOnMobile: true },
      cell: (c) => (
        <div className="flex flex-wrap gap-1">
          {c.row.original.audiences.map((a) => <Pill key={a} tone={audienceTone(a)}>{audienceLabel(a)}</Pill>)}
        </div>
      ),
    },
    { accessorKey: "language", header: "Language", meta: { hideOnMobile: true }, cell: (c) => languageLabel(c.getValue<string>()) },
    { id: "owner", header: "Owner", accessorFn: (d) => d.owner?.name ?? "", meta: { hideOnMobile: true }, cell: (c) => c.row.original.owner?.name ?? "-" },
    {
      accessorKey: "updated_at", header: "Updated", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => fmtDate(c.row.original.current_version?.went_live_at ?? c.row.original.updated_at),
    },
  ], []);

  const chipList = [
    ...categories.map((cat) => ({ id: `category:${cat.key}`, label: cat.name, active: filters.category === cat.key })),
    ...AUDIENCES.map((a) => ({ id: `audience:${a.value}`, label: a.label, active: filters.audience === a.value })),
    ...languages.map((l) => ({ id: `language:${l}`, label: languageLabel(l), active: filters.language === l })),
  ];
  const toggleChip = (id: string) => {
    const i = id.indexOf(":");
    const k = id.slice(0, i) as keyof DocFilters;
    const v = id.slice(i + 1);
    setChips((c) => ({ ...c, [k]: c[k] === v ? undefined : v }));
  };

  const summary = `${total} document${total === 1 ? "" : "s"}${views.active ? ` in ${views.active.label}` : ""}`;

  return (
    <ListFrame>
      <PageHeader flush title="Documents" summary={summary}
        actions={canCreate ? <>
          <Button onClick={generatePriceList} loading={generating}><FileSpreadsheet aria-hidden /> Generate price list</Button>
          <Button variant="primary" onClick={() => setCreating(true)}><Plus aria-hidden /> New document</Button>
        </> : undefined} />
      <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChips({})} />
      <FilterChips chips={chipList} onToggle={toggleChip} onClear={() => setChips({})}>
        <div className="flex w-full flex-col gap-2 md:w-auto md:flex-row md:items-center">
          <div className="md:w-56">
            <Combobox value={filters.product_id ?? ""} allowClear placeholder="Any product" source={catalogSource}
              onChange={(id) => setChips((c) => ({ ...c, product_id: id || undefined }))} />
          </div>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search documents" aria-label="Search documents" className="md:w-56" />
        </div>
      </FilterChips>
      <DataTable label="Documents" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
        empty={total === 0 && !debounced && filters.view === "all" && !chips.category && !chips.audience && !chips.language && !chips.product_id
          ? { icon: FileText, title: "No documents yet", description: "Add the company profile, brochures and datasheets so staff and Nary send the right version.",
              action: canCreate ? <Button variant="primary" onClick={() => setCreating(true)}><Plus aria-hidden /> New document</Button> : undefined }
          : { icon: FileText, title: "No documents in this view", description: "Pick another view or clear the filters." }}
        renderDetails={(d) => {
          const v = d.current_version;
          return (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Current version", value: v ? <span><span className="font-mono">{v.label}</span>{v.went_live_at ? `, live since ${fmtDate(v.went_live_at)}` : ""}</span> : "No live version yet" },
                { label: "Change note", value: v?.change_note || "-" },
                { label: "File", value: v ? `${v.file_name}${v.file_size ? `, ${formatFileSize(v.file_size)}` : ""}${v.page_count ? `, ${v.page_count} pages` : ""}` : "-" },
                {
                  label: "Languages",
                  value: (
                    <span className="flex flex-wrap gap-x-2">
                      <span>{languageLabel(d.language)}</span>
                      {(d.language_variants ?? []).map((lv) => (
                        <Link key={lv.id} href={`/knowledge/documents/${lv.id}`} className="text-brand-ink hover:underline">{languageLabel(lv.language)}</Link>
                      ))}
                    </span>
                  ),
                },
                { label: "Products", value: d.products?.length ? d.products.map(productLabel).join(", ") : "-" },
                {
                  label: "Review by",
                  value: d.review_by
                    ? <span className={d.needs_review ? "inline-flex items-center gap-1 font-medium text-tone-red" : undefined}>
                        {d.needs_review && <CalendarClock className="size-3.5" aria-hidden />}{fmtDate(d.review_by)}
                      </span>
                    : "-",
                },
                ...(d.pending_version ? [{ label: "Waiting for approval", value: <span className="font-mono">{d.pending_version.label}</span> }] : []),
              ]} />
              <div className="flex flex-wrap gap-2">
                <DownloadButton doc={d} />
                {d.can_edit && <Button size="sm" onClick={() => setUploadFor(d)}><Upload aria-hidden /> Upload new version</Button>}
                <Button size="sm" variant="primary" asChild><Link href={`/knowledge/documents/${d.id}`}>Open document</Link></Button>
              </div>
            </div>
          );
        }}
        footer={total > PAGE_SIZE ? <Pager page={page} pageSize={PAGE_SIZE} total={total} onPage={setPage} /> : undefined} />

      <NewDocumentDialog open={creating} onOpenChange={setCreating} categories={categories} />
      {uploadFor && (
        <UploadVersionDialog open onOpenChange={(o) => { if (!o) setUploadFor(null); }} doc={uploadFor} onUploaded={() => void load()} />
      )}
    </ListFrame>
  );
}

