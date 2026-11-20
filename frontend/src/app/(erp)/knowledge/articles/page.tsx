"use client";

import { useEffect, useMemo, useState, useCallback, useRef } from "react";
import { kbApi } from "@/lib/api";
import { useDropToInput } from "@/hooks/useFileDrop";
import {
  BookOpen, Plus, Search, Eye, Edit3, CheckCircle2, Tag, Upload, AlertCircle, Trash2, Lock, X,
} from "lucide-react";
import Link from "next/link";
import {
  Button, DataTable, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader, Pill, SavedViewTabs, Stat, StatGroup,
  StatusPill, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";

interface KBCategory {
  id: string;
  name: string;
  slug: string;
}

interface KBArticle {
  id: string;
  title: string;
  slug: string;
  status: string;
  article_type: string;
  sop_code?: string;
  view_count?: number;
  category_id?: string;
  created_at: string;
  updated_at?: string;
  tags?: string[];
  version?: string;
  is_private?: boolean;
}

type Filters = { status?: string; type?: string; category?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "published", label: "Published", filters: { status: "published" } },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "archived", label: "Archived", filters: { status: "archived" } },
  { id: "sop", label: "SOPs", filters: { type: "sop" } },
];

const fmtDate = (d?: string) => (d ? new Date(d).toLocaleDateString("en-GB") : "-");

