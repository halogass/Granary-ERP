"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import { vendorApi } from "@/lib/api";
import { useCurrencySettings, currencyOptions } from "@/hooks/useCurrencySettings";
import { extractApiError } from "@/lib/apiError";
import { ClipboardList, Plus, Trash2 } from "lucide-react";
import { GRANARY_DEPARTMENTS } from "@/lib/departments";
import {
  Button, DataTable, Dialog, ErrorToast, Field, FieldGrid, IconButton, Input, KeyValues, ListFrame, PageHeader,
  SavedViewTabs, Select, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface ContractorRequest {
  id: string;
  scope_description: string;
  vendor_type?: string;
  department?: string;
  status: string;
  estimated_value?: number | null;
  currency: string;
  needed_by?: string | null;
  requested_by: string;
  rejection_reason?: string | null;
  created_at?: string;
}

const STATUS_LABELS: Record<string, string> = {
  pending_approval: "Pending approval", approved: "Approved", rejected: "Rejected",
  in_progress: "In progress", fulfilled: "Fulfilled", cancelled: "Cancelled",
};

const CONTRACTOR_TYPES = [
  "assembly_subcontractor", "component_supplier", "professional_services",
  "facility_services", "logistics", "professional_nontech", "it_infrastructure", "temp_staff",
];

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "pending_approval", label: "Pending approval", filters: { status: "pending_approval" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "rejected", label: "Rejected", filters: { status: "rejected" } },
];

function fmtValue(val?: number | null, currency = "USD") {
  if (val == null) return "-";
  return new Intl.NumberFormat("en-US", { style: "currency", currency, maximumFractionDigits: 0 }).format(val);
}

const fmtDate = (d?: string | null) =>
  d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-";

