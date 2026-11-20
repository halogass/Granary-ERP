"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { financeApi, systemApi } from "@/lib/api";
import { Plus, Trash2, RefreshCw, Clock, ArrowRight } from "lucide-react";
import {
  Button, Card, CardBody, CardHeader, DataTable, EmptyState, Field, IconButton, Input, PageHeader, Pill,
  Skeleton, type ColumnDef,
} from "@/components/ui";

interface FXRate {
  id: string;
  from_currency: string;
  to_currency: string;
  rate: number;
  effective_date: string;
  source: string;
}

export default function FxPage() {
  const [rates, setRates] = useState<FXRate[]>([]);
  const [loading, setLoading] = useState(true);
  const [activeCurrencies, setActiveCurrencies] = useState<string[]>(["USD", "EUR"]);
  const [functional, setFunctional] = useState("USD");
  const [lastSynced, setLastSynced] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [form, setForm] = useState({
    from_currency: "EUR", to_currency: "USD",
    rate: "", effective_date: new Date().toISOString().slice(0, 10),
  });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [ratesRes, currRes] = await Promise.all([
        financeApi.listExchangeRates(),
        systemApi.getCurrencySettings().catch(() => null),
      ]);
      setRates(ratesRes.data ?? []);
      if (currRes?.data) {
        setActiveCurrencies(currRes.data.active ?? ["USD", "EUR"]);
        setFunctional(currRes.data.functional ?? "USD");
        setLastSynced(currRes.data.last_synced ?? null);
      }
    } catch { setRates([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  // Build quick-fill pairs from active currencies
  const quickPairs: { from: string; to: string }[] = [];
  for (const c of activeCurrencies) {
    if (c !== functional) quickPairs.push({ from: c, to: functional });
  }

  async function save() {
    setSaving(true); setError("");
    try {
      await financeApi.createExchangeRate({
        from_currency: form.from_currency.toUpperCase(),
        to_currency: form.to_currency.toUpperCase(),
        rate: parseFloat(form.rate),
        effective_date: form.effective_date,
      });
      setForm(f => ({ ...f, rate: "" }));
      load();
    } catch (e: unknown) {
      const msg = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      setError(msg ?? "Failed to save rate");
    } finally { setSaving(false); }
  }

  async function deleteRate(id: string) {
    await financeApi.deleteExchangeRate(id);
    load();
  }

  async function handleSyncNow() {
    setSyncing(true);
    try {
      await systemApi.syncExchangeRates();
      setTimeout(load, 3000);
    } finally { setSyncing(false); }
  }

  // Group by pair, filter to active currencies only
  const grouped: Record<string, FXRate[]> = {};
  rates.forEach(r => {
    if (!activeCurrencies.includes(r.from_currency) && !activeCurrencies.includes(r.to_currency)) return;
    const key = `${r.from_currency}/${r.to_currency}`;
    if (!grouped[key]) grouped[key] = [];
    grouped[key].push(r);
  });
  Object.values(grouped).forEach(arr => arr.sort((a, b) => b.effective_date.localeCompare(a.effective_date)));

  const columns = useMemo<ColumnDef<FXRate>[]>(() => [
    {
      accessorKey: "effective_date", header: "Effective", meta: { width: "160px" },
      cell: (c) => <span className="tabular text-fg-muted">{new Date(c.getValue<string>()).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" })}</span>,
    },
    {
      accessorKey: "rate", header: "Rate", meta: { numeric: true },
      cell: (c) => {
        const r = c.row.original;
        return <span className="font-medium">1 {r.from_currency} = {r.rate.toLocaleString("en-US", { maximumFractionDigits: 6 })} {r.to_currency}</span>;
      },
    },
    {
      accessorKey: "source", header: "Source", meta: { hideOnMobile: true },
      cell: (c) => <Pill tone={c.getValue<string>() === "exchangerate-api" ? "blue" : "neutral"}>{c.getValue<string>()}</Pill>,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "56px" },
      cell: (c) => <IconButton size="sm" variant="ghost" icon={Trash2} label="Delete rate" onClick={() => deleteRate(c.row.original.id)} />,
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], []);

  return (
    <div className="animate-fade-up">
      <PageHeader
        title="Exchange rates"
        summary="Multi-currency FX rates, auto-synced every 6 hours"
        actions={
          <>
            {lastSynced && (
              <span className="flex items-center gap-1.5 text-xs text-fg-muted tabular">
                <Clock className="size-3.5" aria-hidden />
                {new Date(lastSynced).toLocaleString("en-US", { dateStyle: "short", timeStyle: "short" })}
              </span>
            )}
            <Button onClick={handleSyncNow} disabled={syncing}>
              <RefreshCw className={`size-4 ${syncing ? "animate-spin" : ""}`} aria-hidden />
              {syncing ? "Syncing…" : "Sync now"}
            </Button>
          </>
        }
      />

      <Card className="mb-4">
        <CardHeader title="Add or update a rate manually" />
        <CardBody>
          <div className="flex flex-wrap items-end gap-3">
            <Field label="From" className="w-24">
              <Input value={form.from_currency} maxLength={3} className="font-mono uppercase"
                onChange={e => setForm(f => ({ ...f, from_currency: e.target.value.toUpperCase() }))} />
            </Field>
            <ArrowRight className="mb-2.5 size-4 text-fg-subtle" aria-hidden />
            <Field label="To" className="w-24">
              <Input value={form.to_currency} maxLength={3} className="font-mono uppercase"
                onChange={e => setForm(f => ({ ...f, to_currency: e.target.value.toUpperCase() }))} />
            </Field>
            <Field label={`Rate (1 ${form.from_currency} = ? ${form.to_currency})`} className="w-52">
              <Input type="number" step="0.000001" numeric value={form.rate} placeholder="16250.00"
                onChange={e => setForm(f => ({ ...f, rate: e.target.value }))} />
            </Field>
            <Field label="Effective date" className="w-44">
              <Input type="date" value={form.effective_date}
                onChange={e => setForm(f => ({ ...f, effective_date: e.target.value }))} />
            </Field>
            <Button variant="primary" onClick={save} disabled={saving || !form.rate} loading={saving}>
              {!saving && <Plus aria-hidden />} {saving ? "Saving…" : "Save rate"}
            </Button>
          </div>

          <div className="mt-3 flex flex-wrap gap-2">
            <span className="self-center text-xs text-fg-muted">Quick fill:</span>
            {quickPairs.map(p => (
              <Button key={`${p.from}${p.to}`} size="sm" className="font-mono"
                onClick={() => setForm(f => ({ ...f, from_currency: p.from, to_currency: p.to }))}>
                {p.from}/{p.to}
              </Button>
            ))}
          </div>

          {error && <p role="alert" className="mt-3 text-sm text-brand-ink">{error}</p>}
        </CardBody>
      </Card>

      {loading ? (
        <Card className="space-y-2 p-4">
          {[0, 1, 2].map(i => <Skeleton key={i} className="h-5 w-full" />)}
        </Card>
      ) : Object.keys(grouped).length === 0 ? (
        <Card>
          <EmptyState icon={RefreshCw} title="No exchange rates yet" description="Click Sync now or add a rate manually." />
        </Card>
      ) : (
        <div className="space-y-4">
          {Object.entries(grouped).map(([pair, rateList]) => (
            <Card key={pair} className="overflow-hidden">
              <CardHeader
                title={<span className="font-mono">{pair}</span>}
                description={`${rateList.length} rate${rateList.length !== 1 ? "s" : ""}`}
                actions={rateList[0] && (
                  <span className="text-sm font-semibold text-fg tabular">
                    Latest: {rateList[0].rate.toLocaleString("en-US", { maximumFractionDigits: 4 })}
                  </span>
                )}
              />
              <DataTable label={`${pair} rates`} columns={columns} data={rateList} getRowId={(r) => r.id} maxHeight="none" />
            </Card>
          ))}
        </div>
      )}
    </div>
  );
}
