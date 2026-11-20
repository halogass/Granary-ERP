"use client";

import { formatCurrency } from "@/lib/currency";
import { useCurrencySettings } from "@/hooks/useCurrencySettings";
import { useEffect, useState, useCallback, useMemo } from "react";
import { useParams } from "next/navigation";
import { messagingApi, projectsApi } from "@/lib/api";
import { TASK_PRIORITY_LABELS, TASK_STATUS_LABELS } from "@/lib/projectTasks";
import { AlertCircle, CheckCircle2, Edit3, Milestone, Plus, Timer, Trash2, UserPlus, Users } from "lucide-react";
import Link from "next/link";
import AttachmentPanel from "@/components/ui/AttachmentPanel";
import KanbanBoard from "@/components/pm/KanbanBoard";
import { projectIssueAdapter } from "@/lib/issues";
import { usePermissions } from "@/hooks/usePermissions";
import { localSource, orgSource } from "@/lib/entitySources";
import {
  Button, Card, CardBody, CardHeader, Combobox, DataTable, Dialog, EmptyState, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues,
  PageHeader, Pill, Select, Skeleton, StatusPill, Tabs, TabsContent, TabsList, TabsTrigger, Textarea, statusTone, TONE_CLASSES,
  type ColumnDef,
} from "@/components/ui";
import { cn } from "@/lib/utils";

interface Task {
  id: string;
  title: string;
  status: string;
  priority: string;
  due_date?: string;
  estimated_hours?: number;
  logged_hours?: number;
  assignee?: { full_name?: string; email: string };
}

interface ProjectMilestone {
  id: string;
  name: string;
  due_date?: string;
  is_completed: boolean;
}

interface Member {
  user_id: string;
  full_name: string;
  email: string;
  role?: string | null;
  allocation_percentage?: number | null;
}

interface Person { id: string; name: string; department?: string | null; position?: string | null }

const MEMBER_ROLE_LABELS: Record<string, string> = {
  project_manager: "Project manager", tech_lead: "Tech lead", member: "Member",
};

function apiDetail(err: unknown, fallback: string) {
  const d = (err as { response?: { data?: { detail?: unknown } } })?.response?.data?.detail;
  return typeof d === "string" ? d : fallback;
}

const EMPTY_EDIT = {
  name: "", description: "", status: "planning", customer_org_id: "", pm_id: "", start_date: "", end_date: "", budget: "",
};

interface Project {
  currency?: string;
  id: string;
  code: string;
  name: string;
  status: string;
  start_date?: string;
  end_date?: string;
  budget?: number;
  description?: string;
  customer_org_id?: string | null;
  customer_org?: { id?: string; name: string } | null;
  pm_id?: string | null;
  pm_user?: { id?: string; full_name?: string; email: string } | null;
  members?: Member[];
  tasks?: Task[];
  milestones?: ProjectMilestone[];
}

const STATUS_LABELS: Record<string, string> = {
  planning: "Planning", active: "Active", on_hold: "On hold", completed: "Completed", cancelled: "Cancelled",
};

const PRIORITY_LABELS = TASK_PRIORITY_LABELS;

const fmt = (n: number, currency: string) => formatCurrency(n, currency);

function Bar({ pct, tone = "blue" }: { pct: number; tone?: "blue" | "red" }) {
  return (
    <div className="h-1.5 w-full overflow-hidden rounded-full bg-surface-2" aria-hidden>
      <div className={cn("h-full rounded-full", tone === "red" ? "bg-tone-red" : "bg-tone-blue")} style={{ width: `${pct}%` }} />
    </div>
  );
}

