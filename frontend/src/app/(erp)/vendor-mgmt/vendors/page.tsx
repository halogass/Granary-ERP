"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { vendorApi } from "@/lib/api";
import { Truck, Plus, Search, Star, MapPin, Mail, ShieldCheck, ShieldOff, Loader2, Trash2, Edit3 } from "lucide-react";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import VendorDocuments from "@/components/vendor/VendorDocuments";
import {
  Button, Checkbox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader,
  Pill, SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

// Shape of GET /vendor/vendors.
interface Vendor {
  id: string;
  name: string;
  vendor_type: string;
  status: string;
  contact_name?: string | null;
  contact_email?: string | null;
  country?: string | null;
  rating?: number | null;
  has_portal_access?: boolean;
}

// Matches models/vendor.py Vendor.vendor_type (same list as the vendor self-registration page).
const VENDOR_TYPES = [
  { value: "professional_services", label: "Professional Services" },
  { value: "it_infrastructure", label: "IT Infrastructure" },
  { value: "component_supplier", label: "Component Supplier" },
  { value: "assembly_subcontractor", label: "Assembly Subcontractor" },
  { value: "facility_services", label: "Facility Services" },
  { value: "logistics", label: "Logistics / Freight" },
  { value: "temp_staff", label: "Temporary Staffing" },
  { value: "professional_nontech", label: "Professional Non-Technical" },
];
const TYPE_LABEL: Record<string, string> = Object.fromEntries(VENDOR_TYPES.map((t) => [t.value, t.label]));
const TOP_RATING = 4;

// Shape of GET /vendor/vendors/{id}: the fields the edit dialog changes.
interface VendorDetail {
  id: string;
  name: string;
  vendor_type: string;
  contact_name?: string | null;
  contact_email?: string | null;
  contact_phone?: string | null;
  contact_whatsapp?: string | null;
  country?: string | null;
  address?: string | null;
  tax_id?: string | null;
  bank_name?: string | null;
  bank_account_name?: string | null;
  bank_account_number?: string | null;
  notes?: string | null;
  withholding_applicable?: boolean | null;
  withholding_rate?: number | null;
}

const EDIT_TEXT_FIELDS = [
  "name", "vendor_type", "contact_name", "contact_email", "contact_phone", "contact_whatsapp", "country", "address",
  "tax_id", "bank_name", "bank_account_name", "bank_account_number", "notes",
] as const;
type EditForm = Record<(typeof EDIT_TEXT_FIELDS)[number], string> & { withholding_applicable: boolean; withholding_rate: string };

function editFormOf(v: VendorDetail): EditForm {
  const text = Object.fromEntries(EDIT_TEXT_FIELDS.map((k) => [k, (v[k] as string | null | undefined) ?? ""]));
  return {
    ...(text as Record<(typeof EDIT_TEXT_FIELDS)[number], string>),
    withholding_applicable: !!v.withholding_applicable,
    withholding_rate: v.withholding_rate == null ? "" : String(v.withholding_rate),
  };
}

type InviteState = "idle" | "loading" | "sent" | "has_access" | "no_email";

type Filters = { status?: string; top?: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "top", label: "Top rated", filters: { top: true } },
];

