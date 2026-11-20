"use client";

import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, Plus, Trash2, XCircle } from "lucide-react";
import { Button, Card, CardBody, CardHeader, Field, IconButton, Input, Select, Stat, StatGroup } from "@/components/ui";
import { systemApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { cn } from "@/lib/utils";
import { useDraftSource } from "./SettingsDraft";

/** The AI budget (ai.budget.*) and prices (ai.pricing), all editable, plus this month's usage. */
export interface BudgetSettings {
  monthly_usd: string;
  warn_pct: string;
  stop_pct: string;
  hard_stop_pct: string;
  essential_jobs: string[];
  job_caps: Record<string, number>;
}
type Price = { input: number; output: number; cache_read: number; cache_write: number; cache_write_1h?: number };
type Row = { calls: number; cost: number; input_tokens: number; output_tokens: number; cache_read_tokens: number; cache_write_tokens: number };
interface Usage {
  since: string;
  total: Row;
  by_job: (Row & { job: string })[];
  by_model: (Row & { model: string })[];
  by_day: { day: string; cost: number; calls: number }[];
  unpriced_calls: number;
  cache_hit_pct: number;
  budget: { cap: number; spent: number; pct: number; level: string; warn_pct: number; stop_pct: number; hard_stop_pct: number; error?: string };
}

const cell = "h-9 text-xs md:h-8";
const usd = (n: number) => `$${n.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
const tokens = (n: number) => n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(0)}K` : String(n);

// Budget state reads as text first; colour only reinforces it.
const LEVEL: Record<string, { label: string; tone: string; bar: string }> = {
  ok: { label: "Within budget", tone: "text-tone-blue", bar: "bg-tone-blue" },
  warn: { label: "Nearly spent", tone: "text-tone-orange", bar: "bg-tone-orange" },
  stopped: { label: "Spent: essential jobs only", tone: "text-tone-orange", bar: "bg-tone-orange" },
  hard_stopped: { label: "Hard stop: all AI work stopped", tone: "text-tone-red", bar: "bg-tone-red" },
};

const sortedPrices = (pricing: Record<string, Price>) =>
  Object.entries(pricing || {}).sort(([a], [z]) => a.localeCompare(z));

export default function AiBudgetBlock({ budget, pricing, batchDiscount, jobs, onSaved }: {
  budget: BudgetSettings; pricing: Record<string, Price>; batchDiscount: string; jobs: string[]; onSaved: () => void;
}) {
  const [b, setB] = useState<BudgetSettings>(budget);
  const [prices, setPrices] = useState<[string, Price][]>(() => sortedPrices(pricing));
  const [discount, setDiscount] = useState(batchDiscount);
  const [capJob, setCapJob] = useState("");
  const [newPrefix, setNewPrefix] = useState("");
  const [usage, setUsage] = useState<Usage | null>(null);
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => { setB(budget); }, [budget]);
  useEffect(() => { setPrices(sortedPrices(pricing)); }, [pricing]);
  useEffect(() => { setDiscount(batchDiscount); }, [batchDiscount]);

  const loadUsage = useCallback(async () => {
    try { setUsage((await systemApi.aiUsage()).data); } catch { setUsage(null); }
  }, []);
  useEffect(() => { loadUsage(); }, [loadUsage]);

  const body = () => ({
    budget: b,
    pricing: Object.fromEntries(prices),
    ...(discount !== "" ? { batch_discount_pct: discount } : {}),
  });

  const changes = (Object.keys(budget || {}) as (keyof BudgetSettings)[])
    .filter((k) => JSON.stringify(b[k]) !== JSON.stringify(budget[k])).length
    + (JSON.stringify(Object.fromEntries(prices)) !== JSON.stringify(Object.fromEntries(sortedPrices(pricing))) ? 1 : 0)
    + (discount !== batchDiscount ? 1 : 0);

  const { inSaveBar } = useDraftSource("ai-budget", {
    count: changes,
    save: async () => { await systemApi.updateAiSettings(body()); onSaved(); loadUsage(); },
    discard: () => { setB(budget); setPrices(sortedPrices(pricing)); setDiscount(batchDiscount); },
  });

  const save = async () => {
    setSaving(true); setMsg(null);
    try {
      await systemApi.updateAiSettings(body());
      setMsg({ ok: true, text: "Budget and prices saved" });
      onSaved();
      loadUsage();
    } catch (e) {
      setMsg({ ok: false, text: extractApiError(e) });
    } finally { setSaving(false); }
  };

  const toggleEssential = (job: string) =>
    setB((x) => ({ ...x, essential_jobs: x.essential_jobs.includes(job) ? x.essential_jobs.filter((j) => j !== job) : [...x.essential_jobs, job] }));
  const setPrice = (prefix: string, k: keyof Price, v: string) =>
    setPrices((ps) => ps.map(([p, row]) => (p === prefix
      ? [p, { ...row, [k]: k === "cache_write_1h" && v === "" ? undefined : Number(v) || 0 }] : [p, row])));

  const level = usage?.budget && !usage.budget.error ? LEVEL[usage.budget.level] ?? LEVEL.ok : null;

  return (
    <Card>
      <CardHeader title="Budget and usage"
        description="Checked before every AI call. Past the stop threshold only essential jobs run; past the hard stop nothing does. A monthly cap of 0 means no cap. Months follow company time." />
      <CardBody className="space-y-5">
        {usage && (
          <div className="space-y-3">
            <StatGroup>
              <Stat label="Spent this month" value={usage.budget.error ? "-" : `${usd(usage.budget.spent)} of ${usd(usage.budget.cap)}`} />
              <Stat label="Calls" value={usage.total.calls.toLocaleString()} />
              <Stat label="Prompt cache hits" value={`${usage.cache_hit_pct}%`} />
              <Stat label="Unpriced calls" value={String(usage.unpriced_calls)} alert={usage.unpriced_calls > 0} />
            </StatGroup>
            {level && usage.budget.cap > 0 && (
              <div>
                <div className="mb-1 flex items-center justify-between text-xs">
                  <span className={cn("inline-flex items-center gap-1 font-medium", level.tone)}>
                    {usage.budget.level === "ok" ? <CheckCircle2 className="size-3.5" aria-hidden /> : <AlertTriangle className="size-3.5" aria-hidden />} {level.label}
                  </span>
                  <span className="text-fg-muted tabular">{usage.budget.pct.toFixed(0)}%</span>
                </div>
                <div className="h-2 overflow-hidden rounded-full bg-surface-2" role="meter"
                  aria-valuemin={0} aria-valuemax={usage.budget.hard_stop_pct} aria-valuenow={usage.budget.pct}
                  aria-label="Share of the monthly AI budget spent">
                  <div className={cn("h-full rounded-full", level.bar)}
                    style={{ width: `${Math.min(100, (usage.budget.pct / usage.budget.hard_stop_pct) * 100)}%` }} />
                </div>
                <p className="mt-1 text-[11px] text-fg-muted">
                  Warn {usage.budget.warn_pct}% · stop {usage.budget.stop_pct}% · hard stop {usage.budget.hard_stop_pct}% (bar ends at the hard stop)
                </p>
              </div>
            )}
            {usage.unpriced_calls > 0 && (
              <p className="text-xs text-tone-orange">Some calls used a model with no row in the price table below; they count as $0 until you add it.</p>
            )}
            <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
              {([["Job", usage.by_job.map((r) => [r.job, r] as const)], ["Model", usage.by_model.map((r) => [r.model, r] as const)]] as const).map(([title, list]) => (
                <table key={title} className="w-full text-xs">
                  <thead><tr className="text-left text-xs text-fg-muted">
                    <th className="py-1 font-medium">{title}</th><th className="text-right font-medium">Calls</th>
                    <th className="text-right font-medium">In / out</th><th className="text-right font-medium">Cost</th>
                  </tr></thead>
                  <tbody className="divide-y divide-line-soft">
                    {list.length === 0 && <tr><td colSpan={4} className="py-2 text-fg-muted">No calls this month.</td></tr>}
                    {list.map(([name, r]) => (
                      <tr key={name}><td className="py-1.5 font-mono text-fg">{name}</td>
                        <td className="text-right text-fg-muted tabular">{r.calls}</td>
                        <td className="text-right text-fg-muted tabular">{tokens(r.input_tokens + r.cache_read_tokens)} / {tokens(r.output_tokens)}</td>
                        <td className="text-right font-medium text-fg tabular">{usd(r.cost)}</td></tr>
                    ))}
                  </tbody>
                </table>
              ))}
            </div>
          </div>
        )}

        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          {([["monthly_usd", "Monthly cap (USD)"], ["warn_pct", "Warn at (%)"], ["stop_pct", "Stop non-essential at (%)"], ["hard_stop_pct", "Hard stop at (%)"]] as const).map(([k, label]) => (
            <Field key={k} label={label} id={`ai-budget-${k}`}>
              <Input type="number" min={0} step="any" numeric value={b[k]} onChange={(e) => setB({ ...b, [k]: e.target.value })} />
            </Field>
          ))}
        </div>

        <div>
          <p className="mb-1.5 text-[13px] font-medium text-fg">Essential jobs (keep running past the stop threshold)</p>
          <div className="flex flex-wrap gap-1.5">
            {jobs.filter((j) => j !== "embedding").map((j) => {
              const on = b.essential_jobs.includes(j);
              return (
                <button type="button" key={j} onClick={() => toggleEssential(j)} aria-pressed={on}
                  className={cn("min-h-11 rounded-full border px-2.5 font-mono text-[11px] transition-colors md:min-h-0 md:py-1",
                    on ? "border-brand bg-brand/10 text-brand-ink" : "border-line text-fg-muted hover:bg-surface-2")}>
                  {j}
                </button>
              );
            })}
          </div>
        </div>

        <div>
          <p className="mb-1.5 text-[13px] font-medium text-fg">Per-job monthly caps (USD, optional)</p>
          <div className="space-y-1.5">
            {Object.entries(b.job_caps).map(([job, cap]) => (
              <div key={job} className="flex items-center gap-2">
                <span className="w-48 truncate font-mono text-xs text-fg">{job}</span>
                <Input aria-label={`Cap for ${job}`} type="number" min={0} step="any" numeric value={cap} className={cn(cell, "max-w-[120px]")}
                  onChange={(e) => setB({ ...b, job_caps: { ...b.job_caps, [job]: Number(e.target.value) || 0 } })} />
                <IconButton size="sm" label={`Remove cap for ${job}`} icon={Trash2}
                  onClick={() => { const c = { ...b.job_caps }; delete c[job]; setB({ ...b, job_caps: c }); }} />
              </div>
            ))}
            <div className="flex items-center gap-2">
              <div className="w-full max-w-[220px]">
                <Select aria-label="Job to cap" value={capJob} onChange={(e) => setCapJob(e.target.value)} placeholder="Add a cap for..."
                  options={jobs.filter((j) => !(j in b.job_caps)).map((j) => ({ value: j, label: j }))} />
              </div>
              <Button size="sm" disabled={!capJob} onClick={() => { setB({ ...b, job_caps: { ...b.job_caps, [capJob]: 10 } }); setCapJob(""); }}>
                <Plus aria-hidden /> Add
              </Button>
            </div>
          </div>
        </div>

        <div>
          <p className="mb-1.5 text-[13px] font-medium text-fg">Prices (USD per 1M tokens, matched by the longest model-name prefix)</p>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead><tr className="text-left text-xs text-fg-muted">
                <th className="py-1 font-medium">Model prefix</th><th className="font-medium">Input</th><th className="font-medium">Output</th>
                <th className="font-medium">Cache read</th><th className="font-medium">Cache write</th>
                <th className="font-medium" title="1-hour cache writes (Claude: 2x input); empty = the cache write price">Write 1h</th><th></th>
              </tr></thead>
              <tbody>
                {prices.map(([prefix, row]) => (
                  <tr key={prefix}>
                    <td className="py-1 pr-2 font-mono text-fg">{prefix}</td>
                    {(["input", "output", "cache_read", "cache_write", "cache_write_1h"] as const).map((k) => (
                      <td key={k} className="w-24 py-1 pr-2">
                        <Input aria-label={`${prefix} ${k.replace(/_/g, " ")}`} type="number" min={0} step="any" numeric className={cell}
                          value={row[k] ?? ""} onChange={(e) => setPrice(prefix, k, e.target.value)} />
                      </td>
                    ))}
                    <td><IconButton size="sm" label={`Remove ${prefix}`} icon={Trash2}
                      onClick={() => setPrices((ps) => ps.filter(([p]) => p !== prefix))} /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="mt-2 flex flex-wrap items-center gap-2">
            <Input aria-label="New model prefix" value={newPrefix} onChange={(e) => setNewPrefix(e.target.value)} placeholder="model prefix" className="max-w-[200px]" />
            <Button size="sm" disabled={!newPrefix.trim() || prices.some(([p]) => p === newPrefix.trim().toLowerCase())}
              onClick={() => { setPrices((ps) => [...ps, [newPrefix.trim().toLowerCase(), { input: 0, output: 0, cache_read: 0, cache_write: 0 }]]); setNewPrefix(""); }}>
              <Plus aria-hidden /> Add price
            </Button>
            <label htmlFor="ai-batch-discount" className="ml-4 text-[13px] text-fg">Batch API discount (%)</label>
            <Input id="ai-batch-discount" type="number" min={0} max={100} numeric value={discount} onChange={(e) => setDiscount(e.target.value)} className="max-w-[80px]" />
          </div>
        </div>

        {(!inSaveBar || msg) && (
          <div className="flex items-center gap-3">
            {!inSaveBar && <Button variant="primary" onClick={save} loading={saving}>Save budget and prices</Button>}
            {msg && (
              <span className={cn("inline-flex items-center gap-1 text-xs", msg.ok ? "text-tone-blue" : "text-tone-red")}>
                {msg.ok ? <CheckCircle2 className="size-3.5" aria-hidden /> : <XCircle className="size-3.5" aria-hidden />} {msg.text}
              </span>
            )}
          </div>
        )}
      </CardBody>
    </Card>
  );
}
