"use client";

import { useCallback, useEffect, useState } from "react";
import { ChevronDown, Loader2, ShieldCheck, Send } from "lucide-react";
import { approvalsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import ApprovalTrail, { ApprovalStatusBadge } from "./ApprovalTrail";
import SignatureSection from "./SignatureSection";
import VerificationSection from "./VerificationSection";
import { Button, Checkbox } from "@/components/ui";
import { cn } from "@/lib/utils";
import { humanizeFlag, type ApprovalRequest, type EntityApproval } from "./types";

interface Props {
  entityType: string;
  entityId: string;
  /** Bump to reload, e.g. after a send attempt that may have submitted the document. */
  refreshKey?: number;
  /** Called after this panel changed the approval (submit / approve / reject / withdraw). */
  onChange?: () => void;
  className?: string;
}

const RESUBMITTABLE = ["not_submitted", "rejected", "cancelled", "stale"];

const GATE_HINT: Record<string, string> = {
  not_submitted: "Submitting is optional: it also happens automatically when you send or publish.",
  rejected: "Rejected. Revise the document and resubmit; approval restarts from the first step.",
  cancelled: "The last request was withdrawn.",
  stale: "The document changed after it was approved. Resubmit for approval.",
};

/** Approval state, trail and actions for one document. Renders nothing when no policy governs it. */
export default function ApprovalPanel({ entityType, entityId, refreshKey = 0, onChange, className = "mb-5" }: Props) {
  const [data, setData] = useState<EntityApproval | null>(null);
  const [hidden, setHidden] = useState(false);
  const [open, setOpen] = useState(false);
  const [flags, setFlags] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [problems, setProblems] = useState<string[]>([]);

  const load = useCallback(async () => {
    try {
      const res = await approvalsApi.forEntity(entityType, entityId);
      setData(res.data);
      const req: ApprovalRequest | null = res.data.request;
      if (req && (req.status === "pending" || req.can_act)) setOpen(true);
    } catch (e) {
      const status = (e as { response?: { status?: number } }).response?.status;
      if (status === 403 || status === 404) setHidden(true);
    }
  }, [entityType, entityId]);

  useEffect(() => { load(); }, [load, refreshKey]);

  if (hidden || !data || (!data.policy && !data.request)) return null;

  const gate = data.gate;
  const request = data.request;
  const canSubmit = !!data.policy && RESUBMITTABLE.includes(gate);
  const showTrail = !!request && request.status !== "superseded";
  const hasBody = showTrail || canSubmit || !!data.policy?.qr_enabled
    || (!!data.policy && data.policy.signature_flow !== "none");

  async function submit() {
    setBusy(true);
    setError("");
    setProblems([]);
    try {
      await approvalsApi.submit({ entity_type: entityType, entity_id: entityId, flags });
      setFlags([]);
      await load();
      setOpen(true);
      window.dispatchEvent(new Event("approvals:changed"));
      onChange?.();
    } catch (e) {
      const detail = (e as { response?: { data?: { detail?: { messages?: string[] } } } }).response?.data?.detail;
      if (detail && typeof detail === "object" && Array.isArray(detail.messages)) setProblems(detail.messages);
      else setError(extractApiError(e, "Could not submit for approval."));
    } finally {
      setBusy(false);
    }
  }

  function changed() {
    load();
    window.dispatchEvent(new Event("approvals:changed"));
    onChange?.();
  }

  return (
    <section className={cn("rounded-lg border border-line bg-surface shadow-card", className)} aria-label="Approval">
      <button type="button" onClick={() => setOpen((v) => !v)} aria-expanded={open}
        className="flex min-h-11 w-full flex-wrap items-center gap-3 rounded-lg px-4 py-2.5 text-left hover:bg-surface-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-[rgb(var(--focus)/0.7)]">
        <ShieldCheck className="size-4 shrink-0 text-fg-muted" aria-hidden />
        <span className="text-sm font-semibold text-fg">Approval</span>
        {gate !== "not_required" && <ApprovalStatusBadge status={gate} />}
        {data.policy?.sop_reference && (
          <span className="text-xs text-fg-muted">{data.policy.name} · {data.policy.sop_reference}</span>
        )}
        <ChevronDown aria-hidden className={cn("ml-auto size-4 text-fg-muted transition-transform", !open && "-rotate-90")} />
      </button>

      {/* 21 Nov 2026: with nothing to show (no request, nothing to submit, no verified PDF or signing) the open panel
          said so instead of showing an empty band */}
      {open && !hasBody && (
        <p className="border-t border-line px-4 py-3 text-xs text-fg-muted">No approval steps for this document.</p>
      )}
      {open && hasBody && (
        <div className="space-y-4 border-t border-line p-4 empty:hidden">
          {showTrail && <ApprovalTrail request={request!} onChanged={changed} />}

          {canSubmit && (
            <div className="space-y-3">
              {GATE_HINT[gate] && <p className="text-xs text-fg-muted">{GATE_HINT[gate]}</p>}
              {data.policy!.flags.length > 0 && (
                <div className="flex flex-wrap gap-x-4 gap-y-1.5">
                  {data.policy!.flags.map((f) => (
                    <Checkbox key={f} label={humanizeFlag(f)} checked={flags.includes(f)}
                      onChange={(e) => setFlags((prev) => e.target.checked ? [...prev, f] : prev.filter((x) => x !== f))} />
                  ))}
                </div>
              )}
              {problems.length > 0 && (
                <ul className="list-disc space-y-0.5 pl-4 text-xs text-tone-red" role="alert">
                  {problems.map((p) => <li key={p}>{p}</li>)}
                </ul>
              )}
              {error && <p className="text-xs text-tone-red" role="alert">{error}</p>}
              <Button onClick={submit} disabled={busy}>
                {busy ? <Loader2 className="animate-spin" aria-hidden /> : <Send aria-hidden />}
                {gate === "not_submitted" ? "Submit for approval" : "Resubmit for approval"}
              </Button>
            </div>
          )}

          {data.policy?.qr_enabled && (
            <VerificationSection entityType={entityType} entityId={entityId} canIssue={gate === "approved"} />
          )}

          {data.policy && data.policy.signature_flow !== "none" && (
            <SignatureSection entityType={entityType} entityId={entityId} flow={data.policy.signature_flow}
              canStart={gate === "approved"} />
          )}
        </div>
      )}
    </section>
  );
}
