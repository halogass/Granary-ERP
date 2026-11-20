"use client";

import { useState, useEffect } from "react";
import { useSession } from "next-auth/react";
import Link from "next/link";
import { ArrowRight, Award, BookOpen, GraduationCap } from "lucide-react";
import { Card, EmptyState, FilterChips, ListFrame, PageHeader, Pill, Skeleton, toast, type Tone } from "@/components/ui";
import { cn } from "@/lib/utils";

interface TrainingModule {
  id: string;
  title: string;
  description: string;
  thumbnail_url: string | null;
  min_tier: string;
  is_published: boolean;
  pass_score: number;
  chapter_count: number;
  progress?: {
    status: string;
    current_chapter: number;
    score: number | null;
  };
  certification?: {
    issued_at: string;
    badge_slug: string;
  };
}

const TIER: Record<string, { label: string; tone: Tone }> = {
  authorized: { label: "Authorized", tone: "neutral" },
  silver:     { label: "Silver",     tone: "neutral" },
  gold:       { label: "Gold",       tone: "orange" },
  platinum:   { label: "Platinum",   tone: "violet" },
};

const PROGRESS: Record<string, { label: string; tone: Tone }> = {
  not_started: { label: "Not started", tone: "neutral" },
  in_progress: { label: "In progress", tone: "blue" },
  passed:      { label: "Passed",      tone: "blue" },
  failed:      { label: "Failed",      tone: "red" },
};

function ProgressRing({ pct, size = 44, stroke = 4 }: { pct: number; size?: number; stroke?: number }) {
  const r = (size - stroke) / 2;
  const circ = 2 * Math.PI * r;
  const offset = circ - (pct / 100) * circ;
  return (
    <svg width={size} height={size} className="-rotate-90" aria-hidden>
      <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="rgb(var(--surface-2))" strokeWidth={stroke} />
      <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="rgb(var(--brand))" strokeWidth={stroke}
        strokeDasharray={circ} strokeDashoffset={offset} strokeLinecap="round" style={{ transition: "stroke-dashoffset 0.5s ease" }} />
    </svg>
  );
}

const FILTERS = [{ id: "in_progress", label: "In progress" }, { id: "passed", label: "Completed" }] as const;

