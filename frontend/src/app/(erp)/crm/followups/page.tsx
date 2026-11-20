"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { AlertTriangle, BellRing, Check, Clock, MessageSquare, X } from "lucide-react";
import {
  Button, DataTable, FilterChips, IconButton, KeyValues, ListFrame, PageHeader, SavedViewTabs, StatusPill,
  toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { remindersApi, type CrmReminder } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";

const SOURCE_LABEL: Record<string, string> = {
  manual: "Set by you",
  no_reply: "No reply from the customer",
  stale_lead: "Lead has gone quiet",
  stale_deal: "Deal stuck in a stage",
  quotation: "Quotation with no answer",
  recontact: "Re-contact a lost or disqualified customer",
  next_action: "Planned next action is due",
};

type Tab = "overdue" | "today" | "upcoming" | "done";
type Filters = { tab?: Tab; scope?: "mine" | "team" };

const VIEWS: SavedView<Filters>[] = [
  { id: "overdue", label: "Overdue", filters: { tab: "overdue" } },
  { id: "today", label: "Today", filters: { tab: "today" } },
  { id: "upcoming", label: "Upcoming", filters: { tab: "upcoming" } },
  { id: "done", label: "Done", filters: { tab: "done" } },
];

const EMPTY: Record<Tab, { title: string; description?: string }> = {
  overdue: { title: "Nothing overdue, nice." },
  today: { title: "Nothing due today." },
  upcoming: { title: "Nothing coming up.", description: "Set one with Remind me on a chat, lead, deal or quotation." },
  done: { title: "No completed follow-ups yet." },
};

function when(iso: string) {
  return new Date(iso).toLocaleString("en-GB", { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
}

function bucket(r: CrmReminder): Tab {
  if (r.status !== "open") return "done";
  const due = new Date(r.due_at).getTime();
  const endOfDay = new Date().setHours(23, 59, 59, 999);
  if (due <= Date.now()) return "overdue";
  return due <= endOfDay ? "today" : "upcoming";
}

export default function FollowUpsPage() {
  const views = useSavedViews<Filters>("crm.followups", VIEWS);
  const [scope, setScope] = useState<"mine" | "team">("mine");
  const [open, setOpen] = useState<CrmReminder[]>([]);
  const [done, setDone] = useState<CrmReminder[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [teamAllowed, setTeamAllowed] = useState(true);
  const tab: Tab = views.active?.filters.tab ?? "overdue";

  // A saved view may carry the team scope
  useEffect(() => {
    const s = views.active?.filters.scope;
    if (s && (s === "mine" || teamAllowed)) setScope(s);
  }, [views.active, teamAllowed]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [openRes, doneRes] = await Promise.all([
        fetchAllPages<CrmReminder>((p) => remindersApi.list(p), { scope, status: "open" }),
        remindersApi.list({ scope, status: "done", limit: 50 }),
      ]);
      setOpen(openRes.rows);
      setDone(doneRes.data);
    } catch (e) {
      if (scope === "team") { setTeamAllowed(false); setScope("mine"); }
      else toast.error(extractApiError(e, "Could not load your follow-ups."));
    } finally {
      setLoading(false);
    }
  }, [scope]);
  useEffect(() => { load(); }, [load]);

  const act = useCallback(async (r: CrmReminder, action: "done" | "cancel" | "snooze", days?: number) => {
    setBusy(r.id);
    try {
      if (action === "done") await remindersApi.done(r.id);
      else if (action === "cancel") await remindersApi.cancel(r.id);
      else await remindersApi.snooze(r.id, days === 3 ? { business_days: 3 } : { days });
      toast.success(action === "snooze" ? "Snoozed" : action === "done" ? "Marked done" : "Cancelled");
      load();
    } catch (e) {
      toast.error(extractApiError(e, "Could not update the reminder."));
    } finally {
      setBusy(null);
    }
  }, [load]);

  const rows = (tab === "done" ? done : open.filter((r) => bucket(r) === tab));
  const counts = {
    overdue: open.filter((r) => bucket(r) === "overdue").length,
    today: open.filter((r) => bucket(r) === "today").length,
    upcoming: open.filter((r) => bucket(r) === "upcoming").length,
    done: done.length,
  };

  const columns = useMemo<ColumnDef<CrmReminder>[]>(() => [
    {
      id: "title", header: "Follow-up", accessorKey: "title",
      cell: (c) => {
        const r = c.row.original;
        const overdue = bucket(r) === "overdue";
        const Icon = overdue ? AlertTriangle : r.entity_type === "whatsapp_chat" ? MessageSquare : Clock;
        return (
          <div className="flex min-w-[14rem] items-center gap-2">
            <Icon className={overdue ? "size-4 shrink-0 text-tone-red" : "size-4 shrink-0 text-fg-subtle"} aria-hidden />
            <Link href={r.link} className="font-medium text-fg hover:text-brand-ink">{r.title}</Link>
          </div>
        );
      },
    },
    {
      id: "source", header: "Why", accessorKey: "source", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{SOURCE_LABEL[c.row.original.source] ?? c.row.original.source}</span>,
    },
    ...(scope === "team" ? [{
      id: "owner", header: "Owner", accessorFn: (r: CrmReminder) => r.owner_name ?? "", meta: { hideOnMobile: true },
    } as ColumnDef<CrmReminder>] : []),
    {
      id: "due", header: "Due", accessorKey: "due_at", meta: { numeric: true },
      cell: (c) => when(c.row.original.due_at),
    },
    {
      id: "status", header: "Status", accessorFn: (r) => bucket(r), enableSorting: false,
      cell: (c) => {
        const r = c.row.original;
        const b = r.status === "open" ? bucket(r) : r.status;
        return <StatusPill family="followup" status={b} />;
      },
    },
    {
      id: "actions", header: () => <span className="sr-only">Actions</span>, enableSorting: false, meta: { align: "right" },
      cell: (c) => {
        const r = c.row.original;
        if (r.status !== "open") return null;
        const off = busy === r.id;
        return (
          <div className="flex items-center justify-end gap-1.5">
            <Button size="sm" onClick={() => act(r, "snooze", 1)} disabled={off}>+1 day</Button>
            <Button size="sm" className="hidden md:inline-flex" onClick={() => act(r, "snooze", 3)} disabled={off}>+3 business days</Button>
            <Button size="sm" variant="primary" onClick={() => act(r, "done")} disabled={off}><Check aria-hidden /> Done</Button>
            <IconButton size="sm" label="Drop this reminder" icon={X} onClick={() => act(r, "cancel")} disabled={off} />
          </div>
        );
      },
    },
  ], [scope, busy, act]);

  return (
    <ListFrame>
      <PageHeader flush title="Follow-ups"
        summary={`${counts.overdue} overdue, ${counts.today} today, ${counts.upcoming} upcoming`} />
      <SavedViewTabs state={views} currentFilters={{ tab, scope }}
        counts={counts} alertIds={["overdue"]} />
      <FilterChips
        chips={teamAllowed ? [{ id: "team", label: "Whole team", active: scope === "team" }] : []}
        onToggle={() => setScope((s) => (s === "team" ? "mine" : "team"))}
        onClear={() => setScope("mine")}>
        <span className="text-[13px] text-fg-muted">Set by you, or raised when a conversation goes quiet.</span>
      </FilterChips>
      <DataTable label="Follow-ups" columns={columns} data={rows} getRowId={(r) => r.id} loading={loading}
        empty={{ icon: BellRing, ...EMPTY[tab] }}
        renderDetails={(r) => (
          <div className="flex flex-wrap items-start gap-6">
            <KeyValues className="min-w-[260px] flex-1" items={[
              { label: "Why", value: SOURCE_LABEL[r.source] ?? r.source },
              { label: "Due", value: when(r.due_at), numeric: true },
              { label: "Note", value: r.note || "-" },
              { label: "Snoozed", value: r.snooze_count > 0 ? `${r.snooze_count} times` : "Never", numeric: true },
              ...(r.owner_name ? [{ label: "Owner", value: r.owner_name }] : []),
              ...(r.completed_at ? [{ label: "Completed", value: when(r.completed_at), numeric: true }] : []),
            ]} />
            <div className="flex flex-wrap gap-2">
              {r.status === "open" && (
                <Button size="sm" className="md:hidden" onClick={() => act(r, "snooze", 3)} disabled={busy === r.id}>+3 business days</Button>
              )}
              <Button size="sm" asChild><Link href={r.link}>Open record</Link></Button>
            </div>
          </div>
        )} />
    </ListFrame>
  );
}
