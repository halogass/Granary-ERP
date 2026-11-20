"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState, useCallback, useMemo } from "react";
import { crmApi } from "@/lib/api";
import OrgContactPicker from "@/components/crm/OrgContactPicker";
import { toast } from "sonner";
import { extractApiError } from "@/lib/apiError";
import {
  Phone, Mail, Users, Calendar, FileText,
  Plus, Search, CheckCircle2, StickyNote, Trash2,
} from "lucide-react";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame,
  PageHeader, Pill, SavedViewTabs, Textarea, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";

// Shape of GET /crm/activities.
interface Activity {
  id: string;
  entity_type: string;
  entity_id: string;
  activity_type: string;
  subject?: string | null;
  content?: string | null;
  outcome?: string | null;
  is_completed: boolean;
  scheduled_at?: string | null;
  completed_at?: string | null;
  duration_minutes?: number | null;
  created_by_name?: string | null;
}

const TYPE_META: Record<string, { label: string; icon: React.ElementType }> = {
  call:     { label: "Call",     icon: Phone },
  email:    { label: "Email",    icon: Mail },
  meeting:  { label: "Meeting",  icon: Users },
  task:     { label: "Task",     icon: CheckCircle2 },
  note:     { label: "Note",     icon: StickyNote },
  demo:     { label: "Demo",     icon: FileText },
};

type Filters = { when?: "today" | "overdue" | "open" | "done" };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "today", label: "Today", filters: { when: "today" } },
  { id: "overdue", label: "Overdue", filters: { when: "overdue" } },
  { id: "open", label: "Open", filters: { when: "open" } },
  { id: "done", label: "Done", filters: { when: "done" } },
];

const isToday = (iso?: string | null) => !!iso && new Date(iso).toLocaleDateString("en-GB") === new Date().toLocaleDateString("en-GB");
const isOverdue = (a: Activity) => !!a.scheduled_at && !a.is_completed && new Date(a.scheduled_at) < new Date();
const fmtWhen = (iso: string) => new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });

