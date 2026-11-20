"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { crmApi, researchApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { fetchAllPages } from "@/lib/fetchAll";
import { safeHref } from "@/lib/sanitize";
import { Plus, Search, Globe, Phone, Mail, Pencil, Trash2, Paperclip, Globe2, Building2 } from "lucide-react";
import Link from "next/link";
import AttachmentPanel from "@/components/ui/AttachmentPanel";
import AiSelectionBar from "@/components/crm/AiSelectionBar";
import {
  Button, Checkbox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame,
  PageHeader, Pill, SavedViewTabs, Select, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Organization {
  id: string;
  name: string;
  industry?: string;
  website?: string;
  phone?: string;
  email?: string;
  city?: string;
  country?: string;
  address?: string;
  tax_id?: string;
  notes?: string;
  created_at: string;
}

type Filters = { withEmail?: boolean; withPhone?: boolean; recent?: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "recent", label: "Added this month", filters: { recent: true } },
  { id: "email", label: "With email", filters: { withEmail: true } },
  { id: "phone", label: "With phone", filters: { withPhone: true } },
];

const INDUSTRIES = ["Technology", "Manufacturing", "Healthcare", "Finance", "Education", "Retail", "Construction", "Consulting", "Other"];

const BLANK_FORM = { name: "", industry: "", website: "", phone: "", email: "", city: "", country: "", address: "", tax_id: "", notes: "" };

const MONTH_MS = 30 * 24 * 3600 * 1000;

