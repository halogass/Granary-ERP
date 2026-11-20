"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { AlertTriangle, FileUp, Plus, Search, ShieldCheck, X } from "lucide-react";
import {
  Button, Checkbox, Combobox, Dialog, Field, FieldGrid, IconButton, Input, KeyValues, Pill, Select, Textarea, toast,
} from "@/components/ui";
import type { Option, OptionSource } from "@/components/ui/SearchSelect";
import { useFileDrop } from "@/hooks/useFileDrop";
import { peopleApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { catalogSource } from "@/lib/entitySources";
import { cn } from "@/lib/utils";
import {
  AUDIENCES, DOC_MAX_MB_DEFAULT, LANGUAGE_OPTIONS, WATERMARK_LIMIT, docLibApi, fetchMaxFileMb, fetchVideoMaxMb,
  fetchSuggestedLabel, formatFileSize, itemsOf, languageLabel, matchedByLabel, needsApproval, productLabel, traceRecipient, uploadProblem,
  type Audience, type DocCategory, type DocMeta, type DocProductRef, type DocumentDetail, type TraceHit,
} from "@/lib/documentLibrary";

/** The library categories this user may see. */
export function useDocCategories() {
  const [categories, setCategories] = useState<DocCategory[]>([]);
  useEffect(() => {
    let alive = true;
    docLibApi.categories()
      .then((r) => { if (alive) setCategories(itemsOf(r.data).filter((c) => c.can_view !== false).sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0))); })
      .catch(() => { if (alive) setCategories([]); });
    return () => { alive = false; };
  }, []);
  return categories;
}

/** documents.max_file_mb, loaded once per dialog. */
function useMaxMb() {
  const [maxMb, setMaxMb] = useState(DOC_MAX_MB_DEFAULT);
  useEffect(() => { let alive = true; fetchMaxFileMb().then((v) => { if (alive) setMaxMb(v); }); return () => { alive = false; }; }, []);
  return maxMb;
}

/** 5 Nov 2026: nary.upload.video_max_mb, loaded once per dialog (null until known; the server checks too). */
function useVideoMaxMb() {
  const [mb, setMb] = useState<number | null>(null);
  useEffect(() => { let alive = true; fetchVideoMaxMb().then((v) => { if (alive) setMb(v); }); return () => { alive = false; }; }, []);
  return mb;
}

/** The file picker's hint: the two caps. */
function fileHint(maxMb: number, videoMaxMb: number | null) {
  return videoMaxMb
    ? `PDF or image up to ${maxMb} MB, or a video (MP4, MOV, WebM) up to ${videoMaxMb} MB.`
    : `PDF or image up to ${maxMb} MB, or a video (MP4, MOV, WebM).`;
}

/** "Needs executive approval" when a customer or partner audience is ticked. */
export function ApprovalNote({ audiences, className }: { audiences: readonly string[]; className?: string }) {
  if (!needsApproval(audiences)) {
    return <p className={cn("text-xs text-fg-muted", className)}>Internal only: a new version goes live as soon as it is uploaded.</p>;
  }
  return (
    <p role="status" className={cn("flex items-start gap-2 rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-3 py-2 text-[13px] text-fg", className)}>
      <ShieldCheck className="mt-0.5 size-4 shrink-0 text-tone-orange" aria-hidden />
      <span><span className="font-medium">Needs executive approval.</span> Customers or partners see it, so a new version waits for a director before it goes live.</span>
    </p>
  );
}

