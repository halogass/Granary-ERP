"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  AlertTriangle, CheckCircle2, FileText, FileType2, Loader2, Pencil, RefreshCw, Search, Trash2, Upload, X,
} from "lucide-react";
import {
  Button, Card, CardBody, CardHeader, Checkbox, Dialog, EmptyState, Field, IconButton, Input, Pill, Skeleton, Textarea,
} from "@/components/ui";
import { aiKnowledgeApi, type AiKnowledgeDoc } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { useFileDrop } from "@/hooks/useFileDrop";
import { cn } from "@/lib/utils";

const MD_EXT = [".md", ".markdown", ".txt"];
const PDF_MAX = 25 * 1024 * 1024;
const MD_MAX = 5 * 1024 * 1024;
const ACCEPT = ".md,.markdown,.txt,.pdf,application/pdf,text/markdown";

/** Why a file can't be added, or null when it can. Mirrors the server's checks. */
export function rejectReason(f: File): string | null {
  const name = f.name.toLowerCase();
  if (name.endsWith(".pdf")) return f.size > PDF_MAX ? "PDF is larger than 25 MB" : null;
  if (MD_EXT.some((e) => name.endsWith(e))) return f.size > MD_MAX ? "Markdown file is larger than 5 MB" : null;
  return "Only Markdown (.md) and PDF files";
}

function titleFrom(name: string) {
  return name.replace(/\.[^.]+$/, "").replace(/[_-]+/g, " ").trim().slice(0, 300) || name;
}

