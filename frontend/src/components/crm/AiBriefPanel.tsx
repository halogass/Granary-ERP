"use client";

import { useState } from "react";
import { Sparkles, RefreshCw, ArrowRight, TrendingUp, AlertTriangle } from "lucide-react";
import { crmRecordsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Card, CardBody, CardHeader, Skeleton } from "@/components/ui";

export interface Brief {
  summary: string;
  next_best_action: string;
  signals: string[];
  risks: string[];
  generated_at?: string;
  model?: string;
  timeline_entries?: number;
}

/**
 * On-demand AI read of the record. Never auto-runs: the brief costs a model call, and the
 * agreed cost policy is manual trigger only.
 */
export default function AiBriefPanel({ objectType, recordId }: {
  objectType: string;
  recordId: string;
}) {
  const [brief, setBrief] = useState<Brief | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  async function generate() {
    setLoading(true); setError("");
    try {
      const res = await crmRecordsApi.brief(objectType, recordId);
      setBrief(res.data);
    } catch (e) {
      setError(extractApiError(e, "Could not generate a brief."));
    } finally { setLoading(false); }
  }

  return (
    <Card>
      <CardHeader
        title={<span className="inline-flex items-center gap-2"><Sparkles className="size-4 text-tone-violet" aria-hidden /> AI brief</span>}
        actions={
          <Button size="sm" variant="ghost" onClick={generate} disabled={loading}>
            <RefreshCw className={loading ? "animate-spin" : ""} aria-hidden />
            {brief ? "Refresh" : "Generate"}
          </Button>
        } />

      <CardBody>
        {error && (
          <p role="alert" className="rounded-lg bg-tone-red-soft px-4 py-3 text-sm text-tone-red">{error}</p>
        )}

        {!brief && !error && !loading && (
          <p className="text-sm text-fg-muted">
            Generate a summary of where this relationship stands and what to do next,
            based on the record and its full interaction history.
          </p>
        )}

        {loading && !brief && (
          <div className="space-y-2">
            <Skeleton className="h-3 w-full" />
            <Skeleton className="h-3 w-5/6" />
            <Skeleton className="h-3 w-2/3" />
          </div>
        )}

        {brief && (
          <div className="space-y-4">
            <p className="whitespace-pre-wrap text-sm leading-relaxed text-fg">{brief.summary}</p>

            {brief.next_best_action && (
              <div className="flex items-start gap-2 rounded-lg border border-brand/20 bg-brand/[0.05] px-4 py-3">
                <ArrowRight className="mt-0.5 size-4 shrink-0 text-brand-ink" aria-hidden />
                <div>
                  <p className="mb-0.5 text-xs font-semibold text-brand-ink">Next best action</p>
                  <p className="text-sm text-fg">{brief.next_best_action}</p>
                </div>
              </div>
            )}

            <div className="grid gap-4 sm:grid-cols-2">
              {brief.signals.length > 0 && (
                <div>
                  <p className="mb-1.5 flex items-center gap-1 text-xs font-semibold text-tone-blue">
                    <TrendingUp className="size-3.5" aria-hidden /> Signals
                  </p>
                  <ul className="list-disc space-y-1 pl-4 text-[13px] text-fg-muted">
                    {brief.signals.map((s, i) => <li key={i}>{s}</li>)}
                  </ul>
                </div>
              )}
              {brief.risks.length > 0 && (
                <div>
                  <p className="mb-1.5 flex items-center gap-1 text-xs font-semibold text-tone-orange">
                    <AlertTriangle className="size-3.5" aria-hidden /> Risks
                  </p>
                  <ul className="list-disc space-y-1 pl-4 text-[13px] text-fg-muted">
                    {brief.risks.map((r, i) => <li key={i}>{r}</li>)}
                  </ul>
                </div>
              )}
            </div>

            <p className="text-xs text-fg-subtle">
              {brief.model} · from {brief.timeline_entries ?? 0} interaction(s)
              {brief.generated_at && ` · ${new Date(brief.generated_at).toLocaleString()}`}
            </p>
          </div>
        )}
      </CardBody>
    </Card>
  );
}