/** A file picker with drag and drop: the button, the chosen file and a way to remove it. */
function FilePick({ label, hint, accept, file, onFile, required, error }: {
  label: string; hint?: string; accept: string; file: File | null; onFile: (f: File | null) => void; required?: boolean; error?: string | null;
}) {
  const ref = useRef<HTMLInputElement>(null);
  const { dragActive, dropHandlers } = useFileDrop((files) => onFile(files[0] ?? null), { single: true });
  return (
    <Field label={label} hint={hint} required={required} error={error ?? undefined}>
      <div {...dropHandlers}
        className={cn("flex min-h-11 flex-wrap items-center gap-2 rounded-lg border border-dashed border-line-strong px-3 py-2",
          dragActive && "border-brand bg-brand/[0.07]")}>
        <input ref={ref} type="file" accept={accept} className="sr-only" aria-label={label}
          onChange={(e) => { onFile(e.target.files?.[0] ?? null); e.target.value = ""; }} />
        {file ? (
          <>
            <span className="min-w-0 flex-1 truncate text-sm text-fg">{file.name}</span>
            <span className="text-xs text-fg-muted tabular">{formatFileSize(file.size)}</span>
            <IconButton icon={X} size="sm" label={`Remove ${file.name}`} onClick={() => onFile(null)} />
          </>
        ) : (
          <>
            <Button size="sm" onClick={() => ref.current?.click()}><FileUp aria-hidden /> Choose file</Button>
            <span className="text-xs text-fg-muted">or drop it here</span>
          </>
        )}
      </div>
    </Field>
  );
}

/** 5 Nov 2026: a version may also be a video (the server judges the content). */
export const PDF_OR_IMAGE = "application/pdf,image/png,image/jpeg,image/webp,video/mp4,video/quicktime,video/webm,"
  + ".pdf,.png,.jpg,.jpeg,.webp,.mp4,.mov,.webm";
const OFFICE = ".docx,.xlsx,.pptx";

/** Search other documents (to mark this one a translation of it). */
function documentSource(excludeId?: string): OptionSource {
  const toOption = (d: { id: string; title: string; language: string }): Option =>
    ({ id: d.id, label: d.title, sublabel: languageLabel(d.language) });
  return {
    fetch: (q) => docLibApi.list({ view: "all", q: q || undefined, limit: 30 })
      .then((r) => itemsOf(r.data).filter((d) => d.id !== excludeId).map(toOption))
      .catch(() => []),
    resolve: (id) => docLibApi.get(id).then((r) => toOption(r.data)).catch(() => null),
  };
}

