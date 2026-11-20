"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Building2, UserCircle2, Coins, Tag, Signal, ArrowUpRight, AlertCircle } from "lucide-react";
import { crmApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { formatCurrency } from "@/lib/currency";
import { Button, Card, CardBody, CardHeader, Pill, StatusPill } from "@/components/ui";
import RecordLayout, { type RecordShell } from "@/components/crm/RecordLayout";
import Fact from "@/components/crm/Fact";
import FollowUpButton from "@/components/crm/FollowUpButton";
import { useCrmStages } from "@/hooks/useCrmStages";
import { stageStyle } from "@/lib/crmStages";
import RecordNaryPanel from "@/components/nary/RecordNaryPanel";
import DealNaryInsights from "@/components/nary/DealNaryInsights";
import SegmentFact from "@/components/crm/SegmentFact";
import { segmentOf, type WithSegment } from "@/lib/segments";

interface AiScore {
  score?: number;
  temperature?: string;
  rationale?: string;
  recommended_next_action?: string;
}

interface Lead extends RecordShell, WithSegment {
  status: string;
  priority?: string | null;
  source?: string | null;
  source_detail?: string | null;
  estimated_value?: number | null;
  currency?: string | null;
  organization_id?: string | null;
  organization_name?: string | null;
  contact_id?: string | null;
  contact_name?: string | null;
  ai_score?: AiScore | null;
  converted_to_deal_id?: string | null;
  disqualification_reason?: string | null;
  needs_next_action?: boolean;
}

function LinkFact({ icon: Icon, label, href, children }: { icon: typeof Building2; label: string; href: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start gap-2.5 py-1.5">
      <Icon className="mt-0.5 size-3.5 shrink-0 text-fg-subtle" aria-hidden />
      <div className="min-w-0 flex-1">
        <p className="text-xs text-fg-muted">{label}</p>
        <Link href={href} className="block truncate text-sm text-brand-ink hover:underline">{children}</Link>
      </div>
    </div>
  );
}

export default function LeadRecordPage({ params }: { params: { id: string } }) {
  const id = params.id;
  const [lead, setLead] = useState<Lead | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const { reg } = useCrmStages();
  const router = useRouter();
  const [promoting, setPromoting] = useState(false);
  const [promoteError, setPromoteError] = useState("");

  async function promote() {
    setPromoting(true);
    setPromoteError("");
    try {
      const r = await crmApi.promoteLead(id);
      router.push(`/crm/deals/${r.data.deal_id}`);
    } catch (e) {
      setPromoteError(extractApiError(e, "Could not promote this lead."));
    } finally { setPromoting(false); }
  }

  const load = useCallback(async () => {
    try {
      const r = await crmApi.getLead(id);
      const d = r.data;
      setLead({
        ...d, id: d.id, title: d.title || "(untitled lead)",
        subtitle: null,
        custom: d.custom ?? {},
      });
    } catch (e) {
      setError(extractApiError(e, "Lead not found."));
    } finally { setLoading(false); }
  }, [id]);

  useEffect(() => { load(); }, [load]);

  const onPatch = useCallback(async (patch: Record<string, unknown>) => {
    if (Object.keys(patch).length > 0) await crmApi.updateLead(id, patch);
    await load();
  }, [id, load]);

  const score = lead?.ai_score;

  return (
    <RecordLayout
      objectType="lead" recordId={id} record={lead}
      backHref="/crm/leads" backLabel="Leads"
      loading={loading} error={error} onPatch={onPatch}
      status={lead && <StatusPill family="lead" status={lead.status} label={stageStyle(reg, lead.status).label} />}
      actions={lead && (
        <>
          <FollowUpButton compact entityType="lead" entityId={id} subject={lead.title || "this lead"} />
          {lead.converted_to_deal_id ? (
            <Button asChild>
              <Link href={`/crm/deals/${lead.converted_to_deal_id}`}>Open deal <ArrowUpRight aria-hidden /></Link>
            </Button>
          ) : !["promoted", "disqualified"].includes(lead.status) && (
            <Button variant="primary" onClick={promote} loading={promoting} title="Create a deal from this lead">
              {!promoting && <ArrowUpRight aria-hidden />} Promote to deal
            </Button>
          )}
        </>
      )}
      facts={lead && (
        <>
          {promoteError && <p role="alert" className="mb-2 text-xs text-tone-red">{promoteError}</p>}
          {lead.needs_next_action && (
            <Pill tone="orange" icon={AlertCircle} className="mb-2">No next action planned</Pill>
          )}
          {lead.disqualification_reason && (
            <Fact icon={AlertCircle} label="Disqualified because"
              value={reg.disqualify_reasons.find((r) => r.key === lead.disqualification_reason)?.label ?? lead.disqualification_reason} />
          )}
          {lead.estimated_value != null && (
            <Fact icon={Coins} label="Estimated value"
              value={formatCurrency(lead.estimated_value, lead.currency || "USD")} />
          )}
          <Fact icon={Signal} label="Priority" value={lead.priority} />
          <Fact icon={Tag} label="Source"
            value={[reg.lead_sources.find((s) => s.key === lead.source)?.label ?? lead.source?.replace(/_/g, " "),
                    lead.source_detail].filter(Boolean).join(" · ")} />
          <SegmentFact segment={segmentOf(lead)} onChange={onPatch} />
          {lead.organization_id && lead.organization_name && (
            <LinkFact icon={Building2} label="Organization" href={`/crm/organizations/${lead.organization_id}`}>{lead.organization_name}</LinkFact>
          )}
          {lead.contact_id && lead.contact_name && (
            <LinkFact icon={UserCircle2} label="Contact" href={`/crm/contacts/${lead.contact_id}`}>{lead.contact_name}</LinkFact>
          )}
        </>
      )}
    >
      <RecordNaryPanel entityType="lead" entityId={id} />
      <DealNaryInsights leadId={id} />
      {/* The existing on-demand lead score keeps its own card, beside the newer AI brief. */}
      {score?.temperature && (
        <Card>
          <CardHeader title="Lead score" />
          <CardBody>
            <div className="mb-2 flex items-center gap-2">
              <StatusPill family="temperature" status={score.temperature} />
              {score.score != null && (
                <span className="text-xl font-semibold text-fg tabular">{score.score}</span>
              )}
            </div>
            {score.rationale && <p className="text-[13px] leading-relaxed text-fg-muted">{score.rationale}</p>}
            {score.recommended_next_action && (
              <p className="mt-2 text-[13px] text-fg">
                <span className="font-semibold">Next:</span> {score.recommended_next_action}
              </p>
            )}
          </CardBody>
        </Card>
      )}
    </RecordLayout>
  );
}
