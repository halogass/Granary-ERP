"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useSession } from "next-auth/react";
import { ArrowUpRight, User2 } from "lucide-react";
import { systemApi, messagingApi, naryApi, remindersApi, financeApi, type CrmReminder } from "@/lib/api";
import { usePermissions } from "@/hooks/usePermissions";
import { formatCurrency } from "@/lib/currency";
import { NARY_VIEW } from "@/components/nary/NaryTabs";
import type { NaryCard } from "@/components/nary/types";
import type { MeetingRow } from "@/components/nary/Meetings";
import { mentionsToPlainText } from "@/lib/messageMentions";
import {
  Button, Card, CardHeader, CountBadge, DataTable, PageHeader, Stat, StatGroup, StatusPill, type ColumnDef,
} from "@/components/ui";

interface KPIs {
  open_leads: number;
  active_deals: number;
  overdue_invoices: number;
  active_projects: number;
  unread_notifications: number;
}

interface Conversation {
  other_user_id: string;
  other_user_name: string;
  last_message?: string;
  last_message_at?: string;
  unread_count: number;
  is_online: boolean;
}

interface MsgStats {
  online: number;
  unreadDms: number;
  pendingMemos: number;
  recent: Conversation[];
}

interface Invoice {
  id: string; number: string; status: string; total: number | null; amount_due?: number; amount_paid: number;
  currency?: string; due_date?: string; customer_name?: string | null;
}

const UNPAID = ["overdue", "sent", "partial", "partially_paid", "issued"];

