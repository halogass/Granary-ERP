"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState, useCallback, useMemo } from "react";
import { projectsApi } from "@/lib/api";
import { TASK_PRIORITY_LABELS, TASK_STATUS_LABELS } from "@/lib/projectTasks";
import { CheckSquare, ChevronDown, ChevronRight, Search, Trash2 } from "lucide-react";
import {
  Button, DataTable, FilterChips, IconButton, Input, ListFrame, PageHeader, Pill, SavedViewTabs, Stat, StatGroup, StatusPill,
  useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Task {
  id: string;
  title: string;
  description?: string;
  status: string;
  priority: string;
  project_id: string;
  project?: { name: string; id: string };
  assignee_id?: string;
  assignee?: { full_name: string };
  due_date?: string;
  estimated_hours?: number;
  created_at: string;
}

const STATUS_LABELS = TASK_STATUS_LABELS;
const PRIORITY_LABELS = TASK_PRIORITY_LABELS;

const NEXT_STATUS: Record<string, string> = {
  todo: "in_progress",
  in_progress: "review",
  review: "done",
};

type Filters = { status?: string; open?: boolean; overdue?: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "open", label: "Open", filters: { open: true } },
  { id: "overdue", label: "Overdue", filters: { overdue: true } },
  { id: "done", label: "Done", filters: { status: "done" } },
  { id: "all", label: "All", filters: {} },
];

