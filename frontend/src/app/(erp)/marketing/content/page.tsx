"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { marketingApi } from "@/lib/api";
import { CalendarDays, Plus, Trash2, ArrowRight, ChevronLeft, ChevronRight, List } from "lucide-react";
import { extractApiError } from "@/lib/apiError";
import { cn } from "@/lib/utils";
import ApprovalPanel from "@/components/approvals/ApprovalPanel";
import {
  Button, DataTable, Dialog, ErrorToast, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame,
  PageHeader, Pill, SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews,
  type ColumnDef, type SavedView, type Tone,
} from "@/components/ui";

interface ContentItem {
  id: string;
  title: string;
  content_type: string;
  platform: string;
  status: string;
  planned_date?: string | null;   // YYYY-MM-DD
  published_at?: string | null;
  campaign_id?: string | null;
  campaign_name?: string | null;
  content_body?: string | null;
}

const TYPE_TONES: Record<string, Tone> = {
  blog_post: "blue", social_post: "violet", email: "neutral", video: "orange",
  infographic: "blue", press_release: "orange", whitepaper: "neutral", other: "neutral",
};

const PLATFORM_LABELS: Record<string, string> = {
  linkedin: "LinkedIn", instagram: "Instagram", twitter: "X/Twitter", facebook: "Facebook",
  website: "Website", email: "Email", youtube: "YouTube", all: "All",
};

// models/marketing.py ContentCalendarItem.status: planned | in_progress | review | approved | scheduled | published
// (idea/draft kept for older rows)
const STATUS_LABELS: Record<string, string> = {
  planned: "Planned", in_progress: "In progress", idea: "Idea", draft: "Draft", review: "In review",
  approved: "Approved", scheduled: "Scheduled", published: "Published", cancelled: "Cancelled",
};

const NEXT_STATUS: Record<string, string> = {
  planned: "in_progress",
  in_progress: "review",
  idea: "draft",
  draft: "review",
  // review -> approved happens only through the approval flow (the Approval panel under the item).
  approved: "scheduled",
  scheduled: "published",
};

const PLATFORMS = Object.keys(PLATFORM_LABELS).filter((p) => p !== "all");
const CONTENT_TYPES = Object.keys(TYPE_TONES);
const WEEKS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

function getWeekStart(date: Date) {
  const d = new Date(date);
  const day = d.getDay();
  const diff = (day === 0 ? -6 : 1 - day);
  d.setDate(d.getDate() + diff);
  d.setHours(0, 0, 0, 0);
  return d;
}

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "planned", label: "Planned", filters: { status: "planned" } },
  { id: "in_progress", label: "In progress", filters: { status: "in_progress" } },
  { id: "review", label: "In review", filters: { status: "review" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "scheduled", label: "Scheduled", filters: { status: "scheduled" } },
  { id: "published", label: "Published", filters: { status: "published" } },
  { id: "idea", label: "Ideas", filters: { status: "idea" } },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "cancelled", label: "Cancelled", filters: { status: "cancelled" } },
];

const EMPTY_FORM = { title: "", content_type: "social_post", platform: "linkedin", content_body: "", planned_at: "" };

function itemDate(item: ContentItem) {
  if (item.planned_date) return `Planned ${new Date(`${item.planned_date}T00:00:00`).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" })}`;
  if (item.published_at) return `Published ${new Date(item.published_at).toLocaleDateString("en-GB", { day: "numeric", month: "short" })}`;
  return "No date set";
}

