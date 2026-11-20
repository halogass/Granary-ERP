"use client";

import { useState } from "react";
import Link from "next/link";
import {
  CheckCircle2, XCircle, Clock, Bell, CircleDashed, MinusCircle, Loader2, Undo2, ExternalLink, AlertTriangle,
} from "lucide-react";
import { approvalsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import ReasonModal from "@/components/ui/ReasonModal";
import { Button, Pill, Textarea } from "@/components/ui";
import { cn } from "@/lib/utils";
import {
  APPROVAL_STATUS_META, formatWhen, humanizeFlag,
  type ApprovalRequest, type StepState,
} from "./types";

const STEP_META: Record<StepState, { icon: React.ElementType; className: string; label: string }> = {
  approved: { icon: CheckCircle2, className: "text-tone-blue bg-tone-blue-soft",       label: "Approved" },
  rejected: { icon: XCircle,      className: "text-tone-red bg-tone-red-soft",         label: "Rejected" },
  current:  { icon: Clock,        className: "text-tone-orange bg-tone-orange-soft",   label: "Waiting" },
  waiting:  { icon: CircleDashed, className: "text-fg-muted bg-surface-2",             label: "Queued" },
  skipped:  { icon: MinusCircle,  className: "text-fg-subtle bg-surface-2",            label: "Not reached" },
  notified: { icon: Bell,         className: "text-tone-violet bg-tone-violet-soft",   label: "Informed" },
};

const ACTION_LABEL: Record<string, string> = {
  submitted: "submitted", approved: "approved", rejected: "rejected", cancelled: "withdrew",
  superseded: "superseded", auto_approved: "auto-approved", stale_cancelled: "cancelled (document changed)",
};

export function ApprovalStatusBadge({ status }: { status: string }) {
  const meta = APPROVAL_STATUS_META[status] ?? APPROVAL_STATUS_META.not_submitted;
  return <Pill tone={meta.tone} data-status={status}>{meta.label}</Pill>;
}

interface Props {
  request: ApprovalRequest;
  /** Called with the updated request after an action, or with nothing when the caller should reload. */
  onChanged: (updated?: ApprovalRequest) => void;
  showDocumentLink?: boolean;
}

export default function ApprovalTrail({ request, onChanged, showDocumentLink = false }: Props) {
  const [comment, setComment] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [rejecting, setRejecting] = useState(false);
  const [withdrawing, setWithdrawing] = useState(false);

  async function approve() {
    setBusy(true);
    setError("");
    try {
      const res = await approvalsApi.approve(request.id, comment.trim() || undefined);
      setComment("");
      onChanged(res.data);
    } catch (e) {
      setError(extractApiError(e, "Could not approve."));
      onChanged();
    } finally {
      setBusy(false);
    }
  }

  const now = Date.now();

  return (
    <div className="space-y-4">
      {showDocumentLink && (
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs text-fg-muted">{request.entity_label}</p>
            <p className="mt-0.5 break-words text-sm font-semibold text-fg">{request.document_label}</p>
            <p className="mt-0.5 text-xs text-fg-muted">
              Submitted by {request.submitted_by.name ?? "-"} · {formatWhen(request.submitted_at)}
            </p>
          </div>
          <Button size="sm" asChild className="shrink-0">
            <Link href={request.link}>Open document <ExternalLink aria-hidden /></Link>
          </Button>
        </div>
      )}

      {request.flags.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {request.flags.map((f) => (
            <Pill key={f}>{humanizeFlag(f)}</Pill>
          ))}
        </div>
      )}

      {request.steps.length > 0 && <ol className="space-y-2">
        {request.steps.map((step) => {
          const meta = STEP_META[step.state] ?? STEP_META.waiting;
          const Icon = meta.icon;
          const overdue = step.state === "current" && step.due_at && new Date(step.due_at).getTime() < now;
          return (
            <li key={step.seq} className="flex gap-3">
              <span className={cn("flex size-7 shrink-0 items-center justify-center rounded-lg", meta.className)}>
                <Icon className="size-3.5" aria-hidden />
              </span>
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                  <span className="text-sm font-semibold text-fg">{step.name}</span>
                  <span className="text-xs text-fg-muted">{meta.label}</span>
                  {overdue && (
                    <span className="inline-flex items-center gap-1 text-xs font-semibold text-tone-red">
                      <AlertTriangle className="size-3" aria-hidden /> Overdue
                    </span>
                  )}
                </div>
                {step.reasons.length > 0 && (
                  <p className="mt-0.5 break-words text-xs text-fg-muted">{step.reasons.join(" · ")}</p>
                )}
                <p className="mt-0.5 break-words text-xs text-fg-subtle">
                  {step.decided_by
                    ? `${step.decided_by.name ?? "-"} · ${formatWhen(step.decided_at)}`
                    : step.candidates.map((c) => c.name ?? "-").join(", ")}
                  {step.state === "current" && step.due_at && !overdue && ` · due ${formatWhen(step.due_at)}`}
                </p>
                {step.routing_note && <p className="mt-0.5 text-xs text-tone-orange">{step.routing_note}</p>}
              </div>
            </li>
          );
        })}
      </ol>}

      {request.actions.some((a) => a.comment) && (
        <div className="space-y-2 border-t border-line pt-3">
          {request.actions.filter((a) => a.comment).map((a, i) => (
            <div key={i} className="text-xs">
              <span className="font-semibold text-fg">{a.actor?.name ?? "System"}</span>{" "}
              <span className="text-fg-muted">{ACTION_LABEL[a.action] ?? a.action} · {formatWhen(a.at)}</span>
              <p className="mt-0.5 whitespace-pre-line break-words text-fg">{a.comment}</p>
            </div>
          ))}
        </div>
      )}

      {error && <p className="text-xs text-tone-red" role="alert">{error}</p>}

      {request.can_act && (
        <div className="space-y-2 border-t border-line pt-3">
          <Textarea value={comment} onChange={(e) => setComment(e.target.value)} rows={2}
            aria-label="Comment" placeholder="Comment (optional when approving)" className="resize-none" />
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" onClick={approve} disabled={busy}>
              {busy ? <Loader2 className="animate-spin" aria-hidden /> : <CheckCircle2 aria-hidden />} Approve
            </Button>
            <Button variant="danger" onClick={() => setRejecting(true)} disabled={busy}>
              <XCircle aria-hidden /> Reject
            </Button>
          </div>
        </div>
      )}

      {request.can_cancel && !request.can_act && (
        <Button size="sm" variant="ghost" className="text-fg-muted" onClick={() => setWithdrawing(true)}>
          <Undo2 aria-hidden /> Withdraw request
        </Button>
      )}

      {rejecting && (
        <ReasonModal
          title="Reject"
          description="The document goes back to its author, and the approval restarts from the first step when it is resubmitted."
          label="Reason *"
          confirmText="Reject"
          onClose={() => setRejecting(false)}
          onConfirm={async (reason) => {
            try {
              const res = await approvalsApi.reject(request.id, reason);
              setRejecting(false);
              onChanged(res.data);
            } catch (e) {
              throw new Error(extractApiError(e, "Could not reject."));
            }
          }}
        />
      )}

      {withdrawing && (
        <ReasonModal
          title="Withdraw approval request"
          label="Reason *"
          confirmText="Withdraw"
          onClose={() => setWithdrawing(false)}
          onConfirm={async (reason) => {
            try {
              const res = await approvalsApi.cancel(request.id, reason);
              setWithdrawing(false);
              onChanged(res.data);
            } catch (e) {
              throw new Error(extractApiError(e, "Could not withdraw."));
            }
          }}
        />
      )}
    </div>
  );
}
