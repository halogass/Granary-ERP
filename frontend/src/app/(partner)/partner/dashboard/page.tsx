"use client";

import { useState, useEffect } from "react";
import { useSession } from "next-auth/react";
import Link from "next/link";
import { ArrowRight, Plus } from "lucide-react";
import { PieChart, Pie, Cell, Tooltip, ResponsiveContainer } from "recharts";
import { Button, Card, CardBody, CardHeader, EmptyState, PageHeader, Skeleton, Stat, StatGroup } from "@/components/ui";
import ForcePasswordChange from "@/components/portal/ForcePasswordChange";
import { formatCurrency } from "@/lib/currency";

interface DashboardStats {
  deals: { stage: string; count: number }[];
  // Money figures are {currency: amount}: amounts in different currencies are never added together.
  commissions: { ytd: Record<string, number>; pending: Record<string, number> };
  training: { total: number; passed: number };
  leads: { new: number; total: number };
  mdf: { approved: Record<string, number>; claimed: Record<string, number> };
}

/** One figure per currency, e.g. "$1,200.00 · ¥150,000"; a dash when there is nothing. */
function perCurrency(m: Record<string, number> | undefined): string {
  const parts = Object.entries(m ?? {}).filter(([, v]) => v).map(([cur, v]) => formatCurrency(v, cur));
  return parts.length ? parts.join(" · ") : "0";
}
const hasAny = (m: Record<string, number> | undefined) => Object.values(m ?? {}).some((v) => v);

// by_stage is keyed by deal-registration status (pending | approved | rejected | expired | won)
const tone = (t: string) => `rgb(var(--tone-${t}))`;
const STAGE_COLORS: Record<string, string> = {
  pending:       tone("orange"),
  approved:      tone("blue"),
  won:           tone("violet"),
  rejected:      tone("red"),
  expired:       tone("neutral"),
  lead:          tone("blue"),
  proposal:      tone("violet"),
  negotiation:   tone("orange"),
  closed_won:    tone("violet"),
  closed_lost:   tone("neutral"),
};

const STAGE_LABELS: Record<string, string> = {
  pending: "Pending", approved: "Approved", won: "Won", rejected: "Rejected", expired: "Expired",
  lead: "Lead", proposal: "Proposal", negotiation: "Negotiation",
  closed_won: "Won", closed_lost: "Lost",
};

function ProgressRing({ pct, size = 80, stroke = 7 }: { pct: number; size?: number; stroke?: number }) {
  const r = (size - stroke) / 2;
  const circ = 2 * Math.PI * r;
  const offset = circ - (pct / 100) * circ;
  return (
    <svg width={size} height={size} className="-rotate-90" aria-hidden>
      <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="rgb(var(--surface-2))" strokeWidth={stroke} />
      <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="rgb(var(--tone-blue))" strokeWidth={stroke}
        strokeDasharray={circ} strokeDashoffset={offset} strokeLinecap="round" style={{ transition: "stroke-dashoffset 0.6s ease" }} />
    </svg>
  );
}

const QUICK = [
  { label: "View leads", href: "/partner/leads" },
  { label: "Request MDF", href: "/partner/mdf" },
  { label: "Training", href: "/partner/training" },
];

