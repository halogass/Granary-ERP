"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { hrApi } from "@/lib/api";
import { Briefcase, Plus, Search } from "lucide-react";
import { GRANARY_DEPARTMENTS } from "@/lib/departments";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, Input, KeyValues, ListFrame, PageHeader, Pill, SavedViewTabs,
  Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface JobOpening {
  id: string;
  title: string;
  department: string;
  employment_type: string;
  location?: string;
  headcount: number;
  status: string;
  description?: string;
  requirements?: string;
  hiring_request_id?: string;
  created_at: string;
  closed_at?: string;
}

const STATUS_LABELS: Record<string, string> = { open: "Open", paused: "Paused", filled: "Filled", cancelled: "Cancelled" };

const EMP_TYPES = ["full_time", "part_time", "contract", "intern", "freelance"];

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "open", label: "Open", filters: { status: "open" } },
  { id: "all", label: "All", filters: {} },
];

const CHIP_STATUSES = ["paused", "filled", "cancelled"];

const fmtDate = (d: string) => new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

export default function JobOpeningsPage() {
  const [openings, setOpenings] = useState<JobOpening[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState({
    title: "", department: "R&D", employment_type: "full_time",
    location: "", headcount: "1", description: "", requirements: "",
  });
  const [saving, setSaving] = useState(false);
  const views = useSavedViews<Filters>("hr.job-openings", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [search, setSearch] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await hrApi.listJobOpenings({ status: "all" });
      setOpenings(res.data.items ?? res.data ?? []);
    } catch { setOpenings([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function create() {
    if (!form.title.trim()) return;
    setSaving(true);
    try {
      await hrApi.createJobOpening({
        title: form.title.trim(),
        department: form.department,
        employment_type: form.employment_type,
        location: form.location.trim() || undefined,
        headcount: parseInt(form.headcount) || 1,
        description: form.description.trim() || undefined,
        requirements: form.requirements.trim() || undefined,
      });
      setShowModal(false);
      setForm({ title: "", department: "R&D", employment_type: "full_time", location: "", headcount: "1", description: "", requirements: "" });
      await load();
    } finally { setSaving(false); }
  }

  const status = chipStatus || views.active?.filters.status || "";
  const filters: Filters = status ? { status } : {};
  const q = search.trim().toLowerCase();
  const filtered = openings.filter((o) => (!status || o.status === status)
    && (!q || [o.title, o.department, o.location].some((v) => (v ?? "").toLowerCase().includes(q))));
  const totalHeadcount = openings.filter((o) => o.status === "open").reduce((s, o) => s + o.headcount, 0);

  const columns = useMemo<ColumnDef<JobOpening>[]>(() => [
    { accessorKey: "title", header: "Position", cell: (c) => <span className="font-medium text-fg">{c.row.original.title}</span> },
    { accessorKey: "department", header: "Department", cell: (c) => <Pill className="capitalize">{c.row.original.department}</Pill> },
    {
      accessorKey: "employment_type", header: "Type", meta: { hideOnMobile: true },
      cell: (c) => <span className="capitalize">{c.row.original.employment_type.replace("_", " ")}</span>,
    },
    { accessorKey: "location", header: "Location", meta: { hideOnMobile: true }, cell: (c) => c.row.original.location || "-" },
    { accessorKey: "headcount", header: "Seats", meta: { numeric: true } },
    {
      accessorKey: "created_at", header: "Posted", meta: { hideOnMobile: true },
      cell: (c) => <span className="tabular">{fmtDate(c.row.original.created_at)}</span>,
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="job_opening" status={c.row.original.status} label={STATUS_LABELS[c.row.original.status]} />,
    },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={openings.length} />
        <Stat label="Open" value={openings.filter((o) => o.status === "open").length} />
        <Stat label="Open headcount" value={totalHeadcount} />
        <Stat label="Departments" value={new Set(openings.filter((o) => o.status === "open").map((o) => o.department)).size} note="with open roles" />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Job openings" summary="Active positions across all departments"
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Post opening</Button>} />
        <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChipStatus("")} />
        <FilterChips
          chips={CHIP_STATUSES.map((s) => ({ id: s, label: STATUS_LABELS[s], active: chipStatus === s }))}
          onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))} onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search openings" aria-label="Search job openings" className="w-56" />
        </FilterChips>
        <DataTable label="Job openings" columns={columns} data={filtered} getRowId={(o) => o.id} loading={loading}
          empty={{
            icon: Briefcase, title: "No job openings here", description: "Pick another view, or post an opening.",
            action: <Button variant="primary" onClick={() => setShowModal(true)}>Post opening</Button>,
          }}
          renderDetails={(o) => (
            <div className="space-y-3">
              <KeyValues cols={3} items={[
                { label: "Location", value: o.location || "-" },
                { label: "Seats", value: o.headcount, numeric: true },
                { label: "Posted", value: fmtDate(o.created_at) },
                ...(o.closed_at ? [{ label: "Closed", value: fmtDate(o.closed_at) }] : []),
              ]} />
              <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                <div>
                  <p className="text-[13px] text-fg-muted">Description</p>
                  <p className="mt-0.5 whitespace-pre-line text-[13px] text-fg">{o.description || "None."}</p>
                </div>
                <div>
                  <p className="text-[13px] text-fg-muted">Requirements</p>
                  <p className="mt-0.5 whitespace-pre-line text-[13px] text-fg">{o.requirements || "None."}</p>
                </div>
              </div>
            </div>
          )} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Post job opening"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!form.title.trim()} loading={saving}>
            {saving ? "Posting..." : "Post opening"}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Job title" required>
            <Input value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} placeholder="e.g. Senior Backend Engineer" />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Department">
              <Select value={form.department} onChange={(e) => setForm({ ...form, department: e.target.value })}
                options={GRANARY_DEPARTMENTS.map((d) => ({ value: d, label: d }))} />
            </Field>
            <Field label="Employment type">
              <Select value={form.employment_type} onChange={(e) => setForm({ ...form, employment_type: e.target.value })}
                options={EMP_TYPES.map((t) => ({ value: t, label: t.replace("_", " ") }))} />
            </Field>
            <Field label="Location">
              <Input value={form.location} onChange={(e) => setForm({ ...form, location: e.target.value })} placeholder="e.g. Tokyo / Remote" />
            </Field>
            <Field label="Headcount">
              <Input numeric type="number" min="1" value={form.headcount} onChange={(e) => setForm({ ...form, headcount: e.target.value })} />
            </Field>
          </FieldGrid>
          <Field label="Description">
            <Textarea value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} rows={3} />
          </Field>
          <Field label="Requirements">
            <Textarea value={form.requirements} onChange={(e) => setForm({ ...form, requirements: e.target.value })}
              rows={3} placeholder="Key skills, years of experience, qualifications..." />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
