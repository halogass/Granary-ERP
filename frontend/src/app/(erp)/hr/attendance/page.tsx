"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState, useCallback, useMemo } from "react";
import { hrApi } from "@/lib/api";
import { Calendar, Plus, ExternalLink, ShieldAlert, MapPin, Camera } from "lucide-react";
import Link from "next/link";
import {
  Button, CountBadge, DataTable, Dialog, FilterChips, Field, FieldGrid, Input, KeyValues, ListFrame, PageHeader, Pill,
  Select, StatusPill, Tabs, TabsContent, TabsList, TabsTrigger, type ColumnDef,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import AttemptsPanel from "@/components/attendance/AttemptsPanel";

interface Employee {
  id: string;
  first_name: string;
  last_name?: string;
  department?: string;
}

interface AttendanceRecord {
  id: string;
  employee_id: string;
  date: string;
  status: string;
  check_in?: string;
  check_out?: string;
  work_hours?: number;
  late_minutes?: number;
  overtime_hours?: number;
  notes?: string;
}

interface ClockEvent {
  id: string;
  employee_id?: string;
  employee_name?: string;
  event_type: string;
  captured_at: string;
  recognition_status: string;
  recognition_confidence?: number;
  liveness_score?: number;
  is_real_face?: boolean;
  geo_distance_meters?: number;
  is_kiosk?: boolean;
  override_at?: string;
  liveness_challenge?: string | null;
  liveness_passed?: boolean | null;
  match_distance?: number | null;
  details?: {
    kiosk_claim?: string; server_match?: string; server_distance?: number;
    /** Release 2: how liveness was checked (passive flash + anti-spoof, or the active fallback and why). */
    passive?: { method?: "passive" | "active"; corr?: number; delta?: number; spoof_real?: number; fallback_reason?: string } | null;
  } | null;
}

interface PendingOverride extends ClockEvent {
  recognition_uid?: string;
}

const STATUS_LABELS: Record<string, string> = {
  present: "Present", absent: "Absent", late: "Late", half_day: "Half day", on_leave: "On leave", holiday: "Holiday", remote: "Remote",
};

const REC_LABELS: Record<string, string> = {
  matched: "Matched", unrecognized: "Unrecognized", liveness_failed: "Liveness failed", low_confidence: "Low confidence",
  pending_override: "Pending override", overridden: "Overridden by HR", kiosk_mismatch: "Kiosk mismatch",
  duplicate: "Already clocked in", no_clock_in: "No clock-in", rejected: "Rejected",
};

const CHALLENGE_LABEL: Record<string, string> = { blink: "Blink", turn_left: "Turn left", turn_right: "Turn right" };

const FALLBACK_LABEL: Record<string, string> = {
  flash_inconclusive: "flash unclear", flash_failed: "flash did not match", spoof: "spoof score low", no_face: "no face",
};

/**
 * "Passive, score 93%" or "Active (fallback: flash unclear), Blink, passed" when passive liveness ran;
 * "Blink, passed" for the active-only engine; the old hosted-API score for historic events.
 */
function livenessLabel(ev: ClockEvent): string {
  const p = ev.details?.passive;
  const score = ev.liveness_score != null ? `, score ${(ev.liveness_score * 100).toFixed(0)}%` : "";
  if (p?.method === "passive") return `Passive${score}`;
  const action = ev.liveness_challenge
    ? `${CHALLENGE_LABEL[ev.liveness_challenge] ?? ev.liveness_challenge}, ${ev.liveness_passed ? "passed" : "failed"}`
    : null;
  if (p?.method === "active") {
    const why = p.fallback_reason ? FALLBACK_LABEL[p.fallback_reason] ?? p.fallback_reason.replace(/_/g, " ") : "";
    return `Active${why ? ` (fallback: ${why})` : ""}${action ? `, ${action}` : ""}${score}`;
  }
  if (action) return action;
  return ev.liveness_score != null ? `${(ev.liveness_score * 100).toFixed(0)}%` : "-";
}

const MONTH_NAMES = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];

const pct = (n?: number, digits = 0) => (n != null ? `${(n * 100).toFixed(digits)}%` : "-");
const meters = (n?: number) => (n != null ? `${Math.round(n)} m` : "-");
const stamp = (d: string) => new Date(d).toLocaleString("en-GB");

function EventPill({ type }: { type: string }) {
  return <Pill tone={type === "clock_in" ? "blue" : "neutral"} className="capitalize">{type.replace("_", " ")}</Pill>;
}