export default function OrganizationsPage() {
  const [orgs, setOrgs] = useState<Organization[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState(""); // the search box, sent 300 ms after the last keystroke
  const [industryFilter, setIndustryFilter] = useState("");
  const [showForm, setShowForm] = useState(false);
  const [editingOrg, setEditingOrg] = useState<Organization | null>(null);
  const [form, setForm] = useState(BLANK_FORM);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [filesFor, setFilesFor] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [confirmBulk, setConfirmBulk] = useState(false);
  const [bulkDeleting, setBulkDeleting] = useState(false);
  const [researching, setResearching] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ kind: "success" | "info" | "error"; text: string } | null>(null);
  const views = useSavedViews<Filters>("crm.organizations", VIEWS);
  const filters: Filters = views.active?.filters ?? {};

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (query) params.q = query;
      // Every page: the API answers 50 rows by default, so the table used to stop there.
      const { rows } = await fetchAllPages<Organization>(crmApi.listOrgs, params);
      let data = rows;
      if (industryFilter) data = data.filter((o) => o.industry === industryFilter);
      setOrgs(data);
      setSelected((prev) => prev.filter((id) => data.some((o) => o.id === id)));
    } catch {
      setOrgs([]);
    } finally {
      setLoading(false);
    }
  }, [query, industryFilter]);

  useEffect(() => { load(); }, [load]);
  useEffect(() => {
    const t = setTimeout(() => setQuery(search.trim()), 300);
    return () => clearTimeout(t);
  }, [search]);

  function openCreate() {
    setEditingOrg(null);
    setForm(BLANK_FORM);
    setShowForm(true);
  }

  function openEdit(org: Organization) {
    setEditingOrg(org);
    setForm({ name: org.name, industry: org.industry ?? "", website: org.website ?? "", phone: org.phone ?? "", email: org.email ?? "", city: org.city ?? "", country: org.country ?? "", address: org.address ?? "", tax_id: org.tax_id ?? "", notes: org.notes ?? "" });
    setShowForm(true);
  }

  function closeForm() {
    setShowForm(false);
    setEditingOrg(null);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    try {
      if (editingOrg) {
        await crmApi.updateOrg(editingOrg.id, form);
      } else {
        await crmApi.createOrg(form);
      }
      setShowForm(false);
      setForm(BLANK_FORM);
      setEditingOrg(null);
      load();
    } finally {
      setSaving(false);
    }
  }

  /** Deep research from the row: it runs on a worker; the result lands on the company page. */
  async function research(org: Organization) {
    setResearching(org.id);
    try {
      const r = await researchApi.start("organization", org.id);
      setNotice({
        kind: r.data.run.status === "failed" ? "error" : "info",
        text: r.data.run.status === "failed"
          ? `Research on ${org.name} could not run: ${r.data.run.error}`
          : `Researching ${org.name}. You'll get a notification when it's done.`,
      });
    } catch (e) {
      setNotice({ kind: "error", text: extractApiError(e, "Research could not be started.") });
    } finally {
      setResearching(null);
    }
  }

  async function deleteOrg(id: string) {
    setDeleting(true);
    try {
      await crmApi.deleteOrg(id);
      setOrgs((prev) => prev.filter((o) => o.id !== id));
      setConfirmDeleteId(null);
      setExpanded((s) => (s === id ? null : s));
    } finally {
      setDeleting(false);
    }
  }

  /** Deletes the selected organizations one by one (same permission check as a single delete). */
  async function deleteSelected() {
    setBulkDeleting(true);
    const done: string[] = [];
    const failed: string[] = [];
    for (const id of selected) {
      try {
        await crmApi.deleteOrg(id);
        done.push(id);
      } catch (e) {
        const name = orgs.find((o) => o.id === id)?.name ?? id;
        failed.push(`${name} (${extractApiError(e, "could not be deleted")})`);
      }
    }
    setOrgs((prev) => prev.filter((o) => !done.includes(o.id)));
    setSelected(failed.length ? selected.filter((id) => !done.includes(id)) : []);
    setConfirmBulk(false);
    setBulkDeleting(false);
    setNotice(failed.length
      ? { kind: "error", text: `Deleted ${done.length}; not deleted: ${failed.join("; ")}` }
      : { kind: "success", text: `Deleted ${done.length} organization${done.length !== 1 ? "s" : ""}.` });
  }

  const usedIndustries = [...new Set(orgs.map((o) => o.industry).filter(Boolean) as string[])];
  const shown = useMemo(() => {
    const now = Date.now();
    return orgs.filter((o) =>
      (!filters.withEmail || !!o.email) &&
      (!filters.withPhone || !!o.phone) &&
      (!filters.recent || (o.created_at && now - new Date(o.created_at).getTime() < MONTH_MS)));
  }, [orgs, filters.withEmail, filters.withPhone, filters.recent]);

  const allSelected = shown.length > 0 && shown.every((o) => selected.includes(o.id));

  const columns = useMemo<ColumnDef<Organization>[]>(() => [
    {
      id: "select", enableSorting: false, meta: { width: "40px" },
      header: () => (
        <Checkbox aria-label="Select every organization shown" checked={allSelected}
          onChange={(e) => setSelected(e.target.checked ? shown.map((o) => o.id) : [])} />
      ),
      cell: (c) => (
        <Checkbox aria-label={`Select ${c.row.original.name}`} checked={selected.includes(c.row.original.id)}
          onChange={(e) => setSelected((prev) =>
            e.target.checked ? [...prev, c.row.original.id] : prev.filter((id) => id !== c.row.original.id))} />
      ),
    },
    {
      accessorKey: "name", header: "Organization",
      cell: (c) => {
        const org = c.row.original;
        return (
          <div className="min-w-0">
            <Link href={`/crm/organizations/${org.id}`} className="font-medium text-fg hover:text-brand-ink">{org.name}</Link>
            {org.website && <p className="max-w-56 truncate text-xs text-fg-muted">{org.website.replace(/^https?:\/\//, "")}</p>}
          </div>
        );
      },
    },
    {
      accessorKey: "industry", header: "Industry", meta: { hideOnMobile: true },
      cell: (c) => c.getValue<string>() ? <Pill>{c.getValue<string>()}</Pill> : <span className="text-fg-subtle">-</span>,
    },
    {
      id: "location", header: "Location", meta: { hideOnMobile: true },
      accessorFn: (o) => [o.city, o.country].filter(Boolean).join(", "),
      cell: (c) => c.getValue<string>() || <span className="text-fg-subtle">-</span>,
    },
    {
      id: "contact", header: "Contact", enableSorting: false,
      cell: (c) => {
        const org = c.row.original;
        return (
          <div className="flex items-center gap-1 text-fg-muted">
            {org.email && <a href={`mailto:${org.email}`} aria-label={`Email ${org.name}`} className="flex size-7 items-center justify-center rounded-lg hover:bg-surface-2 hover:text-fg"><Mail className="size-3.5" aria-hidden /></a>}
            {org.phone && <a href={`tel:${org.phone}`} aria-label={`Call ${org.name}`} className="flex size-7 items-center justify-center rounded-lg hover:bg-surface-2 hover:text-fg"><Phone className="size-3.5" aria-hidden /></a>}
            {safeHref(org.website) && <a href={safeHref(org.website)} target="_blank" rel="noopener noreferrer" aria-label={`Website of ${org.name}`} className="flex size-7 items-center justify-center rounded-lg hover:bg-surface-2 hover:text-fg"><Globe className="size-3.5" aria-hidden /></a>}
          </div>
        );
      },
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right" },
      cell: (c) => {
        const org = c.row.original;
        return (
          <Button size="sm" variant="ghost" onClick={() => research(org)} loading={researching === org.id}
            title="Deep research: find facts, people and news on the web" aria-label={`Deep research ${org.name}`}>
            {researching !== org.id && <Globe2 aria-hidden />} Research
          </Button>
        );
      },
    },
  // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [selected, shown, allSelected, researching]);

  function details(org: Organization) {
    const confirming = confirmDeleteId === org.id;
    return (
      <div className="space-y-3">
        <div className="flex flex-wrap items-start gap-6">
          <KeyValues className="min-w-[260px] flex-1" items={[
            // The website is staff/AI entered: never render a javascript: (or data:) link.
            { label: "Website", value: org.website ? (safeHref(org.website)
              ? <a href={safeHref(org.website)} target="_blank" rel="noopener noreferrer" className="break-all text-brand-ink hover:underline">{org.website}</a>
              : <span className="break-all">{org.website}</span>) : null },
            { label: "Email", value: org.email ? <a href={`mailto:${org.email}`} className="break-all text-brand-ink hover:underline">{org.email}</a> : null },
            { label: "Phone", value: org.phone ? <a href={`tel:${org.phone}`} className="text-brand-ink hover:underline">{org.phone}</a> : null },
            { label: "Location", value: [org.city, org.country].filter(Boolean).join(", ") || null },
            { label: "Tax ID", value: org.tax_id ? <span className="font-mono text-[13px]">{org.tax_id}</span> : null },
            { label: "Industry", value: org.industry || null },
            ...(org.address ? [{ label: "Address", value: <span className="whitespace-pre-line">{org.address}</span> }] : []),
            ...(org.notes ? [{ label: "Notes", value: org.notes }] : []),
          ]} />
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" variant="primary" asChild><Link href={`/crm/organizations/${org.id}`}>Open organization</Link></Button>
            <Button size="sm" onClick={() => openEdit(org)}><Pencil aria-hidden /> Edit</Button>
            <Button size="sm" onClick={() => setFilesFor(filesFor === org.id ? null : org.id)} aria-expanded={filesFor === org.id}>
              <Paperclip aria-hidden /> Files
            </Button>
            {confirming ? (
              <>
                <span className="text-[13px] text-fg-muted">Delete this organization?</span>
                <Button size="sm" variant="danger" loading={deleting} onClick={() => deleteOrg(org.id)}>Delete</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>Cancel</Button>
              </>
            ) : (
              <IconButton size="sm" variant="danger" icon={Trash2} label={`Delete ${org.name}`} onClick={() => setConfirmDeleteId(org.id)} />
            )}
          </div>
        </div>
        {filesFor === org.id && (
          <div className="rounded-lg border border-line bg-surface p-3">
            <AttachmentPanel entityType="crm_org" entityId={org.id} />
          </div>
        )}
      </div>
    );
  }

  const withEmail = orgs.filter((o) => o.email).length;

  return (
    <div className="space-y-3">
      {notice && (
        <div role="status" className={`rounded-lg border px-4 py-3 text-sm ${
          notice.kind === "error" ? "border-tone-red/30 bg-tone-red-soft text-tone-red"
            : "border-tone-blue/30 bg-tone-blue-soft text-tone-blue"}`}>
          {notice.text}
        </div>
      )}


      <ListFrame>
        <PageHeader flush title="Organizations"
          summary={`${orgs.length} organization${orgs.length !== 1 ? "s" : ""}, ${usedIndustries.length} industries, ${withEmail} with email`}
          actions={<Button variant="primary" onClick={openCreate}><Plus aria-hidden /> Add organization</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setExpanded(null)} />
        <FilterChips
          chips={usedIndustries.map((ind) => ({ id: ind, label: ind, active: industryFilter === ind }))}
          onToggle={(id) => setIndustryFilter(industryFilter === id ? "" : id)}
          onClear={() => setIndustryFilter("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)}
            placeholder="Search organizations" aria-label="Search organizations" className="w-56" />
        </FilterChips>
        {selected.length > 0 && (
          <div className="flex flex-wrap items-center gap-2 px-4 py-2 text-[13px]">
            {confirmBulk ? (
              <>
                <span className="text-fg-muted">Delete {selected.length} selected organization{selected.length !== 1 ? "s" : ""}?</span>
                <Button size="sm" variant="danger" loading={bulkDeleting} onClick={deleteSelected}>Delete</Button>
                <Button size="sm" variant="ghost" disabled={bulkDeleting} onClick={() => setConfirmBulk(false)}>Cancel</Button>
              </>
            ) : (
              <Button size="sm" variant="danger" onClick={() => setConfirmBulk(true)}>
                <Trash2 aria-hidden /> Delete {selected.length} selected
              </Button>
            )}
          </div>
        )}
        <AiSelectionBar objectType="organization" selected={selected}
          onClear={() => setSelected([])}
          onMessage={(kind, text) => setNotice({ kind, text })} />
        <DataTable label="Organizations" columns={columns} data={shown} getRowId={(o) => o.id} loading={loading}
          expandedId={expanded} onExpandedChange={(id) => { setExpanded(id); setConfirmDeleteId(null); }}
          empty={{ icon: Building2, title: "No organizations in this view", description: "Pick another view, or add an organization.",
            action: <Button variant="primary" onClick={openCreate}><Plus aria-hidden /> Add organization</Button> }}
          renderDetails={details}
          footer={<span className="tabular">{shown.length} organization{shown.length !== 1 ? "s" : ""}</span>} />
      </ListFrame>

      <Dialog open={showForm} onOpenChange={(o) => { if (!o) closeForm(); }}
        title={editingOrg ? "Edit organization" : "New organization"}
        description={editingOrg ? "Update company details" : "Add a company to your CRM"}
        footer={<>
          <Button onClick={closeForm}>Cancel</Button>
          <Button variant="primary" type="submit" form="org-form" loading={saving}>
            {saving ? (editingOrg ? "Saving" : "Creating") : (editingOrg ? "Save changes" : "Create organization")}
          </Button>
        </>}>
        <form id="org-form" onSubmit={handleSubmit} className="space-y-4">
          <Field label="Company name" required>
            <Input required value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Acme Ltd" />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Industry">
              <Select value={form.industry} onChange={(e) => setForm({ ...form, industry: e.target.value })}
                placeholder="Select" options={INDUSTRIES.map((i) => ({ value: i, label: i }))} />
            </Field>
            <Field label="Website">
              <Input value={form.website} onChange={(e) => setForm({ ...form, website: e.target.value })} placeholder="https://" />
            </Field>
            <Field label="Email">
              <Input type="email" value={form.email} onChange={(e) => setForm({ ...form, email: e.target.value })} />
            </Field>
            <Field label="Phone">
              <Input value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
            </Field>
            <Field label="City">
              <Input value={form.city} onChange={(e) => setForm({ ...form, city: e.target.value })} />
            </Field>
            <Field label="Country">
              <Input value={form.country} onChange={(e) => setForm({ ...form, country: e.target.value })} />
            </Field>
          </FieldGrid>
          <Field label="Notes">
            <Textarea rows={2} value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} />
          </Field>
        </form>
      </Dialog>
    </div>
  );
}
