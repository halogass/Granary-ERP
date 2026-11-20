"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { rndProjectSource } from "@/lib/entitySources";
import { rndApi } from "@/lib/api";
import { List, Search, ChevronRight, Trash2 } from "lucide-react";
import Link from "next/link";
import {
  Button, Combobox, DataTable, FilterChips, IconButton, Input, ListFrame, PageHeader, Pill, SavedViewTabs, Select,
  StatusPill, useSavedViews, type ColumnDef, type SavedView, type Tone,
} from "@/components/ui";

interface BacklogItem {
  id: string;
  rnd_project_id: string;
  title: string;
  item_type: string;
  status: string;
  priority: string;
  story_points?: number;
  sprint_id?: string;
  labels?: string[];
  created_at: string;
  /** 17 Nov 2026: the synced GitLab issue */
  gitlab_issue_iid?: number | null;
  gitlab_issue_url?: string | null;
}

interface RDProject { id: string; code: string; name: string; }

const PRIORITY_META: Record<string, { tone: Tone; label: string }> = {
  critical: { tone: "red",     label: "Critical" },
  high:     { tone: "orange",  label: "High" },
  medium:   { tone: "neutral", label: "Medium" },
  low:      { tone: "neutral", label: "Low" },
};

const TYPE_TONES: Record<string, Tone> = {
  feature: "blue", bug: "red", tech_debt: "violet", research: "blue", spike: "orange",
  security: "red", documentation: "neutral", internal_tool: "neutral",
};

const STATUSES = ["backlog", "refined", "in_sprint", "in_progress", "review", "done"];

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All items", filters: {} },
  ...STATUSES.map((s) => ({ id: s, label: s.charAt(0).toUpperCase() + s.slice(1).replace("_", " "), filters: { status: s } })),
];

export default function RNDBacklogPage() {
  const views = useSavedViews<Filters>("rnd.backlog", VIEWS);
  const [items, setItems] = useState<BacklogItem[]>([]);
  const [projects, setProjects] = useState<RDProject[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [projectFilter, setProjectFilter] = useState("");
  const [priorityFilter, setPriorityFilter] = useState("");
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [itemsRes, projRes] = await Promise.all([
        fetchAllPages<BacklogItem>(rndApi.listBacklog),
        fetchAllPages<RDProject>(rndApi.listProjects),
      ]);
      setItems(itemsRes.rows);
      setProjects(projRes.rows);
    } catch { setItems([]); setProjects([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function deleteBacklogItem(id: string) {
    try {
      await rndApi.deleteBacklogItem(id);
      setConfirmDeleteId(null);
      setItems((prev) => prev.filter((i) => i.id !== id));
    } catch { /* keep */ }
  }

  const statusFilter = views.active?.filters.status ?? "";
  const filters: Filters = statusFilter ? { status: statusFilter } : {};
  const filtered = items.filter((item) => {
    const matchSearch = !search || item.title.toLowerCase().includes(search.toLowerCase());
    const matchProject = !projectFilter || item.rnd_project_id === projectFilter;
    const matchStatus = !statusFilter || item.status === statusFilter;
    const matchPriority = !priorityFilter || item.priority === priorityFilter;
    return matchSearch && matchProject && matchStatus && matchPriority;
  });

  const projectMap = useMemo(() => Object.fromEntries(projects.map((p) => [p.id, p])), [projects]);
  const counts = Object.fromEntries(STATUSES.map((s) => [s, items.filter((i) => i.status === s).length]));

  const columns = useMemo<ColumnDef<BacklogItem>[]>(() => [
    {
      accessorKey: "title", header: "Item",
      cell: (c) => {
        const item = c.row.original;
        const proj = projectMap[item.rnd_project_id];
        return (
          <div className="min-w-0">
            <p className="max-w-md truncate font-medium text-fg">{item.title}</p>
            <div className="mt-0.5 flex flex-wrap items-center gap-2">
              {proj && <Link href={`/rnd/projects/${proj.id}`} className="font-mono text-xs text-brand-ink hover:underline">{proj.code}</Link>}
              {item.gitlab_issue_url && (
                <a href={item.gitlab_issue_url} target="_blank" rel="noreferrer" title="Open the GitLab issue"
                  onClick={(e) => e.stopPropagation()}
                  className="font-mono text-xs text-fg-muted hover:text-brand-ink hover:underline">GitLab #{item.gitlab_issue_iid}</a>
              )}
              {item.labels && item.labels.slice(0, 3).map((l) => (
                <span key={l} className="rounded bg-surface-2 px-1.5 py-0.5 text-xs text-fg-muted">{l}</span>
              ))}
            </div>
          </div>
        );
      },
    },
    { accessorKey: "item_type", header: "Type", meta: { hideOnMobile: true }, cell: (c) => <Pill tone={TYPE_TONES[c.getValue<string>()] ?? "neutral"} className="capitalize">{c.getValue<string>().replace("_", " ")}</Pill> },
    { accessorKey: "story_points", header: "Points", meta: { numeric: true, hideOnMobile: true }, cell: (c) => c.getValue<number>() || <span className="text-fg-subtle">-</span> },
    { accessorKey: "priority", header: "Priority", cell: (c) => { const pm = PRIORITY_META[c.getValue<string>()]; return <Pill tone={pm?.tone ?? "neutral"}>{pm?.label ?? c.getValue<string>()}</Pill>; } },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="rnd_item" status={c.getValue<string>()} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const item = c.row.original;
        if (["done", "released"].includes(item.status)) return null;
        return confirmDeleteId === item.id ? (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <span className="text-xs font-semibold text-tone-red">Delete?</span>
            <Button size="sm" variant="danger" onClick={() => deleteBacklogItem(item.id)}>Yes</Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
          </div>
        ) : <IconButton size="sm" icon={Trash2} label={`Delete ${item.title}`} onClick={() => setConfirmDeleteId(item.id)} />;
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, projectMap]);

  return (
    <ListFrame>
      <PageHeader flush title="R&D backlog" summary="All items across all R&D projects"
        actions={<Button asChild><Link href="/rnd/projects">View projects <ChevronRight aria-hidden /></Link></Button>} />
      <SavedViewTabs state={views} currentFilters={filters} counts={counts} />
      <FilterChips chips={[]} onToggle={() => {}}>
        <div className="w-56">
          <Combobox value={projectFilter} source={rndProjectSource} placeholder="All projects" emptyLabel="No projects found"
            onChange={(id) => setProjectFilter(id)} />
        </div>
        <Select aria-label="Priority" value={priorityFilter} onChange={(e) => setPriorityFilter(e.target.value)} className="w-40"
          placeholder="All priorities" options={Object.entries(PRIORITY_META).map(([k, v]) => ({ value: k, label: v.label }))} />
        <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
          value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search items" aria-label="Search items" className="w-56" />
      </FilterChips>
      <DataTable label="Backlog items" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
        rowHref={(item) => projectMap[item.rnd_project_id] ? `/rnd/projects/${item.rnd_project_id}` : undefined}
        empty={{ icon: List, title: "No items match your filters", description: "Pick another view or clear the filters." }} />
    </ListFrame>
  );
}
