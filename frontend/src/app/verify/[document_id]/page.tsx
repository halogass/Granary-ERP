"use client";

import { useState, useEffect } from "react";
import { CheckCircle2, Shield, AlertTriangle, Loader2 } from "lucide-react";
import { Card, CardBody, CardHeader, StatusPill } from "@/components/ui";
import { PublicPage, PublicNotice } from "@/components/portal/PublicPage";
import { cn } from "@/lib/utils";

function Row({ label, children }: { label: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5 py-2 sm:flex-row sm:gap-4">
      <dt className="shrink-0 text-sm text-fg-muted sm:w-40">{label}</dt>
      <dd className="min-w-0 text-sm text-fg">{children}</dd>
    </div>
  );
}

interface VerifyData {
  document_id: string; title?: string; status: string; document_hash: string;
  hash_algorithm: string; completed_at: string | null;
  certificate_generated_at?: string | null; has_certificate?: boolean;
  signers?: { name: string; email: string; sign_order: number; status: string; signed_at: string | null }[];
  event_count?: number; legal_basis?: string[]; notice?: string;
  signature_level_label?: string; seal_level_label?: string;
  digitally_sealed?: boolean; seal_authority?: string | null;
  timestamped?: boolean; long_term_validation?: boolean;
  jurisdictions?: string[];
  consent?: { agreed_count: number; version: string };
}

export default function VerifyPage({ params }: { params: { document_id: string } }) {
  const [data, setData] = useState<VerifyData | null>(null);
  const [loading, setLoading] = useState(true);
  const [notFound, setNotFound] = useState(false);

  useEffect(() => {
    fetch(`/api/v1/sign/verify/${params.document_id}`)
      .then(r => { if (!r.ok) throw new Error(); return r.json(); })
      .then(setData)
      .catch(() => setNotFound(true))
      .finally(() => setLoading(false));
  }, [params.document_id]);

  if (loading) return (
    <PublicPage subtitle="Document verification">
      <div role="status" aria-label="Verifying" className="flex justify-center py-20"><Loader2 className="size-6 animate-spin text-fg-subtle" aria-hidden /></div>
    </PublicPage>
  );

  if (notFound || !data) return (
    <PublicPage subtitle="Document verification" width="sm" center>
      <PublicNotice icon={AlertTriangle} tone="orange" title="Document not found">
        <p>This document ID does not match any signing record.</p>
      </PublicNotice>
    </PublicPage>
  );

  const isCompleted = data.status === "completed";

  return (
    <PublicPage subtitle="Document verification">
      <div className="space-y-4">
        <div role="status" className={cn("flex items-center gap-4 rounded-lg border p-5",
          isCompleted ? "border-tone-blue/30 bg-tone-blue-soft text-tone-blue" : "border-tone-orange/30 bg-tone-orange-soft text-tone-orange")}>
          <span className="flex size-12 shrink-0 items-center justify-center rounded-full bg-surface/60">
            {isCompleted ? <Shield className="size-6" aria-hidden /> : <AlertTriangle className="size-6" aria-hidden />}
          </span>
          <div>
            <h1 className="text-lg font-semibold">
              {isCompleted ? "Document fully executed" : `Document status: ${data.status.toUpperCase()}`}
            </h1>
            <p className="mt-0.5 text-sm">
              {isCompleted
                ? `All parties signed on ${data.completed_at ? new Date(data.completed_at).toLocaleDateString("en-GB", {day:"2-digit",month:"long",year:"numeric"}) : "-"}`
                : "Signing is not yet complete for this document."}
            </p>
          </div>
        </div>

        <Card>
          <CardHeader title="Document details" description={data.notice} />
          <CardBody>
            <dl className="divide-y divide-line-soft">
              {data.title && <Row label="Document title"><span className="font-medium">{data.title}</span></Row>}
              <Row label="Document ID"><span className="break-all font-mono text-xs text-fg-muted">{data.document_id}</span></Row>
              <Row label={`${data.hash_algorithm} hash`}><span className="break-all font-mono text-xs text-fg-muted">{data.document_hash}</span></Row>
              {data.signature_level_label && <Row label="Signature level"><span className="font-medium">{data.signature_level_label}</span></Row>}
              {data.digitally_sealed && (
                <Row label="Digital seal">
                  {data.seal_level_label || "Sealed"}{data.seal_authority ? ` · ${data.seal_authority}` : ""}
                  {data.timestamped && <span className="ml-1 text-tone-blue">· timestamped</span>}
                  {data.long_term_validation && <span className="ml-1 text-tone-blue">· LTV</span>}
                </Row>
              )}
              {data.has_certificate !== undefined && (
                <Row label="Certificate">
                  {data.has_certificate
                    ? <span className="inline-flex items-center gap-1"><CheckCircle2 className="size-3.5 text-tone-blue" aria-hidden /> Generated</span>
                    : "Not yet generated"}
                </Row>
              )}
              {data.consent && <Row label="E-records consent">{data.consent.agreed_count} signer(s) agreed ({data.consent.version})</Row>}
              {data.event_count !== undefined && <Row label="Audit events"><span className="tabular">{data.event_count}</span> recorded events</Row>}
            </dl>
            <div className="mt-4 rounded-lg border border-line bg-surface-2 p-3">
              <p className="text-xs text-fg-muted">
                <strong className="text-fg">Verify integrity:</strong> run <code className="rounded bg-surface px-1 font-mono">sha256sum &lt;original-document.pdf&gt;</code> and compare the output to the hash above. A match confirms the document has not been altered since signing.
              </p>
            </div>
          </CardBody>
        </Card>

        {(data.signers ?? []).length > 0 && (
          <Card>
            <CardHeader title="Signatories" />
            <CardBody className="py-2">
              <ul className="divide-y divide-line-soft">
                {(data.signers ?? []).map((s, i) => (
                  <li key={i} className="flex items-center gap-3 py-2.5">
                    <span className={cn("flex size-6 shrink-0 items-center justify-center rounded-full text-xs font-semibold tabular",
                      s.status === "signed" ? "bg-tone-blue-soft text-tone-blue" : "bg-surface-2 text-fg-muted")}>
                      {s.sign_order}
                    </span>
                    <div className="min-w-0 flex-1">
                      <p className="text-sm font-medium text-fg">{s.name}</p>
                      <p className="truncate text-xs text-fg-muted">{s.email}</p>
                    </div>
                    <div className="shrink-0 text-right">
                      {s.status === "signed" ? (
                        <span className="inline-flex items-center gap-1 text-xs font-medium text-tone-blue">
                          <CheckCircle2 className="size-3.5" aria-hidden />
                          {s.signed_at ? new Date(s.signed_at).toLocaleDateString("en-GB",{day:"2-digit",month:"short",year:"numeric"}) : "Signed"}
                        </span>
                      ) : (
                        <StatusPill family="sign" status={s.status} />
                      )}
                    </div>
                  </li>
                ))}
              </ul>
            </CardBody>
          </Card>
        )}

        {(data.legal_basis ?? []).length > 0 && (
          <div className="rounded-lg border border-line bg-surface-2 p-4">
            <p className="mb-2 text-xs font-semibold text-fg">Legal basis</p>
            <ul className="list-disc space-y-1.5 pl-4 marker:text-brand-ink">
              {(data.legal_basis || []).map((b, i) => (
                <li key={i} className="text-xs leading-relaxed text-fg-muted">{b}</li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </PublicPage>
  );
}
