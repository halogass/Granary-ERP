"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState, useCallback, useMemo } from "react";
import { researchApi, crmApi } from "@/lib/api";
import { UserCircle2, Plus, Search, Phone, Mail, MessageCircle, Pencil, Trash2, Globe2 } from "lucide-react";
import Link from "next/link";
import AiSelectionBar from "@/components/crm/AiSelectionBar";
import { extractApiError } from "@/lib/apiError";
import { orgSource } from "@/lib/entitySources";
import {
  Button, Checkbox, Combobox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues,
  ListFrame, PageHeader, SavedViewTabs, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Contact {
  id: string;
  first_name: string;
  last_name?: string;
  position?: string;
  email?: string;
  phone?: string;
  whatsapp?: string;
  organization_id?: string;
  organization?: { id: string; name: string };
  notes?: string;
  created_at: string;
}

type Filters = { withEmail?: boolean; withWhatsapp?: boolean; noOrg?: boolean; recent?: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "recent", label: "Added this month", filters: { recent: true } },
  { id: "whatsapp", label: "On WhatsApp", filters: { withWhatsapp: true } },
  { id: "noorg", label: "No organization", filters: { noOrg: true } },
];

const CHIPS: { id: keyof Filters; label: string }[] = [
  { id: "withEmail", label: "Has email" },
  { id: "withWhatsapp", label: "Has WhatsApp" },
  { id: "noOrg", label: "No organization" },
];

const BLANK_FORM = { full_name: "", position: "", email: "", phone: "", whatsapp: "", organization_id: "", notes: "" };

const MONTH_MS = 30 * 24 * 3600 * 1000;

function fullName(c: Contact) {
  return [c.first_name, c.last_name].filter(Boolean).join(" ");
}

