/**
 * One color mapping per status family, so the same status looks the same on every page.
 * Tones: neutral (grey: draft, closed, quiet), blue (active, in progress), violet (AI / proposal /
 * decision stage), orange (waiting on someone, due soon), red (needs you, overdue, failed, rejected).
 * Red is reserved for those; never use it for "fine".
 */
export type Tone = "neutral" | "blue" | "violet" | "orange" | "red";

export type StatusFamily =
  | "deal" | "lead" | "invoice" | "quotation" | "proposal" | "contract" | "approval" | "nary_card"
  | "task" | "project" | "ticket" | "payment" | "purchase" | "leave" | "employee" | "sign"
  | "journal" | "expense" | "asset" | "budget" | "period" | "bank_statement" | "schedule"
  | "temperature" | "followup" | "meeting" | "meeting_file" | "knowledge" | "doc_version" | "license" | "subscription" | "api_key"
  | "memo" | "offer" | "job_opening" | "job_request" | "applicant" | "payroll" | "attendance" | "face_check" | "priority"
  | "partner_deal" | "partner_lead" | "mdf" | "rfq_invite"
  | "production" | "unit" | "qc" | "warranty" | "campaign" | "content" | "email_campaign" | "email_recipient"
  | "rnd_item" | "sprint" | "release"
  | "receipt" | "contractor_request" | "work_order" | "vendor_invoice" | "rfq" | "partner_tier"
  | "vehicle" | "equipment" | "maintenance_request"
  | "generic";

const generic: Record<string, Tone> = {
  draft: "neutral", new: "neutral", open: "blue", active: "blue", in_progress: "blue", running: "blue",
  pending: "orange", waiting: "orange", submitted: "orange", under_review: "orange", scheduled: "blue", sent: "blue",
  done: "neutral", completed: "neutral", closed: "neutral", archived: "neutral", inactive: "neutral",
  cancelled: "neutral", canceled: "neutral", expired: "neutral", superseded: "neutral", paused: "neutral",
  approved: "blue", accepted: "blue", confirmed: "blue", paid: "blue", won: "blue", resolved: "neutral",
  rejected: "red", declined: "red", failed: "red", error: "red", overdue: "red", lost: "neutral", blocked: "red",
  urgent: "red", high: "orange", normal: "neutral", medium: "neutral", low: "neutral",
};

