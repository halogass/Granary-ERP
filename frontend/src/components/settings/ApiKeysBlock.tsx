"use client";

import { useState } from "react";
import { KeyRound } from "lucide-react";
import { Button, Card, CardHeader, Input } from "@/components/ui";
import { cn } from "@/lib/utils";
import { systemApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";

export type ProviderState = { key_set: boolean; keyless: boolean; base_url: string; jobs: string[] };

export const PROVIDER_LABELS: Record<string, string> = {
  anthropic: "Anthropic (Claude)",
  gemini: "Google Gemini",
  openai: "OpenAI / compatible",
  deepseek: "DeepSeek",
  ollama: "Ollama (self-hosted)",
};
const ORDER = ["anthropic", "gemini", "openai", "deepseek", "ollama"];
// Providers that take a base URL: OpenAI-compatible endpoints and a self-hosted Ollama.
const BASE_URL_HINT: Record<string, string> = {
  openai: "Blank for api.openai.com, or an OpenAI-compatible URL, e.g. https://llm.example.com/v1",
  deepseek: "Blank for api.deepseek.com",
  ollama: "http://ollama:11434",
};

/** One key per AI provider. Every job (Model per job below) uses the key of the provider it names. */
export default function ApiKeysBlock({ providers, onSaved }: {
  providers: Record<string, ProviderState>; onSaved: () => void;
}) {
  const [keys, setKeys] = useState<Record<string, string>>({});
  const [urls, setUrls] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  // A key box stays read-only until it is clicked: browsers otherwise autofill a saved login password
  // into password fields, and saving that would replace a working key.
  const [editing, setEditing] = useState<Record<string, boolean>>({});
  const [notes, setNotes] = useState<Record<string, { ok: boolean; text: string }>>({});

  const note = (p: string, ok: boolean, text: string) => setNotes((n) => ({ ...n, [p]: { ok, text } }));

  const save = async (p: string) => {
    setBusy(`save:${p}`);
    try {
      const body: Record<string, unknown> = {};
      if ((keys[p] ?? "").trim()) body.provider_keys = { [p]: keys[p].trim() };
      if (urls[p] !== undefined) body.provider_base_urls = { [p]: urls[p] };
      await systemApi.updateAiSettings(body);
      setKeys((k) => ({ ...k, [p]: "" }));
      note(p, true, "Saved");
      onSaved();
    } catch (e) { note(p, false, extractApiError(e)); } finally { setBusy(null); }
  };

  const test = async (p: string) => {
    setBusy(`test:${p}`);
    try {
      const res = await systemApi.testAiProvider(p);
      note(p, !!res.data?.ok, res.data?.ok ? `Works${res.data.model ? ` (${res.data.model})` : ""}: ${res.data.reply}` : res.data?.error || "failed");
    } catch (e) { note(p, false, extractApiError(e)); } finally { setBusy(null); }
  };

  return (
    <Card>
      <CardHeader title={<span className="flex items-center gap-2"><KeyRound aria-hidden className="size-4 text-fg-muted" /> API keys</span>}
        description={<>One key per provider, stored encrypted. Every job in <span className="font-medium">Model per job</span> (embeddings
          included) uses the key of the provider it names. A key is never shown again; leave a box blank to keep it.</>} />
      <div className="divide-y divide-line">
        {ORDER.filter((p) => providers[p]).map((p) => {
          const st = providers[p];
          const needed = st.jobs.length > 0 && !st.keyless && !st.key_set;
          const dirty = (keys[p] ?? "").trim() !== "" || (urls[p] !== undefined && urls[p] !== st.base_url);
          return (
            <div key={p} className="grid grid-cols-1 gap-2 px-4 py-3 md:grid-cols-[180px_1fr_auto] md:items-start">
              <div>
                <p className="text-sm font-medium text-fg">{PROVIDER_LABELS[p] ?? p}</p>
                <p className={cn("text-xs", needed ? "text-tone-red" : st.key_set || st.keyless ? "text-tone-blue" : "text-fg-muted")}>
                  {st.keyless ? "No key needed" : st.key_set ? "Key saved" : needed ? "No key: jobs below need it" : "No key"}
                </p>
                {st.jobs.length > 0 && (
                  <p className="mt-0.5 text-[11px] text-fg-subtle" title={st.jobs.join(", ")}>
                    Used by {st.jobs.length} job{st.jobs.length === 1 ? "" : "s"}
                  </p>
                )}
              </div>
              <div className="space-y-1.5">
                {!st.keyless && (
                  <Input type="password" autoComplete="new-password" aria-label={`${PROVIDER_LABELS[p] ?? p} API key`}
                    name={`ai-key-${p}`} data-lpignore="true" data-1p-ignore="true"
                    readOnly={!editing[p]} onFocus={() => setEditing((e) => ({ ...e, [p]: true }))}
                    value={keys[p] ?? ""} onChange={(e) => setKeys((k) => ({ ...k, [p]: e.target.value }))}
                    placeholder={st.key_set ? "•••••••• (saved; paste a new one to replace it)" : "Paste the API key"} />
                )}
                {BASE_URL_HINT[p] !== undefined && (
                  <Input aria-label={`${PROVIDER_LABELS[p] ?? p} base URL`} value={urls[p] ?? st.base_url}
                    onChange={(e) => setUrls((u) => ({ ...u, [p]: e.target.value }))}
                    placeholder={BASE_URL_HINT[p]} />
                )}
                {dirty && <p className="text-xs text-tone-orange">Not saved yet: Test uses the saved key.</p>}
                {notes[p] && <p className={cn("text-xs", notes[p].ok ? "text-tone-blue" : "text-tone-red")}>{notes[p].text}</p>}
              </div>
              <div className="flex gap-2">
                <Button size="sm" variant="primary" onClick={() => save(p)} disabled={!dirty || busy !== null}
                  loading={busy === `save:${p}`}>Save</Button>
                <Button size="sm" onClick={() => test(p)} disabled={busy !== null || (!st.key_set && !st.keyless)}
                  loading={busy === `test:${p}`}>Test</Button>
              </div>
            </div>
          );
        })}
      </div>
    </Card>
  );
}