export default function ContactsPage() {
  const [contacts, setContacts] = useState<Contact[]>([]);
  const [aiSelected, setAiSelected] = useState<string[]>([]);
  const [researching, setResearching] = useState<string | null>(null);
  const [aiNotice, setAiNotice] = useState<{ kind: "success" | "info" | "error"; text: string } | null>(null);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [showForm, setShowForm] = useState(false);
  const [editingContact, setEditingContact] = useState<Contact | null>(null);
  const [form, setForm] = useState(BLANK_FORM);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [chips, setChips] = useState<Filters>({});
  const views = useSavedViews<Filters>("crm.contacts", VIEWS);
  const filters: Filters = { ...views.active?.filters, ...Object.fromEntries(Object.entries(chips).filter(([, v]) => v)) };

  function splitName(full: string) {
    const parts = full.trim().split(/\s+/);
    if (parts.length === 1) return { first_name: parts[0], last_name: "" };
    return { first_name: parts.slice(0, -1).join(" "), last_name: parts[parts.length - 1] };
  }

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<Contact>(crmApi.listContacts, search ? { q: search } : {});
      setContacts(rows);
    } catch {
      setContacts([]);
    } finally {
      setLoading(false);
    }
  }, [search]);

  useEffect(() => { load(); }, [load]);
  function openCreate() {
    setEditingContact(null);
    setForm(BLANK_FORM);
    setShowForm(true);
  }

  function openEdit(c: Contact) {
    setEditingContact(c);
    setForm({
      full_name: fullName(c),
      position: c.position ?? "",
      email: c.email ?? "",
      phone: c.phone ?? "",
      whatsapp: c.whatsapp ?? "",
      organization_id: c.organization_id ?? "",
      notes: c.notes ?? "",
    });
    setShowForm(true);
  }

  function closeForm() {
    setShowForm(false);
    setEditingContact(null);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    try {
      const { first_name, last_name } = splitName(form.full_name);
      const payload = {
        first_name,
        last_name: last_name || undefined,
        position: form.position || undefined,
        email: form.email || undefined,
        phone: form.phone || undefined,
        whatsapp: form.whatsapp || undefined,
        organization_id: form.organization_id || undefined,
        notes: form.notes || undefined,
      };
      if (editingContact) {
        await crmApi.updateContact(editingContact.id, payload);
      } else {
        await crmApi.createContact(payload);
      }
      setShowForm(false);
      setForm(BLANK_FORM);
      setEditingContact(null);
      load();
    } finally {
      setSaving(false);
    }
  }

  async function deleteContact(id: string) {
    setDeleting(true);
    try {
      await crmApi.deleteContact(id);
      setContacts((prev) => prev.filter((c) => c.id !== id));
      setConfirmDeleteId(null);
    } finally {
      setDeleting(false);
    }
  }

  /** Research this contact: runs on a worker; results land on the contact page. */
  async function research(c: Contact) {
    setResearching(c.id);
    try {
      const r = await researchApi.start("contact", c.id);
      setAiNotice({
        kind: r.data.run.status === "failed" ? "error" : "info",
        text: r.data.run.status === "failed"
          ? `Research on ${fullName(c)} could not run: ${r.data.run.error}`
          : `Researching ${fullName(c)}. You'll get a notification when it's done.`,
      });
    } catch (e) {
      setAiNotice({ kind: "error", text: extractApiError(e, "Research could not be started.") });
    } finally {
      setResearching(null);
    }
  }

  const shown = useMemo(() => {
    const now = Date.now();
    return contacts.filter((c) =>
      (!filters.withEmail || !!c.email) &&
      (!filters.withWhatsapp || !!c.whatsapp) &&
      (!filters.noOrg || !c.organization) &&
      (!filters.recent || (c.created_at && now - new Date(c.created_at).getTime() < MONTH_MS)));
  }, [contacts, filters.withEmail, filters.withWhatsapp, filters.noOrg, filters.recent]);

  const allSelected = shown.length > 0 && shown.every((c) => aiSelected.includes(c.id));

  const columns = useMemo<ColumnDef<Contact>[]>(() => [
    {
      id: "select", enableSorting: false, meta: { width: "40px" },
      header: () => (
        <Checkbox aria-label="Select every contact shown" checked={allSelected}
          onChange={(e) => setAiSelected(e.target.checked ? shown.map((c) => c.id) : [])} />
      ),
      cell: (c) => (
        <Checkbox aria-label={`Select ${fullName(c.row.original)}`} checked={aiSelected.includes(c.row.original.id)}
          onChange={(e) => setAiSelected((prev) =>
            e.target.checked ? [...prev, c.row.original.id] : prev.filter((id) => id !== c.row.original.id))} />
      ),
    },
    {
      id: "name", header: "Name", accessorFn: (c) => fullName(c),
      cell: (c) => (
        <div className="min-w-0">
          <Link href={`/crm/contacts/${c.row.original.id}`} className="font-medium text-fg hover:text-brand-ink">{c.getValue<string>()}</Link>
          {c.row.original.position && <p className="truncate text-xs text-fg-muted">{c.row.original.position}</p>}
        </div>
      ),
    },
    {
      id: "organization", header: "Organization", accessorFn: (c) => c.organization?.name ?? "",
      cell: (c) => c.row.original.organization
        ? <Link href={`/crm/organizations/${c.row.original.organization.id}`} className="text-fg hover:text-brand-ink">{c.row.original.organization.name}</Link>
        : <span className="text-fg-subtle">-</span>,
    },
    {
      accessorKey: "email", header: "Email", meta: { hideOnMobile: true },
      cell: (c) => c.getValue<string>()
        ? <a href={`mailto:${c.getValue<string>()}`} className="block max-w-[220px] truncate text-fg hover:text-brand-ink">{c.getValue<string>()}</a>
        : <span className="text-fg-subtle">-</span>,
    },
    {
      id: "reach", header: "Reach", enableSorting: false,
      cell: (c) => {
        const ct = c.row.original;
        return (
          <div className="flex items-center gap-1 text-fg-muted">
            {ct.email && <a href={`mailto:${ct.email}`} aria-label={`Email ${fullName(ct)}`} className="flex size-7 items-center justify-center rounded-lg hover:bg-surface-2 hover:text-fg"><Mail className="size-3.5" aria-hidden /></a>}
            {ct.phone && <a href={`tel:${ct.phone}`} aria-label={`Call ${fullName(ct)}`} className="flex size-7 items-center justify-center rounded-lg hover:bg-surface-2 hover:text-fg"><Phone className="size-3.5" aria-hidden /></a>}
            {ct.whatsapp && <a href={`https://wa.me/${ct.whatsapp.replace(/\D/g, "")}`} target="_blank" rel="noreferrer" aria-label={`WhatsApp ${fullName(ct)}`} className="flex size-7 items-center justify-center rounded-lg hover:bg-surface-2 hover:text-fg"><MessageCircle className="size-3.5" aria-hidden /></a>}
          </div>
        );
      },
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right" },
      cell: (c) => {
        const ct = c.row.original;
        return (
          <Button size="sm" variant="ghost" onClick={() => research(ct)} loading={researching === ct.id}
            title="Research this contact on the web" aria-label={`Research ${fullName(ct)}`}>
            {researching !== ct.id && <Globe2 aria-hidden />} Research
          </Button>
        );
      },
    },
  // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [aiSelected, shown, allSelected, researching]);

  function details(c: Contact) {
    const confirming = confirmDeleteId === c.id;
    return (
      <div className="flex flex-wrap items-start gap-6">
        <KeyValues className="min-w-[260px] flex-1" items={[
          { label: "Position", value: c.position || null },
          { label: "Organization", value: c.organization?.name ?? null },
          { label: "Email", value: c.email ? <a href={`mailto:${c.email}`} className="break-all text-brand-ink hover:underline">{c.email}</a> : null },
          { label: "Phone", value: c.phone ? <a href={`tel:${c.phone}`} className="text-brand-ink hover:underline">{c.phone}</a> : null },
          { label: "WhatsApp", value: c.whatsapp || null },
          ...(c.notes ? [{ label: "Notes", value: c.notes }] : []),
        ]} />
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" variant="primary" asChild><Link href={`/crm/contacts/${c.id}`}>Open contact</Link></Button>
          <Button size="sm" onClick={() => openEdit(c)}><Pencil aria-hidden /> Edit</Button>
          {confirming ? (
            <>
              <span className="text-[13px] text-fg-muted">Delete this contact?</span>
              <Button size="sm" variant="danger" loading={deleting} onClick={() => deleteContact(c.id)}>Delete</Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>Cancel</Button>
            </>
          ) : (
            <IconButton size="sm" variant="danger" icon={Trash2} label={`Delete ${fullName(c)}`} onClick={() => setConfirmDeleteId(c.id)} />
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {aiNotice && (
        <div role="status" className={`rounded-lg border px-4 py-3 text-sm ${
          aiNotice.kind === "error" ? "border-tone-red/30 bg-tone-red-soft text-tone-red"
            : "border-tone-blue/30 bg-tone-blue-soft text-tone-blue"}`}>
          {aiNotice.text}
        </div>
      )}

      <ListFrame>
        <PageHeader flush title="Contacts"
          summary={`${contacts.length} contacts, ${contacts.filter((c) => c.whatsapp).length} on WhatsApp`}
          actions={<Button variant="primary" onClick={openCreate}><Plus aria-hidden /> Add contact</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChips({})} />
        <FilterChips
          chips={CHIPS.map((ch) => ({ id: ch.id, label: ch.label, active: !!chips[ch.id] }))}
          onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id as keyof Filters] }))}
          onClear={() => setChips({})}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)}
            placeholder="Search by name, email, company" aria-label="Search contacts" className="w-64" />
        </FilterChips>
        <AiSelectionBar objectType="contact" selected={aiSelected}
          onClear={() => setAiSelected([])}
          onMessage={(kind, text) => setAiNotice({ kind, text })} />
        <DataTable label="Contacts" columns={columns} data={shown} getRowId={(c) => c.id} loading={loading}
          onExpandedChange={() => setConfirmDeleteId(null)}
          empty={{ icon: UserCircle2, title: "No contacts in this view", description: "Pick another view, or add a contact.",
            action: <Button variant="primary" onClick={openCreate}><Plus aria-hidden /> Add contact</Button> }}
          renderDetails={details}
          footer={<span className="tabular">{shown.length} contact{shown.length !== 1 ? "s" : ""}</span>} />
      </ListFrame>

      <Dialog open={showForm} onOpenChange={(o) => { if (!o) closeForm(); }}
        title={editingContact ? "Edit contact" : "New contact"}
        description={editingContact ? "Update contact details" : "Add a person to your network"}
        footer={<>
          <Button onClick={closeForm}>Cancel</Button>
          <Button variant="primary" type="submit" form="contact-form" loading={saving}>
            {saving ? (editingContact ? "Saving" : "Adding") : (editingContact ? "Save changes" : "Add contact")}
          </Button>
        </>}>
        <form id="contact-form" onSubmit={handleSubmit} className="space-y-4">
          <Field label="Full name" required>
            <Input required value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} placeholder="John Doe" />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Position">
              <Input value={form.position} onChange={(e) => setForm({ ...form, position: e.target.value })} placeholder="Owner, Manager" />
            </Field>
            <Field label="Organization">
              <Combobox value={form.organization_id} source={orgSource}
                placeholder="None" emptyLabel="No organizations found"
                onChange={(id) => setForm({ ...form, organization_id: id })} />
            </Field>
            <Field label="Email">
              <Input type="email" value={form.email} onChange={(e) => setForm({ ...form, email: e.target.value })} />
            </Field>
            <Field label="Phone">
              <Input value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
            </Field>
          </FieldGrid>
          <Field label="WhatsApp">
            <Input value={form.whatsapp} onChange={(e) => setForm({ ...form, whatsapp: e.target.value })} placeholder="+1 555 010 0199" />
          </Field>
          <Field label="Notes">
            <Textarea rows={2} value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} />
          </Field>
        </form>
      </Dialog>
    </div>
  );
}
