"use client";

import { useState, useEffect, useMemo, useRef } from "react";
import { useSession } from "next-auth/react";
import { Plus, Megaphone, Receipt } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, Input, KeyValues, ListFrame, PageHeader, Select, Stat, StatGroup,
  StatusPill, Textarea, toast, type ColumnDef,
} from "@/components/ui";
import { cn } from "@/lib/utils";

interface MDFRequest {
  id: string;
  title: string;
  activity_type: string;
  requested_amount: number;
  approved_amount: number | null;
  currency: string;
  planned_date: string | null;
  status: string;
  created_at: string;
  rejection_reason: string | null;
}

interface MDFClaim {
  id: string;
  mdf_request_id: string;
  mdf_request_title?: string;
  claimed_amount: number;
  status: string;
  created_at: string;
  paid_at: string | null;
}

const STATUS_LABEL: Record<string, string> = {
  draft: "Draft", submitted: "Submitted", under_review: "Under review", approved: "Approved", rejected: "Rejected",
  completed: "Completed", cancelled: "Cancelled", paid: "Paid",
};

const ACTIVITY_TYPES = ["event", "advertising", "content", "demo", "other"];

export default function PartnerMDFPage() {
  const { data: session } = useSession();
  const [activeTab, setActiveTab] = useState<"requests" | "claims">("requests");
  const [requests, setRequests] = useState<MDFRequest[]>([]);
  const [claims, setClaims] = useState<MDFClaim[]>([]);
  const [loading, setLoading] = useState(true);
  const [showRequestForm, setShowRequestForm] = useState(false);
  const [showClaimForm, setShowClaimForm] = useState(false);
  const [claimForRequest, setClaimForRequest] = useState<MDFRequest | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const proofFileRef = useRef<HTMLInputElement>(null);

  // Request form state
  const [reqForm, setReqForm] = useState({ title: "", activity_type: "event", requested_amount: "", currency: "USD", planned_date: "", planned_location: "", description: "", expected_outcome: "" });
  // Claim form state
  const [claimForm, setClaimForm] = useState({ claimed_amount: "", description: "" });

  const token = session?.access_token;

  useEffect(() => {
    if (!token) return;
    Promise.all([
      fetch("/api/v1/partner/portal/mdf", { headers: { Authorization: `Bearer ${token}` } }).then(r => r.json()),
      fetch("/api/v1/partner/portal/mdf/claims", { headers: { Authorization: `Bearer ${token}` } }).then(r => r.json()),
    ]).then(([reqs, cls]) => { setRequests(reqs); setClaims(cls); }).catch(() => toast.error("Failed to load MDF data")).finally(() => setLoading(false));
  }, [token]);

  async function submitRequest(e: React.FormEvent) {
    e.preventDefault();
    if (!token) return;
    setSubmitting(true);
    try {
      const res = await fetch("/api/v1/partner/portal/mdf", {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({ ...reqForm, requested_amount: parseFloat(reqForm.requested_amount) }),
      });
      if (!res.ok) throw new Error();
      const created = await res.json();
      setRequests(prev => [created, ...prev]);
      setShowRequestForm(false);
      setReqForm({ title: "", activity_type: "event", requested_amount: "", currency: "USD", planned_date: "", planned_location: "", description: "", expected_outcome: "" });
      toast.success("MDF request submitted");
    } catch { toast.error("Failed to submit request"); }
    finally { setSubmitting(false); }
  }

  async function submitClaim(e: React.FormEvent) {
    e.preventDefault();
    if (!token || !claimForRequest) return;
    setSubmitting(true);
    try {
      const fd = new FormData();
      fd.append("claimed_amount", claimForRequest.approved_amount ? Math.min(parseFloat(claimForm.claimed_amount), claimForRequest.approved_amount).toString() : claimForm.claimed_amount);
      fd.append("description", claimForm.description);
      if (proofFileRef.current?.files?.[0]) fd.append("proof_file", proofFileRef.current.files[0]);
      const res = await fetch(`/api/v1/partner/portal/mdf/${claimForRequest.id}/claims`, {
        method: "POST",
        headers: { Authorization: `Bearer ${token}` },
        body: fd,
      });
      if (!res.ok) throw new Error();
      const created = await res.json();
      setClaims(prev => [{ ...created, mdf_request_title: claimForRequest.title }, ...prev]);
      setShowClaimForm(false);
      setClaimForRequest(null);
      setClaimForm({ claimed_amount: "", description: "" });
      toast.success("Claim submitted");
    } catch { toast.error("Failed to submit claim"); }
    finally { setSubmitting(false); }
  }

  function fmt(d: string) { return new Date(d).toLocaleDateString("en-GB", { day: "2-digit", month: "short", year: "numeric" }); }
  function money(n: number, cur: string) { return new Intl.NumberFormat("en-US").format(n) + " " + cur; }

  const totalApproved = requests.filter(r => r.status === "approved" || r.status === "completed").reduce((s, r) => s + (r.approved_amount ?? 0), 0);
  const totalPaid = claims.filter(c => c.status === "paid").reduce((s, c) => s + c.claimed_amount, 0);
  const totalPending = requests.filter(r => ["submitted", "under_review"].includes(r.status)).length;

  const openClaim = (req: MDFRequest) => { setClaimForRequest(req); setShowClaimForm(true); };
  const closeClaim = () => { setShowClaimForm(false); setClaimForRequest(null); };

  const requestColumns = useMemo<ColumnDef<MDFRequest>[]>(() => [
    { accessorKey: "title", header: "Title", cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
    { accessorKey: "activity_type", header: "Activity", meta: { hideOnMobile: true }, cell: (c) => <span className="capitalize">{c.getValue<string>()}</span> },
    { accessorKey: "requested_amount", header: "Requested", meta: { numeric: true }, cell: ({ row }) => money(row.original.requested_amount, row.original.currency) },
    { accessorKey: "approved_amount", header: "Approved", meta: { numeric: true, hideOnMobile: true },
      cell: ({ row }) => row.original.approved_amount ? money(row.original.approved_amount, row.original.currency) : "-" },
    { accessorKey: "planned_date", header: "Planned", meta: { numeric: true, hideOnMobile: true }, cell: (c) => c.getValue<string>() ? fmt(c.getValue<string>()) : "-" },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="mdf" status={c.getValue<string>()} label={STATUS_LABEL[c.getValue<string>()]} /> },
    { id: "action", header: "", enableSorting: false,
      cell: ({ row }) => row.original.status === "approved"
        ? <Button size="sm" onClick={() => openClaim(row.original)}>Submit claim</Button>
        : null },
  ], []);

  const claimColumns = useMemo<ColumnDef<MDFClaim>[]>(() => [
    { accessorKey: "mdf_request_title", header: "MDF request", cell: (c) => c.getValue<string>() || "-" },
    { accessorKey: "claimed_amount", header: "Claimed", meta: { numeric: true }, cell: (c) => new Intl.NumberFormat("en-US").format(c.getValue<number>()) },
    { accessorKey: "created_at", header: "Submitted", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmt(c.getValue<string>()) },
    { accessorKey: "paid_at", header: "Paid", meta: { numeric: true, hideOnMobile: true }, cell: (c) => c.getValue<string>() ? fmt(c.getValue<string>()) : "-" },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="mdf" status={c.getValue<string>()} label={STATUS_LABEL[c.getValue<string>()]} /> },
  ], []);

  const tabs = [{ id: "requests", label: "Requests", count: requests.length }, { id: "claims", label: "Claims", count: claims.length }] as const;

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Approved budget" value={totalApproved > 0 ? new Intl.NumberFormat("en-US").format(totalApproved) : "-"} note="Across all approved" />
        <Stat label="Paid out" value={totalPaid > 0 ? new Intl.NumberFormat("en-US").format(totalPaid) : "-"} note="Reimbursed" />
        <Stat label="Pending review" value={String(totalPending)} note="request(s) awaiting review" />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="MDF program" summary="Marketing development funds: request co-marketing budget."
          actions={<Button variant="primary" onClick={() => setShowRequestForm(true)}><Plus aria-hidden /> New request</Button>} />
        <div role="tablist" aria-label="MDF" className="flex gap-0.5 overflow-x-auto border-b border-line bg-surface px-2">
          {tabs.map(t => (
            <button key={t.id} type="button" role="tab" aria-selected={activeTab === t.id} onClick={() => setActiveTab(t.id)}
              className={cn("flex h-11 items-center gap-1.5 border-b-2 px-3 text-sm transition-colors md:h-10",
                activeTab === t.id ? "border-brand font-semibold text-fg" : "border-transparent text-fg-muted hover:text-fg")}>
              {t.label}
              <span className="text-xs text-fg-muted tabular">{t.count}</span>
            </button>
          ))}
        </div>
        {activeTab === "requests" ? (
          <DataTable label="MDF requests" columns={requestColumns} data={requests} getRowId={(r) => r.id} loading={loading}
            empty={{ icon: Megaphone, title: "No MDF requests yet", description: "Submit your first marketing budget request." }}
            renderDetails={(req) => (
              <div className="space-y-3">
                <KeyValues cols={3} items={[
                  { label: "Activity", value: <span className="capitalize">{req.activity_type}</span> },
                  { label: "Requested", value: money(req.requested_amount, req.currency), numeric: true },
                  { label: "Submitted", value: fmt(req.created_at) },
                ]} />
                {req.rejection_reason && (
                  <div className="rounded-lg border border-tone-red/30 bg-tone-red-soft p-3 text-[13px] text-tone-red">
                    <p className="font-semibold">Reason for rejection</p>
                    <p className="mt-1">{req.rejection_reason}</p>
                  </div>
                )}
                {req.status === "approved" && <Button size="sm" variant="primary" onClick={() => openClaim(req)}>Submit claim</Button>}
              </div>
            )} />
        ) : (
          <DataTable label="MDF claims" columns={claimColumns} data={claims} getRowId={(c) => c.id} loading={loading}
            empty={{ icon: Receipt, title: "No claims submitted yet", description: "Claims can be submitted after a request is approved." }} />
        )}
      </ListFrame>

      <Dialog open={showRequestForm} onOpenChange={setShowRequestForm} title="New MDF request" dismissable={!submitting}
        footer={<>
          <Button onClick={() => setShowRequestForm(false)}>Cancel</Button>
          <Button type="submit" form="mdf-request-form" variant="primary" loading={submitting}>{submitting ? "Submitting..." : "Submit request"}</Button>
        </>}>
        <form id="mdf-request-form" onSubmit={submitRequest}>
          <FieldGrid cols={2}>
            <Field label="Activity title" required className="md:col-span-2">
              <Input required value={reqForm.title} onChange={e => setReqForm(p => ({ ...p, title: e.target.value }))} placeholder="e.g. Regional tech conference Q3 2026" />
            </Field>
            <Field label="Activity type" required>
              <Select required value={reqForm.activity_type} onChange={e => setReqForm(p => ({ ...p, activity_type: e.target.value }))}
                options={ACTIVITY_TYPES.map(t => ({ value: t, label: t.charAt(0).toUpperCase() + t.slice(1) }))} />
            </Field>
            <Field label="Currency">
              <Select value={reqForm.currency} onChange={e => setReqForm(p => ({ ...p, currency: e.target.value }))}
                options={["USD", "EUR", "GBP", "JPY"].map(c => ({ value: c, label: c }))} />
            </Field>
            <Field label="Requested amount" required>
              <Input required type="number" min="1" numeric value={reqForm.requested_amount} onChange={e => setReqForm(p => ({ ...p, requested_amount: e.target.value }))} placeholder="10000000" />
            </Field>
            <Field label="Planned date">
              <Input type="date" value={reqForm.planned_date} onChange={e => setReqForm(p => ({ ...p, planned_date: e.target.value }))} />
            </Field>
            <Field label="Location" className="md:col-span-2">
              <Input value={reqForm.planned_location} onChange={e => setReqForm(p => ({ ...p, planned_location: e.target.value }))} placeholder="City, country or online" />
            </Field>
            <Field label="Description" required className="md:col-span-2">
              <Textarea required rows={3} value={reqForm.description} onChange={e => setReqForm(p => ({ ...p, description: e.target.value }))} placeholder="Describe the marketing activity..." />
            </Field>
            <Field label="Expected outcome" className="md:col-span-2">
              <Textarea rows={2} value={reqForm.expected_outcome} onChange={e => setReqForm(p => ({ ...p, expected_outcome: e.target.value }))} placeholder="Expected leads, brand exposure, etc." />
            </Field>
          </FieldGrid>
        </form>
      </Dialog>

      <Dialog open={showClaimForm && !!claimForRequest} onOpenChange={(o) => { if (!o) closeClaim(); }} title="Submit claim"
        description={claimForRequest?.title} size="sm" dismissable={!submitting}
        footer={<>
          <Button onClick={closeClaim}>Cancel</Button>
          <Button type="submit" form="mdf-claim-form" variant="primary" loading={submitting}>{submitting ? "Submitting..." : "Submit claim"}</Button>
        </>}>
        {claimForRequest && (
          <form id="mdf-claim-form" onSubmit={submitClaim} className="space-y-4">
            {claimForRequest.approved_amount && (
              <div className="rounded-lg bg-tone-blue-soft px-3 py-2.5 text-sm text-tone-blue">
                <span className="font-semibold">Approved budget: </span>
                <span className="tabular">{money(claimForRequest.approved_amount, claimForRequest.currency)}</span>
              </div>
            )}
            <Field label="Claim amount" required>
              <Input required type="number" min="1" numeric value={claimForm.claimed_amount} onChange={e => setClaimForm(p => ({ ...p, claimed_amount: e.target.value }))} />
            </Field>
            <Field label="Upload proof" required hint="Invoice, receipt, or campaign report (PDF, image, zip).">
              <input ref={proofFileRef} required type="file" accept=".pdf,.jpg,.jpeg,.png,.zip"
                className="block w-full rounded-lg border border-line-strong bg-surface px-3 py-2 text-sm text-fg file:mr-3 file:rounded-md file:border-0 file:bg-surface-2 file:px-3 file:py-1 file:text-xs file:font-medium file:text-fg" />
            </Field>
            <Field label="Description">
              <Textarea rows={2} value={claimForm.description} onChange={e => setClaimForm(p => ({ ...p, description: e.target.value }))} placeholder="Brief description of spend..." />
            </Field>
          </form>
        )}
      </Dialog>
    </div>
  );
}
