"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { rndApi, catalogApi } from "@/lib/api";
import { FlaskConical, Plus, Search, Tag, Trash2 } from "lucide-react";
import Link from "next/link";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader,
  SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface RDProject {
  id: string;
  code: string;
  name: string;
  description?: string;
  status: string;
  product_ref?: string;
  latest_version?: string;
  lead_engineer_id?: string;
  backlog_counts?: Record<string, number>;
  created_at: string;
}

interface ProductLine { code: string; name: string; color: string }

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All projects", filters: {} },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "paused", label: "Paused", filters: { status: "paused" } },
  { id: "archived", label: "Archived", filters: { status: "archived" } },
];

const EMPTY_FORM = { code: "", name: "", description: "", product_ref: "", status: "active" };
const itemTotals = (p: RDProject) => {
  const total = Object.values(p.backlog_counts ?? {}).reduce((a, b) => a + b, 0);
  return { total, done: (p.backlog_counts ?? {})["done"] ?? 0 };
};

export default function RNDProjectsPage() {
  const views = useSavedViews<Filters>("rnd.projects", VIEWS);
  const [projects, setProjects] = useState<RDProject[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [productFilter, setProductFilter] = useState("");
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [productLines, setProductLines] = useState<ProductLine[]>([]);

  useEffect(() => { catalogApi.list().then((r) => setProductLines(r.data ?? [])).catch(() => {}); }, []);
  // Product-line codes are stored lower case; a project's product_ref may not be (older rows, imports, the API)
  const productMeta = useMemo(() => Object.fromEntries(productLines.map((p) => [p.code.toLowerCase(), p])), [productLines]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<RDProject>(rndApi.listProjects);
      setProjects(rows);
    } catch { setProjects([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const statusFilter = views.active?.filters.status ?? "";
  const filters: Filters = statusFilter ? { status: statusFilter } : {};
  const filtered = projects.filter((p) => {
    const matchSearch = !search ||
      p.name.toLowerCase().includes(search.toLowerCase()) ||
      p.code.toLowerCase().includes(search.toLowerCase());
    const matchStatus = !statusFilter || p.status === statusFilter;
    const matchProduct = !productFilter || p.product_ref?.toLowerCase() === productFilter.toLowerCase();
    return matchSearch && matchStatus && matchProduct;
  });

  const active = projects.filter((p) => p.status === "active").length;
  const products = new Set(projects.map((p) => p.product_ref).filter(Boolean)).size;

  async function deleteProject(id: string) {
    try {
      await rndApi.deleteProject(id);
      setConfirmDeleteId(null);
      setProjects((prev) => prev.filter((p) => p.id !== id));
    } catch { /* keep */ }
  }

  async function createProject() {
    if (!form.code.trim() || !form.name.trim()) return;
    setSaving(true);
    try {
      await rndApi.createProject({
        code: form.code.toUpperCase().trim(),
        name: form.name.trim(),
        description: form.description || undefined,
        product_ref: form.product_ref || undefined,
        status: form.status,
      });
      setShowModal(false);
      setForm(EMPTY_FORM);
      await load();
    } finally { setSaving(false); }
  }

  const columns = useMemo<ColumnDef<RDProject>[]>(() => [
    {
      accessorKey: "name", header: "Project",
      cell: (c) => (
        <div className="min-w-0">
          <Link href={`/rnd/projects/${c.row.original.id}`} className="font-medium text-fg hover:text-brand-ink">{c.row.original.name}</Link>
          <p className="font-mono text-xs text-fg-muted">{c.row.original.code}</p>
        </div>
      ),
    },
    {
      accessorKey: "product_ref", header: "Product", meta: { hideOnMobile: true },
      cell: (c) => {
        const pm = c.getValue<string>() ? productMeta[c.getValue<string>().toLowerCase()] : undefined;
        if (!pm) return <span className="text-fg-subtle">-</span>;
        return (
          <span className="inline-flex items-center gap-1.5 text-[13px] text-fg">
            {/* the line's own color (data, set per product line) */}
            <span className="size-2 rounded-full" style={{ background: pm.color }} aria-hidden />{pm.name}
          </span>
        );
      },
    },
    {
      id: "progress", accessorFn: (p) => { const t = itemTotals(p); return t.total ? t.done / t.total : -1; }, header: "Items done", meta: { numeric: true },
      cell: (c) => {
        const { total, done } = itemTotals(c.row.original);
        if (!total) return <span className="text-fg-subtle">-</span>;
        return (
          <span className="inline-flex items-center justify-end gap-2">
            <span className="hidden h-1.5 w-20 overflow-hidden rounded-full bg-surface-2 sm:block" aria-hidden>
              <span className="block h-full rounded-full bg-tone-blue" style={{ width: `${Math.round((done / total) * 100)}%` }} />
            </span>
            {done}/{total}
          </span>
        );
      },
    },
    {
      accessorKey: "latest_version", header: "Version", meta: { hideOnMobile: true },
      cell: (c) => c.getValue<string>() ? <span className="inline-flex items-center gap-1 text-fg-muted"><Tag className="size-3" aria-hidden />{c.getValue<string>()}</span> : <span className="text-fg-subtle">-</span>,
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill status={c.getValue<string>()} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const p = c.row.original;
        return confirmDeleteId === p.id ? (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <span className="text-xs font-semibold text-tone-red">Delete?</span>
            <Button size="sm" variant="danger" onClick={() => deleteProject(p.id)}>Yes</Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
          </div>
        ) : <IconButton size="sm" icon={Trash2} label={`Delete ${p.name}`} onClick={() => setConfirmDeleteId(p.id)} />;
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, productMeta]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total projects" value={projects.length} />
        <Stat label="Active" value={active} />
        <Stat label="Products" value={products} />
        <Stat label="Total items" value={projects.reduce((s, p) => s + itemTotals(p).total, 0)} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="R&D projects" summary="Internal product development and sprint tracking"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New project</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} />
        <FilterChips
          chips={productLines.map((pl) => ({ id: pl.code, label: pl.name, active: productFilter === pl.code }))}
          onToggle={(id) => setProductFilter((cur) => (cur === id ? "" : id))} onClear={() => setProductFilter("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search projects" aria-label="Search projects" className="w-60" />
        </FilterChips>
        <DataTable label="R&D projects" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{
            icon: FlaskConical, title: "No R&D projects yet", description: "Track internal product development and sprints.",
            action: <Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Start a project</Button>,
          }}
          renderDetails={(p) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Description", value: p.description || "-" },
                { label: "Latest version", value: p.latest_version ?? "-" },
                { label: "Created", value: new Date(p.created_at).toLocaleDateString("en-GB") },
              ]} />
              <Button size="sm" asChild><Link href={`/rnd/projects/${p.id}`}>Open project</Link></Button>
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="New R&D project"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={createProject} loading={saving} disabled={!form.code.trim() || !form.name.trim()}>
            {saving ? "Creating..." : "Create project"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Code" required>
            <Input value={form.code} onChange={(e) => setForm({ ...form, code: e.target.value.toUpperCase() })} placeholder="RND-INVAPP" className="font-mono" />
          </Field>
          <Field label="Product">
            <Select value={form.product_ref} onChange={(e) => setForm({ ...form, product_ref: e.target.value })}
              placeholder="Select a product line" options={productLines.map((pl) => ({ value: pl.code, label: pl.name }))} />
          </Field>
          <Field label="Project name" required className="md:col-span-2">
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Customer portal v3, CSV export" />
          </Field>
          <Field label="Description" className="md:col-span-2">
            <Textarea value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} rows={3}
              placeholder="Brief overview of this project's scope and goals" />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
