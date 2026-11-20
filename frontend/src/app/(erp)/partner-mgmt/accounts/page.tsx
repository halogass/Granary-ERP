"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { partnerApi } from "@/lib/api";
import { optionLabel, usePartnerOptions } from "@/lib/partnerOptions";
import { Handshake, Plus, Search, ShieldCheck, ShieldOff, Loader2, Trash2 } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader,
  Pill, SavedViewTabs, Select, Stat, StatGroup, StatusPill, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface PartnerAccount {
  id: string;
  company_name: string;
  partner_type: string;
  tier: string;
  status: string;
  contact_name?: string;
  contact_email?: string;
  country?: string;
  total_deal_value?: number;
  created_at: string;
  /** An active portal login exists (GET /partner/accounts). */
  portal_access?: boolean;
}

type InviteState = "idle" | "loading" | "sent" | "has_access" | "no_email" | "revoked";

const EMPTY_FORM = { company_name: "", partner_type: "reseller", tier: "authorized", contact_name: "", contact_email: "", country: "" };

type Filters = { tier?: string; status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "gold", label: "Gold", filters: { tier: "gold" } },
  { id: "platinum", label: "Platinum", filters: { tier: "platinum" } },
];

export default function PartnerAccountsPage() {
  const options = usePartnerOptions();
  const views = useSavedViews<Filters>("partner.accounts", VIEWS);
  const [typeChip, setTypeChip] = useState("");
  const [partners, setPartners] = useState<PartnerAccount[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [inviteStates, setInviteStates] = useState<Record<string, InviteState>>({});
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<PartnerAccount>(partnerApi.listAccounts);
      setPartners(rows);
    } catch { setPartners([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const vf = views.active?.filters ?? {};
  const filtered = partners.filter((p) =>
    (!vf.tier || p.tier === vf.tier) && (!vf.status || p.status === vf.status) &&
    (!typeChip || p.partner_type === typeChip) &&
    (!search ||
      p.company_name.toLowerCase().includes(search.toLowerCase()) ||
      (p.contact_email ?? "").toLowerCase().includes(search.toLowerCase())));

  async function create() {
    if (!form.company_name.trim()) return;
    setSaving(true);
    try {
      await partnerApi.createAccount({
        company_name: form.company_name.trim(),
        partner_type: form.partner_type,
        tier: form.tier,
        contact_name: form.contact_name.trim() || undefined,
        contact_email: form.contact_email.trim() || undefined,
        country: form.country.trim() || undefined,
      });
      setShowModal(false);
      setForm(EMPTY_FORM);
      await load();
    } finally { setSaving(false); }
  }

  async function deletePartner(id: string) {
    try {
      await partnerApi.deletePartnerAccount(id);
      setConfirmDeleteId(null);
      setPartners((prev) => prev.filter((p) => p.id !== id));
    } catch { /* keep */ }
  }

  async function invite(partner: PartnerAccount) {
    if (!partner.contact_email) {
      setInviteStates((s) => ({ ...s, [partner.id]: "no_email" }));
      return;
    }
    setInviteStates((s) => ({ ...s, [partner.id]: "loading" }));
    try {
      await partnerApi.invitePartner(partner.id);
      setInviteStates((s) => ({ ...s, [partner.id]: "sent" }));
    } catch (err: unknown) {
      const status = (err as { response?: { status?: number } })?.response?.status;
      setInviteStates((s) => ({ ...s, [partner.id]: status === 409 ? "has_access" : "no_email" }));
    }
  }

  async function revoke(p: PartnerAccount) {
    setInviteStates((s) => ({ ...s, [p.id]: "loading" }));
    try {
      await partnerApi.revokeAccess(p.id);
      setInviteStates((s) => ({ ...s, [p.id]: "revoked" }));
      setPartners((prev) => prev.map((x) => (x.id === p.id ? { ...x, portal_access: false } : x)));
      toast.success(`Portal access revoked for ${p.company_name}`);
    } catch {
      setInviteStates((s) => ({ ...s, [p.id]: "idle" }));
      toast.error("Could not revoke portal access");
    }
  }

  async function change(p: PartnerAccount, data: { tier?: string; status?: string; partner_type?: string }) {
    try {
      const res = await partnerApi.updateAccount(p.id, data);
      const ended = data.status === "suspended" || data.status === "terminated";
      setPartners((prev) => prev.map((x) => (x.id === p.id
        ? { ...x, ...res.data, portal_access: ended ? false : x.portal_access } : x)));
      toast.success("Partner updated");
    } catch (err: unknown) {
      const detail = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(typeof detail === "string" ? detail : "Could not update the partner");
    }
  }

  function portal(p: PartnerAccount) {
    const inv = inviteStates[p.id] ?? "idle";
    if (inv === "sent") return <Pill tone="blue" icon={ShieldCheck}>Invite sent</Pill>;
    if (inv === "has_access" || (inv === "idle" && p.portal_access)) {
      return <Button size="sm" onClick={() => revoke(p)}><ShieldOff aria-hidden /> Revoke portal access</Button>;
    }
    if (inv === "no_email") return <Pill tone="orange" icon={ShieldOff}>No email on file</Pill>;
    if (inv === "loading") return <span className="inline-flex items-center gap-1.5 text-xs text-fg-muted"><Loader2 className="size-3.5 animate-spin" aria-hidden /> Sending</span>;
    return <Button size="sm" onClick={() => invite(p)}><ShieldCheck aria-hidden /> Invite to portal</Button>;
  }

  const typeLabel = (t: string) => optionLabel(options.partner_types, t);
  const tierLabel = (t: string) => optionLabel(options.tiers, t);

  const columns = useMemo<ColumnDef<PartnerAccount>[]>(() => [
    { accessorKey: "company_name", header: "Partner", cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span> },
    { accessorKey: "partner_type", header: "Type", cell: (c) => <Pill className="capitalize">{typeLabel(c.getValue<string>())}</Pill> },
    {
      accessorKey: "tier", header: "Tier",
      cell: (c) => <StatusPill family="partner_tier" status={c.getValue<string>()} label={tierLabel(c.getValue<string>())} />,
    },
    {
      accessorKey: "contact_name", header: "Contact", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | undefined>() || "-"}</span>,
    },
    {
      accessorKey: "total_deal_value", header: "Deal value", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => c.getValue<number | undefined>() ? `$${c.getValue<number>().toLocaleString()}` : <span className="text-fg-subtle">-</span>,
    },
    { id: "portal", header: "Portal", enableSorting: false, meta: { hideOnMobile: true }, cell: (c) => portal(c.row.original) },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill status={c.getValue<string>()} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const p = c.row.original;
        return confirmDeleteId === p.id ? (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <span className="text-xs font-semibold text-tone-red">Delete?</span>
            <Button size="sm" variant="danger" onClick={() => deletePartner(p.id)}>Yes</Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
          </div>
        ) : (
          <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${p.company_name}`} onClick={() => setConfirmDeleteId(p.id)} />
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, inviteStates, options]);

  const typesInUse = Array.from(new Set(partners.map((p) => p.partner_type)));

  return (
    <div className="space-y-4">
      <StatGroup>
        {options.tiers.map(({ value, label }) => (
          <Stat key={value} label={label} value={partners.filter((p) => p.tier === value).length} />
        ))}
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Partner accounts"
          summary={`${partners.length} partner${partners.length !== 1 ? "s" : ""}. Resellers, SIs, distributors and referral partners.`}
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Add partner</Button>} />
        <SavedViewTabs state={views} currentFilters={vf} onSelect={() => setTypeChip("")} />
        <FilterChips
          chips={typesInUse.map((t) => ({ id: t, label: <span className="capitalize">{typeLabel(t)}</span>, active: typeChip === t }))}
          onToggle={(id) => setTypeChip((cur) => (cur === id ? "" : id))} onClear={() => setTypeChip("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search partners" aria-label="Search partners" className="w-56" />
        </FilterChips>
        <DataTable label="Partner accounts" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Handshake, title: "No partners yet", description: "Add a partner, or pick another view." }}
          renderDetails={(p) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Contact", value: p.contact_name || "-" },
                { label: "Email", value: p.contact_email || "-" },
                { label: "Country", value: p.country || "-" },
                { label: "Tier", value: tierLabel(p.tier) },
                { label: "Total deal value", value: p.total_deal_value ? `$${p.total_deal_value.toLocaleString()}` : "-", numeric: true },
                { label: "Since", value: p.created_at ? new Date(p.created_at).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-" },
              ]} />
              <FieldGrid cols={2} className="min-w-[260px]">
                <Field label="Tier">
                  <Select aria-label={`Tier of ${p.company_name}`} value={p.tier}
                    onChange={(e) => change(p, { tier: e.target.value })}
                    options={options.tiers.map((t) => ({ value: t.value, label: t.label }))} />
                </Field>
                <Field label="Status" hint="Suspending or terminating also ends portal access.">
                  <Select aria-label={`Status of ${p.company_name}`} value={p.status}
                    onChange={(e) => change(p, { status: e.target.value })}
                    options={options.statuses.map((t) => ({ value: t.value, label: t.label }))} />
                </Field>
              </FieldGrid>
              <div className="md:hidden">{portal(p)}</div>
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Add partner"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!form.company_name.trim()} loading={saving}>{saving ? "Adding" : "Add partner"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Company name" required className="md:col-span-2">
            <Input value={form.company_name} onChange={(e) => setForm({ ...form, company_name: e.target.value })} />
          </Field>
          <Field label="Partner type">
            <Select value={form.partner_type} onChange={(e) => setForm({ ...form, partner_type: e.target.value })}
              options={options.partner_types.map((t) => ({ value: t.value, label: t.label }))} />
          </Field>
          <Field label="Tier">
            <Select value={form.tier} onChange={(e) => setForm({ ...form, tier: e.target.value })}
              options={options.tiers.map((t) => ({ value: t.value, label: t.label }))} />
          </Field>
          <Field label="Contact name">
            <Input value={form.contact_name} onChange={(e) => setForm({ ...form, contact_name: e.target.value })} />
          </Field>
          <Field label="Contact email">
            <Input type="email" value={form.contact_email} onChange={(e) => setForm({ ...form, contact_email: e.target.value })} />
          </Field>
          <Field label="Country" className="md:col-span-2">
            <Input value={form.country} onChange={(e) => setForm({ ...form, country: e.target.value })} placeholder="Japan, Singapore..." />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
