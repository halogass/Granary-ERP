"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { marketingApi } from "@/lib/api";
import { Megaphone, Plus, Trash2, ArrowRight, CheckCircle2 } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, IconButton, Input, KeyValues, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView, type Tone,
} from "@/components/ui";

// Shape of GET /marketing/campaigns.
interface Campaign {
  id: string;
  name: string;
  objective?: string | null;
  status: string;
  channel?: string | null;
  budget?: number | null;
  currency?: string | null;
  target_audience?: string | null;
  description?: string | null;
  start_date?: string | null;
  end_date?: string | null;
  leads_generated?: number | null;
}

// Matches models/marketing.py MarketingCampaign.objective
const TYPE_META: Record<string, { tone: Tone }> = {
  lead_generation: { tone: "violet" },
  brand_awareness: { tone: "blue" },
  product_launch:  { tone: "blue" },
  nurturing:       { tone: "neutral" },
  retention:       { tone: "orange" },
};

const NEXT_STATUS: Record<string, string> = {
  planned: "active",
  draft: "active",
  active: "paused",
  paused: "active",
};

const CHANNELS = ["email", "social_media", "whatsapp", "ads", "events", "referral", "seo", "other"];
const human = (s?: string | null) => (s ?? "").replace(/_/g, " ");
const shortDate = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short" });

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "planned", label: "Planned", filters: { status: "planned" } },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "paused", label: "Paused", filters: { status: "paused" } },
  { id: "completed", label: "Completed", filters: { status: "completed" } },
];

const EMPTY_FORM = {
  name: "", objective: "lead_generation", channel: "email", target_audience: "",
  budget: "", currency: "USD", start_date: "", end_date: "", description: "",
};

