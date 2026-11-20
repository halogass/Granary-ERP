"use client";

import { useCallback, useEffect, useState } from "react";
import { Activity, RefreshCw } from "lucide-react";
import { hrApi, type FaceAttemptsSummary } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Card, CardBody, CardHeader, EmptyState, Skeleton, Stat, StatGroup } from "@/components/ui";

const OUTCOME_LABEL: Record<string, string> = {
  done: "Finished", timeout: "Timed out", aborted: "Cancelled",
  passive_inconclusive: "Passive inconclusive", passive_failed: "Passive failed",
};
const MODE_LABEL: Record<string, string> = { kiosk: "Kiosk tablet", personal: "Personal device" };
const STEP_LABEL: Record<string, string> = {
  find_face: "Finding the face", hold_still: "Holding still", do_action: "Doing the action",
  return: "Facing the camera again", done: "Finished",
};
const STEP_ORDER = ["find_face", "hold_still", "do_action", "return", "done"];

const REASON_LABEL: Record<string, string> = {
  flash_failed: "Colours did not match", spoof: "Anti-spoof score too low",
  flash_inconclusive: "Too little colour response", no_face: "No single clear face",
};
const REASON_ORDER = ["flash_failed", "spoof", "flash_inconclusive", "no_face"];

const pct = (n: number) => `${(n * 100).toFixed(1)}%`;
const fixed = (n: number | null | undefined, d: number) => (n == null ? "-" : n.toFixed(d));

function Breakdown({ title, counts, labels, order }: {
  title: string; counts: Record<string, number>; labels: Record<string, string>; order?: string[];
}) {
  const keys = (order ?? Object.keys(counts)).filter((k) => counts[k]);
  const total = keys.reduce((n, k) => n + counts[k], 0);
  return (
    <div className="min-w-0">
      <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-fg-muted">{title}</h3>
      {keys.length === 0 ? <p className="text-[13px] text-fg-subtle">None</p> : (
        <ul className="space-y-1.5">
          {keys.map((k) => (
            <li key={k} className="text-[13px]">
              <div className="flex items-baseline justify-between gap-2">
                <span className="truncate text-fg">{labels[k] ?? k}</span>
                <span className="tabular text-fg-muted">{counts[k]}</span>
              </div>
              <div className="mt-0.5 h-1.5 overflow-hidden rounded-full bg-surface-2" aria-hidden>
                <div className="h-full rounded-full bg-brand" style={{ width: `${total ? (counts[k] / total) * 100 : 0}%` }} />
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** HR > Attendance > Attempts: how face checks ended over the last week (did the timeout fix work?). */
export default function AttemptsPanel({ days = 7 }: { days?: number }) {
  const [data, setData] = useState<FaceAttemptsSummary | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setData((await hrApi.faceAttemptsSummary(days)).data);
    } catch (e) {
      setError(extractApiError(e, "Could not load the attempts."));
    } finally {
      setLoading(false);
    }
  }, [days]);

  useEffect(() => { load(); }, [load]);

  const timeoutsBy = (mode: string) => data?.by_mode_outcome[mode]?.timeout ?? 0;
  const totalBy = (mode: string) => data?.by_mode[mode] ?? 0;

  return (
    <div className="space-y-4 p-4">
      <Card>
        <CardHeader title={`Face check attempts, last ${days} days`}
          description="Every face check (colour flash, head turn) from the kiosk and from personal devices, and how it ended. No photos are kept."
          actions={<Button size="sm" onClick={load} disabled={loading}><RefreshCw aria-hidden /> Refresh</Button>} />
        <CardBody className="space-y-4">
          {error && <p role="alert" className="text-[13px] text-tone-red">{error}</p>}
          {loading && !data ? <Skeleton className="h-24" /> : !data || data.total === 0 ? (
            <EmptyState compact icon={Activity} title="No attempts yet"
              description="Attempts appear here once someone clocks in with the face check." />
          ) : (
            <>
              <StatGroup>
                <Stat label="Attempts" value={data.total} />
                <Stat label="Timeout rate" value={pct(data.timeout_rate)} note={`${data.timeouts} timed out`} alert={data.timeout_rate > 0.1} />
                <Stat label="Kiosk timeouts" value={timeoutsBy("kiosk")} note={`of ${totalBy("kiosk")}`} />
                <Stat label="Personal timeouts" value={timeoutsBy("personal")} note={`of ${totalBy("personal")}`} />
                <Stat label="Mean tick" value={data.avg_tick_ms != null ? `${data.avg_tick_ms} ms` : "-"} note="Time per camera frame" />
              </StatGroup>
              <div className="grid grid-cols-1 gap-6 sm:grid-cols-3">
                <Breakdown title="By outcome" counts={data.by_outcome} labels={OUTCOME_LABEL} />
                <Breakdown title="By device" counts={data.by_mode} labels={MODE_LABEL} />
                <Breakdown title="By step reached" counts={data.by_step} labels={STEP_LABEL} order={STEP_ORDER} />
              </div>
              {data.passive && (data.passive.passed > 0 || data.passive.not_passed > 0) && (
                <div className="space-y-3 border-t border-line pt-4">
                  <h3 className="text-xs font-semibold uppercase tracking-wide text-fg-muted">Passive check (colour flash)</h3>
                  <StatGroup>
                    <Stat label="Passed" value={data.passive.passed} />
                    <Stat label="Fell back" value={data.passive.not_passed} note="to the head turn" />
                    <Stat label="Median colour match" value={fixed(data.passive.median_corr, 2)} note="corr, 0 to 1" />
                    <Stat label="Median colour response" value={fixed(data.passive.median_delta, 2)} note="delta" />
                    <Stat label="Median real-face score" value={fixed(data.passive.median_spoof_real, 2)} note="anti-spoof" />
                  </StatGroup>
                  <div className="grid grid-cols-1 gap-6 sm:grid-cols-3">
                    <Breakdown title="Why it fell back" counts={data.passive.reasons} labels={REASON_LABEL} order={REASON_ORDER} />
                  </div>
                </div>
              )}
            </>
          )}
        </CardBody>
      </Card>
    </div>
  );
}
