"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { AlertTriangle, Ban, MessageSquareReply, Pencil, Send, Trash2 } from "lucide-react";
import { hrApi } from "@/lib/api";
import { formatCurrency } from "@/lib/currency";
import { extractApiError } from "@/lib/apiError";
import ReasonModal from "@/components/ui/ReasonModal";
import ApprovalPanel from "@/components/approvals/ApprovalPanel";
import OfferLetterFormModal, { OFFER_STATUS_LABELS, type OfferLetter } from "@/components/hr/OfferLetterFormModal";
import {
  Button, Card, CardBody, CardHeader, Dialog, EmptyState, Field, KeyValues, PageHeader, Skeleton, StatusPill, Textarea,
} from "@/components/ui";
import ChoiceButtons from "@/components/hr/ChoiceButtons";

const RESPONSES = [
  { value: "accepted", label: "Accepted" },
  { value: "declined", label: "Declined" },
  { value: "countered", label: "Countered" },
];

export default function OfferLetterDetailPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const [offer, setOffer] = useState<OfferLetter | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const [approvalKey, setApprovalKey] = useState(0);
  const [editing, setEditing] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [responding, setResponding] = useState(false);
  const [response, setResponse] = useState({ response: "accepted", notes: "" });

  const load = useCallback(async () => {
    try {
      const res = await hrApi.getOfferLetter(id);
      setOffer(res.data);
    } catch (e) {
      setNotice(extractApiError(e, "Could not load the offer letter."));
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => { load(); }, [load]);

  async function run(action: () => Promise<unknown>, fallback: string) {
    setBusy(true);
    setNotice("");
    try {
      await action();
      await load();
    } catch (e) {
      setNotice(extractApiError(e, fallback));
    } finally {
      setBusy(false);
      setApprovalKey((k) => k + 1);
      window.dispatchEvent(new Event("approvals:changed"));
    }
  }

  if (loading) return <div className="space-y-4"><Skeleton className="h-8 w-64" /><Skeleton className="h-48 w-full" /></div>;
  if (!offer) return (
    <EmptyState icon={AlertTriangle} title={notice || "Offer letter not found."}
      action={<Button asChild><Link href="/hr/offer-letters">Back to offer letters</Link></Button>} />
  );

  const date = (d: string | null) => (d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-");

  return (
    <div className="mx-auto max-w-4xl">
      <PageHeader title={offer.applicant_name ?? offer.number}
        back={{ href: "/hr/offer-letters", label: "Offer letters" }}
        status={<StatusPill family="offer" status={offer.status} label={OFFER_STATUS_LABELS[offer.status]} />}
        summary={<span><span className="font-mono">{offer.number}</span>, {offer.position_title}{offer.department ? `, ${offer.department}` : ""}</span>}
        actions={<>
          {["draft", "cancelled"].includes(offer.status) && (
            <Button variant="danger" disabled={busy}
              onClick={() => run(async () => { await hrApi.deleteOfferLetter(offer.id); router.push("/hr/offer-letters"); }, "Could not delete.")}>
              <Trash2 aria-hidden /> Delete
            </Button>
          )}
          {["draft", "sent", "countered"].includes(offer.status) && (
            <Button onClick={() => setCancelling(true)} disabled={busy}><Ban aria-hidden /> Cancel offer</Button>
          )}
          {offer.status === "draft" && (
            <>
              <Button onClick={() => setEditing(true)} disabled={busy}><Pencil aria-hidden /> Edit</Button>
              <Button variant="primary" onClick={() => run(() => hrApi.sendOfferLetter(offer.id), "Could not send the offer.")} disabled={busy}>
                <Send aria-hidden /> Issue to candidate
              </Button>
            </>
          )}
          {(offer.status === "sent" || offer.status === "countered") && (
            <Button variant="primary" onClick={() => setResponding(true)} disabled={busy}>
              <MessageSquareReply aria-hidden /> Record response
            </Button>
          )}
        </>} />

      {notice && (
        <div role="alert" className="mb-4 flex items-center gap-3 rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-4 py-3">
          <AlertTriangle className="size-4 shrink-0 text-tone-orange" aria-hidden />
          <p className="flex-1 text-sm font-medium text-tone-orange">{notice}</p>
          <Button size="sm" variant="ghost" onClick={() => setNotice("")}>Dismiss</Button>
        </div>
      )}

      <div className="space-y-4">
        <ApprovalPanel entityType="offer_letter" entityId={offer.id} refreshKey={approvalKey} onChange={load} />

        <Card>
          <CardHeader title="Offer" description={offer.applicant_email ?? undefined} />
          <CardBody>
            <KeyValues cols={3} items={[
              { label: "Gross salary per month", value: offer.salary_gross_monthly != null ? formatCurrency(offer.salary_gross_monthly, offer.salary_currency) : "-", numeric: true },
              { label: "Employment type", value: <span className="capitalize">{offer.employment_type?.replace("_", " ") ?? "-"}</span> },
              { label: "Probation", value: offer.probation_months != null ? `${offer.probation_months} month${offer.probation_months === 1 ? "" : "s"}` : "-" },
              { label: "Start date", value: date(offer.start_date) },
              { label: "Respond by", value: date(offer.acceptance_deadline) },
              { label: "Issued", value: offer.sent_at ? date(offer.sent_at) : "Not yet" },
            ]} />
          </CardBody>
        </Card>

        {(offer.benefits || offer.other_terms) && (
          <Card>
            <CardHeader title="Terms" />
            <CardBody className="grid grid-cols-1 gap-5 sm:grid-cols-2">
              {offer.benefits && (
                <div>
                  <p className="mb-1 text-[13px] text-fg-muted">Benefits</p>
                  <p className="whitespace-pre-line break-words text-sm text-fg">{offer.benefits}</p>
                </div>
              )}
              {offer.other_terms && (
                <div>
                  <p className="mb-1 text-[13px] text-fg-muted">Other terms</p>
                  <p className="whitespace-pre-line break-words text-sm text-fg">{offer.other_terms}</p>
                </div>
              )}
            </CardBody>
          </Card>
        )}

        {offer.candidate_response_notes && (
          <Card>
            <CardHeader title="Candidate response" description={offer.responded_at ? date(offer.responded_at) : undefined} />
            <CardBody><p className="whitespace-pre-line break-words text-sm text-fg">{offer.candidate_response_notes}</p></CardBody>
          </Card>
        )}
      </div>

      {editing && (
        <OfferLetterFormModal offer={offer} onClose={() => setEditing(false)}
          onSaved={(updated) => { setEditing(false); setOffer(updated); setApprovalKey((k) => k + 1); }} />
      )}

      {cancelling && (
        <ReasonModal title="Cancel offer" label="Reason *" confirmText="Cancel offer"
          onClose={() => setCancelling(false)}
          onConfirm={async (reason) => {
            try {
              await hrApi.cancelOfferLetter(offer.id, reason);
              setCancelling(false);
              await load();
              setApprovalKey((k) => k + 1);
            } catch (e) {
              throw new Error(extractApiError(e, "Could not cancel the offer."));
            }
          }} />
      )}

      <Dialog open={responding} onOpenChange={setResponding} size="sm" title="Candidate response"
        footer={<>
          <Button onClick={() => setResponding(false)}>Close</Button>
          <Button variant="primary" disabled={busy}
            onClick={() => { setResponding(false); run(() => hrApi.recordOfferResponse(offer.id, { response: response.response, notes: response.notes.trim() || undefined }), "Could not record the response."); }}>
            Save response
          </Button>
        </>}>
        <div className="space-y-4">
          <ChoiceButtons label="Response" value={response.response} options={RESPONSES}
            onChange={(v) => setResponse((s) => ({ ...s, response: v }))} />
          <Field label="Notes">
            <Textarea rows={3} value={response.notes} placeholder="e.g. counter-offer details"
              onChange={(e) => setResponse((s) => ({ ...s, notes: e.target.value }))} />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
