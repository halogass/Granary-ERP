"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import { portalPartnerApi } from "@/lib/api";
import { FileText, Plus } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, Input, KeyValues, ListFrame, PageHeader, Select, Stat, StatGroup,
  StatusPill, type ColumnDef,
} from "@/components/ui";
import { formatCurrency, sumByCurrency } from "@/lib/currency";

// The shape GET /partner/portal/deals returns (routers/partner/portal.rs portal_list_deals).
interface DealReg {
  id: string;
  title: string;
  customer_org_name: string | null;
  estimated_value: number | null;
  currency: string;
  expected_close_date?: string;
  status: string;
  /** Why the registration was turned down (only on the partner's own rejected registrations). */
  rejection_reason?: string | null;
  created_at: string;
}

const STATUS_LABEL: Record<string, string> = {
  pending: "Pending review", approved: "Approved", rejected: "Rejected", expired: "Expired",
  extended: "Extended", closed_won: "Won", closed_lost: "Lost",
};

const fmtDate = (d?: string) => d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-";
const EMPTY = { opportunity_name: "", end_customer_name: "", estimated_value: "", currency: "USD", expected_close_date: "" };

export default function PartnerDealsPage() {
  const [deals, setDeals] = useState<DealReg[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await portalPartnerApi.listMyDeals();
      setDeals(res.data.items ?? res.data ?? []);
    } catch { setDeals([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function create() {
    if (!form.opportunity_name.trim()) return;
    setSaving(true);
    try {
      await portalPartnerApi.createDeal({
        // PortalDealCreate field names: title (required) and customer_org_name
        title: form.opportunity_name.trim(),
        customer_org_name: form.end_customer_name.trim() || undefined,
        estimated_value: parseFloat(form.estimated_value) || 0,
        currency: form.currency,
        expected_close_date: form.expected_close_date || undefined,
      });
      setShowModal(false);
      setForm(EMPTY);
      await load();
    } finally { setSaving(false); }
  }

  // Open (approved) deals, grouped by currency: deals in different currencies are never added up.
  const pipeline = sumByCurrency(deals.filter((d) => ["approved", "extended"].includes(d.status)),
    (d) => d.estimated_value, (d) => d.currency);

  const columns = useMemo<ColumnDef<DealReg>[]>(() => [
    { accessorKey: "title", header: "Opportunity", cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
    { accessorKey: "customer_org_name", header: "End customer", meta: { hideOnMobile: true }, cell: (c) => c.getValue<string | null>() || "-" },
    { accessorKey: "expected_close_date", header: "Close", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmtDate(c.getValue<string>()) },
    { accessorKey: "estimated_value", header: "Value", meta: { numeric: true },
      cell: (c) => <>{(c.row.original.estimated_value ?? 0).toLocaleString()} <span className="text-xs text-fg-muted">{c.row.original.currency}</span></> },
    { accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="partner_deal" status={c.getValue<string>()} label={STATUS_LABEL[c.getValue<string>()]} /> },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Submitted" value={deals.length} />
        <Stat label="Approved" value={deals.filter((d) => d.status === "approved").length} />
        <Stat label="Open pipeline" value={formatCurrency(pipeline.total, pipeline.currency)}
          note={pipeline.mixed ? `Mixed currencies: ${pipeline.currency} total shown` : undefined} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Deal registrations" summary="Register deals to lock in your commission protection."
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Register deal</Button>} />
        <DataTable label="Deal registrations" columns={columns} data={deals} getRowId={(d) => d.id} loading={loading}
          empty={{ icon: FileText, title: "No deal registrations yet", description: "Register a deal to protect your commission.",
            action: <Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Register deal</Button> }}
          renderDetails={(deal) => (
            <div className="space-y-3">
              <KeyValues cols={3} items={[
                { label: "End customer", value: deal.customer_org_name || "-" },
                { label: "Expected close", value: fmtDate(deal.expected_close_date) },
                { label: "Registered", value: fmtDate(deal.created_at) },
              ]} />
              {deal.status === "rejected" && deal.rejection_reason && (
                <div className="rounded-lg border border-line bg-surface p-3 text-[13px]">
                  <p className="text-xs text-fg-muted">Why this registration was not approved</p>
                  <p className="mt-1 text-fg">{deal.rejection_reason}</p>
                </div>
              )}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Register a deal" dismissable={!saving}
        description="The partnership team reviews new registrations within 2 business days."
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving} disabled={!form.opportunity_name.trim()}>
            {saving ? "Submitting..." : "Submit registration"}
          </Button>
        </>}>
        <FieldGrid cols={1}>
          <Field label="Opportunity name" required>
            <Input value={form.opportunity_name} onChange={(e) => setForm({ ...form, opportunity_name: e.target.value })}
              placeholder="e.g. Acme Ltd: support plan rollout" />
          </Field>
          <Field label="End customer name">
            <Input value={form.end_customer_name} onChange={(e) => setForm({ ...form, end_customer_name: e.target.value })}
              placeholder="Final buyer or organization" />
          </Field>
          <div className="grid grid-cols-3 gap-3">
            <Field label="Estimated value" className="col-span-2">
              <Input type="number" numeric value={form.estimated_value} onChange={(e) => setForm({ ...form, estimated_value: e.target.value })} />
            </Field>
            <Field label="Currency">
              <Select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}
                options={["USD", "EUR", "GBP", "JPY"].map((c) => ({ value: c, label: c }))} />
            </Field>
          </div>
          <Field label="Expected close date">
            <Input type="date" value={form.expected_close_date} onChange={(e) => setForm({ ...form, expected_close_date: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
