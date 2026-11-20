/**
 * The CRM funnel vocabulary, as served by GET /crm/stages (backend app/services/crm/stages.py).
 * The backend owns the stages, their order, what counts as won or closed, the default
 * probabilities and the reason / source picklists; the UI only adds colours.
 */

export interface DealStage {
  key: string;
  label: string;
  order: number;
  is_closed: boolean;
  is_won: boolean;
  side: boolean;            // nurture / lost / disqualified: not a rung of the ladder
  probability: number | null;
}

export interface LeadStatus {
  key: string;
  label: string;
  order: number;
  is_closed: boolean;
  is_active: boolean;
}

export interface FunnelReason {
  key: string;
  label: string;
  recontact_months: number | null;
}

export interface FunnelSource {
  key: string;
  label: string;
}

export interface FunnelRegistry {
  deal_stages: DealStage[];
  lead_statuses: LeadStatus[];
  lead_transitions: Record<string, string[]>;
  lead_start_statuses: string[];
  lead_sources: FunnelSource[];
  loss_reasons: FunnelReason[];
  disqualify_reasons: FunnelReason[];
}

export const EMPTY_REGISTRY: FunnelRegistry = {
  deal_stages: [], lead_statuses: [], lead_transitions: {}, lead_start_statuses: [],
  lead_sources: [], loss_reasons: [], disqualify_reasons: [],
};

/** Stages whose move needs reasons (picklist in the registry). */
export const REASON_STAGES: Record<string, "loss_reasons" | "disqualify_reasons"> = {
  lost: "loss_reasons",
  disqualified: "disqualify_reasons",
};


/** Label for a deal stage or lead status; an unknown key shows as itself. Colours: status.ts. */
export function stageStyle(reg: FunnelRegistry, key: string | null | undefined) {
  const found = reg.deal_stages.find((s) => s.key === key) ?? reg.lead_statuses.find((s) => s.key === key);
  return { key: key ?? "", label: found?.label ?? (key ?? "").replace(/_/g, " ") };
}

export function isWonStage(reg: FunnelRegistry, key: string) {
  return !!reg.deal_stages.find((s) => s.key === key)?.is_won;
}

/** Open and on the ladder: what counts as the active pipeline. */
export function isActiveStage(reg: FunnelRegistry, key: string) {
  const s = reg.deal_stages.find((x) => x.key === key);
  return !!s && !s.is_closed && !s.side;
}

/** The kanban columns: the ladder plus nurture; lost and disqualified are reached from the form. */
export function boardStages(reg: FunnelRegistry): DealStage[] {
  return [...reg.deal_stages].filter((s) => !(s.side && s.is_closed)).sort((a, b) => a.order - b.order);
}

/** Won out of everything decided (won + lost + disqualified); null when nothing is decided yet. */
export function winRate(reg: FunnelRegistry, stages: string[]): number | null {
  const decided = stages.filter((k) => reg.deal_stages.find((s) => s.key === k)?.is_closed);
  if (!decided.length) return null;
  return Math.round(decided.filter((k) => isWonStage(reg, k)).length / decided.length * 100);
}

export function canMoveLead(reg: FunnelRegistry, from: string, to: string) {
  return (reg.lead_transitions[from] ?? []).includes(to);
}
