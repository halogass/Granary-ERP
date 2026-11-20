"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Bot, CalendarClock, Loader2, Lock, Mic, Upload } from "lucide-react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { safeHref } from "@/lib/sanitize";
import { usePermissions } from "@/hooks/usePermissions";
import { Button, Input, Pill, StatusPill, type Tone } from "@/components/ui";
import { stack } from "@/lib/stackTable";
import { cn } from "@/lib/utils";

/** Nary's internal notes on a deal or lead (Phase 3): the BANT score, the requirement fit check,
 *  negotiation hints, call extracts, the handoff package, the loss analysis, the follow-up cadence
 *  and meetings. Internal only: none of this ever reaches a customer. */

export interface Score {
  id: string; at: string; budget: number; authority: number; need: number; timeline: number; total: number;
  class: string; red_flags: string[]; green_flags: string[]; rationale: string | null; intent: string | null; cascaded: boolean;
}
export interface Insight { id: string; kind: string; status: string; content: Record<string, unknown>; at: string }
export interface CadenceStep { id: string; day: number; channel: string; topic: string | null; intent: string | null; due_at: string; status: string; reason: string | null }
export interface Meeting {
  id: string; status: string; title: string | null; starts_at: string | null; meet_link: string | null;
  slots: { start: string; company?: string; customer?: string; extended?: boolean }[]; picked: number | null;
}
export interface Panel { scores: Score[]; insights: Insight[]; cadence: CadenceStep[]; meetings: Meeting[] }

const KIND_LABEL: Record<string, string> = {
  fit_check: "Requirement fit check", negotiation: "Negotiation hints (for you only)", call: "From the call",
  handoff: "Handoff package", loss: "Why it was lost", partner_conflict: "Registration conflict evidence",
};
const STATUS_TONE: Record<string, Tone> = {
  covered: "blue", optional_custom: "orange", not_in_scope: "neutral", unknown: "violet",
};
const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "");

function List({ items }: { items: unknown }) {
  const arr = Array.isArray(items) ? items : [];
  if (!arr.length) return <span className="text-fg-subtle">none</span>;
  return (
    <ul className="list-disc pl-4 space-y-0.5">
      {arr.map((x, i) => (
        <li key={i}>{typeof x === "string" ? x : (x as { quote?: string; what?: string; requirement?: string }).quote
          ?? (x as { what?: string }).what ?? JSON.stringify(x)}</li>
      ))}
    </ul>
  );
}

