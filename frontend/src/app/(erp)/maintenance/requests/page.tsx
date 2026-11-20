"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, AlertCircle } from "lucide-react";
import api from "@/lib/api";
import {
  Button, DataTable, Dialog, Field, Input, KeyValues, ListFrame, PageHeader, SavedViewTabs, Select, StatusPill,
  Textarea, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Request {
  id: string; number: string; equipment_id: string; equipment_name: string | null;
  description: string; priority: string; status: string;
  submitted_at: string | null; reviewed_at: string | null; rejection_reason: string | null;
}
interface Equipment { id: string; name: string; code: string; }

const STATUS_LABELS: Record<string, string> = {
  submitted: "Submitted", under_review: "Under review", approved: "Approved", rejected: "Rejected",
};

const PRIORITY_OPTIONS = [
  { value: "low", label: "Low" }, { value: "normal", label: "Normal" },
  { value: "high", label: "High" }, { value: "critical", label: "Critical" },
];

const EMPTY_FORM = { equipment_id: "", description: "", priority: "normal" };
const EMPTY_REVIEW = { action: "approve", rejection_reason: "", work_type: "corrective", scheduled_date: "" };

type Filters = { status: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "submitted", label: "Submitted", filters: { status: "submitted" } },
  { id: "under_review", label: "Under review", filters: { status: "under_review" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "rejected", label: "Rejected", filters: { status: "rejected" } },
  { id: "all", label: "All", filters: { status: "" } },
];

const fmtDate = (d: string | null) => d ? new Date(d).toLocaleDateString("en-GB", { day: "2-digit", month: "short" }) : "-";

export default function RequestsPage() {
  const views = useSavedViews<Filters>("maintenance.requests", VIEWS);
  const statusFilter = views.active?.filters.status ?? "submitted";
  const [requests, setRequests] = useState<Request[]>([]);
  const [equipment, setEquipment] = useState<Equipment[]>([]);
  const [loading, setLoading] = useState(true);

  const [showCreate, setShowCreate] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [creating, setCreating] = useState(false);

  const [reviewingId, setReviewingId] = useState<string | null>(null);
  const [reviewForm, setReviewForm] = useState(EMPTY_REVIEW);
  const [reviewing, setReviewing] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (statusFilter) params.status = statusFilter;
      const [rRes, eRes] = await Promise.all([
        fetchAllPages<Request>((p) => api.get("/maintenance/requests", { params: p }), params),
        fetchAllPages<Equipment>((p) => api.get("/maintenance/equipment", { params: p })),
      ]);
      setRequests(rRes.rows);
      setEquipment(eRes.rows);
    } catch { setRequests([]); } finally { setLoading(false); }
  }, [statusFilter]);

  useEffect(() => { load(); }, [load]);

  async function createRequest() {
    if (!form.equipment_id || !form.description.trim()) return toast.error("Equipment and description required");
    setCreating(true);
    try {
      await api.post("/maintenance/requests", { equipment_id: form.equipment_id, description: form.description.trim(), priority: form.priority });
      toast.success("Request submitted");
      setShowCreate(false);
      setForm(EMPTY_FORM);
      load();
    } catch { toast.error("Failed to submit request"); } finally { setCreating(false); }
  }

  async function submitReview() {
    if (!reviewingId) return;
    if (reviewForm.action === "reject" && !reviewForm.rejection_reason.trim()) return toast.error("Please provide a rejection reason");
    setReviewing(true);
    try {
      await api.patch(`/maintenance/requests/${reviewingId}/review`, {
        action: reviewForm.action,
        rejection_reason: reviewForm.rejection_reason || null,
        work_type: reviewForm.work_type,
        scheduled_date: reviewForm.scheduled_date || null,
      });
      toast.success(reviewForm.action === "approve" ? "Request approved, work order created" : "Request rejected");
      setReviewingId(null);
      load();
    } catch { toast.error("Failed to submit review"); } finally { setReviewing(false); }
  }

  const pending = requests.filter(r => ["submitted", "under_review"].includes(r.status)).length;

  const columns = useMemo<ColumnDef<Request>[]>(() => [
    { accessorKey: "number", header: "Request", cell: (c) => <span className="font-mono text-[13px] font-medium">{c.getValue<string>()}</span> },
    {
      id: "equipment", accessorFn: (r) => r.equipment_name ?? "-", header: "Equipment",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.getValue<string>()}</p>
          <p className="line-clamp-1 text-xs text-fg-muted">{c.row.original.description}</p>
        </div>
      ),
    },
    { accessorKey: "priority", header: "Priority", cell: (c) => <StatusPill family="priority" status={c.getValue<string>()} /> },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="maintenance_request" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} />,
    },
    {
      accessorKey: "submitted_at", header: "Submitted", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted tabular">{fmtDate(c.getValue<string | null>())}</span>,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => ["submitted", "under_review"].includes(c.row.original.status)
        ? <Button size="sm" variant="primary" onClick={() => { setReviewingId(c.row.original.id); setReviewForm(EMPTY_REVIEW); }}>Review</Button>
        : null,
    },
  ], []);

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Maintenance requests" summary={pending > 0 ? `${pending} pending review` : "Self-service request queue"}
          actions={<Button variant="primary" onClick={() => { setForm(EMPTY_FORM); setShowCreate(true); }}><Plus aria-hidden /> Submit request</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters ?? { status: statusFilter }} />
        <DataTable label="Maintenance requests" columns={columns} data={requests} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: AlertCircle, title: "No requests", description: "Pick another view, or submit a request." }}
          renderDetails={(r) => (
            <div className="space-y-2">
              <p className="whitespace-pre-wrap text-sm text-fg">{r.description}</p>
              <KeyValues cols={3} items={[
                { label: "Submitted", value: fmtDate(r.submitted_at) },
                { label: "Reviewed", value: fmtDate(r.reviewed_at) },
              ]} />
              {r.rejection_reason && <p className="text-sm italic text-tone-red">Rejected: {r.rejection_reason}</p>}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showCreate} onOpenChange={setShowCreate} title="Submit maintenance request"
        footer={<>
          <Button onClick={() => setShowCreate(false)}>Cancel</Button>
          <Button variant="primary" onClick={createRequest} loading={creating}>{creating ? "Submitting" : "Submit"}</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Equipment" required>
            <Select value={form.equipment_id} onChange={e => setForm({ ...form, equipment_id: e.target.value })} placeholder="Select equipment"
              options={equipment.filter(e => e.id).map(e => ({ value: e.id, label: `${e.name} (${e.code})` }))} />
          </Field>
          <Field label="Priority">
            <Select value={form.priority} onChange={e => setForm({ ...form, priority: e.target.value })} options={PRIORITY_OPTIONS} />
          </Field>
          <Field label="Description" required>
            <Textarea value={form.description} onChange={e => setForm({ ...form, description: e.target.value })} rows={3}
              placeholder="Describe the issue or maintenance needed" />
          </Field>
        </div>
      </Dialog>

      <Dialog open={!!reviewingId} onOpenChange={(o) => { if (!o) setReviewingId(null); }} title="Review maintenance request"
        footer={<>
          <Button onClick={() => setReviewingId(null)}>Cancel</Button>
          <Button variant={reviewForm.action === "approve" ? "primary" : "danger"} onClick={submitReview} loading={reviewing}>
            {reviewing ? "Saving" : reviewForm.action === "approve" ? "Approve and create WO" : "Reject"}
          </Button>
        </>}>
        <div className="space-y-4">
          <div className="flex gap-2" role="group" aria-label="Decision">
            {["approve", "reject"].map(a => (
              <Button key={a} className="flex-1" aria-pressed={reviewForm.action === a}
                variant={reviewForm.action === a ? (a === "approve" ? "primary" : "danger") : "secondary"}
                onClick={() => setReviewForm({ ...reviewForm, action: a })}>
                {a === "approve" ? "Approve" : "Reject"}
              </Button>
            ))}
          </div>
          {reviewForm.action === "approve" && (
            <>
              <Field label="Work type">
                <Select value={reviewForm.work_type} onChange={e => setReviewForm({ ...reviewForm, work_type: e.target.value })}
                  options={[{ value: "corrective", label: "Corrective (repair)" }, { value: "preventive", label: "Preventive" }]} />
              </Field>
              <Field label="Schedule date">
                <Input type="date" value={reviewForm.scheduled_date} onChange={e => setReviewForm({ ...reviewForm, scheduled_date: e.target.value })} />
              </Field>
            </>
          )}
          {reviewForm.action === "reject" && (
            <Field label="Rejection reason" required>
              <Textarea value={reviewForm.rejection_reason} onChange={e => setReviewForm({ ...reviewForm, rejection_reason: e.target.value })} rows={2}
                placeholder="Explain why this request is rejected" />
            </Field>
          )}
        </div>
      </Dialog>
    </>
  );
}
