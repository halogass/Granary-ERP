"use client";

import { useEffect, useState, useCallback, useRef } from "react";
import { displaySummary } from "@/lib/kbSummary";
import { useParams, useRouter, useSearchParams } from "next/navigation";
import { useSession } from "next-auth/react";
import { kbApi, attachmentApi } from "@/lib/api";
import Link from "next/link";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Components } from "react-markdown";
import { extractApiError } from "@/lib/apiError";
import { Button, Card, Dialog, ErrorToast, Field, Input, PageHeader, Pill, Skeleton, StatusPill, EmptyState } from "@/components/ui";
import { cn } from "@/lib/utils";
import { MD_PREVIEW } from "../../_components/ArticleEditor";
import ApprovalPanel from "@/components/approvals/ApprovalPanel";
import { ArticleDocumentCards } from "@/components/documents/ArticleDocuments";
import {
  BookOpen, Edit3, Download, CheckCircle2, FileText, Clock, Tag, Zap, User, AlertCircle, PenLine, Trash2, RefreshCw,
} from "lucide-react";

interface Article {
  id: string;
  title: string;
  slug: string;
  article_type: string;
  content: string;
  summary?: string;
  sop_code?: string;
  sop_trigger?: string;
  sop_owner_role?: string;
  tags: string[];
  access_level: string;
  status: string;
  version: string;
  category_id?: string;
  created_by_id?: string;
  created_by_name?: string;
  author_signed_at?: string;
  author_signature_url?: string;
  owner_signed_by_id?: string;
  owner_signed_by_name?: string;
  owner_signed_at?: string;
  owner_signature_url?: string;
  // SOP revisions: this row is a draft revision of `revision_of`, or the live SOP has an open revision
  revision_of?: string | null;
  pending_revision_id?: string | null;
  // PDF import
  pdf_import_status?: "pending" | "processing" | "completed" | "failed" | null;
  pdf_import_error?: string | null;
  pdf_import_mode?: "text" | "vision" | "mixed" | null;
  pdf_import_pages?: number | null;
  pdf_import_truncated?: boolean;
  source_pdf?: { id: string; file_name: string; file_size: number } | null;
  updated_at?: string;
}

const MD_COMPONENTS: Components = {
  ...MD_PREVIEW,
  h4: ({ children }) => <h4 className="mb-1 mt-4 text-sm font-semibold text-fg">{children}</h4>,
  thead: ({ children }) => <thead className="bg-surface-2">{children}</thead>,
  img: ({ src, alt }) => (
    // eslint-disable-next-line @next/next/no-img-element
    <img src={src} alt={alt} className="my-4 max-w-full rounded-lg border border-line" />
  ),
  input: ({ checked, type }) =>
    type === "checkbox" ? <input type="checkbox" checked={checked} readOnly className="mr-2 rounded accent-[rgb(var(--brand))]" /> : null,
};

function SignatureImage({ src, alt }: { src: string; alt: string }) {
  // eslint-disable-next-line @next/next/no-img-element
  return <img src={src} alt={alt} className="mt-1 h-10 rounded object-contain dark:bg-fg p-0.5" />;
}

