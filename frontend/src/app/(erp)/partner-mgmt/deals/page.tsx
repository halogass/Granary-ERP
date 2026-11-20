"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { crmApi, partnerApi } from "@/lib/api";
import { FileText, Plus, Search, Trash2 } from "lucide-react";
import { extractApiError } from "@/lib/apiError";
import { formatCurrency, sumByCurrency } from "@/lib/currency";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader,
  SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

// Shape of GET /partner/deal-registrations.
interface DealReg {
  id: string;
  partner_id: string;
  partner_name?: string | null;
  title: string;
  customer_org_name?: string | null;
  estimated_value: number | null;
  currency: string;
  expected_close_date?: string | null;
  status: string;
  rejection_reason?: string | null;
  expires_at?: string | null;
  extended_until?: string | null;
  created_at: string;
}

const STATUS_LABELS: Record<string, string> = {
  pending: "Pending", approved: "Approved", rejected: "Rejected", expired: "Expired", extended: "Extended",
  closed_won: "Won", closed_lost: "Lost",
};
const CLOSABLE = ["approved", "extended"];

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "pending", label: "Pending review", filters: { status: "pending" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "closed_won", label: "Won", filters: { status: "closed_won" } },
];

const fmtDate = (d?: string | null) =>
  d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-";