export default function PartnerTrainingPage() {
  const { data: session } = useSession();
  const [modules, setModules] = useState<TrainingModule[]>([]);
  const [certifications, setCertifications] = useState<{ module_id: string; badge_slug: string; issued_at: string }[]>([]);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<"all" | "in_progress" | "passed">("all");

  useEffect(() => {
    const token = session?.access_token;
    if (!token) return;
    Promise.all([
      fetch("/api/v1/partner/portal/training", { headers: { Authorization: `Bearer ${token}` } }).then(r => r.json()),
      fetch("/api/v1/partner/portal/training/certifications", { headers: { Authorization: `Bearer ${token}` } }).then(r => r.json()),
    ])
      .then(([mods, certs]) => { setModules(mods); setCertifications(certs); })
      .catch(() => toast.error("Failed to load training data"))
      .finally(() => setLoading(false));
  }, [session?.access_token]);

  const certMap = Object.fromEntries(certifications.map(c => [c.module_id, c]));

  const filtered = modules.filter(m => {
    if (filter === "all") return true;
    if (filter === "in_progress") return m.progress?.status === "in_progress";
    if (filter === "passed") return m.progress?.status === "passed";
    return true;
  });

  const totalPassed = modules.filter(m => m.progress?.status === "passed").length;
  const completionPct = modules.length > 0 ? Math.round((totalPassed / modules.length) * 100) : 0;

  return (
    <div className="space-y-4">
      <PageHeader title="Training and certification" summary="Complete modules, pass quizzes, and earn certification badges."
        actions={
          <div className="flex items-center gap-3 rounded-lg border border-line bg-surface px-3 py-2">
            <ProgressRing pct={completionPct} size={40} stroke={4} />
            <div>
              <p className="text-base font-semibold leading-tight text-fg tabular">{completionPct}%</p>
              <p className="text-xs text-fg-muted tabular">{totalPassed}/{modules.length} complete</p>
            </div>
          </div>
        } />

      <ListFrame>
        <FilterChips className="border-b-0"
          chips={FILTERS.map((f) => ({ id: f.id, label: f.label, active: filter === f.id }))}
          onToggle={(id) => setFilter((cur) => (cur === id ? "all" : (id as "in_progress" | "passed")))}
          onClear={() => setFilter("all")} />
      </ListFrame>

      {loading ? (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3" aria-busy="true">
          {[0, 1, 2].map((i) => <Skeleton key={i} className="h-64" />)}
        </div>
      ) : filtered.length === 0 ? (
        <Card><EmptyState icon={GraduationCap} title="No modules found" description="Pick another filter, or check back when new modules are published." /></Card>
      ) : (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {filtered.map(mod => {
            const status = mod.progress?.status ?? "not_started";
            const progress = PROGRESS[status] ?? PROGRESS.not_started;
            const cert = certMap[mod.id];
            const tier = TIER[mod.min_tier] ?? TIER.authorized;
            const chaptersDone = status === "passed" ? mod.chapter_count : (mod.progress?.current_chapter ?? 0);
            const pct = mod.chapter_count > 0 ? Math.round((chaptersDone / mod.chapter_count) * 100) : 0;

            return (
              <Link key={mod.id} href={`/partner/training/${mod.id}`}
                className="group flex flex-col overflow-hidden rounded-lg border border-line bg-surface shadow-card transition-colors hover:border-line-strong focus-visible:outline focus-visible:outline-2 focus-visible:outline-[rgb(var(--focus)/0.7)]">
                <div className="relative flex h-32 items-center justify-center overflow-hidden bg-surface-2">
                  {mod.thumbnail_url ? (
                    // eslint-disable-next-line @next/next/no-img-element
                    <img src={mod.thumbnail_url} alt="" className="size-full object-cover" />
                  ) : (
                    <BookOpen className="size-10 text-fg-subtle" aria-hidden />
                  )}
                  {cert && <Pill tone="blue" icon={Award} className="absolute right-3 top-3">Certified</Pill>}
                  <Pill tone={tier.tone} className="absolute bottom-3 left-3">{tier.label}+</Pill>
                </div>

                <div className="flex flex-1 flex-col p-4">
                  <h3 className="line-clamp-2 text-sm font-semibold leading-snug text-fg">{mod.title}</h3>
                  <p className="mt-1 line-clamp-2 text-[13px] text-fg-muted">{mod.description}</p>

                  <div className="mt-auto pt-3">
                    <div className="flex items-center justify-between">
                      <Pill tone={progress.tone}>{progress.label}</Pill>
                      <span className="text-xs text-fg-muted tabular">{mod.chapter_count} chapter{mod.chapter_count !== 1 ? "s" : ""}</span>
                    </div>
                    <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-2" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} aria-label="Chapters done">
                      <div className={cn("h-full rounded-full transition-all", status === "failed" ? "bg-tone-red" : "bg-tone-blue")} style={{ width: `${pct}%` }} />
                    </div>
                    <div className="mt-3 flex items-center justify-between">
                      <span className="text-xs text-fg-muted tabular">Pass: {mod.pass_score}%{mod.progress?.score != null ? ` · Got: ${mod.progress.score}%` : ""}</span>
                      <span className="inline-flex items-center gap-1 text-xs font-medium text-brand-ink group-hover:underline">
                        {status === "not_started" ? "Start" : status === "passed" ? "Review" : "Continue"} <ArrowRight className="size-3.5" aria-hidden />
                      </span>
                    </div>
                  </div>
                </div>
              </Link>
            );
          })}
        </div>
      )}
    </div>
  );
}
