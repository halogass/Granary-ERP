"use client";

import { useState, useEffect } from "react";
import { useParams, useRouter } from "next/navigation";
import { kbApi } from "@/lib/api";
import { extractApiError, apiErrorCode } from "@/lib/apiError";
import { AlertCircle, History, Save } from "lucide-react";
import {
  Button, Card, CardBody, CardHeader, CountBadge, Field, FieldGrid, Input, PageHeader, Select, Skeleton,
} from "@/components/ui";
import { ArticleDocumentLinks } from "@/components/documents/ArticleDocuments";
import { ACCESS_OPTIONS, MarkdownEditor, PrivateSwitch, SopStepsEditor, type SopStep } from "../../../_components/ArticleEditor";

const ARTICLE_TYPES = [
  { key: "wiki",     label: "Wiki" },
  { key: "sop",      label: "SOP" },
  { key: "runbook",  label: "Runbook" },
  { key: "guide",    label: "Guide" },
  { key: "policy",   label: "Policy" },
  { key: "template", label: "Template" },
  { key: "faq",      label: "FAQ" },
];

interface Revision {
  id: string;
  version: string;
  change_summary: string;
  changed_by_id: string | null;
  created_at: string;
}

export default function EditArticlePage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();

  const [showHistory, setShowHistory] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [changeSummary, setChangeSummary] = useState("Updated content");
  const [revisions, setRevisions] = useState<Revision[]>([]);
  // A published SOP is not edited in place: saving opens a new draft revision (signed + approved).
  const [articleStatus, setArticleStatus] = useState("");

  const [form, setForm] = useState({
    title: "",
    article_type: "wiki",
    access_level: "all_staff",
    summary: "",
    sop_code: "",
    sop_trigger: "",
    sop_owner_role: "",
    tags: "",
    content: "",
    category_id: "",
    is_private: false,
  });
  const [sopSteps, setSopSteps] = useState<SopStep[]>([]);
  const isSop = form.article_type === "sop";

  useEffect(() => {
    async function load() {
      setLoading(true);
      try {
        const [articleRes, revisionsRes] = await Promise.all([
          kbApi.getArticle(id),
          kbApi.getRevisions(id).catch(() => ({ data: [] })),
        ]);
        const a = articleRes.data;
        if (a.pending_revision_id) {
          // This SOP already has a draft revision: edit that instead of the live version
          router.replace(`/knowledge/articles/${a.pending_revision_id}/edit`);
          return;
        }
        setArticleStatus(a.status ?? "");
        setForm({
          title: a.title ?? "",
          article_type: a.article_type ?? "wiki",
          access_level: a.access_level ?? "all_staff",
          summary: a.summary ?? "",
          sop_code: a.sop_code ?? "",
          sop_trigger: a.sop_trigger ?? "",
          sop_owner_role: a.sop_owner_role ?? "",
          tags: (a.tags ?? []).join(", "),
          content: a.content ?? "",
          category_id: a.category_id ?? "",
          is_private: a.is_private ?? false,
        });
        setSopSteps(a.sop_steps ?? []);
        setRevisions(revisionsRes.data ?? []);
      } catch { setError("Failed to load article"); }
      finally { setLoading(false); }
    }
    load();
  }, [id, router]);

  const opensRevision = isSop && articleStatus === "published";

  async function save(publish = false) {
    if (!form.title.trim()) { setError("Title is required"); return; }
    setSaving(true);
    setError("");
    try {
      const res = await kbApi.updateArticle(id, {
        title: form.title.trim(),
        content: form.content,
        summary: form.summary.trim() || undefined,
        access_level: form.access_level,
        is_private: form.is_private,
        sop_code: isSop ? (form.sop_code.trim() || undefined) : undefined,
        sop_trigger: isSop ? (form.sop_trigger.trim() || undefined) : undefined,
        sop_owner_role: isSop ? (form.sop_owner_role.trim() || undefined) : undefined,
        sop_steps: isSop ? sopSteps : undefined,
        tags: form.tags ? form.tags.split(",").map((t) => t.trim()).filter(Boolean) : [],
        category_id: form.category_id || undefined,
        change_summary: changeSummary,
      });
      if (res.data?.created_revision) {
        // The edit became a new draft revision: it is signed and approved from its own page
        router.push(`/knowledge/articles/${res.data.id}`);
        return;
      }
      if (publish) {
        try {
          await kbApi.publishArticle(id);
        } catch (pubErr) {
          // An SOP is saved and submitted for Owner approval; it publishes once approved.
          if (apiErrorCode(pubErr) !== "approval_pending") throw pubErr;
        }
      }
      router.push(`/knowledge/articles/${id}`);
    } catch (e: unknown) {
      setError(extractApiError(e, "Failed to save"));
    } finally {
      setSaving(false);
    }
  }

  if (loading) return (
    <div className="mx-auto max-w-5xl space-y-4"><Skeleton className="h-10 w-64" /><Skeleton className="h-48 w-full" /><Skeleton className="h-80 w-full" /></div>
  );

  return (
    <div className="mx-auto flex max-w-6xl flex-col">
      <PageHeader title={form.title || "Edit"} back={{ href: `/knowledge/articles/${id}`, label: "Back to article" }}
        actions={<>
          <Button aria-pressed={showHistory} onClick={() => setShowHistory(!showHistory)} className={showHistory ? "bg-surface-2" : undefined}>
            <History aria-hidden /> History <CountBadge count={revisions.length} tone="muted" />
          </Button>
          <Button onClick={() => save(false)} disabled={saving || !form.title.trim()}>
            <Save aria-hidden /> Save draft
          </Button>
          <Button variant="primary" onClick={() => save(true)} disabled={!form.title.trim()} loading={saving}>
            {saving ? "Saving..." : opensRevision ? "Save as revision" : "Save and publish"}
          </Button>
        </>} />

      {opensRevision && (
        <div role="status" className="mb-4 flex items-center gap-2 rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-4 py-2.5 text-sm text-fg">
          <AlertCircle className="size-4 shrink-0 text-tone-orange" aria-hidden />
          This SOP is published. Saving creates a new draft revision; the published version stays live
          until the revision is signed by its author and the Owner and approved.
        </div>
      )}

      {error && (
        <div role="alert" className="mb-4 flex items-center gap-2 rounded-lg border border-tone-red/30 bg-tone-red-soft px-4 py-2.5 text-sm text-tone-red">
          <AlertCircle className="size-4 shrink-0" aria-hidden /> {error}
        </div>
      )}

      <div className="flex min-h-0 flex-1 flex-col gap-4 lg:flex-row">
        <div className="flex min-w-0 flex-1 flex-col gap-4">
          <Card>
            <CardBody className="space-y-4">
              <FieldGrid cols={2}>
                <Field label="Title" required>
                  <Input value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} autoComplete="off" />
                </Field>
                <Field label="Summary">
                  <Input value={form.summary} onChange={(e) => setForm({ ...form, summary: e.target.value })} autoComplete="off" />
                </Field>
              </FieldGrid>
              <FieldGrid cols={3}>
                <Field label="Type">
                  <Select value={form.article_type} onChange={(e) => setForm({ ...form, article_type: e.target.value })}
                    options={ARTICLE_TYPES.map((t) => ({ value: t.key, label: t.label }))} />
                </Field>
                <Field label="Access level">
                  <Select value={form.access_level} onChange={(e) => setForm({ ...form, access_level: e.target.value })} options={ACCESS_OPTIONS} />
                </Field>
                <Field label="Tags">
                  <Input value={form.tags} onChange={(e) => setForm({ ...form, tags: e.target.value })} placeholder="comma-separated" autoComplete="off" />
                </Field>
              </FieldGrid>
              <PrivateSwitch checked={form.is_private} onChange={(v) => setForm((f) => ({ ...f, is_private: v }))} />
              <Field label="Change summary" hint="Saved in the audit log.">
                <Input value={changeSummary} onChange={(e) => setChangeSummary(e.target.value)} placeholder="Describe what changed" autoComplete="off" />
              </Field>
            </CardBody>
          </Card>

          {isSop && (
            <Card>
              <CardHeader title="SOP fields" description="Executive only." />
              <CardBody className="space-y-4">
                <FieldGrid cols={3}>
                  <Field label="SOP code">
                    <Input value={form.sop_code} onChange={(e) => setForm({ ...form, sop_code: e.target.value })} placeholder="SOP-001" className="font-mono" autoComplete="off" />
                  </Field>
                  <Field label="Owner role">
                    <Input value={form.sop_owner_role} onChange={(e) => setForm({ ...form, sop_owner_role: e.target.value })} autoComplete="off" />
                  </Field>
                  <Field label="Trigger">
                    <Input value={form.sop_trigger} onChange={(e) => setForm({ ...form, sop_trigger: e.target.value })} autoComplete="off" />
                  </Field>
                </FieldGrid>
                <SopStepsEditor steps={sopSteps} onChange={setSopSteps} emptyText="No structured steps. Use the Markdown body below." />
              </CardBody>
            </Card>
          )}

          <MarkdownEditor value={form.content} onChange={(content) => setForm((f) => ({ ...f, content }))} />

          <ArticleDocumentLinks articleId={id} />
        </div>

        {showHistory && (
          <Card className="h-fit lg:w-72 lg:flex-shrink-0">
            <CardHeader title={<span className="inline-flex items-center gap-2"><History className="size-4 text-fg-muted" aria-hidden /> Change history</span>} />
            <CardBody>
              {revisions.length === 0 ? (
                <p className="text-xs italic text-fg-muted">No revisions yet. History is saved each time you update.</p>
              ) : (
                <ul className="space-y-2">
                  {revisions.map((rev) => (
                    <li key={rev.id} className="rounded-lg bg-surface-2 p-3">
                      <div className="mb-1 flex items-center justify-between">
                        <span className="font-mono text-xs font-medium text-fg">v{rev.version}</span>
                        <span className="text-xs text-fg-muted tabular">
                          {new Date(rev.created_at).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "2-digit" })}
                        </span>
                      </div>
                      <p className="text-xs leading-relaxed text-fg-muted">{rev.change_summary || "Updated"}</p>
                    </li>
                  ))}
                </ul>
              )}
            </CardBody>
          </Card>
        )}
      </div>
    </div>
  );
}
