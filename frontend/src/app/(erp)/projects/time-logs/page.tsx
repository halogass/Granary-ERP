"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState, useCallback } from "react";
import { projectsApi } from "@/lib/api";
import { Timer, Plus } from "lucide-react";
import {
  Button, Card, CardBody, CardHeader, DataTable, Dialog, Field, FieldGrid, FilterChips, Input, ListFrame, PageHeader,
  Select, Stat, StatGroup, Textarea, type ColumnDef,
} from "@/components/ui";

interface TimeLog {
  id: string;
  project_id: string;
  project?: { name: string };
  task_id?: string;
  task?: { title: string };
  user_id: string;
  user?: { full_name: string };
  hours: number;
  description?: string;
  date: string;          // YYYY-MM-DD
}

interface Project {
  id: string;
  name: string;
}

export default function TimeLogsPage() {
  const [logs, setLogs] = useState<TimeLog[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState({ project_id: "", hours: "", description: "", date: new Date().toISOString().split("T")[0] });
  const [saving, setSaving] = useState(false);
  const [filterProject, setFilterProject] = useState("all");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [logRes, projRes] = await Promise.all([
        fetchAllPages<TimeLog>(projectsApi.listTimeLogs),
        fetchAllPages<Project>(projectsApi.list),
      ]);
      setLogs(logRes.rows);
      const projData = projRes.rows;
      setProjects(projData);
    } catch { setLogs([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function create() {
    if (!form.project_id || !form.hours) return;
    setSaving(true);
    try {
      await projectsApi.logTime({
        project_id: form.project_id,
        hours: parseFloat(form.hours),
        description: form.description.trim() || undefined,
        date: form.date,
      });
      setShowModal(false);
      setForm({ project_id: "", hours: "", description: "", date: new Date().toISOString().split("T")[0] });
      await load();
    } finally { setSaving(false); }
  }

  const filtered = filterProject === "all" ? logs : logs.filter((l) => l.project_id === filterProject);
  // The list endpoint returns project_id only; resolve names from the loaded projects.
  const projectName = (id: string) => projects.find((p) => p.id === id)?.name;

  // Stats
  const totalHours = logs.reduce((s, l) => s + l.hours, 0);
  const thisWeekHours = logs.filter((l) => {
    const d = new Date(`${l.date}T00:00:00`);
    const now = new Date();
    const diff = (now.getTime() - d.getTime()) / (1000 * 60 * 60 * 24);
    return diff <= 7;
  }).reduce((s, l) => s + l.hours, 0);

  // Hours by project (for the mini chart)
  const byProject: Record<string, { name: string; hours: number }> = {};
  logs.forEach((l) => {
    const key = l.project_id;
    const name = l.project?.name ?? projectName(l.project_id) ?? "Unknown";
    if (!byProject[key]) byProject[key] = { name, hours: 0 };
    byProject[key].hours += l.hours;
  });
  const projectList = Object.entries(byProject).sort((a, b) => b[1].hours - a[1].hours);
  const maxProjHours = Math.max(...projectList.map(([, v]) => v.hours), 1);

  const rows = [...filtered].sort((a, b) => b.date.localeCompare(a.date));
  const filteredHours = filtered.reduce((s, l) => s + l.hours, 0);

  const columns: ColumnDef<TimeLog>[] = [
    {
      accessorKey: "date", header: "Date", meta: { numeric: true },
      cell: (c) => new Date(c.row.original.date).toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" }),
    },
    {
      id: "project", accessorFn: (l) => l.project?.name ?? projectName(l.project_id) ?? "", header: "Project",
      cell: (c) => <span className="block max-w-56 truncate font-medium text-fg">{c.getValue<string>() || "-"}</span>,
    },
    {
      id: "work", accessorFn: (l) => l.task?.title ?? l.description ?? "", header: "Work", meta: { hideOnMobile: true },
      cell: (c) => <span className="block max-w-72 truncate text-fg-muted">{c.getValue<string>() || "-"}</span>,
    },
    {
      id: "user", accessorFn: (l) => l.user?.full_name ?? "", header: "By", meta: { hideOnMobile: true },
      cell: (c) => c.getValue<string>() || "-",
    },
    { accessorKey: "hours", header: "Hours", meta: { numeric: true }, cell: (c) => <span className="font-medium">{c.row.original.hours} h</span> },
  ];

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total hours" value={`${totalHours} h`} />
        <Stat label="This week" value={`${thisWeekHours} h`} />
        <Stat label="Total entries" value={logs.length} />
        <Stat label="Projects" value={projectList.length} />
      </StatGroup>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <Card className="self-start">
          <CardHeader title="By project" />
          <CardBody className="space-y-2.5">
            {projectList.slice(0, 6).map(([id, { name, hours }]) => (
              <div key={id} className="space-y-1">
                <div className="flex justify-between gap-2 text-xs">
                  <span className="truncate font-medium text-fg">{name}</span>
                  <span className="text-fg-muted tabular">{hours} h</span>
                </div>
                <div className="h-1.5 rounded-full bg-surface-2" aria-hidden>
                  <div className="h-1.5 rounded-full bg-tone-blue" style={{ width: `${(hours / maxProjHours) * 100}%` }} />
                </div>
              </div>
            ))}
            {projectList.length === 0 && <p className="text-xs text-fg-muted">No data yet</p>}
          </CardBody>
        </Card>

        <ListFrame className="lg:col-span-2">
          <PageHeader flush title="Time logs" summary={`${filteredHours} h in ${filtered.length} entr${filtered.length === 1 ? "y" : "ies"}`}
            actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Log time</Button>} />
          <FilterChips
            chips={projects.slice(0, 5).map((p) => ({ id: p.id, label: <span className="block max-w-[140px] truncate">{p.name}</span>, active: filterProject === p.id }))}
            onToggle={(id) => setFilterProject((cur) => (cur === id ? "all" : id))} onClear={() => setFilterProject("all")}>
            {projects.length > 5 && (
              <Select aria-label="Project" value={filterProject} onChange={(e) => setFilterProject(e.target.value)} className="w-48"
                options={[{ value: "all", label: "All projects" }, ...projects.map((p) => ({ value: p.id, label: p.name }))]} />
            )}
          </FilterChips>
          <DataTable label="Time logs" columns={columns} data={rows} getRowId={(l) => l.id} loading={loading} maxHeight="480px"
            empty={{ icon: Timer, title: "No time logged yet", action: <Button variant="primary" onClick={() => setShowModal(true)}>Log time</Button> }} />
        </ListFrame>
      </div>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Log time"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!form.project_id || !form.hours} loading={saving}>{saving ? "Saving..." : "Log time"}</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Project" required>
            <Select value={form.project_id} onChange={(e) => setForm({ ...form, project_id: e.target.value })}
              placeholder="Select a project" options={projects.map((p) => ({ value: p.id, label: p.name }))} />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Hours" required>
              <Input numeric type="number" step="0.5" min="0.5" value={form.hours} onChange={(e) => setForm({ ...form, hours: e.target.value })} placeholder="e.g. 2.5" />
            </Field>
            <Field label="Date" required><Input type="date" value={form.date} onChange={(e) => setForm({ ...form, date: e.target.value })} /></Field>
          </FieldGrid>
          <Field label="Description">
            <Textarea value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} rows={3} placeholder="What did you work on?" />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