export default function ProjectDetailPage() {
  const { id } = useParams<{ id: string }>();
  const { functional } = useCurrencySettings();
  const [project, setProject] = useState<Project | null>(null);
  const [milestones, setMilestones] = useState<ProjectMilestone[]>([]);
  const [loading, setLoading] = useState(true);
  const [activeTab, setActiveTab] = useState<"board" | "tasks" | "milestones" | "team" | "files">("board");
  const [showTaskModal, setShowTaskModal] = useState(false);
  const [showMilestoneModal, setShowMilestoneModal] = useState(false);
  const [showTimeModal, setShowTimeModal] = useState(false);
  const [taskForm, setTaskForm] = useState({ title: "", priority: "medium", due_date: "", estimated_hours: "" });
  const [milestoneForm, setMilestoneForm] = useState({ name: "", due_date: "" });
  const [timeForm, setTimeForm] = useState({ task_id: "", hours: "", date: new Date().toISOString().slice(0, 10), notes: "" });
  const [saving, setSaving] = useState(false);
  const [taskFilter, setTaskFilter] = useState("");
  // Edit project + team (projects:edit, as the API checks it)
  const { holds } = usePermissions();
  const canEdit = holds("projects:edit");
  const [showEdit, setShowEdit] = useState(false);
  const [editForm, setEditForm] = useState(EMPTY_EDIT);
  const [editError, setEditError] = useState("");
  const [people, setPeople] = useState<Person[]>([]);
  const [showAddMember, setShowAddMember] = useState(false);
  const [memberForm, setMemberForm] = useState({ user_id: "", role: "member", allocation: "" });
  const [memberError, setMemberError] = useState("");
  const [editMember, setEditMember] = useState<Member | null>(null);
  const [editMemberForm, setEditMemberForm] = useState({ role: "member", allocation: "" });
  const [editMemberError, setEditMemberError] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [projRes, msRes] = await Promise.all([
        projectsApi.get(id),
        projectsApi.listMilestones(id),
      ]);
      setProject(projRes.data);
      setMilestones(msRes.data ?? []);
    } catch {
      setProject(null);
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => { load(); }, [load]);

  // Staff to pick the project manager and team from (the messaging directory: active internal users).
  useEffect(() => {
    if (!canEdit) return;
    messagingApi.people().then((r) => setPeople(r.data ?? [])).catch(() => setPeople([]));
  }, [canEdit]);

  const peopleSource = useMemo(() => localSource(people.map((p) => ({
    id: p.id, label: p.name, sublabel: [p.position, p.department].filter(Boolean).join(" · ") || undefined,
  }))), [people]);

  function openEdit() {
    if (!project) return;
    setEditForm({
      name: project.name, description: project.description ?? "", status: project.status,
      customer_org_id: project.customer_org_id ?? project.customer_org?.id ?? "",
      pm_id: project.pm_id ?? project.pm_user?.id ?? "",
      start_date: project.start_date ?? "", end_date: project.end_date ?? "",
      budget: project.budget != null ? String(project.budget) : "",
    });
    setEditError("");
    setShowEdit(true);
  }

  async function saveProject() {
    if (!editForm.name.trim()) { setEditError("The project needs a name"); return; }
    setSaving(true); setEditError("");
    try {
      await projectsApi.update(id, {
        name: editForm.name.trim(),
        description: editForm.description,
        status: editForm.status,
        customer_org_id: editForm.customer_org_id || null,
        pm_id: editForm.pm_id || null,
        start_date: editForm.start_date,
        end_date: editForm.end_date,
        budget: editForm.budget,
      });
      setShowEdit(false);
      await load();
    } catch (err) {
      setEditError(apiDetail(err, "Could not save the project"));
    } finally { setSaving(false); }
  }

  async function addMember() {
    if (!memberForm.user_id) return;
    setSaving(true); setMemberError("");
    try {
      await projectsApi.addMember(id, {
        user_id: memberForm.user_id,
        role: memberForm.role,
        allocation_percentage: memberForm.allocation ? parseInt(memberForm.allocation, 10) : null,
      });
      setShowAddMember(false);
      setMemberForm({ user_id: "", role: "member", allocation: "" });
      await load();
    } catch (err) {
      setMemberError(apiDetail(err, "Could not add this person"));
    } finally { setSaving(false); }
  }

  function openEditMember(m: Member) {
    setEditMember(m);
    setEditMemberForm({
      role: m.role ?? "member",
      allocation: m.allocation_percentage != null ? String(m.allocation_percentage) : "",
    });
    setEditMemberError("");
  }

  async function saveMember() {
    if (!editMember || !project) return;
    const allocation = editMemberForm.allocation.trim();
    const pct = allocation ? Number(allocation) : null;
    if (pct != null && (!Number.isInteger(pct) || pct < 0 || pct > 100)) {
      setEditMemberError("Allocation is a whole number from 0 to 100");
      return;
    }
    setSaving(true); setEditMemberError("");
    try {
      const isPm = editMember.user_id === project.pm_id;
      await projectsApi.updateMember(id, editMember.user_id, isPm
        ? { allocation_percentage: pct }
        : { role: editMemberForm.role, allocation_percentage: pct });
      setEditMember(null);
      await load();
    } catch (err) {
      setEditMemberError(apiDetail(err, "Could not save this team member"));
    } finally { setSaving(false); }
  }

  async function removeMember(m: Member) {
    if (!confirm(`Take ${m.full_name} off this project?`)) return;
    try {
      await projectsApi.removeMember(id, m.user_id);
      await load();
    } catch (err) {
      alert(apiDetail(err, "Could not remove this person"));
    }
  }

  async function createTask() {
    if (!taskForm.title) return;
    setSaving(true);
    try {
      await projectsApi.createTask({
        project_id: id,
        title: taskForm.title,
        priority: taskForm.priority,
        due_date: taskForm.due_date || undefined,
        estimated_hours: taskForm.estimated_hours ? parseFloat(taskForm.estimated_hours) : undefined,
      });
      setShowTaskModal(false);
      setTaskForm({ title: "", priority: "medium", due_date: "", estimated_hours: "" });
      await load();
    } finally { setSaving(false); }
  }

  async function createMilestone() {
    if (!milestoneForm.name || !milestoneForm.due_date) return;
    setSaving(true);
    try {
      await projectsApi.createMilestone({
        project_id: id,
        name: milestoneForm.name,
        due_date: milestoneForm.due_date,
      });
      setShowMilestoneModal(false);
      setMilestoneForm({ name: "", due_date: "" });
      await load();
    } finally { setSaving(false); }
  }

  async function logTime() {
    if (!timeForm.hours) return;
    setSaving(true);
    try {
      await projectsApi.logTime({
        project_id: id,
        task_id: timeForm.task_id || undefined,
        hours: parseFloat(timeForm.hours),
        date: timeForm.date,
        description: timeForm.notes || undefined,
      });
      setShowTimeModal(false);
      setTimeForm({ task_id: "", hours: "", date: new Date().toISOString().slice(0, 10), notes: "" });
      await load();
    } finally { setSaving(false); }
  }

  async function updateTaskStatus(taskId: string, status: string) {
    await projectsApi.updateTask(taskId, { status });
    await load();
  }

  const tasksAll = project?.tasks ?? [];
  const today = new Date();

  const taskColumns = useMemo<ColumnDef<Task>[]>(() => [
    {
      accessorKey: "status", header: "Status",
      cell: (c) => {
        const task = c.row.original;
        return (
          <select value={task.status} onChange={(e) => updateTaskStatus(task.id, e.target.value)} aria-label={`Status of ${task.title}`}
            className={cn("h-[22px] cursor-pointer appearance-none rounded-full border-0 px-2.5 text-xs font-medium focus:outline-none focus:ring-2 focus:ring-brand/30",
              TONE_CLASSES[statusTone("task", task.status)])}>
            {Object.entries(TASK_STATUS_LABELS).map(([v, l]) => <option key={v} value={v}>{l}</option>)}
          </select>
        );
      },
    },
    {
      accessorKey: "title", header: "Task",
      cell: (c) => (
        <span className={cn("font-medium", c.row.original.status === "done" ? "text-fg-subtle line-through" : "text-fg")}>{c.row.original.title}</span>
      ),
    },
    {
      accessorKey: "priority", header: "Priority",
      cell: (c) => <StatusPill family="priority" status={c.row.original.priority} label={PRIORITY_LABELS[c.row.original.priority]} />,
    },
    {
      accessorKey: "due_date", header: "Due", meta: { numeric: true },
      cell: (c) => {
        const t = c.row.original;
        if (!t.due_date) return <span className="text-fg-subtle">-</span>;
        const late = t.status !== "done" && new Date(t.due_date) < today;
        return <span className={late ? "font-semibold text-tone-red" : ""}>{new Date(t.due_date).toLocaleDateString("en-GB", { day: "numeric", month: "short" })}</span>;
      },
    },
    {
      accessorKey: "estimated_hours", header: "Estimate", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => (c.row.original.estimated_hours ? `${c.row.original.estimated_hours} h` : "-"),
    },
    {
      id: "assignee", accessorFn: (t) => t.assignee?.full_name ?? t.assignee?.email ?? "", header: "Assignee", meta: { hideOnMobile: true },
      cell: (c) => <span className="block max-w-36 truncate">{c.getValue<string>() || "-"}</span>,
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [tasksAll]);

  const milestoneColumns = useMemo<ColumnDef<ProjectMilestone>[]>(() => [
    {
      accessorKey: "name", header: "Milestone",
      cell: (c) => (
        <span className="flex items-center gap-2">
          <Milestone className={cn("size-4 shrink-0", c.row.original.is_completed ? "text-tone-blue" : "text-fg-subtle")} aria-hidden />
          <span className={cn("font-medium", c.row.original.is_completed ? "text-fg-subtle line-through" : "text-fg")}>{c.row.original.name}</span>
        </span>
      ),
    },
    {
      accessorKey: "due_date", header: "Due", meta: { numeric: true },
      cell: (c) => (c.row.original.due_date
        ? new Date(c.row.original.due_date).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-"),
    },
    {
      id: "state", header: "State", enableSorting: false,
      cell: (c) => {
        const ms = c.row.original;
        if (ms.is_completed) return <Pill tone="blue" icon={CheckCircle2}>Done</Pill>;
        if (ms.due_date && new Date(ms.due_date) < today) return <Pill tone="red">Overdue</Pill>;
        return <Pill>Open</Pill>;
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], []);

  if (loading) return (
    <div className="space-y-4" aria-label="Loading project">
      <Skeleton className="h-8 w-72" /><Skeleton className="h-32 w-full" /><Skeleton className="h-64 w-full" />
    </div>
  );

  if (!project) return (
    <EmptyState icon={AlertCircle} title="Project not found."
      action={<Button asChild><Link href="/projects">Back to projects</Link></Button>} />
  );

  const tasks = project.tasks ?? [];
  const filteredTasks = taskFilter ? tasks.filter((t) => t.status === taskFilter) : tasks;
  const doneTasks = tasks.filter((t) => t.status === "done").length;
  const taskProgress = tasks.length > 0 ? Math.round((doneTasks / tasks.length) * 100) : 0;

  const end = project.end_date ? new Date(project.end_date) : null;
  const start = project.start_date ? new Date(project.start_date) : null;
  const isOverdue = end && project.status === "active" && end < today;
  let timelineProgress = 0;
  if (start && end) {
    const total = end.getTime() - start.getTime();
    const elapsed = today.getTime() - start.getTime();
    timelineProgress = Math.min(100, Math.max(0, Math.round((elapsed / total) * 100)));
  }

  return (
    <div>
      <PageHeader title={project.name} back={{ href: "/projects", label: "Projects" }}
        status={<span className="flex items-center gap-1.5">
          <StatusPill family="project" status={project.status} label={STATUS_LABELS[project.status]} />
          {isOverdue && <Pill tone="red">Overdue</Pill>}
        </span>}
        summary={<span className="font-mono">{project.code}</span>}
        actions={<>
          {canEdit && <Button onClick={openEdit}><Edit3 aria-hidden /> Edit project</Button>}
          <Button onClick={() => setShowTimeModal(true)}><Timer aria-hidden /> Log time</Button>
          <Button variant="primary" onClick={() => setShowTaskModal(true)}><Plus aria-hidden /> Add task</Button>
        </>} />

      <Card className="mb-4">
        <CardBody className="space-y-4">
          {project.description && <p className="max-w-2xl text-sm text-fg-muted">{project.description}</p>}
          <div className="grid grid-cols-1 gap-5 md:grid-cols-4">
            {start && end && (
              <div className="md:col-span-2">
                <div className="mb-1.5 flex justify-between text-xs text-fg-muted tabular">
                  <span>{start.toLocaleDateString("en-GB", { day: "numeric", month: "short" })}</span>
                  <span className={isOverdue ? "font-semibold text-tone-red" : ""}>
                    {end.toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" })}
                  </span>
                </div>
                <Bar pct={timelineProgress} tone={isOverdue ? "red" : "blue"} />
                <p className="mt-1 text-xs text-fg-muted tabular">{timelineProgress}% of the timeline elapsed</p>
              </div>
            )}
            <div>
              <div className="mb-1.5 flex justify-between text-xs text-fg-muted tabular">
                <span>Tasks complete</span>
                <span>{doneTasks}/{tasks.length}</span>
              </div>
              <Bar pct={taskProgress} />
              <p className="mt-1 text-xs text-fg-muted tabular">{taskProgress}% complete</p>
            </div>
            <KeyValues cols={1} items={[
              { label: "Project manager", value: project.pm_user ? (project.pm_user.full_name ?? project.pm_user.email) : "-" },
              { label: "Customer", value: project.customer_org?.name ?? "-" },
              ...(project.budget ? [{ label: "Budget", value: fmt(project.budget, project.currency || functional), numeric: true }] : []),
            ]} />
          </div>
        </CardBody>
      </Card>

      <Tabs value={activeTab} onValueChange={(v) => setActiveTab(v as typeof activeTab)}>
        <TabsList>
          <TabsTrigger value="board">Board</TabsTrigger>
          <TabsTrigger value="tasks">Tasks <span className="text-fg-muted tabular">{tasks.length}</span></TabsTrigger>
          <TabsTrigger value="milestones">Milestones <span className="text-fg-muted tabular">{milestones.length}</span></TabsTrigger>
          <TabsTrigger value="team">Team <span className="text-fg-muted tabular">{(project.members ?? []).length}</span></TabsTrigger>
          <TabsTrigger value="files">Files</TabsTrigger>
        </TabsList>

        <TabsContent value="board">
          <KanbanBoard adapter={projectIssueAdapter} scopeId={id} />
        </TabsContent>

        <TabsContent value="tasks">
          <Card className="overflow-hidden">
            <FilterChips
              chips={Object.entries(TASK_STATUS_LABELS).map(([k, l]) => ({ id: k, label: l, active: taskFilter === k, count: tasks.filter((t) => t.status === k).length }))}
              onToggle={(k) => setTaskFilter(taskFilter === k ? "" : k)} onClear={() => setTaskFilter("")}>
              <Button size="sm" onClick={() => setShowTaskModal(true)}><Plus aria-hidden /> Add task</Button>
            </FilterChips>
            <DataTable label="Tasks" columns={taskColumns} data={filteredTasks} getRowId={(t) => t.id} maxHeight="none"
              empty={taskFilter
                ? { title: "No tasks in this status" }
                : { title: "No tasks yet", action: <Button variant="primary" onClick={() => setShowTaskModal(true)}><Plus aria-hidden /> Create first task</Button> }} />
          </Card>
        </TabsContent>

        <TabsContent value="milestones">
          <Card className="overflow-hidden">
            <CardHeader title="Milestones" actions={<Button size="sm" onClick={() => setShowMilestoneModal(true)}><Plus aria-hidden /> Add milestone</Button>} />
            <DataTable label="Milestones" columns={milestoneColumns} data={milestones} getRowId={(m) => m.id} maxHeight="none"
              empty={{ title: "No milestones defined", action: <Button variant="primary" onClick={() => setShowMilestoneModal(true)}><Plus aria-hidden /> Add first milestone</Button> }} />
          </Card>
        </TabsContent>

        <TabsContent value="team">
          <Card className="overflow-hidden">
            <CardHeader title="Team" actions={canEdit
              ? <Button size="sm" onClick={() => { setMemberError(""); setShowAddMember(true); }}><UserPlus aria-hidden /> Add member</Button>
              : undefined} />
            {(project.members ?? []).length === 0 ? (
              <EmptyState icon={Users} title="No one is on this project yet" />
            ) : (
              <ul className="divide-y divide-line">
                {(project.members ?? []).map((m) => (
                  <li key={m.user_id} className="flex items-center gap-3 px-5 py-2.5">
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-sm font-medium text-fg">{m.full_name}</p>
                      <p className="truncate text-xs text-fg-muted">{m.email}</p>
                    </div>
                    {m.user_id === project.pm_id
                      ? <Pill tone="violet">Project manager</Pill>
                      : m.role ? <Pill>{MEMBER_ROLE_LABELS[m.role] ?? m.role}</Pill> : null}
                    {m.allocation_percentage != null && <span className="text-xs text-fg-muted tabular">{m.allocation_percentage}%</span>}
                    {canEdit && (
                      <IconButton label={`Edit ${m.full_name}`} icon={Edit3} size="sm"
                        onClick={() => openEditMember(m)} />
                    )}
                    {canEdit && m.user_id !== project.pm_id && (
                      <IconButton label={`Remove ${m.full_name}`} icon={Trash2} size="sm" className="text-tone-red"
                        onClick={() => removeMember(m)} />
                    )}
                  </li>
                ))}
              </ul>
            )}
          </Card>
        </TabsContent>

        <TabsContent value="files">
          <Card><CardBody><AttachmentPanel entityType="project" entityId={project.id} /></CardBody></Card>
        </TabsContent>
      </Tabs>

      <Dialog open={showTaskModal} onOpenChange={setShowTaskModal} title="Add task"
        footer={<>
          <Button onClick={() => setShowTaskModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={createTask} disabled={!taskForm.title} loading={saving}>{saving ? "Creating..." : "Add task"}</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Title" required>
            <Input value={taskForm.title} onChange={(e) => setTaskForm({ ...taskForm, title: e.target.value })} placeholder="e.g. Implement authentication module" />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Priority">
              <Select value={taskForm.priority} onChange={(e) => setTaskForm({ ...taskForm, priority: e.target.value })}
                options={Object.entries(PRIORITY_LABELS).map(([v, l]) => ({ value: v, label: l }))} />
            </Field>
            <Field label="Due date"><Input type="date" value={taskForm.due_date} onChange={(e) => setTaskForm({ ...taskForm, due_date: e.target.value })} /></Field>
          </FieldGrid>
          <Field label="Estimated hours">
            <Input numeric type="number" value={taskForm.estimated_hours} onChange={(e) => setTaskForm({ ...taskForm, estimated_hours: e.target.value })} placeholder="e.g. 8" />
          </Field>
        </div>
      </Dialog>

      <Dialog open={showMilestoneModal} onOpenChange={setShowMilestoneModal} title="Add milestone"
        footer={<>
          <Button onClick={() => setShowMilestoneModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={createMilestone} disabled={!milestoneForm.name || !milestoneForm.due_date} loading={saving}>
            {saving ? "Creating..." : "Add milestone"}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Name" required>
            <Input value={milestoneForm.name} onChange={(e) => setMilestoneForm({ ...milestoneForm, name: e.target.value })} placeholder="e.g. Phase 1 delivery" />
          </Field>
          <Field label="Due date" required>
            <Input type="date" value={milestoneForm.due_date} onChange={(e) => setMilestoneForm({ ...milestoneForm, due_date: e.target.value })} />
          </Field>
        </div>
      </Dialog>

      <Dialog open={showTimeModal} onOpenChange={setShowTimeModal} title="Log time"
        footer={<>
          <Button onClick={() => setShowTimeModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={logTime} disabled={!timeForm.hours} loading={saving}>{saving ? "Saving..." : "Log time"}</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Task (optional)">
            <Select value={timeForm.task_id} onChange={(e) => setTimeForm({ ...timeForm, task_id: e.target.value })}
              options={[{ value: "", label: "Project level (no specific task)" },
                ...tasks.filter((t) => t.status !== "done" && t.status !== "cancelled").map((t) => ({ value: t.id, label: t.title }))]} />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Hours" required>
              <Input numeric type="number" value={timeForm.hours} onChange={(e) => setTimeForm({ ...timeForm, hours: e.target.value })} placeholder="1.5" />
            </Field>
            <Field label="Date"><Input type="date" value={timeForm.date} onChange={(e) => setTimeForm({ ...timeForm, date: e.target.value })} /></Field>
          </FieldGrid>
          <Field label="Notes">
            <Textarea value={timeForm.notes} onChange={(e) => setTimeForm({ ...timeForm, notes: e.target.value })} rows={2} placeholder="What did you work on?" />
          </Field>
        </div>
      </Dialog>
      <Dialog open={showEdit} onOpenChange={setShowEdit} title="Edit project" size="lg"
        footer={<>
          <Button onClick={() => setShowEdit(false)}>Cancel</Button>
          <Button variant="primary" onClick={saveProject} loading={saving}>Save changes</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Name" required>
            <Input value={editForm.name} onChange={(e) => setEditForm({ ...editForm, name: e.target.value })} />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Status">
              <Select value={editForm.status} onChange={(e) => setEditForm({ ...editForm, status: e.target.value })}
                options={Object.entries(STATUS_LABELS).map(([v, l]) => ({ value: v, label: l }))} />
            </Field>
            <Field label="Project manager">
              <Combobox value={editForm.pm_id} source={peopleSource} allowClear
                placeholder="No project manager" emptyLabel="No staff found"
                onChange={(pm) => setEditForm({ ...editForm, pm_id: pm })} />
            </Field>
            <Field label="Customer" className="md:col-span-2">
              <Combobox value={editForm.customer_org_id} source={orgSource} allowClear
                placeholder="No customer" emptyLabel="No organizations found"
                onChange={(org) => setEditForm({ ...editForm, customer_org_id: org })} />
            </Field>
            <Field label="Start date">
              <Input type="date" value={editForm.start_date} onChange={(e) => setEditForm({ ...editForm, start_date: e.target.value })} />
            </Field>
            <Field label="End date">
              <Input type="date" value={editForm.end_date} onChange={(e) => setEditForm({ ...editForm, end_date: e.target.value })} />
            </Field>
            <Field label={`Budget (${project.currency || functional})`}>
              <Input numeric type="number" min={0} value={editForm.budget} onChange={(e) => setEditForm({ ...editForm, budget: e.target.value })} />
            </Field>
          </FieldGrid>
          <Field label="Description">
            <Textarea rows={3} value={editForm.description} onChange={(e) => setEditForm({ ...editForm, description: e.target.value })} />
          </Field>
          {editError && <p role="alert" className="rounded-lg bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{editError}</p>}
        </div>
      </Dialog>

      <Dialog open={showAddMember} onOpenChange={setShowAddMember} title="Add team member"
        footer={<>
          <Button onClick={() => setShowAddMember(false)}>Cancel</Button>
          <Button variant="primary" onClick={addMember} disabled={!memberForm.user_id} loading={saving}>Add member</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Person" required>
            <Combobox value={memberForm.user_id} source={peopleSource}
              excludeIds={(project.members ?? []).map((m) => m.user_id)}
              placeholder="Choose a staff member" emptyLabel="No staff found"
              onChange={(uid) => setMemberForm({ ...memberForm, user_id: uid })} />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Role">
              <Select value={memberForm.role} onChange={(e) => setMemberForm({ ...memberForm, role: e.target.value })}
                options={Object.entries(MEMBER_ROLE_LABELS).filter(([v]) => v !== "project_manager").map(([v, l]) => ({ value: v, label: l }))} />
            </Field>
            <Field label="Allocation (%)">
              <Input numeric type="number" min={0} max={100} value={memberForm.allocation}
                onChange={(e) => setMemberForm({ ...memberForm, allocation: e.target.value })} placeholder="e.g. 50" />
            </Field>
          </FieldGrid>
          {memberError && <p role="alert" className="rounded-lg bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{memberError}</p>}
        </div>
      </Dialog>

      <Dialog open={editMember != null} onOpenChange={(o) => { if (!o) setEditMember(null); }}
        title={editMember ? `Edit ${editMember.full_name}` : "Edit team member"}
        footer={<>
          <Button onClick={() => setEditMember(null)}>Cancel</Button>
          <Button variant="primary" onClick={saveMember} loading={saving}>Save changes</Button>
        </>}>
        <div className="space-y-4">
          <FieldGrid cols={2}>
            <Field label="Role" hint={editMember?.user_id === project.pm_id ? "Follows the project manager field" : undefined}>
              {editMember?.user_id === project.pm_id
                ? <Input value={MEMBER_ROLE_LABELS.project_manager} disabled readOnly />
                : <Select value={editMemberForm.role} onChange={(e) => setEditMemberForm({ ...editMemberForm, role: e.target.value })}
                    options={[
                      ...Object.entries(MEMBER_ROLE_LABELS).filter(([v]) => v !== "project_manager").map(([v, l]) => ({ value: v, label: l })),
                      ...(editMemberForm.role && !(editMemberForm.role in MEMBER_ROLE_LABELS)
                        ? [{ value: editMemberForm.role, label: editMemberForm.role }] : []),
                    ]} />}
            </Field>
            <Field label="Allocation (%)">
              <Input numeric type="number" min={0} max={100} value={editMemberForm.allocation}
                onChange={(e) => setEditMemberForm({ ...editMemberForm, allocation: e.target.value })} placeholder="Leave empty to clear" />
            </Field>
          </FieldGrid>
          {editMemberError && <p role="alert" className="rounded-lg bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{editMemberError}</p>}
        </div>
      </Dialog>
    </div>
  );
}