function timeAgo(iso?: string) {
  if (!iso) return "";
  const diff = Date.now() - new Date(iso).getTime();
  const m = Math.floor(diff / 60000);
  if (m < 1) return "just now";
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

const fmtTime = (iso: string) => new Date(iso).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
const fmtWhen = (iso: string) => new Date(iso).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
const fmtDate = (iso?: string) => (iso ? new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short" }) : "-");
const isToday = (iso: string | null) => {
  if (!iso) return false;
  const d = new Date(iso);
  const n = new Date();
  return d.getFullYear() === n.getFullYear() && d.getMonth() === n.getMonth() && d.getDate() === n.getDate();
};

const QUICK_LINKS = [
  { label: "New quotation",    href: "/sales/quotations/new",     desc: "Create a price proposal" },
  { label: "Add lead",         href: "/crm/leads",                desc: "Track a new prospect" },
  { label: "Production order", href: "/manufacturing/production", desc: "Start a hardware build" },
  { label: "Start call",       href: "/calls",                    desc: "Video conference room" },
  { label: "Send message",     href: "/messaging",                desc: "Chat with the team" },
  { label: "Knowledge base",   href: "/knowledge/articles",       desc: "SOPs and documentation" },
];

const MODULE_SHORTCUTS = [
  { label: "CRM",           href: "/crm/deals",                desc: "Deals and pipeline" },
  { label: "Sales",         href: "/sales/quotations",         desc: "Quotations" },
  { label: "Finance",       href: "/finance/invoices",         desc: "Invoices" },
  { label: "Projects",      href: "/projects",                 desc: "Delivery" },
  { label: "Manufacturing", href: "/manufacturing/production", desc: "Production" },
  { label: "HR",            href: "/hr/employees",             desc: "People" },
  { label: "Partners",      href: "/partner-mgmt/accounts",    desc: "Channel" },
  { label: "Vendors",       href: "/vendor-mgmt/vendors",      desc: "Procurement" },
];

/** "Today": what needs you now, then the usual numbers and shortcuts. */
export default function DashboardPage() {
  const { data: session } = useSession();
  const { loaded, holds } = usePermissions();
  const firstName = session?.user?.name?.split(" ")[0] ?? "there";
  // The greeting and date follow the browser's clock, set after mount: worked out during render they
  // differ between the server (UTC) and the browser, and React rebuilds the page (hydration error).
  const [{ greeting, dateStr }, setNow] = useState({ greeting: "Hello", dateStr: "" });
  useEffect(() => {
    const now = new Date();
    const hour = now.getHours();
    setNow({
      greeting: hour < 12 ? "Good morning" : hour < 17 ? "Good afternoon" : "Good evening",
      dateStr: now.toLocaleDateString("en-GB", { weekday: "long", day: "numeric", month: "long", year: "numeric" }),
    });
  }, []);

  const [kpis, setKpis] = useState<KPIs | null>(null);
  const [msgStats, setMsgStats] = useState<MsgStats | null>(null);
  const [cards, setCards] = useState<NaryCard[] | null>(null);
  const [meetings, setMeetings] = useState<MeetingRow[] | null>(null);
  const [followups, setFollowups] = useState<CrmReminder[] | null>(null);
  const [invoices, setInvoices] = useState<Invoice[] | null>(null);
  const naryView = loaded && holds(NARY_VIEW);

  useEffect(() => {
    systemApi.getDashboardKpis()
      .then((res) => setKpis(res.data))
      .catch(() => setKpis(null));
  }, []);

  useEffect(() => {
    Promise.all([
      messagingApi.onlineUsers(),
      messagingApi.listConversations(),
      messagingApi.listMemos("received"),
    ])
      .then(([onlineRes, convoRes, memosRes]) => {
        const convos: Conversation[] = convoRes.data ?? [];
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const memos: any[] = memosRes.data ?? [];
        const myId = session?.user?.id;
        setMsgStats({
          online: (onlineRes.data?.online ?? []).length,
          unreadDms: convos.reduce((sum, c) => sum + (c.unread_count ?? 0), 0),
          pendingMemos: memos.filter((m) => {
            if (m.status !== "sent") return false;
            // Channel memo: check if I haven't acknowledged yet
            if (m.channel_id) {
              return !(m.acknowledgements ?? []).some((a: { user_id: string }) => a.user_id === myId);
            }
            // DM memo: check acknowledgements empty
            return (m.acknowledgements ?? []).length === 0;
          }).length,
          recent: convos.slice(0, 3),
        });
      })
      .catch(() => {});
  }, [session?.user?.id]);

  // What waits for you. Each list is the one its own page loads; one you may not see stays hidden.
  useEffect(() => {
    if (!naryView) return;
    naryApi.cards({ status: "pending" }).then((r) => setCards(r.data ?? [])).catch(() => setCards(null));
    naryApi.meetings().then((r) => setMeetings(r.data?.items ?? [])).catch(() => setMeetings(null));
  }, [naryView]);
  useEffect(() => {
    fetchAllPages<CrmReminder>((p) => remindersApi.list(p), { scope: "mine", status: "open" })
      .then(({ rows }) => setFollowups(rows))
      .catch(() => setFollowups(null));
    // Every unpaid invoice, one status at a time (the list filters on a single status).
    Promise.all(UNPAID.map((status) => fetchAllPages<Invoice>(financeApi.listInvoices, { status })))
      .then((pages) => setInvoices(pages.flatMap((p) => p.rows)))
      .catch(() => setInvoices(null));
  }, []);

  const dueFollowups = useMemo(() => {
    const endOfDay = new Date().setHours(23, 59, 59, 999);
    return (followups ?? []).filter((r) => new Date(r.due_at).getTime() <= endOfDay)
      .sort((a, b) => a.due_at.localeCompare(b.due_at));
  }, [followups]);
  const overdueFollowups = dueFollowups.filter((r) => new Date(r.due_at).getTime() <= Date.now()).length;
  const todayMeetings = useMemo(() => (meetings ?? []).filter((m) => m.status === "booked" && isToday(m.starts_at))
    .sort((a, b) => (a.starts_at ?? "").localeCompare(b.starts_at ?? "")), [meetings]);
  const unpaid = useMemo(() => (invoices ?? []).filter((i) => UNPAID.includes(i.status))
    .sort((a, b) => (a.status === "overdue" ? 0 : 1) - (b.status === "overdue" ? 0 : 1) || (a.due_date ?? "").localeCompare(b.due_date ?? "")), [invoices]);
  const overdueInvoices = kpis?.overdue_invoices ?? unpaid.filter((i) => i.status === "overdue").length;

  const cardCols = useMemo<ColumnDef<NaryCard>[]>(() => [
    {
      accessorKey: "label", header: "What Nary wants to do",
      cell: (c) => (
        <div className="min-w-0 py-1">
          <p className="truncate font-medium text-fg">{c.row.original.label}</p>
          {c.row.original.rationale && <p className="max-w-md truncate text-xs text-fg-muted">{c.row.original.rationale}</p>}
        </div>
      ),
    },
    { accessorKey: "created_at", header: "Since", meta: { numeric: true }, cell: (c) => timeAgo(c.getValue<string>()) },
  ], []);
  const followupCols = useMemo<ColumnDef<CrmReminder>[]>(() => [
    { accessorKey: "title", header: "Follow-up", cell: (c) => <span className="block max-w-xs truncate font-medium text-fg">{c.getValue<string>()}</span> },
    {
      accessorKey: "due_at", header: "Due", meta: { numeric: true },
      cell: (c) => {
        const overdue = new Date(c.getValue<string>()).getTime() <= Date.now();
        return <StatusPill family="followup" status={overdue ? "overdue" : "today"} label={fmtWhen(c.getValue<string>())} />;
      },
    },
  ], []);
  const meetingCols = useMemo<ColumnDef<MeetingRow>[]>(() => [
    { accessorKey: "starts_at", header: "Time", meta: { numeric: true, width: "72px" }, cell: (c) => fmtTime(c.getValue<string>()) },
    { accessorKey: "title", header: "Meeting", cell: (c) => <span className="block max-w-xs truncate font-medium text-fg">{c.getValue<string | null>() || "Meeting"}</span> },
    {
      id: "meet", header: "", meta: { align: "right" },
      cell: (c) => c.row.original.meet_link ? (
        <a href={c.row.original.meet_link} target="_blank" rel="noreferrer" className="text-[13px] font-medium text-brand-ink hover:underline">Join</a>
      ) : null,
    },
  ], []);
  const invoiceCols = useMemo<ColumnDef<Invoice>[]>(() => [
    {
      accessorKey: "number", header: "Invoice",
      cell: (c) => (
        <div className="min-w-0">
          <p className="font-mono text-[13px] text-fg">{c.row.original.number}</p>
          <p className="max-w-[12rem] truncate text-xs text-fg-muted">{c.row.original.customer_name ?? "-"}</p>
        </div>
      ),
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="invoice" status={c.getValue<string>()} /> },
    { accessorKey: "due_date", header: "Due", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmtDate(c.getValue<string | undefined>()) },
    {
      id: "due", header: "Due amount", meta: { numeric: true },
      cell: (c) => {
        const i = c.row.original;
        return formatCurrency(i.amount_due ?? Math.max(0, (i.total ?? 0) - (i.amount_paid ?? 0)), i.currency ?? "USD");
      },
    },
  ], []);

  const dash = (v: number | null | undefined) => (v == null ? "-" : v);

  return (
    <div className="mx-auto max-w-7xl space-y-4">
      <PageHeader title="Today" summary={`${greeting}, ${firstName}.${dateStr ? ` ${dateStr}` : ""}`} />

      {/* What needs you */}
      <StatGroup>
        {naryView && (
          <Stat label="Nary cards waiting" value={dash(cards?.length)} href="/nary/approvals"
            alert={!!cards?.length} note={cards?.length ? "Needs your decision" : "Nothing waiting"} />
        )}
        <Stat label="Follow-ups due" value={dash(followups ? dueFollowups.length : null)} href="/crm/followups"
          alert={overdueFollowups > 0} note={overdueFollowups ? `${overdueFollowups} overdue` : undefined} />
        {naryView && <Stat label="Meetings today" value={dash(meetings ? todayMeetings.length : null)} href="/nary/meetings" />}
        <Stat label="Overdue invoices" value={dash(kpis ? kpis.overdue_invoices : invoices ? overdueInvoices : null)} href="/finance/invoices"
          alert={overdueInvoices > 0} note={invoices ? `${unpaid.length} unpaid` : undefined} />
      </StatGroup>

      <div className="grid gap-4 lg:grid-cols-2">
        {naryView && (
          <Card>
            <CardHeader title={<span className="inline-flex items-center gap-2">Nary is waiting for you <CountBadge count={cards?.length} /></span>}
              actions={<Button size="sm" variant="ghost" asChild><Link href="/nary/approvals">Open approvals <ArrowUpRight aria-hidden /></Link></Button>} />
            <DataTable label="Nary cards waiting" columns={cardCols} data={(cards ?? []).slice(0, 6)} getRowId={(r) => r.id}
              loading={cards === null} maxHeight="none" rowHref={(r) => `/nary/approvals?card=${r.id}`}
              empty={{ title: "Nothing waiting", description: "Nary has no card for you right now.", compact: true }} />
          </Card>
        )}
        <Card>
          <CardHeader title="Follow-ups due today or overdue"
            actions={<Button size="sm" variant="ghost" asChild><Link href="/crm/followups">All follow-ups <ArrowUpRight aria-hidden /></Link></Button>} />
          <DataTable label="Follow-ups due" columns={followupCols} data={dueFollowups.slice(0, 6)} getRowId={(r) => r.id}
            loading={followups === null} maxHeight="none" rowHref={(r) => r.link || "/crm/followups"}
            empty={{ title: "Nothing due today", compact: true }} />
        </Card>
        {naryView && (
          <Card>
            <CardHeader title="Meetings today"
              actions={<Button size="sm" variant="ghost" asChild><Link href="/nary/meetings">All meetings <ArrowUpRight aria-hidden /></Link></Button>} />
            <DataTable label="Meetings today" columns={meetingCols} data={todayMeetings} getRowId={(r) => r.id}
              loading={meetings === null} maxHeight="none" rowHref={(r) => (r.deal_id ? `/crm/deals/${r.deal_id}` : "/nary/meetings")}
              empty={{ title: "No meetings today", compact: true }} />
          </Card>
        )}
        <Card>
          <CardHeader title="Unpaid invoices"
            actions={<Button size="sm" variant="ghost" asChild><Link href="/finance/invoices">All invoices <ArrowUpRight aria-hidden /></Link></Button>} />
          <DataTable label="Unpaid invoices" columns={invoiceCols} data={unpaid.slice(0, 6)} getRowId={(r) => r.id}
            loading={invoices === null && kpis === null} maxHeight="none" rowHref={(r) => `/finance/invoices/${r.id}`}
            empty={{ title: "No unpaid invoices", compact: true }} />
        </Card>
      </div>

      {/* The business at a glance */}
      <StatGroup>
        <Stat label="Open leads" value={dash(kpis?.open_leads)} href="/crm/leads" />
        <Stat label="Active deals" value={dash(kpis?.active_deals)} href="/crm/deals" />
        <Stat label="Active projects" value={dash(kpis?.active_projects)} href="/projects" />
        <Stat label="Unread messages" value={dash(msgStats?.unreadDms)} href="/messaging" alert={!!msgStats?.unreadDms} />
        <Stat label="Memos to acknowledge" value={dash(msgStats?.pendingMemos)} href="/messaging" alert={!!msgStats?.pendingMemos} />
        <Stat label="Online now" value={dash(msgStats?.online)} href="/messaging" />
      </StatGroup>

      <div className="grid gap-4 lg:grid-cols-3">
        {/* Messaging */}
        <Card>
          <CardHeader title="Recent conversations"
            actions={<Button size="sm" variant="ghost" asChild><Link href="/messaging">Open <ArrowUpRight aria-hidden /></Link></Button>} />
          <div className="p-2">
            {msgStats && msgStats.recent.length > 0 ? (
              msgStats.recent.map((c) => (
                <Link key={c.other_user_id} href={`/messaging?user=${c.other_user_id}`}
                  className="group flex min-h-11 items-center gap-3 rounded-lg px-2 py-2 transition-colors hover:bg-surface-2">
                  <div className="relative shrink-0">
                    <div className="flex size-8 items-center justify-center rounded-full bg-surface-2">
                      <User2 className="size-4 text-fg-muted" aria-hidden />
                    </div>
                    {c.is_online && (
                      <span className="absolute -bottom-0.5 -right-0.5 size-2.5 rounded-full border-2 border-surface bg-tone-blue" aria-label="Online" />
                    )}
                  </div>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium text-fg">{c.other_user_name}</p>
                    {c.last_message && <p className="truncate text-xs text-fg-muted">{mentionsToPlainText(c.last_message)}</p>}
                  </div>
                  <div className="flex shrink-0 flex-col items-end gap-1">
                    {c.last_message_at && <span className="text-[11px] text-fg-subtle tabular">{timeAgo(c.last_message_at)}</span>}
                    <CountBadge count={c.unread_count} />
                  </div>
                </Link>
              ))
            ) : msgStats ? (
              <p className="py-3 text-center text-[13px] text-fg-muted">No recent conversations</p>
            ) : (
              <div className="space-y-2 p-1">
                {[1, 2, 3].map((i) => <div key={i} className="h-11 animate-pulse rounded-lg bg-surface-2" />)}
              </div>
            )}
          </div>
        </Card>

        {/* Quick actions */}
        <Card>
          <CardHeader title="Quick actions" />
          <ul className="p-2">
            {QUICK_LINKS.map((link) => (
              <li key={link.href}>
                <Link href={link.href} className="group flex min-h-11 items-center justify-between gap-2 rounded-lg px-2 py-1.5 transition-colors hover:bg-surface-2">
                  <span>
                    <span className="block text-sm font-medium text-fg">{link.label}</span>
                    <span className="block text-xs text-fg-muted">{link.desc}</span>
                  </span>
                  <ArrowUpRight className="size-3.5 shrink-0 text-fg-subtle group-hover:text-brand-ink" aria-hidden />
                </Link>
              </li>
            ))}
          </ul>
        </Card>

        {/* Module shortcuts */}
        <Card>
          <CardHeader title="Modules" />
          <ul className="grid grid-cols-2 gap-1 p-2">
            {MODULE_SHORTCUTS.map((mod) => (
              <li key={mod.href}>
                <Link href={mod.href} className="block min-h-11 rounded-lg px-2 py-1.5 transition-colors hover:bg-surface-2">
                  <span className="block text-sm font-medium text-fg">{mod.label}</span>
                  <span className="block text-xs text-fg-muted">{mod.desc}</span>
                </Link>
              </li>
            ))}
          </ul>
        </Card>
      </div>
    </div>
  );
}
