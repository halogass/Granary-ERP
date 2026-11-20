"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { Building2, UserCircle2, Calendar, Percent, Coins, Plus, AlertCircle, Mail } from "lucide-react";
import { crmApi, crmInboxApi, type InboxMailbox } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { formatCurrency } from "@/lib/currency";
import { Button, Card, CardHeader, DataTable, Pill, Select, StatusPill, type ColumnDef } from "@/components/ui";
import RecordLayout, { type RecordShell } from "@/components/crm/RecordLayout";
import Fact from "@/components/crm/Fact";
import { useCrmStages } from "@/hooks/useCrmStages";
import { stageStyle } from "@/lib/crmStages";
import RecordNaryPanel from "@/components/nary/RecordNaryPanel";
import DealNaryInsights from "@/components/nary/DealNaryInsights";
import SegmentFact from "@/components/crm/SegmentFact";
import { segmentOf, type WithSegment } from "@/lib/segments";

interface DealQuotation {
  id: string;
  number?: string | null;
  status: string;
  total?: number | null;
  currency?: string | null;
  issue_date?: string | null;
}

interface Deal extends RecordShell, WithSegment {
  deal_number?: string | null;
  stage: string;
  value?: number | null;
  currency?: string | null;
  probability?: number | null;
  expected_close_date?: string | null;
  organization_id?: string | null;
  organization_name?: string | null;
  contact_id?: string | null;
  contact_name?: string | null;
  stage_entered_at?: string | null;
  needs_next_action?: boolean;
  loss_reasons?: string[];
  quotations?: DealQuotation[];
  /** The CRM inbox the deal's email goes from; null is the default inbox. */
  mailbox_id?: string | null;
}