/** The document fields shared by "New document" and "Edit details". */
function MetaFields({ meta, setMeta, categories, excludeId, owners, errors }: {
  meta: DocMeta;
  setMeta: (fn: (m: DocMeta) => DocMeta) => void;
  categories: DocCategory[];
  excludeId?: string;
  /** Given when the user may reassign the owner. */
  owners?: { id: string; name: string }[];
  errors: Record<string, string>;
}) {
  const [family, setFamily] = useState("");
  const docSource = useMemo(() => documentSource(excludeId), [excludeId]);
  const toggleAudience = (a: Audience, on: boolean) =>
    setMeta((m) => ({ ...m, audiences: on ? [...m.audiences.filter((x) => x !== a), a] : m.audiences.filter((x) => x !== a) }));
  const addProduct = (p: DocProductRef) =>
    setMeta((m) => (m.products.some((x) => (p.id ? x.id === p.id : x.family === p.family)) ? m : { ...m, products: [...m.products, p] }));
  const removeProduct = (i: number) => setMeta((m) => ({ ...m, products: m.products.filter((_, j) => j !== i) }));

  return (
    <FieldGrid cols={2}>
      <Field label="Title" required error={errors.title} className="md:col-span-2">
        <Input value={meta.title} onChange={(e) => setMeta((m) => ({ ...m, title: e.target.value }))} autoComplete="off" placeholder="Product brochure" />
      </Field>
      <Field label="Description" hint="What it is for and when to use it." className="md:col-span-2">
        <Textarea rows={2} value={meta.description ?? ""} onChange={(e) => setMeta((m) => ({ ...m, description: e.target.value }))} />
      </Field>
      <Field label="Category" required error={errors.category}>
        <Select value={meta.category} onChange={(e) => setMeta((m) => ({ ...m, category: e.target.value }))}
          placeholder="Choose a category" options={categories.map((c) => ({ value: c.key, label: c.name }))} />
      </Field>
      <Field label="Language">
        <Select value={meta.language} onChange={(e) => setMeta((m) => ({ ...m, language: e.target.value }))}
          options={LANGUAGE_OPTIONS.some((o) => o.value === meta.language) ? LANGUAGE_OPTIONS
            : [...LANGUAGE_OPTIONS, { value: meta.language, label: languageLabel(meta.language) }]} />
      </Field>
      <Field label="Who it is for" required error={errors.audiences} className="md:col-span-2">
        <div className="flex flex-wrap gap-x-5 gap-y-2" role="group" aria-label="Who it is for">
          {AUDIENCES.map((a) => (
            <Checkbox key={a.value} label={a.label} checked={meta.audiences.includes(a.value)}
              onChange={(e) => toggleAudience(a.value, e.target.checked)} />
          ))}
        </div>
      </Field>
      <div className="md:col-span-2"><ApprovalNote audiences={meta.audiences} /></div>
      <Field label="Translation of" hint="Pick the same document in another language so they are kept as variants.">
        <Combobox value={meta.language_group_document_id ?? ""} allowClear placeholder="Not a translation"
          onChange={(id) => setMeta((m) => ({ ...m, language_group_document_id: id || null }))} source={docSource} />
      </Field>
      <Field label="Review by" hint="The owner is reminded before this date.">
        <Input type="date" value={meta.review_by ?? ""} onChange={(e) => setMeta((m) => ({ ...m, review_by: e.target.value || null }))} />
      </Field>
      <Field label="Products" hint="Link catalog products or a whole family; Nary picks documents by these." className="md:col-span-2">
        <div className="space-y-2">
          {meta.products.length > 0 && (
            <ul className="flex flex-wrap gap-1.5">
              {meta.products.map((p, i) => (
                <li key={`${p.id ?? ""}${p.family ?? ""}`} className="inline-flex items-center gap-1 rounded-full bg-surface-2 py-0.5 pl-2.5 pr-0.5 text-xs text-fg">
                  {productLabel(p)}
                  <IconButton icon={X} size="sm" label={`Remove ${productLabel(p)}`} onClick={() => removeProduct(i)} />
                </li>
              ))}
            </ul>
          )}
          <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
            <Combobox value="" placeholder="Add a product" source={catalogSource}
              onChange={(id, opt) => { if (id) addProduct({ id, name: opt?.label, sku: opt?.sublabel ?? null }); }} />
            <div className="flex gap-2">
              <Input value={family} onChange={(e) => setFamily(e.target.value)} placeholder="Or a family, e.g. Desks" aria-label="Product family"
                onKeyDown={(e) => { if (e.key === "Enter" && family.trim()) { e.preventDefault(); addProduct({ family: family.trim() }); setFamily(""); } }} />
              <Button onClick={() => { if (family.trim()) { addProduct({ family: family.trim() }); setFamily(""); } }} disabled={!family.trim()}>
                <Plus aria-hidden /> Add
              </Button>
            </div>
          </div>
        </div>
      </Field>
      {owners && (
        <Field label="Owner" hint="Keeps it up to date and gets the review reminders.">
          <Select value={meta.owner_id ?? ""} onChange={(e) => setMeta((m) => ({ ...m, owner_id: e.target.value }))}
            options={owners.map((u) => ({ value: u.id, label: u.name }))} />
        </Field>
      )}
    </FieldGrid>
  );
}

function validateMeta(meta: DocMeta): Record<string, string> {
  const e: Record<string, string> = {};
  if (!meta.title.trim()) e.title = "Give the document a title.";
  if (!meta.category) e.category = "Choose a category.";
  if (meta.audiences.length === 0) e.audiences = "Tick at least one.";
  return e;
}

const EMPTY_META: DocMeta = { title: "", description: "", category: "", audiences: ["internal"], language: "en", products: [], review_by: null, language_group_document_id: null };

