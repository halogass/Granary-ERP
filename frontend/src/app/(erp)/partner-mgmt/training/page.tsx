"use client";

import { useState, useEffect, useMemo } from "react";
import { useSession } from "next-auth/react";
import { GraduationCap, Plus, Trash2, Users } from "lucide-react";
import { partnerApi } from "@/lib/api";
import { optionLabel, usePartnerOptions } from "@/lib/partnerOptions";
import {
  Button, Card, CardBody, Checkbox, DataTable, Dialog, Field, FieldGrid, IconButton, Input, ListFrame, PageHeader,
  SavedViewTabs, Select, StatusPill, Switch, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Module {
  id: string;
  title: string;
  description: string;
  min_tier: string;
  partner_types: string[];
  is_published: boolean;
  pass_score: number;
  chapter_count: number;
  created_at: string;
}

interface ChapterInput {
  title: string;
  content: string;
  video_url: string;
  order: number;
  questions: QuizQuestionInput[];
}

interface QuizQuestionInput {
  question: string;
  options: string[];
  correct_index: number;
  order: number;
}

/** One partner user's progress on a module (GET /partner/training/modules/{id}/progress). */
interface Progress {
  id: string;
  user_name: string | null;
  user_email: string | null;
  partner_name: string | null;
  status: string | null;
  attempts: number;
  attempts_left: number;
}

function emptyChapter(order: number): ChapterInput {
  return { title: "", content: "", video_url: "", order, questions: [] };
}

function emptyQuestion(order: number): QuizQuestionInput {
  return { question: "", options: ["", "", "", ""], correct_index: 0, order };
}

type Filters = { published?: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "published", label: "Published", filters: { published: true } },
  { id: "unpublished", label: "Unpublished", filters: { published: false } },
];

