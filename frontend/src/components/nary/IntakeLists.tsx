"use client";

import { Lock, ShieldAlert, Undo2 } from "lucide-react";
import { Button, EmptyState, Select } from "@/components/ui";

/** The Intake page's lists (Nary Phase 2). Held chats show only who, never what: Nary hasn't read
 *  them and neither does this page. */
export interface Held {
  chat_id: string; phone: string | null; name: string | null; saved_on_phone: boolean; triage: string | null;
  last_inbound_at: string | null;
}
export interface Verdict {
  id: string; subject_type: string; subject_ref: string; verdict: string; reason: string | null; excerpt: string | null;
  cascaded: boolean; corrected_to: string | null; created_at: string;
}
export interface Link {
  id: string; chat_id: string; tier: string; score: number; status: string; cascaded: boolean; review: string | null;
  target: string | null; signals: { signal: string; score: number; detail: string }[]; created_at: string;
}

export const VERDICT_LABEL: Record<string, string> = {
  prospect: "Prospect", customer: "Customer", partner: "Partner", unclear: "Unclear", vendor: "Vendor",
  job_seeker: "Job seeker", spam: "Spam", scam: "Scam", wrong_number: "Wrong number", personal: "Personal",
};
const TIER: Record<string, string> = { certain: "Certain", likely: "Likely (checked by a second model)", weak: "Weak (your call)" };
const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "");

const LIST = "overflow-hidden rounded-lg border border-line bg-surface";
const ROW = "border-b border-line-soft px-4 py-3 last:border-0 even:bg-surface-2/60";

export function HeldChats({ rows, canDecide, onDecide }: {
  rows: Held[]; canDecide: boolean; onDecide: (chatId: string, business: boolean) => void;
}) {
  if (rows.length === 0) return <EmptyState title="No chats waiting for you." compact />;
  return (
    <ul className={LIST}>
      {rows.map((c) => (
        <li key={c.chat_id} className={`${ROW} flex flex-wrap items-center gap-3`}>
          <Lock className="size-4 shrink-0 text-fg-subtle" aria-hidden />
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium text-fg">{c.name || c.phone || c.chat_id}</p>
            <p className="text-xs text-fg-muted">
              {c.phone}{c.saved_on_phone ? " · saved on the company phone" : ""}{c.triage === "personal" ? " · Nary thinks it may be personal" : ""}
              {c.last_inbound_at ? ` · last message ${when(c.last_inbound_at)}` : ""}
            </p>
          </div>
          {canDecide && (
            <div className="flex shrink-0 gap-1.5">
              <Button size="sm" variant="primary" onClick={() => onDecide(c.chat_id, true)}>Business</Button>
              <Button size="sm" onClick={() => onDecide(c.chat_id, false)}>Personal</Button>
            </div>
          )}
        </li>
      ))}
    </ul>
  );
}

export function FilteredList({ rows, canCorrect, onCorrect }: {
  rows: Verdict[]; canCorrect: boolean; onCorrect: (id: string, verdict: string) => void;
}) {
  if (rows.length === 0) return <EmptyState title="Nothing filtered." compact />;
  return (
    <ul className={LIST}>
      {rows.map((v) => (
        <li key={v.id} className={ROW}>
          <div className="flex flex-wrap items-start gap-3">
            <div className="min-w-0 flex-1">
              <p className="text-xs text-fg-muted">
                {v.subject_type === "lead" ? "Web form" : v.subject_type === "email_thread" ? "Email" : "WhatsApp"} · {when(v.created_at)}
                {v.cascaded && " · checked by a second model"}
              </p>
              <p className="text-sm font-medium text-fg">
                {VERDICT_LABEL[v.verdict] ?? v.verdict}
                {v.corrected_to && <span className="ml-2 text-tone-blue">→ you said {VERDICT_LABEL[v.corrected_to] ?? v.corrected_to}</span>}
              </p>
              {v.reason && <p className="text-xs text-fg-muted">{v.reason}</p>}
              {v.excerpt && <p className="mt-1 line-clamp-3 whitespace-pre-wrap text-sm text-fg">{v.excerpt}</p>}
            </div>
            {canCorrect && (
              <Select value="" aria-label="Correct the verdict" className="w-40 shrink-0"
                onChange={(e) => e.target.value && onCorrect(v.id, e.target.value)}>
                <option value="">Correct...</option>
                {Object.entries(VERDICT_LABEL).filter(([k]) => k !== v.verdict).map(([k, label]) => (
                  <option key={k} value={k}>{label}</option>
                ))}
              </Select>
            )}
          </div>
        </li>
      ))}
    </ul>
  );
}

export function IdentityLinks({ rows, canUndo, onUndo }: { rows: Link[]; canUndo: boolean; onUndo: (id: string) => void }) {
  if (rows.length === 0) return <EmptyState title="No chats linked yet." compact />;
  return (
    <ul className={LIST}>
      {rows.map((l) => (
        <li key={l.id} className={`${ROW} flex flex-wrap items-start gap-3`}>
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium text-fg">{l.chat_id} → {l.target || "?"}</p>
            <p className="text-xs text-fg-muted">
              {TIER[l.tier] ?? l.tier} · score <span className="tabular">{l.score}</span>
              {l.status === "pending" && " · waiting for you in Approvals"}
              {l.status === "undone" && " · undone"}
            </p>
            <p className="text-xs text-fg-subtle">{l.signals.map((s) => `${s.signal.replace("_", " ")}: ${s.detail}`).join("; ")}</p>
            {l.review && (
              <p className="mt-1 inline-flex items-center gap-1 text-xs text-tone-orange">
                <ShieldAlert className="size-3.5" aria-hidden /> {l.review}
              </p>
            )}
          </div>
          {canUndo && l.status === "linked" && (
            <Button size="sm" onClick={() => onUndo(l.id)} title="Undo this link">
              <Undo2 aria-hidden /> Undo
            </Button>
          )}
        </li>
      ))}
    </ul>
  );
}