export default function ArticlePage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const { data: session } = useSession();
  const myId = (session?.user as { id?: string })?.id ?? "";
  const isSuperadmin = (session?.user as { is_superadmin?: boolean })?.is_superadmin ?? false;
  const [actionError, setActionError] = useState("");
  const [approvalKey, setApprovalKey] = useState(0);

  const contentRef = useRef<HTMLDivElement>(null);
  const [article, setArticle] = useState<Article | null>(null);
  const [loading, setLoading] = useState(true);

  // Sign modals
  const [showSignModal, setShowSignModal]       = useState(false);
  const [showSignOWNERModal, setShowSignOWNERModal] = useState(false);
  const [sigImageUrl, setSigImageUrl]           = useState<string | undefined>(undefined);
  const [signing, setSigning]                   = useState(false);
  const [confirmDel, setConfirmDel]             = useState(false);
  const [deleting, setDeleting]                 = useState(false);
  const [reconverting, setReconverting]         = useState(false);
  const [confirmReconvert, setConfirmReconvert] = useState(false);

  async function deleteArticle() {
    if (!article) return;
    setDeleting(true);
    try {
      await kbApi.deleteArticle(article.id);
      router.push("/knowledge/articles");
    } finally {
      setDeleting(false);
    }
  }

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await kbApi.getArticle(id);
      setArticle(res.data);
    } catch { setArticle(null); }
    finally { setLoading(false); }
  }, [id]);

  useEffect(() => { load(); }, [load]);

  // "Save and sign" on the new-article page lands here with ?sign=1: open the author's signature dialog once.
  const searchParams = useSearchParams();
  const askedToSign = useRef(false);
  useEffect(() => {
    if (askedToSign.current || !article || searchParams?.get("sign") !== "1") return;
    askedToSign.current = true;
    if (!article.author_signed_at && article.status === "draft") {
      setSigImageUrl(undefined);
      setShowSignModal(true);
    }
  }, [article, searchParams]);

  // A PDF conversion runs in the background worker; poll until it settles.
  const importPending = article?.pdf_import_status === "pending" || article?.pdf_import_status === "processing";
  // updated_at is written when the status is set, so it marks when this stage began.
  const importMinutes = article?.updated_at
    ? Math.floor((Date.now() - new Date(article.updated_at).getTime()) / 60000)
    : 0;
  // Longer than any real conversion: something ate the job.
  const importStalled = importPending && importMinutes >= 20;
  useEffect(() => {
    if (!importPending) return;
    const t = setInterval(async () => {
      try {
        const res = await kbApi.getArticle(id);
        setArticle(res.data);
      } catch { /* keep polling */ }
    }, 5000);
    return () => clearInterval(t);
  }, [importPending, id]);

  async function downloadSourcePdf() {
    if (!article?.source_pdf) return;
    try {
      const res = await attachmentApi.download(article.source_pdf.id);
      const url = URL.createObjectURL(res.data as Blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = article.source_pdf.file_name;
      a.click();
      URL.revokeObjectURL(url);
    } catch { /* ignore */ }
  }

  async function reconvert() {
    if (!article) return;
    setReconverting(true);
    try {
      await kbApi.reconvertPdf(article.id);
      await load();
    } catch { /* the banner will show the stored error */ }
    finally { setReconverting(false); setConfirmReconvert(false); }
  }

  async function exportToPDF() {
    if (!article) return;
    try {
      const res = await kbApi.exportPdf(article.id);
      const url = URL.createObjectURL(res.data as Blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${article.sop_code || article.slug}.pdf`;
      a.click();
      URL.revokeObjectURL(url);
    } catch { /* ignore */ }
  }

  async function handleSignImage(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = (ev) => setSigImageUrl(ev.target?.result as string);
    reader.readAsDataURL(file);
  }

  async function submitSign(owner = false) {
    if (!article) return;
    setSigning(true);
    try {
      const res = owner
        ? await kbApi.signArticleCEO(article.id, sigImageUrl ? { signature_url: sigImageUrl } : {})
        : await kbApi.signArticle(article.id, sigImageUrl ? { signature_url: sigImageUrl } : {});
      if (owner) setShowSignOWNERModal(false); else setShowSignModal(false);
      setSigImageUrl(undefined);
      setApprovalKey((k) => k + 1);
      window.dispatchEvent(new Event("approvals:changed"));
      // An approved revision replaces the live SOP: show the live one
      const publishedId = res.data?.published_id;
      if (publishedId && publishedId !== article.id) { router.push(`/knowledge/articles/${publishedId}`); return; }
      await load();
    } catch { /* ignore */ } finally { setSigning(false); }
  }

  async function publish() {
    if (!article) return;
    setActionError("");
    try {
      await kbApi.publishArticle(article.id);
    } catch (e) {
      setActionError(extractApiError(e, "Could not publish the article."));
    }
    setApprovalKey((k) => k + 1);
    window.dispatchEvent(new Event("approvals:changed"));
    await load();
  }

  if (loading) return (
    <div className="mx-auto max-w-4xl space-y-4"><Skeleton className="h-10 w-72" /><Skeleton className="h-96 w-full" /></div>
  );

  if (!article) return (
    <EmptyState icon={BookOpen} title="Article not found."
      action={<Button asChild><Link href="/knowledge/articles">Back to the knowledge base</Link></Button>} />
  );

  const type = article.article_type.replace(/_/g, " ");
  const summary = displaySummary(article.summary, article.content);
  const banner = "mb-4 rounded-lg border px-4 py-2.5 text-sm";

  const signatureDialog = (owner: boolean) => (
    <Dialog open={owner ? showSignOWNERModal : showSignModal} onOpenChange={(o) => (owner ? setShowSignOWNERModal(o) : setShowSignModal(o))}
      title={owner ? "Owner countersignature" : "Sign this article"}
      footer={<>
        <Button onClick={() => (owner ? setShowSignOWNERModal(false) : setShowSignModal(false))}>Cancel</Button>
        <Button variant="primary" onClick={() => submitSign(owner)} loading={signing}>
          {signing ? "Signing..." : owner ? "Countersign as Owner" : "Sign article"}
        </Button>
      </>}>
      <div className="space-y-4">
        {owner ? (
          <>
            <p className="text-sm text-fg-muted">
              Signing as <strong className="text-fg">Owner</strong> confirms this SOP is approved for publication and operational use.
            </p>
            <p className="rounded-lg bg-surface-2 px-3 py-2 text-sm font-medium text-fg">
              {article.sop_code && <span className="mr-2 font-mono text-brand-ink">{article.sop_code}</span>}
              {article.title}
            </p>
          </>
        ) : (
          <p className="text-sm text-fg-muted">
            By signing, you confirm that you are the author and accept responsibility for the accuracy of this content.
          </p>
        )}
        <Field label="Signature image" hint="Optional.">
          <Input type="file" accept="image/*" onChange={handleSignImage} className="py-1.5" />
        </Field>
        {/* eslint-disable-next-line @next/next/no-img-element */}
        {sigImageUrl && <img src={sigImageUrl} alt="Signature preview" className="h-12 rounded border border-line object-contain dark:bg-fg p-0.5" />}
      </div>
    </Dialog>
  );

  return (
    <div className="mx-auto max-w-4xl">
      <PageHeader title={article.title} back={{ href: "/knowledge/articles", label: "Knowledge base" }}
        status={<StatusPill family="knowledge" status={article.status} />}
        actions={<>
          {article.status === "draft" && (
            <Button variant="primary" onClick={publish}><Zap aria-hidden /> Publish</Button>
          )}
          <Button onClick={() => { exportToPDF(); }}><Download aria-hidden /> Export PDF</Button>
          <Button asChild><Link href={`/knowledge/articles/${id}/edit`}><Edit3 aria-hidden /> Edit</Link></Button>
          {isSuperadmin && (
            confirmDel ? (
              <span className="flex items-center gap-1.5">
                <span className="text-xs text-fg-muted">Delete?</span>
                <Button variant="danger" onClick={deleteArticle} loading={deleting}>Yes</Button>
                <Button variant="ghost" onClick={() => setConfirmDel(false)}>No</Button>
              </span>
            ) : (
              <Button variant="danger" onClick={() => setConfirmDel(true)}><Trash2 aria-hidden /> Delete</Button>
            )
          )}
        </>} />

      {article.revision_of && article.status !== "archived" && (
        <div className={cn(banner, "border-tone-orange/30 bg-tone-orange-soft text-fg")}>
          Draft revision of a published SOP. The <Link href={`/knowledge/articles/${article.revision_of}`}
            className="font-medium underline">published version</Link> stays live until this revision is
          signed by its author and the Owner and approved.
        </div>
      )}
      {article.pending_revision_id && (
        <div className={cn(banner, "border-tone-orange/30 bg-tone-orange-soft text-fg")}>
          A <Link href={`/knowledge/articles/${article.pending_revision_id}`}
            className="font-medium underline">draft revision</Link> of this SOP is in progress.
        </div>
      )}

      {article.article_type === "sop" && (
        <ApprovalPanel entityType="sop_article" entityId={article.id} refreshKey={approvalKey} onChange={load} />
      )}

      {/* PDF import state: the body is written by a background conversion */}
      {article.pdf_import_status && (
        <div className={cn(banner, "py-3",
          article.pdf_import_status === "failed"
            ? "border-tone-red/30 bg-tone-red-soft"
            : article.pdf_import_status === "completed"
              ? "border-tone-blue/30 bg-tone-blue-soft"
              : "border-tone-orange/30 bg-tone-orange-soft")}>
          <div className="flex flex-wrap items-start gap-3">
            {article.pdf_import_status === "failed"
              ? <AlertCircle className="mt-0.5 size-4 shrink-0 text-tone-red" aria-hidden />
              : article.pdf_import_status === "completed"
                ? <FileText className="mt-0.5 size-4 shrink-0 text-tone-blue" aria-hidden />
                : <Clock className="mt-0.5 size-4 shrink-0 animate-pulse text-tone-orange" aria-hidden />}
            <div className="min-w-0 flex-1 text-sm text-fg">
              {importPending && (
                <>
                  <p className="font-medium">
                    {article.pdf_import_status === "pending"
                      ? `Queued: ${article.source_pdf?.file_name || "the PDF"}`
                      : `Reading ${article.source_pdf?.file_name || "the PDF"}...`}
                    {importMinutes >= 1 && (
                      <span className="font-normal text-fg-muted tabular"> · {importMinutes} min</span>
                    )}
                  </p>
                  <p className="mt-0.5 text-xs text-fg-muted">
                    {importStalled ? (
                      <>
                        This has not progressed for {importMinutes} minutes.
                        {article.pdf_import_status === "pending"
                          ? " The job was queued but nothing has picked it up: check that the background worker is running and up to date."
                          : " The conversion started but never finished: the worker may have restarted."}
                        {" "}Use Re-convert to try again.
                      </>
                    ) : article.pdf_import_status === "pending" ? (
                      "Waiting for a background worker to pick this up."
                    ) : (
                      "The AI is converting it to Markdown. This can take a few minutes for a long document; the page refreshes itself when it's done."
                    )}
                  </p>
                </>
              )}
              {article.pdf_import_status === "completed" && (
                <>
                  <p className="font-medium">
                    Written by AI from {article.source_pdf?.file_name || "a PDF"}
                    {article.pdf_import_pages ? ` · ${article.pdf_import_pages} pages` : ""}
                    {article.pdf_import_mode === "vision" ? " · scanned document" : ""}
                    {article.pdf_import_mode === "mixed" ? " · partly scanned" : ""}
                  </p>
                  <p className="mt-0.5 text-xs text-fg-muted">
                    {article.pdf_import_truncated
                      ? "Only the first 150 pages were converted. "
                      : ""}
                    Check it against the original before publishing: conversion can misread tables
                    and figures.
                  </p>
                </>
              )}
              {article.pdf_import_status === "failed" && (
                <>
                  <p className="font-medium text-tone-red">Could not convert the PDF</p>
                  <p className="mt-0.5 text-xs text-fg-muted">{article.pdf_import_error}</p>
                </>
              )}
            </div>
            <div className="flex shrink-0 flex-wrap items-center gap-2">
              {article.source_pdf && (
                <Button size="sm" onClick={() => downloadSourcePdf()}><Download aria-hidden /> Original</Button>
              )}
              {article.source_pdf && (
                confirmReconvert ? (
                  <span className="flex items-center gap-1.5">
                    <span className="text-xs text-fg-muted">Replace body?</span>
                    <Button size="sm" variant="primary" onClick={reconvert} loading={reconverting}>Yes</Button>
                    <Button size="sm" variant="ghost" onClick={() => setConfirmReconvert(false)}>No</Button>
                  </span>
                ) : (
                  <Button size="sm" onClick={() => setConfirmReconvert(true)}><RefreshCw aria-hidden /> Re-convert</Button>
                )
              )}
            </div>
          </div>
        </div>
      )}

      <Card className="overflow-hidden">
        {/* Article header */}
        <div className="border-b border-line px-5 pb-5 pt-5 md:px-8 md:pt-6">
          <div className="mb-2 flex flex-wrap items-center gap-2">
            <Pill tone={article.article_type === "sop" ? "violet" : "neutral"} className="capitalize">{type}</Pill>
            {article.sop_code && <span className="font-mono text-xs text-fg-muted">{article.sop_code}</span>}
            <span className="text-xs text-fg-subtle">v{article.version}</span>
          </div>
          {summary && <p className="text-sm leading-relaxed text-fg-muted">{summary}</p>}
          {article.article_type === "sop" && (article.sop_trigger || article.sop_owner_role) && (
            <div className="mt-3 flex flex-wrap gap-4">
              {article.sop_trigger && (
                <div className="flex items-start gap-1.5 text-xs text-fg-muted">
                  <AlertCircle className="mt-0.5 size-3.5 shrink-0 text-brand-ink" aria-hidden />
                  <span><span className="font-medium text-fg">Trigger:</span> {article.sop_trigger}</span>
                </div>
              )}
              {article.sop_owner_role && (
                <div className="flex items-center gap-1.5 text-xs text-fg-muted">
                  <User className="size-3.5 shrink-0 text-brand-ink" aria-hidden />
                  <span><span className="font-medium text-fg">Owner:</span> {article.sop_owner_role}</span>
                </div>
              )}
            </div>
          )}
          {article.tags && article.tags.length > 0 && (
            <div className="mt-3 flex flex-wrap gap-1.5">
              {article.tags.map((tag) => <Pill key={tag} icon={Tag}>{tag}</Pill>)}
            </div>
          )}
        </div>

        {/* Markdown body */}
        <div className="px-5 py-6 md:px-8">
          <div ref={contentRef} id="article-md-content">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>
              {article.content}
            </ReactMarkdown>
          </div>
        </div>

        <ArticleDocumentCards articleId={article.id} />

        {/* Signatures */}
        <div className="border-t border-line px-5 pb-6 pt-5 md:px-8">
          <h2 className="mb-3 text-sm font-semibold text-fg">Signatures</h2>
          <div className={cn("grid gap-4", article.article_type === "sop" ? "grid-cols-1 sm:grid-cols-2" : "max-w-xs grid-cols-1")}>
            <div className="rounded-lg border border-line bg-surface-2 p-4">
              <p className="mb-2 text-xs font-medium text-fg-muted">Author</p>
              {article.author_signed_at ? (
                <div className="space-y-1">
                  <p className="text-sm font-medium text-fg">{article.created_by_name || "Author"}</p>
                  {article.author_signature_url && <SignatureImage src={article.author_signature_url} alt="Signature" />}
                  <p className="text-xs text-fg-muted tabular">{new Date(article.author_signed_at).toLocaleString("en-US")}</p>
                  <p className="inline-flex items-center gap-1 text-xs font-medium text-tone-blue"><CheckCircle2 className="size-3.5" aria-hidden /> Digitally signed</p>
                </div>
              ) : (
                <div className="space-y-2">
                  <p className="text-sm italic text-fg-muted">Pending author signature</p>
                  {myId === article.created_by_id && (
                    <Button size="sm" variant="primary" onClick={() => { setSigImageUrl(undefined); setShowSignModal(true); }}>
                      <PenLine aria-hidden /> Sign article
                    </Button>
                  )}
                </div>
              )}
            </div>

            {article.article_type === "sop" && (
              <div className="rounded-lg border border-line bg-surface-2 p-4">
                <p className="mb-2 text-xs font-medium text-fg-muted">Owner</p>
                {article.owner_signed_at ? (
                  <div className="space-y-1">
                    <p className="text-sm font-medium text-fg">{article.owner_signed_by_name || "Owner"}</p>
                    {article.owner_signature_url && <SignatureImage src={article.owner_signature_url} alt="Owner signature" />}
                    <p className="text-xs text-fg-muted tabular">{new Date(article.owner_signed_at).toLocaleString("en-US")}</p>
                    <p className="inline-flex items-center gap-1 text-xs font-medium text-tone-blue"><CheckCircle2 className="size-3.5" aria-hidden /> Digitally signed</p>
                  </div>
                ) : (
                  <div className="space-y-2">
                    <p className="text-sm italic text-fg-muted">Pending Owner countersignature</p>
                    {isSuperadmin && (
                      <Button size="sm" variant="primary" onClick={() => { setSigImageUrl(undefined); setShowSignOWNERModal(true); }}>
                        <PenLine aria-hidden /> Sign as Owner
                      </Button>
                    )}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      </Card>

      {signatureDialog(false)}
      {signatureDialog(true)}
      <ErrorToast message={actionError} onClose={() => setActionError("")} />
    </div>
  );
}