export function InsightView({ insight }: { insight: Insight }) {
  const c = insight.content as Record<string, unknown>;
  if (insight.kind === "fit_check") {
    const reqs = (c.requirements as { requirement: string; status: string; how: string }[]) ?? [];
    return (
      <div className="space-y-1.5 text-xs text-fg">
        <p>{String(c.summary ?? "")}</p>
        <table className={cn("w-full text-left", stack.table)}>
          <thead className={stack.thead}><tr className="text-fg-muted"><th className="font-medium">Requirement</th><th className="font-medium">Status</th><th className="font-medium">How</th></tr></thead>
          <tbody className={cn(stack.tbody, "max-md:p-0")}>
            {reqs.map((r, i) => (
              <tr key={i} className={cn("border-t border-line-soft", stack.tr)}>
                <td className={cn("py-1 pr-2", stack.title)}>{r.requirement}</td>
                <td data-label="Status" className={cn("py-1 pr-2", stack.td)}><Pill tone={STATUS_TONE[r.status] ?? "neutral"}>{r.status.replace("_", " ")}</Pill></td>
                <td data-label="How" className={cn("py-1", stack.td)}>{r.how}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {c.proposal_trigger ? <p className="font-medium text-tone-orange">A Solution Proposal may be needed ({String(c.proposal_trigger)}).</p> : null}
      </div>
    );
  }
  if (insight.kind === "handoff") {
    const sections: [string, string][] = [["scope_sold", "Scope sold"], ["scope_not_sold", "Not sold"],
      ["success_criteria", "Success criteria (their words)"], ["commitments", "Commitments"], ["sensitivities", "Sensitivities"],
      ["red_flags", "Red flags"], ["contacts", "Contacts"], ["open_items", "Open items"]];
    return (
      <div className="space-y-2 text-xs text-fg">
        <p>{String(c.summary ?? "")}</p>
        <div className="grid gap-3 sm:grid-cols-2">
          {sections.map(([k, label]) => (
            <div key={k}><p className="mb-0.5 font-medium text-fg-muted">{label}</p><List items={c[k]} /></div>
          ))}
        </div>
      </div>
    );
  }
  if (insight.kind === "negotiation") {
    return (
      <div className="space-y-1 text-xs text-fg">
        <p className="font-semibold">{String(c.objection ?? "")}</p>
        <List items={c.hints} />
        <p><span className="font-medium text-fg-muted">Room: </span>{String(c.room ?? "")}</p>
        {Array.isArray(c.watch_out) && c.watch_out.length > 0 && <div className="text-tone-orange"><List items={c.watch_out} /></div>}
      </div>
    );
  }
  if (insight.kind === "call") {
    return (
      <div className="space-y-1 text-xs text-fg">
        <p>{String(c.summary ?? "")}</p>
        <p className="font-medium text-fg-muted">Commitments</p><List items={c.commitments} />
        <p className="font-medium text-fg-muted">Next steps</p><List items={c.next_steps} />
      </div>
    );
  }
  if (insight.kind === "loss") {
    return (
      <div className="space-y-1 text-xs text-fg">
        <p>{String(c.analysis ?? "")}</p>
        <p className="font-semibold">{String(c.lesson ?? "")}</p>
      </div>
    );
  }
  return <pre className="whitespace-pre-wrap font-mono text-[11px] text-fg-muted">{JSON.stringify(c, null, 1)}</pre>;
}

export function ScoreView({ score }: { score: Score }) {
  const parts: [string, number][] = [["Budget", score.budget], ["Authority", score.authority], ["Need", score.need], ["Timeline", score.timeline]];
  return (
    <div className="space-y-1.5">
      <div className="flex items-center gap-2">
        <StatusPill family="temperature" status={score.class} label={score.class.toUpperCase()} />
        <span className="text-sm font-semibold text-fg tabular">BANT {score.total}/12</span>
        {score.cascaded && <span className="text-xs text-fg-muted">checked by a second model</span>}
      </div>
      <div className="grid grid-cols-4 gap-2">
        {parts.map(([label, v]) => (
          <div key={label}>
            <p className="text-[11px] text-fg-muted">{label}</p>
            <div className="h-1.5 rounded bg-surface-2" aria-label={`${label} ${v} of 3`}>
              <div className="h-1.5 rounded bg-tone-violet" style={{ width: `${(v / 3) * 100}%` }} />
            </div>
          </div>
        ))}
      </div>
      {score.rationale && <p className="text-xs text-fg-muted">{score.rationale}</p>}
      {score.red_flags.length > 0 && <p className="text-xs font-medium text-tone-red">Red flags: {score.red_flags.join("; ")}</p>}
    </div>
  );
}

export function PanelView({ panel, canApprove, onProposeMeeting, onDebrief, busy }: {
  panel: Panel; canApprove: boolean; busy?: boolean;
  onProposeMeeting?: () => void; onDebrief?: (data: FormData) => void;
}) {
  const fileRef = useRef<HTMLInputElement>(null);
  const [note, setNote] = useState("");
  const latest = panel.scores[0];
  const empty = !latest && !panel.insights.length && !panel.cadence.length && !panel.meetings.length;
  return (
    <section className="space-y-4 rounded-lg border border-line bg-surface p-4 shadow-card">
      <h2 className="inline-flex items-center gap-1.5 text-sm font-semibold text-fg">
        <Bot className="size-4 text-tone-violet" aria-hidden /> Nary&apos;s notes <Lock className="size-3 text-fg-muted" aria-label="internal only" />
      </h2>
      {empty && <p className="text-[13px] text-fg-muted">Nary hasn&apos;t worked on this one yet.</p>}
      {latest && <ScoreView score={latest} />}
      {panel.insights.map((i) => (
        <div key={i.id} className="border-t border-line-soft pt-3">
          <p className="mb-1 text-xs font-semibold text-fg">{KIND_LABEL[i.kind] ?? i.kind} <span className="font-normal text-fg-subtle">{when(i.at)}</span></p>
          <InsightView insight={i} />
        </div>
      ))}
      {panel.cadence.length > 0 && (
        <div className="border-t border-line-soft pt-3">
          <p className="mb-1 text-xs font-semibold text-fg">Follow-up cadence</p>
          <ol className="space-y-0.5 text-xs">
            {panel.cadence.map((s) => (
              <li key={s.id} className="flex gap-2">
                <span className="w-12 text-fg-muted tabular">Day {s.day}</span>
                <span className="w-16 text-fg-muted">{s.channel}</span>
                <span className={`w-20 font-medium ${s.status === "drafted" || s.status === "done" ? "text-tone-blue" : s.status === "skipped" || s.status === "cancelled" ? "text-fg-subtle" : "text-fg"}`}>{s.status}</span>
                <span className="flex-1 truncate text-fg-muted" title={s.reason || s.intent || ""}>{s.reason || s.intent}</span>
              </li>
            ))}
          </ol>
        </div>
      )}
      {panel.meetings.length > 0 && (
        <div className="space-y-1 border-t border-line-soft pt-3">
          <p className="text-xs font-semibold text-fg">Meetings</p>
          {panel.meetings.map((m) => (
            <p key={m.id} className="text-xs text-fg-muted">
              <CalendarClock className="mr-1 inline size-3.5" aria-hidden />
              {m.status === "booked" ? `${when(m.starts_at)} ` : `${m.status.replace("_", " ")} `}
              {safeHref(m.meet_link) && <a href={safeHref(m.meet_link)} target="_blank" rel="noreferrer" className="font-medium text-brand-ink hover:underline">Meet</a>}
            </p>
          ))}
        </div>
      )}
      {canApprove && (onProposeMeeting || onDebrief) && (
        <div className="flex flex-wrap items-center gap-2 border-t border-line-soft pt-3">
          {onProposeMeeting && (
            <Button size="sm" disabled={busy} onClick={onProposeMeeting}>
              Propose meeting times
            </Button>
          )}
          {onDebrief && (
            <>
              <Input value={note} onChange={(e) => setNote(e.target.value)} placeholder="How did the call go? (or upload)"
                aria-label="Debrief note" autoComplete="off" className="min-w-[12rem] flex-1" />
              <Button size="sm" variant="primary" disabled={busy || !note.trim()}
                onClick={() => { const f = new FormData(); f.append("text", note.trim()); onDebrief(f); setNote(""); }}>Send debrief</Button>
              <input ref={fileRef} type="file" className="hidden" aria-label="Debrief file"
                accept="audio/*,.ogg,.opus,.mp3,.m4a,.wav,.txt,.srt,.vtt,.docx"
                onChange={(e) => { const file = e.target.files?.[0]; if (file) { const f = new FormData(); f.append("file", file); onDebrief(f); } e.target.value = ""; }} />
              <Button size="sm" disabled={busy} onClick={() => fileRef.current?.click()}>
                <Mic aria-hidden /><Upload aria-hidden /> Voice note or transcript
              </Button>
            </>
          )}
        </div>
      )}
    </section>
  );
}

/** Loads and shows the panel on a deal or lead page. Hidden without nary:view. */
export default function DealNaryInsights({ dealId, leadId }: { dealId?: string; leadId?: string }) {
  const { can, loaded } = usePermissions();
  const [panel, setPanel] = useState<Panel | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const load = useCallback(async () => {
    try {
      setPanel((dealId ? await naryApi.dealPanel(dealId) : await naryApi.leadPanel(leadId as string)).data);
    } catch { setPanel(null); }
  }, [dealId, leadId]);
  useEffect(() => { if (loaded && can("nary:view")) load(); }, [load, loaded, can]);
  if (!panel) return null;
  const act = async (fn: () => Promise<unknown>, done: string) => {
    setBusy(true); setNotice(null);
    try { await fn(); setNotice(done); await load(); } catch (e) { setNotice(extractApiError(e)); } finally { setBusy(false); }
  };
  return (
    <div className="space-y-1">
      <PanelView panel={panel} canApprove={can("nary:approve")} busy={busy}
        onProposeMeeting={dealId ? () => act(() => naryApi.dealMeeting(dealId), "Nary proposed meeting times: approve them in Nary approvals.") : undefined}
        onDebrief={dealId ? (f) => act(() => naryApi.dealDebrief(dealId, f), "Nary read the debrief: reminders set, summary email drafted.") : undefined} />
      {busy && <Loader2 className="size-4 animate-spin text-fg-subtle" aria-label="Working" />}
      {notice && <p role="status" className="text-xs text-fg-muted">{notice}</p>}
    </div>
  );
}
