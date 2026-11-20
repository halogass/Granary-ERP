"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { kbApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { AlertCircle, FileText, Save, Upload, X } from "lucide-react";
import {
  Button, Card, CardBody, CardHeader, Field, FieldGrid, IconButton, Input, PageHeader, Select,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { ACCESS_OPTIONS, MarkdownEditor, PrivateSwitch, SopStepsEditor, type SopStep } from "../../_components/ArticleEditor";

const ARTICLE_TYPES = [
  { key: "wiki",     label: "Wiki",      desc: "General documentation" },
  { key: "sop",      label: "SOP",       desc: "Standard Operating Procedure, executive only" },
  { key: "runbook",  label: "Runbook",   desc: "Step-by-step operational guide" },
  { key: "guide",    label: "Guide",     desc: "How-to guide" },
  { key: "policy",   label: "Policy",    desc: "Company policy document" },
  { key: "template", label: "Template",  desc: "Reusable document template" },
  { key: "faq",      label: "FAQ",       desc: "Frequently asked questions" },
];

const SOP_TEMPLATE = `## Purpose

Describe the purpose of this SOP.

## Scope

Who this SOP applies to and when.

## Trigger

What event or condition triggers this procedure.

## Prerequisites

List anything that must be in place before starting.

## Procedure

### Step 1: [Action]

Description of what to do and who does it.

### Step 2: [Action]

Description.

## Notes

Any exceptions, edge cases, or important notes.

## Related SOPs

- [[SOP-000]] Related procedure
`;

function slugify(text: string): string {
  return text.toLowerCase().replace(/[^\w\s-]/g, "").replace(/[-\s]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 80);
}

export default function NewArticlePage() {
  const router = useRouter();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const [form, setForm] = useState({
    title: "",
    slug: "",
    article_type: "wiki",
    category_id: "",
    access_level: "all_staff",
    summary: "",
    sop_code: "",
    sop_trigger: "",
    sop_owner_role: "",
    tags: "",
    content: "",
    is_private: false,
  });

  const [sopSteps, setSopSteps] = useState<SopStep[]>([]);
  const [slugEdited, setSlugEdited] = useState(false);
  const [pdfFile, setPdfFile] = useState<File | null>(null);

  const isSop = form.article_type === "sop";
  // A PDF only converts into an empty body, so the two are mutually exclusive.
  const bodyTouched = form.content.trim().length > 0;

  // Keep the slug in sync with the full title until the user edits the slug manually.
  const handleTitleChange = (title: string) => {
    setForm((f) => ({ ...f, title, slug: slugEdited ? f.slug : slugify(title) }));
  };

  /** `sign`: open the saved article with the signature dialog: an article publishes only once its author has
   *  signed it (an SOP also needs the Owner), so "save and publish" in one step could never work. */
  async function save(sign = false) {
    if (!form.title.trim()) { setError("Title is required"); return; }
    if (!form.slug.trim()) { setError("Slug is required"); return; }
    setSaving(true);
    setError("");
    try {
      const res = await kbApi.createArticle({
        title: form.title.trim(),
        slug: form.slug.trim(),
        article_type: form.article_type,
        // Leave the body empty when a PDF is attached; the conversion fills it in.
        content: isSop && !form.content.trim() && !pdfFile ? SOP_TEMPLATE : form.content,
        summary: form.summary.trim() || undefined,
        category_id: form.category_id || undefined,
        access_level: form.access_level,
        is_private: form.is_private,
        sop_code: isSop ? (form.sop_code.trim() || undefined) : undefined,
        sop_trigger: isSop ? (form.sop_trigger.trim() || undefined) : undefined,
        sop_owner_role: isSop ? (form.sop_owner_role.trim() || undefined) : undefined,
        sop_steps: isSop ? sopSteps : undefined,
        tags: form.tags ? form.tags.split(",").map((t) => t.trim()).filter(Boolean) : [],
      });
      const id = res.data.id;
      if (pdfFile) {
        // Upload the source PDF and queue the conversion; the article page shows progress.
        await kbApi.importPdf(id, pdfFile);
      }
      router.push(sign ? `/knowledge/articles/${id}?sign=1` : `/knowledge/articles/${id}`);
    } catch (e: unknown) {
      setError(extractApiError(e, "Failed to save"));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="mx-auto flex max-w-5xl flex-col">
      <PageHeader title="New article" back={{ href: "/knowledge/articles", label: "Articles" }}
        actions={<>
          <Button onClick={() => save(false)} disabled={saving || !form.title.trim()}>
            <Save aria-hidden /> Save draft
          </Button>
          {/* Publishing a PDF-sourced article makes no sense until the conversion has been
              reviewed: the body is still empty at this point. */}
          <Button variant="primary" onClick={() => save(true)} disabled={!form.title.trim() || !!pdfFile} loading={saving}
            title={pdfFile
              ? "Review the converted article first, then sign and publish it"
              : "Saves the draft and opens it to sign; publish once it is signed"}>
            {saving ? "Saving..." : "Save and sign"}
          </Button>
        </>} />

      {error && (
        <div role="alert" className="mb-4 flex items-center gap-2 rounded-lg border border-tone-red/30 bg-tone-red-soft px-4 py-2.5 text-sm text-tone-red">
          <AlertCircle className="size-4 shrink-0" aria-hidden /> {error}
        </div>
      )}

      <Card className="mb-4">
        <CardBody className="space-y-4">
          <FieldGrid cols={2}>
            <Field label="Title" required>
              <Input value={form.title} onChange={(e) => handleTitleChange(e.target.value)} placeholder="e.g. Lead Qualification Process" autoComplete="off" />
            </Field>
            <Field label="Slug" required>
              <Input value={form.slug} onChange={(e) => { setSlugEdited(true); setForm({ ...form, slug: e.target.value }); }}
                placeholder="lead-qualification-process" className="font-mono" autoComplete="off" />
            </Field>
          </FieldGrid>
          <FieldGrid cols={3}>
            <Field label="Type">
              <Select value={form.article_type} onChange={(e) => setForm({ ...form, article_type: e.target.value })}
                options={ARTICLE_TYPES.map((t) => ({ value: t.key, label: `${t.label}: ${t.desc}` }))} />
            </Field>
            <Field label="Access level">
              <Select value={form.access_level} onChange={(e) => setForm({ ...form, access_level: e.target.value })} options={ACCESS_OPTIONS} />
            </Field>
            <Field label="Tags" hint="Comma-separated.">
              <Input value={form.tags} onChange={(e) => setForm({ ...form, tags: e.target.value })} placeholder="sales, process, onboarding" autoComplete="off" />
            </Field>
          </FieldGrid>
          <PrivateSwitch checked={form.is_private} onChange={(v) => setForm((f) => ({ ...f, is_private: v }))} />
          <Field label="Summary">
            <Input value={form.summary} onChange={(e) => setForm({ ...form, summary: e.target.value })} placeholder="Brief description of this article" autoComplete="off" />
          </Field>

          {/* Build the article from a PDF instead of typing it */}
          <div className="border-t border-line pt-4">
            <p className="mb-1.5 text-[13px] font-medium text-fg">Source PDF <span className="font-normal text-fg-muted">(optional)</span></p>
            {pdfFile ? (
              <div className="flex items-center gap-3 rounded-lg border border-line bg-surface-2 px-4 py-2.5">
                <FileText className="size-4 shrink-0 text-brand-ink" aria-hidden />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm text-fg">{pdfFile.name}</p>
                  <p className="text-xs text-fg-muted"><span className="tabular">{(pdfFile.size / 1024 / 1024).toFixed(1)} MB</span> · converted to Markdown after saving</p>
                </div>
                <IconButton icon={X} size="sm" label="Remove the PDF" className="hover:text-tone-red" onClick={() => setPdfFile(null)} />
              </div>
            ) : (
              <label className={cn("flex items-center gap-3 rounded-lg border border-dashed px-4 py-3 transition-colors",
                bodyTouched ? "cursor-not-allowed border-line bg-surface-2 opacity-60" : "cursor-pointer border-line-strong hover:border-brand")}>
                <Upload className="size-4 shrink-0 text-fg-subtle" aria-hidden />
                <span className="text-sm text-fg-muted">
                  {bodyTouched
                    ? "Clear the editor below to build this article from a PDF instead"
                    : "Attach a PDF: the AI reads it and writes the Markdown body for you"}
                </span>
                <input type="file" accept="application/pdf,.pdf" className="hidden" disabled={bodyTouched}
                  onChange={(e) => {
                    const f = e.target.files?.[0];
                    if (!f) return;
                    if (f.size > 25 * 1024 * 1024) { setError("PDF exceeds the 25 MB limit"); return; }
                    setError("");
                    setPdfFile(f);
                  }} />
              </label>
            )}
            {pdfFile && (
              <p className="mt-1.5 text-xs text-fg-muted">
                Conversion runs in the background and can take a few minutes. The article is saved as
                a draft: review it, then publish to make it searchable by the assistant.
              </p>
            )}
          </div>
        </CardBody>
      </Card>

      {isSop && (
        <Card className="mb-4">
          <CardHeader title="SOP fields" description="Only executive management can create or edit SOPs." />
          <CardBody className="space-y-4">
            <FieldGrid cols={3}>
              <Field label="SOP code">
                <Input value={form.sop_code} onChange={(e) => setForm({ ...form, sop_code: e.target.value })} placeholder="SOP-001" className="font-mono" autoComplete="off" />
              </Field>
              <Field label="Owner role">
                <Input value={form.sop_owner_role} onChange={(e) => setForm({ ...form, sop_owner_role: e.target.value })} placeholder="BDR, PM, Finance Admin..." autoComplete="off" />
              </Field>
              <Field label="Trigger">
                <Input value={form.sop_trigger} onChange={(e) => setForm({ ...form, sop_trigger: e.target.value })} placeholder="What initiates this SOP?" autoComplete="off" />
              </Field>
            </FieldGrid>
            <SopStepsEditor steps={sopSteps} onChange={setSopSteps}
              emptyText="No steps added. Steps can also be written in the Markdown body below." />
          </CardBody>
        </Card>
      )}

      <MarkdownEditor value={form.content} onChange={(content) => setForm((f) => ({ ...f, content }))}
        placeholder={isSop ? "Write the SOP content in Markdown. The SOP template will be used if left empty." : "Write article content in Markdown..."}
        emptyPreview="Nothing to preview: write some content first" />
    </div>
  );
}
