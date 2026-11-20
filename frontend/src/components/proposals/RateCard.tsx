"use client";

import { useState } from "react";
import { Button, Field, Input, Select } from "@/components/ui";
import { stack } from "@/lib/stackTable";
import { cn } from "@/lib/utils";

/** Settings, Proposals, Rate card: cost per man-day (in the functional currency) by role and seniority,
 *  at the lowest market rate, with its source. A new rate replaces the active one from its date. */
export interface Rate { id: string; role: string; seniority: string; cost_base: string; source: string | null; valid_from: string; status: string; proposed_by_nary: boolean }

export default function RateCard({ rates, canManage, onAdd }: {
  rates: Rate[]; canManage: boolean; onAdd: (r: { role: string; seniority: string; cost_base: string; source: string; valid_from: string }) => void;
}) {
  const [form, setForm] = useState({ role: "", seniority: "mid", cost_base: "", source: "", valid_from: new Date().toISOString().slice(0, 10) });
  const active = rates.filter((r) => r.status === "active");
  return (
    <div className="space-y-3">
      <div className="overflow-x-auto rounded-lg border border-line max-md:overflow-visible max-md:rounded-none max-md:border-0">
        <table className={cn("w-full text-sm", stack.table)}>
          <thead className={cn("bg-surface-2", stack.thead)}><tr className="text-left text-xs text-fg-muted">
            <th className="px-3 py-2 font-medium">Role</th><th className="px-3 py-2 font-medium">Seniority</th>
            <th className="px-3 py-2 text-right font-medium">Cost / man-day</th><th className="px-3 py-2 font-medium">From</th><th className="px-3 py-2 font-medium">Source</th>
          </tr></thead>
          <tbody className={cn(stack.tbody, "max-md:p-0")}>
            {active.map((r) => (
              <tr key={r.id} className={cn("border-t border-line-soft even:bg-surface-2/50", stack.tr)}>
                <td className={cn("px-3 py-2", stack.title)}>{r.role}</td><td data-label="Seniority" className={cn("px-3 py-2", stack.td)}>{r.seniority}</td>
                <td data-label="Cost / man-day" className={cn("px-3 py-2 text-right tabular", stack.td)}>{Number(r.cost_base).toLocaleString("en-US")}</td>
                <td data-label="From" className={cn("px-3 py-2 tabular text-fg-muted", stack.td)}>{r.valid_from}</td>
                <td data-label="Source" className={cn("max-w-xs truncate px-3 py-2 text-xs text-fg-muted", stack.td, "max-md:max-w-none max-md:whitespace-normal")} title={r.source ?? ""}>{r.proposed_by_nary ? "Nary: " : ""}{r.source}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {canManage && (
        <div className="flex flex-wrap items-end gap-2">
          <Field label="Role" className="max-md:w-full"><Input value={form.role} autoComplete="off" onChange={(e) => setForm({ ...form, role: e.target.value })} /></Field>
          <Field label="Seniority" className="max-md:w-full">
            <Select value={form.seniority} onChange={(e) => setForm({ ...form, seniority: e.target.value })}
              options={[{ value: "junior", label: "junior" }, { value: "mid", label: "mid" }, { value: "senior", label: "senior" }]} />
          </Field>
          <Field label="Cost per man-day" className="max-md:w-full"><Input value={form.cost_base} inputMode="numeric" numeric autoComplete="off" onChange={(e) => setForm({ ...form, cost_base: e.target.value })} className="md:w-36" /></Field>
          <Field label="From" className="max-md:w-full"><Input type="date" value={form.valid_from} onChange={(e) => setForm({ ...form, valid_from: e.target.value })} /></Field>
          <Field label="Source" className="min-w-[12rem] flex-1"><Input value={form.source} autoComplete="off" onChange={(e) => setForm({ ...form, source: e.target.value })} /></Field>
          <Button variant="primary" disabled={!form.role.trim() || !form.cost_base.trim() || !form.source.trim()} onClick={() => onAdd(form)}>Set rate</Button>
        </div>
      )}
    </div>
  );
}