export default function CampaignsPage() {
  const views = useSavedViews<Filters>("marketing.campaigns", VIEWS);
  const [campaigns, setCampaigns] = useState<Campaign[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<Campaign>(marketingApi.listCampaigns);
      setCampaigns(rows);
    } catch { setCampaigns([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function create() {
    if (!form.name.trim()) return;
    setSaving(true);
    try {
      await marketingApi.createCampaign({
        name: form.name.trim(),
        objective: form.objective,
        channel: form.channel,
        target_audience: form.target_audience.trim() || undefined,
        budget: form.budget ? parseFloat(form.budget) : undefined,
        currency: form.currency,
        start_date: form.start_date || undefined,
        end_date: form.end_date || undefined,
        description: form.description.trim() || undefined,
      });
      setShowModal(false);
      setForm(EMPTY_FORM);
      await load();
    } finally { setSaving(false); }
  }

  async function deleteCampaign(id: string) {
    try {
      await marketingApi.deleteCampaign(id);
      setConfirmDeleteId(null);
      setCampaigns((prev) => prev.filter((c) => c.id !== id));
    } catch { /* keep */ }
  }

  async function advance(id: string, nextStatus: string) {
    await marketingApi.updateStatus(id, nextStatus);
    await load();
  }

  const filterStatus = views.active?.filters.status ?? "";
  const filters: Filters = filterStatus ? { status: filterStatus } : {};
  const filtered = !filterStatus ? campaigns : campaigns.filter((c) => c.status === filterStatus);
  const activeCampaigns = campaigns.filter((c) => c.status === "active");
  const totalBudget = campaigns.reduce((s, c) => s + (c.budget ?? 0), 0);
  const totalLeads = campaigns.reduce((s, c) => s + (c.leads_generated ?? 0), 0);

  function rowActions(campaign: Campaign) {
    const nextStatus = NEXT_STATUS[campaign.status];
    const canDelete = campaign.status !== "active";
    if (canDelete && confirmDeleteId === campaign.id) {
      return (
        <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
          <span className="text-xs font-semibold text-tone-red">Delete?</span>
          <Button size="sm" variant="danger" onClick={() => deleteCampaign(campaign.id)}>Yes</Button>
          <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
        </div>
      );
    }
    return (
      <div className="flex items-center justify-end gap-1 whitespace-nowrap">
        {nextStatus && (
          <Button size="sm" variant="ghost" className="capitalize" onClick={() => advance(campaign.id, nextStatus)}>
            <ArrowRight aria-hidden /> {nextStatus === "active" ? "Activate" : nextStatus === "paused" ? "Pause" : nextStatus}
          </Button>
        )}
        {campaign.status === "active" && (
          <Button size="sm" variant="ghost" onClick={() => advance(campaign.id, "completed")}><CheckCircle2 aria-hidden /> Complete</Button>
        )}
        {canDelete && <IconButton size="sm" icon={Trash2} label={`Delete ${campaign.name}`} onClick={() => setConfirmDeleteId(campaign.id)} />}
      </div>
    );
  }

  const columns = useMemo<ColumnDef<Campaign>[]>(() => [
    {
      accessorKey: "name", header: "Campaign",
      cell: (c) => {
        const cp = c.row.original;
        return (
          <div className="min-w-0">
            <p className="truncate font-medium text-fg">{cp.name}</p>
            {cp.target_audience && <p className="max-w-xs truncate text-xs text-fg-muted">{cp.target_audience}</p>}
          </div>
        );
      },
    },
    {
      accessorKey: "objective", header: "Objective", meta: { hideOnMobile: true },
      cell: (c) => c.getValue<string>() ? <Pill tone={TYPE_META[c.getValue<string>()]?.tone ?? "neutral"} className="capitalize">{human(c.getValue<string>())}</Pill> : null,
    },
    { accessorKey: "channel", header: "Channel", meta: { hideOnMobile: true }, cell: (c) => <span className="capitalize text-fg-muted">{human(c.getValue<string>()) || "-"}</span> },
    {
      id: "dates", accessorFn: (cp) => cp.start_date ?? "", header: "Dates", meta: { hideOnMobile: true },
      cell: (c) => {
        const cp = c.row.original;
        if (!cp.start_date && !cp.end_date) return <span className="text-fg-subtle">-</span>;
        return <span className="text-fg-muted tabular">{cp.start_date ? shortDate(cp.start_date) : ""}{cp.end_date ? ` to ${shortDate(cp.end_date)}` : ""}</span>;
      },
    },
    { accessorKey: "leads_generated", header: "Leads", meta: { numeric: true }, cell: (c) => c.getValue<number | null>() ?? "-" },
    {
      accessorKey: "budget", header: "Budget", meta: { numeric: true },
      cell: (c) => c.row.original.budget ? <span className="font-medium">{c.row.original.budget.toLocaleString()} <span className="text-xs font-normal text-fg-muted">{c.row.original.currency}</span></span> : <span className="text-fg-subtle">-</span>,
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="campaign" status={c.getValue<string>()} /> },
    { id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" }, cell: (c) => rowActions(c.row.original) },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={campaigns.length} />
        <Stat label="Active" value={activeCampaigns.length} />
        <Stat label="Total budget" value={`$${totalBudget.toLocaleString()}`} />
        <Stat label="Leads generated" value={totalLeads} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Campaigns" summary="Marketing campaigns across all channels"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New campaign</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} />
        <DataTable label="Campaigns" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Megaphone, title: "No campaigns", description: "Create a campaign to track its budget and leads." }}
          renderDetails={(cp) => (
            <KeyValues cols={3} items={[
              { label: "Objective", value: <span className="capitalize">{human(cp.objective) || "-"}</span> },
              { label: "Channel", value: <span className="capitalize">{human(cp.channel) || "-"}</span> },
              { label: "Target audience", value: cp.target_audience ?? "-" },
              { label: "Start", value: cp.start_date ? shortDate(cp.start_date) : "-" },
              { label: "End", value: cp.end_date ? shortDate(cp.end_date) : "-" },
              { label: "Description", value: cp.description ?? "-" },
            ]} />
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="New campaign"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving} disabled={!form.name.trim()}>{saving ? "Creating..." : "Create campaign"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Campaign name" required className="md:col-span-2">
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Q2 product launch, Europe" />
          </Field>
          <Field label="Objective">
            <Select value={form.objective} onChange={(e) => setForm({ ...form, objective: e.target.value })}
              options={Object.keys(TYPE_META).map((t) => ({ value: t, label: human(t) }))} />
          </Field>
          <Field label="Channel">
            <Select value={form.channel} onChange={(e) => setForm({ ...form, channel: e.target.value })}
              options={CHANNELS.map((c) => ({ value: c, label: human(c) }))} />
          </Field>
          <Field label="Target audience" className="md:col-span-2">
            <Input value={form.target_audience} onChange={(e) => setForm({ ...form, target_audience: e.target.value })} placeholder="e.g. Government, banks, retail" />
          </Field>
          <Field label="Budget">
            <Input numeric type="number" value={form.budget} onChange={(e) => setForm({ ...form, budget: e.target.value })} />
          </Field>
          <Field label="Currency">
            <Select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}
              options={["USD", "EUR", "GBP", "JPY"].map((c) => ({ value: c, label: c }))} />
          </Field>
          <Field label="Start date">
            <Input type="date" value={form.start_date} onChange={(e) => setForm({ ...form, start_date: e.target.value })} />
          </Field>
          <Field label="End date">
            <Input type="date" value={form.end_date} onChange={(e) => setForm({ ...form, end_date: e.target.value })} />
          </Field>
          <Field label="Description" className="md:col-span-2">
            <Textarea value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} rows={2} />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
