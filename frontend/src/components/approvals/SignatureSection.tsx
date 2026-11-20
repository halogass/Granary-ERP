"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { CalendarPlus, CheckCircle2, Clock, Loader2, PenLine, XCircle } from "lucide-react";
import { signingFlowApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Pill, Select, type Tone } from "@/components/ui";
import { formatWhen } from "./types";

interface SignDoc {
  id: string;
  status: string;
  expires_at: string | null;
  completed_at: string | null;
  latest_deadline: string | null;
  can_extend: boolean;
  signers: { name: string; email: string; sign_order: number; status: string; signer_type: string; signed_at: string | null }[];
}

const DOC_STATUS: Record<string, { label: string; tone: Tone }> = {
  pending:   { label: "Out for signature", tone: "orange" },
  completed: { label: "Signed",            tone: "blue" },
  declined:  { label: "Declined",          tone: "red" },
  cancelled: { label: "Cancelled",         tone: "neutral" },
};

const FLOW_HINT: Record<string, string> = {
  counterparty: "Our approval is our signature (stamped with the QR). The counterparty is invited to sign before the deadline.",
  internal: "The designated signatory signs the approved document electronically.",
  internal_then_counterparty: "Our signatory signs first, then the counterparty is invited to sign before the deadline.",
};

interface Props {
  entityType: string;
  entityId: string;
  flow: string;
  /** The document is approved and unchanged. */
  canStart: boolean;
}

/** E-signature for an approved document: send, track signers, extend the counterparty deadline. */
export default function SignatureSection({ entityType, entityId, flow, canStart }: Props) {
  const [doc, setDoc] = useState<SignDoc | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [hidden, setHidden] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [extendDays, setExtendDays] = useState(3);

  const load = useCallback(async () => {
    try {
      const res = await signingFlowApi.status(entityType, entityId);
      setDoc(res.data.sign_document);
      setLoaded(true);
    } catch (e) {
      const status = (e as { response?: { status?: number } }).response?.status;
      if (status === 403 || status === 404) setHidden(true);
    }
  }, [entityType, entityId]);

  useEffect(() => { load(); }, [load, canStart]);

  if (hidden || !loaded) return null;

  async function run(action: () => Promise<unknown>, fallback: string) {
    setBusy(true);
    setError("");
    try {
      await action();
      await load();
    } catch (e) {
      setError(extractApiError(e, fallback));
    } finally {
      setBusy(false);
    }
  }

  const open = doc && doc.status === "pending";
  const meta = doc ? DOC_STATUS[doc.status] ?? { label: doc.status, tone: "neutral" as Tone } : null;

  return (
    <div className="space-y-3 border-t border-line pt-4">
      <div className="flex flex-wrap items-center gap-2">
        <PenLine className="size-4 text-fg-muted" aria-hidden />
        <span className="text-sm font-semibold text-fg">E-signature</span>
        {meta && <Pill tone={meta.tone}>{meta.label}</Pill>}
      </div>

      {doc && (
        <div className="space-y-2 rounded-lg border border-line p-3">
          <ol className="space-y-1.5">
            {doc.signers.map((s) => (
              <li key={`${s.sign_order}-${s.email}`} className="flex flex-wrap items-center gap-2 text-xs">
                {s.status === "signed" ? <CheckCircle2 className="size-3.5 text-tone-blue" aria-label="Signed" />
                  : s.status === "declined" ? <XCircle className="size-3.5 text-tone-red" aria-label="Declined" />
                  : <Clock className="size-3.5 text-fg-subtle" aria-hidden />}
                <span className="font-semibold text-fg">{s.name}</span>
                <span className="break-all text-fg-muted">{s.email}</span>
                <span className="text-fg-muted">
                  {s.status === "signed" ? `signed ${formatWhen(s.signed_at)}` : s.status}
                </span>
              </li>
            ))}
          </ol>
          {doc.expires_at && open && (
            <p className="text-xs text-fg-muted">Counterparty deadline {formatWhen(doc.expires_at)}</p>
          )}
          {doc.completed_at && <p className="text-xs text-fg-muted">Completed {formatWhen(doc.completed_at)}</p>}
          <div className="flex flex-wrap items-center gap-2">
            <Link href="/sign/documents" className="text-xs font-semibold text-brand-ink hover:underline">Open in Signing</Link>
            {doc.can_extend && (
              <span className="inline-flex items-center gap-1.5">
                <Select aria-label="Extend by" value={extendDays} onChange={(e) => setExtendDays(Number(e.target.value))} className="w-44"
                  options={[1, 2, 3, 5, 10].map((d) => ({ value: String(d), label: `${d} business day${d > 1 ? "s" : ""}` }))} />
                <Button size="sm" onClick={() => run(() => signingFlowApi.extend(doc.id, extendDays), "Could not extend the deadline.")}
                  disabled={busy}>
                  <CalendarPlus aria-hidden /> Extend
                </Button>
              </span>
            )}
          </div>
        </div>
      )}

      {!open && canStart && (
        <div className="space-y-2">
          {FLOW_HINT[flow] && <p className="text-xs text-fg-muted">{FLOW_HINT[flow]}</p>}
          <Button variant="primary" onClick={() => run(() => signingFlowApi.start(entityType, entityId), "Could not send for signature.")}
            disabled={busy}>
            {busy ? <Loader2 className="animate-spin" aria-hidden /> : <PenLine aria-hidden />}
            {doc ? "Send again for e-signature" : "Send for e-signature"}
          </Button>
        </div>
      )}
      {!doc && !canStart && <p className="text-xs text-fg-muted">It can be sent for e-signature once approved.</p>}

      {error && <p className="text-xs text-tone-red" role="alert">{error}</p>}
    </div>
  );
}
