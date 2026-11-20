"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import Link from "next/link";
import { getSession } from "next-auth/react";
import { AlertTriangle, CheckCircle2, Download, FileText, Loader2, Lock, ShieldAlert, ShieldCheck, XCircle } from "lucide-react";
import { Button, Card, CardBody, CardHeader } from "@/components/ui";
import { PublicPage, PublicNotice } from "@/components/portal/PublicPage";
import { cn } from "@/lib/utils";

interface VerifyPayload {
  state: "valid" | "revoked" | "superseded" | "expired" | "void" | "not_approved";
  valid: boolean;
  issuer: string;
  entity_label: string;
  document_number: string | null;
  document_title: string | null;
  revision: number;
  issued_at: string | null;
  expires_at: string | null;
  revoked_at: string | null;
  facts: { label: string; value: string }[];
  approvals: { step: string; name: string; date: string }[];
  signature: { status: string; completed_at: string | null; digitally_sealed: boolean; signature_level: string } | null;
  /** 3 Nov 2026: the copy sent and downloaded carries the company's digital seal (PAdES certification). */
  digitally_sealed?: boolean;
  seal_level?: string | null;
  sealed_hash?: string | null;
  document_hash: string | null;
  hash_algorithm: string;
  can_view_document: boolean;
  allow_download: boolean;
}

type LoadError = "not_found" | "login_required" | "forbidden" | "rate_limited" | "failed";

const STATE_COPY: Record<VerifyPayload["state"], { title: string; body: string; tone: "ok" | "warn" | "bad" }> = {
  valid:        { title: "Authentic document", body: "This document was issued and approved by the organisation named below, and it is still current.", tone: "ok" },
  superseded:   { title: "Replaced by a newer version", body: "This copy was genuine but has been superseded. Ask the issuer for the current document.", tone: "warn" },
  expired:      { title: "Verification expired", body: "This copy was genuine, but its verification period has ended. Ask the issuer for a current copy.", tone: "warn" },
  not_approved: { title: "No longer matches the approved version", body: "The document has changed since this copy was approved. Do not rely on this copy.", tone: "warn" },
  revoked:      { title: "Revoked by the issuer", body: "The issuer has withdrawn this document. Do not rely on it.", tone: "bad" },
  void:         { title: "Document cancelled", body: "The issuer has cancelled this document. Do not rely on it.", tone: "bad" },
};

/** "pades-b-lta" -> "PAdES-B-LTA". */
function sealLevelLabel(level: string | null | undefined): string | null {
  if (!level) return null;
  return level.toLowerCase().startsWith("pades-") ? `PAdES-${level.slice(6).toUpperCase()}` : level;
}

const fmt = (iso: string | null) =>
  iso ? new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" }) : "-";

async function authHeaders(): Promise<HeadersInit> {
  try {
    const session = await getSession();
    const token = (session as { access_token?: string } | null)?.access_token;
    return token ? { Authorization: `Bearer ${token}` } : {};
  } catch {
    return {};
  }
}

