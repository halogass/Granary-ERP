"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { formatCurrency } from "@/lib/currency";
import { useCurrencySettings } from "@/hooks/useCurrencySettings";
import { useEffect, useState, useCallback, useMemo } from "react";
import { projectsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { FolderKanban, Plus, Search, Trash2 } from "lucide-react";
import Link from "next/link";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";

interface Project {
  currency?: string;
  id: string;
  code: string;
  name: string;
  status: string;
  start_date?: string;
  end_date?: string;
  budget?: number;
  customer_org_id?: string;
  pm_user_id?: string;
  customer_org?: { name: string };
  pm_user?: { full_name?: string; email: string };
  description?: string;
}

const STATUS_LABELS: Record<string, string> = {
  planning: "Planning", active: "Active", on_hold: "On hold", completed: "Completed", cancelled: "Cancelled", archived: "Archived",
};

type Filters = { status?: string; overdue?: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "planning", label: "Planning", filters: { status: "planning" } },
  { id: "overdue", label: "Overdue", filters: { overdue: true } },
];

const CHIP_STATUSES = ["on_hold", "completed", "cancelled", "archived"];

const fmt = (n: number, currency: string) => formatCurrency(n, currency);

function isOverdue(p: Project) {
  return p.status === "active" && !!p.end_date && new Date(p.end_date) < new Date();
}

/** How much of the planned timeline has passed, 0 to 100 (null without dates). */
function elapsed(p: Project): number | null {
  if (!p.start_date || !p.end_date) return null;
  const start = new Date(p.start_date).getTime();
  const total = new Date(p.end_date).getTime() - start;
  return Math.min(100, Math.max(0, Math.round(((Date.now() - start) / total) * 100)));
}