export default function VendorsPage() {
  const views = useSavedViews<Filters>("vendor.vendors", VIEWS);
  const [typeChip, setTypeChip] = useState("");
  const [vendors, setVendors] = useState<Vendor[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState({ name: "", vendor_type: "professional_services", contact_name: "", contact_email: "", country: "", tax_id: "" });
  const [saving, setSaving] = useState(false);
  const [inviteStates, setInviteStates] = useState<Record<string, InviteState>>({});
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const { holds } = usePermissions();
  const canManage = holds("vendor_mgmt:manage_vendors");
  // Edit dialog: the vendor's full record is loaded when it opens.
  const [editId, setEditId] = useState<string | null>(null);
  const [editForm, setEditForm] = useState<EditForm | null>(null);
  const [editSaving, setEditSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<Vendor>(vendorApi.listVendors);
      setVendors(rows);
    } catch { setVendors([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const vf = views.active?.filters ?? {};
  const filters: Filters = vf;
  const filtered = vendors.filter((v) =>
    (!vf.status || v.status === vf.status) &&
    (!vf.top || (v.rating ?? 0) >= TOP_RATING) &&
    (!typeChip || v.vendor_type === typeChip) &&
    (!search ||
      v.name.toLowerCase().includes(search.toLowerCase()) ||
      (v.contact_email ?? "").toLowerCase().includes(search.toLowerCase())));

  async function deleteVendor(id: string) {
    try {
      await vendorApi.deleteVendor(id);
      setConfirmDeleteId(null);
      setVendors((prev) => prev.filter((v) => v.id !== id));
    } catch { /* keep */ }
  }

  async function create() {
    if (!form.name.trim()) return;
    setSaving(true);
    try {
      await vendorApi.createVendor({
        name: form.name.trim(),
        vendor_type: form.vendor_type,
        contact_name: form.contact_name.trim() || undefined,
        contact_email: form.contact_email.trim() || undefined,
        country: form.country.trim() || undefined,
        tax_id: form.tax_id.trim() || undefined,
      });
      setShowModal(false);
      setForm({ name: "", vendor_type: "professional_services", contact_name: "", contact_email: "", country: "", tax_id: "" });
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to add vendor"));
    } finally { setSaving(false); }
  }

  async function openEdit(id: string) {
    setEditId(id);
    setEditForm(null);
    try {
      const { data } = await vendorApi.getVendor(id);
      setEditForm(editFormOf(data as VendorDetail));
    } catch (err) {
      toast.error(extractApiError(err, "Failed to load the vendor"));
      setEditId(null);
    }
  }

  async function saveEdit() {
    if (!editId || !editForm || !editForm.name.trim()) return;
    const rate = editForm.withholding_rate.trim();
    if (rate && (Number.isNaN(Number(rate)) || Number(rate) < 0 || Number(rate) > 100)) {
      toast.error("Withholding rate must be a percentage from 0 to 100");
      return;
    }
    setEditSaving(true);
    try {
      const body: Record<string, unknown> = Object.fromEntries(EDIT_TEXT_FIELDS.map((k) => [k, editForm[k].trim()]));
      body.withholding_applicable = editForm.withholding_applicable;
      // Empty rate while switched on: the server takes Settings > Procurement's default rate.
      if (rate) body.withholding_rate = Number(rate);
      await vendorApi.updateVendor(editId, body);
      setEditId(null);
      toast.success("Vendor saved");
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to save the vendor"));
    } finally { setEditSaving(false); }
  }

  const setEdit = (k: keyof EditForm, v: string | boolean) => setEditForm((f) => (f ? { ...f, [k]: v } : f));

  async function invite(vendor: Vendor) {
    if (!vendor.contact_email) {
      setInviteStates((s) => ({ ...s, [vendor.id]: "no_email" }));
      return;
    }
    setInviteStates((s) => ({ ...s, [vendor.id]: "loading" }));
    try {
      await vendorApi.inviteVendor(vendor.id);
      setInviteStates((s) => ({ ...s, [vendor.id]: "sent" }));
    } catch (err: unknown) {
      const status = (err as { response?: { status?: number } })?.response?.status;
      setInviteStates((s) => ({ ...s, [vendor.id]: status === 409 ? "has_access" : "no_email" }));
    }
  }

  const preferred = vendors.filter((v) => (v.rating ?? 0) >= TOP_RATING).length;
  const active = vendors.filter((v) => v.status === "active").length;
  const typesInUse = Array.from(new Set(vendors.map((v) => v.vendor_type)));

  function portal(v: Vendor) {
    const inv = inviteStates[v.id] ?? (v.has_portal_access ? "has_access" : "idle");
    if (inv === "sent") return <Pill tone="blue" icon={ShieldCheck}>Invite sent</Pill>;
    if (inv === "has_access") return <Pill tone="blue" icon={ShieldCheck}>Has access</Pill>;
    if (inv === "no_email") return <Pill tone="orange" icon={ShieldOff}>No email on file</Pill>;
    if (inv === "loading") return <span className="inline-flex items-center gap-1.5 text-xs text-fg-muted"><Loader2 className="size-3.5 animate-spin" aria-hidden /> Sending</span>;
    if (!canManage) return <Pill icon={ShieldOff}>No access</Pill>;
    return <Button size="sm" onClick={() => invite(v)}><ShieldCheck aria-hidden /> Grant portal access</Button>;
  }

  const columns = useMemo<ColumnDef<Vendor>[]>(() => [
    {
      accessorKey: "name", header: "Vendor",
      cell: (c) => (
        <span className="inline-flex items-center gap-1.5 font-medium text-fg">
          {c.getValue<string>()}
          {(c.row.original.rating ?? 0) >= TOP_RATING && <Star className="size-3.5 fill-current text-tone-orange" aria-label="Top rated" />}
        </span>
      ),
    },
    {
      accessorKey: "vendor_type", header: "Type",
      cell: (c) => <Pill>{TYPE_LABEL[c.getValue<string>()] ?? c.getValue<string>().replace(/_/g, " ")}</Pill>,
    },
    {
      accessorKey: "contact_name", header: "Contact", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() || "-"}</span>,
    },
    {
      accessorKey: "country", header: "Country", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | null>() || "-"}</span>,
    },
    { id: "portal", header: "Portal", enableSorting: false, meta: { hideOnMobile: true }, cell: (c) => portal(c.row.original) },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill status={c.getValue<string>()} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const v = c.row.original;
        return confirmDeleteId === v.id ? (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            <span className="text-xs font-semibold text-tone-red">Delete?</span>
            <Button size="sm" variant="danger" onClick={() => deleteVendor(v.id)}>Yes</Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
          </div>
        ) : (
          <div className="flex items-center justify-end gap-0.5">
            {canManage && <IconButton size="sm" variant="ghost" icon={Edit3} label={`Edit ${v.name}`} onClick={() => openEdit(v.id)} />}
            {canManage && <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${v.name}`} onClick={() => setConfirmDeleteId(v.id)} />}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, inviteStates, canManage]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={vendors.length} />
        <Stat label="Active" value={active} />
        <Stat label="Top rated" value={preferred} note={`rating ${TOP_RATING} or more`} />
        <Stat label="Types" value={typesInUse.length} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Vendors and contractors"
          summary={`${vendors.length} vendor${vendors.length !== 1 ? "s" : ""}, ${active} active. External suppliers, freelancers and service providers.`}
          actions={canManage ? <Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Add vendor</Button> : undefined} />
        <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setTypeChip("")} />
        <FilterChips
          chips={typesInUse.map((t) => ({ id: t, label: TYPE_LABEL[t] ?? t.replace(/_/g, " "), active: typeChip === t }))}
          onToggle={(id) => setTypeChip((cur) => (cur === id ? "" : id))} onClear={() => setTypeChip("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search vendors" aria-label="Search vendors" className="w-56" />
        </FilterChips>
        <DataTable label="Vendors" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: Truck, title: "No vendors yet", description: "Add a vendor, or pick another view." }}
          renderDetails={(v) => (
            <div className="space-y-4">
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Type", value: TYPE_LABEL[v.vendor_type] ?? v.vendor_type },
                { label: "Contact", value: v.contact_name || "-" },
                { label: "Email", value: v.contact_email ? <span className="inline-flex items-center gap-1"><Mail className="size-3" aria-hidden />{v.contact_email}</span> : "-" },
                { label: "Country", value: v.country ? <span className="inline-flex items-center gap-1"><MapPin className="size-3" aria-hidden />{v.country}</span> : "-" },
                { label: "Rating", value: v.rating ?? "-", numeric: true },
              ]} />
              <div className="md:hidden">{portal(v)}</div>
              {canManage && <Button size="sm" onClick={() => openEdit(v.id)}><Edit3 aria-hidden /> Edit vendor</Button>}
            </div>
            {/* Listing, verifying and downloading a vendor's documents all need vendor_mgmt:manage_vendors. */}
            {canManage && <VendorDocuments vendorId={v.id} />}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Add vendor"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!form.name.trim()} loading={saving}>{saving ? "Adding" : "Add vendor"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Company name" required>
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
          </Field>
          <Field label="Type">
            <Select value={form.vendor_type} onChange={(e) => setForm({ ...form, vendor_type: e.target.value })} options={VENDOR_TYPES} />
          </Field>
          <Field label="Contact name">
            <Input value={form.contact_name} onChange={(e) => setForm({ ...form, contact_name: e.target.value })} />
          </Field>
          <Field label="Contact email">
            <Input type="email" value={form.contact_email} onChange={(e) => setForm({ ...form, contact_email: e.target.value })} />
          </Field>
          <Field label="Country">
            <Input value={form.country} onChange={(e) => setForm({ ...form, country: e.target.value })} />
          </Field>
          <Field label="Tax ID">
            <Input value={form.tax_id} onChange={(e) => setForm({ ...form, tax_id: e.target.value })} className="font-mono" />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={!!editId} onOpenChange={(o) => { if (!o) setEditId(null); }} title="Edit vendor" size="lg"
        footer={<>
          <Button onClick={() => setEditId(null)}>Cancel</Button>
          <Button variant="primary" onClick={saveEdit} disabled={!editForm || !editForm.name.trim()} loading={editSaving}>Save changes</Button>
        </>}>
        {!editForm ? (
          <p className="inline-flex items-center gap-1.5 text-sm text-fg-muted"><Loader2 className="size-4 animate-spin" aria-hidden /> Loading</p>
        ) : (
          <div className="space-y-4">
            <FieldGrid cols={2}>
              <Field label="Company name" required>
                <Input value={editForm.name} onChange={(e) => setEdit("name", e.target.value)} />
              </Field>
              <Field label="Type">
                <Select value={editForm.vendor_type} onChange={(e) => setEdit("vendor_type", e.target.value)}
                  options={VENDOR_TYPES.some((t) => t.value === editForm.vendor_type) ? VENDOR_TYPES
                    : [...VENDOR_TYPES, { value: editForm.vendor_type, label: editForm.vendor_type.replace(/_/g, " ") }]} />
              </Field>
              <Field label="Contact name">
                <Input value={editForm.contact_name} onChange={(e) => setEdit("contact_name", e.target.value)} />
              </Field>
              <Field label="Contact email">
                <Input type="email" value={editForm.contact_email} onChange={(e) => setEdit("contact_email", e.target.value)} />
              </Field>
              <Field label="Phone">
                <Input value={editForm.contact_phone} onChange={(e) => setEdit("contact_phone", e.target.value)} />
              </Field>
              <Field label="WhatsApp">
                <Input value={editForm.contact_whatsapp} onChange={(e) => setEdit("contact_whatsapp", e.target.value)} />
              </Field>
              <Field label="Country">
                <Input value={editForm.country} onChange={(e) => setEdit("country", e.target.value)} />
              </Field>
              <Field label="Tax ID">
                <Input value={editForm.tax_id} onChange={(e) => setEdit("tax_id", e.target.value)} className="font-mono" />
              </Field>
              <Field label="Address" className="md:col-span-2">
                <Textarea rows={2} value={editForm.address} onChange={(e) => setEdit("address", e.target.value)} />
              </Field>
              <Field label="Bank name">
                <Input value={editForm.bank_name} onChange={(e) => setEdit("bank_name", e.target.value)} />
              </Field>
              <Field label="Account name">
                <Input value={editForm.bank_account_name} onChange={(e) => setEdit("bank_account_name", e.target.value)} />
              </Field>
              <Field label="Account number" className="md:col-span-2">
                <Input value={editForm.bank_account_number} onChange={(e) => setEdit("bank_account_number", e.target.value)} className="font-mono" />
              </Field>
            </FieldGrid>
            <div className="space-y-3 rounded-lg border border-line p-3">
              <Checkbox checked={editForm.withholding_applicable} label="Withhold tax on this vendor's invoices"
                hint="Work orders and vendor invoices then deduct withholding tax at this rate."
                onChange={(e) => setEdit("withholding_applicable", e.target.checked)} />
              {editForm.withholding_applicable && (
                <Field label="Withholding rate" hint="Percent. Leave empty to use the default rate in Settings, Procurement.">
                  <Input type="number" min={0} max={100} step="0.001" inputMode="decimal" className="w-32"
                    value={editForm.withholding_rate} onChange={(e) => setEdit("withholding_rate", e.target.value)} />
                </Field>
              )}
            </div>
            <Field label="Notes">
              <Textarea rows={3} value={editForm.notes} onChange={(e) => setEdit("notes", e.target.value)} />
            </Field>
          </div>
        )}
      </Dialog>
    </div>
  );
}
