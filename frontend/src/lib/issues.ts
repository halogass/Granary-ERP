// Shared "issue" abstraction so one Kanban board + detail drawer can drive both the
// Project module (ProjectTask) and the R&D module (RDBacklogItem).
import { projectsApi, rndApi } from "@/lib/api";

export interface Issue {
  id: string;
  title: string;
  description?: string | null;
  status: string;
  priority?: string | null;
  assigneeId?: string | null;
  labels: string[];
  storyPoints?: number | null;
  estimate?: number | null;     // hours (Project module)
  dueDate?: string | null;
  epicId?: string | null;
  rank: number;
  raw: Record<string, unknown>;
}

export interface StatusColumn { key: string; label: string; accent: string; }

export interface Member { id: string; name: string; avatar_url?: string | null }

export interface Epic { id: string; title: string; color: string; status: string }

export interface IssueAdapter {
  entityType: "project_task" | "rnd_backlog_item";
  scopeType: "project" | "rnd_project";
  label: string;                 // singular noun, e.g. "Task" / "Item"
  statuses: StatusColumn[];
  usesStoryPoints: boolean;
  rankField: string;             // "sort_order" | "board_rank"
  assigneeField: string;         // "assigned_to_id" | "assignee_id"
  labelsField: string;           // "tags" | "labels"
  list: (scopeId: string) => Promise<Issue[]>;
  update: (id: string, patch: Record<string, unknown>) => Promise<void>;
  create: (scopeId: string, data: { title: string; status: string }) => Promise<Issue | null>;
  remove: (id: string) => Promise<void>;
}

export const PRIORITY_META: Record<string, { label: string }> = {
  critical: { label: "Critical" },
  urgent:   { label: "Urgent" },
  high:     { label: "High" },
  medium:   { label: "Medium" },
  low:      { label: "Low" },
};

// ─── Project tasks ─────────────────────────────────────────────────────────────
const PROJECT_STATUSES: StatusColumn[] = [
  { key: "todo",        label: "To Do",       accent: "#94a3b8" },
  { key: "in_progress", label: "In Progress", accent: "#3b82f6" },
  { key: "review",      label: "In Review",   accent: "#f97316" },
  { key: "done",        label: "Done",        accent: "#22c55e" },
];

function normalizeTask(raw: Record<string, any>): Issue {
  return {
    id: String(raw.id),
    title: raw.title ?? "",
    description: raw.description ?? null,
    status: raw.status ?? "todo",
    priority: raw.priority ?? null,
    assigneeId: raw.assigned_to_id ?? null,
    labels: Array.isArray(raw.tags) ? raw.tags : [],
    estimate: raw.estimated_hours ?? null,
    storyPoints: null,
    dueDate: raw.due_date ?? null,
    epicId: raw.epic_id ?? null,
    rank: raw.sort_order ?? 0,
    raw,
  };
}

export const projectIssueAdapter: IssueAdapter = {
  entityType: "project_task",
  scopeType: "project",
  label: "Task",
  statuses: PROJECT_STATUSES,
  usesStoryPoints: false,
  rankField: "sort_order",
  assigneeField: "assigned_to_id",
  labelsField: "tags",
  list: async (projectId) => {
    const res = await projectsApi.get(projectId);
    const tasks = (res.data?.tasks ?? []) as Record<string, any>[];
    return tasks.map(normalizeTask);
  },
  update: async (id, patch) => { await projectsApi.updateTask(id, patch); },
  create: async (projectId, data) => {
    const res = await projectsApi.createTask({ project_id: projectId, title: data.title, status: data.status });
    return res.data?.id ? normalizeTask({ ...res.data, status: data.status }) : null;
  },
  remove: async (id) => { await projectsApi.deleteTask(id); },
};

// ─── R&D backlog items ─────────────────────────────────────────────────────────
const RND_STATUSES: StatusColumn[] = [
  { key: "backlog",     label: "Backlog",     accent: "#94a3b8" },
  { key: "refined",     label: "Refined",     accent: "#a855f7" },
  { key: "in_sprint",   label: "In Sprint",   accent: "#6366f1" },
  { key: "in_progress", label: "In Progress", accent: "#3b82f6" },
  { key: "review",      label: "Review",      accent: "#f97316" },
  { key: "done",        label: "Done",        accent: "#22c55e" },
];

function normalizeBacklog(raw: Record<string, any>): Issue {
  return {
    id: String(raw.id),
    title: raw.title ?? "",
    description: raw.description ?? null,
    status: raw.status ?? "backlog",
    priority: raw.priority ?? null,
    assigneeId: raw.assignee_id ?? null,
    labels: Array.isArray(raw.labels) ? raw.labels : [],
    storyPoints: raw.story_points ?? null,
    estimate: null,
    dueDate: null,
    epicId: raw.epic_id ?? null,
    rank: raw.board_rank ?? 0,
    raw,
  };
}

export const rndIssueAdapter: IssueAdapter = {
  entityType: "rnd_backlog_item",
  scopeType: "rnd_project",
  label: "Item",
  statuses: RND_STATUSES,
  usesStoryPoints: true,
  rankField: "board_rank",
  assigneeField: "assignee_id",
  labelsField: "labels",
  list: async (projectId) => {
    const res = await rndApi.listBacklog({ project_id: projectId, limit: 200 });
    const items = (Array.isArray(res.data) ? res.data : res.data?.items ?? []) as Record<string, any>[];
    return items.map(normalizeBacklog);
  },
  update: async (id, patch) => { await rndApi.updateBacklogItem(id, patch); },
  create: async (projectId, data) => {
    const res = await rndApi.createBacklogItem({ rnd_project_id: projectId, title: data.title, status: data.status });
    return res.data?.id ? normalizeBacklog(res.data) : null;
  },
  remove: async (id) => { await rndApi.deleteBacklogItem(id); },
};