export default function DealRegistrationsPage() {
  const views = useSavedViews<Filters>("partner.deals", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [search, setSearch] = useState("");
  const [deals, setDeals] = useState<DealReg[]>([]);
  const [partners, setPartners] = useState<{ id: string; company_name: string }[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [reviewModal, setReviewModal] = useState<DealReg | null>(null);
  const [form, setForm] = useState({ partner_account_id: "", opportunity_name: "", end_customer_name: "", estimated_value: "", currency: "USD", expected_close_date: "" });
  // deal_id "" = create a new deal at "new" from the registration on approval
  const [reviewForm, setReviewForm] = useState({ decision: "approved", notes: "", deal_id: "" });
  const [crmDeals, setCrmDeals] = useState<{ id: string; title: string }[]>([]);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [extendModal, setExtendModal] = useState<DealReg | null>(null);
  const [extendUntil, setExtendUntil] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [dRes, pRes] = await Promise.all([
        fetchAllPages<DealReg>(partnerApi.listDealRegs),
        fetchAllPages<{ id: string; company_name: string }>(partnerApi.listAccounts),
      ]);
      setDeals(dRes.rows);
      setPartners(pRes.rows);
    } catch { setDeals([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function create() {
    if (!form.opportunity_name.trim() || !form.partner_account_id) return;
    setSaving(true);
    try {
      await partnerApi.createDealReg({
        partner_id: form.partner_account_id,
        title: form.opportunity_name.trim(),
        customer_org_name: form.end_customer_name.trim() || undefined,
        estimated_value: parseFloat(form.estimated_value) || 0,
        currency: form.currency,
        expected_close_date: form.expected_close_date || undefined,
      });
      setShowModal(false);
      setForm({ partner_account_id: "", opportunity_name: "", end_customer_name: "", estimated_value: "", currency: "USD", expected_close_date: "" });
      await load();
    } finally { setSaving(false); }
  }

  async function deleteDeal(id: string) {
    try {
      await partnerApi.deleteDealReg(id);
      setConfirmDeleteId(null);
      setDeals((prev) => prev.filter((d) => d.id !== id));
    } catch { /* keep */ }
  }

  function openReview(deal: DealReg) {
    setReviewModal(deal); setReviewForm({ decision: "approved", notes: "", deal_id: "" });
    fetchAllPages<{ id: string; title: string }>(crmApi.listDeals).then(({ rows }) => setCrmDeals(rows)).catch(() => setCrmDeals([]));
  }

  async function review() {
    if (!reviewModal) return;
    setSaving(true);
    try {
      await partnerApi.reviewDealReg(reviewModal.id, {
        decision: reviewForm.decision, notes: reviewForm.notes,
        ...(reviewForm.decision === "approved" && reviewForm.deal_id ? { deal_id: reviewForm.deal_id } : {}),
      });
      setReviewModal(null);
      await load();
    } catch (err) {
      toast.error(extractApiError(err));
    } finally { setSaving(false); }
  }

  async function closeDeal(deal: DealReg, outcome: "won" | "lost") {
    const what = outcome === "won"
      ? `Mark "${deal.title}" as won? ${deal.estimated_value ?? 0} ${deal.currency} counts toward the partner's revenue.`
      : `Mark "${deal.title}" as lost?`;
    if (!window.confirm(what)) return;
    try {
      await partnerApi.closeDealReg(deal.id, { outcome });
      await load();
    } catch (e) { toast.error(extractApiError(e, "Could not close the deal.")); }
  }

  async function expireDeal(deal: DealReg) {
    if (!window.confirm(`End the protection of "${deal.title}" now? The registration becomes expired.`)) return;
    try {
      await partnerApi.setProtection(deal.id, { action: "expire" });
      await load();
    } catch (e) { toast.error(extractApiError(e, "Could not expire the registration.")); }
  }

  async function extendDeal() {
    if (!extendModal || !extendUntil) return;
    setSaving(true);
    try {
      await partnerApi.setProtection(extendModal.id, { action: "extend", until: extendUntil });
      setExtendModal(null);
      await load();
    } catch (e) { toast.error(extractApiError(e, "Could not extend the registration.")); } finally { setSaving(false); }
  }

  // Deals carry their own currency: the pipeline is grouped by currency, never added across them.
  const pipeline = sumByCurrency(deals.filter((d) => CLOSABLE.includes(d.status)),
    (d) => d.estimated_value, (d) => d.currency, "USD");
  const pendingCount = deals.filter((d) => d.status === "pending").length;

  const status = chipStatus || views.active?.filters.status;
  const q = search.trim().toLowerCase();
  const rows = deals.filter((d) => (!status || d.status === status) &&
    (!q || d.title.toLowerCase().includes(q) || (d.partner_name ?? "").toLowerCase().includes(q) || (d.customer_org_name ?? "").toLowerCase().includes(q)));

  const columns = useMemo<ColumnDef<DealReg>[]>(() => [
    { accessorKey: "title", header: "Opportunity", cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span> },
    { id: "partner", accessorFn: (d) => d.partner_name ?? "-", header: "Partner" },
    {
      accessorKey: "customer_org_name", header: "End customer", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() || "-"}</span>,
    },
    {
      accessorKey: "expected_close_date", header: "Close", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{fmtDate(c.getValue<string | null>())}</span>,
    },
    {
      accessorKey: "estimated_value", header: "Est. value", meta: { numeric: true },
      cell: (c) => <span className="font-medium">{formatCurrency(c.row.original.estimated_value ?? 0, c.row.original.currency)}</span>,
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="partner_deal" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} />,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const deal = c.row.original;
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            {deal.status === "pending" && <Button size="sm" variant="primary" onClick={() => openReview(deal)}>Review</Button>}
            {CLOSABLE.includes(deal.status) && (
              <>
                <Button size="sm" onClick={() => closeDeal(deal, "won")}>Won</Button>
                <Button size="sm" variant="ghost" onClick={() => closeDeal(deal, "lost")}>Lost</Button>
                <Button size="sm" variant="ghost" onClick={() => { setExtendModal(deal); setExtendUntil(""); }}>Extend</Button>
                <Button size="sm" variant="ghost" onClick={() => expireDeal(deal)}>Expire</Button>
              </>
            )}
            {deal.status === "pending" && (confirmDeleteId === deal.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deleteDeal(deal.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </>
            ) : (
              <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${deal.title}`} onClick={() => setConfirmDeleteId(deal.id)} />
            ))}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={deals.length} />
        <Stat label="Pending" value={pendingCount} note="waiting for review" alert={pendingCount > 0} />
        <Stat label="Approved" value={deals.filter((d) => d.status === "approved").length} />
        <Stat label="Pipeline" value={formatCurrency(pipeline.total, pipeline.currency)}
          note={pipeline.mixed ? `Mixed currencies: ${pipeline.currency} total shown` : undefined} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Deal registrations"
          summary="Partner deal protection: approve before the commission locks in."
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Register deal</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} onSelect={() => setChipStatus("")}
          alertIds={pendingCount > 0 ? ["pending"] : undefined} counts={{ pending: pendingCount }} />
        <FilterChips
          chips={["rejected", "expired", "extended", "closed_lost"].map((s) => ({ id: s, label: STATUS_LABELS[s], active: chipStatus === s }))}
          onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))} onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search deals" aria-label="Search deal registrations" className="w-56" />
        </FilterChips>
        <DataTable label="Deal registrations" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: FileText, title: "No deal registrations", description: "Pick another view, or register a deal." }}
          renderDetails={(d) => (
            <div className="space-y-2">
              <KeyValues cols={3} items={[
                { label: "Partner", value: d.partner_name ?? "-" },
                { label: "End customer", value: d.customer_org_name || "-" },
                { label: "Expected close", value: fmtDate(d.expected_close_date) },
                { label: "Estimated value", value: formatCurrency(d.estimated_value ?? 0, d.currency), numeric: true },
                { label: "Registered", value: fmtDate(d.created_at) },
                { label: "Protected until", value: fmtDate(d.extended_until ?? d.expires_at) },
              ]} />
              {d.rejection_reason && <p className="text-sm text-tone-red">Reason: {d.rejection_reason}</p>}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Register deal"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!form.opportunity_name.trim() || !form.partner_account_id} loading={saving}>
            {saving ? "Registering" : "Register deal"}
          </Button>
        </>}>
        <FieldGrid cols={3}>
          <Field label="Partner" required className="md:col-span-3">
            <Select value={form.partner_account_id} onChange={(e) => setForm({ ...form, partner_account_id: e.target.value })}
              placeholder="Select partner" options={partners.map((p) => ({ value: p.id, label: p.company_name }))} />
          </Field>
          <Field label="Opportunity name" required className="md:col-span-3">
            <Input value={form.opportunity_name} onChange={(e) => setForm({ ...form, opportunity_name: e.target.value })}
              placeholder="e.g. Acme Ltd, support plan rollout" />
          </Field>
          <Field label="End customer" className="md:col-span-3">
            <Input value={form.end_customer_name} onChange={(e) => setForm({ ...form, end_customer_name: e.target.value })} placeholder="Final customer name" />
          </Field>
          <Field label="Est. value" className="md:col-span-2">
            <Input numeric type="number" value={form.estimated_value} onChange={(e) => setForm({ ...form, estimated_value: e.target.value })} />
          </Field>
          <Field label="Currency">
            <Select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}
              options={["USD", "EUR", "GBP", "JPY"].map((c) => ({ value: c, label: c }))} />
          </Field>
          <Field label="Expected close" className="md:col-span-3">
            <Input type="date" value={form.expected_close_date} onChange={(e) => setForm({ ...form, expected_close_date: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={!!reviewModal} onOpenChange={(o) => { if (!o) setReviewModal(null); }} title="Review deal" size="sm"
        description={reviewModal?.title}
        footer={<>
          <Button onClick={() => setReviewModal(null)}>Cancel</Button>
          <Button variant="primary" onClick={review} loading={saving}>{saving ? "Saving" : "Submit"}</Button>
        </>}>
        <div className="space-y-4">
          <div className="flex gap-2" role="group" aria-label="Decision">
            {["approved", "rejected"].map((d) => (
              <Button key={d} className="flex-1 capitalize" aria-pressed={reviewForm.decision === d}
                variant={reviewForm.decision === d ? (d === "approved" ? "primary" : "danger") : "secondary"}
                onClick={() => setReviewForm({ ...reviewForm, decision: d })}>
                {d === "approved" ? "Approve" : "Reject"}
              </Button>
            ))}
          </div>
          {reviewForm.decision === "approved" && (
            <Field label="Our deal" hint="Linking the deal lets a win credit this partner automatically.">
              <Select value={reviewForm.deal_id} onChange={(e) => setReviewForm({ ...reviewForm, deal_id: e.target.value })}
                placeholder="Create a new deal from this registration"
                options={crmDeals.map((d) => ({ value: d.id, label: d.title }))} />
            </Field>
          )}
          <Field label={reviewForm.decision === "rejected" ? "Reason (shown to the partner)" : "Review notes"}>
            <Textarea value={reviewForm.notes} onChange={(e) => setReviewForm({ ...reviewForm, notes: e.target.value })} rows={3} />
          </Field>
        </div>
      </Dialog>

      <Dialog open={!!extendModal} onOpenChange={(o) => { if (!o) setExtendModal(null); }} title="Extend protection" size="sm"
        description={extendModal ? `${extendModal.title}: protected until ${fmtDate(extendModal.extended_until ?? extendModal.expires_at)}` : undefined}
        footer={<>
          <Button onClick={() => setExtendModal(null)}>Cancel</Button>
          <Button variant="primary" onClick={extendDeal} disabled={!extendUntil} loading={saving}>Extend</Button>
        </>}>
        <Field label="New end of protection" required>
          <Input type="date" value={extendUntil} onChange={(e) => setExtendUntil(e.target.value)} />
        </Field>
      </Dialog>
    </div>
  );
}
