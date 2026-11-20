import type { Tone } from "@/components/ui/status";

// Shapes returned by the approvals API (backend/app/services/approvals/engine.py `serialize`).

export type StepState = "current" | "waiting" | "approved" | "rejected" | "skipped" | "notified";

export interface ApprovalPerson { id: string | null; name?: string | null }

export interface ApprovalStep {
  seq: number;
  name: string;
  is_notify_only: boolean;
  reasons: string[];
  routing_note?: string | null;
  candidates: ApprovalPerson[];
  due_at?: string | null;
  decided_at?: string | null;
  decided_by?: ApprovalPerson | null;
  decision?: string | null;
  state: StepState;
}

export interface ApprovalActionRow {
  action: string;
  step_seq: number | null;
  comment: string | null;
  actor: ApprovalPerson | null;
  at: string | null;
}

export interface ApprovalRequest {
  id: string;
  entity_type: string;
  entity_label: string;
  entity_id: string;
  document_label: string;
  document_revision: number | null;
  link: string;
  status: "pending" | "approved" | "rejected" | "cancelled" | "superseded";
  submitted_by: ApprovalPerson;
  submitted_at: string | null;
  completed_at: string | null;
  current_step_seq: number | null;
  flags: string[];
  amount: number | null;
  currency: string | null;
  amount_base: number | null;
  steps: ApprovalStep[];
  actions: ApprovalActionRow[];
  can_act: boolean;
  can_cancel: boolean;
}

export type ApprovalGate =
  | "not_required" | "not_submitted" | "pending" | "approved" | "rejected" | "cancelled" | "stale";

export interface EntityApproval {
  policy: {
    id: string; name: string; sop_reference: string | null;
    signature_flow: string; qr_enabled: boolean; flags: string[];
  } | null;
  gate: ApprovalGate;
  request: ApprovalRequest | null;
  history: { id: string; status: string; document_revision: number | null; submitted_at: string; completed_at: string | null }[];
}

/** Labels and tones of an approval gate/request status (tones follow components/ui/status.ts). */
export const APPROVAL_STATUS_META: Record<string, { label: string; tone: Tone }> = {
  pending:       { label: "Pending approval", tone: "orange" },
  approved:      { label: "Approved",         tone: "blue" },
  rejected:      { label: "Rejected",         tone: "red" },
  cancelled:     { label: "Withdrawn",        tone: "neutral" },
  superseded:    { label: "Superseded",       tone: "neutral" },
  stale:         { label: "Changed since approval", tone: "orange" },
  not_submitted: { label: "Not submitted",    tone: "neutral" },
};

export function humanizeFlag(flag: string): string {
  const text = flag.replace(/_/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export function formatWhen(iso?: string | null): string {
  if (!iso) return "";
  return new Date(iso).toLocaleString("en-GB", {
    day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit",
  });
}