export default function MyTasksPage() {
  const [tasks, setTasks] = useState<Task[]>([]);
  const [loading, setLoading] = useState(true);
  const views = useSavedViews<Filters>("projects.my-tasks", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [search, setSearch] = useState("");
  const [groupByProject, setGroupByProject] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows: projects } = await fetchAllPages<{ id: string; name: string; tasks?: Task[] }>(projectsApi.list, { include_tasks: true });
      const allTasks: Task[] = [];
      projects.forEach((p: { id: string; name: string; tasks?: Task[] }) => {
        (p.tasks ?? []).forEach((t: Task) => {
          allTasks.push({ ...t, project: { id: p.id, name: p.name } });
        });
      });
      setTasks(allTasks);
    } catch {
      try {
        await fetchAllPages(projectsApi.list);
        setTasks([]);
      } catch { setTasks([]); }
    } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function deleteTask(id: string) {
    try {
      await projectsApi.deleteTask(id);
      setConfirmDeleteId(null);
      setTasks((prev) => prev.filter((t) => t.id !== id));
    } catch { /* keep */ }
  }

  async function advance(taskId: string, projectId: string, nextStatus: string) {
    await projectsApi.updateTask(taskId, { status: nextStatus });
    await load();
  }

  const now = new Date();
  const isLate = (t: Task) => !!t.due_date && new Date(t.due_date) < now && t.status !== "done";
  const view = views.active?.filters;
  const q = search.trim().toLowerCase();
  const filtered = tasks.filter((t) => {
    if (chipStatus && t.status !== chipStatus) return false;
    if (view?.open && t.status === "done") return false;
    if (view?.status && t.status !== view.status) return false;
    if (view?.overdue && !isLate(t)) return false;
    return !q || [t.title, t.project?.name, t.assignee?.full_name].some((v) => (v ?? "").toLowerCase().includes(q));
  });
  const overdue = filtered.filter((t) => t.due_date && new Date(t.due_date) < now && t.status !== "done").length;
  const done = filtered.filter((t) => t.status === "done").length;
  const inProgress = filtered.filter((t) => t.status === "in_progress").length;

  const grouped: Record<string, { projectName: string; tasks: Task[] }> = {};
  filtered.forEach((t) => {
    const key = t.project_id;
    if (!grouped[key]) grouped[key] = { projectName: t.project?.name ?? "Unknown Project", tasks: [] };
    grouped[key].tasks.push(t);
  });

  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [collapsedProjects, setCollapsedProjects] = useState<Set<string>>(new Set());
  function toggleProject(id: string) {
    setCollapsedProjects((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  }

  const columns = useMemo<ColumnDef<Task>[]>(() => [
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="task" status={c.row.original.status} label={STATUS_LABELS[c.row.original.status]} />,
    },
    {
      accessorKey: "title", header: "Task",
      cell: (c) => (
        <div className="min-w-0">
          <p className="max-w-80 truncate font-medium text-fg">{c.row.original.title}</p>
          {!groupByProject && c.row.original.project && <p className="max-w-80 truncate text-xs text-fg-muted">{c.row.original.project.name}</p>}
        </div>
      ),
    },
    {
      accessorKey: "priority", header: "Priority", meta: { hideOnMobile: true },
      cell: (c) => <StatusPill family="priority" status={c.row.original.priority} label={PRIORITY_LABELS[c.row.original.priority]} />,
    },
    {
      id: "assignee", accessorFn: (t) => t.assignee?.full_name ?? "", header: "Assignee", meta: { hideOnMobile: true },
      cell: (c) => <span className="block max-w-36 truncate">{c.getValue<string>() || "-"}</span>,
    },
    {
      accessorKey: "due_date", header: "Due", meta: { numeric: true },
      cell: (c) => {
        const t = c.row.original;
        if (!t.due_date) return <span className="text-fg-subtle">-</span>;
        const d = new Date(t.due_date).toLocaleDateString("en-GB", { day: "numeric", month: "short" });
        return isLate(t) ? <Pill tone="red">{d}, overdue</Pill> : d;
      },
    },
    {
      accessorKey: "estimated_hours", header: "Estimate", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => (c.row.original.estimated_hours ? `${c.row.original.estimated_hours} h` : "-"),
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const task = c.row.original;
        const nextStatus = NEXT_STATUS[task.status];
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            {nextStatus && (
              <Button size="sm" onClick={() => advance(task.id, task.project_id, nextStatus)}>Move to {STATUS_LABELS[nextStatus]?.toLowerCase()}</Button>
            )}
            {confirmDeleteId === task.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deleteTask(task.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </>
            ) : (
              <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${task.title}`} onClick={() => setConfirmDeleteId(task.id)} />
            )}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, groupByProject]);

  const table = (rows: Task[], label: string) => (
    <DataTable label={label} columns={columns} data={rows} getRowId={(t) => t.id} loading={loading} maxHeight={groupByProject ? "none" : undefined}
      empty={{ icon: CheckSquare, title: "No tasks found", description: "Pick another view or clear the filters." }} />
  );

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={tasks.length} />
        <Stat label="In progress" value={inProgress} />
        <Stat label="Done" value={done} />
        <Stat label="Overdue" value={overdue} alert={overdue > 0} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="My tasks" summary="Tasks assigned across all projects" />
        <SavedViewTabs state={views} currentFilters={chipStatus ? { status: chipStatus } : view} onSelect={() => setChipStatus("")} alertIds={["overdue"]} />
        <FilterChips
          chips={[
            ...Object.entries(STATUS_LABELS).map(([k, l]) => ({ id: k, label: l, active: chipStatus === k })),
            { id: "__group", label: "Group by project", active: groupByProject },
          ]}
          onToggle={(id) => (id === "__group" ? setGroupByProject((v) => !v) : setChipStatus((cur) => (cur === id ? "" : id)))}
          onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search tasks" aria-label="Search tasks" className="w-56" />
        </FilterChips>
        {groupByProject && !loading && filtered.length > 0 ? (
          <div className="divide-y divide-line">
            {Object.entries(grouped).map(([projectId, { projectName, tasks: ptasks }]) => {
              const collapsed = collapsedProjects.has(projectId);
              return (
                <section key={projectId}>
                  <button type="button" onClick={() => toggleProject(projectId)} aria-expanded={!collapsed}
                    className="flex min-h-11 w-full items-center gap-2 bg-surface-2 px-4 text-left hover:bg-surface md:min-h-9">
                    {collapsed ? <ChevronRight className="size-4 text-fg-muted" aria-hidden /> : <ChevronDown className="size-4 text-fg-muted" aria-hidden />}
                    <span className="text-sm font-semibold text-fg">{projectName}</span>
                    <span className="text-xs text-fg-muted tabular">{ptasks.length}</span>
                  </button>
                  {!collapsed && table(ptasks, `Tasks in ${projectName}`)}
                </section>
              );
            })}
          </div>
        ) : table(filtered, "Tasks")}
      </ListFrame>
    </div>
  );
}
