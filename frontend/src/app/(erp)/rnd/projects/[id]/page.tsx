"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState, useCallback } from "react";
import { useParams } from "next/navigation";
import { rndApi } from "@/lib/api";
import { Plus, GitBranch, Tag, CheckCircle2, PlayCircle, Flag, Rocket, Target, Loader2 } from "lucide-react";
import Link from "next/link";
import AttachmentPanel from "@/components/ui/AttachmentPanel";
import KanbanBoard from "@/components/pm/KanbanBoard";
import { rndIssueAdapter } from "@/lib/issues";
import {
  Button, Card, CardBody, CardHeader, DataTable, Dialog, EmptyState, Field, FieldGrid, Input, PageHeader, Pill, Select,
  StatusPill, Tabs, TabsContent, TabsList, TabsTrigger, Textarea, type ColumnDef, type Tone,
} from "@/components/ui";

interface Sprint {
  id: string;
  name: string;
  goal?: string;
  status: string;
  start_date?: string;
  end_date?: string;
  velocity_planned?: number;
  velocity_achieved?: number;
  retrospective_notes?: string;
}

interface BacklogItem {
  id: string;
  title: string;
  description?: string;
  item_type: string;
  status: string;
  priority: string;
  story_points?: number;
  sprint_id?: string;
  labels?: string[];
  gitlab_issue_url?: string;
}

interface Release {
  id: string;
  version: string;
  release_name?: string;
  status: string;
  release_date?: string;
  release_notes?: string;
  changelog?: { type: string; message: string }[];
  gitlab_tag_url?: string;
  gitlab_tag_name?: string;
}

interface RDProject {
  id: string;
  code: string;
  name: string;
  description?: string;
  status: string;
  product_ref?: string;
  latest_version?: string;
  roadmap_notes?: string;
  sprints?: Sprint[];
  backlog_counts?: Record<string, number>;
  latest_release?: Release;
  gitlab_repos?: { id: string; name: string; web_url?: string; last_pipeline_status?: string; full_path: string }[];
}

const PRIORITY_META: Record<string, { label: string; tone: Tone }> = {
  critical: { label: "Critical", tone: "red" },
  high:     { label: "High",     tone: "orange" },
  medium:   { label: "Medium",   tone: "neutral" },
  low:      { label: "Low",      tone: "neutral" },
};

const TYPE_META: Record<string, { label: string }> = {
  feature:       { label: "Feature" },
  bug:           { label: "Bug" },
  tech_debt:     { label: "Tech debt" },
  research:      { label: "Research" },
  spike:         { label: "Spike" },
  security:      { label: "Security" },
  documentation: { label: "Docs" },
  internal_tool: { label: "Internal tool" },
};

const STATUS_LABELS: Record<string, string> = {
  backlog: "Backlog", refined: "Refined", in_sprint: "In sprint",
  in_progress: "In progress", review: "Review", done: "Done",
};

const PIPELINE_STATUS: Record<string, { tone: Tone; label: string }> = {
  passed:  { tone: "blue",   label: "Passed" },
  failed:  { tone: "red",    label: "Failed" },
  running: { tone: "violet", label: "Running" },
  pending: { tone: "orange", label: "Pending" },
};

const CHANGE_TONES: Record<string, Tone> = { feat: "blue", fix: "orange", breaking: "red", security: "red" };

const shortDate = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short" });

type TabKey = "board" | "sprints" | "releases" | "roadmap" | "files";