/** "New document": the document and its first version in one go. */
export function NewDocumentDialog({ open, onOpenChange, categories, defaults }: {
  open: boolean; onOpenChange: (v: boolean) => void; categories: DocCategory[]; defaults?: Partial<DocMeta>;
}) {
  const router = useRouter();
  const maxMb = useMaxMb();
  const videoMaxMb = useVideoMaxMb();
  const [meta, setMetaState] = useState<DocMeta>({ ...EMPTY_META, ...defaults });
  const [label, setLabel] = useState("v1");
  const [note, setNote] = useState("First version");
  const [file, setFile] = useState<File | null>(null);
  const [source, setSource] = useState<File | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const setMeta = (fn: (m: DocMeta) => DocMeta) => setMetaState(fn);

  useEffect(() => {
    if (open) {
      setMetaState({ ...EMPTY_META, ...defaults });
      setLabel("v1"); setNote("First version"); setFile(null); setSource(null); setErrors({});
    }
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  const fileError = file || source ? uploadProblem(file, source, maxMb, videoMaxMb ?? undefined) : null;

  async function save() {
    const e = validateMeta(meta);
    if (!label.trim()) e.label = "Give the version a label.";
    if (!note.trim()) e.note = "Say what this version is.";
    const fp = uploadProblem(file, source, maxMb, videoMaxMb ?? undefined);
    if (fp) e.file = fp;
    setErrors(e);
    if (Object.keys(e).length) return;
    setSaving(true);
    try {
      const res = await docLibApi.create({ ...meta, title: meta.title.trim(), label: label.trim(), change_note: note.trim() }, file!, source);
      toast.success(needsApproval(meta.audiences) ? "Document created, first version sent for approval" : "Document created");
      onOpenChange(false);
      if (res.data?.id) router.push(`/knowledge/documents/${res.data.id}`);
    } catch (err) {
      toast.error(extractApiError(err, "The document could not be created."));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange} size="lg" dismissable={!saving} title="New document"
      description="Upload the PDF or image people will see; attach the Word, Excel or PowerPoint source if there is one."
      footer={<>
        <Button onClick={() => onOpenChange(false)} disabled={saving}>Cancel</Button>
        <Button variant="primary" loading={saving} onClick={save}>Create document</Button>
      </>}>
      <div className="space-y-5">
        <MetaFields meta={meta} setMeta={setMeta} categories={categories} errors={errors} />
        <div className="border-t border-line pt-4">
          <h3 className="mb-3 text-sm font-semibold text-fg">First version</h3>
          <FieldGrid cols={2}>
            <Field label="Version label" required error={errors.label} hint="As you name it, e.g. v1 or v2.0.">
              <Input value={label} onChange={(e) => setLabel(e.target.value)} autoComplete="off" />
            </Field>
            <Field label="Change note" required error={errors.note}>
              <Input value={note} onChange={(e) => setNote(e.target.value)} autoComplete="off" />
            </Field>
            <FilePick label="File" hint={fileHint(maxMb, videoMaxMb)} accept={PDF_OR_IMAGE} file={file} onFile={setFile} required
              error={errors.file ?? fileError} />
            <FilePick label="Source file (optional)" hint="DOCX, XLSX or PPTX. Never sent to customers." accept={OFFICE} file={source} onFile={setSource} />
          </FieldGrid>
        </div>
      </div>
    </Dialog>
  );
}

/** "Edit details": title, category, audiences, language, products, review date, owner. */
export function EditDocumentDialog({ open, onOpenChange, doc, categories, onSaved }: {
  open: boolean; onOpenChange: (v: boolean) => void; doc: DocumentDetail; categories: DocCategory[]; onSaved: () => void;
}) {
  const [meta, setMetaState] = useState<DocMeta>(EMPTY_META);
  const [owners, setOwners] = useState<{ id: string; name: string }[] | undefined>(undefined);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const setMeta = (fn: (m: DocMeta) => DocMeta) => setMetaState(fn);
  const canReassign = !!doc.permissions?.reassign;

  useEffect(() => {
    if (!open) return;
    setErrors({});
    setMetaState({
      title: doc.title, description: doc.description ?? "", category: doc.category?.key ?? "", audiences: [...doc.audiences],
      language: doc.language || "en", products: [...(doc.products ?? [])], review_by: doc.review_by ?? null,
      language_group_document_id: doc.language_variants?.[0]?.id ?? null, owner_id: doc.owner?.id,
    });
    if (canReassign) {
      // The staff directory (any internal user may read it), not the admin users list (403 for staff).
      peopleApi.directory().then((r) => {
        setOwners((r.data ?? []).map((u) => ({ id: String(u.id), name: u.name || "User" })));
      }).catch(() => setOwners(doc.owner ? [doc.owner] : []));
    }
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  async function save() {
    const e = validateMeta(meta);
    setErrors(e);
    if (Object.keys(e).length) return;
    setSaving(true);
    try {
      const { owner_id, ...rest } = meta;
      const body: Partial<DocMeta> = { ...rest, title: meta.title.trim() };
      if (canReassign && owner_id && owner_id !== doc.owner?.id) body.owner_id = owner_id;
      await docLibApi.update(doc.id, body);
      toast.success("Document saved");
      onOpenChange(false);
      onSaved();
    } catch (err) {
      toast.error(extractApiError(err, "The document could not be saved."));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange} size="lg" dismissable={!saving} title="Edit details"
      footer={<>
        <Button onClick={() => onOpenChange(false)} disabled={saving}>Cancel</Button>
        <Button variant="primary" loading={saving} onClick={save}>Save details</Button>
      </>}>
      <MetaFields meta={meta} setMeta={setMeta} categories={categories} excludeId={doc.id}
        owners={canReassign ? (owners ?? (doc.owner ? [doc.owner] : [])) : undefined} errors={errors} />
    </Dialog>
  );
}

/** "Upload new version": label suggested by the server, a change note, the file and an optional source. */
export function UploadVersionDialog({ open, onOpenChange, doc, onUploaded }: {
  open: boolean; onOpenChange: (v: boolean) => void;
  doc: { id: string; title: string; audiences: readonly string[]; current_version?: { label: string } | null };
  onUploaded: () => void;
}) {
  const maxMb = useMaxMb();
  const videoMaxMb = useVideoMaxMb();
  const [label, setLabel] = useState("");
  const [note, setNote] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [source, setSource] = useState<File | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;
    let alive = true;
    setLabel(""); setNote(""); setFile(null); setSource(null); setErrors({});
    fetchSuggestedLabel(doc.id, doc.current_version?.label).then((l) => { if (alive) setLabel((cur) => cur || l); });
    return () => { alive = false; };
  }, [open, doc.id, doc.current_version?.label]);

  const fileError = file || source ? uploadProblem(file, source, maxMb, videoMaxMb ?? undefined) : null;

  async function save() {
    const e: Record<string, string> = {};
    if (!label.trim()) e.label = "Give the version a label.";
    if (!note.trim()) e.note = "Say what changed; it is kept with the version.";
    const fp = uploadProblem(file, source, maxMb, videoMaxMb ?? undefined);
    if (fp) e.file = fp;
    setErrors(e);
    if (Object.keys(e).length) return;
    setSaving(true);
    try {
      await docLibApi.uploadVersion(doc.id, { label: label.trim(), change_note: note.trim(), file: file!, source });
      toast.success(needsApproval(doc.audiences) ? `${label.trim()} sent for approval` : `${label.trim()} is live`);
      onOpenChange(false);
      onUploaded();
    } catch (err) {
      toast.error(extractApiError(err, "The version could not be uploaded."));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange} size="md" dismissable={!saving} title="Upload new version" description={doc.title}
      footer={<>
        <Button onClick={() => onOpenChange(false)} disabled={saving}>Cancel</Button>
        <Button variant="primary" loading={saving} onClick={save}>
          {needsApproval(doc.audiences) ? "Upload and send for approval" : "Upload and publish"}
        </Button>
      </>}>
      <div className="space-y-4">
        <ApprovalNote audiences={doc.audiences} />
        <FieldGrid cols={1}>
          <Field label="Version label" required error={errors.label}
            hint={doc.current_version ? `The live version is ${doc.current_version.label}. Change the suggestion if you number differently.` : "Suggested from the last version."}>
            <Input value={label} onChange={(e) => setLabel(e.target.value)} autoComplete="off" />
          </Field>
          <Field label="Change note" required error={errors.note} hint="What changed, in one or two lines. Everyone sees it.">
            <Textarea rows={2} value={note} onChange={(e) => setNote(e.target.value)} />
          </Field>
          <FilePick label="File" hint={fileHint(maxMb, videoMaxMb)} accept={PDF_OR_IMAGE} file={file} onFile={setFile} required
            error={errors.file ?? fileError} />
          <FilePick label="Source file (optional)" hint="DOCX, XLSX or PPTX; needs the PDF above. Never sent to customers."
            accept={OFFICE} file={source} onFile={setSource} />
        </FieldGrid>
      </div>
    </Dialog>
  );
}

const KIND_LABEL: Record<string, string> = {
  staff_download: "Staff download", customer_send: "Sent to a customer", partner_download: "Partner download",
  nary_send: "Sent by Nary", source_download: "Source file download",
};

/** "Trace a leaked copy": drop the file found outside, or type the ID from its footer. */
export function TraceDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (v: boolean) => void }) {
  const [file, setFile] = useState<File | null>(null);
  const [id, setId] = useState("");
  const [busy, setBusy] = useState(false);
  const [hit, setHit] = useState<TraceHit | null | undefined>(undefined);

  useEffect(() => { if (open) { setFile(null); setId(""); setHit(undefined); } }, [open]);

  async function trace() {
    setBusy(true);
    setHit(undefined);
    try {
      const res = file ? await docLibApi.traceFile(file) : await docLibApi.traceId(id.trim().toUpperCase());
      setHit(res.data ?? null);
    } catch (err) {
      const status = (err as { response?: { status?: number } }).response?.status;
      if (status === 404) setHit(null);
      else toast.error(extractApiError(err, "The copy could not be traced."));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange} size="md" title="Trace a leaked copy"
      description="Find who a copy of a company document was given to."
      footer={<>
        <Button onClick={() => onOpenChange(false)}>Close</Button>
        <Button variant="primary" loading={busy} disabled={!file && !id.trim()} onClick={trace}><Search aria-hidden /> Trace</Button>
      </>}>
      <div className="space-y-4">
        <FilePick label="The copy you found" hint="PDF, image, or a Word, Excel or PowerPoint source." accept={`${PDF_OR_IMAGE},${OFFICE}`}
          file={file} onFile={(f) => { setFile(f); if (f) setId(""); }} />
        <Field label="Or the ID from its footer" hint="Starts with ID-, e.g. ID-7K2M9QX4TB.">
          <Input value={id} onChange={(e) => { setId(e.target.value); if (e.target.value) setFile(null); }} placeholder="ID-" className="font-mono" autoComplete="off" />
        </Field>
        <p className="text-xs text-fg-muted">{WATERMARK_LIMIT}</p>
        {hit === null && (
          <p role="status" className="flex items-start gap-2 rounded-lg border border-line bg-surface-2 px-3 py-2 text-[13px] text-fg">
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-tone-orange" aria-hidden />
            No fingerprint found. The copy may have been retyped or screenshotted, or it did not come from this library.
          </p>
        )}
        {hit && (
          <div role="status" className="rounded-lg border border-line bg-surface-2 p-3">
            <div className="mb-2 flex flex-wrap items-center gap-2">
              <span className="font-mono text-xs text-fg">{hit.fingerprint}</span>
              <Pill tone="violet">{KIND_LABEL[hit.kind] ?? hit.kind}</Pill>
            </div>
            <KeyValues items={[
              { label: "Given to", value: traceRecipient(hit) },
              { label: "When", value: hit.created_at ? new Date(hit.created_at).toLocaleString("en-GB", { day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit" }) : "-", numeric: true },
              { label: "Document", value: hit.document?.name ?? "-" },
              { label: "Version", value: hit.version?.label ?? "-" },
              { label: "Channel", value: hit.channel ? hit.channel.charAt(0).toUpperCase() + hit.channel.slice(1) : "-" },
              { label: "Found by", value: matchedByLabel(hit.matched_by) },
            ]} />
            {hit.note && <p role="note" className="mt-2 text-xs text-fg-muted">{hit.note}</p>}
          </div>
        )}
      </div>
    </Dialog>
  );
}