export default function PartnerDashboard() {
  const { data: session } = useSession();
  const [stats, setStats] = useState<DashboardStats | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const token = session?.access_token;
    if (!token) return;

    Promise.all([
      fetch("/api/v1/partner/portal/deals/stats", { headers: { Authorization: `Bearer ${token}` } }).then(r => r.ok ? r.json() : null),
      fetch("/api/v1/partner/portal/commissions/stats", { headers: { Authorization: `Bearer ${token}` } }).then(r => r.ok ? r.json() : null),
      fetch("/api/v1/partner/portal/training/stats", { headers: { Authorization: `Bearer ${token}` } }).then(r => r.ok ? r.json() : null),
      fetch("/api/v1/partner/portal/leads/stats", { headers: { Authorization: `Bearer ${token}` } }).then(r => r.ok ? r.json() : null),
      fetch("/api/v1/partner/portal/mdf/stats", { headers: { Authorization: `Bearer ${token}` } }).then(r => r.ok ? r.json() : null),
    ]).then(([deals, comm, training, leads, mdf]) => {
      setStats({
        deals: deals?.by_stage ?? [],
        commissions: comm ?? { ytd: {}, pending: {} },
        training: training ?? { total: 0, passed: 0 },
        leads: leads ?? { new: 0, total: 0 },
        mdf: mdf ?? { approved: {}, claimed: {} },
      });
    }).catch(() => {}).finally(() => setLoading(false));
  }, [session?.access_token]);

  const trainingPct = stats ? (stats.training.total > 0 ? Math.round((stats.training.passed / stats.training.total) * 100) : 0) : 0;
  const dealPieData = stats?.deals.filter(d => d.count > 0).map(d => ({ name: STAGE_LABELS[d.stage] || d.stage, value: d.count, color: STAGE_COLORS[d.stage] || tone("neutral") })) ?? [];

  return (
    <div className="space-y-4">
      {session?.user?.must_change_password && <ForcePasswordChange />}

      <PageHeader title="Partner dashboard" summary={session?.user?.name ? `Welcome back, ${session.user.name}.` : undefined}
        actions={<>
          {QUICK.map((a) => <Button key={a.href} asChild><Link href={a.href}>{a.label}</Link></Button>)}
          <Button variant="primary" asChild><Link href="/partner/deals"><Plus aria-hidden /> Register deal</Link></Button>
        </>} />

      {loading ? (
        <div className="space-y-4" aria-busy="true" aria-label="Loading dashboard">
          <Skeleton className="h-24 w-full" />
          <Skeleton className="h-56 w-full" />
        </div>
      ) : (
        <>
          <StatGroup>
            <Stat label="Commission this year" value={perCurrency(stats?.commissions.ytd)}
              note={hasAny(stats?.commissions.pending) ? `${perCurrency(stats!.commissions.pending)} pending` : undefined} />
            <Stat label="New leads" value={stats?.leads.new ?? 0} note={`of ${stats?.leads.total ?? 0} assigned`} href="/partner/leads" />
            <Stat label="MDF approved" value={perCurrency(stats?.mdf.approved)} note={`${perCurrency(stats?.mdf.claimed)} claimed`} href="/partner/mdf" />
            <Link href="/partner/training" className="flex items-center gap-4 bg-surface px-4 py-3 transition-colors hover:bg-surface-2">
              <span className="relative shrink-0">
                <ProgressRing pct={trainingPct} size={56} stroke={5} />
                <span className="absolute inset-0 flex items-center justify-center text-xs font-semibold text-fg tabular">{trainingPct}%</span>
              </span>
              <span className="flex min-w-0 flex-col gap-0.5">
                <span className="text-[13px] text-fg-muted">Training</span>
                <span className="text-sm font-semibold text-fg tabular">{stats?.training.passed ?? 0}/{stats?.training.total ?? 0}</span>
                <span className="text-xs text-fg-muted">modules certified</span>
              </span>
            </Link>
          </StatGroup>

          <Card>
            <CardHeader title="Deal pipeline" actions={dealPieData.length > 0 ? (
              <Button size="sm" variant="ghost" asChild><Link href="/partner/deals">All deals <ArrowRight aria-hidden /></Link></Button>
            ) : undefined} />
            {dealPieData.length > 0 ? (
              <CardBody className="flex flex-col items-center gap-6 sm:flex-row">
                <ResponsiveContainer width={180} height={180}>
                  <PieChart>
                    <Pie data={dealPieData} cx="50%" cy="50%" innerRadius={50} outerRadius={80} dataKey="value" paddingAngle={2} stroke="rgb(var(--surface))">
                      {dealPieData.map((d, i) => <Cell key={i} fill={d.color} />)}
                    </Pie>
                    <Tooltip formatter={(v) => [`${v} deal(s)`]}
                      contentStyle={{ background: "rgb(var(--surface-raised))", border: "1px solid rgb(var(--line))", borderRadius: 8, color: "rgb(var(--fg))", fontSize: 13 }} />
                  </PieChart>
                </ResponsiveContainer>
                <ul className="w-full max-w-xs space-y-2">
                  {dealPieData.map(d => (
                    <li key={d.name} className="flex items-center gap-2.5 text-sm">
                      <span className="size-2.5 shrink-0 rounded-full" style={{ backgroundColor: d.color }} aria-hidden />
                      <span className="text-fg-muted">{d.name}</span>
                      <span className="ml-auto font-semibold text-fg tabular">{d.value}</span>
                    </li>
                  ))}
                </ul>
              </CardBody>
            ) : (
              <EmptyState title="No deals registered yet" description="Register a deal to lock in your commission protection."
                action={<Button variant="primary" asChild><Link href="/partner/deals"><Plus aria-hidden /> Register your first deal</Link></Button>} />
            )}
          </Card>
        </>
      )}
    </div>
  );
}