export default function RNDProjectDetailPage() {
  const { id } = useParams<{ id: string }>();
  const [project, setProject] = useState<RDProject | null>(null);
  const [backlog, setBacklog] = useState<BacklogItem[]>([]);
  const [releases, setReleases] = useState<Release[]>([]);
  const [loading, setLoading] = useState(true);
  const [tab, setTab] = useState<TabKey>("board");

  // Modals
  const [showSprintModal, setShowSprintModal] = useState(false);
  const [showItemModal, setShowItemModal] = useState(false);
  const [boardReload, setBoardReload] = useState(0);
  const [showReleaseModal, setShowReleaseModal] = useState(false);
  const [showCompleteModal, setShowCompleteModal] = useState<string | null>(null);
  const [sprintForm, setSprintForm] = useState({ name: "", goal: "", start_date: "", end_date: "", velocity_planned: "" });
  const [itemForm, setItemForm] = useState({ title: "", item_type: "feature", priority: "medium", story_points: "", sprint_id: "", labels: "", description: "" });
  const [releaseForm, setReleaseForm] = useState({ version: "", release_name: "", release_notes: "", release_date: "" });
  const [retroForm, setRetroForm] = useState({ velocity_achieved: "", retrospective_notes: "" });
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [projRes, backlogRes, releasesRes] = await Promise.all([
        rndApi.getProject(id),
        fetchAllPages<BacklogItem>(rndApi.listBacklog, { project_id: id }),
        rndApi.listReleases(id),
      ]);
      setProject(projRes.data);
      setBacklog(backlogRes.rows);
      setReleases(releasesRes.data ?? []);
    } catch { /* handled below */ } finally { setLoading(false); }
  }, [id]);

  useEffect(() => { load(); }, [load]);

  async function createSprint() {
    if (!sprintForm.name.trim() || !project) return;
    setSaving(true);
    try {
      await rndApi.createSprint(project.id, {
        name: sprintForm.name.trim(),
        goal: sprintForm.goal || undefined,
        start_date: sprintForm.start_date || undefined,
        end_date: sprintForm.end_date || undefined,
        velocity_planned: sprintForm.velocity_planned ? parseInt(sprintForm.velocity_planned) : undefined,
      });
      setShowSprintModal(false);
      setSprintForm({ name: "", goal: "", start_date: "", end_date: "", velocity_planned: "" });
      await load();
    } finally { setSaving(false); }
  }

  async function createItem() {
    if (!itemForm.title.trim() || !project) return;
    setSaving(true);
    try {
      await rndApi.createBacklogItem({
        rnd_project_id: project.id,
        title: itemForm.title.trim(),
        item_type: itemForm.item_type,
        priority: itemForm.priority,
        story_points: itemForm.story_points ? parseInt(itemForm.story_points) : undefined,
        sprint_id: itemForm.sprint_id || undefined,
        description: itemForm.description || undefined,
        labels: itemForm.labels ? itemForm.labels.split(",").map((l) => l.trim()).filter(Boolean) : [],
      });
      setShowItemModal(false);
      setItemForm({ title: "", item_type: "feature", priority: "medium", story_points: "", sprint_id: "", labels: "", description: "" });
      await load();
      setBoardReload((v) => v + 1);
    } finally { setSaving(false); }
  }

  async function createRelease() {
    if (!releaseForm.version.trim() || !project) return;
    setSaving(true);
    try {
      await rndApi.createRelease(project.id, {
        version: releaseForm.version.trim(),
        release_name: releaseForm.release_name || undefined,
        release_notes: releaseForm.release_notes || undefined,
        release_date: releaseForm.release_date || undefined,
      });
      setShowReleaseModal(false);
      setReleaseForm({ version: "", release_name: "", release_notes: "", release_date: "" });
      await load();
    } finally { setSaving(false); }
  }

  async function startSprint(sprintId: string) {
    await rndApi.startSprint(sprintId);
    await load();
  }

  async function completeSprint(sprintId: string) {
    setSaving(true);
    try {
      await rndApi.completeSprint(sprintId, {
        velocity_achieved: retroForm.velocity_achieved ? parseInt(retroForm.velocity_achieved) : undefined,
        retrospective_notes: retroForm.retrospective_notes || undefined,
      });
      setShowCompleteModal(null);
      setRetroForm({ velocity_achieved: "", retrospective_notes: "" });
      await load();
    } finally { setSaving(false); }
  }

  async function publishRelease(releaseId: string) {
    await rndApi.publishRelease(releaseId);
    await load();
  }

  if (loading) {
    return <div className="flex justify-center py-20"><Loader2 className="size-6 animate-spin text-fg-muted" aria-label="Loading project" /></div>;
  }

  if (!project) {
    return <EmptyState title="Project not found." action={<Button asChild><Link href="/rnd/projects">Back to R&D projects</Link></Button>} />;
  }

  const activeSprint = project.sprints?.find((s) => s.status === "active");
  const sprintColumns: ColumnDef<Sprint>[] = [
    {
      accessorKey: "name", header: "Sprint",
      cell: (c) => (
        <div className="min-w-0">
          <p className="font-medium text-fg">{c.row.original.name}</p>
          {c.row.original.goal && <p className="max-w-md truncate text-xs text-fg-muted">{c.row.original.goal}</p>}
        </div>
      ),
    },
    {
      id: "dates", accessorFn: (sp) => sp.start_date ?? "", header: "Dates", meta: { hideOnMobile: true },
      cell: (c) => c.row.original.start_date
        ? <span className="text-fg-muted tabular">{shortDate(c.row.original.start_date)}{c.row.original.end_date ? ` to ${shortDate(c.row.original.end_date)}` : ""}</span>
        : <span className="text-fg-subtle">-</span>,
    },
    {
      id: "items", header: "Items done", meta: { numeric: true },
      cell: (c) => {
        const items = backlog.filter((b) => b.sprint_id === c.row.original.id);
        return items.length ? `${items.filter((b) => b.status === "done").length}/${items.length}` : <span className="text-fg-subtle">-</span>;
      },
    },
    {
      id: "velocity", header: "Points", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => c.row.original.velocity_planned ? `${c.row.original.velocity_achieved ?? 0}/${c.row.original.velocity_planned}` : <span className="text-fg-subtle">-</span>,
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="sprint" status={c.getValue<string>()} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const sp = c.row.original;
        if (sp.status === "planning") return <Button size="sm" onClick={() => startSprint(sp.id)}><PlayCircle aria-hidden /> Start</Button>;
        if (sp.status === "active") return <Button size="sm" onClick={() => setShowCompleteModal(sp.id)}><CheckCircle2 aria-hidden /> Complete</Button>;
        return null;
      },
    },
  ];

  return (
    <div className="space-y-4">
      <PageHeader title={project.name} back={{ href: "/rnd/projects", label: "R&D projects" }}
        status={<span className="inline-flex items-center gap-2"><StatusPill status={project.status} /><span className="font-mono text-xs text-fg-muted">{project.code}</span></span>}
        actions={<>
          {project.latest_version && <Pill icon={Tag}>{project.latest_version}</Pill>}
          {(project.gitlab_repos ?? []).map((repo) => {
            const ps = repo.last_pipeline_status;
            const pm = ps ? PIPELINE_STATUS[ps] : undefined;
            return (
              <Button key={repo.id} size="sm" asChild>
                <a href={repo.web_url ?? "#"} target="_blank" rel="noreferrer">
                  <GitBranch aria-hidden /> {repo.name}
                  {pm && <Pill tone={pm.tone}>{pm.label}</Pill>}
                </a>
              </Button>
            );
          })}
        </>} />

      {activeSprint && (
        <Card className="flex flex-wrap items-center justify-between gap-3 border-tone-blue/30 bg-tone-blue-soft px-4 py-3">
          <div className="flex items-center gap-3">
            <PlayCircle className="size-5 text-tone-blue" aria-hidden />
            <div>
              <p className="text-sm font-semibold text-fg">{activeSprint.name}</p>
              {activeSprint.goal && <p className="text-xs text-fg-muted">{activeSprint.goal}</p>}
            </div>
          </div>
          <div className="flex items-center gap-3">
            {activeSprint.end_date && <span className="text-xs text-fg-muted tabular">Ends {shortDate(activeSprint.end_date)}</span>}
            <Button size="sm" variant="primary" onClick={() => setShowCompleteModal(activeSprint.id)}>
              <CheckCircle2 aria-hidden /> Complete sprint
            </Button>
          </div>
        </Card>
      )}

      <Tabs value={tab} onValueChange={(v) => setTab(v as TabKey)}>
        <TabsList>
          <TabsTrigger value="board">Backlog board</TabsTrigger>
          <TabsTrigger value="sprints">Sprints</TabsTrigger>
          <TabsTrigger value="releases">Releases</TabsTrigger>
          <TabsTrigger value="roadmap">Roadmap</TabsTrigger>
          <TabsTrigger value="files">Files</TabsTrigger>
        </TabsList>

        <TabsContent value="board" className="pt-4">
          <div className="mb-3 flex items-center justify-between">
            <p className="text-[13px] text-fg-muted tabular">{backlog.length} items total</p>
            <Button size="sm" variant="primary" onClick={() => setShowItemModal(true)}><Plus aria-hidden /> Add item</Button>
          </div>
          <KanbanBoard adapter={rndIssueAdapter} scopeId={id} reloadKey={boardReload} />
        </TabsContent>

        <TabsContent value="sprints" className="pt-4">
          <Card className="overflow-hidden">
            <CardHeader title="Sprints" description={`${project.sprints?.length ?? 0} sprints total`}
              actions={<Button size="sm" variant="primary" onClick={() => setShowSprintModal(true)}><Plus aria-hidden /> New sprint</Button>} />
            <DataTable label="Sprints" columns={sprintColumns} data={project.sprints ?? []} getRowId={(r) => r.id} maxHeight="none"
              empty={{ icon: Target, title: "No sprints yet", description: "Plan your first iteration." }}
              renderDetails={(sprint) => {
                const sprintItems = backlog.filter((b) => b.sprint_id === sprint.id);
                return (
                  <div className="space-y-3">
                    {sprintItems.length === 0 ? (
                      <p className="text-xs text-fg-muted">No items in this sprint.</p>
                    ) : (
                      <ul className="divide-y divide-line-soft rounded-lg border border-line bg-surface">
                        {sprintItems.map((item) => {
                          const pm = PRIORITY_META[item.priority];
                          return (
                            <li key={item.id} className="flex items-center gap-3 px-3 py-1.5 text-[13px]">
                              <span className="flex-1 truncate text-fg">{item.title}</span>
                              <Pill tone={pm?.tone ?? "neutral"}>{pm?.label ?? item.priority}</Pill>
                              <StatusPill family="rnd_item" status={item.status} label={STATUS_LABELS[item.status]} />
                              {item.story_points && <span className="text-xs font-medium text-fg-muted tabular">{item.story_points}pt</span>}
                            </li>
                          );
                        })}
                      </ul>
                    )}
                    {sprint.retrospective_notes && (
                      <div>
                        <p className="mb-1 text-xs font-medium text-fg-muted">Retrospective</p>
                        <p className="whitespace-pre-wrap text-[13px] text-fg">{sprint.retrospective_notes}</p>
                      </div>
                    )}
                  </div>
                );
              }} />
          </Card>
        </TabsContent>

        <TabsContent value="releases" className="pt-4">
          <div className="mb-3 flex items-center justify-between">
            <p className="text-[13px] text-fg-muted tabular">{releases.length} releases</p>
            <Button size="sm" variant="primary" onClick={() => setShowReleaseModal(true)}><Plus aria-hidden /> New release</Button>
          </div>
          <div className="space-y-3">
            {releases.map((rel) => (
              <Card key={rel.id}>
                <CardHeader
                  title={<span className="inline-flex flex-wrap items-center gap-2"><span className="font-mono">{rel.version}</span>{rel.release_name && <span className="font-normal text-fg-muted">{rel.release_name}</span>}</span>}
                  description={rel.release_date ? new Date(rel.release_date).toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" }) : undefined}
                  actions={<>
                    <StatusPill family="release" status={rel.status} />
                    {rel.gitlab_tag_url && (
                      <Button size="sm" asChild><a href={rel.gitlab_tag_url} target="_blank" rel="noreferrer"><GitBranch aria-hidden /> {rel.gitlab_tag_name ?? "tag"}</a></Button>
                    )}
                    {rel.status !== "released" && (
                      <Button size="sm" variant="primary" onClick={() => publishRelease(rel.id)}><Rocket aria-hidden /> Publish</Button>
                    )}
                  </>} />
                {((rel.changelog && rel.changelog.length > 0) || rel.release_notes) && (
                  <CardBody className="space-y-2">
                    {rel.changelog && rel.changelog.length > 0 && (
                      <ul className="space-y-1">
                        {rel.changelog.slice(0, 5).map((entry, i) => (
                          <li key={i} className="flex items-start gap-2 text-xs">
                            <Pill tone={CHANGE_TONES[entry.type] ?? "neutral"} className="shrink-0">{entry.type}</Pill>
                            <span className="text-fg">{entry.message}</span>
                          </li>
                        ))}
                      </ul>
                    )}
                    {rel.release_notes && <p className="line-clamp-2 text-xs text-fg-muted">{rel.release_notes}</p>}
                  </CardBody>
                )}
              </Card>
            ))}
            {releases.length === 0 && (
              <Card><EmptyState icon={Rocket} title="No releases yet" description="Create a release when a version is ready." /></Card>
            )}
          </div>
        </TabsContent>

        <TabsContent value="roadmap" className="pt-4">
          <Card>
            <CardBody className="min-h-[200px]">
              {project.roadmap_notes ? (
                <pre className="whitespace-pre-wrap font-sans text-sm leading-relaxed text-fg">{project.roadmap_notes}</pre>
              ) : (
                <EmptyState icon={Flag} title="No roadmap notes yet" />
              )}
            </CardBody>
          </Card>
        </TabsContent>

        <TabsContent value="files" className="pt-4">
          <Card><CardBody><AttachmentPanel entityType="rnd_project" entityId={project.id} /></CardBody></Card>
        </TabsContent>
      </Tabs>

      <Dialog open={showSprintModal} onOpenChange={setShowSprintModal} title="New sprint"
        footer={<>
          <Button onClick={() => setShowSprintModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={createSprint} loading={saving} disabled={!sprintForm.name.trim()}>{saving ? "Creating..." : "Create sprint"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Sprint name" required className="md:col-span-2">
            <Input value={sprintForm.name} onChange={(e) => setSprintForm({ ...sprintForm, name: e.target.value })} placeholder="Sprint 1, auth and dashboard" />
          </Field>
          <Field label="Sprint goal" className="md:col-span-2">
            <Input value={sprintForm.goal} onChange={(e) => setSprintForm({ ...sprintForm, goal: e.target.value })} placeholder="Ship user auth and dashboard skeleton" />
          </Field>
          <Field label="Start date">
            <Input type="date" value={sprintForm.start_date} onChange={(e) => setSprintForm({ ...sprintForm, start_date: e.target.value })} />
          </Field>
          <Field label="End date">
            <Input type="date" value={sprintForm.end_date} onChange={(e) => setSprintForm({ ...sprintForm, end_date: e.target.value })} />
          </Field>
          <Field label="Velocity target (points)" className="md:col-span-2">
            <Input numeric type="number" value={sprintForm.velocity_planned} onChange={(e) => setSprintForm({ ...sprintForm, velocity_planned: e.target.value })} placeholder="e.g. 40" />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={showItemModal} onOpenChange={setShowItemModal} title="Add backlog item"
        footer={<>
          <Button onClick={() => setShowItemModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={createItem} loading={saving} disabled={!itemForm.title.trim()}>{saving ? "Adding..." : "Add item"}</Button>
        </>}>
        <FieldGrid cols={3}>
          <Field label="Title" required className="md:col-span-3">
            <Input value={itemForm.title} onChange={(e) => setItemForm({ ...itemForm, title: e.target.value })} placeholder="e.g. Add CSV export to the orders endpoint" />
          </Field>
          <Field label="Type">
            <Select value={itemForm.item_type} onChange={(e) => setItemForm({ ...itemForm, item_type: e.target.value })}
              options={Object.entries(TYPE_META).map(([k, v]) => ({ value: k, label: v.label }))} />
          </Field>
          <Field label="Priority">
            <Select value={itemForm.priority} onChange={(e) => setItemForm({ ...itemForm, priority: e.target.value })}
              options={Object.entries(PRIORITY_META).map(([k, v]) => ({ value: k, label: v.label }))} />
          </Field>
          <Field label="Points">
            <Input numeric type="number" value={itemForm.story_points} onChange={(e) => setItemForm({ ...itemForm, story_points: e.target.value })} placeholder="SP" />
          </Field>
          <Field label="Sprint" className="md:col-span-3">
            <Select value={itemForm.sprint_id} onChange={(e) => setItemForm({ ...itemForm, sprint_id: e.target.value })} placeholder="Unassigned (backlog)"
              options={(project.sprints ?? []).filter((s) => s.status !== "completed").map((s) => ({ value: s.id, label: s.name }))} />
          </Field>
          <Field label="Labels" hint="Comma-separated." className="md:col-span-3">
            <Input value={itemForm.labels} onChange={(e) => setItemForm({ ...itemForm, labels: e.target.value })} placeholder="backend, performance, breaking" />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={showReleaseModal} onOpenChange={setShowReleaseModal} title="New release"
        footer={<>
          <Button onClick={() => setShowReleaseModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={createRelease} loading={saving} disabled={!releaseForm.version.trim()}>{saving ? "Creating..." : "Create release"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Version" required>
            <Input value={releaseForm.version} onChange={(e) => setReleaseForm({ ...releaseForm, version: e.target.value })} placeholder="2.1.0" className="font-mono" />
          </Field>
          <Field label="Release date">
            <Input type="date" value={releaseForm.release_date} onChange={(e) => setReleaseForm({ ...releaseForm, release_date: e.target.value })} />
          </Field>
          <Field label="Release name" className="md:col-span-2">
            <Input value={releaseForm.release_name} onChange={(e) => setReleaseForm({ ...releaseForm, release_name: e.target.value })} placeholder="Spring Update" />
          </Field>
          <Field label="Release notes" className="md:col-span-2">
            <Textarea value={releaseForm.release_notes} onChange={(e) => setReleaseForm({ ...releaseForm, release_notes: e.target.value })} rows={4} placeholder="Markdown release notes" className="font-mono" />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={!!showCompleteModal} onOpenChange={(o) => { if (!o) setShowCompleteModal(null); }} title="Complete sprint"
        description="Record the retrospective before closing this sprint."
        footer={<>
          <Button onClick={() => setShowCompleteModal(null)}>Cancel</Button>
          <Button variant="primary" onClick={() => showCompleteModal && completeSprint(showCompleteModal)} loading={saving}>{saving ? "Completing..." : "Complete sprint"}</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Velocity achieved (points)">
            <Input numeric type="number" value={retroForm.velocity_achieved} onChange={(e) => setRetroForm({ ...retroForm, velocity_achieved: e.target.value })} placeholder="e.g. 36" />
          </Field>
          <Field label="Retrospective notes">
            <Textarea value={retroForm.retrospective_notes} onChange={(e) => setRetroForm({ ...retroForm, retrospective_notes: e.target.value })} rows={4}
              placeholder="What went well? What to improve next sprint?" />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
