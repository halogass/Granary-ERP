"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { hrApi } from "@/lib/api";
import { Users, Plus, Search } from "lucide-react";
import { GRANARY_DEPARTMENTS as DEPTS } from "@/lib/departments";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, Input, KeyValues, ListFrame, PageHeader, SavedViewTabs,
  Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface HiringRequest {
  id: string;
  title: string;
  department: string;
  employment_type: string;
  headcount: number;
  justification?: string;
  status: string;
  requested_by?: { full_name?: string; email: string };
  created_at: string;
}

const STATUS_LABELS: Record<string, string> = {
  pending_approval: "Awaiting Owner", pending: "Pending", on_hold: "On hold",
  approved: "Approved", rejected: "Rejected", fulfilled: "Fulfilled",
};

const EMP_TYPES = ["full_time", "part_time", "contract", "intern"];

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "awaiting", label: "Awaiting Owner", filters: { status: "pending_approval" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
];

const CHIP_STATUSES = ["pending", "on_hold", "rejected", "fulfilled"];

const fmtDate = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

export default function HiringPage() {
  const [requests, setRequests] = useState<HiringRequest[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState({ title: "", department: "", employment_type: "full_time", headcount: "1", justification: "" });
  const [saving, setSaving] = useState(false);
  const views = useSavedViews<Filters>("hr.hiring", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [search, setSearch] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await hrApi.listHiringRequests();
      setRequests(res.data.items ?? res.data ?? []);
    } catch { setRequests([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function submit() {
    if (!form.title.trim() || !form.department) return;
    setSaving(true);
    try {
      await hrApi.createHiringRequest({
        title: form.title.trim(),
        department: form.department,
        employment_type: form.employment_type,
        headcount: parseInt(form.headcount) || 1,
        justification: form.justification.trim() || undefined,
      });
      setShowModal(false);
      setForm({ title: "", department: "", employment_type: "full_time", headcount: "1", justification: "" });
      await load();
    } finally { setSaving(false); }
  }

  const pending = requests.filter((r) => r.status === "pending_approval").length;
  const approved = requests.filter((r) => r.status === "approved").length;

  const status = chipStatus || views.active?.filters.status || "";
  const filters: Filters = status ? { status } : {};
  const q = search.trim().toLowerCase();
  const visible = requests.filter((r) => (!status || r.status === status)
    && (!q || [r.title, r.department, r.requested_by?.full_name, r.requested_by?.email].some((v) => (v ?? "").toLowerCase().includes(q))));

  const columns = useMemo<ColumnDef<HiringRequest>[]>(() => [
    {
      accessorKey: "title", header: "Position",
      cell: (c) => <span className="font-medium text-fg">{c.row.original.title}</span>,
    },
    { accessorKey: "department", header: "Department" },
    {
      accessorKey: "employment_type", header: "Type", meta: { hideOnMobile: true },
      cell: (c) => <span className="capitalize">{c.row.original.employment_type?.replace("_", " ")}</span>,
    },
    {
      id: "requested_by", accessorFn: (r) => r.requested_by?.full_name ?? r.requested_by?.email ?? "", header: "Requested by",
      meta: { hideOnMobile: true }, cell: (c) => c.getValue<string>() || "-",
    },
    {
      accessorKey: "created_at", header: "Requested", meta: { hideOnMobile: true },
      cell: (c) => <span className="tabular">{fmtDate(c.row.original.created_at)}</span>,
    },
    { accessorKey: "headcount", header: "Headcount", meta: { numeric: true } },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="job_request" status={c.row.original.status} label={STATUS_LABELS[c.row.original.status]} />,
    },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={requests.length} />
        <Stat label="Awaiting Owner" value={pending} alert={pending > 0} note={pending > 0 ? "needs a decision" : undefined} />
        <Stat label="Approved" value={approved} />
        <Stat label="Headcount" value={requests.reduce((s, r) => s + (r.headcount ?? 1), 0)} note="across all requests" />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Hiring requests" summary="Submitted by managers, processed by HR"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New request</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChipStatus("")}
          counts={{ awaiting: pending || undefined }} alertIds={["awaiting"]} />
        <FilterChips
          chips={CHIP_STATUSES.map((s) => ({ id: s, label: STATUS_LABELS[s], active: chipStatus === s }))}
          onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))} onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search requests" aria-label="Search hiring requests" className="w-56" />
        </FilterChips>
        <DataTable label="Hiring requests" columns={columns} data={visible} getRowId={(r) => r.id} loading={loading}
          empty={{
            icon: Users, title: "No hiring requests here", description: "Pick another view, or submit a new request.",
            action: <Button variant="primary" onClick={() => setShowModal(true)}>New request</Button>,
          }}
          renderDetails={(r) => (
            <div className="space-y-3">
              <KeyValues cols={3} items={[
                { label: "Requested by", value: r.requested_by?.full_name ?? r.requested_by?.email ?? "-" },
                { label: "Requested", value: fmtDate(r.created_at) },
                { label: "Headcount", value: r.headcount, numeric: true },
              ]} />
              <div>
                <p className="text-[13px] text-fg-muted">Justification</p>
                <p className="mt-0.5 whitespace-pre-line text-[13px] text-fg">{r.justification || "None given."}</p>
              </div>
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="New hiring request"
        description="HR will process it after approval from your manager."
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={submit} disabled={!form.title.trim() || !form.department} loading={saving}>
            {saving ? "Submitting..." : "Submit request"}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Position title" required>
            <Input value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} placeholder="e.g. Senior Backend Engineer" />
          </Field>
          <FieldGrid cols={3}>
            <Field label="Department" required>
              <Select value={form.department} onChange={(e) => setForm({ ...form, department: e.target.value })}
                placeholder="-" options={DEPTS.map((d) => ({ value: d, label: d }))} />
            </Field>
            <Field label="Type">
              <Select value={form.employment_type} onChange={(e) => setForm({ ...form, employment_type: e.target.value })}
                options={EMP_TYPES.map((t) => ({ value: t, label: t.replace("_", " ") }))} />
            </Field>
            <Field label="Headcount">
              <Input numeric type="number" min="1" value={form.headcount} onChange={(e) => setForm({ ...form, headcount: e.target.value })} />
            </Field>
          </FieldGrid>
          <Field label="Justification">
            <Textarea value={form.justification} onChange={(e) => setForm({ ...form, justification: e.target.value })}
              rows={3} placeholder="Why is this hire needed? Impact if not filled?" />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
