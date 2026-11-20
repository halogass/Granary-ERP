"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { hrApi } from "@/lib/api";
import { CalendarDays, Plus, Search, Trash2 } from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import ChoiceButtons from "@/components/hr/ChoiceButtons";

interface LeaveRequest {
  id: string;
  employee_id: string;
  employee?: { full_name?: string; email: string };
  leave_type: string;
  start_date: string;
  end_date: string;
  days_requested: number;
  reason?: string;
  status: string;
  reviewed_by_id?: string;
  review_notes?: string;
  created_at: string;
}

const LEAVE_TYPES = ["annual", "sick", "unpaid", "maternity", "paternity", "emergency", "other"];

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "pending", label: "Pending", filters: { status: "pending" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "rejected", label: "Rejected", filters: { status: "rejected" } },
];

const shortDate = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short" });
const longDate = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

export default function LeaveRequestsPage() {
  const [requests, setRequests] = useState<LeaveRequest[]>([]);
  const [loading, setLoading] = useState(true);
  const views = useSavedViews<Filters>("hr.leave", VIEWS);
  const [search, setSearch] = useState("");
  const [typeChip, setTypeChip] = useState("");
  const statusFilter = views.active?.filters.status ?? "";
  const [showModal, setShowModal] = useState(false);
  const [reviewModal, setReviewModal] = useState<LeaveRequest | null>(null);
  const [form, setForm] = useState({ leave_type: "annual", start_date: "", end_date: "", reason: "" });
  const [reviewForm, setReviewForm] = useState({ decision: "approved", notes: "" });
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await hrApi.listLeave();
      setRequests(res.data.items ?? res.data ?? []);
    } catch { setRequests([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const q = search.trim().toLowerCase();
  const filtered = requests.filter((r) => (!statusFilter || r.status === statusFilter) && (!typeChip || r.leave_type === typeChip)
    && (!q || [r.employee?.full_name, r.employee?.email, r.leave_type].some((v) => (v ?? "").toLowerCase().includes(q))));
  const pending = requests.filter((r) => r.status === "pending").length;

  async function submitLeave() {
    if (!form.start_date || !form.end_date) return;
    setSaving(true);
    try {
      await hrApi.submitLeave({ ...form });
      setShowModal(false);
      setForm({ leave_type: "annual", start_date: "", end_date: "", reason: "" });
      await load();
    } finally { setSaving(false); }
  }

  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  async function deleteLeave(id: string) {
    try {
      await hrApi.deleteLeave(id);
      setConfirmDeleteId(null);
      setRequests((prev) => prev.filter((r) => r.id !== id));
    } catch { /* keep */ }
  }

  async function review() {
    if (!reviewModal) return;
    setSaving(true);
    try {
      await hrApi.reviewLeave(reviewModal.id, { decision: reviewForm.decision, notes: reviewForm.notes });
      setReviewModal(null);
      await load();
    } finally { setSaving(false); }
  }

  const columns = useMemo<ColumnDef<LeaveRequest>[]>(() => [
    {
      id: "employee", accessorFn: (r) => r.employee?.full_name ?? r.employee?.email ?? "Employee", header: "Employee",
      cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span>,
    },
    { accessorKey: "leave_type", header: "Type", cell: (c) => <Pill className="capitalize">{c.row.original.leave_type}</Pill> },
    {
      accessorKey: "start_date", header: "Dates",
      cell: (c) => <span className="tabular">{shortDate(c.row.original.start_date)} to {longDate(c.row.original.end_date)}</span>,
    },
    { accessorKey: "days_requested", header: "Days", meta: { numeric: true } },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="leave" status={c.row.original.status} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const req = c.row.original;
        return (
          <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
            {req.status === "pending" && (
              <Button size="sm" onClick={() => { setReviewModal(req); setReviewForm({ decision: "approved", notes: "" }); }}>Review</Button>
            )}
            {confirmDeleteId === req.id ? (
              <>
                <span className="text-xs font-semibold text-tone-red">Delete?</span>
                <Button size="sm" variant="danger" onClick={() => deleteLeave(req.id)}>Yes</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </>
            ) : ["pending", "rejected"].includes(req.status) && (
              <IconButton size="sm" variant="ghost" icon={Trash2} label="Delete leave request" onClick={() => setConfirmDeleteId(req.id)} />
            )}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={requests.length} />
        <Stat label="Pending" value={pending} alert={pending > 0} note={pending > 0 ? "waiting for review" : undefined} />
        <Stat label="Approved" value={requests.filter((r) => r.status === "approved").length} />
        <Stat label="Rejected" value={requests.filter((r) => r.status === "rejected").length} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Leave requests" summary="Submit and manage employee leave"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Request leave</Button>} />
        <SavedViewTabs state={views} currentFilters={{ ...(statusFilter ? { status: statusFilter } : {}) }}
          counts={{ pending: pending || undefined }} alertIds={["pending"]} />
        <FilterChips
          chips={LEAVE_TYPES.map((t) => ({ id: t, label: t.charAt(0).toUpperCase() + t.slice(1), active: typeChip === t }))}
          onToggle={(id) => setTypeChip((cur) => (cur === id ? "" : id))} onClear={() => setTypeChip("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search leave" aria-label="Search leave requests" className="w-56" />
        </FilterChips>
        <DataTable label="Leave requests" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
          empty={{ icon: CalendarDays, title: "No leave requests here", description: "Pick another view, or request leave." }}
          renderDetails={(r) => (
            <KeyValues cols={3} items={[
              { label: "Employee", value: r.employee?.full_name ?? r.employee?.email ?? "-" },
              { label: "Requested", value: longDate(r.created_at) },
              { label: "Days", value: r.days_requested, numeric: true },
              { label: "Reason", value: r.reason || "-" },
              { label: "Review notes", value: r.review_notes || "-" },
            ]} />
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Request leave"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={submitLeave} disabled={!form.start_date || !form.end_date} loading={saving}>
            {saving ? "Submitting..." : "Submit request"}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Leave type">
            <Select value={form.leave_type} onChange={(e) => setForm({ ...form, leave_type: e.target.value })} className="capitalize"
              options={LEAVE_TYPES.map((t) => ({ value: t, label: t.charAt(0).toUpperCase() + t.slice(1) }))} />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Start date" required>
              <Input type="date" value={form.start_date} onChange={(e) => setForm({ ...form, start_date: e.target.value })} />
            </Field>
            <Field label="End date" required>
              <Input type="date" value={form.end_date} onChange={(e) => setForm({ ...form, end_date: e.target.value })} />
            </Field>
          </FieldGrid>
          <Field label="Reason">
            <Textarea value={form.reason} onChange={(e) => setForm({ ...form, reason: e.target.value })} rows={3} />
          </Field>
        </div>
      </Dialog>

      <Dialog open={!!reviewModal} onOpenChange={(o) => { if (!o) setReviewModal(null); }} size="sm" title="Review leave"
        description={reviewModal ? `${reviewModal.employee?.full_name ?? ""}, ${reviewModal.leave_type}, ${reviewModal.days_requested} days` : undefined}
        footer={<>
          <Button onClick={() => setReviewModal(null)}>Cancel</Button>
          <Button variant="primary" onClick={review} loading={saving}>{saving ? "Saving..." : "Submit review"}</Button>
        </>}>
        <div className="space-y-4">
          <ChoiceButtons label="Decision" value={reviewForm.decision}
            options={[{ value: "approved", label: "Approve" }, { value: "rejected", label: "Reject" }]}
            onChange={(d) => setReviewForm({ ...reviewForm, decision: d })} />
          <Field label="Notes">
            <Textarea value={reviewForm.notes} onChange={(e) => setReviewForm({ ...reviewForm, notes: e.target.value })}
              rows={3} placeholder="Optional review notes..." />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
