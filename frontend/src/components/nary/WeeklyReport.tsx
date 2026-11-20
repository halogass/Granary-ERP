"use client";

import { useState } from "react";
import { CheckCircle2, FileSignature, TriangleAlert } from "lucide-react";
import { Button, Card, CardBody, CardHeader, Checkbox } from "@/components/ui";

/** Nary report (Phase 3): the weekly report as it stands now, and SOP revision drafts. */

export interface Kpi { kpi: string; value: number | null; target: number; ok: boolean | null }
export interface ReportData {
  week_ending: string; kpis: Kpi[]; pipeline: { stage: string; currency: string; deals: number; value: string }[];
  at_risk: { title: string; stage: string }[]; approval_latency_hours: number | null;
  quality: Record<string, { decided: number; unedited: number; rejected: number; unedited_pct: number | null }>;
  promotion_candidates: { action: string }[]; filtered: number; budget: { spent: string; cap: string } | null;
}
export interface SopBrief { code: string; brief: string }
export interface SopDraft { id: string; title: string; status: string; version: string; sop_code: string | null; previous_version_id: string | null; updated_at: string }

const KPI_LABEL: Record<string, string> = {
  response_sla_pct: "First replies within the SLA (%)", lead_to_sql_pct: "Lead → SQL (%)", sql_to_deal_pct: "SQL → deal (%)",
  quote_to_win_pct: "Quotation → win (%)", win_rate_pct: "Win rate (%)", quote_to_decision_days: "Quotation → decision (days)",
  loss_reason_pct: "Losses with a reason (%)",
};

export function ReportView({ data }: { data: ReportData }) {
  return (
    <div className="space-y-4">
      <Card>
        <CardHeader title="SOP KPIs" description={`Week ending ${data.week_ending}`} />
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-line bg-surface-2 text-left text-xs text-fg-muted">
              <th className="px-4 py-2 font-medium">KPI</th><th className="px-4 py-2 text-right font-medium">Now</th>
              <th className="px-4 py-2 text-right font-medium">Target</th><th className="w-10 px-4 py-2"><span className="sr-only">On target</span></th>
            </tr>
          </thead>
          <tbody>
            {data.kpis.map((k) => (
              <tr key={k.kpi} className="border-b border-line-soft last:border-0 even:bg-surface-2/60">
                <td className="px-4 py-2 text-fg">{KPI_LABEL[k.kpi] ?? k.kpi}</td>
                <td className="px-4 py-2 text-right font-medium text-fg tabular">{k.value ?? "n/a"}</td>
                <td className="px-4 py-2 text-right text-fg-muted tabular">{k.target}</td>
                <td className="px-4 py-2">{k.ok === null ? null : k.ok
                  ? <CheckCircle2 className="size-4 text-tone-blue" aria-label="on target" />
                  : <TriangleAlert className="size-4 text-tone-red" aria-label="below target" />}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader title="Pipeline" />
          <CardBody>
            {data.pipeline.length === 0 ? <p className="text-[13px] text-fg-muted">No open deals.</p> : (
              <ul className="space-y-1 text-sm">
                {data.pipeline.map((p) => (
                  <li key={`${p.stage}-${p.currency}`} className="flex justify-between gap-3">
                    <span className="capitalize text-fg">{p.stage}</span>
                    <span className="text-fg-muted tabular">{p.deals} · {p.currency} {Number(p.value).toLocaleString()}</span>
                  </li>
                ))}
              </ul>
            )}
            {data.at_risk.length > 0 && <p className="mt-2 text-xs font-medium text-tone-red">At risk: {data.at_risk.map((d) => d.title).join(", ")}</p>}
          </CardBody>
        </Card>
        <Card>
          <CardHeader title="Nary's quality this week" />
          <CardBody>
            {Object.keys(data.quality).length === 0 ? <p className="text-[13px] text-fg-muted">No decided cards this week.</p> : (
              <ul className="space-y-1 text-sm">
                {Object.entries(data.quality).sort().map(([a, q]) => (
                  <li key={a} className="flex justify-between gap-3">
                    <span className="font-mono text-xs text-fg">{a}</span>
                    <span className="text-fg-muted tabular">{q.unedited_pct ?? 0}% unedited of {q.decided}</span>
                  </li>
                ))}
              </ul>
            )}
            {data.promotion_candidates.length > 0 && (
              <p className="mt-2 text-xs font-medium text-tone-violet">Ready to promote: {data.promotion_candidates.map((c) => c.action).join(", ")}</p>
            )}
            <p className="mt-2 text-xs text-fg-muted tabular">
              Cards waited {data.approval_latency_hours ?? "n/a"} h on average · {data.filtered} filtered conversation(s)
              {data.budget ? ` · AI $${data.budget.spent} of $${data.budget.cap}` : ""}
            </p>
          </CardBody>
        </Card>
      </div>
    </div>
  );
}

export function SopRevisions({ briefs, drafts, canManage, onDraft }: {
  briefs: SopBrief[]; drafts: SopDraft[]; canManage: boolean; onDraft: (codes: string[]) => void;
}) {
  const [picked, setPicked] = useState<string[]>([]);
  const toggle = (code: string) => setPicked(picked.includes(code) ? picked.filter((c) => c !== code) : [...picked, code]);
  return (
    <Card>
      <CardHeader title={<span className="inline-flex items-center gap-1.5"><FileSignature className="size-4 text-fg-muted" aria-hidden /> SOP revisions</span>}
        description="Nary drafts each revision for the way you work with it now. Drafts go through the usual signing and approval in the knowledge base; the live SOP stays until you sign." />
      <CardBody className="space-y-3">
        <ul className="space-y-1.5">
          {briefs.map((b) => (
            <li key={b.code} className="flex gap-2 text-sm">
              {canManage && <span className="mt-0.5"><Checkbox checked={picked.includes(b.code)} onChange={() => toggle(b.code)} aria-label={`Draft ${b.code}`} /></span>}
              <span><span className="font-mono text-[13px] font-medium text-fg">{b.code}</span> <span className="text-fg-muted">{b.brief}</span></span>
            </li>
          ))}
        </ul>
        {canManage && (
          <Button variant="primary" disabled={!picked.length} onClick={() => { onDraft(picked); setPicked([]); }}>
            Draft {picked.length || ""} with Nary
          </Button>
        )}
        {drafts.length > 0 && (
          <div>
            <p className="mb-1 text-xs font-semibold text-fg-muted">Waiting in the knowledge base</p>
            <ul className="space-y-0.5 text-sm">
              {drafts.map((d) => (
                <li key={d.id}>
                  <a href={`/knowledge/articles/${d.id}`} className="font-medium text-brand-ink hover:underline">{d.title}</a>{" "}
                  <span className="text-xs text-fg-muted">v{d.version} · {d.status}</span>
                </li>
              ))}
            </ul>
          </div>
        )}
      </CardBody>
    </Card>
  );
}
