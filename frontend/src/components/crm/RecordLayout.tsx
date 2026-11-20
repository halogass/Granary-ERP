"use client";

import { useCallback, useEffect, useState, type ReactNode } from "react";
import Link from "next/link";
import { ArrowLeft, AlertTriangle } from "lucide-react";
import { crmRecordsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import AttributePanel, { type AttributeDef, type AiFieldRun } from "./AttributePanel";
import RecordTimeline, { type TimelineEntry } from "./RecordTimeline";
import AiBriefPanel from "./AiBriefPanel";
import { Button, Card, CardBody, CardHeader, CountBadge, EmptyState, PageHeader, Skeleton, Stat, StatGroup } from "@/components/ui";

export type ObjectType = "organization" | "contact" | "lead" | "deal";

export interface RecordShell {
  id: string;
  title: string;
  subtitle?: string | null;
  custom?: Record<string, unknown>;
}

/**
 * The Attio-style record page: identity + built-in facts + custom/AI attributes down the
 * left, AI brief and unified timeline on the right.
 *
 * The caller owns loading and saving its own object, and passes built-in fields in as
 * `facts`; everything shared (attribute schema, AI runs, timeline, brief) lives here.
 */
export default function RecordLayout({
  objectType, recordId, record, backHref, backLabel, facts, onPatch, loading, error, children, actions, aside, status,
}: {
  objectType: ObjectType;
  recordId: string;
  record: RecordShell | null;
  backHref: string;
  backLabel: string;
  facts?: ReactNode;
  onPatch: (patch: Record<string, unknown>) => Promise<void>;
  loading?: boolean;
  error?: string;
  children?: ReactNode;
  /** Buttons for the page header, right-aligned (e.g. research on this record). */
  actions?: ReactNode;
  /** Shown at the top of the right column, above the AI brief (e.g. research results). */
  aside?: ReactNode;
  /** A status pill shown after the title (deal stage, lead status). */
  status?: ReactNode;
}) {
  const [defs, setDefs] = useState<AttributeDef[]>([]);
  const [runs, setRuns] = useState<Record<string, AiFieldRun>>({});
  const [timeline, setTimeline] = useState<TimelineEntry[]>([]);
  const [related, setRelated] = useState<{ contacts: number; leads: number; deals: number } | null>(null);
  const [sideError, setSideError] = useState("");

  const loadSchema = useCallback(() => {
    crmRecordsApi.listAttributes({ object_type: objectType })
      .then((r) => setDefs(r.data.items ?? []))
      .catch(() => {});
  }, [objectType]);

  const loadRuns = useCallback(() => {
    crmRecordsApi.aiFields(objectType, recordId)
      .then((r) => setRuns(r.data.runs ?? {}))
      .catch(() => {});
  }, [objectType, recordId]);

  const loadTimeline = useCallback(() => {
    crmRecordsApi.timeline(objectType, recordId)
      .then((r) => { setTimeline(r.data.items ?? []); setRelated(r.data.related ?? null); })
      .catch((e) => setSideError(extractApiError(e, "Could not load the timeline.")));
  }, [objectType, recordId]);

  useEffect(() => { loadSchema(); }, [loadSchema]);
  useEffect(() => { loadRuns(); loadTimeline(); }, [loadRuns, loadTimeline]);

  // An AI run writes into the record's `custom`, so pull both the runs and the record back.
  const afterRun = useCallback(() => { loadRuns(); onPatch({}); }, [loadRuns, onPatch]);

  const saveAttribute = useCallback(
    (key: string, value: unknown) => onPatch({ custom: { [key]: value } }),
    [onPatch],
  );

  if (loading) {
    return (
      <div className="mx-auto max-w-6xl space-y-4" aria-busy>
        <Skeleton className="h-7 w-64" />
        <div className="grid grid-cols-[minmax(0,1fr)] items-start gap-4 lg:grid-cols-[320px_minmax(0,1fr)]">
          <Skeleton className="h-64" />
          <Skeleton className="h-96" />
        </div>
      </div>
    );
  }

  if (error || !record) {
    return (
      <div className="mx-auto max-w-xl">
        <EmptyState icon={AlertTriangle} title={error || "Record not found"}
          action={<Button asChild><Link href={backHref}><ArrowLeft aria-hidden /> Back to {backLabel}</Link></Button>} />
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-6xl">
      <PageHeader title={record.title} summary={record.subtitle || undefined} status={status}
        back={{ href: backHref, label: backLabel }} actions={actions} />

      <div className="grid grid-cols-[minmax(0,1fr)] items-start gap-4 lg:grid-cols-[320px_minmax(0,1fr)]">
        {/* Left: identity, built-in facts, attributes */}
        <div className="space-y-4">
          {facts && (
            <Card>
              <CardHeader title="Details" />
              <CardBody className="py-2">{facts}</CardBody>
            </Card>
          )}

          <Card>
            <CardHeader title="Attributes" />
            <CardBody className="py-2">
              <AttributePanel
                defs={defs}
                values={record.custom ?? {}}
                runs={runs}
                objectType={objectType}
                recordId={recordId}
                onSave={saveAttribute}
                onRan={afterRun}
              />
            </CardBody>
          </Card>

          {related && objectType === "organization" && (
            <Card>
              <CardHeader title="Related" />
              <StatGroup className="rounded-none border-0 border-t-0 sm:grid-cols-3">
                {([["Contacts", related.contacts], ["Leads", related.leads], ["Deals", related.deals]] as const)
                  .map(([label, n]) => <Stat key={label} label={label} value={n} />)}
              </StatGroup>
            </Card>
          )}

          {children}
        </div>

        {/* Right: AI brief + timeline */}
        <div className="space-y-4">
          {aside}
          <AiBriefPanel objectType={objectType} recordId={recordId} />

          <Card>
            <CardHeader title={<span className="inline-flex items-center gap-2">Timeline <CountBadge count={timeline.length} tone="muted" /></span>} />
            <CardBody className="py-2">
              {sideError
                ? <p className="py-6 text-sm text-tone-red">{sideError}</p>
                : <RecordTimeline entries={timeline} />}
            </CardBody>
          </Card>
        </div>
      </div>
    </div>
  );
}