export default function VerifyTokenPage({ params }: { params: { token: string } }) {
  const token = params.token;
  const [data, setData] = useState<VerifyPayload | null>(null);
  const [error, setError] = useState<LoadError | null>(null);
  const [loading, setLoading] = useState(true);
  const [viewer, setViewer] = useState<"idle" | "loading" | "shown" | "failed">("idle");
  const pagesRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    (async () => {
      try {
        const res = await fetch(`/api/v1/verify/t/${encodeURIComponent(token)}`, { headers: await authHeaders(), cache: "no-store" });
        if (res.ok) setData(await res.json());
        else setError(res.status === 401 ? "login_required" : res.status === 403 ? "forbidden"
          : res.status === 404 ? "not_found" : res.status === 429 ? "rate_limited" : "failed");
      } catch {
        setError("failed");
      } finally {
        setLoading(false);
      }
    })();
  }, [token]);

  const fetchPdf = useCallback(async (download: boolean) => {
    const res = await fetch(`/api/v1/verify/t/${encodeURIComponent(token)}/pdf${download ? "?download=true" : ""}`,
      { headers: await authHeaders(), cache: "no-store" });
    if (!res.ok) throw new Error(String(res.status));
    return res.arrayBuffer();
  }, [token]);

  async function showDocument() {
    if (!pagesRef.current) return;
    setViewer("loading");
    try {
      const buf = await fetchPdf(false);
      const pdfjsLib = await import("pdfjs-dist");
      pdfjsLib.GlobalWorkerOptions.workerSrc = "/pdf.worker.min.mjs";
      const doc = await pdfjsLib.getDocument({ data: new Uint8Array(buf) }).promise;
      const container = pagesRef.current;
      container.innerHTML = "";
      for (let i = 1; i <= doc.numPages; i++) {
        const page = await doc.getPage(i);
        const viewport = page.getViewport({ scale: 1.6 });
        const canvas = document.createElement("canvas");
        canvas.width = viewport.width;
        canvas.height = viewport.height;
        canvas.className = "w-full h-auto rounded-lg border border-line shadow-card";
        canvas.oncontextmenu = (e) => e.preventDefault();
        const ctx = canvas.getContext("2d");
        if (!ctx) continue;
        await page.render({ canvasContext: ctx, viewport }).promise;
        container.appendChild(canvas);
      }
      setViewer("shown");
    } catch {
      setViewer("failed");
    }
  }

  async function download() {
    try {
      const buf = await fetchPdf(true);
      const url = URL.createObjectURL(new Blob([buf], { type: "application/pdf" }));
      const a = document.createElement("a");
      a.href = url;
      a.download = `${data?.document_number ?? "document"}.pdf`;
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 30_000);
    } catch {
      setViewer("failed");
    }
  }

  const shell = (children: React.ReactNode) => (
    <PublicPage subtitle="Document verification">
      <div className="space-y-4">{children}</div>
    </PublicPage>
  );

  if (loading) return shell(
    <div role="status" aria-label="Verifying" className="flex justify-center py-20"><Loader2 className="size-6 animate-spin text-fg-subtle" aria-hidden /></div>
  );

  if (error || !data) {
    const copy: Record<LoadError, { icon: React.ElementType; title: string; body: string }> = {
      not_found: { icon: ShieldAlert, title: "Code not recognised", body: "This QR code does not match any document we issued. The document may not be genuine." },
      login_required: { icon: Lock, title: "Sign in to verify", body: "This is a confidential document. Sign in with your company account to view its verification." },
      forbidden: { icon: Lock, title: "Not permitted", body: "Your account is not permitted to view this confidential document." },
      rate_limited: { icon: AlertTriangle, title: "Too many requests", body: "Please wait a minute and scan again." },
      failed: { icon: AlertTriangle, title: "Could not verify right now", body: "Please try again in a moment." },
    };
    const c = copy[error ?? "failed"];
    return shell(
      <PublicNotice icon={c.icon} tone={error === "not_found" ? "red" : "orange"} title={c.title}
        action={error === "login_required" ? (
          <Button variant="primary" asChild><Link href={`/login?callbackUrl=${encodeURIComponent(`/v/${token}`)}`}>Sign in</Link></Button>
        ) : undefined}>
        <p>{c.body}</p>
      </PublicNotice>
    );
  }

  const copy = STATE_COPY[data.state];
  const toneClass = copy.tone === "ok" ? "border-tone-blue/30 bg-tone-blue-soft text-tone-blue"
    : copy.tone === "warn" ? "border-tone-orange/30 bg-tone-orange-soft text-tone-orange" : "border-tone-red/30 bg-tone-red-soft text-tone-red";
  const ToneIcon = copy.tone === "ok" ? CheckCircle2 : copy.tone === "warn" ? AlertTriangle : XCircle;

  return shell(
    <>
      <div role="status" className={cn("flex gap-4 rounded-lg border p-5", toneClass)}>
        <ToneIcon className="mt-0.5 size-7 shrink-0" aria-hidden />
        <div className="min-w-0">
          <h1 className="text-lg font-semibold">{copy.title}</h1>
          <p className="mt-0.5 text-sm">{copy.body}</p>
        </div>
      </div>

      <Card>
        <CardBody className="sm:p-6">
          <p className="text-xs font-medium text-fg-muted">{data.entity_label}</p>
          <p className="mt-1 break-words text-lg font-semibold text-fg">{data.document_number}</p>
          <p className="mt-0.5 text-sm text-fg-muted">Issued by {data.issuer} on {fmt(data.issued_at)}</p>

          {data.facts.length > 0 && (
            <dl className="mt-5 divide-y divide-line-soft border-t border-line-soft">
              {data.facts.map((f) => (
                <div key={f.label} className="flex flex-col py-2 sm:flex-row sm:gap-4">
                  <dt className="shrink-0 text-sm text-fg-muted sm:w-40">{f.label}</dt>
                  <dd className="break-words text-sm font-medium text-fg tabular">{f.value}</dd>
                </div>
              ))}
              {data.expires_at && (
                <div className="flex flex-col py-2 sm:flex-row sm:gap-4">
                  <dt className="shrink-0 text-sm text-fg-muted sm:w-40">Verifiable until</dt>
                  <dd className="text-sm text-fg">{fmt(data.expires_at)}</dd>
                </div>
              )}
            </dl>
          )}
        </CardBody>
      </Card>

      {data.approvals.length > 0 && (
        <Card>
          <CardHeader title="Approval record" />
          <CardBody>
            <ul className="space-y-2">
              {data.approvals.map((a, i) => (
                <li key={i} className="flex flex-col gap-0.5 text-sm sm:flex-row sm:items-center sm:justify-between">
                  <span className="flex items-center gap-2 text-fg"><CheckCircle2 className="size-4 text-tone-blue" aria-hidden />{a.step}</span>
                  <span className="text-fg-muted sm:text-right">{a.name}{a.date ? ` · ${a.date}` : ""}</span>
                </li>
              ))}
            </ul>
          </CardBody>
        </Card>
      )}

      {data.digitally_sealed && (
        <Card>
          <CardHeader title="Digital seal" />
          <CardBody>
            <p className="flex items-center gap-2 text-sm text-fg">
              <ShieldCheck className="size-4 text-tone-blue" aria-hidden />
              Digitally sealed by {data.issuer || "the issuer"}{data.seal_level ? ` (${sealLevelLabel(data.seal_level)})` : ""}
            </p>
            <p className="mt-1 text-[13px] text-fg-muted">
              The PDF carries the issuer&apos;s certification seal: a PDF reader shows it in its signature panel and flags any change made after sealing.
            </p>
          </CardBody>
        </Card>
      )}

      {data.signature && (
        <Card>
          <CardHeader title="Electronic signature" />
          <CardBody>
            <p className="text-sm text-fg">
              {data.signature.status === "completed" ? `Signed by all parties on ${fmt(data.signature.completed_at)}` : `Signing ${data.signature.status}`}
              {data.signature.digitally_sealed && " · digitally sealed"}
            </p>
          </CardBody>
        </Card>
      )}

      {(data.can_view_document || data.allow_download) && (
        <Card>
          <CardHeader title="Issued document" description="Compare the copy you hold with the issued document below."
            actions={<>
              {data.can_view_document && viewer !== "shown" && (
                <Button variant="primary" onClick={showDocument} loading={viewer === "loading"}>
                  {viewer !== "loading" && <FileText aria-hidden />} View document
                </Button>
              )}
              {data.allow_download && (
                <Button onClick={download}><Download aria-hidden /> Download</Button>
              )}
            </>} />
          <CardBody className="space-y-3">
            {viewer === "failed" && <p role="alert" className="text-[13px] text-tone-red">The document could not be loaded. Please try again.</p>}
            <div ref={pagesRef} className="select-none space-y-3" />
          </CardBody>
        </Card>
      )}

      {data.document_hash && (
        <p className="break-all text-[11px] text-fg-subtle">
          {data.hash_algorithm} of the issued PDF: <span className="font-mono">{data.document_hash}</span>
        </p>
      )}
      {data.sealed_hash && (
        <p className="break-all text-[11px] text-fg-subtle">
          {data.hash_algorithm} of the digitally sealed copy: <span className="font-mono">{data.sealed_hash}</span>
        </p>
      )}
    </>
  );
}
