/** Shapes served by backend routers/nary.py. */
export interface NaryCard {
  id: string;
  action: string;
  label: string;
  origin: "nary" | "mcp";
  status: "pending" | "scheduled" | "applied" | "rejected" | "shadow" | "failed" | "expired" | "applying";
  level: number;
  customer_facing: boolean;
  payload: Record<string, unknown>;
  edited_payload: Record<string, unknown> | null;
  rationale: string | null;
  sop_ref: string | null;
  sources: { type?: string; id?: string; label?: string }[];
  diff: { label?: string; title?: string; currency?: string; before?: Record<string, unknown>; after?: Record<string, unknown>; error?: string } | null;
  price: Record<string, unknown> | null;
  entity_type: string | null;
  entity_id: string | null;
  run_id: string | null;
  created_at: string;
  decided_at: string | null;
  reject_reason: string | null;
  edit_ratio: number | null;
  result: Record<string, unknown> | null;
  error: string | null;
  actual_text: string | null;
  diff_ratio: number | null;
}

export const LEVEL_LABELS: Record<number, string> = { 0: "Shadow", 1: "Ask", 2: "Act and tell", 3: "Silent" };