/** Which CRM inbox the deal's email goes from (only shown when there is more than one). */
function EmailFromFact({ mailboxes, value, onChange }: {
  mailboxes: InboxMailbox[]; value: string | null; onChange: (id: string | null) => Promise<void>;
}) {
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const active = mailboxes.filter((m) => m.active || m.id === value);
  if (mailboxes.filter((m) => m.active).length < 2) return null;
  const def = mailboxes.find((m) => m.default);
  return (
    <div className="flex items-start gap-2.5 py-1.5">
      <Mail className="mt-0.5 size-3.5 shrink-0 text-fg-subtle" aria-hidden />
      <div className="min-w-0 flex-1">
        <p className="text-xs text-fg-muted">Email from</p>
        <Select aria-label="Email from" value={value ?? ""} disabled={saving} className="h-8 text-[13px] md:h-8"
          onChange={async (e) => {
            setSaving(true); setError("");
            try { await onChange(e.target.value || null); }
            catch (err) { setError(extractApiError(err, "Could not change the inbox.")); }
            finally { setSaving(false); }
          }}
          options={[
            { value: "", label: def ? `Default inbox (${def.address})` : "Default inbox" },
            ...active.map((m) => ({ value: m.id, label: m.address })),
          ]} />
        {error && <p role="alert" className="mt-1 text-xs text-tone-red">{error}</p>}
      </div>
    </div>
  );
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

const QUOTATION_COLUMNS: ColumnDef<DealQuotation>[] = [
  { accessorKey: "number", header: "Quotation", cell: (c) => <span className="font-mono text-[13px]">{c.getValue<string>() || "-"}</span> },
  { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="quotation" status={c.getValue<string>()} /> },
  { accessorKey: "total", header: "Total", meta: { numeric: true },
    cell: (c) => c.row.original.total != null ? formatCurrency(c.row.original.total, c.row.original.currency || "USD") : "-" },
];

export default function DealRecordPage({ params }: { params: { id: string } }) {
  const id = params.id;
  const [deal, setDeal] = useState<Deal | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const { reg } = useCrmStages();
  const [mailboxes, setMailboxes] = useState<InboxMailbox[]>([]);
  useEffect(() => {
    crmInboxApi.mailboxes().then((r) => setMailboxes(r.data?.mailboxes ?? [])).catch(() => { /* one inbox */ });
  }, []);

  const load = useCallback(async () => {
    try {
      const r = await crmApi.getDeal(id);
      const d = r.data;
      setDeal({
        ...d, id: d.id, title: d.title,
        subtitle: d.deal_number || null,
        custom: d.custom ?? {},
      });
    } catch (e) {
      setError(extractApiError(e, "Deal not found."));
    } finally { setLoading(false); }
  }, [id]);

  useEffect(() => { load(); }, [load]);

  const onPatch = useCallback(async (patch: Record<string, unknown>) => {
    if (Object.keys(patch).length > 0) await crmApi.updateDeal(id, patch);
    await load();
  }, [id, load]);

  return (
    <RecordLayout
      objectType="deal" recordId={id} record={deal}
      backHref="/crm/deals" backLabel="Deals"
      loading={loading} error={error} onPatch={onPatch}
      status={deal && (
        <span className="inline-flex items-center gap-2">
          <StatusPill family="deal" status={deal.stage} label={stageStyle(reg, deal.stage).label} />
          {deal.stage_entered_at && (
            <span className="text-[13px] text-fg-muted tabular">since {new Date(deal.stage_entered_at).toLocaleDateString("en-GB", { day: "numeric", month: "short" })}</span>
          )}
        </span>
      )}
      facts={deal && (
        <>
          {deal.needs_next_action && (
            <Pill tone="orange" icon={AlertCircle} className="mb-2">No next action planned</Pill>
          )}
          {(deal.loss_reasons ?? []).length > 0 && (
            <Fact icon={AlertCircle} label="Reasons" value={(deal.loss_reasons ?? []).map((k) =>
              [...reg.loss_reasons, ...reg.disqualify_reasons].find((r) => r.key === k)?.label ?? k).join(", ")} />
          )}
          {deal.value != null && (
            <Fact icon={Coins} label="Value" value={formatCurrency(deal.value, deal.currency || "USD")} />
          )}
          <Fact icon={Percent} label="Probability" value={deal.probability != null ? `${deal.probability}%` : null} />
          <Fact icon={Calendar} label="Expected close" value={deal.expected_close_date} />
          <SegmentFact segment={segmentOf(deal)} onChange={onPatch} />
          <EmailFromFact mailboxes={mailboxes} value={deal.mailbox_id ?? null} onChange={(mailbox_id) => onPatch({ mailbox_id })} />
          {deal.organization_id && deal.organization_name && (
            <LinkFact icon={Building2} label="Organization" href={`/crm/organizations/${deal.organization_id}`}>{deal.organization_name}</LinkFact>
          )}
          {deal.contact_id && deal.contact_name && (
            <LinkFact icon={UserCircle2} label="Contact" href={`/crm/contacts/${deal.contact_id}`}>{deal.contact_name}</LinkFact>
          )}
        </>
      )}
    >
      <RecordNaryPanel entityType="deal" entityId={id} />
      <DealNaryInsights dealId={id} />
      <Card>
        <CardHeader title="Quotations"
          actions={<>
            <Button size="sm" variant="ghost" asChild>
              <Link href={`/sales/proposals?deal_id=${id}`}><Plus aria-hidden /> Solution proposal</Link>
            </Button>
            <Button size="sm" asChild>
              <Link href={`/sales/quotations/new?deal_id=${id}`}><Plus aria-hidden /> New quotation</Link>
            </Button>
          </>} />
        <DataTable label="Quotations for this deal" columns={QUOTATION_COLUMNS} data={deal?.quotations ?? []}
          getRowId={(q) => q.id} rowHref={(q) => `/sales/quotations/${q.id}`} maxHeight="none" className="rounded-b-lg"
          empty={{ title: "No quotations for this deal yet", description: "Create one from New quotation.", compact: true }} />
      </Card>
    </RecordLayout>
  );
}
