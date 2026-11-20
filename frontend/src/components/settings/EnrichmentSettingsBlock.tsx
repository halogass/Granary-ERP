"use client";

import { useCallback, useEffect, useState } from "react";
import { Button, Card, CardFooter, CardHeader, Checkbox, Input, Skeleton, Switch, toast } from "@/components/ui";
import { systemApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { useDraftSource } from "./SettingsDraft";

interface Setting { key: string; value: string }

const FIELDS: { key: string; label: string }[] = [
  { key: "website", label: "Website" },
  { key: "industry", label: "Industry" },
  { key: "country", label: "Country" },
  { key: "city", label: "City" },
  { key: "employee_count", label: "Employees" },
  { key: "annual_revenue", label: "Annual revenue" },
  { key: "description", label: "Description" },
];

const NUMBERS: { key: string; label: string; hint: string; min: number; max: number }[] = [
  { key: "min_confidence", label: "Confidence to write a value", hint: "Below this, a value is kept as a suggestion instead of filling the field", min: 0, max: 100 },
  { key: "refresh_days", label: "Re-research after", hint: "Days before a company is looked up again", min: 1, max: 365 },
  { key: "daily_limit", label: "Companies refreshed per day", hint: "The daily pass stops here, and at the AI budget", min: 1, max: 500 },
];

/**
 * Settings, Nary & AI: what company enrichment is allowed to fill in by itself.
 *
 * `searchBackend` is what the AI settings say would answer a research question right now:
 * with nothing configured, enrichment cannot run, and saying so here is the only warning
 * anybody gets before wondering why new companies stay empty.
 */
export default function EnrichmentSettingsBlock({ searchBackend }: { searchBackend?: string | null }) {
  const [values, setValues] = useState<Record<string, string>>({});
  const [loaded, setLoaded] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await systemApi.listSettings();
      const all: Setting[] = res.data.items ?? res.data ?? [];
      const v = Object.fromEntries(all
        .filter((s) => s.key.startsWith("crm.enrich."))
        .map((s) => [s.key.replace("crm.enrich.", ""), s.value ?? ""]));
      setValues(v);
      setLoaded(v);
    } catch {
      setValues({});
      setLoaded({});
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => { load(); }, [load]);

  const set = (key: string, value: string) => setValues((v) => ({ ...v, [key]: value }));
  const chosen = (values.fields ?? "").split(",").map((f) => f.trim()).filter(Boolean);

  function toggleField(key: string, on: boolean) {
    const next = on ? [...chosen, key] : chosen.filter((f) => f !== key);
    set("fields", FIELDS.filter((f) => next.includes(f.key)).map((f) => f.key).join(","));
  }

  const persist = () => Promise.all(Object.entries(values).map(([key, value]) =>
    systemApi.updateSetting(`crm.enrich.${key}`, value)));

  const { inSaveBar } = useDraftSource("ai-enrichment", {
    count: Object.keys(values).filter((k) => values[k] !== loaded[k]).length,
    save: async () => { await persist(); await load(); },
    discard: () => setValues(loaded),
  });

  async function save() {
    setSaving(true);
    try {
      await persist();
      toast.success("Enrichment settings saved");
      load();
    } catch (e) {
      toast.error(extractApiError(e, "Could not save the settings."));
    } finally {
      setSaving(false);
    }
  }

  if (loading) {
    return <Card className="space-y-2 p-4"><Skeleton className="h-4 w-48" /><Skeleton className="h-4 w-72" /></Card>;
  }

  return (
    <Card>
      <CardHeader title="Company enrichment"
        description="Research a company on the web and fill the fields nobody has filled in. A value a person entered is never overwritten, and every filled field keeps its sources." />

      {!searchBackend && (
        <p className="mx-4 mt-4 rounded-lg bg-tone-orange-soft px-3 py-2 text-xs text-tone-orange">
          No web-search backend is configured, so enrichment and research cannot run. Choose SearXNG
          above (free, runs with the ERP), use Gemini, or add a Tavily or Brave key.
        </p>
      )}

      <div className="divide-y divide-line">
        <div className="flex items-center justify-between gap-4 px-4 py-3">
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium text-fg" id="enrich-on-create">Research a new company on create</p>
            <p className="mt-0.5 text-xs text-fg-muted">Creating a company stays instant; the fields fill in seconds later.</p>
          </div>
          <Switch aria-labelledby="enrich-on-create"
            checked={(values.on_create ?? "true").toLowerCase() !== "false"}
            onCheckedChange={(on) => set("on_create", on ? "true" : "false")} />
        </div>

        <div className="px-4 py-3">
          <p className="text-sm font-medium text-fg">Fields it may fill</p>
          <div className="mt-2 flex flex-wrap gap-x-5 gap-y-1">
            {FIELDS.map((f) => (
              <Checkbox key={f.key} label={f.label} checked={chosen.includes(f.key)}
                onChange={(e) => toggleField(f.key, e.target.checked)} />
            ))}
          </div>
        </div>

        {NUMBERS.map((n) => (
          <div key={n.key} className="flex items-center justify-between gap-4 px-4 py-3">
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium text-fg" id={`enrich-${n.key}`}>{n.label}</p>
              <p className="mt-0.5 text-xs text-fg-muted">{n.hint}</p>
            </div>
            <Input aria-labelledby={`enrich-${n.key}`} type="number" min={n.min} max={n.max} numeric value={values[n.key] ?? ""}
              onChange={(e) => set(n.key, e.target.value)} className="w-28" />
          </div>
        ))}
      </div>

      {!inSaveBar && (
        <CardFooter>
          <Button variant="primary" onClick={save} loading={saving}>Save enrichment settings</Button>
        </CardFooter>
      )}
    </Card>
  );
}
