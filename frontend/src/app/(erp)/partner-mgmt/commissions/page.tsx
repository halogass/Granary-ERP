"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import api, { partnerApi } from "@/lib/api";
import { fetchAllPages } from "@/lib/fetchAll";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import { DollarSign, Plus } from "lucide-react";
import { formatCurrency, sumByCurrency } from "@/lib/currency";
import {
  Button, DataTable, Dialog, Field, FieldGrid, Input, ListFrame, PageHeader, SavedViewTabs, Select, Stat, StatGroup,
  StatusPill, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

// Commissions are recorded by staff (POST /partner/commissions): nothing creates them on its own when a
// registration is won, so the page says so and offers the form.
const NO_COMMISSIONS_TEXT =
  "Commissions are recorded here by the partnership team, usually after a registered deal is won. Winning a deal does not create one on its own.";

const EMPTY_FORM = { partner_id: "", deal_registration_id: "", base_amount: "", rate: "", currency: "", notes: "" };

// Shape of GET /partner/commissions.
interface Commission {
  id: string;
  partner_id: string;
  partner_name?: string | null;
  deal_registration_id?: string | null;
  amount: number;
  currency: string;
  rate: number;          // fraction, 0.1 = 10%
  status: string;
  paid_at?: string | null;
  created_at: string;
}

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "pending", label: "Pending", filters: { status: "pending" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "paid", label: "Paid", filters: { status: "paid" } },
];

const fmtDate = (d?: string | null) =>
  d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-";

export default function CommissionsPage() {
  const views = useSavedViews<Filters>("partner.commissions", VIEWS);
  const [commissions, setCommissions] = useState<Commission[]>([]);
  const [loading, setLoading] = useState(true);
  const { holds } = usePermissions();
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [partners, setPartners] = useState<{ id: string; company_name: string }[]>([]);
  const [regs, setRegs] = useState<{ id: string; partner_id: string; title: string; status: string; currency: string }[]>([]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await partnerApi.listCommissions();
      setCommissions(res.data ?? []);
    } catch { setCommissions([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  function openRecord() {
    setForm(EMPTY_FORM);
    setShowModal(true);
    fetchAllPages<{ id: string; company_name: string }>(partnerApi.listAccounts).then(({ rows }) => setPartners(rows)).catch(() => setPartners([]));
    partnerApi.listDealRegs().then((r) => setRegs(r.data ?? [])).catch(() => setRegs([]));
  }

  async function record() {
    if (!form.partner_id || !form.rate) return;
    setSaving(true);
    try {
      await api.post("/partner/commissions", {
        partner_id: form.partner_id,
        deal_registration_id: form.deal_registration_id || undefined,
        base_amount: form.base_amount ? form.base_amount : undefined,
        rate: String(Number(form.rate) / 100),
        currency: form.currency.trim().toUpperCase() || undefined,
        notes: form.notes.trim() || undefined,
      });
      setShowModal(false);
      await load();
    } catch (e) { toast.error(extractApiError(e, "Could not record the commission.")); } finally { setSaving(false); }
  }

  async function setStatus(c: Commission, status: "approved" | "cancelled" | "paid") {
    try {
      await api.patch(`/partner/commissions/${c.id}/status`, { status });
      await load();
    } catch (e) { toast.error(extractApiError(e, "Could not change the commission.")); }
  }

  // Commissions carry their own currency: grouped by currency, never added across them.
  const totalPaid = sumByCurrency(commissions.filter((c) => c.status === "paid"), (c) => c.amount, (c) => c.currency);
  const totalPending = sumByCurrency(commissions.filter((c) => c.status === "pending"), (c) => c.amount, (c) => c.currency);
  const mixedNote = (s: { mixed: boolean; currency: string }) => s.mixed ? `Mixed currencies: ${s.currency} total shown` : undefined;

  const status = views.active?.filters.status;
  const rows = commissions.filter((c) => !status || c.status === status);

  const columns = useMemo<ColumnDef<Commission>[]>(() => [
    { id: "partner", accessorFn: (c) => c.partner_name ?? "Partner", header: "Partner", cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span> },
    { accessorKey: "rate", header: "Rate", meta: { numeric: true }, cell: (c) => `${(c.getValue<number>() * 100).toFixed(1)}%` },
    { accessorKey: "created_at", header: "Created", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{fmtDate(c.getValue<string>())}</span> },
    { accessorKey: "paid_at", header: "Paid", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{fmtDate(c.getValue<string | null>())}</span> },
    {
      accessorKey: "amount", header: "Amount", meta: { numeric: true },
      cell: (c) => <span className="font-medium">{formatCurrency(c.row.original.amount, c.row.original.currency)}</span>,
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill status={c.getValue<string>()} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const row = c.row.original;
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            {row.status === "pending" && holds("partner_mgmt:approve_commission") && (
              <>
                <Button size="sm" onClick={() => setStatus(row, "approved")}>Approve</Button>
                <Button size="sm" variant="ghost" onClick={() => setStatus(row, "cancelled")}>Cancel</Button>
              </>
            )}
            {row.status === "approved" && holds("partner_mgmt:pay_commission") && (
              <Button size="sm" onClick={() => setStatus(row, "paid")}>Mark paid</Button>
            )}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [holds]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={commissions.length} />
        <Stat label="Pending" value={formatCurrency(totalPending.total, totalPending.currency)} note={mixedNote(totalPending)} />
        <Stat label="Paid out" value={formatCurrency(totalPaid.total, totalPaid.currency)} note={mixedNote(totalPaid)} />
        <Stat label="Partners" value={new Set(commissions.map((c) => c.partner_id)).size} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Partner commissions"
          summary={`${commissions.length} commission${commissions.length !== 1 ? "s" : ""}. Recorded by the partnership team.`}
          actions={holds("partner_mgmt:manage_commissions")
            ? <Button variant="primary" onClick={openRecord}><Plus aria-hidden /> Record commission</Button> : undefined} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} />
        <DataTable label="Partner commissions" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: DollarSign, title: "No commissions yet", description: NO_COMMISSIONS_TEXT }} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Record commission" dismissable={!saving}
        description="The amount is the base amount times the rate. Leave the base amount empty to use the registration's estimated value."
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={record} loading={saving} disabled={!form.partner_id || !form.rate}>Record</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Partner" required className="md:col-span-2">
            <Select value={form.partner_id} placeholder="Select partner"
              onChange={(e) => setForm({ ...form, partner_id: e.target.value, deal_registration_id: "" })}
              options={partners.map((p) => ({ value: p.id, label: p.company_name }))} />
          </Field>
          <Field label="Deal registration" className="md:col-span-2">
            <Select value={form.deal_registration_id} placeholder="None"
              onChange={(e) => setForm({ ...form, deal_registration_id: e.target.value })}
              options={regs.filter((r) => r.partner_id === form.partner_id)
                .map((r) => ({ value: r.id, label: `${r.title} (${r.status}, ${r.currency})` }))} />
          </Field>
          <Field label="Base amount">
            <Input numeric type="number" min={0} value={form.base_amount} onChange={(e) => setForm({ ...form, base_amount: e.target.value })} />
          </Field>
          <Field label="Rate (%)" required>
            <Input numeric type="number" min={0} max={100} step="0.1" value={form.rate} onChange={(e) => setForm({ ...form, rate: e.target.value })} />
          </Field>
          <Field label="Currency" hint="Defaults to the registration's currency.">
            <Input value={form.currency} maxLength={3} onChange={(e) => setForm({ ...form, currency: e.target.value })} placeholder="USD" />
          </Field>
          <Field label="Notes" className="md:col-span-2">
            <Textarea rows={2} value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
