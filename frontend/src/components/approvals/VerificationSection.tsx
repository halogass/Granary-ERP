"use client";

import { useCallback, useEffect, useState } from "react";
import { Ban, Check, Download, Link2, Loader2, QrCode, RefreshCw } from "lucide-react";
import { verificationApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import ReasonModal from "@/components/ui/ReasonModal";
import { Button, Pill, type Tone } from "@/components/ui";
import { formatWhen } from "./types";

interface Verification {
  id: string;
  status: string;
  live_state: string | null;
  short_code: string;
  url: string | null;
  document_number: string | null;
  revision: number;
  issued_at: string | null;
  expires_at: string | null;
  revoked_at: string | null;
  revoke_reason: string | null;
  view_count: number;
  last_viewed_at: string | null;
  allow_download: boolean;
  requires_login: boolean;
  has_pdf: boolean;
}

const STATE_META: Record<string, { label: string; tone: Tone }> = {
  valid:        { label: "Verifying",               tone: "blue" },
  revoked:      { label: "Revoked",                 tone: "red" },
  superseded:   { label: "Superseded",              tone: "neutral" },
  expired:      { label: "Expired",                 tone: "neutral" },
  void:         { label: "Document cancelled",      tone: "red" },
  not_approved: { label: "No longer matches approval", tone: "orange" },
};

function StateBadge({ state }: { state: string }) {
  const meta = STATE_META[state] ?? { label: state, tone: "neutral" as Tone };
  return <Pill tone={meta.tone}>{meta.label}</Pill>;
}

interface Props {
  entityType: string;
  entityId: string;
  /** The document is approved and unchanged, so a new verified PDF may be issued. */
  canIssue: boolean;
}

/** Verified PDFs (QR on every page) issued for one document: issue, download, copy link, revoke. */
export default function VerificationSection({ entityType, entityId, canIssue }: Props) {
  const [rows, setRows] = useState<Verification[] | null>(null);
  const [hidden, setHidden] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [revoking, setRevoking] = useState<Verification | null>(null);

  const load = useCallback(async () => {
    try {
      const res = await verificationApi.forEntity(entityType, entityId);
      if (!res.data.supported || !res.data.qr_enabled) { setHidden(true); return; }
      setRows(res.data.verifications ?? []);
    } catch (e) {
      const status = (e as { response?: { status?: number } }).response?.status;
      if (status === 403 || status === 404) setHidden(true);
    }
  }, [entityType, entityId]);

  useEffect(() => { load(); }, [load, canIssue]);

  if (hidden || rows === null) return null;

  const active = rows.find((r) => r.status === "active");
  const past = rows.filter((r) => r !== active);

  async function issue() {
    setBusy(true);
    setError("");
    try {
      await verificationApi.issue(entityType, entityId);
      await load();
    } catch (e) {
      setError(extractApiError(e, "Could not issue the verified document."));
    } finally {
      setBusy(false);
    }
  }

  async function download(v: Verification) {
    setError("");
    try {
      const res = await verificationApi.downloadPdf(v.id);
      const url = URL.createObjectURL(res.data as Blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${v.document_number ?? "document"}.pdf`;
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 30_000);
    } catch (e) {
      setError(extractApiError(e, "Could not download the document."));
    }
  }

  async function copyLink(v: Verification) {
    if (!v.url) return;
    try {
      await navigator.clipboard.writeText(v.url);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch { /* clipboard unavailable */ }
  }

  return (
    <div className="space-y-3 border-t border-line pt-4">
      <div className="flex flex-wrap items-center gap-2">
        <QrCode className="size-4 text-fg-muted" aria-hidden />
        <span className="text-sm font-semibold text-fg">Verified document</span>
      </div>

      {active ? (
        <div className="space-y-2 rounded-lg border border-line p-3">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-semibold text-fg">{active.document_number}</span>
            <StateBadge state={active.live_state ?? "valid"} />
            {active.requires_login && <span className="text-xs text-fg-muted">Login required to view</span>}
          </div>
          <p className="text-xs text-fg-muted">
            Issued {formatWhen(active.issued_at)}
            {active.expires_at && ` · expires ${formatWhen(active.expires_at)}`}
            {` · scanned ${active.view_count}×`}
            {" · code "}<span className="font-mono">{active.short_code}</span>
          </p>
          <div className="flex flex-wrap gap-2">
            {active.has_pdf && (
              <Button size="sm" onClick={() => download(active)}><Download aria-hidden /> Download PDF</Button>
            )}
            {active.url && (
              <Button size="sm" onClick={() => copyLink(active)}>
                {copied ? <Check className="text-tone-blue" aria-hidden /> : <Link2 aria-hidden />}
                {copied ? "Copied" : "Copy verify link"}
              </Button>
            )}
            {canIssue && (
              <Button size="sm" onClick={issue} disabled={busy}>
                {busy ? <Loader2 className="animate-spin" aria-hidden /> : <RefreshCw aria-hidden />} Reissue
              </Button>
            )}
            <Button size="sm" variant="danger" onClick={() => setRevoking(active)}><Ban aria-hidden /> Revoke</Button>
          </div>
        </div>
      ) : canIssue ? (
        <div className="space-y-2">
          <p className="text-xs text-fg-muted">
            Render the approved document with a QR code on every page. Anyone who scans it can confirm it is authentic and current.
          </p>
          <Button onClick={issue} disabled={busy}>
            {busy ? <Loader2 className="animate-spin" aria-hidden /> : <QrCode aria-hidden />} Issue verified PDF
          </Button>
        </div>
      ) : (
        <p className="text-xs text-fg-muted">A verified PDF can be issued once the document is approved.</p>
      )}

      {past.length > 0 && (
        <details className="text-xs">
          <summary className="cursor-pointer text-fg-muted hover:text-fg">Earlier codes ({past.length})</summary>
          <ul className="mt-2 space-y-1.5">
            {past.map((v) => (
              <li key={v.id} className="flex flex-wrap items-center gap-2 text-fg-muted">
                <span className="font-mono">{v.short_code}</span>
                <StateBadge state={v.live_state ?? v.status} />
                <span>{formatWhen(v.issued_at)}</span>
                {v.revoke_reason && <span className="text-fg-subtle">: {v.revoke_reason}</span>}
              </li>
            ))}
          </ul>
        </details>
      )}

      {error && <p className="text-xs text-tone-red" role="alert">{error}</p>}

      {revoking && (
        <ReasonModal
          title="Revoke verification code"
          description="Anyone scanning this document's QR code will be told it is no longer valid. This cannot be undone."
          confirmText="Revoke"
          onClose={() => setRevoking(null)}
          onConfirm={async (reason) => {
            try {
              await verificationApi.revoke(revoking.id, reason);
              setRevoking(null);
              await load();
            } catch (e) {
              throw new Error(extractApiError(e, "Could not revoke."));
            }
          }}
        />
      )}
    </div>
  );
}
