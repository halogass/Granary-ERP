"use client";

import { bearerGet, fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useMemo } from "react";
import { useSession } from "next-auth/react";
import { Download, Megaphone, Receipt } from "lucide-react";
import { partnerApi } from "@/lib/api";
import { filenameFromDisposition, saveBlob } from "@/lib/documentLibrary";
import { formatCurrency, sumByCurrency } from "@/lib/currency";
import {
  Button, CountBadge, DataTable, Dialog, Field, Input, KeyValues, ListFrame, PageHeader, Stat, StatGroup, StatusPill,
  Tabs, TabsContent, TabsList, TabsTrigger, Textarea, toast, type ColumnDef,
} from "@/components/ui";

interface MDFRequest {
  id: string;
  title: string;
  partner_name: string;
  activity_type: string;
  requested_amount: number;
  approved_amount: number | null;
  currency: string;
  status: string;
  planned_date: string | null;
  created_at: string;
  description: string | null;
  expected_outcome: string | null;
  claims: MDFClaim[];
}

interface MDFClaim {
  id: string;
  claimed_amount: number;
  status: string;
  proof_file_url: string | null;
  description: string | null;
  created_at: string;
  paid_at: string | null;
}

type ClaimRow = MDFClaim & { request: MDFRequest };

const REQ_LABELS: Record<string, string> = {
  submitted: "Submitted", under_review: "Under review", approved: "Approved", rejected: "Rejected",
  completed: "Completed", draft: "Draft", cancelled: "Cancelled",
};

const CLAIM_LABELS: Record<string, string> = {
  submitted: "Submitted", approved: "Approved", rejected: "Rejected", paid: "Paid",
};

function fmt(d: string) { return new Date(d).toLocaleDateString("en-GB", { day: "2-digit", month: "short", year: "numeric" }); }
function money(n: number, cur: string) { return new Intl.NumberFormat("en-US").format(n) + " " + cur; }

/** A claim's proof through the authorised download route (GET /partner/mdf/claims/{id}/proof). */
async function downloadProof(path: string) {
  try {
    const res = await partnerApi.downloadProof(path);
    const name = filenameFromDisposition(res.headers?.["content-disposition"] as string | undefined, "proof");
    saveBlob(res.data, name);
  } catch {
    toast.error("The proof file could not be downloaded.");
  }
}

