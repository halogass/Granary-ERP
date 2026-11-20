"use client";

import { useEffect, useState, useCallback } from "react";
import { authApi, financeApi } from "@/lib/api";
import { CalendarDays, ChevronLeft, ChevronRight, Lock, Unlock, Plus } from "lucide-react";
import { Button, Card, IconButton, PageHeader, Skeleton, StatusPill, toast } from "@/components/ui";
import { extractApiError } from "@/lib/apiError";

interface FiscalPeriod {
  id: string;
  year: number;
  month: number;
  name: string;
  start_date: string;
  end_date: string;
  is_locked: boolean;
}

const MONTHS = ["January", "February", "March", "April", "May", "June",
                "July", "August", "September", "October", "November", "December"];

const fmtDate = (iso: string) =>
  new Date(`${iso}T00:00:00`).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

export default function FiscalPeriodsPage() {
  const [year, setYear] = useState(() => new Date().getFullYear());
  const [periods, setPeriods] = useState<FiscalPeriod[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);   // period id being toggled, or "generate"
  // Mirrors the backend: locking and generating need finance:post_journal (or superadmin).
  const [canManage, setCanManage] = useState(false);
  useEffect(() => {
    authApi.me().then((r) => {
      const me = r.data as { is_superadmin?: boolean; permissions?: string[] };
      setCanManage(!!me?.is_superadmin || !!me?.permissions?.includes("finance:post_journal"));
    }).catch(() => setCanManage(false));
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await financeApi.listFiscalPeriods({ year });
      setPeriods(res.data ?? []);
    } catch (err) {
      setPeriods([]);
      toast.error(extractApiError(err, "Failed to load fiscal periods"));
    } finally { setLoading(false); }
  }, [year]);

  useEffect(() => { load(); }, [load]);

  async function toggleLock(p: FiscalPeriod) {
    setBusy(p.id);
    try {
      await financeApi.togglePeriodLock(p.id, !p.is_locked);
      setPeriods((ps) => ps.map((x) => x.id === p.id ? { ...x, is_locked: !p.is_locked } : x));
    } catch (err) {
      toast.error(extractApiError(err, "Failed to change the lock"));
    } finally { setBusy(null); }
  }

  async function generate() {
    setBusy("generate");
    try {
      const res = await financeApi.generateFiscalPeriods(year);
      setPeriods(res.data?.periods ?? []);
      toast.success(`${res.data?.created ?? 0} period(s) created for ${year}`);
    } catch (err) {
      toast.error(extractApiError(err, "Failed to create the periods"));
    } finally { setBusy(null); }
  }

  const byMonth = new Map(periods.map((p) => [p.month, p]));
  const missing = 12 - byMonth.size;

  return (
    <div>
      <PageHeader title="Fiscal periods" summary="A locked period accepts no new or changed journal entries"
        actions={<>
          <div className="flex items-center rounded-lg border border-line-strong bg-surface">
            <IconButton variant="ghost" size="sm" icon={ChevronLeft} label="Previous year" onClick={() => setYear((y) => y - 1)} />
            <input type="number" value={year} aria-label="Year"
              onChange={(e) => { const v = parseInt(e.target.value, 10); if (v > 0 && v < 10000) setYear(v); }}
              className="no-spinner w-16 bg-transparent text-center text-sm font-semibold text-fg tabular focus:outline-none" />
            <IconButton variant="ghost" size="sm" icon={ChevronRight} label="Next year" onClick={() => setYear((y) => y + 1)} />
          </div>
          {canManage && !loading && missing > 0 && (
            <Button variant="primary" onClick={generate} loading={busy === "generate"}>
              <Plus aria-hidden /> {busy === "generate" ? "Creating..." : `Create ${year} periods`}
            </Button>
          )}
        </>} />

      {loading ? (
        <div className="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4">
          {MONTHS.map((m) => <Skeleton key={m} className="h-24" />)}
        </div>
      ) : (
        <>
          {missing > 0 && (
            <div role="status" className="mb-4 rounded-lg border border-line bg-tone-orange-soft p-3 text-sm text-tone-orange">
              {missing === 12 ? `No fiscal periods exist for ${year}.` : `${missing} month(s) of ${year} have no fiscal period.`}
              {" "}Entries dated in a month without a period can&apos;t be locked against later changes.
            </div>
          )}
          <div className="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4">
            {MONTHS.map((label, i) => {
              const p = byMonth.get(i + 1);
              if (!p) {
                return (
                  <div key={label} className="rounded-lg border border-dashed border-line-strong bg-surface p-4">
                    <p className="text-sm font-semibold text-fg-muted">{label} {year}</p>
                    <p className="mt-1 text-xs text-fg-subtle">Not created</p>
                  </div>
                );
              }
              return (
                <Card key={p.id} className="p-4">
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <p className="text-sm font-semibold text-fg">{p.name}</p>
                      <p className="mt-0.5 flex items-center gap-1 text-xs text-fg-muted tabular">
                        <CalendarDays className="size-3" aria-hidden /> {fmtDate(p.start_date)} to {fmtDate(p.end_date)}
                      </p>
                    </div>
                    <StatusPill family="period" status={p.is_locked ? "locked" : "open"}
                      icon={p.is_locked ? Lock : Unlock} label={p.is_locked ? "Locked" : "Open"} />
                  </div>
                  {canManage && (
                    <Button size="sm" className="mt-3 w-full" onClick={() => toggleLock(p)} loading={busy === p.id}>
                      {p.is_locked ? <><Unlock aria-hidden /> Unlock</> : <><Lock aria-hidden /> Lock</>}
                    </Button>
                  )}
                </Card>
              );
            })}
          </div>
        </>
      )}
    </div>
  );
}