export default function PartnerTrainingAdminPage() {
  const { data: session } = useSession();
  const views = useSavedViews<Filters>("partner.training", VIEWS);
  const [modules, setModules] = useState<Module[]>([]);
  const [loading, setLoading] = useState(true);
  const [showCreate, setShowCreate] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const options = usePartnerOptions();
  const [progressFor, setProgressFor] = useState<Module | null>(null);
  const [progress, setProgress] = useState<Progress[]>([]);

  // Module creation form
  const [modForm, setModForm] = useState({
    title: "", description: "", min_tier: "authorized",
    partner_types: [] as string[], pass_score: 70, is_published: false,
  });
  const [chapters, setChapters] = useState<ChapterInput[]>([emptyChapter(1)]);

  const token = session?.access_token;

  useEffect(() => {
    if (!token) return;
    fetch("/api/v1/partner/training/modules", { headers: { Authorization: `Bearer ${token}` } })
      .then(r => r.ok ? r.json() : [])
      .then(data => setModules(Array.isArray(data) ? data : data?.items ?? []))
      .catch(() => toast.error("Failed to load modules"))
      .finally(() => setLoading(false));
  }, [token]);

  async function createModule(e: React.FormEvent) {
    e.preventDefault();
    if (!token) return;
    setSubmitting(true);
    try {
      // Create module
      const modRes = await fetch("/api/v1/partner/training/modules", {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        // Every partner type: the module is offered by tier.
        body: JSON.stringify({ ...modForm, partner_types: options.partner_types.map((o) => o.value) }),
      });
      if (!modRes.ok) throw new Error("Module creation failed");
      const mod = await modRes.json();

      // Add chapters
      for (const ch of chapters) {
        if (!ch.title.trim()) continue;
        const chRes = await fetch(`/api/v1/partner/training/modules/${mod.id}/chapters`, {
          method: "POST",
          headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
          body: JSON.stringify({ ...ch, questions: ch.questions.filter(q => q.question.trim()) }),
        });
        if (!chRes.ok) toast.error(`Chapter "${ch.title}" failed to save`);
      }

      setModules(prev => [{ ...mod, chapter_count: chapters.filter(c => c.title).length }, ...prev]);
      setShowCreate(false);
      setModForm({ title: "", description: "", min_tier: "authorized", partner_types: [], pass_score: 70, is_published: false });
      setChapters([emptyChapter(1)]);
      toast.success("Training module created");
    } catch (err) { toast.error((err as Error).message || "Failed to create module"); }
    finally { setSubmitting(false); }
  }

  async function togglePublish(mod: Module) {
    if (!token) return;
    try {
      const res = await fetch(`/api/v1/partner/training/modules/${mod.id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({ is_published: !mod.is_published }),
      });
      if (!res.ok) throw new Error();
      setModules(prev => prev.map(m => m.id === mod.id ? { ...m, is_published: !m.is_published } : m));
      toast.success(`Module ${mod.is_published ? "unpublished" : "published"}`);
    } catch { toast.error("Failed to update module"); }
  }

  async function deleteModule(mod: Module) {
    if (!token || !confirm(`Delete "${mod.title}"?`)) return;
    try {
      const res = await fetch(`/api/v1/partner/training/modules/${mod.id}`, {
        method: "DELETE", headers: { Authorization: `Bearer ${token}` },
      });
      if (!res.ok) throw new Error();
      setModules(prev => prev.filter(m => m.id !== mod.id));
      toast.success("Module deleted");
    } catch { toast.error("Delete failed"); }
  }

  async function openProgress(mod: Module) {
    setProgressFor(mod);
    setProgress([]);
    try {
      const res = await partnerApi.trainingProgress(mod.id);
      setProgress(res.data ?? []);
    } catch { toast.error("Could not load the progress"); }
  }

  async function resetAttempts(p: Progress) {
    try {
      await partnerApi.resetAttempts(p.id);
      setProgress((prev) => prev.map((x) => (x.id === p.id ? { ...x, attempts: 0, attempts_left: x.attempts_left + x.attempts } : x)));
      toast.success(`Quiz attempts reset for ${p.user_name ?? p.user_email ?? "the partner"}`);
    } catch { toast.error("Could not reset the attempts"); }
  }

  function addChapter() { setChapters(prev => [...prev, emptyChapter(prev.length + 1)]); }
  function updateChapter(idx: number, updates: Partial<ChapterInput>) {
    setChapters(prev => prev.map((c, i) => i === idx ? { ...c, ...updates } : c));
  }
  function addQuestion(chapterIdx: number) {
    setChapters(prev => prev.map((c, i) => i === chapterIdx ? { ...c, questions: [...c.questions, emptyQuestion(c.questions.length + 1)] } : c));
  }
  function updateQuestion(chapterIdx: number, qIdx: number, updates: Partial<QuizQuestionInput>) {
    setChapters(prev => prev.map((c, i) => i === chapterIdx ? {
      ...c,
      questions: c.questions.map((q, qi) => qi === qIdx ? { ...q, ...updates } : q),
    } : c));
  }

  const published = views.active?.filters.published;
  const rows = modules.filter(m => published === undefined || m.is_published === published);

  const columns = useMemo<ColumnDef<Module>[]>(() => [
    {
      accessorKey: "title", header: "Module",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.row.original.title}</p>
          {c.row.original.description && <p className="line-clamp-1 text-xs text-fg-muted">{c.row.original.description}</p>}
        </div>
      ),
    },
    {
      accessorKey: "min_tier", header: "Min tier",
      cell: (c) => <StatusPill family="partner_tier" status={c.getValue<string>()} label={`${optionLabel(options.tiers, c.getValue<string>())}+`} />,
    },
    { accessorKey: "chapter_count", header: "Chapters", meta: { numeric: true } },
    { accessorKey: "pass_score", header: "Pass score", meta: { numeric: true, hideOnMobile: true }, cell: (c) => `${c.getValue<number>()}%` },
    {
      accessorKey: "is_published", header: "Published",
      cell: (c) => (
        <Switch checked={c.row.original.is_published} onCheckedChange={() => togglePublish(c.row.original)}
          aria-label={`${c.row.original.is_published ? "Unpublish" : "Publish"} ${c.row.original.title}`} />
      ),
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => (
        <div className="flex items-center justify-end gap-1">
          <IconButton size="sm" variant="ghost" icon={Users} label={`Progress on ${c.row.original.title}`} onClick={() => openProgress(c.row.original)} />
          <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${c.row.original.title}`} onClick={() => deleteModule(c.row.original)} />
        </div>
      ),
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [token, options]);

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Training modules" summary="Create and manage partner training content and certifications."
          actions={<Button variant="primary" onClick={() => setShowCreate(true)}><Plus aria-hidden /> New module</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} />
        <DataTable label="Training modules" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: GraduationCap, title: "No training modules yet", description: "Create one to get started." }} />
      </ListFrame>

      <Dialog open={showCreate} onOpenChange={setShowCreate} title="Create training module" size="xl"
        footer={<>
          <Button onClick={() => setShowCreate(false)}>Cancel</Button>
          <Button type="submit" form="training-module-form" variant="primary" loading={submitting}>{submitting ? "Creating" : "Create module"}</Button>
        </>}>
        <form id="training-module-form" onSubmit={createModule} className="space-y-6">
          <section className="space-y-4">
            <h3 className="text-sm font-semibold text-fg">Module info</h3>
            <FieldGrid cols={3}>
              <Field label="Title" required className="md:col-span-3">
                <Input required value={modForm.title} onChange={e => setModForm(p => ({ ...p, title: e.target.value }))} />
              </Field>
              <Field label="Description" className="md:col-span-3">
                <Textarea rows={2} value={modForm.description} onChange={e => setModForm(p => ({ ...p, description: e.target.value }))} />
              </Field>
              <Field label="Min tier">
                <Select value={modForm.min_tier} onChange={e => setModForm(p => ({ ...p, min_tier: e.target.value }))}
                  options={options.tiers.map(t => ({ value: t.value, label: t.label }))} />
              </Field>
              <Field label="Pass score (%)">
                <Input numeric type="number" min="50" max="100" value={modForm.pass_score} onChange={e => setModForm(p => ({ ...p, pass_score: parseInt(e.target.value) }))} />
              </Field>
              <div className="flex items-end">
                <Checkbox label="Publish now" checked={modForm.is_published} onChange={e => setModForm(p => ({ ...p, is_published: e.target.checked }))} />
              </div>
            </FieldGrid>
          </section>

          <section className="space-y-3">
            <div className="flex items-center justify-between">
              <h3 className="text-sm font-semibold text-fg">Chapters</h3>
              <Button size="sm" variant="ghost" onClick={addChapter}><Plus aria-hidden /> Add chapter</Button>
            </div>
            {chapters.map((ch, ci) => (
              <Card key={ci}>
                <CardBody className="space-y-3">
                  <div className="flex items-center gap-2">
                    <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-brand text-xs font-semibold text-brand-on tabular">{ci + 1}</span>
                    <Input placeholder="Chapter title" aria-label={`Chapter ${ci + 1} title`} value={ch.title} onChange={e => updateChapter(ci, { title: e.target.value })} />
                  </div>
                  <Textarea rows={4} placeholder="Chapter content (Markdown supported)" aria-label={`Chapter ${ci + 1} content`} className="font-mono text-xs"
                    value={ch.content} onChange={e => updateChapter(ci, { content: e.target.value })} />
                  <Input placeholder="Video URL (YouTube embed or direct)" aria-label={`Chapter ${ci + 1} video URL`}
                    value={ch.video_url} onChange={e => updateChapter(ci, { video_url: e.target.value })} />

                  {ch.questions.length > 0 && (
                    <div className="space-y-3 pt-1">
                      {ch.questions.map((q, qi) => (
                        <fieldset key={qi} className="space-y-2 rounded-lg border border-line bg-surface-2 p-3">
                          <legend className="sr-only">Question {qi + 1}</legend>
                          <Input placeholder={`Question ${qi + 1}`} aria-label={`Question ${qi + 1}`} value={q.question}
                            onChange={e => updateQuestion(ci, qi, { question: e.target.value })} />
                          {q.options.map((opt, oi) => (
                            <div key={oi} className="flex items-center gap-2">
                              <input type="radio" name={`q${ci}_${qi}`} checked={q.correct_index === oi}
                                aria-label={`Option ${String.fromCharCode(65 + oi)} is correct`}
                                onChange={() => updateQuestion(ci, qi, { correct_index: oi })} className="size-4 accent-brand" />
                              <Input placeholder={`Option ${String.fromCharCode(65 + oi)}`} aria-label={`Option ${String.fromCharCode(65 + oi)}`} value={opt}
                                onChange={e => { const opts = [...q.options]; opts[oi] = e.target.value; updateQuestion(ci, qi, { options: opts }); }} />
                            </div>
                          ))}
                          <p className="text-xs text-fg-muted">The selected option is the correct answer.</p>
                        </fieldset>
                      ))}
                    </div>
                  )}
                  <Button size="sm" variant="ghost" onClick={() => addQuestion(ci)}><Plus aria-hidden /> Add quiz question</Button>
                </CardBody>
              </Card>
            ))}
          </section>
        </form>
      </Dialog>

      <Dialog open={!!progressFor} onOpenChange={(o) => { if (!o) setProgressFor(null); }} title="Partner progress" size="lg"
        description={progressFor ? `${progressFor.title}: quiz attempts used by each partner user` : undefined}
        footer={<Button onClick={() => setProgressFor(null)}>Close</Button>}>
        {progress.length === 0 ? (
          <p className="text-sm text-fg-muted">No partner has started this module yet.</p>
        ) : (
          <ul className="divide-y divide-line">
            {progress.map((p) => (
              <li key={p.id} className="flex items-center justify-between gap-3 py-2 text-sm">
                <div className="min-w-0">
                  <p className="truncate font-medium text-fg">{p.user_name ?? p.user_email ?? "-"}</p>
                  <p className="truncate text-xs text-fg-muted">{p.partner_name ?? "-"} · {p.status ?? "-"} · {p.attempts} used, {p.attempts_left} left</p>
                </div>
                {p.status !== "passed" && p.attempts > 0 && (
                  <Button size="sm" onClick={() => resetAttempts(p)}>Reset attempts</Button>
                )}
              </li>
            ))}
          </ul>
        )}
      </Dialog>
    </>
  );
}
