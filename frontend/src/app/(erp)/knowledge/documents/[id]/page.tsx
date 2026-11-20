"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import {
  BookOpen, ChevronDown, Download, Edit3, Eye, FileCode2, FileText, History, Languages, Link2, Package, RotateCcw,
  ScanSearch, Trash2, Upload, X,
} from "lucide-react";
import {
  Button, Card, CardBody, CardHeader, Combobox, Dialog, DropdownMenu, EmptyState, IconButton, KeyValues, MenuItem,
  PageHeader, Pill, Skeleton, StatusPill, toast,
} from "@/components/ui";
import type { Option, OptionSource } from "@/components/ui/SearchSelect";
import ApprovalPanel from "@/components/approvals/ApprovalPanel";
import DocumentViewer from "@/components/documents/DocumentViewer";
import {
  EditDocumentDialog, TraceDialog, UploadVersionDialog, useDocCategories,
} from "@/components/documents/DocumentDialogs";
import { usePermissions } from "@/hooks/usePermissions";
import { kbApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { cn } from "@/lib/utils";
import {
  WATERMARK_LIMIT, audienceLabel, docLibApi, downloadStamped, formatDuration, formatFileSize, itemsOf, languageLabel, personName,
  productLabel, versionStatusLabel, type DocPermissions, type DocVersion, type DocumentDetail,
} from "@/lib/documentLibrary";

const fmtDate = (d?: string | null) => (d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-");
const audienceTone = (a: string) => (a === "internal" ? "neutral" : a === "partner" ? "violet" : "blue");

// The articles list filters in the browser (as the Articles page does).
const articleSource: OptionSource = {
  fetch: (q) => kbApi.listArticles({ limit: 1000 })
    .then((r) => itemsOf<{ id: string; title: string; sop_code?: string }>(r.data)
      .filter((a) => !q || `${a.title} ${a.sop_code ?? ""}`.toLowerCase().includes(q.toLowerCase()))
      .slice(0, 50)
      .map((a): Option => ({ id: a.id, label: a.title, sublabel: a.sop_code || undefined })))
    .catch(() => []),
};

export default function DocumentPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const { can } = usePermissions();
  const categories = useDocCategories();
  const [doc, setDoc] = useState<DocumentDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [notFound, setNotFound] = useState(false);
  const [viewing, setViewing] = useState<string | undefined>(undefined);
  const [showViewer, setShowViewer] = useState(true);
  const [uploading, setUploading] = useState(false);
  const [editing, setEditing] = useState(false);
  const [tracing, setTracing] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [restoring, setRestoring] = useState<DocVersion | null>(null);
  const [linking, setLinking] = useState(false);
  const [approvalKey, setApprovalKey] = useState(0);

  const load = useCallback(async () => {
    try {
      const res = await docLibApi.get(id);
      setDoc(res.data);
      setNotFound(false);
    } catch (e) {
      const status = (e as { response?: { status?: number } }).response?.status;
      if (status === 404 || status === 403) setNotFound(true);
      else toast.error(extractApiError(e, "The document could not be loaded."));
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => { void load(); }, [load]);

  const perms: DocPermissions = doc?.permissions ?? { edit: false, approve: false, history: false, reassign: false, trace: false };
  const versions = doc?.versions ?? null;
  const viewed = useMemo(() => (viewing && versions ? versions.find((v) => v.id === viewing) : undefined), [viewing, versions]);
  const articles = doc?.articles ?? doc?.linked_articles ?? [];
  const current = doc?.current_version ?? null;

  async function download(opts: { version?: string; source?: boolean; name?: string } = {}) {
    if (!doc) return;
    try {
      await downloadStamped(() => docLibApi.download(doc.id, opts), opts.name ?? current?.file_name ?? `${doc.title}.pdf`);
    } catch (e) {
      toast.error(extractApiError(e, "The download failed."));
    }
  }

  async function remove() {
    if (!doc) return;
    setDeleting(true);
    try {
      await docLibApi.remove(doc.id);
      toast.success("Document deleted");
      router.push("/knowledge/documents");
    } catch (e) {
      toast.error(extractApiError(e, "The document could not be deleted."));
      setDeleting(false);
    }
  }

  async function restore(v: DocVersion) {
    if (!doc) return;
    try {
      await docLibApi.restore(doc.id, v.id);
      toast.success(doc.audiences.some((a) => a !== "internal") ? `${v.label} restored as a new version, sent for approval` : `${v.label} restored as the live version`);
      setRestoring(null);
      setViewing(undefined);
      setApprovalKey((k) => k + 1);
      await load();
    } catch (e) {
      toast.error(extractApiError(e, "The version could not be restored."));
    }
  }

  async function unlinkArticle(articleId: string) {
    if (!doc) return;
    try {
      const cur = await docLibApi.articleDocuments(articleId);
      await docLibApi.setArticleDocuments(articleId, itemsOf(cur.data).map((d) => d.id).filter((x) => x !== doc.id));
      toast.success("Article unlinked");
      await load();
    } catch (e) {
      toast.error(extractApiError(e, "The article could not be unlinked."));
    }
  }

  if (loading) return (
    <div className="space-y-4"><Skeleton className="h-10 w-72" /><div className="grid gap-4 lg:grid-cols-[1fr_360px]"><Skeleton className="h-[480px]" /><Skeleton className="h-80" /></div></div>
  );

  if (notFound || !doc) return (
    <Card>
      <EmptyState icon={FileText} title="Document not found" description="It may have been deleted, or its category is not open to you."
        action={<Button asChild><Link href="/knowledge/documents">Back to documents</Link></Button>} />
    </Card>
  );

  const hasSource = !!current?.has_source || !!current?.source_file_name
    || !!versions?.find((v) => v.id === current?.id)?.source_file_name;
  const canDownloadSource = perms.download_source ?? perms.edit;
  const canDelete = perms.delete ?? can("documents:manage_all");

  const status = doc.pending_version
    ? <StatusPill family="doc_version" status="pending_approval" label={`${doc.pending_version.label} waiting for approval`} />
    : current ? <StatusPill family="doc_version" status="live" label={`${current.label} live`} /> : <StatusPill family="doc_version" status="draft" label="No live version" />;

  return (
    <div className="space-y-4">
      <PageHeader title={doc.title} back={{ href: "/knowledge/documents", label: "Documents" }} status={status}
        summary={[doc.category?.name, languageLabel(doc.language), doc.owner ? `Owner ${doc.owner.name}` : null].filter(Boolean).join(" · ")}
        actions={<>
          {perms.trace && <Button onClick={() => setTracing(true)}><ScanSearch aria-hidden /> Trace a leaked copy</Button>}
          {perms.edit && <Button onClick={() => setEditing(true)}><Edit3 aria-hidden /> Edit details</Button>}
          {perms.edit && <Button onClick={() => setUploading(true)}><Upload aria-hidden /> Upload new version</Button>}
          {current && (canDownloadSource && hasSource ? (
            <DropdownMenu trigger={<Button variant="primary"><Download aria-hidden /> Download <ChevronDown aria-hidden /></Button>}>
              <MenuItem icon={FileText} onSelect={() => void download()}>PDF, stamped for you</MenuItem>
              <MenuItem icon={FileCode2} onSelect={() => void download({ source: true, name: undefined })}>Source file (DOCX, XLSX or PPTX)</MenuItem>
            </DropdownMenu>
          ) : (
            <Button variant="primary" onClick={() => void download()}><Download aria-hidden /> Download</Button>
          ))}
          {canDelete && (
            <IconButton icon={Trash2} label="Delete document" className="hover:text-tone-red" onClick={() => setConfirmDelete(true)} />
          )}
        </>} />

      {doc.description && <p className="max-w-3xl text-sm text-fg-muted">{doc.description}</p>}

      <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_360px]">
        <Card className="min-w-0">
          <CardHeader
            title={viewed ? <span>Viewing <span className="font-mono">{viewed.label}</span> ({versionStatusLabel(viewed.status).toLowerCase()})</span>
              : current ? <span>Current version <span className="font-mono">{current.label}</span></span> : "No live version"}
            description="Your copy carries a footer with your name and a tracking ID."
            actions={<>
              {viewed && <Button size="sm" onClick={() => setViewing(undefined)}><X aria-hidden /> Back to current</Button>}
              {(current || viewed) && (
                <Button size="sm" variant="ghost" aria-pressed={showViewer} onClick={() => setShowViewer((v) => !v)}>
                  <Eye aria-hidden /> {showViewer ? "Hide" : "Show"}
                </Button>
              )}
            </>} />
          <CardBody className="bg-surface-2/50">
            {!current && !viewed ? (
              <EmptyState icon={FileText} compact title="Nothing live yet"
                description={doc.pending_version ? "The first version is waiting for approval." : "Upload the first version."}
                action={perms.edit && !doc.pending_version ? <Button variant="primary" onClick={() => setUploading(true)}><Upload aria-hidden /> Upload a version</Button> : undefined} />
            ) : showViewer ? (
              <DocumentViewer key={viewing ?? "current"} documentId={doc.id} versionId={viewing} title={doc.title}
                fileName={viewed?.file_name ?? current?.file_name} mime={viewed?.file_mime ?? current?.file_mime} />
            ) : (
              <p className="py-6 text-center text-[13px] text-fg-muted">The viewer is hidden.</p>
            )}
            {current && !viewed && (
              <p className="mt-3 text-xs text-fg-muted">
                <span className="font-medium text-fg">{current.change_note}</span>
                {" · "}{current.file_name}{current.file_size ? `, ${formatFileSize(current.file_size)}` : ""}{current.page_count ? `, ${current.page_count} pages` : ""}
                {formatDuration(current.duration_seconds) ? `, video ${formatDuration(current.duration_seconds)}, logged but not watermarked` : ""}
                {current.went_live_at ? `, live since ${fmtDate(current.went_live_at)}` : ""}
              </p>
            )}
          </CardBody>
        </Card>

        <div className="min-w-0 space-y-4">
          {doc.pending_version && (
            <div className="space-y-2">
              <ApprovalPanel entityType="library_document_version" entityId={doc.pending_version.id} refreshKey={approvalKey}
                onChange={() => { void load(); }} className="" />
              <p className="px-1 text-xs text-fg-muted">
                <span className="font-mono">{doc.pending_version.label}</span> goes live when a director approves it.
                {versions && <> <button type="button" className="text-brand-ink hover:underline" onClick={() => { setViewing(doc.pending_version!.id); setShowViewer(true); }}>View it</button></>}
              </p>
            </div>
          )}

          <Card>
            <CardHeader title="Details" />
            <CardBody>
              <KeyValues cols={1} items={[
                { label: "Category", value: doc.category?.name ?? "-" },
                {
                  label: "For",
                  value: <span className="flex flex-wrap justify-end gap-1 sm:justify-start">{doc.audiences.map((a) => <Pill key={a} tone={audienceTone(a)}>{audienceLabel(a)}</Pill>)}</span>,
                },
                { label: "Owner", value: doc.owner?.name ?? "-" },
                {
                  label: "Review by",
                  value: doc.review_by ? <span className={doc.needs_review ? "font-medium text-tone-red" : undefined}>{fmtDate(doc.review_by)}{doc.needs_review ? ", review due" : ""}</span> : "-",
                  numeric: true,
                },
                { label: "Updated", value: fmtDate(doc.updated_at), numeric: true },
              ]} />
              {(doc.audiences.includes("customer") || doc.audiences.includes("partner")) && (
                <p className="mt-3 text-xs text-fg-muted">Every copy sent out is stamped with the recipient&apos;s company and a tracking ID. {WATERMARK_LIMIT}</p>
              )}
            </CardBody>
          </Card>

          <Card>
            <CardHeader title={<span className="inline-flex items-center gap-2"><Languages className="size-4 text-fg-muted" aria-hidden /> Languages</span>} />
            <CardBody>
              <ul className="space-y-1.5 text-sm">
                <li className="flex items-center justify-between gap-2"><span className="text-fg">{languageLabel(doc.language)}</span><span className="text-xs text-fg-muted">This document</span></li>
                {(doc.language_variants ?? []).map((v) => (
                  <li key={v.id} className="flex items-center justify-between gap-2">
                    <Link href={`/knowledge/documents/${v.id}`} className="min-w-0 truncate text-brand-ink hover:underline">{languageLabel(v.language)}</Link>
                    <span className="min-w-0 truncate text-xs text-fg-muted">{v.title}</span>
                  </li>
                ))}
              </ul>
              {(doc.language_variants ?? []).length === 0 && <p className="mt-2 text-xs text-fg-muted">No translations. Mark one with Edit details, Translation of.</p>}
            </CardBody>
          </Card>

          <Card>
            <CardHeader title={<span className="inline-flex items-center gap-2"><Package className="size-4 text-fg-muted" aria-hidden /> Products</span>} />
            <CardBody>
              {doc.products?.length ? (
                <ul className="flex flex-wrap gap-1.5">
                  {doc.products.map((p) => <li key={`${p.id ?? ""}${p.family ?? ""}`}><Pill>{productLabel(p)}</Pill></li>)}
                </ul>
              ) : <p className="text-xs text-fg-muted">Not linked to a product. Nary finds documents for a deal by its products.</p>}
            </CardBody>
          </Card>

          <Card>
            <CardHeader title={<span className="inline-flex items-center gap-2"><BookOpen className="size-4 text-fg-muted" aria-hidden /> Linked articles</span>}
              actions={perms.edit ? <Button size="sm" onClick={() => setLinking(true)}><Link2 aria-hidden /> Link an article</Button> : undefined} />
            <CardBody>
              {articles.length ? (
                <ul className="space-y-1">
                  {articles.map((a) => (
                    <li key={a.id} className="flex items-center gap-2">
                      <Link href={`/knowledge/articles/${a.id}`} className="min-w-0 flex-1 truncate text-sm text-brand-ink hover:underline">{a.title}</Link>
                      {perms.edit && <IconButton icon={X} size="sm" label={`Unlink ${a.title}`} onClick={() => void unlinkArticle(a.id)} />}
                    </li>
                  ))}
                </ul>
              ) : <p className="text-xs text-fg-muted">No article embeds this document.</p>}
            </CardBody>
          </Card>

          {perms.history && versions && (
            <Card>
              <CardHeader title={<span className="inline-flex items-center gap-2"><History className="size-4 text-fg-muted" aria-hidden /> Version history</span>}
                description={`${versions.length} version${versions.length === 1 ? "" : "s"}`} />
              <ol className="divide-y divide-line">
                {versions.map((v) => (
                  <li key={v.id} className={cn("space-y-1.5 px-4 py-3", viewing === v.id && "bg-brand/[0.07]")}>
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-mono text-sm font-medium text-fg">{v.label}</span>
                      <StatusPill family="doc_version" status={v.status} label={versionStatusLabel(v.status)} />
                      <span className="ml-auto text-xs text-fg-muted tabular">{fmtDate(v.went_live_at ?? v.created_at)}</span>
                    </div>
                    <p className="text-[13px] text-fg">{v.change_note}</p>
                    <p className="text-xs text-fg-muted">
                      {[v.file_name, v.file_size ? formatFileSize(v.file_size) : null, formatDuration(v.duration_seconds) ? `video ${formatDuration(v.duration_seconds)}` : null, v.source_file_name ? `source ${v.source_file_name}` : null].filter(Boolean).join(", ")}
                      {v.uploaded_by ? ` · by ${personName(v.uploaded_by)}` : ""}
                      {v.approved_by ? ` · approved by ${personName(v.approved_by)}` : ""}
                    </p>
                    {v.status === "rejected" && v.rejected_reason && <p className="text-xs text-tone-red">Rejected: {v.rejected_reason}</p>}
                    <div className="flex flex-wrap gap-2 pt-1">
                      {v.id !== (viewing ?? current?.id) && (
                        <Button size="sm" variant="ghost" onClick={() => { setViewing(v.id === current?.id ? undefined : v.id); setShowViewer(true); }}><Eye aria-hidden /> View</Button>
                      )}
                      <Button size="sm" variant="ghost" onClick={() => void download({ version: v.id, name: v.file_name })}><Download aria-hidden /> Download</Button>
                      {(perms.restore ?? perms.history) && (v.status === "superseded" || v.status === "rejected") && (
                        <Button size="sm" onClick={() => setRestoring(v)}><RotateCcw aria-hidden /> Restore</Button>
                      )}
                    </div>
                  </li>
                ))}
              </ol>
            </Card>
          )}
        </div>
      </div>

      {uploading && <UploadVersionDialog open onOpenChange={setUploading} doc={doc} onUploaded={() => { setApprovalKey((k) => k + 1); void load(); }} />}
      {editing && <EditDocumentDialog open onOpenChange={setEditing} doc={doc} categories={categories} onSaved={() => void load()} />}
      <TraceDialog open={tracing} onOpenChange={setTracing} />

      <Dialog open={confirmDelete} onOpenChange={setConfirmDelete} size="sm" title="Delete this document?"
        description="It disappears from the library, from Nary and from the partner portal. Copies already sent keep their tracking ID."
        dismissable={!deleting}
        footer={<>
          <Button onClick={() => setConfirmDelete(false)} disabled={deleting}>Cancel</Button>
          <Button variant="danger" loading={deleting} onClick={remove}>Delete document</Button>
        </>} />

      {restoring && (
        <Dialog open onOpenChange={(o) => { if (!o) setRestoring(null); }} size="sm" title={`Restore ${restoring.label}?`}
          description={doc.audiences.some((a) => a !== "internal")
            ? "It is copied as a new version and waits for a director's approval before it goes live."
            : "It is copied as a new version and goes live now; the current version is kept in the history."}
          footer={<>
            <Button onClick={() => setRestoring(null)}>Cancel</Button>
            <Button variant="primary" onClick={() => void restore(restoring)}><RotateCcw aria-hidden /> Restore {restoring.label}</Button>
          </>} />
      )}

      {linking && <LinkArticleDialog documentId={doc.id} linked={articles.map((a) => a.id)} onClose={() => setLinking(false)} onLinked={() => void load()} />}
    </div>
  );
}

/** Embed this document in an article (the article page then shows it as a card). */
function LinkArticleDialog({ documentId, linked, onClose, onLinked }: {
  documentId: string; linked: string[]; onClose: () => void; onLinked: () => void;
}) {
  const [articleId, setArticleId] = useState("");
  const [busy, setBusy] = useState(false);
  async function link() {
    if (!articleId) return;
    setBusy(true);
    try {
      const cur = await docLibApi.articleDocuments(articleId);
      const ids = itemsOf(cur.data).map((d) => d.id);
      if (!ids.includes(documentId)) await docLibApi.setArticleDocuments(articleId, [...ids, documentId]);
      toast.success("Article linked");
      onLinked();
      onClose();
    } catch (e) {
      toast.error(extractApiError(e, "The article could not be linked."));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} size="md" title="Link an article" dismissable={!busy}
      description="The article shows this document as a card with its current version."
      footer={<>
        <Button onClick={onClose} disabled={busy}>Cancel</Button>
        <Button variant="primary" loading={busy} disabled={!articleId} onClick={link}><Link2 aria-hidden /> Link article</Button>
      </>}>
      <Combobox value={articleId} onChange={(id) => setArticleId(id)} source={articleSource} placeholder="Search articles and SOPs" excludeIds={linked} />
    </Dialog>
  );
}
