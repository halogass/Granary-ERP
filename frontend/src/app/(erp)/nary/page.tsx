"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { AlertTriangle, Pause, Play, Send, ShieldCheck } from "lucide-react";
import { naryApi, peopleApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import NaryTabs from "@/components/nary/NaryTabs";
import OwnerTopics from "@/components/nary/OwnerTopics";
import { Button, Card, CardBody, CardHeader, Select, Skeleton, Stat, StatGroup, StatusPill } from "@/components/ui";
import { stack } from "@/lib/stackTable";
import { cn } from "@/lib/utils";

interface Status {
  paused: boolean;
  shadow_mode: boolean;
  customer_sending_enabled: boolean;
  working_time: boolean | null;
  brief_times: Record<string, string>;
  budget: { cap?: number; spent?: number; pct?: number; level?: string; error?: string };
  pending_cards: number;
  shadow_drafts: number;
  runs_24h: number;
  failed_runs_24h: number;
  events_queued: number;
  /** Events Nary stopped retrying (each raised a "couldn't process" card); dismissed once their cause is fixed. */
  events_given_up?: number;
}
interface RoleRow { sop_role: string; is_nary: boolean; user_id: string | null; user_name: string | null }

const BUDGET_LABEL: Record<string, string> = {
  ok: "Within budget", warn: "Nearly spent", stopped: "Spent: essential jobs only", hard_stopped: "Hard stop",
};

export default function NaryOverviewPage() {
  const { can } = usePermissions();
  const manage = can("nary:manage");
  const [status, setStatus] = useState<Status | null>(null);
  const [roles, setRoles] = useState<RoleRow[]>([]);
  const [users, setUsers] = useState<{ id: string; name: string }[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [s, r] = await Promise.all([naryApi.status(), naryApi.roles()]);
      setStatus(s.data);
      setRoles(r.data);
    } catch (e) { setNotice(extractApiError(e)); }
  }, []);
  useEffect(() => { load(); }, [load]);
  useEffect(() => {
    if (!manage) return;
    // The staff directory (any internal user may read it), not the admin users list (403 for staff).
    peopleApi.directory().then((r) => {
      setUsers((r.data ?? []).map((u) => ({ id: String(u.id), name: u.name })));
    }).catch(() => setUsers([]));
  }, [manage]);

  const act = async (key: string, fn: () => Promise<unknown>, done?: string) => {
    setBusy(key); setNotice(null);
    try { await fn(); if (done) setNotice(done); await load(); }
    catch (e) { setNotice(extractApiError(e)); }
    finally { setBusy(null); }
  };

  return (
    <div className="max-w-5xl">
      <NaryTabs subtitle="The company's AI sales and operations assistant. It drafts; you approve." />
      {notice && <p role="status" className="mb-4 rounded-lg border border-line bg-surface-2 px-4 py-2 text-sm text-fg">{notice}</p>}
      {!status ? (
        <div className="space-y-4"><Skeleton className="h-20 w-full" /><Skeleton className="h-40 w-full" /></div>
      ) : (
        <div className="space-y-4">
          <StatGroup>
            <Stat label="Waiting for you" value={status.pending_cards} href="/nary/approvals" alert={status.pending_cards > 0}
              note={status.pending_cards > 0 ? "Needs your decision" : undefined} />
            <Stat label="Shadow drafts" value={status.shadow_drafts} href="/nary/approvals?status=shadow" />
            <Stat label="Runs (24 h)" value={status.runs_24h} href="/nary/runs" />
            <Stat label="Failed runs (24 h)" value={status.failed_runs_24h} href="/nary/runs?status=failed" alert={status.failed_runs_24h > 0} />
          </StatGroup>

          <OwnerTopics />

          {!!status.events_given_up && (
            <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-4 py-3">
              <p className="inline-flex items-start gap-2 text-sm text-fg">
                <AlertTriangle className="mt-0.5 size-4 shrink-0 text-tone-orange" aria-hidden />
                <span>
                  Nary gave up on {status.events_given_up} event{status.events_given_up === 1 ? "" : "s"} after repeated
                  errors (see the &ldquo;couldn&apos;t process&rdquo; cards for why). Once the cause is fixed, dismiss them so
                  the health check stops counting them.
                </span>
              </p>
              {manage && (
                <Button size="sm" loading={busy === "dismiss"}
                  onClick={() => act("dismiss", () => naryApi.dismissGivenUp(), "Dismissed.")}>
                  Dismiss
                </Button>
              )}
            </div>
          )}

          <div className="grid gap-4 md:grid-cols-2">
            <Card>
              <CardHeader title="Switches" />
              <CardBody className="space-y-4">
                <div className="flex items-center justify-between gap-3">
                  <div>
                    <p className="inline-flex items-center gap-2 text-sm text-fg">
                      <StatusPill status={status.paused ? "paused" : "running"} label={status.paused ? "Paused" : "Running"} />
                    </p>
                    <p className="mt-1 text-xs text-fg-muted">Paused: no runs, no actions; events wait. Briefs still arrive.</p>
                  </div>
                  {manage && (
                    <Button variant={status.paused ? "primary" : "secondary"} loading={busy === "pause"}
                      onClick={() => act("pause", () => naryApi.pause(!status.paused))}>
                      {busy !== "pause" && (status.paused ? <Play aria-hidden /> : <Pause aria-hidden />)} {status.paused ? "Resume" : "Pause"}
                    </Button>
                  )}
                </div>
                <div className="flex items-center justify-between gap-3">
                  <div>
                    <p className="text-sm text-fg">Shadow mode {status.shadow_mode ? "on" : "off"}</p>
                    <p className="text-xs text-fg-muted">Customer-facing drafts are stored and compared with what you actually send.</p>
                  </div>
                  {manage && (
                    <Button loading={busy === "shadow"} onClick={() => act("shadow", () => naryApi.shadow(!status.shadow_mode))}>
                      Turn {status.shadow_mode ? "off" : "on"}
                    </Button>
                  )}
                </div>
                {!status.customer_sending_enabled && (
                  <p className="inline-flex items-center gap-1.5 text-xs text-fg-muted">
                    <ShieldCheck className="size-4 text-tone-blue" aria-hidden /> Nothing is sent to customers in this phase, whatever the switches say.
                  </p>
                )}
              </CardBody>
            </Card>

            <Card>
              <CardHeader title="Rhythm and budget" />
              <CardBody className="space-y-3">
                <p className="text-sm text-fg">
                  {status.working_time == null ? "Working hours not set" : status.working_time ? "Inside working hours" : "Outside working hours: urgent items wait for the morning"}
                </p>
                <p className="text-xs text-fg-muted">
                  Briefs: <span className="tabular">{Object.entries(status.brief_times).map(([k, v]) => `${k} ${v}`).join(" · ") || "none"}</span> ·{" "}
                  <Link href="/settings?tab=general" className="text-brand-ink hover:underline">change in Settings</Link>
                </p>
                {status.budget.error ? (
                  <p className="inline-flex items-center gap-1 text-xs text-tone-red"><AlertTriangle className="size-3.5" aria-hidden />{status.budget.error}</p>
                ) : (
                  <p className="text-sm text-fg">
                    AI spend <span className="tabular">${status.budget.spent?.toFixed(2)}</span> of <span className="tabular">${status.budget.cap?.toFixed(2)}</span>{" "}
                    <StatusPill status={status.budget.level ?? "ok"} label={BUDGET_LABEL[status.budget.level ?? "ok"]}
                      tone={status.budget.level === "hard_stopped" || status.budget.level === "stopped" ? "red" : status.budget.level === "warn" ? "orange" : "blue"} />
                  </p>
                )}
                {manage && (
                  <div className="flex flex-wrap gap-2 pt-1">
                    {(["morning", "wrapup"] as const).map((slot) => (
                      <Button key={slot} size="sm" disabled={!!busy && busy !== slot} loading={busy === slot}
                        onClick={() => act(slot, () => naryApi.sendBrief(slot), "Sent: check the bell, WhatsApp and email.")}>
                        {busy !== slot && <Send aria-hidden />}
                        Send {slot === "morning" ? "morning brief" : "wrap-up"} now
                      </Button>
                    ))}
                  </div>
                )}
              </CardBody>
            </Card>
          </div>

          <Card>
            <CardHeader title="SOP roles" description="Who does each SOP role. Nary's cards go to whoever holds Owner." />
            <table className={cn("w-full text-sm", stack.table)}>
              <tbody className={stack.tbody}>
                {roles.map((r) => (
                  <tr key={r.sop_role} className={cn("border-b border-line-soft last:border-0 even:bg-surface-2/60", stack.tr)}>
                    <td className={cn("w-48 px-4 py-2 font-medium text-fg", stack.title)}>{r.sop_role}</td>
                    <td data-label="Held by" className={cn("px-4 py-1.5", manage ? stack.field : stack.td)}>
                      {manage ? (
                        <Select value={r.is_nary ? "__nary" : r.user_id ?? ""} aria-label={`Who holds ${r.sop_role}`} className="md:max-w-xs"
                          onChange={(e) => act(r.sop_role, () => naryApi.setRole(r.sop_role,
                            e.target.value === "__nary" ? { is_nary: true } : { is_nary: false, user_id: e.target.value }))}>
                          {!r.is_nary && !r.user_id && <option value="">Nobody yet</option>}
                          <option value="__nary">Nary</option>
                          {users.map((u) => <option key={u.id} value={u.id}>{u.name}</option>)}
                        </Select>
                      ) : (
                        <span className="text-fg-muted">{r.is_nary ? "Nary" : r.user_name ?? "Nobody yet"}</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Card>
        </div>
      )}
    </div>
  );
}
