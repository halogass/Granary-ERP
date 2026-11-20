"use client";

import { useEffect, useMemo, useState } from "react";
import { CheckCircle2, Plus, Trash2, XCircle } from "lucide-react";
import { Button, Card, CardBody, CardHeader, IconButton, Input, Select } from "@/components/ui";
import { systemApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { cn } from "@/lib/utils";
import type { ProviderState } from "./ApiKeysBlock";
import { useDraftSource } from "./SettingsDraft";

/** One job's model (setting ai.routing). Every AI feature asks for its job; "default" covers the rest. */
export interface Route {
  provider: string;
  model: string;
  effort: string | null;
  max_tokens: number | null;
  /** Prompt-cache lifetime (Claude): "5m" or "1h"; empty = the default (ai.cache.default_ttl). */
  cache_ttl?: string | null;
}

const PROVIDERS = ["anthropic", "gemini", "openai", "deepseek", "ollama"];
const EFFORTS = ["", "low", "medium", "high", "xhigh", "max"];
// Suggestions only; any model id can be typed.
const MODEL_HINTS: Record<string, string[]> = {
  anthropic: ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
  gemini: ["gemini-2.5-flash", "gemini-2.5-pro", "gemini-2.0-flash", "gemini-embedding-001"],
};

const JOB_HELP: Record<string, string> = {
  default: "Any job not listed",
  assistant: "Nary chat",
  suggestion: "Reply suggestions",
  scoring: "Lead scoring",
  record_brief: "Record briefs",
  crm_ai_field: "AI fields",
  crm_enrichment: "Company enrichment",
  crm_research: "Deep research",
  doc_intel: "Document analysis",
  pdf_import: "PDF to Markdown",
  embedding: "Embeddings (KB search)",
  web_search: "Web search grounding",
  "nary.brief": "Nary: briefs",
  "nary.card": "Nary: approval cards",
  "nary.reply_draft": "Nary: reply drafts",
  "nary.memory": "Nary: memory",
  "nary.cascade_review": "Nary: second opinion",
  "nary.triage": "Nary: triage",
  "nary.classify": "Nary: classification",
  "nary.web3forms_extract": "Nary: web-form fields (when the format changes)",
  "nary.identity_match": "Nary: who a new chat says they are",
  "nary.history_classify": "Nary: WhatsApp history, business or not (Batch API)",
  "nary.history_read": "Nary: WhatsApp history, reading business chats (Batch API)",
  "nary.vision": "Nary: photos and PDFs customers send",
};

const cell = "h-9 text-xs md:h-8";
/** Selects keep room for their longest option ("anthropic", "medium", "1 hour") plus the
 *  chevron: the table squeezed them until the values were clipped. */
export const SELECT_MIN = { provider: "min-w-[8.5rem]", effort: "min-w-[7rem]", cache: "min-w-[7rem]" } as const;

function sorted(routing: Record<string, Route>): [string, Route][] {
  return Object.entries(routing || {}).sort(([a], [b]) => (a === "default" ? -1 : b === "default" ? 1 : a.localeCompare(b)));
}

/** Jobs added, removed or edited compared with the saved routing. */
function changedJobs(rows: [string, Route][], routing: Record<string, Route>) {
  const now = Object.fromEntries(rows);
  const keys = new Set([...Object.keys(now), ...Object.keys(routing || {})]);
  let n = 0;
  keys.forEach((k) => {
    const a = now[k], b = (routing || {})[k];
    if (!a || !b) { n += 1; return; }
    const norm = (r: Route) => JSON.stringify([r.provider, r.model, r.effort || null, r.max_tokens || null, r.cache_ttl || null]);
    if (norm(a) !== norm(b)) n += 1;
  });
  return n;
}

export default function AiRoutingBlock({ routing, providers, onSaved }: {
  routing: Record<string, Route>; providers: Record<string, ProviderState>; onSaved: () => void;
}) {
  const [rows, setRows] = useState<[string, Route][]>(() => sorted(routing));
  const [newJob, setNewJob] = useState("");
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [testing, setTesting] = useState<string | null>(null);
  const [tests, setTests] = useState<Record<string, { ok: boolean; text: string }>>({});

  useEffect(() => {
    setRows(sorted(routing));
  }, [routing]);

  const jobs = useMemo(() => rows.map(([j]) => j), [rows]);
  const update = (job: string, patch: Partial<Route>) =>
    setRows((r) => r.map(([j, route]) => (j === job ? [j, { ...route, ...patch }] : [j, route])));

  const body = () => ({
    routing: Object.fromEntries(rows.map(([j, r]) => [j, { ...r, effort: r.effort || null, max_tokens: r.max_tokens || null, cache_ttl: r.cache_ttl || null }])),
  });

  const save = async () => {
    setSaving(true); setMsg(null);
    try {
      await systemApi.updateAiSettings(body());
      setMsg({ ok: true, text: "Routing saved" });
      onSaved();
    } catch (e) {
      setMsg({ ok: false, text: extractApiError(e) });
    } finally { setSaving(false); }
  };

  const { inSaveBar } = useDraftSource("ai-routing", {
    count: changedJobs(rows, routing),
    save: async () => { await systemApi.updateAiSettings(body()); onSaved(); },
    discard: () => setRows(sorted(routing)),
  });

  const test = async (job: string) => {
    setTesting(job);
    try {
      const res = await systemApi.testAiConnection(job);
      setTests((t) => ({ ...t, [job]: res.data?.ok ? { ok: true, text: `${res.data.model}: ${res.data.reply}` }
                                                     : { ok: false, text: res.data?.error || "failed" } }));
    } catch (e) {
      setTests((t) => ({ ...t, [job]: { ok: false, text: extractApiError(e) } }));
    } finally { setTesting(null); }
  };

  return (
    <Card>
      <CardHeader title="Model per job"
        description={<>Every AI feature and every Nary job asks for its own row here (ai.routing). Claude Opus for what
          people read, Sonnet for moderate work, Haiku for scoring and sorting. Effort sets how hard the model thinks.
          Each job uses the key of its provider from API keys above. The <span className="font-mono">embedding</span> row picks
          the embeddings model, whose output must be 768-dimensional (a different size needs a migration and a re-index).</>} />
      <CardBody className="space-y-4">
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-left text-xs font-medium text-fg-muted">
                <th className="py-1.5 pr-2 font-medium">Job</th><th className="pr-2 font-medium">Provider</th><th className="pr-2 font-medium">Model</th>
                <th className="pr-2 font-medium">Effort</th><th className="pr-2 font-medium">Max tokens</th>
                <th className="pr-2 font-medium" title="How long Claude keeps this job's prompt cached. 1 hour suits chats; it costs 2x input to write instead of 1.25x.">Cache</th><th></th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line-soft">
              {rows.map(([job, r]) => (
                <tr key={job} className="align-top">
                  <td className="py-2 pr-2">
                    <span className="font-mono text-fg">{job}</span>
                    {JOB_HELP[job] && <span className="block text-[11px] text-fg-muted">{JOB_HELP[job]}</span>}
                    {providers[r.provider] && !providers[r.provider].keyless && !providers[r.provider].key_set && (
                      <span className="block text-[11px] text-tone-red">No {r.provider} key: add it in API keys above.</span>
                    )}
                    {tests[job] && (
                      <span className={cn("block text-[11px]", tests[job].ok ? "text-tone-blue" : "text-tone-red")}>{tests[job].text}</span>
                    )}
                  </td>
                  <td className="py-2 pr-2">
                    <Select aria-label={`Provider for ${job}`} value={r.provider} onChange={(e) => update(job, { provider: e.target.value })}
                      className={cn(cell, SELECT_MIN.provider)} options={PROVIDERS.map((p) => ({ value: p, label: p }))} />
                  </td>
                  <td className="min-w-[180px] py-2 pr-2">
                    <Input aria-label={`Model for ${job}`} list={`models-${r.provider}`} value={r.model}
                      onChange={(e) => update(job, { model: e.target.value })} className={cell} />
                  </td>
                  <td className="py-2 pr-2">
                    <Select aria-label={`Effort for ${job}`} value={r.effort || ""} onChange={(e) => update(job, { effort: e.target.value || null })}
                      className={cn(cell, SELECT_MIN.effort)} options={EFFORTS.map((x) => ({ value: x, label: x || "-" }))} />
                  </td>
                  <td className="w-24 py-2 pr-2">
                    <Input aria-label={`Max tokens for ${job}`} type="number" min={1} value={r.max_tokens ?? ""} className={cell} numeric
                      onChange={(e) => update(job, { max_tokens: e.target.value ? Number(e.target.value) : null })} />
                  </td>
                  <td className="py-2 pr-2">
                    <Select aria-label={`Cache for ${job}`} value={r.cache_ttl || ""} className={cn(cell, SELECT_MIN.cache)}
                      onChange={(e) => update(job, { cache_ttl: e.target.value || null })}
                      options={[{ value: "", label: "default" }, { value: "5m", label: "5 min" }, { value: "1h", label: "1 hour" }]} />
                  </td>
                  <td className="whitespace-nowrap py-2">
                    <div className="flex items-center gap-1">
                      <Button size="sm" variant="ghost" onClick={() => test(job)} loading={testing === job}>Test</Button>
                      {job !== "default" && (
                        <IconButton size="sm" label={`Remove ${job}`} icon={Trash2}
                          onClick={() => setRows((rs) => rs.filter(([j]) => j !== job))} />
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {Object.entries(MODEL_HINTS).map(([p, list]) => (
            <datalist key={p} id={`models-${p}`}>{list.map((m) => <option key={m} value={m} />)}</datalist>
          ))}
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Input aria-label="New job name" value={newJob} onChange={(e) => setNewJob(e.target.value)} placeholder="job name, e.g. nary.handoff" className="max-w-[220px]" />
          <Button size="sm" disabled={!newJob.trim() || jobs.includes(newJob.trim())}
            onClick={() => { setRows((r) => [...r, [newJob.trim(), { provider: "anthropic", model: "claude-sonnet-5-5", effort: "medium", max_tokens: 4000 }]]); setNewJob(""); }}>
            <Plus aria-hidden /> Add job
          </Button>
          {!inSaveBar && (
            <Button variant="primary" className="ml-auto" onClick={save} loading={saving}>Save routing</Button>
          )}
          {msg && (
            <span className={cn("inline-flex items-center gap-1 text-xs", msg.ok ? "text-tone-blue" : "text-tone-red")}>
              {msg.ok ? <CheckCircle2 className="size-3.5" aria-hidden /> : <XCircle className="size-3.5" aria-hidden />} {msg.text}
            </span>
          )}
        </div>
      </CardBody>
    </Card>
  );
}
