"use client";

import type { Budget } from "@/lib/faceLiveness";
import { cn } from "@/lib/utils";

const LABEL: Record<Budget, string> = { find: "Find", action: "Action", return: "Back" };

/**
 * Countdown of the step running now: the ring empties as the step's own budget runs out and refills when the
 * next step begins. Sits on the black camera frame, so it is drawn in white on both themes.
 */
export default function StepRing({ budget, budgetMs, remainingMs, size = 44 }: {
  budget?: Budget; budgetMs?: number; remainingMs?: number; size?: number;
}) {
  if (!budget || !budgetMs || remainingMs === undefined) return null;
  const r = (size - 6) / 2;
  const c = 2 * Math.PI * r;
  const left = Math.max(0, Math.min(1, remainingMs / budgetMs));
  const secs = Math.ceil(remainingMs / 1000);
  return (
    <div className="flex flex-col items-center gap-0.5" role="timer" aria-label={`${LABEL[budget]} step: ${secs} seconds left`}>
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden>
        <circle cx={size / 2} cy={size / 2} r={r} fill="rgba(0,0,0,0.55)" stroke="rgba(255,255,255,0.25)" strokeWidth={3} />
        <circle cx={size / 2} cy={size / 2} r={r} fill="none" strokeWidth={3} strokeLinecap="round"
          className={cn("stroke-white transition-[stroke-dashoffset] duration-200 ease-linear", left < 0.25 && "animate-pulse")}
          strokeDasharray={c} strokeDashoffset={c * (1 - left)} transform={`rotate(-90 ${size / 2} ${size / 2})`} />
        <text x="50%" y="50%" dominantBaseline="central" textAnchor="middle" className="fill-white text-[13px] font-semibold tabular">
          {secs}
        </text>
      </svg>
      <span className="text-[10px] font-medium uppercase tracking-wide text-white/80">{LABEL[budget]}</span>
    </div>
  );
}