export default function KBArticlesPage() {
  const [articles, setArticles] = useState<KBArticle[]>([]);
  const [categories, setCategories] = useState<KBCategory[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const views = useSavedViews<Filters>("knowledge.articles", VIEWS);
  const [chips, setChips] = useState<Filters>({});
  const [importing, setImporting] = useState(false);
  const [importResult, setImportResult] = useState<{ success: boolean; title?: string; id?: string } | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const { dragActive: mdDrag, dropHandlers: mdDrop } = useDropToInput(fileInputRef, { single: true, accept: (f) => f.name.toLowerCase().endsWith(".md") });

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [articlesRes, catsRes] = await Promise.all([
        kbApi.listArticles({ limit: 1000 }),
        kbApi.listCategories(),
      ]);
      setArticles(articlesRes.data.items ?? articlesRes.data ?? []);
      setCategories(catsRes.data.items ?? catsRes.data ?? []);
    } catch { setArticles([]); setCategories([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const filters: Filters = { ...views.active?.filters, ...Object.fromEntries(Object.entries(chips).filter(([, v]) => v)) };
  const categoryFilter = filters.category ?? "";
  const statusFilter = filters.status ?? "";
  const typeFilter = filters.type ?? "";

  const filtered = articles.filter((a) => {
    const matchSearch = !search || a.title.toLowerCase().includes(search.toLowerCase());
    const matchCat = !categoryFilter || a.category_id === categoryFilter;
    const matchStatus = !statusFilter || a.status === statusFilter;
    const matchType = !typeFilter || a.article_type === typeFilter;
    return matchSearch && matchCat && matchStatus && matchType;
  });

  const published = articles.filter((a) => a.status === "published").length;
  const sops = articles.filter((a) => a.article_type === "sop").length;
  const totalViews = articles.reduce((s, a) => s + (a.view_count ?? 0), 0);

  async function deleteArticle(id: string) {
    try {
      await kbApi.deleteArticle(id);
      setConfirmDeleteId(null);
      setArticles((prev) => prev.filter((a) => a.id !== id));
    } catch { /* keep */ }
  }

  async function handleImport(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setImporting(true);
    setImportResult(null);
    try {
      const res = await kbApi.importMarkdown(file);
      setImportResult({ success: true, title: res.data.title, id: res.data.id });
      await load();
    } catch {
      setImportResult({ success: false });
    } finally {
      setImporting(false);
      if (fileInputRef.current) fileInputRef.current.value = "";
    }
  }

  const articleTypes = [...new Set(articles.map((a) => a.article_type))].sort();
  const catName = (id?: string) => categories.find((c) => c.id === id)?.name;

  const columns = useMemo<ColumnDef<KBArticle>[]>(() => [
    {
      accessorKey: "title", header: "Article",
      cell: (c) => {
        const a = c.row.original;
        return (
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <Link href={`/knowledge/articles/${a.id}`} className="truncate font-medium text-fg hover:text-brand-ink">{a.title}</Link>
              {a.is_private && <span title="Only visible to you"><Pill tone="orange" icon={Lock}>Private</Pill></span>}
              {a.sop_code && <span className="shrink-0 font-mono text-xs text-fg-muted">{a.sop_code}</span>}
            </div>
            {a.tags && a.tags.length > 0 && (
              <div className="mt-0.5 flex gap-1">
                {a.tags.slice(0, 3).map((tag) => (
                  <span key={tag} className="inline-flex items-center gap-0.5 text-xs text-fg-subtle"><Tag className="size-2.5" aria-hidden />{tag}</span>
                ))}
              </div>
            )}
          </div>
        );
      },
    },
    {
      accessorKey: "article_type", header: "Type", meta: { hideOnMobile: true },
      cell: (c) => <Pill tone={c.getValue<string>() === "sop" ? "violet" : "neutral"} className="capitalize">{c.getValue<string>()}</Pill>,
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="knowledge" status={c.getValue<string>()} /> },
    {
      accessorKey: "view_count", header: "Views", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => c.getValue<number | undefined>() ?? 0,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right" },
      cell: (c) => {
        const a = c.row.original;
        return (
          <div className="flex items-center justify-end gap-1">
            <Button size="sm" variant="ghost" asChild><Link href={`/knowledge/articles/${a.id}/edit`} aria-label={`Edit ${a.title}`}><Edit3 aria-hidden /></Link></Button>
            {confirmDeleteId === a.id ? (
              <span className="flex items-center gap-1">
                <span className="text-xs font-medium text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deleteArticle(a.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </span>
            ) : (
              <IconButton icon={Trash2} size="sm" label={`Delete ${a.title}`} className="hover:text-tone-red" onClick={() => setConfirmDeleteId(a.id)} />
            )}
          </div>
        );
      },
    },
  // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  const chipList = [
    ...articleTypes.map((t) => ({ id: `type:${t}`, label: <span className="capitalize">{t}</span>, active: typeFilter === t })),
    ...categories.map((cat) => ({ id: `category:${cat.id}`, label: cat.name, active: categoryFilter === cat.id })),
  ];
  const toggleChip = (id: string) => {
    const i = id.indexOf(":");
    const k = id.slice(0, i) as "type" | "category";
    const v = id.slice(i + 1);
    setChips((c) => ({ ...c, [k]: c[k] === v ? undefined : v }));
  };

  return (
    <div className="space-y-4">
      {importResult && (
        <div role="status" className={cn("flex items-center gap-3 rounded-lg border px-4 py-2.5 text-sm",
          importResult.success ? "border-line bg-tone-blue-soft text-fg" : "border-tone-red/30 bg-tone-red-soft text-tone-red")}>
          {importResult.success ? (
            <>
              <CheckCircle2 className="size-4 shrink-0 text-tone-blue" aria-hidden />
              <span>Imported: <Link href={`/knowledge/articles/${importResult.id}`} className="font-medium underline">{importResult.title}</Link></span>
            </>
          ) : (
            <>
              <AlertCircle className="size-4 shrink-0" aria-hidden />
              <span>Import failed: make sure the file is a valid .md file</span>
            </>
          )}
          <IconButton icon={X} size="sm" label="Dismiss" className="ml-auto" onClick={() => setImportResult(null)} />
        </div>
      )}

      <StatGroup>
        <Stat label="Total articles" value={articles.length} />
        <Stat label="Published" value={published} />
        <Stat label="SOPs" value={sops} />
        <Stat label="Total views" value={totalViews} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Knowledge base" summary={`${filtered.length} of ${articles.length} articles, SOPs, runbooks and documentation`}
          actions={<>
            <input ref={fileInputRef} type="file" accept=".md" className="hidden" onChange={handleImport} aria-label="Import a Markdown file" />
            <Button onClick={() => fileInputRef.current?.click()} disabled={importing} {...mdDrop}
              className={cn(mdDrag && "border-brand bg-brand/[0.07] text-brand-ink")}>
              <Upload aria-hidden />
              {importing ? "Importing..." : mdDrag ? "Drop .md" : "Import .md"}
            </Button>
            <Button variant="primary" asChild><Link href="/knowledge/articles/new"><Plus aria-hidden /> New article</Link></Button>
          </>} />
        <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChips({})} />
        <FilterChips chips={chipList} onToggle={toggleChip} onClear={() => setChips({})}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search articles" aria-label="Search articles" className="w-56" />
        </FilterChips>
        <DataTable label="Articles" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={articles.length === 0
            ? { icon: BookOpen, title: "No articles yet", description: "Document processes and SOPs for your team.",
                action: <Button variant="primary" asChild><Link href="/knowledge/articles/new"><Plus aria-hidden /> Write first article</Link></Button> }
            : { icon: BookOpen, title: "No articles in this view", description: "Pick another view or clear the filters." }}
          renderDetails={(a) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Category", value: catName(a.category_id) ?? "-" },
                { label: "Version", value: a.version ? `v${a.version}` : "-" },
                { label: "Views", value: <span className="inline-flex items-center gap-1"><Eye className="size-3" aria-hidden />{a.view_count ?? 0}</span>, numeric: true },
                { label: "Created", value: fmtDate(a.created_at), numeric: true },
                { label: "Updated", value: fmtDate(a.updated_at), numeric: true },
                { label: "Tags", value: a.tags?.length ? a.tags.join(", ") : "-" },
              ]} />
              <div className="flex gap-2">
                <Button size="sm" asChild><Link href={`/knowledge/articles/${a.id}/edit`}><Edit3 aria-hidden /> Edit</Link></Button>
                <Button size="sm" variant="primary" asChild><Link href={`/knowledge/articles/${a.id}`}>Open article</Link></Button>
              </div>
            </div>
          )} />
      </ListFrame>
    </div>
  );
}
