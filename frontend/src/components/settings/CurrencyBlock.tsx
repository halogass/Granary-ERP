"use client";

import { useCallback, useEffect, useState } from "react";
import { CheckCircle, Eye, EyeOff, RefreshCw, XCircle } from "lucide-react";
import { systemApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { SUPPORTED_CURRENCIES, CURRENCY_META } from "@/lib/currency";
import { invalidateCurrencySettingsCache } from "@/hooks/useCurrencySettings";
import { Button, Card, CardBody, CardFooter, Field, IconButton, Input, Select, Skeleton, toast } from "@/components/ui";
import { cn } from "@/lib/utils";
import { useDraftSource } from "./SettingsDraft";

interface CurrencyConfig { functional: string; active: string[]; all_supported: string[]; has_api_key: boolean; last_synced: string | null }

const sameSet = (a: string[], b: string[]) => a.length === b.length && a.every((x) => b.includes(x));

/**
 * Functional currency, active currencies, the ExchangeRate API key and the sync. Edits collect in
 * the save bar and go out as one PATCH /system/currencies.
 */
export default function CurrencyBlock() {
  const [cfg, setCfg] = useState<CurrencyConfig | null>(null);
  const [functional, setFunctional] = useState("");
  const [active, setActive] = useState<string[]>([]);
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [saving, setSaving] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [syncResult, setSyncResult] = useState<{ ok: boolean; msg: string } | null>(null);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; msg: string } | null>(null);

  const load = useCallback(async () => {
    try {
      const res = await systemApi.getCurrencySettings();
      setCfg(res.data);
      setFunctional(res.data.functional);
      setActive(res.data.active);
    } catch { /* non-critical */ }
  }, []);
  useEffect(() => { load(); }, [load]);

  const patch: { functional?: string; active?: string[]; api_key?: string } = {};
  if (cfg && functional !== cfg.functional) patch.functional = functional;
  if (cfg && !sameSet(active, cfg.active)) patch.active = active;
  if (apiKey.trim()) patch.api_key = apiKey.trim();
  const count = Object.keys(patch).length;

  const save = async () => {
    setSaving(true);
    try {
      await systemApi.updateCurrencySettings(patch);
      // Other pages read these through a module-level cache; drop it so a changed functional
      // currency or active list takes effect without a full reload.
      invalidateCurrencySettingsCache();
      setApiKey("");
      await load();
    } finally {
      setSaving(false);
    }
  };
  const discard = () => {
    if (cfg) { setFunctional(cfg.functional); setActive(cfg.active); }
    setApiKey("");
  };
  const { inSaveBar } = useDraftSource("currency", { count, save, discard });

  const syncNow = async () => {
    setSyncing(true); setSyncResult(null);
    try {
      await systemApi.syncExchangeRates();
      setSyncResult({ ok: true, msg: "Sync queued. Rates update in a few seconds." });
      setTimeout(() => { load(); setSyncResult(null); }, 5000);
    } catch {
      setSyncResult({ ok: false, msg: "Could not queue the sync." });
    } finally {
      setSyncing(false);
    }
  };

  const testKey = async () => {
    setTesting(true); setTestResult(null);
    try {
      const res = await systemApi.testFxApiKey();
      setTestResult({ ok: true, msg: `Connected: ${res.data.pairs} currency pairs available` });
    } catch (e) {
      setTestResult({ ok: false, msg: extractApiError(e, "Connection failed") });
    } finally {
      setTesting(false);
    }
  };

  if (!cfg) {
    return <Card><CardBody className="space-y-3"><Skeleton className="h-4 w-40" /><Skeleton className="h-9 w-full" /></CardBody></Card>;
  }
  const all = cfg.all_supported ?? SUPPORTED_CURRENCIES;
  const meta = (c: string) => CURRENCY_META[c as keyof typeof CURRENCY_META];

  return (
    <Card>
      <div className="divide-y divide-line">
        <div className="grid gap-2 px-4 py-3 md:grid-cols-[minmax(0,1fr)_minmax(0,18rem)] md:items-center">
          <div>
            <p className="text-[13px] font-medium text-fg">Functional currency</p>
            <p className="text-xs text-fg-muted">Base currency for P&amp;L, balance sheet and tax reports.</p>
            <p className="font-mono text-[11px] text-fg-subtle">currency.functional</p>
          </div>
          <Select aria-label="Functional currency" value={functional} onChange={(e) => setFunctional(e.target.value)}
            options={all.map((c) => ({ value: c, label: `${c}, ${meta(c)?.name ?? c}` }))} />
        </div>

        <div className="space-y-2 px-4 py-3">
          <div>
            <p className="text-[13px] font-medium text-fg">Active currencies</p>
            <p className="text-xs text-fg-muted">Only these appear in transaction dropdowns. The functional currency is always active.</p>
            <p className="font-mono text-[11px] text-fg-subtle">currency.active</p>
          </div>
          <div className="flex flex-wrap gap-2" role="group" aria-label="Active currencies">
            {all.map((c) => {
              const on = active.includes(c);
              const isFunctional = c === functional;
              return (
                <button key={c} type="button" aria-pressed={on} disabled={isFunctional}
                  onClick={() => setActive((a) => (on ? a.filter((x) => x !== c) : [...a, c]))}
                  className={cn("inline-flex min-h-11 items-center gap-1.5 rounded-full border px-3 text-xs font-medium transition-colors md:min-h-7",
                    on ? "border-brand/30 bg-brand/[0.07] text-brand-ink" : "border-line-strong text-fg-muted hover:bg-surface-2",
                    isFunctional && "cursor-not-allowed opacity-70")}>
                  <span aria-hidden>{meta(c)?.symbol}</span> {c}
                  {isFunctional && <span className="text-[10px] text-fg-subtle">(base)</span>}
                </button>
              );
            })}
          </div>
        </div>

        <div className="space-y-2 px-4 py-3">
          <Field label="ExchangeRate API key" hint="Free tier at exchangerate-api.com. Rates sync every 6 hours.">
            <div className="flex flex-wrap gap-2">
              <div className="flex min-w-0 flex-1 gap-1 md:max-w-sm">
                <Input type={showKey ? "text" : "password"} value={apiKey} onChange={(e) => setApiKey(e.target.value)}
                  placeholder={cfg.has_api_key ? "Saved. Paste a new key to replace it" : "Paste the API key"} autoComplete="off" />
                <IconButton label={showKey ? "Hide key" : "Show key"} icon={showKey ? EyeOff : Eye} onClick={() => setShowKey((s) => !s)} />
              </div>
              <Button onClick={testKey} loading={testing} disabled={!cfg.has_api_key}>Test</Button>
            </div>
          </Field>
          {testResult && <Result r={testResult} />}
        </div>

        <div className="flex flex-wrap items-center gap-3 px-4 py-3">
          <div className="min-w-0 flex-1">
            <p className="text-[13px] font-medium text-fg">Exchange rate sync</p>
            <p className="text-xs text-fg-muted tabular">
              {cfg.last_synced
                ? `Last synced ${new Date(cfg.last_synced).toLocaleString("en-US", { dateStyle: "medium", timeStyle: "short" })}`
                : "Never synced"}
            </p>
            {syncResult && <Result r={syncResult} />}
          </div>
          <Button onClick={syncNow} disabled={syncing || !cfg.has_api_key}>
            <RefreshCw className={syncing ? "animate-spin" : undefined} aria-hidden /> {syncing ? "Syncing..." : "Sync now"}
          </Button>
        </div>
      </div>
      {!inSaveBar && count > 0 && (
        <CardFooter>
          <Button variant="primary" loading={saving}
            onClick={() => save().then(() => toast.success("Currency settings saved"), (e) => toast.error(extractApiError(e)))}>
            Save currency settings
          </Button>
        </CardFooter>
      )}
    </Card>
  );
}

function Result({ r }: { r: { ok: boolean; msg: string } }) {
  return (
    <p role="status" className={cn("mt-1 inline-flex items-center gap-1.5 text-xs font-medium", r.ok ? "text-tone-blue" : "text-tone-red")}>
      {r.ok ? <CheckCircle className="size-3.5" aria-hidden /> : <XCircle className="size-3.5" aria-hidden />} {r.msg}
    </p>
  );
}
