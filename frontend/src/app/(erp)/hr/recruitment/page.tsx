"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { localSource } from "@/lib/entitySources";
import { hrApi } from "@/lib/api";
import { Users, Plus, ChevronRight, Search, Star } from "lucide-react";
import {
  Button, Combobox, DataTable, Dialog, Field, FieldGrid, FilterChips, Input, KeyValues, ListFrame, PageHeader,
  SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";

interface JobOpening {
  id: string;
  title: string;
  department: string;
  status: string;
}

interface Applicant {
  id: string;
  name: string;
  email?: string;
  phone?: string;
  stage: string;
  source?: string;
  job_opening_id: string;
  rating?: number;
  notes?: string;
  resume_url?: string;
  created_at: string;
}

const STAGES = [
  { key: "applied",        label: "Applied" },
  { key: "screening",      label: "Screening" },
  { key: "phone_screen",   label: "Phone screen" },
  { key: "interview_1",    label: "Interview 1" },
  { key: "interview_2",    label: "Interview 2" },
  { key: "technical_test", label: "Technical" },
  { key: "offer_sent",     label: "Offer sent" },
  { key: "offer_accepted", label: "Accepted" },
  { key: "hired",          label: "Hired" },
  { key: "rejected",       label: "Rejected" },
];

const NEXT_STAGE: Record<string, string> = {
  applied: "screening",
  screening: "phone_screen",
  phone_screen: "interview_1",
  interview_1: "interview_2",
  interview_2: "technical_test",
  technical_test: "offer_sent",
  offer_sent: "offer_accepted",
  offer_accepted: "hired",
};

const ACTIVE_STAGES = ["applied","screening","phone_screen","interview_1","interview_2","technical_test","offer_sent","offer_accepted"];

const CLOSED_STAGES = ["hired", "rejected", "offer_rejected", "withdrawn"];

type Filters = { group?: "active" | "hired" | "rejected" };

const VIEWS: SavedView<Filters>[] = [
  { id: "active", label: "In pipeline", filters: { group: "active" } },
  { id: "all", label: "All", filters: {} },
  { id: "hired", label: "Hired", filters: { group: "hired" } },
  { id: "rejected", label: "Rejected", filters: { group: "rejected" } },
];

function Rating({ value }: { value?: number }) {
  return (
    <span className="inline-flex gap-0.5" aria-label={value ? `${value} of 5` : "Not rated"}>
      {[1, 2, 3, 4, 5].map((n) => (
        <Star key={n} aria-hidden className={cn("size-3", value && n <= value ? "fill-tone-orange text-tone-orange" : "text-line-strong")} />
      ))}
    </span>
  );
}

export default function RecruitmentPage() {
  const [openings, setOpenings] = useState<JobOpening[]>([]);
  const [applicants, setApplicants] = useState<Applicant[]>([]);
  const [selectedOpening, setSelectedOpening] = useState("all");
  const [loading, setLoading] = useState(true);
  const [showAddModal, setShowAddModal] = useState(false);
  const [showStageModal, setShowStageModal] = useState<Applicant | null>(null);
  const [saving, setSaving] = useState(false);
  const [form, setForm] = useState({ job_opening_id: "", name: "", email: "", phone: "", source: "", resume_url: "" });
  const [stageData, setStageData] = useState({ stage: "", notes: "", rating: "" });
  const views = useSavedViews<Filters>("hr.recruitment", VIEWS);
  const [stageChip, setStageChip] = useState("");
  const [search, setSearch] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [oRes, aRes] = await Promise.all([
        hrApi.listJobOpenings({ status: undefined }),
        hrApi.listApplicants(),
      ]);
      const od = oRes.data.items ?? oRes.data ?? [];
      setOpenings(od.filter((o: JobOpening) => o.status === "open"));
      setApplicants(aRes.data.items ?? aRes.data ?? []);
    } catch { setApplicants([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const openingOptions = useMemo(() => localSource(openings.map((o) => ({
    id: o.id, label: o.title, sublabel: o.department,
  }))), [openings]);

  async function addApplicant() {
    if (!form.job_opening_id || !form.name.trim()) return;
    setSaving(true);
    try {
      await hrApi.createApplicant({
        job_opening_id: form.job_opening_id,
        full_name: form.name.trim(),
        email: form.email || undefined,
        phone: form.phone || undefined,
        source: form.source || undefined,
        resume_url: form.resume_url || undefined,
      });
      setShowAddModal(false);
      setForm({ job_opening_id: "", name: "", email: "", phone: "", source: "", resume_url: "" });
      await load();
    } finally { setSaving(false); }
  }

  async function advanceStage() {
    if (!showStageModal || !stageData.stage) return;
    setSaving(true);
    try {
      await hrApi.updateApplicantStage(showStageModal.id, {
        stage: stageData.stage,
        notes: stageData.notes || undefined,
        rating: stageData.rating ? parseInt(stageData.rating) : undefined,
      });
      setShowStageModal(null);
      setStageData({ stage: "", notes: "", rating: "" });
      await load();
    } finally { setSaving(false); }
  }

  const displayed = selectedOpening === "all"
    ? applicants
    : applicants.filter((a) => a.job_opening_id === selectedOpening);

  const active = displayed.filter((a) => ACTIVE_STAGES.includes(a.stage)).length;
  const hired = displayed.filter((a) => a.stage === "hired").length;
  const rejected = displayed.filter((a) => ["rejected","offer_rejected","withdrawn"].includes(a.stage)).length;
  const openingMap = Object.fromEntries(openings.map((o) => [o.id, o.title]));

  const stageLabel = (s: string) => STAGES.find((x) => x.key === s)?.label ?? s;
  const stageIndex = (s: string) => STAGES.findIndex((x) => x.key === s);

  const group = views.active?.filters.group;
  const q = search.trim().toLowerCase();
  const rows = displayed
    .filter((a) => group === "active" ? ACTIVE_STAGES.includes(a.stage)
      : group === "hired" ? a.stage === "hired"
      : group === "rejected" ? ["rejected", "offer_rejected", "withdrawn"].includes(a.stage) : true)
    .filter((a) => !stageChip || a.stage === stageChip)
    .filter((a) => !q || [a.name, a.email, a.source, openingMap[a.job_opening_id]].some((v) => (v ?? "").toLowerCase().includes(q)))
    .sort((a, b) => stageIndex(b.stage) - stageIndex(a.stage));

  function openStage(a: Applicant, stage: string) {
    setShowStageModal(a);
    setStageData({ stage, notes: "", rating: "" });
  }

  const columns = useMemo<ColumnDef<Applicant>[]>(() => [
    {
      accessorKey: "name", header: "Applicant",
      cell: (c) => (
        <div className="min-w-0">
          <p className="max-w-56 truncate font-medium text-fg">{c.row.original.name}</p>
          <p className="max-w-56 truncate text-xs text-fg-muted">
            {c.row.original.email}{c.row.original.source ? `${c.row.original.email ? ", " : ""}via ${c.row.original.source}` : ""}
          </p>
        </div>
      ),
    },
    {
      id: "position", accessorFn: (a) => openingMap[a.job_opening_id] ?? "", header: "Position", meta: { hideOnMobile: true },
      cell: (c) => <span className="block max-w-48 truncate">{c.getValue<string>() || "-"}</span>,
    },
    {
      id: "stage", accessorFn: (a) => stageIndex(a.stage), header: "Stage",
      cell: (c) => <StatusPill family="applicant" status={c.row.original.stage} label={stageLabel(c.row.original.stage)} />,
    },
    { accessorKey: "rating", header: "Rating", meta: { hideOnMobile: true }, cell: (c) => <Rating value={c.row.original.rating} /> },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const a = c.row.original;
        const nextStage = NEXT_STAGE[a.stage];
        if (nextStage) return <Button size="sm" variant="ghost" onClick={() => openStage(a, nextStage)}>Next <ChevronRight aria-hidden /></Button>;
        if (!CLOSED_STAGES.includes(a.stage)) return <Button size="sm" variant="danger" onClick={() => openStage(a, "rejected")}>Reject</Button>;
        return null;
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [openings]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={displayed.length} />
        <Stat label="Active" value={active} note="in the pipeline" />
        <Stat label="Hired" value={hired} />
        <Stat label="Rejected" value={rejected} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Recruitment" summary="Applicant tracking pipeline across open positions"
          actions={<Button variant="primary" onClick={() => setShowAddModal(true)}><Plus aria-hidden /> Add applicant</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters} onSelect={() => setStageChip("")} />
        <FilterChips
          chips={STAGES.filter((s) => s.key !== "rejected").map((s) => ({
            id: s.key, label: s.label, active: stageChip === s.key, count: displayed.filter((a) => a.stage === s.key).length,
          }))}
          onToggle={(id) => setStageChip((cur) => (cur === id ? "" : id))} onClear={() => setStageChip("")}>
          <Select aria-label="Opening" value={selectedOpening} onChange={(e) => setSelectedOpening(e.target.value)} className="w-48"
            options={[{ value: "all", label: "All openings" }, ...openings.map((o) => ({ value: o.id, label: o.title }))]} />
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search applicants" aria-label="Search applicants" className="w-56" />
        </FilterChips>
        <DataTable label="Applicants" columns={columns} data={rows} getRowId={(a) => a.id} loading={loading}
          empty={{
            icon: Users, title: "No applicants here", description: "Add the first applicant to start the pipeline.",
            action: <Button variant="primary" onClick={() => setShowAddModal(true)}>Add applicant</Button>,
          }}
          renderDetails={(a) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Email", value: a.email || "-" },
                { label: "Phone", value: a.phone || "-" },
                { label: "Source", value: a.source || "-" },
                { label: "Position", value: openingMap[a.job_opening_id] ?? "-" },
                { label: "Applied", value: new Date(a.created_at).toLocaleDateString("en-GB") },
                { label: "Resume", value: a.resume_url ? <a href={a.resume_url} target="_blank" rel="noopener noreferrer" className="text-brand-ink hover:underline">Open resume</a> : "-" },
                ...(a.notes ? [{ label: "Notes", value: <span className="whitespace-pre-line">{a.notes}</span> }] : []),
              ]} />
              {!CLOSED_STAGES.includes(a.stage) && (
                <Button size="sm" onClick={() => openStage(a, NEXT_STAGE[a.stage] ?? a.stage)}>Update stage</Button>
              )}
            </div>
          )} />
      </ListFrame>

      <Dialog open={showAddModal} onOpenChange={setShowAddModal} title="Add applicant"
        footer={<>
          <Button onClick={() => setShowAddModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={addApplicant} disabled={!form.job_opening_id || !form.name.trim()} loading={saving}>
            {saving ? "Adding..." : "Add applicant"}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Position" required>
            <Combobox value={form.job_opening_id} source={openingOptions}
              placeholder="Select an opening" emptyLabel="No open positions"
              onChange={(id) => setForm({ ...form, job_opening_id: id })} />
          </Field>
          <Field label="Full name" required>
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="e.g. Robert Smith" />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Email"><Input type="email" value={form.email} onChange={(e) => setForm({ ...form, email: e.target.value })} /></Field>
            <Field label="Phone"><Input value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} /></Field>
            <Field label="Source">
              <Select value={form.source} onChange={(e) => setForm({ ...form, source: e.target.value })} placeholder="Select..."
                options={["LinkedIn", "Indeed", "Referral", "Website", "Job Fair", "Direct"].map((s) => ({ value: s, label: s }))} />
            </Field>
            <Field label="Resume URL">
              <Input value={form.resume_url} onChange={(e) => setForm({ ...form, resume_url: e.target.value })} placeholder="https://..." />
            </Field>
          </FieldGrid>
        </div>
      </Dialog>

      <Dialog open={!!showStageModal} onOpenChange={(o) => { if (!o) setShowStageModal(null); }} title="Update stage"
        description={showStageModal?.name}
        footer={<>
          <Button onClick={() => setShowStageModal(null)}>Cancel</Button>
          <Button variant="primary" onClick={advanceStage} disabled={!stageData.stage} loading={saving}>
            {saving ? "Saving..." : "Update stage"}
          </Button>
        </>}>
        <div className="space-y-4">
          <Field label="Move to stage">
            <Select value={stageData.stage} onChange={(e) => setStageData({ ...stageData, stage: e.target.value })}
              options={STAGES.map((s) => ({ value: s.key, label: s.label }))} />
          </Field>
          <Field label="Rating (1 to 5)">
            <Select value={stageData.rating} onChange={(e) => setStageData({ ...stageData, rating: e.target.value })} placeholder="No rating"
              options={[1, 2, 3, 4, 5].map((n) => ({ value: String(n), label: `${n} star${n > 1 ? "s" : ""}` }))} />
          </Field>
          <Field label="Notes">
            <Textarea value={stageData.notes} onChange={(e) => setStageData({ ...stageData, notes: e.target.value })}
              rows={3} placeholder="Interview notes, feedback..." />
          </Field>
        </div>
      </Dialog>
    </div>
  );
}
