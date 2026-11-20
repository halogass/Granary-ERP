import api from "@/lib/api";

/** 3 Nov 2026 (Stream A): the work queue (GET /nary/work-queue), what is waiting while internal Nary is paused. */
export type Urgency = "urgent" | "today" | "normal";

export interface QueueItem {
  id: string;
  kind: string;
  title: string;
  subject: Record<string, unknown>;
  since: string;
  urgency: Urgency;
  suggested_tool: string | null;
  suggested_args: Record<string, unknown>;
  hint: string;
  link: string;
}

export interface QueueResponse {
  total: number;
  matching: number;
  shown: number;
  by_urgency: Record<Urgency, number>;
  by_kind: Record<string, number>;
  nary_paused: boolean;
  scheduled_task_prompt: string;
  items: QueueItem[];
}

export const KIND_LABELS: Record<string, string> = {
  whatsapp_reply: "WhatsApp reply",
  email_reply: "Email reply",
  new_lead: "New lead",
  unknown_chat: "Unknown number",
  held_chat: "Held chat",
  followup_due: "Follow-up due",
  quotation_expiring: "Quotation expiring",
  quotation_expired: "Quotation expired",
  accepted_no_contract: "Accepted, no contract",
  contract_no_proforma: "Contract, no proforma",
  ready_to_win: "Ready to win",
  meeting_reminder: "Meeting reminder",
  meeting_brief: "Meeting brief",
  meeting_file: "Meeting file",
  duplicate_lead: "Duplicate leads",
  duplicate_deal: "Duplicate deals",
  card_pending: "Card waiting",
  card_expiring: "Card expiring",
  approval_overdue: "Overdue approval",
};

export const workQueueApi = {
  list: (params: { kinds?: string; urgency?: string; limit?: number }) =>
    api.get<QueueResponse>("/nary/work-queue", { params }),
};