function size(n?: number | null) {
  if (!n) return "";
  return n > 1024 * 1024 ? `${(n / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;
}

/**
 * Settings, Nary & AI, Knowledge: documents only the AI reads.
 *
 * Employees have no page for these: the assistant, reply suggestions, drafting, scoring, record
 * briefs and connected agents use them, and may quote them. Admins upload, edit and remove them here.
 */
export default function AiKnowledgeTab() {
  const [docs, setDocs] = useState<AiKnowledgeDoc[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [adding, setAdding] = useState<null | "file" | "paste">(null);
  const [dropped, setDropped] = useState<File[]>([]);
  // Drop files anywhere on the tab: they open in the upload dialog, ready to add.
  const { dragActive, dropHandlers } = useFileDrop((files) => {
    setDropped(files);
    setAdding("file");
  }, { disabled: adding !== null });
  const [editing, setEditing] = useState<AiKnowledgeDoc | null>(null);

  const load = useCallback(async () => {
    try {
      const r = await aiKnowledgeApi.list();
      setDocs(r.data.items ?? []);
      setError("");
    } catch (e) {
      setError(extractApiError(e, "Could not load the AI knowledge."));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => { load(); }, [load]);

  // A PDF converts on a worker: keep the list fresh until nothing is processing.
  useEffect(() => {
    if (!docs.some((d) => d.status === "processing")) return;
    const t = setTimeout(load, 4000);
    return () => clearTimeout(t);
  }, [docs, load]);

  async function toggleSafe(doc: AiKnowledgeDoc) {
    setDocs((ds) => ds.map((d) => (d.id === doc.id ? { ...d, customer_safe: !d.customer_safe } : d)));
    try { await aiKnowledgeApi.update(doc.id, { customer_safe: !doc.customer_safe }); } catch { load(); }
  }

  async function toggle(doc: AiKnowledgeDoc) {
    setDocs((ds) => ds.map((d) => (d.id === doc.id ? { ...d, is_active: !d.is_active } : d)));
    try { await aiKnowledgeApi.update(doc.id, { is_active: !doc.is_active }); } catch { load(); }
  }

  return (
    <div className="relative space-y-4" {...dropHandlers}>
      {dragActive && (
        <div className="pointer-events-none absolute inset-0 z-40 flex items-center justify-center rounded-lg border-2 border-dashed border-brand/50 bg-surface/90">
          <p className="flex items-center gap-2 text-sm font-medium text-brand-ink">
            <Upload className="size-4" aria-hidden /> Drop Markdown or PDF files to add them to the AI knowledge
          </p>
        </div>
      )}
      <Card>
        <CardHeader title="AI knowledge"
          description="Documents for the AI only: pricing and discount rules, product facts, playbooks, how you work. They are not part of the Knowledge Base and employees can't browse them, but the AI uses them in the assistant, reply suggestions, drafting, lead scoring, record briefs and connected agents, and may quote them in its answers. Markdown or PDF."
          actions={<>
            <Button onClick={() => setAdding("paste")}><Pencil aria-hidden /> Write Markdown</Button>
            <Button variant="primary" onClick={() => setAdding("file")}><Upload aria-hidden /> Upload files</Button>
          </>} />
        {error && <p className="mx-4 mt-3 rounded-lg bg-tone-red-soft px-3 py-2 text-sm text-tone-red">{error}</p>}
        {loading ? (
          <div className="space-y-2 p-4"><Skeleton className="h-4 w-56" /><Skeleton className="h-4 w-80" /></div>
        ) : docs.length === 0 ? (
          <EmptyState compact icon={FileText} title="No AI knowledge yet"
            description="Drop Markdown or PDF files here, or use Upload files." />
        ) : (
          <ul className="divide-y divide-line">
            {docs.map((d) => (
              <DocRow key={d.id} doc={d} onToggle={() => toggle(d)} onToggleSafe={() => toggleSafe(d)} onOpen={() => setEditing(d)} onChanged={load} />
            ))}
          </ul>
        )}
      </Card>

      <TestSearch />

      {adding === "paste" && <PasteDialog onClose={() => setAdding(null)} onAdded={() => { setAdding(null); load(); }} />}
      {adding === "file" && (
        <UploadDialog initial={dropped} onChanged={load}
          onClose={() => { setAdding(null); setDropped([]); load(); }} />
      )}
      {editing && <EditDialog id={editing.id} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); load(); }} />}
    </div>
  );
}

function DocRow({ doc, onToggle, onToggleSafe, onOpen, onChanged }: {
  doc: AiKnowledgeDoc; onToggle: () => void; onToggleSafe: () => void; onOpen: () => void; onChanged: () => void;
}) {
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const Icon = doc.source_type === "pdf" ? FileType2 : FileText;

  async function reindex() {
    setBusy(true);
    try { await aiKnowledgeApi.reindex(doc.id); } finally { setBusy(false); onChanged(); }
  }
  async function remove() {
    setBusy(true);
    try { await aiKnowledgeApi.remove(doc.id); } finally { setBusy(false); onChanged(); }
  }

  return (
    <li className="flex flex-wrap items-start gap-3 px-4 py-3">
      <div className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-fg-muted">
        <Icon className="size-4" aria-hidden />
      </div>
      <div className="min-w-0 flex-1">
        <button type="button" onClick={onOpen} className="text-left text-sm font-medium text-fg hover:text-brand-ink">{doc.title}</button>
        {doc.system && (
          <Pill className="ml-2" title="Built from the ERP's code and settings, kept up to date automatically. It can be switched off, not edited.">
            ERP guide (auto)
          </Pill>
        )}
        {doc.description && <p className="mt-0.5 line-clamp-2 text-xs text-fg-muted">{doc.description}</p>}
        <p className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-fg-muted">
          {doc.status === "processing" && <span className="inline-flex items-center gap-1 text-tone-orange"><Loader2 className="size-3 animate-spin" aria-hidden /> Processing</span>}
          {doc.status === "ready" && <span className="inline-flex items-center gap-1 text-tone-blue"><CheckCircle2 className="size-3" aria-hidden /> Ready</span>}
          {doc.status === "failed" && <span className="inline-flex items-center gap-1 text-tone-red"><AlertTriangle className="size-3" aria-hidden /> Failed</span>}
          <span>{doc.source_type === "pdf" ? "PDF" : "Markdown"}{doc.original_size ? ` · ${size(doc.original_size)}` : ""}</span>
          <span className="tabular">{doc.characters.toLocaleString()} characters · {doc.chunk_count} passages</span>
          <span className="tabular">Used {doc.use_count}×</span>
        </p>
        {doc.status === "failed" && doc.error && <p className="mt-1 text-xs text-tone-red">{doc.error}</p>}
      </div>
      <div className="flex shrink-0 items-center gap-3">
        {!doc.system && (
          <span title="Nary may use it when talking to a customer. Leave off for anything with costs, tiers, margins or internal rules.">
            <Checkbox label="Customer-safe" className="text-xs text-fg-muted" checked={!!doc.customer_safe} onChange={onToggleSafe} />
          </span>
        )}
        <span title="Off: kept, but the AI doesn't use it">
          <Checkbox label={doc.is_active ? "On" : "Off"} className="text-xs text-fg-muted" checked={doc.is_active} onChange={onToggle} />
        </span>
        <IconButton size="sm" label="Re-index (e.g. after changing the embedding model)" icon={RefreshCw} onClick={reindex} disabled={busy}
          className={busy ? "[&_svg]:animate-spin" : undefined} />
        {doc.system ? null : confirm ? (
          <span className="flex items-center gap-1">
            <Button size="sm" variant="danger" onClick={remove} disabled={busy}>Delete</Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirm(false)}>Cancel</Button>
          </span>
        ) : (
          <IconButton size="sm" label={`Delete ${doc.title}`} icon={Trash2} onClick={() => setConfirm(true)} />
        )}
      </div>
    </li>
  );
}

function Modal({ title, onClose, dismissable = true, footer, children }: {
  title: string; onClose: () => void; dismissable?: boolean; footer?: React.ReactNode; children: React.ReactNode;
}) {
  return (
    <Dialog open size="lg" title={title} dismissable={dismissable} footer={footer}
      onOpenChange={(o) => { if (!o) onClose(); }}>
      {/* A file dropped outside a drop zone must not make the browser open it instead. */}
      <div onDragOver={(e) => e.preventDefault()} onDrop={(e) => e.preventDefault()}>{children}</div>
    </Dialog>
  );
}

type Item = { key: string; file: File; title: string; status: "waiting" | "uploading" | "done" | "failed"; error?: string };

/** Several Markdown/PDF files at once: queued, titled from their names, uploaded one by one. */
function UploadDialog({ initial, onClose, onChanged }: { initial: File[]; onClose: () => void; onChanged: () => void }) {
  const [items, setItems] = useState<Item[]>([]);
  const [skipped, setSkipped] = useState<{ name: string; reason: string }[]>([]);
  const [description, setDescription] = useState("");
  const [running, setRunning] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  const addFiles = useCallback((files: File[]) => {
    const ok: Item[] = [];
    const bad: { name: string; reason: string }[] = [];
    for (const f of files) {
      const reason = rejectReason(f);
      if (reason) bad.push({ name: f.name, reason });
      else ok.push({ key: `${f.name}-${f.size}-${f.lastModified}-${Math.random()}`, file: f, title: titleFrom(f.name), status: "waiting" });
    }
    setItems((prev) => {
      const seen = new Set(prev.map((p) => `${p.file.name}/${p.file.size}`));
      return [...prev, ...ok.filter((i) => !seen.has(`${i.file.name}/${i.file.size}`))];
    });
    setSkipped((prev) => [...prev, ...bad]);
  }, []);

  useEffect(() => { if (initial.length) addFiles(initial); }, [initial, addFiles]);
  const { dragActive, dropHandlers } = useFileDrop(addFiles, { disabled: running });

  const pending = items.filter((i) => i.status === "waiting" || i.status === "failed");
  const set = (key: string, patch: Partial<Item>) =>
    setItems((prev) => prev.map((i) => (i.key === key ? { ...i, ...patch } : i)));

  async function uploadAll() {
    setRunning(true);
    // One at a time: a PDF is converted on upload, and several at once would queue on the converter.
    for (const item of pending) {
      set(item.key, { status: "uploading", error: undefined });
      const form = new FormData();
      form.append("title", item.title);
      form.append("description", description);
      form.append("file", item.file);
      try {
        await aiKnowledgeApi.upload(form);
        set(item.key, { status: "done" });
      } catch (e) {
        set(item.key, { status: "failed", error: extractApiError(e, "Upload failed") });
      }
    }
    setRunning(false);
    onChanged();
  }

  const allDone = items.length > 0 && items.every((i) => i.status === "done");
  useEffect(() => { if (allDone) onClose(); }, [allDone, onClose]);

  return (
    <Modal title="Upload documents for the AI" onClose={onClose} dismissable={!running}
      footer={<>
        <Button onClick={onClose} disabled={running}>{items.some((i) => i.status === "done") ? "Close" : "Cancel"}</Button>
        <Button variant="primary" onClick={uploadAll} loading={running}
          disabled={pending.length === 0 || pending.some((i) => !i.title.trim())}>
          {running ? "Uploading..." : pending.some((i) => i.status === "failed") ? `Retry ${pending.length}`
            : `Add ${pending.length} document${pending.length === 1 ? "" : "s"}`}
        </Button>
      </>}>
      <div className="space-y-3">
        <input ref={input} type="file" multiple accept={ACCEPT} className="hidden" aria-label="Choose files"
          onChange={(e) => { addFiles(Array.from(e.target.files ?? [])); e.target.value = ""; }} />
        <div {...dropHandlers} onClick={() => !running && input.current?.click()} role="button" tabIndex={0}
          onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && input.current?.click()}
          className={cn("flex cursor-pointer flex-col items-center justify-center gap-1 rounded-lg border-2 border-dashed px-4 py-6 text-center text-sm transition-colors",
            dragActive ? "border-brand bg-brand/5 text-brand-ink" : "border-line-strong text-fg-muted hover:border-brand/40 hover:text-fg")}>
          <Upload className="size-5" aria-hidden />
          <span className="font-medium">Drop files here or click to choose</span>
          <span className="text-xs text-fg-subtle">Markdown (.md) up to 5 MB · PDF up to 25 MB · several at once</span>
        </div>

        {items.length > 0 && (
          <ul className="divide-y divide-line rounded-lg border border-line">
            {items.map((i) => (
              <li key={i.key} className="flex items-center gap-2 px-3 py-2">
                {i.file.name.toLowerCase().endsWith(".pdf")
                  ? <FileType2 className="size-4 shrink-0 text-fg-subtle" aria-hidden />
                  : <FileText className="size-4 shrink-0 text-fg-subtle" aria-hidden />}
                <div className="min-w-0 flex-1">
                  <Input value={i.title} disabled={i.status === "uploading" || i.status === "done"}
                    onChange={(e) => set(i.key, { title: e.target.value })} aria-label={`Title for ${i.file.name}`} />
                  <p className="mt-0.5 truncate text-xs text-fg-muted">
                    {i.file.name} · {size(i.file.size)}
                    {i.status === "failed" && i.error && <span className="text-tone-red"> · {i.error}</span>}
                  </p>
                </div>
                {i.status === "uploading" && <Loader2 className="size-4 shrink-0 animate-spin text-tone-orange" aria-label="Uploading" />}
                {i.status === "done" && <CheckCircle2 className="size-4 shrink-0 text-tone-blue" aria-label="Uploaded" />}
                {i.status === "failed" && <AlertTriangle className="size-4 shrink-0 text-tone-red" aria-label="Failed" />}
                {(i.status === "waiting" || i.status === "failed") && !running && (
                  <IconButton size="sm" label={`Remove ${i.file.name}`} icon={X}
                    onClick={() => setItems((prev) => prev.filter((p) => p.key !== i.key))} />
                )}
              </li>
            ))}
          </ul>
        )}

        {skipped.length > 0 && (
          <div className="rounded-lg bg-tone-orange-soft px-3 py-2 text-xs text-tone-orange">
            <p className="font-medium">Not added:</p>
            {skipped.map((s, n) => <p key={n}>{s.name}: {s.reason}</p>)}
          </div>
        )}

        <Field label="When they apply (optional, for all of these)">
          <Input value={description} onChange={(e) => setDescription(e.target.value)} disabled={running}
            placeholder="e.g. Any quotation, price or discount question" />
        </Field>
        <p className="text-xs text-fg-muted">
          A PDF is converted to text for the AI; with AI document conversion on, tables and headings are kept.
        </p>
      </div>
    </Modal>
  );
}

function PasteDialog({ onClose, onAdded }: { onClose: () => void; onAdded: () => void }) {
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [content, setContent] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  async function save() {
    setSaving(true);
    setError("");
    const form = new FormData();
    form.append("title", title);
    form.append("description", description);
    form.append("content", content);
    try {
      await aiKnowledgeApi.upload(form);
      onAdded();
    } catch (e) {
      setError(extractApiError(e, "Could not add the document."));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal title="Write Markdown for the AI" onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" onClick={save} loading={saving} disabled={!title.trim() || !content.trim()}>Add</Button>
      </>}>
      <div className="space-y-3">
        <Field label="Title">
          <Input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="e.g. Discount and pricing rules" />
        </Field>
        <Field label="When it applies (optional)">
          <Input value={description} onChange={(e) => setDescription(e.target.value)}
            placeholder="e.g. Any quotation, price or discount question" />
        </Field>
        <Field label="Content (Markdown)" error={error || undefined}>
          <Textarea value={content} onChange={(e) => setContent(e.target.value)} rows={14} className="font-mono text-xs"
            placeholder={"# Discount rules\n\n- Distributors: 12% off list\n- Above 20% needs Owner approval"} />
        </Field>
      </div>
    </Modal>
  );
}

function EditDialog({ id, onClose, onSaved }: { id: string; onClose: () => void; onSaved: () => void }) {
  const [doc, setDoc] = useState<AiKnowledgeDoc | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    aiKnowledgeApi.get(id).then((r) => setDoc(r.data)).catch((e) => setError(extractApiError(e, "Could not open it.")));
  }, [id]);

  async function save() {
    if (!doc) return;
    setSaving(true);
    setError("");
    try {
      await aiKnowledgeApi.update(id, { title: doc.title, description: doc.description ?? "", content: doc.content ?? "" });
      onSaved();
    } catch (e) {
      setError(extractApiError(e, "Could not save."));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal title="What the AI reads" onClose={onClose}
      footer={doc ? <>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" onClick={save} loading={saving} disabled={!(doc.content ?? "").trim()}>Save and re-index</Button>
      </> : undefined}>
      {!doc ? (
        error ? <p className="text-sm text-tone-red">{error}</p>
              : <div className="space-y-2"><Skeleton className="h-4 w-48" /><Skeleton className="h-24 w-full" /></div>
      ) : (
        <div className="space-y-3">
          <Field label="Title">
            <Input value={doc.title} onChange={(e) => setDoc({ ...doc, title: e.target.value })} />
          </Field>
          <Field label="When it applies (optional)">
            <Input value={doc.description ?? ""} onChange={(e) => setDoc({ ...doc, description: e.target.value })} />
          </Field>
          {doc.source_type === "pdf" && (
            <p className="text-xs text-fg-muted">
              Converted from {doc.original_filename}. Fix anything the conversion got wrong: the AI reads this text.
            </p>
          )}
          <Field label="Content" error={error || undefined}>
            <Textarea value={doc.content ?? ""} onChange={(e) => setDoc({ ...doc, content: e.target.value })}
              rows={18} className="font-mono text-xs" />
          </Field>
        </div>
      )}
    </Modal>
  );
}

/** "What would the AI find for this question?": to check a document is picked up. */
function TestSearch() {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<{ title: string; content: string; distance: number; used: boolean }[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function run() {
    if (!query.trim()) return;
    setBusy(true);
    setError("");
    try {
      setResults((await aiKnowledgeApi.search(query)).data.results);
    } catch (e) {
      setError(extractApiError(e, "Search failed."));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <CardHeader title="Test: what would the AI find?"
        description={<>Ask a question the way a user would. Passages marked &ldquo;added automatically&rdquo; are close enough to be
          put into an answer&apos;s context without the AI asking; the rest it finds only when it searches.</>} />
      <CardBody>
        <div className="flex gap-2">
          <div className="min-w-0 flex-1">
            <Input type="search" autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other" aria-label="Test question"
              value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === "Enter" && run()}
              placeholder="e.g. What discount can a distributor get?" />
          </div>
          <Button onClick={run} loading={busy} disabled={!query.trim()}>{!busy && <Search aria-hidden />} Search</Button>
        </div>
        {error && <p className="mt-2 text-xs text-tone-red">{error}</p>}
        {results && (
          results.length === 0 ? <p className="mt-3 text-xs text-fg-muted">Nothing found.</p> : (
            <ul className="mt-3 space-y-2">
              {results.map((r, i) => (
                <li key={i} className="rounded-lg bg-surface-2 px-3 py-2">
                  <p className="flex flex-wrap items-center gap-2 text-xs font-medium text-fg">
                    {r.title}
                    <Pill tone={r.used ? "blue" : "neutral"}>{r.used ? "added automatically" : "on search only"}</Pill>
                    <span className="text-[11px] font-normal text-fg-muted tabular">match {Math.round((1 - r.distance) * 100)}%</span>
                  </p>
                  <p className="mt-1 line-clamp-4 whitespace-pre-line text-xs text-fg-muted">{r.content}</p>
                </li>
              ))}
            </ul>
          )
        )}
      </CardBody>
    </Card>
  );
}
