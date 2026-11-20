"use client";

import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import { CheckCircle2, RefreshCw, XCircle } from "lucide-react";
import {
  Button, Card, CardBody, CardFooter, Field, FieldGrid, Input, Select, Skeleton, Switch, Textarea,
} from "@/components/ui";
import { systemApi } from "@/lib/api";
import { cn } from "@/lib/utils";
import EnrichmentSettingsBlock from "./EnrichmentSettingsBlock";
import AiRoutingBlock, { type Route } from "./AiRoutingBlock";
import AiBudgetBlock, { type BudgetSettings } from "./AiBudgetBlock";
import ApiKeysBlock, { type ProviderState } from "./ApiKeysBlock";
import AiKnowledgeTab from "./AiKnowledgeTab";
import { useDraftSource } from "./SettingsDraft";

type AssistantPersona = { system_prompt: string; default_identity: string };
type SearchProvider = { provider: string; key_set: boolean; base_url?: string; backend: string | null };
type Features = { assistant: boolean; scoring: boolean; suggestions: boolean; doc_intel: boolean; drafting: boolean };
export type AiSettings = {
  enabled: boolean;
  providers: Record<string, ProviderState>;
  features: Features;
  scoring_rubric: string;
  search: SearchProvider;
  assistant?: AssistantPersona;
  no_em_dash?: boolean;
  routing: Record<string, Route>;
  pricing: Record<string, { input: number; output: number; cache_read: number; cache_write: number }>;
  batch_discount_pct: string;
  budget: BudgetSettings;
};

/** The fields of PUT /system/ai-settings this provider edits, as the server would read them back. */
function editable(s: AiSettings, searchApiKey: string) {
  return {
    enabled: s.enabled,
    feature_assistant: s.features.assistant, feature_scoring: s.features.scoring,
    feature_suggestions: s.features.suggestions, feature_doc_intel: s.features.doc_intel,
    feature_drafting: s.features.drafting,
    scoring_rubric: s.scoring_rubric,
    search_provider: s.search?.provider ?? "",
    search_base_url: s.search?.base_url ?? "",
    assistant_system_prompt: s.assistant?.system_prompt ?? "",
    no_em_dash: s.no_em_dash ?? true,
    search_api_key: searchApiKey.trim(),
  };
}

interface AiCtx {
  s: AiSettings | null;
  loading: boolean;
  reload: () => Promise<void>;
  patch: (p: Partial<AiSettings>) => void;
  toggleFeature: (k: keyof Features) => void;
  patchAssistant: (p: Partial<AssistantPersona>) => void;
  patchSearch: (p: Partial<SearchProvider>) => void;
  searchApiKey: string;
  setSearchApiKey: (v: string) => void;
  /** Saves the AI settings form (master switch, features, persona, research, rubric). */
  save: () => Promise<void>;
  inSaveBar: boolean;
}

const Ctx = createContext<AiCtx | null>(null);