export default function ContentCalendarPage() {
  const views = useSavedViews<Filters>("marketing.content", VIEWS);
  const [actionError, setActionError] = useState("");
  const [items, setItems] = useState<ContentItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [showModal, setShowModal] = useState(false);
  const [view, setView] = useState<"list" | "week">("list");
  const [filterPlatform, setFilterPlatform] = useState("all");
  const [weekOffset, setWeekOffset] = useState(0);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const { rows } = await fetchAllPages<ContentItem>(marketingApi.listContent);
      setItems(rows);
    } catch { setItems([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function create() {
    if (!form.title.trim()) return;
    setSaving(true);
    try {
      await marketingApi.createContent({
        title: form.title.trim(),
        content_type: form.content_type,
        platform: form.platform,
        content_body: form.content_body.trim() || undefined,
        planned_date: form.planned_at ? form.planned_at.slice(0, 10) : undefined,
      });
      setShowModal(false);
      setForm(EMPTY_FORM);
      await load();
    } finally { setSaving(false); }
  }

  async function advance(id: string, nextStatus: string) {
    setActionError("");
    try {
      await marketingApi.updateContentStatus(id, nextStatus);
    } catch (e) {
      setActionError(extractApiError(e, "Could not update the content status."));
    }
    window.dispatchEvent(new Event("approvals:changed"));
    await load();
  }

  async function deleteContent(id: string) {
    try {
      await marketingApi.deleteContent(id);
      setConfirmDeleteId(null);
      setItems((prev) => prev.filter((i) => i.id !== id));
    } catch { /* keep */ }
  }

  const published = items.filter((i) => i.status === "published").length;
  const scheduled = items.filter((i) => i.status === "scheduled").length;
  const inPipeline = items.filter((i) => ["planned", "in_progress", "idea", "draft", "review", "approved"].includes(i.status)).length;

  const filterStatus = views.active?.filters.status ?? "";
  const filters: Filters = filterStatus ? { status: filterStatus } : {};
  const filtered = items.filter((i) => {
    if (filterStatus && i.status !== filterStatus) return false;
    if (filterPlatform !== "all" && i.platform !== filterPlatform) return false;
    return true;
  });

  // Week view data
  const weekStart = getWeekStart(new Date());
  weekStart.setDate(weekStart.getDate() + weekOffset * 7);
  const weekDays = Array.from({ length: 7 }, (_, i) => {
    const d = new Date(weekStart);
    d.setDate(d.getDate() + i);
    return d;
  });
  const weekItems = (day: Date) => items.filter((i) => {
    if (!i.planned_date) return false;
    // planned_date is a plain date: compare as local Y-M-D, not via UTC parsing
    const [y, m, d] = i.planned_date.split("-").map(Number);
    return new Date(y, m - 1, d).toDateString() === day.toDateString();
  });
  const today = new Date().toDateString();

  function rowActions(item: ContentItem) {
    const nextStatus = NEXT_STATUS[item.status];
    if (confirmDeleteId === item.id) {
      return (
        <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
          <span className="text-xs font-semibold text-tone-red">Delete?</span>
          <Button size="sm" variant="danger" onClick={() => deleteContent(item.id)}>Yes</Button>
          <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
        </div>
      );
    }
    return (
      <div className="flex items-center justify-end gap-1 whitespace-nowrap">
        {nextStatus && (
          <Button size="sm" variant="ghost" onClick={() => advance(item.id, nextStatus)}>
            <ArrowRight aria-hidden /> {STATUS_LABELS[nextStatus] ?? nextStatus}
          </Button>
        )}
        <IconButton size="sm" icon={Trash2} label={`Delete ${item.title}`} onClick={() => setConfirmDeleteId(item.id)} />
      </div>
    );
  }

  const columns = useMemo<ColumnDef<ContentItem>[]>(() => [
    {
      accessorKey: "title", header: "Title",
      cell: (c) => (
        <div className="min-w-0">
          <p className="max-w-md truncate font-medium text-fg">{c.row.original.title}</p>
          <p className="text-xs text-fg-muted">{itemDate(c.row.original)}{c.row.original.campaign_name ? `, ${c.row.original.campaign_name}` : ""}</p>
        </div>
      ),
    },
    {
      accessorKey: "content_type", header: "Type", meta: { hideOnMobile: true },
      cell: (c) => <Pill tone={TYPE_TONES[c.getValue<string>()] ?? "neutral"} className="capitalize">{c.getValue<string>().replace("_", " ")}</Pill>,
    },
    { accessorKey: "platform", header: "Platform", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{PLATFORM_LABELS[c.getValue<string>()] ?? c.getValue<string>()}</span> },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="content" status={c.getValue<string>()} label={STATUS_LABELS[c.getValue<string>()]} /> },
    { id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" }, cell: (c) => rowActions(c.row.original) },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId]);

  const viewToggle = (
    <div className="flex rounded-lg border border-line-strong p-0.5" role="group" aria-label="Layout">
      {([["list", List, "List"], ["week", CalendarDays, "Week"]] as const).map(([v, Icon, label]) => (
        <button key={v} type="button" aria-pressed={view === v} onClick={() => setView(v)}
          className={cn("inline-flex min-h-10 items-center gap-1.5 rounded-md px-2.5 text-[13px] md:min-h-7",
            view === v ? "bg-surface-2 font-medium text-fg" : "text-fg-muted hover:text-fg")}>
          <Icon className="size-3.5" aria-hidden /> {label}
        </button>
      ))}
    </div>
  );

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={items.length} />
        <Stat label="In pipeline" value={inPipeline} />
        <Stat label="Scheduled" value={scheduled} />
        <Stat label="Published" value={published} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Content calendar" summary="Editorial planning across all platforms"
          actions={<>{viewToggle}<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> New content</Button></>} />

        {view === "week" ? (
          <>
            <div className="flex items-center justify-between border-b border-line px-4 py-2">
              <IconButton size="sm" icon={ChevronLeft} label="Previous week" onClick={() => setWeekOffset((o) => o - 1)} />
              <p className="text-sm font-medium text-fg tabular">
                {weekDays[0].toLocaleDateString("en-GB", { day: "numeric", month: "short" })} to{" "}
                {weekDays[6].toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" })}
              </p>
              <IconButton size="sm" icon={ChevronRight} label="Next week" onClick={() => setWeekOffset((o) => o + 1)} />
            </div>
            <div className="overflow-x-auto">
              <div className="grid min-h-[200px] min-w-[720px] grid-cols-7 divide-x divide-line-soft">
                {weekDays.map((day, di) => {
                  const dayItems = weekItems(day);
                  const isToday = day.toDateString() === today;
                  return (
                    <div key={di} className={cn("p-3", isToday && "bg-brand/[0.04]")}>
                      <p className={cn("mb-2 text-xs font-medium", isToday ? "text-brand-ink" : "text-fg-muted")}>
                        {WEEKS[di]} {day.getDate()}
                      </p>
                      <div className="space-y-1.5">
                        {dayItems.map((item) => (
                          <div key={item.id} className="rounded-md border border-line bg-surface-2 px-2 py-1.5 text-xs">
                            <p className="truncate font-medium text-fg">{item.title}</p>
                            <p className="text-fg-muted">{PLATFORM_LABELS[item.platform] ?? item.platform}</p>
                          </div>
                        ))}
                        {dayItems.length === 0 && <p className="text-xs text-fg-subtle">-</p>}
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          </>
        ) : (
          <>
            <SavedViewTabs state={views} currentFilters={filters} />
            <FilterChips
              chips={PLATFORMS.map((p) => ({ id: p, label: PLATFORM_LABELS[p], active: filterPlatform === p }))}
              onToggle={(id) => setFilterPlatform((cur) => (cur === id ? "all" : id))}
              onClear={() => setFilterPlatform("all")} />
            <DataTable label="Content items" columns={columns} data={filtered} getRowId={(r) => r.id} loading={loading}
              empty={{ icon: CalendarDays, title: "No content items", description: "Plan a post, article or video." }}
              renderDetails={(item) => (
                <div className="space-y-3">
                  <KeyValues cols={3} items={[
                    { label: "Date", value: itemDate(item) },
                    { label: "Campaign", value: item.campaign_name ?? "-" },
                    { label: "Platform", value: PLATFORM_LABELS[item.platform] ?? item.platform },
                    { label: "Brief", value: item.content_body ? <span className="whitespace-pre-wrap">{item.content_body}</span> : "-" },
                  ]} />
                  <ApprovalPanel entityType="marketing_content" entityId={item.id} className="mb-0" onChange={load} />
                </div>
              )} />
          </>
        )}
      </ListFrame>

      <Dialog open={showModal} onOpenChange={setShowModal} title="New content item"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving} disabled={!form.title.trim()}>{saving ? "Creating..." : "Create"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Title" required className="md:col-span-2">
            <Input value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} placeholder="e.g. Five ways to shorten your sales cycle" />
          </Field>
          <Field label="Type">
            <Select value={form.content_type} onChange={(e) => setForm({ ...form, content_type: e.target.value })}
              options={CONTENT_TYPES.map((t) => ({ value: t, label: t.replace("_", " ") }))} />
          </Field>
          <Field label="Platform">
            <Select value={form.platform} onChange={(e) => setForm({ ...form, platform: e.target.value })}
              options={PLATFORMS.map((p) => ({ value: p, label: PLATFORM_LABELS[p] }))} />
          </Field>
          <Field label="Planned date" className="md:col-span-2">
            <Input type="datetime-local" value={form.planned_at} onChange={(e) => setForm({ ...form, planned_at: e.target.value })} />
          </Field>
          <Field label="Brief / description" className="md:col-span-2">
            <Textarea value={form.content_body} onChange={(e) => setForm({ ...form, content_body: e.target.value })} rows={3} placeholder="Key messages, target audience, CTA" />
          </Field>
        </FieldGrid>
      </Dialog>
      <ErrorToast message={actionError} onClose={() => setActionError("")} />
    </div>
  );
}