export default function ActivitiesPage() {
  const [activities, setActivities] = useState<Activity[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [typeFilter, setTypeFilter] = useState("");
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState({
    type: "call", subject: "", notes: "",
    scheduled_at: new Date().toISOString().slice(0, 16),
    duration_minutes: "30",
    organization_id: "", contact_id: "",
  });
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const views = useSavedViews<Filters>("crm.activities", VIEWS);
  const when = views.active?.filters.when;

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<Activity>(crmApi.listActivities);
      setActivities(rows);
    } catch { setActivities([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const filtered = useMemo(() => activities.filter((a) => {
    const matchSearch = !search || (a.subject ?? "").toLowerCase().includes(search.toLowerCase());
    const matchType = !typeFilter || a.activity_type === typeFilter;
    const matchWhen = !when
      || (when === "today" && isToday(a.scheduled_at))
      || (when === "overdue" && isOverdue(a))
      || (when === "open" && !a.is_completed)
      || (when === "done" && a.is_completed);
    return matchSearch && matchType && matchWhen;
  }), [activities, search, typeFilter, when]);

  const todayCount = activities.filter((a) => isToday(a.scheduled_at)).length;
  const overdueCount = activities.filter(isOverdue).length;
  const calls = activities.filter((a) => a.activity_type === "call").length;
  const meetings = activities.filter((a) => a.activity_type === "meeting").length;

  async function deleteActivity(id: string) {
    setDeleting(true);
    try {
      await crmApi.deleteActivity(id);
      setConfirmDeleteId(null);
      setActivities((prev) => prev.filter((a) => a.id !== id));
    } catch { /* keep */ } finally { setDeleting(false); }
  }

  async function createActivity() {
    // An activity belongs to a record: the contact if one is picked, else the organization.
    const entity = form.contact_id
      ? { entity_type: "contact", entity_id: form.contact_id, contact_id: form.contact_id }
      : { entity_type: "organization", entity_id: form.organization_id };
    if (!form.subject.trim() || !entity.entity_id) return;
    setSaving(true);
    try {
      await crmApi.createActivity({
        ...entity,
        activity_type: form.type,
        subject: form.subject,
        content: form.notes || undefined,
        // datetime-local is local time: send an explicit UTC instant
        scheduled_at: form.scheduled_at ? new Date(form.scheduled_at).toISOString() : undefined,
        duration_minutes: parseInt(form.duration_minutes) || undefined,
      });
      setShowModal(false);
      setForm({ type: "call", subject: "", notes: "", scheduled_at: new Date().toISOString().slice(0, 16), duration_minutes: "30", organization_id: "", contact_id: "" });
      await load();
    } catch (err) {
      toast.error(extractApiError(err, "Failed to log activity"));
    } finally { setSaving(false); }
  }

  const columns = useMemo<ColumnDef<Activity>[]>(() => [
    {
      id: "subject", header: "Activity", accessorFn: (a) => a.subject || TYPE_META[a.activity_type]?.label || a.activity_type,
      cell: (c) => {
        const a = c.row.original;
        return (
          <div className="min-w-0">
            <p className="flex items-center gap-1.5 font-medium text-fg">
              {a.is_completed && <CheckCircle2 className="size-3.5 shrink-0 text-tone-blue" aria-label="Done" />}
              <span className="truncate">{c.getValue<string>()}</span>
            </p>
            {a.content && <p className="max-w-lg truncate text-xs text-fg-muted">{a.content}</p>}
          </div>
        );
      },
    },
    {
      accessorKey: "activity_type", header: "Type",
      cell: (c) => {
        const meta = TYPE_META[c.getValue<string>()] ?? TYPE_META.note;
        return <Pill icon={meta.icon}>{meta.label}</Pill>;
      },
    },
    {
      accessorKey: "scheduled_at", header: "When", meta: { numeric: true },
      cell: (c) => {
        const a = c.row.original;
        if (!a.scheduled_at) return <span className="text-fg-subtle">-</span>;
        return isOverdue(a)
          ? <Pill tone="red">Overdue, {fmtWhen(a.scheduled_at)}</Pill>
          : fmtWhen(a.scheduled_at);
      },
    },
    {
      accessorKey: "duration_minutes", header: "Duration", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => c.getValue<number>() ? `${c.getValue<number>()}m` : <span className="text-fg-subtle">-</span>,
    },
    {
      accessorKey: "entity_type", header: "Record", meta: { hideOnMobile: true },
      cell: (c) => <span className="capitalize text-fg-muted">{c.getValue<string>()}</span>,
    },
    {
      accessorKey: "created_by_name", header: "By", meta: { hideOnMobile: true },
      cell: (c) => c.getValue<string>() || <span className="text-fg-subtle">-</span>,
    },
  ], []);

  function details(a: Activity) {
    const meta = TYPE_META[a.activity_type] ?? TYPE_META.note;
    return (
      <div className="flex flex-wrap items-start gap-6">
        <KeyValues className="min-w-[260px] flex-1" items={[
          { label: "Type", value: meta.label },
          { label: "Scheduled", value: a.scheduled_at ? fmtWhen(a.scheduled_at) : null, numeric: true },
          { label: "Completed", value: a.completed_at ? fmtWhen(a.completed_at) : a.is_completed ? "Yes" : "No", numeric: true },
          { label: "Duration", value: a.duration_minutes ? `${a.duration_minutes} min` : null, numeric: true },
          { label: "Record", value: <span className="capitalize">{a.entity_type}</span> },
          { label: "By", value: a.created_by_name || null },
          ...(a.outcome ? [{ label: "Outcome", value: a.outcome }] : []),
          ...(a.content ? [{ label: "Notes", value: <span className="whitespace-pre-line">{a.content}</span> }] : []),
        ]} />
        <div className="flex flex-wrap items-center gap-2">
          {confirmDeleteId === a.id ? (
            <>
              <span className="text-[13px] text-fg-muted">Delete this activity?</span>
              <Button size="sm" variant="danger" loading={deleting} onClick={() => deleteActivity(a.id)}>Delete</Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>Cancel</Button>
            </>
          ) : (
            <IconButton size="sm" variant="danger" icon={Trash2} label="Delete activity" onClick={() => setConfirmDeleteId(a.id)} />
          )}
        </div>
      </div>
    );
  }

  const canSave = !!form.subject.trim() && !!(form.contact_id || form.organization_id) && !saving;

  return (
    <>
      <ListFrame>
        <PageHeader flush title="Activities"
          summary={<>
            {todayCount} today, <span className={cn(overdueCount > 0 && "font-medium text-tone-red")}>{overdueCount} overdue</span>, {calls} calls, {meetings} meetings
          </>}
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Log activity</Button>} />
        <SavedViewTabs state={views} currentFilters={views.active?.filters}
          counts={{ today: todayCount, overdue: overdueCount }} alertIds={["overdue"]} />
        <FilterChips
          chips={Object.entries(TYPE_META).map(([key, meta]) => ({
            id: key, active: typeFilter === key,
            label: <><meta.icon className="size-3" aria-hidden /> {meta.label}</>,
          }))}
          onToggle={(id) => setTypeFilter(typeFilter === id ? "" : id)}
          onClear={() => setTypeFilter("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)}
            placeholder="Search activities" aria-label="Search activities" className="w-56" />
        </FilterChips>
        <DataTable label="Activities" columns={columns} data={filtered} getRowId={(a) => a.id} loading={loading}
          onExpandedChange={() => setConfirmDeleteId(null)}
          empty={{ icon: Calendar, title: "No activities in this view", description: "Log calls, meetings, and follow-ups here.",
            action: <Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Log activity</Button> }}
          renderDetails={details}
          footer={<span className="tabular">{filtered.length} activit{filtered.length !== 1 ? "ies" : "y"}</span>} />
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="Log activity"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={createActivity} disabled={!canSave} loading={saving}>
            {saving ? "Saving" : "Log activity"}
          </Button>
        </>}>
        <div className="space-y-4">
          <div role="group" aria-label="Type">
            <p className="mb-1.5 text-[13px] font-medium text-fg">Type</p>
            <div className="flex flex-wrap gap-1.5">
              {Object.entries(TYPE_META).map(([key, meta]) => (
                <button key={key} type="button" aria-pressed={form.type === key} onClick={() => setForm({ ...form, type: key })}
                  className={cn("inline-flex min-h-9 items-center gap-1.5 rounded-lg border px-2.5 text-xs transition-colors md:min-h-7",
                    form.type === key ? "border-fg bg-fg text-surface" : "border-line bg-surface text-fg hover:bg-surface-2")}>
                  <meta.icon className="size-3" aria-hidden />
                  {meta.label}
                </button>
              ))}
            </div>
          </div>

          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <OrgContactPicker
              organizationId={form.organization_id}
              contactId={form.contact_id}
              onChange={(next) => setForm({ ...form, ...next })}
              labelClass="mb-1.5 block text-[13px] font-medium text-fg"
              orgLabel="Organization *"
              contactLabel="Contact"
            />
          </div>

          <Field label="Subject" required>
            <Input value={form.subject} onChange={(e) => setForm({ ...form, subject: e.target.value })}
              placeholder="e.g. Follow-up call with Acme Ltd" />
          </Field>

          <FieldGrid cols={2}>
            <Field label="Scheduled">
              <Input type="datetime-local" value={form.scheduled_at} onChange={(e) => setForm({ ...form, scheduled_at: e.target.value })} />
            </Field>
            <Field label="Duration">
              <Input type="number" numeric trailing="min" value={form.duration_minutes} onChange={(e) => setForm({ ...form, duration_minutes: e.target.value })}
                placeholder="30" />
            </Field>
          </FieldGrid>

          <Field label="Notes">
            <Textarea value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })}
              rows={3} placeholder="What happened or needs to happen?" />
          </Field>
        </div>
      </Dialog>
    </>
  );
}