/** Loads the AI settings once for every AI block on the page and registers their form with the save bar. */
export function AiSettingsProvider({ children }: { children: React.ReactNode }) {
  const [loaded, setLoaded] = useState<AiSettings | null>(null);
  const [s, setS] = useState<AiSettings | null>(null);
  const [searchApiKey, setSearchApiKey] = useState("");
  const [loading, setLoading] = useState(true);

  const reload = useCallback(async () => {
    try {
      const res = await systemApi.getAiSettings();
      setLoaded(res.data);
      setS(res.data);
    } catch { /* ignore */ } finally { setLoading(false); }
  }, []);
  useEffect(() => { reload(); }, [reload]);

  const save = useCallback(async () => {
    if (!s) return;
    const body: Record<string, unknown> = editable(s, "");
    delete body.search_api_key;
    if (searchApiKey.trim()) body.search_api_key = searchApiKey.trim();
    await systemApi.updateAiSettings(body);
    setSearchApiKey("");
    await reload();
  }, [s, searchApiKey, reload]);

  let count = 0;
  if (s && loaded) {
    const a = editable(s, searchApiKey) as Record<string, unknown>;
    const b = editable(loaded, "") as Record<string, unknown>;
    count = Object.keys(a).filter((k) => a[k] !== b[k]).length;
  }
  const { inSaveBar } = useDraftSource("ai-settings", {
    count,
    save,
    discard: () => { setS(loaded); setSearchApiKey(""); },
  });

  const value = useMemo<AiCtx>(() => ({
    s, loading, reload, save, inSaveBar, searchApiKey, setSearchApiKey,
    patch: (p) => setS((cur) => (cur ? { ...cur, ...p } : cur)),
    toggleFeature: (k) => setS((cur) => (cur ? { ...cur, features: { ...cur.features, [k]: !cur.features[k] } } : cur)),
    patchAssistant: (p) => setS((cur) => (cur ? { ...cur, assistant: { ...(cur.assistant ?? { system_prompt: "", default_identity: "" }), ...p } } : cur)),
    patchSearch: (p) => setS((cur) => (cur ? { ...cur, search: { ...cur.search, ...p } } : cur)),
  }), [s, loading, reload, save, inSaveBar, searchApiKey]);

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** The AI settings for a block inside AiSettingsProvider. */
export function useAiSettings(): AiCtx {
  const c = useContext(Ctx);
  if (!c) throw new Error("useAiSettings needs an AiSettingsProvider");
  return c;
}

function Loading() {
  return <Card className="space-y-2 p-4"><Skeleton className="h-4 w-48" /><Skeleton className="h-4 w-72" /></Card>;
}

function Result({ r }: { r: { ok: boolean; msg: string } | null }) {
  if (!r) return null;
  return (
    <span className={cn("inline-flex items-center gap-1 text-sm", r.ok ? "text-tone-blue" : "text-tone-red")}>
      {r.ok ? <CheckCircle2 className="size-4" aria-hidden /> : <XCircle className="size-4" aria-hidden />} {r.msg}
    </span>
  );
}

function SwitchRow({ id, label, hint, checked, onChange }: {
  id: string; label: string; hint?: React.ReactNode; checked: boolean; onChange: () => void;
}) {
  return (
    <div className="flex items-center justify-between gap-4 px-4 py-3">
      <div className="min-w-0 flex-1">
        <p id={id} className="text-sm text-fg">{label}</p>
        {hint && <p className="mt-0.5 text-xs text-fg-muted">{hint}</p>}
      </div>
      <Switch aria-labelledby={id} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

const FEATURES = [
  ["assistant", "Nary chat (Ctrl J)"],
  ["drafting", "Draft quotations and invoices"],
  ["scoring", "CRM lead scoring"],
  ["suggestions", "Reply and draft suggestions"],
  ["doc_intel", "Document intelligence"],
] as const;

/** Master switch, feature switches, the no-em-dash rule, Test connection (and Save outside the save bar). */
export function AiGeneralBlock() {
  const { s, loading, patch, toggleFeature, save, inSaveBar } = useAiSettings();
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; msg: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  if (loading || !s) return <Loading />;

  const test = async () => {
    setTesting(true); setTestResult(null);
    try {
      const res = await systemApi.testAiConnection();
      setTestResult(res.data?.ok
        ? { ok: true, msg: `Connected (${res.data.provider}). Reply: ${res.data.reply || "OK"}` }
        : { ok: false, msg: res.data?.error || "Connection failed" });
    } catch (e) {
      setTestResult({ ok: false, msg: (e as Error).message });
    } finally { setTesting(false); }
  };

  const ownSave = async () => {
    setSaving(true); setSaved(false);
    try {
      await save();
      setSaved(true);
      setTimeout(() => setSaved(false), 2500);
    } catch { /* ignore */ } finally { setSaving(false); }
  };

  return (
    <Card>
      <div className="divide-y divide-line">
        <SwitchRow id="ai-enabled" label="AI features" checked={s.enabled} onChange={() => patch({ enabled: !s.enabled })}
          hint="Off stops every AI feature, Nary included. Keys are in API keys; models in Model per job." />
        <SwitchRow id="ai-no-em-dash" label="No em dashes in anything the AI writes" checked={s.no_em_dash ?? true}
          onChange={() => patch({ no_em_dash: !(s.no_em_dash ?? true) })}
          hint="Assistant, reply suggestions, drafts, briefs, AI fields, research and connected agents. Removed automatically, not just requested. Imported documents keep their own punctuation." />
        {FEATURES.map(([k, lbl]) => (
          <SwitchRow key={k} id={`ai-feature-${k}`} label={lbl} checked={s.features[k]} onChange={() => toggleFeature(k)} />
        ))}
      </div>
      <CardFooter className="justify-start">
        {!inSaveBar && <Button variant="primary" onClick={ownSave} loading={saving}>Save settings</Button>}
        <Button onClick={test} loading={testing}>Test connection</Button>
        {saved && <Result r={{ ok: true, msg: "Saved" }} />}
        <Result r={testResult} />
      </CardFooter>
    </Card>
  );
}

/** One key per provider (saved one at a time, at once). */
export function AiKeysBlock() {
  const { s, loading, reload } = useAiSettings();
  if (loading || !s) return <Loading />;
  return <ApiKeysBlock providers={s.providers || {}} onSaved={reload} />;
}

export function AiRoutingSection() {
  const { s, loading, reload } = useAiSettings();
  if (loading || !s) return <Loading />;
  return <AiRoutingBlock routing={s.routing} providers={s.providers || {}} onSaved={reload} />;
}

export function AiBudgetSection() {
  const { s, loading, reload } = useAiSettings();
  if (loading || !s) return <Loading />;
  return (
    <AiBudgetBlock budget={s.budget} pricing={s.pricing} batchDiscount={String(s.batch_discount_pct ?? "")}
      jobs={Object.keys(s.routing || {})} onSaved={reload} />
  );
}

/** Nary's persona for staff chats. */
export function AiPersonaBlock() {
  const { s, loading, patchAssistant } = useAiSettings();
  if (loading || !s) return <Loading />;
  const prompt = s.assistant?.system_prompt ?? "";
  return (
    <Card>
      <CardBody className="space-y-3">
        <Field label="Nary persona for the chat"
          hint={<>How Nary thinks and advises when staff chat with it. It comes after Nary&apos;s own identity and fixed
            rules (shown greyed when this is empty), and where it asks for a format or length it wins over Nary&apos;s
            default brevity. The operating rules always apply on top: company data only from the ERP, never invented
            numbers, and nothing saved without the user confirming. Customers never see this text.</>}>
          <Textarea value={prompt} onChange={(e) => patchAssistant({ system_prompt: e.target.value })}
            rows={16} placeholder={s.assistant?.default_identity || ""} className="font-mono text-xs" />
        </Field>
        <div className="flex flex-wrap items-center justify-between gap-3 text-xs text-fg-muted">
          <span>Changes apply from the next message after you save.</span>
          <span className="flex items-center gap-3">
            <span className="tabular">{prompt.length.toLocaleString()} characters</span>
            {prompt && (
              <Button size="sm" variant="ghost" onClick={() => patchAssistant({ system_prompt: "" })}>Use the built-in assistant</Button>
            )}
          </span>
        </div>
      </CardBody>
    </Card>
  );
}

/** Where research, enrichment and research attributes get their facts. */
export function AiResearchBlock() {
  const { s, loading, patchSearch, searchApiKey, setSearchApiKey } = useAiSettings();
  if (loading || !s) return <Loading />;
  return (
    <Card>
      <CardBody className="space-y-3">
        <p className="text-xs text-fg-muted">
          SearXNG runs with the ERP and costs nothing; Gemini can search on its own; Tavily and Brave need a paid key.
        </p>
        <FieldGrid cols={2}>
          <Field label="Search backend">
            <Select value={s.search?.provider ?? ""} onChange={(e) => patchSearch({ provider: e.target.value })}
              options={[
                { value: "", label: "None (or the provider's own search)" },
                { value: "searxng", label: "SearXNG (self-hosted, free)" },
                { value: "tavily", label: "Tavily" },
                { value: "brave", label: "Brave Search" },
              ]} />
          </Field>
          {s.search?.provider === "searxng" ? (
            <Field label="SearXNG address">
              <Input value={s.search?.base_url ?? ""} onChange={(e) => patchSearch({ base_url: e.target.value })}
                placeholder="http://searxng:8080" />
            </Field>
          ) : (
            <Field label="Search API key" hint={s.search?.key_set ? "Set: leave blank to keep it." : undefined}>
              <Input type="password" value={searchApiKey} onChange={(e) => setSearchApiKey(e.target.value)}
                autoComplete="new-password" placeholder={s.search?.key_set ? "••••••••" : "Paste search API key"} />
            </Field>
          )}
        </FieldGrid>
        <p className="text-xs">
          {s.search?.backend
            ? <span className="text-tone-blue">Research is available, answered by {s.search.backend === "grounded" ? "the model provider's own search" : s.search.backend === "searxng" ? "SearXNG" : s.search.backend}.</span>
            : <span className="text-tone-orange">No backend would answer right now, so research attributes and enrichment stay off.</span>}
        </p>
      </CardBody>
    </Card>
  );
}

export function AiEnrichmentBlock() {
  const { s, loading } = useAiSettings();
  if (loading || !s) return <Loading />;
  return <EnrichmentSettingsBlock searchBackend={s.search?.backend} />;
}

/** How the AI scores leads (hot/warm/cold). */
export function AiScoringBlock() {
  const { s, loading, patch } = useAiSettings();
  if (loading || !s) return <Loading />;
  return (
    <Card>
      <CardBody>
        <Field label="Lead-scoring rubric" hint="Guides how the AI scores leads (hot/warm/cold). Leave blank to use the default.">
          <Textarea value={s.scoring_rubric || ""} onChange={(e) => patch({ scoring_rubric: e.target.value })} rows={4}
            placeholder="e.g. Hot = explicit budget + timeline + decision-maker engaged; Warm = clear interest, no timeline; Cold = info-gathering only..." />
        </Field>
      </CardBody>
    </Card>
  );
}

/** Re-embedding the SOPs, then the AI-only documents. */
export function AiKnowledgeBlock() {
  const [reindexing, setReindexing] = useState(false);
  const reindex = async () => {
    setReindexing(true);
    try { await systemApi.reindexKb(); } catch { /* ignore */ }
    setTimeout(() => setReindexing(false), 1500);
  };
  return (
    <div className="space-y-4">
      <Card>
        <div className="flex flex-wrap items-center justify-between gap-4 px-4 py-3">
          <div>
            <p className="text-sm font-medium text-fg">Knowledge index</p>
            <p className="mt-0.5 text-xs text-fg-muted">Re-embed published, non-private SOPs so the assistant can cite them.</p>
          </div>
          <Button onClick={reindex} disabled={reindexing}>
            <RefreshCw aria-hidden className={reindexing ? "animate-spin" : undefined} /> {reindexing ? "Queued" : "Reindex KB"}
          </Button>
        </div>
      </Card>
      <AiKnowledgeTab />
    </div>
  );
}