function Timeline({ p }: { p: Project }) {
  const pct = elapsed(p);
  if (pct == null) return <span className="text-fg-subtle">-</span>;
  const late = isOverdue(p);
  return (
    <div className="flex items-center justify-end gap-2" title={`${pct}% of the timeline elapsed`}>
      <span className={cn("text-[13px]", late ? "font-semibold text-tone-red" : "text-fg")}>
        {new Date(p.end_date!).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" })}
      </span>
      <div className="h-1 w-14 overflow-hidden rounded-full bg-surface-2" aria-hidden>
        <div className={cn("h-full rounded-full", late ? "bg-tone-red" : "bg-tone-blue")} style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

export default function ProjectsPage() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const views = useSavedViews<Filters>("projects.list", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [showModal, setShowModal] = useState(false);
  const { functional, active: activeCurrencies } = useCurrencySettings();
  const [form, setForm] = useState({ name: "", start_date: "", end_date: "", budget: "", currency: "", description: "" });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<Project>(projectsApi.list);
      setProjects(rows);
    } catch { setProjects([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const viewFilter = views.active?.filters;
  const statusFilter = chipStatus || viewFilter?.status || "";
  const filtered = projects.filter((p) => {
    if (viewFilter?.overdue && !isOverdue(p)) return false;
    const matchSearch = !search ||
      p.name.toLowerCase().includes(search.toLowerCase()) ||
      p.code.toLowerCase().includes(search.toLowerCase());
    const matchStatus = !statusFilter || p.status === statusFilter;
    return matchSearch && matchStatus;
  });

  const active = projects.filter((p) => p.status === "active").length;
  const planning = projects.filter((p) => p.status === "planning").length;
  const completed = projects.filter((p) => p.status === "completed").length;
  const overdue = projects.filter(isOverdue).length;

  async function deleteProject(id: string) {
    try {
      await projectsApi.delete(id);
      setConfirmDeleteId(null);
      setProjects((prev) => prev.filter((p) => p.id !== id));
    } catch { /* keep */ }
  }

  async function createProject() {
    setSaving(true);
    setError("");
    try {
      await projectsApi.create({
        name: form.name,
        start_date: form.start_date || undefined,
        end_date: form.end_date || undefined,
        budget: form.budget ? parseFloat(form.budget) : undefined,
        currency: form.currency || functional,
        description: form.description || undefined,
      });
      setShowModal(false);
      setForm({ name: "", start_date: "", end_date: "", budget: "", currency: "", description: "" });
      await load();
    } catch (err) {
      setError(extractApiError(err, "Failed to create project."));
    } finally { setSaving(false); }
  }

  const columns = useMemo<ColumnDef<Project>[]>(() => [
    {
      accessorKey: "name", header: "Project",
      cell: (c) => (
        <div className="min-w-0">
          <p className="max-w-64 truncate font-medium text-fg">{c.row.original.name}</p>
          <p className="font-mono text-xs text-fg-muted">{c.row.original.code}</p>
        </div>
      ),
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => (
        <div className="flex flex-wrap items-center gap-1.5">
          <StatusPill family="project" status={c.row.original.status} label={STATUS_LABELS[c.row.original.status]} />
          {isOverdue(c.row.original) && <Pill tone="red">Overdue</Pill>}
        </div>
      ),
    },
    {
      id: "customer", accessorFn: (p) => p.customer_org?.name ?? "", header: "Customer", meta: { hideOnMobile: true },
      cell: (c) => <span className="block max-w-40 truncate">{c.getValue<string>() || "-"}</span>,
    },
    {
      id: "pm", accessorFn: (p) => p.pm_user?.full_name ?? p.pm_user?.email ?? "", header: "PM", meta: { hideOnMobile: true },
      cell: (c) => <span className="block max-w-32 truncate">{c.getValue<string>() || "-"}</span>,
    },
    {
      accessorKey: "end_date", header: "Due", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => <Timeline p={c.row.original} />,
    },
    {
      accessorKey: "budget", header: "Budget", meta: { numeric: true },
      cell: (c) => (c.row.original.budget ? fmt(c.row.original.budget, c.row.original.currency || functional) : "-"),
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const p = c.row.original;
        if (p.status === "active") return null;
        if (confirmDeleteId === p.id) {
          return (
            <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
              <span className="text-xs font-semibold text-tone-red">Delete?</span>
              <Button size="sm" variant="danger" onClick={() => deleteProject(p.id)}>Yes</Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
            </div>
          );
        }
        return <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${p.name}`} onClick={() => setConfirmDeleteId(p.id)} />;
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, functional]);

  const closeCreate = () => { setShowModal(false); setError(""); };

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Active" value={active} />
        <Stat label="Planning" value={planning} />
        <Stat label="Completed" value={completed} />
        <Stat label="Overdue" value={overdue} alert={overdue > 0} note={overdue > 0 ? "past their end date" : undefined} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Projects" summary="Track deliverables, timelines and team effort"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New project</Button>} />
        <SavedViewTabs state={views} currentFilters={statusFilter ? { status: statusFilter } : viewFilter}
          onSelect={() => setChipStatus("")} counts={{ overdue: overdue || undefined }} alertIds={["overdue"]} />
        <FilterChips
          chips={CHIP_STATUSES.map((st) => ({ id: st, label: STATUS_LABELS[st], active: chipStatus === st }))}
          onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))} onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search projects" aria-label="Search projects" className="w-56" />
        </FilterChips>
        <DataTable label="Projects" columns={columns} data={filtered} getRowId={(p) => p.id} loading={loading}
          empty={search || statusFilter ? {
            icon: FolderKanban, title: "No matching projects", description: "Try adjusting the filters.",
          } : {
            icon: FolderKanban, title: "No projects yet", description: "Create your first project to start tracking work.",
            action: <Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New project</Button>,
          }}
          renderDetails={(p) => (
            <div className="flex flex-wrap items-start gap-6">
              <div className="min-w-[260px] flex-1 space-y-3">
                {p.description && <p className="line-clamp-3 text-[13px] text-fg-muted">{p.description}</p>}
                <KeyValues cols={3} items={[
                  { label: "Customer", value: p.customer_org?.name ?? "-" },
                  { label: "Project manager", value: p.pm_user?.full_name ?? p.pm_user?.email ?? "-" },
                  { label: "Budget", value: p.budget ? fmt(p.budget, p.currency || functional) : "-", numeric: true },
                  { label: "Start", value: p.start_date ? new Date(p.start_date).toLocaleDateString("en-GB") : "-" },
                  { label: "End", value: p.end_date ? new Date(p.end_date).toLocaleDateString("en-GB") : "-" },
                  { label: "Timeline elapsed", value: elapsed(p) != null ? `${elapsed(p)}%` : "-", numeric: true },
                ]} />
              </div>
              <Button size="sm" asChild><Link href={`/projects/${p.id}`}>Open project</Link></Button>
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={(o) => { if (!o) closeCreate(); }} title="New project"
        footer={<>
          <Button onClick={closeCreate}>Cancel</Button>
          <Button variant="primary" onClick={createProject} disabled={!form.name.trim()} loading={saving}>
            {saving ? "Creating..." : "Create project"}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Project name" required>
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Website relaunch, phase 2" />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Start date"><Input type="date" value={form.start_date} onChange={(e) => setForm({ ...form, start_date: e.target.value })} /></Field>
            <Field label="End date"><Input type="date" value={form.end_date} onChange={(e) => setForm({ ...form, end_date: e.target.value })} /></Field>
          </FieldGrid>
          <div className="grid grid-cols-3 gap-2">
            <Field label="Currency">
              <Select value={form.currency || functional} onChange={(e) => setForm({ ...form, currency: e.target.value })}
                options={activeCurrencies.map((c) => ({ value: c, label: c }))} />
            </Field>
            <Field label="Budget" className="col-span-2">
              <Input numeric type="number" value={form.budget} onChange={(e) => setForm({ ...form, budget: e.target.value })} placeholder="0" />
            </Field>
          </div>
          <Field label="Description">
            <Textarea value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} rows={3} placeholder="Brief project overview..." />
          </Field>
          {error && <p role="alert" className="rounded-lg border border-tone-red/30 bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{error}</p>}
        </div>
      </Dialog>
    </div>
  );
}
