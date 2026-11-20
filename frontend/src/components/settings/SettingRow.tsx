"use client";

import { useId } from "react";
import { Input, Label, Pill, Select, Switch, Textarea } from "@/components/ui";
import { cn } from "@/lib/utils";
import { differsFromDefault, type SettingDef } from "@/lib/settings-registry";
import { JsonEditor, parseJson, sameJson } from "./JsonEditor";

/** Whether a draft value differs from the saved one (JSON compared by value). */
export function isEdited(def: SettingDef, draft: string | undefined, saved: string | undefined) {
  if (draft === undefined) return false;
  if (def.type === "json") return !sameJson(draft, saved);
  return draft !== (saved ?? "");
}

/** What is wrong with a value, or null. */
export function validateSetting(def: SettingDef, value: string): string | null {
  const v = value.trim();
  if (!v) return def.required ? "Required: payroll will not run without it." : null;
  if (def.type === "json") return parseJson(value).ok ? null : "Not valid JSON.";
  if (def.type === "number" || def.type === "percent" || def.type === "money") {
    const n = Number(v);
    if (!Number.isFinite(n)) return "Enter a number.";
    if (def.min !== undefined && n < def.min) return `At least ${def.min}.`;
    if (def.max !== undefined && n > def.max) return `At most ${def.max}.`;
  }
  if (def.type === "time" && !/^\d{1,2}:\d{2}$/.test(v)) return "Use HH:MM.";
  return null;
}

function shortDefault(def: SettingDef) {
  const d = def.default ?? "";
  if (def.type === "boolean") return d === "true" ? "on" : "off";
  return d === "" ? "empty" : d.length > 24 ? `${d.slice(0, 24)}...` : `${d}${def.unit ? ` ${def.unit}` : ""}`;
}

export interface SettingRowProps {
  def: SettingDef;
  /** The value on the server (undefined when the server has no such setting). */
  saved: string | undefined;
  /** The local edit, if any. */
  draft: string | undefined;
  onChange: (value: string) => void;
  error?: string | null;
  highlight?: boolean;
  disabled?: boolean;
}

/**
 * One setting: plain label and hint on the left, the control on the right (JSON editors span the
 * row), an "unsaved" mark while edited, a red "changed" mark when it differs from its default, and
 * the raw key small and grey.
 */
export function SettingRow({ def, saved, draft, onChange, error, highlight, disabled }: SettingRowProps) {
  const auto = useId();
  const id = `s${auto.replace(/:/g, "")}`;
  const value = draft ?? saved ?? "";
  const edited = isEdited(def, draft, saved);
  const changed = differsFromDefault(def, value);
  const wide = def.type === "json" || def.type === "textarea";
  const hintId = `${id}-hint`;
  const describedBy = [error ? `${id}-err` : null, def.hint ? hintId : null].filter(Boolean).join(" ") || undefined;
  const common = { id, disabled, "aria-describedby": describedBy, "aria-invalid": error ? true : undefined } as const;

  let control: React.ReactNode;
  switch (def.type) {
    case "boolean":
      control = (
        <Switch {...common} checked={value === "true"} onCheckedChange={(on) => onChange(on ? "true" : "false")} aria-label={def.label} />
      );
      break;
    case "select":
      control = (
        <Select {...common} value={value} onChange={(e) => onChange(e.target.value)}
          options={[...(def.options ?? []), ...(value && !def.options?.some((o) => o.value === value) ? [{ value, label: value }] : [])]}
          placeholder="Not set" />
      );
      break;
    case "textarea":
      control = <Textarea {...common} value={value} onChange={(e) => onChange(e.target.value)} rows={3} />;
      break;
    case "json":
      control = <JsonEditor id={id} value={value} onChange={onChange} columns={def.columns} label={def.label} invalid={!!error} />;
      break;
    case "number":
    case "percent":
    case "money":
      control = (
        <Input {...common} type="number" inputMode="decimal" numeric value={value} onChange={(e) => onChange(e.target.value)}
          min={def.min} max={def.max} step={def.step ?? "any"} trailing={def.unit} placeholder={def.default} />
      );
      break;
    default:
      control = (
        <Input {...common} type={def.type === "secret" ? "password" : def.type === "email" ? "email" : def.type === "url" ? "url" : def.type === "time" ? "time" : def.type === "date" ? "date" : "text"}
          value={value} onChange={(e) => onChange(e.target.value)} trailing={def.unit} placeholder={def.default ?? (saved === undefined ? "Not set" : undefined)} />
      );
  }

  return (
    <div id={`setting-${def.key}`} data-setting={def.key}
      className={cn("grid scroll-mt-24 gap-x-6 gap-y-2 px-4 py-3 transition-colors",
        wide ? "grid-cols-1" : "grid-cols-1 md:grid-cols-[minmax(0,1fr)_minmax(0,18rem)] md:items-center",
        highlight && "bg-brand/[0.07]")}>
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <Label htmlFor={id}>{def.label}</Label>
          {edited && <Pill tone="orange">Unsaved</Pill>}
          {changed && (
            <span className="text-xs font-medium text-brand-ink" title={def.default ? `Default: ${def.default}` : undefined}>
              changed (default {shortDefault(def)})
            </span>
          )}
        </div>
        {def.hint && <p id={hintId} className="mt-0.5 text-xs text-fg-muted">{def.hint}</p>}
        <p className="mt-0.5 break-all font-mono text-[11px] text-fg-subtle">{def.key}{saved === undefined ? " (not on the server)" : ""}</p>
      </div>
      <div className={cn("min-w-0", def.type === "boolean" && !wide && "md:justify-self-end")}>
        {control}
        {error && <p id={`${id}-err`} className="mt-1 text-xs text-tone-red">{error}</p>}
      </div>
    </div>
  );
}