function Liveness({ ev }: { ev: ClockEvent }) {
  return <span className={cn("text-[13px]", ev.is_real_face ? "text-fg" : "font-medium text-tone-red")}>{livenessLabel(ev)}</span>;
}

export default function AttendancePage() {
  const now = new Date();

  const [tab, setTab] = useState<"records" | "overrides" | "clock_events" | "attempts">("records");

  const [employees, setEmployees] = useState<Employee[]>([]);
  const employeeName = (id?: string) => {
    const e = employees.find((x) => x.id === id);
    return e ? `${e.first_name} ${e.last_name ?? ""}`.trim() : "an unknown employee";
  };
  const [records, setRecords] = useState<AttendanceRecord[]>([]);
  const [loadingRec, setLoadingRec] = useState(false);
  const [filterMonth, setFilterMonth] = useState(now.getMonth() + 1);
  const [filterYear, setFilterYear] = useState(now.getFullYear());
  const [filterEmployee, setFilterEmployee] = useState("");
  const [showLog, setShowLog] = useState(false);
  const [form, setForm] = useState({
    employee_id: "", date: now.toISOString().slice(0, 10),
    status: "present", check_in: "09:00", check_out: "18:00",
    late_minutes: "0", overtime_hours: "0", notes: "",
  });
  const [saving, setSaving] = useState(false);
  const [statusChip, setStatusChip] = useState("");

  const [pendingOverrides, setPendingOverrides] = useState<PendingOverride[]>([]);
  const [loadingOverrides, setLoadingOverrides] = useState(false);
  const [overrideModal, setOverrideModal] = useState<PendingOverride | null>(null);
  const [overrideEmployeeId, setOverrideEmployeeId] = useState("");
  const [overrideNotes, setOverrideNotes] = useState("");

  const [clockEvents, setClockEvents] = useState<ClockEvent[]>([]);
  const [loadingClock, setLoadingClock] = useState(false);

  const loadRecords = useCallback(async () => {
    setLoadingRec(true);
    try {
      const params: Record<string, unknown> = { month: filterMonth, year: filterYear };
      if (filterEmployee) params.employee_id = filterEmployee;
      const [rRes, eRes] = await Promise.all([
        fetchAllPages<AttendanceRecord>(hrApi.listAttendance, params),
        fetchAllPages<Employee>(hrApi.listEmployees),
      ]);
      setRecords(rRes.rows);
      setEmployees(eRes.rows);
    } catch { setRecords([]); } finally { setLoadingRec(false); }
  }, [filterMonth, filterYear, filterEmployee]);

  const loadOverrides = useCallback(async () => {
    setLoadingOverrides(true);
    try {
      const res = await hrApi.listPendingOverrides();
      setPendingOverrides(res.data ?? []);
    } catch { } finally { setLoadingOverrides(false); }
  }, []);

  const loadClockEvents = useCallback(async () => {
    setLoadingClock(true);
    try {
      const { rows } = await fetchAllPages<ClockEvent>(hrApi.listClockEvents);
      setClockEvents(rows);
    } catch { } finally { setLoadingClock(false); }
  }, []);

  useEffect(() => { if (tab === "records") loadRecords(); }, [tab, loadRecords]);
  useEffect(() => { if (tab === "overrides") loadOverrides(); }, [tab, loadOverrides]);
  useEffect(() => { if (tab === "clock_events") loadClockEvents(); }, [tab, loadClockEvents]);

  // Also load employees for override modal
  useEffect(() => {
    if (employees.length === 0) fetchAllPages<Employee>(hrApi.listEmployees).then(({ rows }) => setEmployees(rows)).catch(() => {});
  }, []);

  const empMap: Record<string, string> = {};
  employees.forEach(e => { empMap[e.id] = `${e.first_name} ${e.last_name ?? ""}`.trim(); });

  async function logAttendance() {
    if (!form.employee_id || !form.date) return;
    setSaving(true);
    try {
      await hrApi.recordAttendance({
        employee_id: form.employee_id, date: form.date, status: form.status,
        check_in: form.check_in || undefined, check_out: form.check_out || undefined,
        late_minutes: parseInt(form.late_minutes) || 0,
        overtime_hours: parseFloat(form.overtime_hours) || 0,
        notes: form.notes || undefined,
      });
      setShowLog(false);
      loadRecords();
    } catch { } finally { setSaving(false); }
  }

  async function submitOverride(action: "approve" | "reject") {
    if (!overrideModal) return;
    if (action === "approve" && !overrideEmployeeId) return;
    setSaving(true);
    try {
      await hrApi.overrideClockEvent(overrideModal.id, {
        action,
        employee_id: overrideEmployeeId || "00000000-0000-0000-0000-000000000000",
        notes: overrideNotes || undefined,
      });
      setOverrideModal(null);
      loadOverrides();
    } catch { } finally { setSaving(false); }
  }

  // Summary counts
  const counts: Record<string, number> = {};
  records.forEach(r => { counts[r.status] = (counts[r.status] ?? 0) + 1; });
  const totalDays = Object.values(counts).reduce((a, b) => a + b, 0);
  const years = [now.getFullYear() - 1, now.getFullYear()];

  const visibleRecords = statusChip ? records.filter(r => r.status === statusChip) : records;
  const employeeOptions = employees.map(e => ({ value: e.id, label: `${e.first_name} ${e.last_name ?? ""}`.trim() }));

  const recordColumns = useMemo<ColumnDef<AttendanceRecord>[]>(() => [
    {
      id: "employee", accessorFn: (r) => empMap[r.employee_id] ?? r.employee_id.slice(0, 8), header: "Employee",
      cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span>,
    },
    { accessorKey: "date", header: "Date", meta: { numeric: true }, cell: (c) => new Date(c.row.original.date).toLocaleDateString("en-GB") },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="attendance" status={c.row.original.status} label={STATUS_LABELS[c.row.original.status]} />,
    },
    { accessorKey: "check_in", header: "Check in", meta: { numeric: true }, cell: (c) => c.row.original.check_in ?? "-" },
    { accessorKey: "check_out", header: "Check out", meta: { numeric: true }, cell: (c) => c.row.original.check_out ?? "-" },
    { accessorKey: "work_hours", header: "Hours", meta: { numeric: true, hideOnMobile: true }, cell: (c) => c.row.original.work_hours?.toFixed(1) ?? "-" },
    { accessorKey: "late_minutes", header: "Late (min)", meta: { numeric: true, hideOnMobile: true }, cell: (c) => c.row.original.late_minutes ?? 0 },
    { accessorKey: "overtime_hours", header: "OT (hrs)", meta: { numeric: true, hideOnMobile: true }, cell: (c) => Number(c.row.original.overtime_hours ?? 0).toFixed(1) },
    {
      accessorKey: "notes", header: "Notes", enableSorting: false, meta: { hideOnMobile: true },
      cell: (c) => <span className="block max-w-xs truncate text-xs text-fg-muted">{c.row.original.notes ?? "-"}</span>,
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [employees]);

  const overrideColumns = useMemo<ColumnDef<PendingOverride>[]>(() => [
    { accessorKey: "captured_at", header: "Time", meta: { numeric: true }, cell: (c) => stamp(c.row.original.captured_at) },
    { accessorKey: "event_type", header: "Event", cell: (c) => <EventPill type={c.row.original.event_type} /> },
    {
      accessorKey: "recognition_status", header: "Status",
      cell: (c) => <StatusPill family="face_check" status={c.row.original.recognition_status} label={REC_LABELS[c.row.original.recognition_status]} />,
    },
    { accessorKey: "recognition_confidence", header: "Confidence", meta: { numeric: true }, cell: (c) => pct(c.row.original.recognition_confidence) },
    { id: "liveness", header: "Liveness", enableSorting: false, cell: (c) => <Liveness ev={c.row.original} /> },
    { accessorKey: "geo_distance_meters", header: "Distance", meta: { numeric: true, hideOnMobile: true }, cell: (c) => meters(c.row.original.geo_distance_meters) },
    { accessorKey: "is_kiosk", header: "Kiosk", meta: { hideOnMobile: true }, cell: (c) => (c.row.original.is_kiosk ? "Yes" : "No") },
    {
      id: "action", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => (
        <Button size="sm" variant="primary" onClick={() => { setOverrideModal(c.row.original); setOverrideEmployeeId(""); setOverrideNotes(""); }}>
          Review
        </Button>
      ),
    },
  ], []);

  const clockColumns = useMemo<ColumnDef<ClockEvent>[]>(() => [
    {
      accessorKey: "employee_name", header: "Employee",
      cell: (c) => c.row.original.employee_name
        ? <span className="font-medium text-fg">{c.row.original.employee_name}</span>
        : <span className="text-xs italic text-fg-subtle">Unknown</span>,
    },
    { accessorKey: "captured_at", header: "Time", meta: { numeric: true }, cell: (c) => stamp(c.row.original.captured_at) },
    { accessorKey: "event_type", header: "Event", cell: (c) => <EventPill type={c.row.original.event_type} /> },
    {
      accessorKey: "recognition_status", header: "Recognition",
      cell: (c) => <StatusPill family="face_check" status={c.row.original.recognition_status} label={REC_LABELS[c.row.original.recognition_status]} />,
    },
    { accessorKey: "recognition_confidence", header: "Confidence", meta: { numeric: true, hideOnMobile: true }, cell: (c) => pct(c.row.original.recognition_confidence) },
    { id: "liveness", header: "Liveness", enableSorting: false, meta: { hideOnMobile: true }, cell: (c) => <Liveness ev={c.row.original} /> },
    { accessorKey: "geo_distance_meters", header: "Distance", meta: { numeric: true, hideOnMobile: true }, cell: (c) => meters(c.row.original.geo_distance_meters) },
    {
      accessorKey: "is_kiosk", header: "Kiosk", meta: { hideOnMobile: true },
      cell: (c) => (
        <MapPin aria-label={c.row.original.is_kiosk ? "Kiosk" : "Personal device"}
          className={cn("size-3.5", c.row.original.is_kiosk ? "text-tone-blue" : "text-fg-subtle")} />
      ),
    },
  ], []);

  return (
    <div>
      <Tabs value={tab} onValueChange={(v) => setTab(v as typeof tab)}>
        <ListFrame>
          <PageHeader flush title="Attendance" summary="Face recognition clock-in and clock-out, and attendance records"
            actions={<>
              <Button asChild>
                <Link href="/clock?kiosk=1" target="_blank"><Camera aria-hidden /> Kiosk clock <ExternalLink aria-hidden className="text-fg-subtle" /></Link>
              </Button>
              <Button variant="primary" onClick={() => setShowLog(true)}><Plus aria-hidden /> Log attendance</Button>
            </>} />
          <TabsList className="bg-surface px-4">
            <TabsTrigger value="records">Records</TabsTrigger>
            <TabsTrigger value="overrides">Pending overrides <CountBadge count={pendingOverrides.length} /></TabsTrigger>
            <TabsTrigger value="clock_events">Clock events</TabsTrigger>
            <TabsTrigger value="attempts">Attempts</TabsTrigger>
          </TabsList>

          <TabsContent value="records" className="pt-0">
            <FilterChips
              chips={Object.keys(STATUS_LABELS).map((k) => ({ id: k, label: STATUS_LABELS[k], active: statusChip === k, count: counts[k] ?? 0 }))}
              onToggle={(id) => setStatusChip((cur) => (cur === id ? "" : id))} onClear={() => setStatusChip("")}>
              <Select aria-label="Month" value={filterMonth} onChange={e => setFilterMonth(Number(e.target.value))} className="w-24"
                options={MONTH_NAMES.map((m, i) => ({ value: String(i + 1), label: m }))} />
              <Select aria-label="Year" value={filterYear} onChange={e => setFilterYear(Number(e.target.value))} className="w-24"
                options={years.map(y => ({ value: String(y), label: String(y) }))} />
              <Select aria-label="Employee" value={filterEmployee} onChange={e => setFilterEmployee(e.target.value)} className="w-48"
                options={[{ value: "", label: "All employees" }, ...employeeOptions]} />
              <span className="text-xs text-fg-muted tabular">{totalDays} records</span>
            </FilterChips>
            <DataTable label="Attendance records" columns={recordColumns} data={visibleRecords} getRowId={(r) => r.id} loading={loadingRec}
              empty={{ icon: Calendar, title: "No records for this period", description: "Pick another month, or log attendance." }} />
          </TabsContent>

          <TabsContent value="overrides" className="pt-0">
            <DataTable label="Pending overrides" columns={overrideColumns} data={pendingOverrides} getRowId={(r) => r.id} loading={loadingOverrides}
              empty={{ icon: ShieldAlert, title: "No pending override requests", description: "Clock-ins the system could not confirm show up here." }} />
          </TabsContent>

          <TabsContent value="clock_events" className="pt-0">
            <DataTable label="Clock events" columns={clockColumns} data={clockEvents} getRowId={(r) => r.id} loading={loadingClock}
              empty={{ icon: Camera, title: "No clock events yet" }} />
          </TabsContent>

          <TabsContent value="attempts" className="pt-0">
            {tab === "attempts" && <AttemptsPanel days={7} />}
          </TabsContent>
        </ListFrame>
      </Tabs>

      <Dialog open={showLog} onOpenChange={setShowLog} title="Log attendance"
        footer={<>
          <Button onClick={() => setShowLog(false)}>Cancel</Button>
          <Button variant="primary" onClick={logAttendance} disabled={!form.employee_id || !form.date} loading={saving}>
            {saving ? "Saving..." : "Log attendance"}
          </Button>
        </>}>
        <div className="space-y-4">
          <FieldGrid cols={2}>
            <Field label="Employee" required>
              <Select value={form.employee_id} onChange={e => setForm(f => ({ ...f, employee_id: e.target.value }))}
                placeholder="Select..." options={employeeOptions} />
            </Field>
            <Field label="Date" required>
              <Input type="date" value={form.date} onChange={e => setForm(f => ({ ...f, date: e.target.value }))} />
            </Field>
          </FieldGrid>
          <Field label="Status" required>
            <div role="radiogroup" aria-label="Status" className="grid grid-cols-2 gap-2 sm:grid-cols-4">
              {Object.entries(STATUS_LABELS).map(([key, label]) => (
                <button key={key} type="button" role="radio" aria-checked={form.status === key}
                  onClick={() => setForm(f => ({ ...f, status: key }))}
                  className={cn("h-11 rounded-lg border px-2 text-[13px] font-medium transition-colors md:h-8",
                    form.status === key ? "border-brand bg-brand text-brand-on" : "border-line-strong bg-surface text-fg hover:bg-surface-2")}>
                  {label}
                </button>
              ))}
            </div>
          </Field>
          {["present","late","remote","half_day"].includes(form.status) && (
            <FieldGrid cols={2}>
              <Field label="Check in"><Input type="time" value={form.check_in} onChange={e => setForm(f => ({ ...f, check_in: e.target.value }))} /></Field>
              <Field label="Check out"><Input type="time" value={form.check_out} onChange={e => setForm(f => ({ ...f, check_out: e.target.value }))} /></Field>
              <Field label="Late (min)">
                <Input numeric type="number" min="0" value={form.late_minutes} onChange={e => setForm(f => ({ ...f, late_minutes: e.target.value }))} />
              </Field>
              <Field label="Overtime (hrs)">
                <Input numeric type="number" min="0" step="0.5" value={form.overtime_hours} onChange={e => setForm(f => ({ ...f, overtime_hours: e.target.value }))} />
              </Field>
            </FieldGrid>
          )}
          <Field label="Notes"><Input value={form.notes} onChange={e => setForm(f => ({ ...f, notes: e.target.value }))} /></Field>
        </div>
      </Dialog>

      <Dialog open={!!overrideModal} onOpenChange={(o) => { if (!o) setOverrideModal(null); }} title="Review clock event"
        description={overrideModal ? `${stamp(overrideModal.captured_at)}, ${overrideModal.event_type.replace("_", " ")}` : undefined}
        footer={<>
          <Button onClick={() => setOverrideModal(null)}>Cancel</Button>
          <Button variant="danger" onClick={() => submitOverride("reject")} disabled={saving}>Reject</Button>
          <Button variant="primary" onClick={() => submitOverride("approve")} disabled={!overrideEmployeeId} loading={saving}>
            {saving ? "Saving..." : "Approve"}
          </Button>
        </>}>
        {overrideModal && (
          <div className="space-y-4">
            <div className="rounded-lg border border-line bg-surface-2 p-3">
              <KeyValues items={[
                { label: "Status", value: <StatusPill family="face_check" status={overrideModal.recognition_status} label={REC_LABELS[overrideModal.recognition_status]} /> },
                { label: "Confidence", value: pct(overrideModal.recognition_confidence, 1), numeric: true },
                { label: "Liveness", value: livenessLabel(overrideModal) },
                { label: "Distance", value: meters(overrideModal.geo_distance_meters), numeric: true },
              ]} />
              {overrideModal.recognition_status === "kiosk_mismatch" && overrideModal.details && (
                <p className="mt-3 border-t border-line pt-2 text-[13px] text-fg">
                  The kiosk identified{" "}
                  <strong>{employeeName(overrideModal.details.kiosk_claim)}</strong>, but the server matched{" "}
                  <strong>{employeeName(overrideModal.details.server_match)}</strong>
                  {overrideModal.details.server_distance != null ? ` (distance ${overrideModal.details.server_distance.toFixed(2)})` : ""}.
                </p>
              )}
            </div>
            <Field label="Assign to employee">
              <Select value={overrideEmployeeId} onChange={e => setOverrideEmployeeId(e.target.value)} placeholder="Select employee..." options={employeeOptions} />
            </Field>
            <Field label="Override notes">
              <Input placeholder="Reason for override..." value={overrideNotes} onChange={e => setOverrideNotes(e.target.value)} />
            </Field>
          </div>
        )}
      </Dialog>
    </div>
  );
}
