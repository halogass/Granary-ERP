"use client";

import { bearerGet, fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useMemo } from "react";
import { useSession } from "next-auth/react";
import { Plus, Search, UserPlus } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, Input, KeyValues, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface PartnerAccount {
  id: string;
  company_name: string;
  tier: string;
}

interface Lead {
  id: string;
  customer_name: string;
  customer_email: string | null;
  customer_company: string | null;
  customer_country: string | null;
  product_interest: string[];
  notes: string | null;
  status: string;
  partner_id: string;
  partner_name: string;
  assigned_at: string | null;
  expires_at: string | null;
  created_at: string;
}

const STATUS_LABELS: Record<string, string> = {
  new: "New", contacted: "Contacted", qualified: "Qualified", converted: "Converted", dead: "Dead",
};
const STATUS_OPTIONS = Object.entries(STATUS_LABELS).map(([value, label]) => ({ value, label }));

interface LeadForm {
  customer_name: string;
  customer_email: string;
  customer_company: string;
  customer_country: string;
  product_interest: string;
  notes: string;
  partner_id: string;
  expires_at: string;
}

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "new", label: "New", filters: { status: "new" } },
  { id: "contacted", label: "Contacted", filters: { status: "contacted" } },
  { id: "qualified", label: "Qualified", filters: { status: "qualified" } },
  { id: "converted", label: "Converted", filters: { status: "converted" } },
];

function fmt(d: string) { return new Date(d).toLocaleDateString("en-GB", { day: "2-digit", month: "short", year: "numeric" }); }

