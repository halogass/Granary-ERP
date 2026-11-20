"use client";

import { bearerGet, fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useMemo } from "react";
import { useSession } from "next-auth/react";
import { Plus } from "lucide-react";
import { cn } from "@/lib/utils";
import { usePartnerOptions } from "@/lib/partnerOptions";
import {
  Button, DataTable, Dialog, Field, FilterChips, Input, KeyValues, ListFrame, PageHeader, Pill, SavedViewTabs, Select,
  Stat, StatGroup, StatusPill, Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface RegistrationRequest {
  id: string;
  portal_type: "vendor" | "partner";
  company_name: string;
  contact_name: string;
  email: string;
  phone: string | null;
  vendor_type: string | null;
  partner_type: string | null;
  country: string | null;
  website: string | null;
  notes: string | null;
  status: string;
  created_at: string;
  rejection_reason?: string;
}

interface PortalUser {
  id: string;
  name: string;
  email: string;
  portal_type: "vendor" | "partner";
  is_active: boolean;
  company_name: string | null;
  created_at: string;
}

interface CreateUserForm {
  portal_type: "vendor" | "partner";
  name: string;
  email: string;
  company_name: string;
  phone: string;
  type: string;
  country: string;
}

const VENDOR_TYPES = ["freelancer", "contractor", "supplier", "consultant", "it_service", "logistics", "maintenance", "other"];

type Filters = { tab: "pending" | "active" };

const VIEWS: SavedView<Filters>[] = [
  { id: "pending", label: "Pending", filters: { tab: "pending" } },
  { id: "active", label: "Active users", filters: { tab: "active" } },
];

const CREATE_FIELDS = [
  { label: "Full name", key: "name", type: "text", required: true },
  { label: "Email", key: "email", type: "email", required: true },
  { label: "Company name", key: "company_name", type: "text", required: true },
  { label: "Phone", key: "phone", type: "text", required: false },
  { label: "Country", key: "country", type: "text", required: false },
];

const portalPill = (t: string) => <Pill tone={t === "vendor" ? "orange" : "blue"} className="capitalize">{t}</Pill>;

export default function PortalUsersPage() {
  const { data: session } = useSession();
  const views = useSavedViews<Filters>("admin.portal-users", VIEWS);
  const activeTab = views.active?.filters.tab ?? "pending";
  const [requests, setRequests] = useState<RegistrationRequest[]>([]);
  const [users, setUsers] = useState<PortalUser[]>([]);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<RegistrationRequest | null>(null);
  const [rejectionReason, setRejectionReason] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [createForm, setCreateForm] = useState<CreateUserForm>({ portal_type: "vendor", name: "", email: "", company_name: "", phone: "", type: "", country: "" });
  const [submitting, setSubmitting] = useState(false);
  const partnerOptions = usePartnerOptions();
  const [portalFilter, setPortalFilter] = useState<"all" | "vendor" | "partner">("all");

  const token = session?.access_token;

  useEffect(() => {
    if (!token) return;
    Promise.all([
      fetchAllPages<RegistrationRequest>(bearerGet("/api/v1/portal/registrations", token)),
      fetchAllPages<PortalUser>(bearerGet("/api/v1/users", token), { portal_type: "vendor,partner" }),
    ]).then(([reqs, usr]) => { setRequests(reqs.rows); setUsers(usr.rows); })
      .catch(() => toast.error("Failed to load portal users"))
      .finally(() => setLoading(false));
  }, [token]);

  async function approve(req: RegistrationRequest) {
    if (!token) return;
    setSubmitting(true);
    try {
      const res = await fetch(`/api/v1/portal/registrations/${req.id}/approve`, {
        method: "POST", headers: { Authorization: `Bearer ${token}` },
      });
      if (!res.ok) {
        const detail = await res.json().catch(() => null);
        throw new Error(detail?.detail ?? "Approval failed");
      }
      setRequests(prev => prev.map(r => r.id === req.id ? { ...r, status: "approved" } : r));
      setSelected(null);
      toast.success(`${req.contact_name} approved. Welcome email sent.`);
    } catch (err) { toast.error(err instanceof Error ? err.message : "Approval failed"); }
    finally { setSubmitting(false); }
  }

  async function reject(req: RegistrationRequest) {
    if (!token || !rejectionReason.trim()) { toast.error("Please enter a rejection reason"); return; }
    setSubmitting(true);
    try {
      const res = await fetch(`/api/v1/portal/registrations/${req.id}/reject`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({ reason: rejectionReason }),
      });
      if (!res.ok) throw new Error();
      setRequests(prev => prev.map(r => r.id === req.id ? { ...r, status: "rejected", rejection_reason: rejectionReason } : r));
      setSelected(null);
      setRejectionReason("");
      toast.success("Registration rejected");
    } catch { toast.error("Rejection failed"); }
    finally { setSubmitting(false); }
  }

  async function createAccount(e: React.FormEvent) {
    e.preventDefault();
    if (!token) return;
    setSubmitting(true);
    try {
      const body = {
        portal_type: createForm.portal_type,
        company_name: createForm.company_name,
        contact_name: createForm.name,
        email: createForm.email,
        phone: createForm.phone,
        [`${createForm.portal_type}_type`]: createForm.type,
        country: createForm.country,
      };
      const endpoint = createForm.portal_type === "vendor" ? "/api/v1/portal/register/vendor" : "/api/v1/portal/register/partner";
      const regRes = await fetch(endpoint, {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!regRes.ok) throw new Error();
      const reg = await regRes.json();
      // Immediately approve: the registration exists either way, so report approval failures
      const apprRes = await fetch(`/api/v1/portal/registrations/${reg.id}/approve`, {
        method: "POST", headers: { Authorization: `Bearer ${token}` },
      });
      if (!apprRes.ok) {
        const detail = await apprRes.json().catch(() => null);
        toast.error(detail?.detail ?? "Registration created but approval failed, approve it from the list");
        setShowCreate(false);
        return;
      }
      toast.success(`Account created for ${createForm.name}. Credentials emailed.`);
      setShowCreate(false);
      setCreateForm({ portal_type: "vendor", name: "", email: "", company_name: "", phone: "", type: "", country: "" });
    } catch { toast.error("Failed to create account"); }
    finally { setSubmitting(false); }
  }

  function fmt(d: string) { return new Date(d).toLocaleDateString("en-GB", { day: "2-digit", month: "short", year: "numeric" }); }

  const pendingRequests = requests.filter(r => r.status === "pending");
  const filteredUsers = portalFilter === "all" ? users : users.filter(u => u.portal_type === portalFilter);

  const requestColumns = useMemo<ColumnDef<RegistrationRequest>[]>(() => [
    { accessorKey: "company_name", header: "Company", cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span> },
    {
      accessorKey: "contact_name", header: "Contact",
      cell: (c) => <div><p className="text-fg">{c.row.original.contact_name}</p><p className="text-xs text-fg-muted">{c.row.original.email}</p></div>,
    },
    { id: "type", accessorFn: (r) => r.vendor_type || r.partner_type || "", header: "Type", meta: { hideOnMobile: true }, cell: (c) => <span className="capitalize text-fg-muted">{c.getValue<string>().replace("_", " ") || "-"}</span> },
    { accessorKey: "portal_type", header: "Portal", cell: (c) => portalPill(c.getValue<string>()) },
    { accessorKey: "created_at", header: "Submitted", meta: { numeric: true, hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{fmt(c.getValue<string>())}</span> },
    { id: "review", header: "", enableSorting: false, meta: { align: "right", width: "1%" }, cell: (c) => <StatusPill family="approval" status="awaiting_me" label="Review" /> },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], []);

  const userColumns = useMemo<ColumnDef<PortalUser>[]>(() => [
    { accessorKey: "name", header: "Name", cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span> },
    { accessorKey: "email", header: "Email", cell: (c) => <span className="text-fg-muted">{c.getValue<string>()}</span> },
    { accessorKey: "company_name", header: "Company", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<string>() || "-"}</span> },
    { accessorKey: "portal_type", header: "Portal", cell: (c) => portalPill(c.getValue<string>()) },
    { accessorKey: "is_active", header: "Status", cell: (c) => <StatusPill status={c.getValue<boolean>() ? "active" : "inactive"} /> },
    { accessorKey: "created_at", header: "Joined", meta: { numeric: true, hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{fmt(c.getValue<string>())}</span> },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Pending approval" value={pendingRequests.length} alert={pendingRequests.length > 0} />
        <Stat label="Vendor users" value={users.filter(u => u.portal_type === "vendor").length} />
        <Stat label="Partner users" value={users.filter(u => u.portal_type === "partner").length} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Portal user management" summary="Manage vendor and partner portal access."
          actions={<Button variant="primary" onClick={() => setShowCreate(true)}><Plus aria-hidden /> Create account</Button>} />
        <SavedViewTabs state={views} counts={{ pending: pendingRequests.length }} alertIds={["pending"]} onSelect={() => setSelected(null)} />

        {activeTab === "pending" ? (
          <DataTable label="Pending registrations" columns={requestColumns} data={pendingRequests} getRowId={(r) => r.id} loading={loading}
            expandedId={selected?.id ?? null}
            onExpandedChange={(id) => { setSelected(pendingRequests.find((r) => r.id === id) ?? null); setRejectionReason(""); }}
            empty={{ title: "No pending registrations." }}
            renderDetails={(req) => (
              <div className="space-y-4">
                <p className="text-xs capitalize text-fg-muted">{req.portal_type} portal request</p>
                <KeyValues cols={3} items={[
                  { label: "Company", value: req.company_name },
                  { label: "Contact", value: req.contact_name },
                  { label: "Email", value: req.email },
                  { label: "Phone", value: req.phone || "-" },
                  { label: "Type", value: req.vendor_type || req.partner_type || "-" },
                  { label: "Country", value: req.country || "-" },
                  { label: "Website", value: req.website || "-" },
                ]} />
                {req.notes && (
                  <div>
                    <p className="mb-1 text-[13px] text-fg-muted">Notes</p>
                    <p className="rounded-lg bg-surface-2 p-3 text-[13px] text-fg">{req.notes}</p>
                  </div>
                )}
                <div className="flex flex-wrap items-end gap-3">
                  <Field label="Rejection reason" hint="Required to reject." className="min-w-[260px] flex-1">
                    <Textarea rows={2} value={rejectionReason} onChange={e => setRejectionReason(e.target.value)} />
                  </Field>
                  <div className="flex gap-2">
                    <Button variant="danger" onClick={() => reject(req)} disabled={submitting}>Reject</Button>
                    <Button variant="primary" onClick={() => approve(req)} loading={submitting}>
                      {submitting ? "Processing..." : "Approve & send credentials"}
                    </Button>
                  </div>
                </div>
              </div>
            )} />
        ) : (
          <>
            <FilterChips
              chips={[{ id: "vendor", label: "Vendor", active: portalFilter === "vendor" }, { id: "partner", label: "Partner", active: portalFilter === "partner" }]}
              onToggle={(id) => setPortalFilter((cur) => (cur === id ? "all" : id as "vendor" | "partner"))}
              onClear={() => setPortalFilter("all")} />
            <DataTable label="Portal users" columns={userColumns} data={filteredUsers} getRowId={(r) => r.id} loading={loading}
              empty={{ title: "No portal users found." }} />
          </>
        )}
      </ListFrame>

      <Dialog open={showCreate} onOpenChange={setShowCreate} title="Create portal account"
        footer={<>
          <Button onClick={() => setShowCreate(false)}>Cancel</Button>
          <Button type="submit" form="create-portal-account" variant="primary" loading={submitting}>
            {submitting ? "Creating..." : "Create & send credentials"}
          </Button>
        </>}>
        <form id="create-portal-account" onSubmit={createAccount} className="space-y-4">
          <fieldset>
            <legend className="mb-1.5 text-[13px] font-medium text-fg">Portal type</legend>
            <div className="flex gap-2" role="radiogroup">
              {["vendor", "partner"].map(pt => (
                <button key={pt} type="button" role="radio" aria-checked={createForm.portal_type === pt}
                  onClick={() => setCreateForm(p => ({ ...p, portal_type: pt as "vendor" | "partner", type: "" }))}
                  className={cn("min-h-11 flex-1 rounded-lg border text-sm font-medium capitalize transition-colors md:min-h-9",
                    createForm.portal_type === pt ? "border-brand bg-brand/[0.06] text-brand-ink" : "border-line text-fg-muted hover:bg-surface-2")}>
                  {pt}
                </button>
              ))}
            </div>
          </fieldset>
          {CREATE_FIELDS.map(f => (
            <Field key={f.key} label={f.label} required={f.required}>
              <Input type={f.type} value={(createForm as unknown as Record<string, string>)[f.key]}
                onChange={e => setCreateForm(p => ({ ...p, [f.key]: e.target.value }))} />
            </Field>
          ))}
          <Field label={createForm.portal_type === "vendor" ? "Vendor type" : "Partner type"}>
            <Select value={createForm.type} onChange={e => setCreateForm(p => ({ ...p, type: e.target.value }))} className="capitalize"
              placeholder="Select type" options={createForm.portal_type === "vendor"
                ? VENDOR_TYPES.map(t => ({ value: t, label: t.replace("_", " ") }))
                : partnerOptions.partner_types.map(t => ({ value: t.value, label: t.label }))} />
          </Field>
        </form>
      </Dialog>
    </div>
  );
}