export default function PartnerMDFAdminPage() {
  const { data: session } = useSession();
  const [requests, setRequests] = useState<MDFRequest[]>([]);
  const [loading, setLoading] = useState(true);
  const [activeTab, setActiveTab] = useState<"requests" | "claims">("requests");
  const [selected, setSelected] = useState<MDFRequest | null>(null);
  const [reviewAction, setReviewAction] = useState<"approve" | "reject" | null>(null);
  const [approvedAmount, setApprovedAmount] = useState("");
  const [rejectionReason, setRejectionReason] = useState("");
  const [selectedClaim, setSelectedClaim] = useState<{ claim: MDFClaim; request: MDFRequest } | null>(null);
  const [claimAction, setClaimAction] = useState<"approve" | "reject" | "paid" | null>(null);
  const [paymentRef, setPaymentRef] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const token = session?.access_token;

  useEffect(() => {
    if (!token) return;
    fetchAllPages<MDFRequest>(bearerGet("/api/v1/partner/mdf", token))
      .then(({ rows }) => setRequests(rows))
      .catch(() => toast.error("Failed to load MDF data"))
      .finally(() => setLoading(false));
  }, [token]);

  async function reviewRequest() {
    if (!token || !selected || !reviewAction) return;
    setSubmitting(true);
    try {
      const body: Record<string, unknown> = { action: reviewAction };
      if (reviewAction === "approve") body.approved_amount = parseFloat(approvedAmount) || selected.requested_amount;
      if (reviewAction === "reject") body.rejection_reason = rejectionReason;
      const res = await fetch(`/api/v1/partner/mdf/${selected.id}/review`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error();
      const updated = await res.json();
      setRequests(prev => prev.map(r => r.id === selected.id ? { ...r, ...updated } : r));
      setSelected(null);
      setReviewAction(null);
      toast.success(`MDF request ${reviewAction === "approve" ? "approved" : "rejected"}`);
    } catch { toast.error("Review failed"); }
    finally { setSubmitting(false); }
  }

  async function reviewClaim() {
    if (!token || !selectedClaim || !claimAction) return;
    setSubmitting(true);
    try {
      const body: Record<string, unknown> = { action: claimAction };
      if (claimAction === "paid") body.payment_reference = paymentRef;
      const res = await fetch(`/api/v1/partner/mdf/claims/${selectedClaim.claim.id}/review`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error();
      const updated = await res.json();
      setRequests(prev => prev.map(r => r.id === selectedClaim.request.id
        ? { ...r, claims: r.claims.map(c => c.id === selectedClaim.claim.id ? { ...c, ...updated } : c) }
        : r));
      setSelectedClaim(null);
      setClaimAction(null);
      toast.success(`Claim marked as ${claimAction}`);
    } catch { toast.error("Claim review failed"); }
    finally { setSubmitting(false); }
  }

  // MDF requests carry their own currency: grouped by currency, never added across them.
  const approvedTotal = sumByCurrency(requests.filter(r => ["approved", "completed"].includes(r.status)),
    r => r.approved_amount, r => r.currency, "USD");

  const allClaims: ClaimRow[] = requests.flatMap(r => r.claims?.map(c => ({ ...c, request: r })) ?? []);
  const pendingClaims = allClaims.filter(c => c.status === "submitted").length;
  const pendingReview = requests.filter(r => r.status === "submitted").length;

  const requestColumns = useMemo<ColumnDef<MDFRequest>[]>(() => [
    { accessorKey: "partner_name", header: "Partner", cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span> },
    { accessorKey: "activity_type", header: "Activity", meta: { hideOnMobile: true }, cell: (c) => <span className="capitalize text-fg-muted">{c.getValue<string>()}</span> },
    { accessorKey: "requested_amount", header: "Requested", meta: { numeric: true }, cell: (c) => money(c.getValue<number>(), c.row.original.currency) },
    {
      accessorKey: "approved_amount", header: "Approved", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => c.getValue<number | null>() ? money(c.getValue<number>(), c.row.original.currency) : "-",
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="mdf" status={c.getValue<string>()} label={REQ_LABELS[c.getValue<string>()]} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const req = c.row.original;
        return ["submitted", "under_review"].includes(req.status) ? (
          <Button size="sm" variant="primary"
            onClick={() => { setSelected(req); setApprovedAmount(String(req.requested_amount)); setRejectionReason(""); setReviewAction(null); }}>
            Review
          </Button>
        ) : null;
      },
    },
  ], []);

  const claimColumns = useMemo<ColumnDef<ClaimRow>[]>(() => [
    { id: "partner", accessorFn: (c) => c.request.partner_name, header: "Partner", cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span> },
    { id: "request", accessorFn: (c) => c.request.title, header: "MDF request", meta: { hideOnMobile: true }, cell: (c) => <span className="block max-w-[220px] truncate text-fg-muted">{c.getValue<string>()}</span> },
    { accessorKey: "claimed_amount", header: "Claimed", meta: { numeric: true }, cell: (c) => <span className="font-medium">{new Intl.NumberFormat("en-US").format(c.getValue<number>())}</span> },
    {
      id: "proof", header: "Proof", enableSorting: false, meta: { hideOnMobile: true },
      cell: (c) => c.row.original.proof_file_url ? (
        <button type="button" onClick={(e) => { e.stopPropagation(); downloadProof(c.row.original.proof_file_url!); }}
          className="inline-flex items-center gap-1 text-xs text-brand-ink hover:underline">
          Download proof <Download className="size-3" aria-hidden />
        </button>
      ) : <span className="text-xs text-fg-subtle">-</span>,
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="mdf" status={c.getValue<string>()} label={CLAIM_LABELS[c.getValue<string>()]} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const claim = c.row.original;
        if (claim.status === "submitted") {
          return <Button size="sm" variant="primary" onClick={() => { setSelectedClaim({ claim, request: claim.request }); setClaimAction(null); setPaymentRef(""); }}>Review</Button>;
        }
        if (claim.status === "approved") {
          return <Button size="sm" onClick={() => { setSelectedClaim({ claim, request: claim.request }); setClaimAction("paid"); setPaymentRef(""); }}>Mark paid</Button>;
        }
        return null;
      },
    },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Pending review" value={pendingReview} alert={pendingReview > 0} note={pendingReview > 0 ? "requests waiting" : undefined} />
        <Stat label="Pending claims" value={pendingClaims} alert={pendingClaims > 0} note={pendingClaims > 0 ? "claims waiting" : undefined} />
        <Stat label="Total approved" value={formatCurrency(approvedTotal.total, approvedTotal.currency)}
          note={approvedTotal.mixed ? `Mixed currencies: ${approvedTotal.currency} total shown` : undefined} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="MDF review" summary="Approve or reject partner marketing development fund requests and claims." />
        <Tabs value={activeTab} onValueChange={(v) => setActiveTab(v as "requests" | "claims")}>
          <TabsList className="px-4">
            <TabsTrigger value="requests">MDF requests</TabsTrigger>
            <TabsTrigger value="claims">Claims <CountBadge count={pendingClaims} /></TabsTrigger>
          </TabsList>
          <TabsContent value="requests" className="pt-0">
            <DataTable label="MDF requests" columns={requestColumns} data={requests} getRowId={(r) => r.id} loading={loading}
              empty={{ icon: Megaphone, title: "No MDF requests yet", description: "Partners submit requests from the partner portal." }}
              renderDetails={(r) => (
                <div className="space-y-2">
                  <KeyValues cols={3} items={[
                    { label: "Title", value: r.title },
                    { label: "Planned date", value: r.planned_date ? fmt(r.planned_date) : "-" },
                    { label: "Submitted", value: fmt(r.created_at) },
                  ]} />
                  {r.description && <p className="whitespace-pre-wrap text-sm text-fg">{r.description}</p>}
                  {r.expected_outcome && <p className="text-sm text-fg-muted">Expected outcome: {r.expected_outcome}</p>}
                </div>
              )} />
          </TabsContent>
          <TabsContent value="claims" className="pt-0">
            <DataTable label="MDF claims" columns={claimColumns} data={allClaims} getRowId={(r) => r.id} loading={loading}
              empty={{ icon: Receipt, title: "No claims submitted yet", description: "Claims appear once partners report spend on an approved request." }}
              renderDetails={(c) => (
                <div className="space-y-2">
                  <KeyValues cols={3} items={[
                    { label: "MDF request", value: c.request.title },
                    { label: "Submitted", value: fmt(c.created_at) },
                    { label: "Paid", value: c.paid_at ? fmt(c.paid_at) : "-" },
                  ]} />
                  {c.description && <p className="whitespace-pre-wrap text-sm text-fg-muted">{c.description}</p>}
                </div>
              )} />
          </TabsContent>
        </Tabs>
      </ListFrame>

      <Dialog open={!!selected} onOpenChange={(o) => { if (!o) setSelected(null); }} title="Review MDF request"
        description={selected && `${selected.partner_name} · ${selected.activity_type}`}
        footer={<>
          <Button onClick={() => setSelected(null)}>Cancel</Button>
          <Button variant={reviewAction === "reject" ? "danger" : "primary"} onClick={reviewRequest} loading={submitting} disabled={!reviewAction}>
            {submitting ? "Saving" : reviewAction === "approve" ? "Confirm approval" : reviewAction === "reject" ? "Confirm rejection" : "Select action"}
          </Button>
        </>}>
        {selected && (
          <div className="space-y-4 text-sm">
            <KeyValues cols={2} items={[
              { label: "Requested", value: money(selected.requested_amount, selected.currency), numeric: true },
              ...(selected.planned_date ? [{ label: "Planned date", value: fmt(selected.planned_date) }] : []),
            ]} />
            {selected.description && <p className="rounded-lg bg-surface-2 p-3 text-fg">{selected.description}</p>}
            <div className="flex gap-2" role="group" aria-label="Decision">
              <Button className="flex-1" aria-pressed={reviewAction === "approve"} variant={reviewAction === "approve" ? "primary" : "secondary"}
                onClick={() => setReviewAction("approve")}>Approve</Button>
              <Button className="flex-1" aria-pressed={reviewAction === "reject"} variant={reviewAction === "reject" ? "danger" : "secondary"}
                onClick={() => setReviewAction("reject")}>Reject</Button>
            </div>
            {reviewAction === "approve" && (
              <Field label={`Approved amount (${selected.currency})`}>
                <Input numeric type="number" value={approvedAmount} onChange={e => setApprovedAmount(e.target.value)} />
              </Field>
            )}
            {reviewAction === "reject" && (
              <Field label="Rejection reason" required>
                <Textarea rows={2} value={rejectionReason} onChange={e => setRejectionReason(e.target.value)} />
              </Field>
            )}
          </div>
        )}
      </Dialog>

      <Dialog open={!!selectedClaim} onOpenChange={(o) => { if (!o) { setSelectedClaim(null); setClaimAction(null); } }} title="Review claim"
        description={selectedClaim?.request.partner_name}
        footer={<>
          <Button onClick={() => { setSelectedClaim(null); setClaimAction(null); }}>Cancel</Button>
          {claimAction && (
            <Button variant={claimAction === "reject" ? "danger" : "primary"} onClick={reviewClaim} loading={submitting}>
              {submitting ? "Saving" : `Confirm ${claimAction === "paid" ? "paid" : claimAction}`}
            </Button>
          )}
        </>}>
        {selectedClaim && (
          <div className="space-y-4 text-sm">
            <KeyValues cols={2} items={[
              { label: "Claim amount", value: new Intl.NumberFormat("en-US").format(selectedClaim.claim.claimed_amount), numeric: true },
              ...(selectedClaim.request.approved_amount
                ? [{ label: "Approved budget", value: money(selectedClaim.request.approved_amount, selectedClaim.request.currency), numeric: true }]
                : []),
            ]} />
            {selectedClaim.claim.description && <p className="rounded-lg bg-surface-2 p-3 text-fg">{selectedClaim.claim.description}</p>}
            {selectedClaim.claim.proof_file_url && (
              <Button className="w-full" onClick={() => downloadProof(selectedClaim.claim.proof_file_url!)}>
                <Download aria-hidden /> Download proof document
              </Button>
            )}
            {claimAction === "paid" && (
              <Field label="Payment reference">
                <Input value={paymentRef} onChange={e => setPaymentRef(e.target.value)} placeholder="Bank transfer ref, receipt #, etc." />
              </Field>
            )}
            {!claimAction && (
              <div className="flex gap-2">
                <Button className="flex-1" onClick={() => setClaimAction("approve")}>Approve</Button>
                <Button className="flex-1" variant="danger" onClick={() => setClaimAction("reject")}>Reject</Button>
                <Button className="flex-1" onClick={() => setClaimAction("paid")}>Mark paid</Button>
              </div>
            )}
          </div>
        )}
      </Dialog>
    </div>
  );
}
