"use client";

import { useState } from "react";
import { Zap, Loader2, Check, X } from "lucide-react";
import { crmRecordsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Checkbox, IconButton, Input, Pill, Select, Textarea } from "@/components/ui";

export interface AttributeDef {
  id: string;
  object_type: string;
  key: string;
  label: string;
  description?: string | null;
  field_type: string;
  options: { choices?: string[]; currency?: string; max?: number; object_type?: string };
  is_ai: boolean;
  ai_kind?: string | null;
  ai_config: Record<string, unknown>;
  sort_order: number;
  is_active: boolean;
}

export interface AiFieldRun {
  value: unknown;
  confidence: number | null;
  reasoning: string | null;
  sources: { url?: string; title?: string }[];
  model: string | null;
  at: string | null;
}

function displayValue(def: AttributeDef, v: unknown): string {
  if (v === null || v === undefined || v === "") return "-";
  if (def.field_type === "checkbox") return v ? "Yes" : "No";
  if (def.field_type === "multi_select" && Array.isArray(v)) return v.join(", ");
  if (def.field_type === "currency") {
    const cur = def.options?.currency || "USD";
    return `${cur} ${Number(v).toLocaleString()}`;
  }
  if (def.field_type === "rating") return `${v} / ${def.options?.max ?? 5}`;
  return String(v);
}

/** One editable attribute row. AI attributes render read-only with a Run button. */
function AttributeRow({
  def, value, run, objectType, recordId, onSave, onRan,
}: {
  def: AttributeDef;
  value: unknown;
  run?: AiFieldRun;
  objectType: string;
  recordId: string;
  onSave: (key: string, v: unknown) => Promise<void>;
  onRan: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<unknown>(value ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function commit() {
    setBusy(true); setError("");
    try {
      await onSave(def.key, draft === "" ? null : draft);
      setEditing(false);
    } catch (e) {
      setError(extractApiError(e, "Could not save."));
    } finally { setBusy(false); }
  }

  async function runAi() {
    setBusy(true); setError("");
    try {
      await crmRecordsApi.runAiField(objectType, recordId, def.key);
      onRan();
    } catch (e) {
      setError(extractApiError(e, "The AI run failed."));
    } finally { setBusy(false); }
  }

  const confidence = run?.confidence;

  return (
    <div className="border-b border-line-soft py-2.5 last:border-0">
      <div className="mb-1 flex items-center gap-1.5">
        <span className="text-xs text-fg-muted">{def.label}</span>
        {def.is_ai && (
          <Pill tone="violet" icon={Zap} title={run?.reasoning || "Filled by AI"} className="h-[18px] px-1.5 text-[11px]">
            AI{confidence != null && <span className="ml-1 tabular opacity-80">{Math.round(confidence)}%</span>}
          </Pill>
        )}
      </div>

      {def.is_ai ? (
        <div className="flex items-start gap-2">
          <p className="flex-1 whitespace-pre-wrap break-words text-sm text-fg">
            {displayValue(def, value)}
          </p>
          <IconButton size="sm" icon={busy ? Loader2 : Zap} onClick={runAi} disabled={busy}
            label={`Recompute ${def.label} with AI`} title="Recompute with AI"
            className={busy ? "[&_svg]:animate-spin" : undefined} />
        </div>
      ) : editing ? (
        <div className="flex items-center gap-1.5">
          <div className="min-w-0 flex-1"><AttributeEditor def={def} value={draft} onChange={setDraft} /></div>
          <IconButton size="sm" icon={busy ? Loader2 : Check} onClick={commit} disabled={busy}
            label={`Save ${def.label}`} className={busy ? "[&_svg]:animate-spin" : "text-tone-blue"} />
          <IconButton size="sm" icon={X} label={`Cancel editing ${def.label}`}
            onClick={() => { setEditing(false); setDraft(value ?? ""); setError(""); }} />
        </div>
      ) : (
        <button type="button" onClick={() => { setDraft(value ?? ""); setEditing(true); }}
          className="w-full break-words rounded text-left text-sm text-fg transition-colors hover:text-brand-ink">
          {displayValue(def, value)}
        </button>
      )}

      {run?.reasoning && def.is_ai && (
        <p className="mt-1 line-clamp-2 text-xs italic text-fg-muted">{run.reasoning}</p>
      )}
      {error && <p className="mt-1 text-xs text-tone-red">{error}</p>}
    </div>
  );
}

function AttributeEditor({ def, value, onChange }: {
  def: AttributeDef; value: unknown; onChange: (v: unknown) => void;
}) {
  const v = value as string | number | boolean | string[] | undefined;

  const label = def.label;

  switch (def.field_type) {
    case "long_text":
      return <Textarea aria-label={label} rows={3} value={(v as string) ?? ""} onChange={(e) => onChange(e.target.value)} />;
    case "checkbox":
      return <Checkbox aria-label={label} checked={Boolean(v)} onChange={(e) => onChange(e.target.checked)} />;
    case "number": case "currency": case "rating":
      return <Input aria-label={label} numeric type="number" step="any" value={(v as number) ?? ""} onChange={(e) => onChange(e.target.value)} />;
    case "date":
      return <Input aria-label={label} type="date" value={(v as string) ?? ""} onChange={(e) => onChange(e.target.value)} />;
    case "select":
      return (
        <Select aria-label={label} value={(v as string) ?? ""} onChange={(e) => onChange(e.target.value)}>
          <option value="">-</option>
          {(def.options?.choices ?? []).map((c) => <option key={c} value={c}>{c}</option>)}
        </Select>
      );
    case "multi_select": {
      const selected = new Set(Array.isArray(v) ? (v as string[]) : []);
      return (
        <div className="flex flex-wrap gap-1" role="group" aria-label={label}>
          {(def.options?.choices ?? []).map((c) => {
            const on = selected.has(c);
            return (
              <button key={c} type="button" aria-pressed={on}
                onClick={() => {
                  const next = new Set(selected);
                  if (on) next.delete(c); else next.add(c);
                  onChange([...next]);
                }}
                className={`rounded-lg border px-2 py-0.5 text-xs font-medium transition-colors ${
                  on ? "border-fg bg-fg text-surface" : "border-line bg-surface text-fg-muted hover:bg-surface-2"}`}>
                {c}
              </button>
            );
          })}
        </div>
      );
    }
    default:
      return <Input aria-label={label} type={def.field_type === "email" ? "email" : "text"}
        value={(v as string) ?? ""} onChange={(e) => onChange(e.target.value)} />;
  }
}

export default function AttributePanel({
  defs, values, runs, objectType, recordId, onSave, onRan,
}: {
  defs: AttributeDef[];
  values: Record<string, unknown>;
  runs: Record<string, AiFieldRun>;
  objectType: string;
  recordId: string;
  onSave: (key: string, v: unknown) => Promise<void>;
  onRan: () => void;
}) {
  if (defs.length === 0) {
    return (
      <p className="py-3 text-[13px] text-fg-muted">
        No custom attributes yet. Add them in Settings, CRM attributes.
      </p>
    );
  }
  return (
    <div>
      {defs.map((d) => (
        <AttributeRow key={d.id} def={d} value={values[d.key]} run={runs[d.key]}
          objectType={objectType} recordId={recordId} onSave={onSave} onRan={onRan} />
      ))}
    </div>
  );
}