export default function PartnerLeadsAdminPage() {
  const { data: session } = useSession();
  const views = useSavedViews<Filters>("partner.leads", VIEWS);
  const [chips, setChips] = useState<Record<string, boolean>>({});
  const [search, setSearch] = useState("");
  const [leads, setLeads] = useState<Lead[]>([]);
  const [partners, setPartners] = useState<PartnerAccount[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [form, setForm] = useState<LeadForm>({ customer_name: "", customer_email: "", customer_company: "", customer_country: "", product_interest: "", notes: "", partner_id: "", expires_at: "" });
  const [submitting, setSubmitting] = useState(false);

  const token = session?.access_token;

  useEffect(() => {
    if (!token) return;
    Promise.all([
      fetchAllPages<Lead>(bearerGet("/api/v1/partner/leads", token)),
      fetchAllPages<PartnerAccount>(bearerGet("/api/v1/partner/accounts", token), { status: "active" }),
    ]).then(([ls, pts]) => { setLeads(ls.rows); setPartners(pts.rows); })
      .catch(() => toast.error("Failed to load data"))
      .finally(() => setLoading(false));
  }, [token]);

  async function assignLead(e: React.FormEvent) {
    e.preventDefault();
    if (!token) return;
    setSubmitting(true);
    try {
      const res = await fetch("/api/v1/partner/leads", {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({
          ...form,
          product_interest: form.product_interest.split(",").map(s => s.trim()).filter(Boolean),
          expires_at: form.expires_at || null,
        }),
      });
      if (!res.ok) throw new Error();
      const created = await res.json();
      setLeads(prev => [created, ...prev]);
      setShowForm(false);
      setForm({ customer_name: "", customer_email: "", customer_company: "", customer_country: "", product_interest: "", notes: "", partner_id: "", expires_at: "" });
      toast.success("Lead assigned to partner");
    } catch { toast.error("Failed to assign lead"); }
    finally { setSubmitting(false); }
  }

  async function updateStatus(leadId: string, status: string) {
    if (!token) return;
    try {
      await fetch(`/api/v1/partner/leads/${leadId}/status`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({ status }),
      });
      setLeads(prev => prev.map(l => l.id === leadId ? { ...l, status } : l));
    } catch { toast.error("Failed to update status"); }
  }

  const status = chips.dead ? "dead" : views.active?.filters.status;
  const q = search.trim().toLowerCase();
  const filtered = leads.filter(l => (!status || l.status === status) &&
    (!q || l.customer_name.toLowerCase().includes(q) || (l.customer_company ?? "").toLowerCase().includes(q) || (l.partner_name ?? "").toLowerCase().includes(q)));

  const columns = useMemo<ColumnDef<Lead>[]>(() => [
    {
      accessorKey: "customer_name", header: "Customer",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.row.original.customer_name}</p>
          {c.row.original.customer_email && <p className="truncate text-xs text-fg-muted">{c.row.original.customer_email}</p>}
        </div>
      ),
    },
    {
      accessorKey: "customer_company", header: "Company", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() || "-"}</span>,
    },
    { accessorKey: "partner_name", header: "Assigned partner", cell: (c) => <span className="font-medium">{c.getValue<string>() || "-"}</span> },
    {
      id: "products", header: "Products", enableSorting: false, meta: { hideOnMobile: true },
      cell: (c) => (
        <div className="flex flex-wrap gap-1">
          {(c.row.original.product_interest || []).slice(0, 2).map(p => <Pill key={p}>{p}</Pill>)}
        </div>
      ),
    },
    {
      accessorKey: "expires_at", header: "Expires", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() ? fmt(c.getValue<string>()) : "-"}</span>,
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => (
        <Select value={c.row.original.status} onChange={e => updateStatus(c.row.original.id, e.target.value)}
          aria-label={`Status of ${c.row.original.customer_name}`} className="w-32 text-xs md:h-7" options={STATUS_OPTIONS} />
      ),
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [token]);

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Partner lead management"
          summary={`${leads.length} lead${leads.length !== 1 ? "s" : ""}. Assign warm leads to partner accounts for follow-up.`}
          actions={<Button variant="primary" onClick={() => setShowForm(true)}><Plus aria-hidden /> Assign lead</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}} onSelect={() => setChips({})} />
        <FilterChips chips={[{ id: "dead", label: "Dead", active: !!chips.dead }]}
          onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id] }))} onClear={() => setChips({})}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search leads" aria-label="Search partner leads" className="w-56" />
        </FilterChips>
        <DataTable label="Partner leads" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: UserPlus, title: "No leads found", description: "Pick another view, or assign a lead to a partner." }}
          renderDetails={(l) => (
            <div className="space-y-2">
              <KeyValues cols={3} items={[
                { label: "Email", value: l.customer_email || "-" },
                { label: "Company", value: l.customer_company || "-" },
                { label: "Country", value: l.customer_country || "-" },
                { label: "Products", value: (l.product_interest || []).join(", ") || "-" },
                { label: "Assigned", value: l.assigned_at ? fmt(l.assigned_at) : "-" },
                { label: "Expires", value: l.expires_at ? fmt(l.expires_at) : "-" },
              ]} />
              {l.notes && <p className="whitespace-pre-wrap text-sm text-fg-muted">{l.notes}</p>}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showForm} onOpenChange={setShowForm} title="Assign lead to partner"
        footer={<>
          <Button onClick={() => setShowForm(false)}>Cancel</Button>
          <Button type="submit" form="assign-lead-form" variant="primary" loading={submitting}>{submitting ? "Assigning" : "Assign lead"}</Button>
        </>}>
        <form id="assign-lead-form" onSubmit={assignLead}>
          <FieldGrid cols={2}>
            <Field label="Assign to partner" required className="md:col-span-2">
              <Select required value={form.partner_id} onChange={e => setForm(p => ({ ...p, partner_id: e.target.value }))}
                placeholder="Select partner" options={partners.map(pt => ({ value: pt.id, label: `${pt.company_name} (${pt.tier})` }))} />
            </Field>
            <Field label="Customer name" required>
              <Input required value={form.customer_name} onChange={e => setForm(p => ({ ...p, customer_name: e.target.value }))} />
            </Field>
            <Field label="Email">
              <Input type="email" value={form.customer_email} onChange={e => setForm(p => ({ ...p, customer_email: e.target.value }))} />
            </Field>
            <Field label="Company">
              <Input value={form.customer_company} onChange={e => setForm(p => ({ ...p, customer_company: e.target.value }))} />
            </Field>
            <Field label="Country">
              <Input value={form.customer_country} onChange={e => setForm(p => ({ ...p, customer_country: e.target.value }))} />
            </Field>
            <Field label="Product interest" hint="Comma-separated." className="md:col-span-2">
              <Input value={form.product_interest} onChange={e => setForm(p => ({ ...p, product_interest: e.target.value }))} placeholder="Laptops, support plans" />
            </Field>
            <Field label="Notes for partner" className="md:col-span-2">
              <Textarea rows={2} value={form.notes} onChange={e => setForm(p => ({ ...p, notes: e.target.value }))} />
            </Field>
            <Field label="Lead expiry date">
              <Input type="date" value={form.expires_at} onChange={e => setForm(p => ({ ...p, expires_at: e.target.value }))} />
            </Field>
          </FieldGrid>
        </form>
      </Dialog>
    </>
  );
}