export const STATUS_TONES: Record<StatusFamily, Record<string, Tone>> = {
  deal: {
    new: "neutral", triaging: "violet", qualifying: "violet", discovery: "blue", assessment: "blue",
    proposal: "orange", contract: "orange", won: "blue", handover: "blue", onboarding: "blue",
    promoted: "blue", nurture: "neutral", lost: "neutral", disqualified: "neutral",
  },
  lead: {
    new: "neutral", contacted: "blue", triaging: "violet", qualifying: "violet", qualified: "blue",
    nurture: "neutral", promoted: "blue", converted: "blue", disqualified: "neutral", lost: "neutral",
  },
  invoice: {
    draft: "neutral", proforma: "violet", sent: "blue", issued: "blue", partially_paid: "orange", partial: "orange",
    paid: "blue", overdue: "red", cancelled: "neutral", void: "neutral", converted: "neutral", reversed: "neutral",
  },
  quotation: {
    // approved (17 Nov 2026) = approved by the Owner, not sent yet: violet so it reads apart from Sent
    draft: "neutral", pending_approval: "orange", approved: "violet", sent: "blue", accepted: "blue", signed: "blue",
    rejected: "red", declined: "red", expired: "neutral", superseded: "neutral", invoiced: "neutral", converted: "neutral",
    cancelled: "neutral",
  },
  proposal: {
    draft: "neutral", generating: "violet", review: "orange", sent: "blue", accepted: "blue",
    rejected: "red", superseded: "neutral",
  },
  contract: {
    draft: "neutral", sent: "blue", sent_for_signing: "orange", signed: "blue", active: "blue", expired: "neutral",
    terminated: "neutral",
  },
  approval: {
    pending: "orange", awaiting_me: "red", approved: "blue", rejected: "red", returned: "orange",
    cancelled: "neutral", not_required: "neutral",
  },
  nary_card: {
    pending: "red", scheduled: "orange", applying: "blue", applied: "blue", rejected: "neutral", expired: "neutral",
    failed: "red", shadow: "violet", cancelled: "neutral",
  },
  task: { todo: "neutral", backlog: "neutral", in_progress: "blue", review: "violet", blocked: "red", done: "neutral", cancelled: "neutral" },
  project: { planning: "neutral", active: "blue", on_hold: "orange", completed: "neutral", cancelled: "neutral", archived: "neutral" },
  ticket: { new: "neutral", open: "blue", pending: "orange", on_hold: "orange", resolved: "neutral", closed: "neutral", breached: "red" },
  payment: { pending: "orange", completed: "blue", confirmed: "blue", failed: "red", refunded: "neutral" },
  purchase: {
    draft: "neutral", sent: "blue", approved: "blue", confirmed: "blue", partial: "orange", partially_received: "orange",
    received: "neutral", cancelled: "neutral",
  },
  leave: { pending: "orange", approved: "blue", rejected: "red", cancelled: "neutral" },
  employee: { active: "blue", probation: "violet", on_leave: "orange", resigned: "neutral", terminated: "neutral" },
  sign: { draft: "neutral", sent: "orange", viewed: "violet", signed: "blue", completed: "blue", declined: "red", expired: "neutral" },
  journal: { draft: "neutral", posted: "blue", reversed: "neutral", void: "neutral" },
  expense: {
    draft: "neutral", pending: "orange", submitted: "orange", approved: "blue", rejected: "red",
    reimbursed: "neutral", paid: "neutral", cancelled: "neutral",
  },
  asset: { active: "blue", fully_depreciated: "neutral", disposed: "neutral", draft: "neutral" },
  budget: { draft: "neutral", approved: "violet", active: "blue", closed: "neutral" },
  period: { open: "blue", closed: "neutral", locked: "neutral" },
  bank_statement: { draft: "neutral", open: "orange", in_progress: "orange", reconciled: "blue" },
  schedule: { pending: "orange", paid: "blue", overdue: "red", cancelled: "neutral", invoiced: "neutral", waived: "neutral" },
  // A lead's AI score: hot needs you now
  temperature: { hot: "red", warm: "orange", cold: "neutral" },
  // CRM follow-up reminders: due and overdue need you
  followup: { open: "blue", upcoming: "blue", today: "orange", overdue: "red", done: "neutral", cancelled: "neutral" },
  // Nary meetings: proposed slots wait for your approval
  meeting: { proposed: "red", slots_sent: "orange", booked: "blue", done: "neutral", no_show: "orange", cancelled: "neutral" },
  // Knowledge base articles and SOPs: one waiting for approval is with the approver
  knowledge: { draft: "neutral", review: "orange", pending_approval: "orange", published: "blue", archived: "neutral" },
  // Document library versions: one waiting for an executive's approval is orange, a rejected one needs the owner
  doc_version: { draft: "neutral", pending_approval: "orange", live: "blue", superseded: "neutral", rejected: "red" },
  // Meeting recordings and notes files Nary transcribes and reads
  meeting_file: { stored: "neutral", queued: "orange", transcribing: "blue", analysing: "violet", done: "neutral", failed: "red" },
  // Product licences, subscriptions and API keys; an expiring licence is a renewal to chase
  license: { active: "blue", inactive: "orange", expired: "orange", revoked: "neutral" },
  subscription: { active: "blue", trial: "violet", past_due: "red", paused: "neutral", cancelled: "neutral" },
  api_key: { active: "blue", revoked: "neutral", expired: "neutral" },
  // Formal memos in messaging: one not yet acknowledged waits on the recipient
  memo: { sent: "orange", partially_acknowledged: "orange", acknowledged: "blue", recalled: "neutral" },
  // HR: an offer the candidate countered waits on HR; a declined offer is a lost hire
  offer: { draft: "neutral", sent: "blue", countered: "orange", accepted: "blue", declined: "red", expired: "neutral", cancelled: "neutral" },
  job_opening: { open: "blue", paused: "orange", filled: "neutral", cancelled: "neutral", closed: "neutral" },
  // Hiring requests wait for the Owner
  job_request: { pending_approval: "orange", pending: "orange", on_hold: "neutral", approved: "blue", rejected: "red", fulfilled: "neutral" },
  // Recruitment pipeline: early stages quiet, interviews in progress, offers wait on the candidate
  applicant: {
    applied: "neutral", screening: "violet", phone_screen: "violet", interview_1: "blue", interview_2: "blue",
    technical_test: "blue", offer_sent: "orange", offer_accepted: "blue", hired: "blue", rejected: "neutral", withdrawn: "neutral",
  },
  payroll: { draft: "neutral", calculating: "orange", approved: "violet", paid: "blue", cancelled: "neutral" },
  attendance: {
    present: "blue", remote: "blue", late: "orange", half_day: "violet", on_leave: "violet", holiday: "neutral", absent: "red",
  },
  // A face clock-in: failures and pending overrides need HR
  face_check: {
    matched: "blue", overridden: "violet", low_confidence: "orange", pending_override: "red", unrecognized: "red",
    liveness_failed: "red", kiosk_mismatch: "red", duplicate: "neutral", no_clock_in: "neutral", rejected: "neutral",
  },
  // Partner portal: a deal registration waits on the company's review; an assigned lead waits on the partner
  partner_deal: {
    pending: "orange", approved: "blue", extended: "blue", rejected: "red", expired: "neutral",
    won: "blue", closed_won: "blue", closed_lost: "neutral",
  },
  partner_lead: { new: "orange", contacted: "blue", qualified: "violet", converted: "neutral", dead: "neutral" },
  // Vendor portal: an RFQ invite waits on the vendor's quote
  rfq_invite: { invited: "orange", responded: "blue", awarded: "blue", declined: "neutral", no_response: "neutral" },
  // Partner marketing funds: requests and claims wait on the company's review
  mdf: {
    draft: "neutral", submitted: "orange", under_review: "orange", approved: "blue", rejected: "red",
    completed: "neutral", cancelled: "neutral", paid: "blue",
  },
  priority: { critical: "red", urgent: "red", high: "orange", medium: "neutral", normal: "neutral", low: "neutral" },
  // Manufacturing: a failed QC and an open RMA are the ones to act on
  production: { planned: "neutral", component_check: "orange", in_production: "blue", qc: "violet", completed: "neutral", cancelled: "neutral" },
  unit: {
    in_production: "blue", qc_pending: "orange", qc_passed: "blue", qc_failed: "red", rework: "orange",
    in_stock: "neutral", reserved: "violet", shipped: "blue", returned: "neutral",
  },
  qc: { pass: "blue", fail: "red", conditional: "orange" },
  warranty: { active: "blue", expired: "neutral", rma_open: "orange", rma_closed: "neutral", voided: "neutral" },
  // Marketing campaigns, the content calendar and email campaigns
  campaign: { planned: "neutral", draft: "neutral", active: "blue", paused: "orange", completed: "neutral", cancelled: "neutral" },
  content: {
    planned: "neutral", idea: "neutral", draft: "neutral", in_progress: "blue", review: "orange", approved: "violet",
    scheduled: "blue", published: "neutral", cancelled: "neutral",
  },
  email_campaign: { draft: "neutral", scheduled: "blue", sending: "violet", sent: "blue", failed: "red" },
  email_recipient: { pending: "neutral", sent: "blue", opened: "violet", failed: "red", skipped: "neutral" },
  // R&D backlog items, sprints and releases
  rnd_item: { backlog: "neutral", refined: "neutral", in_sprint: "violet", in_progress: "blue", review: "orange", done: "neutral", released: "neutral" },
  sprint: { planning: "neutral", active: "blue", completed: "neutral" },
  release: { draft: "neutral", rc: "violet", released: "blue", yanked: "red" },
  // Procurement: requests and vendor invoices wait on the company's review; RFQs wait on vendor quotes
  receipt: { draft: "neutral", pending: "orange", confirmed: "blue", cancelled: "neutral" },
  contractor_request: {
    pending_approval: "orange", approved: "blue", rejected: "red", in_progress: "violet", fulfilled: "neutral", cancelled: "neutral",
  },
  // Vendor and maintenance work orders: planned work is scheduled, a paused one waits on someone
  work_order: {
    draft: "neutral", planned: "violet", active: "blue", in_progress: "blue", paused: "orange", completed: "neutral", cancelled: "neutral",
  },
  vendor_invoice: {
    submitted: "orange", under_review: "violet", approved: "blue", partially_paid: "orange", paid: "neutral", rejected: "red",
  },
  rfq: { draft: "neutral", sent: "blue", evaluating: "orange", closed: "neutral", cancelled: "neutral" },
  // Partner tiers (accounts, resources and training gated by tier)
  partner_tier: { registered: "neutral", authorized: "neutral", silver: "neutral", gold: "blue", platinum: "violet" },
  // Fleet and maintenance: a vehicle or machine in maintenance is out of service
  vehicle: { active: "blue", maintenance: "orange", inactive: "neutral" },
  equipment: { active: "blue", under_maintenance: "orange", decommissioned: "neutral" },
  maintenance_request: { submitted: "orange", under_review: "violet", approved: "blue", rejected: "red" },
  generic,
};

/** The tone for a status in a family; unknown statuses fall back to the generic map, then neutral. */
export function statusTone(family: StatusFamily, status: string | null | undefined): Tone {
  const key = (status ?? "").toLowerCase().replace(/[\s-]+/g, "_");
  return STATUS_TONES[family]?.[key] ?? generic[key] ?? "neutral";
}

/** "partially_paid" -> "Partially paid" (when the caller has no label of its own). */
export function statusLabel(status: string | null | undefined): string {
  const s = (status ?? "").replace(/_/g, " ").trim();
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : "-";
}

export const TONE_CLASSES: Record<Tone, string> = {
  neutral: "bg-tone-neutral-soft text-tone-neutral",
  blue: "bg-tone-blue-soft text-tone-blue",
  violet: "bg-tone-violet-soft text-tone-violet",
  orange: "bg-tone-orange-soft text-tone-orange",
  red: "bg-tone-red-soft text-tone-red font-semibold",
};
