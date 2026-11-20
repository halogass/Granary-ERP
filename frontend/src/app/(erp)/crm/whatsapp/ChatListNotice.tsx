"use client";

import { AlertTriangle, Clock } from "lucide-react";
import { Button } from "@/components/ui";
import type { ChatListProblem } from "./messages";

/** fix-walist: what the inbox says when WhatsApp is slow: an older list on show, or no list and Retry. */
export default function ChatListNotice({ problem, onRetry, busy }: {
  problem: ChatListProblem | null; onRetry: () => void; busy?: boolean;
}) {
  if (!problem) return null;
  if (problem.kind === "stale") {
    return (
      <p role="status" className="flex items-center gap-1.5 border-b border-line-soft px-4 py-1.5 text-[11px] text-fg-muted">
        <Clock className="h-3.5 w-3.5 shrink-0" aria-hidden /> {problem.text}
      </p>
    );
  }
  return (
    <div role="alert" className="flex items-center gap-2 border-b border-line-soft px-4 py-3 text-xs text-fg">
      <AlertTriangle className="h-4 w-4 shrink-0 text-tone-orange" aria-hidden />
      <span className="flex-1">{problem.text}</span>
      <Button size="sm" variant="ghost" onClick={onRetry} disabled={busy}>Retry</Button>
    </div>
  );
}