export default function ContractorRequestsPage() {
  const views = useSavedViews<Filters>("vendor.contractor-requests", VIEWS);
  const tab = views.active?.filters.status ?? "";
  const [requests, setRequests] = useState<ContractorRequest[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [rejectModal, setRejectModal] = useState<ContractorRequest | null>(null);
  const [actionError, setActionError] = useState("");
  const [rejectNotes, setRejectNotes] = useState("");
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const { functional: functionalCurrency, active: activeCurrencies } = useCurrencySettings();
  const [form, setForm] = useState({
    scope_description: "", vendor_type: "", department: "",
    estimated_value: "", currency: "", needed_by: "",
  });

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params = tab ? { status: tab } : undefined;
      const res = await vendorApi.listContractorRequests(params);
      setRequests(res.data.items ?? res.data ?? []);
    } catch { setRequests([]); } finally { setLoading(false); }
  }, [tab]);

  useEffect(() => { load(); }, [load]);

  async function create() {
    if (!form.scope_description.trim()) return;
    setSaving(true);
    try {
      await vendorApi.createContractorRequest({
        scope_description: form.scope_description.trim(),
        vendor_type: form.vendor_type || undefined,
        department: form.department.trim() || undefined,
        estimated_value: form.estimated_value ? parseFloat(form.estimated_value) : undefined,
        currency: form.currency,
        needed_by: form.needed_by || undefined,
      });
      setShowModal(false);
      setForm({ scope_description: "", vendor_type: "", department: "", estimated_value: "", currency: functionalCurrency, needed_by: "" });
      await load();
    } finally { setSaving(false); }
  }

  async function approve(id: string) {
    setSaving(true);
    try {
      await vendorApi.approveContractorRequest(id, { action: "approved" });
      window.dispatchEvent(new Event("approvals:changed"));
      await load();
    } catch (e) {
      setActionError(extractApiError(e, "Could not approve the request."));
    } finally { setSaving(false); }
  }

  async function reject() {
    if (!rejectModal) return;
    setSaving(true);
    try {
      await vendorApi.approveContractorRequest(rejectModal.id, { action: "rejected", notes: rejectNotes });
      setRejectModal(null);
      setRejectNotes("");
      window.dispatchEvent(new Event("approvals:changed"));
      await load();
    } catch (e) {
      setActionError(extractApiError(e, "Could not reject the request."));
      setRejectModal(null);
    } finally { setSaving(false); }
  }

  async function deleteRequest(id: string) {
    try {
      await vendorApi.deleteContractorRequest(id);
      setConfirmDeleteId(null);
      setRequests((prev) => prev.filter((r) => r.id !== id));
    } catch { /* keep */ }
  }

  const counts: Record<string, number> = {
    all: requests.length,
    pending_approval: requests.filter((r) => r.status === "pending_approval").length,
    approved: requests.filter((r) => r.status === "approved").length,
    rejected: requests.filter((r) => r.status === "rejected").length,
  };

  const columns = useMemo<ColumnDef<ContractorRequest>[]>(() => [
    {
      accessorKey: "scope_description", header: "Scope",
      cell: (c) => <p className="line-clamp-1 max-w-md font-medium text-fg">{c.getValue<string>()}</p>,
    },
    {
      accessorKey: "vendor_type", header: "Type", meta: { hideOnMobile: true },
      cell: (c) => <span className="capitalize text-fg-muted">{(c.getValue<string | undefined>() ?? "-").replace(/_/g, " ")}</span>,
    },
    {
      accessorKey: "department", header: "Department", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string | undefined>() || "-"}</span>,
    },
    {
      accessorKey: "needed_by", header: "Needed by", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{fmtDate(c.getValue<string | null>())}</span>,
    },
    {
      accessorKey: "estimated_value", header: "Est. value", meta: { numeric: true },
      cell: (c) => fmtValue(c.row.original.estimated_value, c.row.original.currency),
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="contractor_request" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} />,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const req = c.row.original;
        const isPending = req.status === "pending_approval";
        const canDelete = ["pending_approval", "rejected", "cancelled"].includes(req.status);
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            {isPending && (
              <>
                <Button size="sm" variant="primary" onClick={() => approve(req.id)} disabled={saving}>Approve</Button>
                <Button size="sm" variant="danger" onClick={() => { setRejectModal(req); setRejectNotes(""); }}>Reject</Button>
              </>
            )}
            {canDelete && (confirmDeleteId === req.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deleteRequest(req.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </>
            ) : (
              <IconButton size="sm" variant="ghost" icon={Trash2} label="Delete request" onClick={() => setConfirmDeleteId(req.id)} />
            ))}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, saving]);

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Contractor requests"
          summary={`${requests.length} request${requests.length !== 1 ? "s" : ""}. Contractor and vendor engagement approvals.`}
          actions={<Button variant="primary" onClick={() => { setForm((f) => ({ ...f, currency: f.currency || functionalCurrency })); setShowModal(true); }}><Plus aria-hidden /> New request</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? {}}
          counts={counts} alertIds={counts.pending_approval > 0 ? ["pending_approval"] : undefined} />
        <DataTable label="Contractor requests" columns={columns} data={requests} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: ClipboardList, title: "No contractor requests", description: "Pick another view, or create a request." }}
          renderDetails={(req) => (
            <div className="space-y-3">
              <p className="whitespace-pre-wrap text-sm text-fg">{req.scope_description}</p>
              <KeyValues cols={3} items={[
                { label: "Requested by", value: req.requested_by },
                { label: "Type", value: req.vendor_type ? req.vendor_type.replace(/_/g, " ") : "-" },
                { label: "Department", value: req.department || "-" },
                { label: "Needed by", value: fmtDate(req.needed_by) },
                { label: "Estimated value", value: fmtValue(req.estimated_value, req.currency), numeric: true },
                { label: "Created", value: fmtDate(req.created_at) },
              ]} />
              {req.rejection_reason && <p className="text-sm text-tone-red">Reason: {req.rejection_reason}</p>}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="New contractor request"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!form.scope_description.trim()} loading={saving}>
            {saving ? "Submitting" : "Submit request"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Scope description" required className="md:col-span-2">
            <Textarea value={form.scope_description} onChange={(e) => setForm({ ...form, scope_description: e.target.value })} rows={3} />
          </Field>
          <Field label="Contractor type">
            <Select value={form.vendor_type} onChange={(e) => setForm({ ...form, vendor_type: e.target.value })} placeholder="Select type"
              options={CONTRACTOR_TYPES.map((t) => ({ value: t, label: t.replace(/_/g, " ") }))} />
          </Field>
          <Field label="Department">
            <Select value={form.department} onChange={(e) => setForm({ ...form, department: e.target.value })} placeholder="Select"
              options={GRANARY_DEPARTMENTS.map((d) => ({ value: d, label: d }))} />
          </Field>
          <Field label="Needed by">
            <Input type="date" value={form.needed_by} onChange={(e) => setForm({ ...form, needed_by: e.target.value })} />
          </Field>
          <Field label="Currency">
            <Select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}
              options={currencyOptions(activeCurrencies, form.currency)} />
          </Field>
          <Field label={`Estimated value (${form.currency})`} className="md:col-span-2">
            <Input numeric type="number" min="0" value={form.estimated_value} placeholder="0"
              onChange={(e) => setForm({ ...form, estimated_value: e.target.value })} />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={!!rejectModal} onOpenChange={(o) => { if (!o) { setRejectModal(null); setRejectNotes(""); } }}
        title="Reject request" size="sm" description={<span className="line-clamp-2">{rejectModal?.scope_description}</span>}
        footer={<>
          <Button onClick={() => { setRejectModal(null); setRejectNotes(""); }}>Cancel</Button>
          <Button variant="danger" onClick={reject} loading={saving}>{saving ? "Rejecting" : "Reject"}</Button>
        </>}>
        <Field label="Rejection reason">
          <Textarea value={rejectNotes} onChange={(e) => setRejectNotes(e.target.value)} rows={3}
            placeholder="Explain why this request is being rejected" />
        </Field>
      </Dialog>
      <ErrorToast message={actionError} onClose={() => setActionError("")} />
    </>
  );
}
