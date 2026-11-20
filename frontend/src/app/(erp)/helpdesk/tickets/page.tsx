"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useState, useEffect, useCallback, useMemo } from "react";
import { MessageCircle, AlertTriangle, CheckCircle2, Clock, Send, Lock, Loader2, Search } from "lucide-react";
import api from "@/lib/api";
import { cn } from "@/lib/utils";
import {
  Button, DataTable, FilterChips, Input, ListFrame, PageHeader, Pill, SavedViewTabs, StatusPill, Textarea, toast,
  useSavedViews, type ColumnDef, type SavedView, type Tone,
} from "@/components/ui";

interface Ticket {
  id: string; number: string; subject: string; status: string; priority: string;
  team_name: string | null; category_name: string | null;
  customer_name: string; customer_email: string; customer_company: string | null;
  assignee_id: string | null; source: string;
  response_deadline: string | null; resolution_deadline: string | null;
  sla_response_breached: boolean; sla_resolution_breached: boolean;
  first_responded_at: string | null; resolved_at: string | null;
  created_at: string;
  messages?: Message[];
}
interface Message { id: string; body: string; is_internal: boolean; author_name: string | null; created_at: string; }
interface Summary { total: number; new: number; open: number; pending: number; resolved: number; sla_breached: number; }

const PRIORITY_TONES: Record<string, Tone> = { low: "neutral", normal: "neutral", high: "orange", critical: "red" };

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "new", label: "New", filters: { status: "new" } },
  { id: "open", label: "Open", filters: { status: "open" } },
  { id: "pending", label: "Pending", filters: { status: "pending" } },
  { id: "resolved", label: "Resolved", filters: { status: "resolved" } },
  { id: "closed", label: "Closed", filters: { status: "closed" } },
  { id: "all", label: "All", filters: {} },
];

function fmt(d: string) {
  return new Date(d).toLocaleString("en-GB", { day: "2-digit", month: "short", hour: "2-digit", minute: "2-digit" });
}

function fmtDate(d: string) {
  return new Date(d).toLocaleDateString("en-GB", { day: "2-digit", month: "short", year: "numeric" });
}

export default function HelpdeskTicketsPage() {
  const views = useSavedViews<Filters>("helpdesk.tickets", VIEWS);
  const [tickets, setTickets] = useState<Ticket[]>([]);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [active, setActive] = useState<Ticket | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [replyBody, setReplyBody] = useState("");
  const [isInternal, setIsInternal] = useState(false);
  const [sending, setSending] = useState(false);

  const statusFilter = views.active?.filters.status ?? "";
  const filters: Filters = statusFilter ? { status: statusFilter } : {};

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (statusFilter) params.status = statusFilter;
      if (search) params.q = search;
      const [tRes, sRes] = await Promise.all([
        fetchAllPages<Ticket>((p) => api.get("/helpdesk/tickets", { params: p }), params),
        api.get("/helpdesk/summary"),
      ]);
      setTickets(tRes.rows);
      setSummary(sRes.data);
    } catch { setTickets([]); } finally { setLoading(false); }
  }, [statusFilter, search]);

  useEffect(() => { load(); }, [load]);

  async function openDetail(t: Ticket) {
    setActive(t);
    setDetailLoading(true);
    try {
      const res = await api.get(`/helpdesk/tickets/${t.id}`);
      setActive(res.data);
    } catch { } finally { setDetailLoading(false); }
  }

  async function sendReply() {
    if (!active || !replyBody.trim()) return;
    setSending(true);
    try {
      await api.post(`/helpdesk/tickets/${active.id}/messages`, { body: replyBody.trim(), is_internal: isInternal });
      toast.success(isInternal ? "Internal note added" : "Reply sent");
      setReplyBody("");
      openDetail(active);
      load();
    } catch { toast.error("Failed to send"); } finally { setSending(false); }
  }

  async function updateStatus(id: string, status: string) {
    try {
      await api.patch(`/helpdesk/tickets/${id}`, { status });
      toast.success(`Ticket ${status}`);
      load();
      if (active?.id === id) openDetail({ ...active, id });
    } catch { toast.error("Failed to update"); }
  }

  function onExpand(id: string | null) {
    if (!id) { setActive(null); return; }
    const t = tickets.find((x) => x.id === id);
    if (t) openDetail(t);
  }

  const columns = useMemo<ColumnDef<Ticket>[]>(() => [
    { accessorKey: "number", header: "Ticket", meta: { width: "110px" }, cell: (c) => <span className="font-mono text-[13px] text-fg-muted">{c.getValue<string>()}</span> },
    {
      accessorKey: "subject", header: "Subject",
      cell: (c) => {
        const t = c.row.original;
        return (
          <div className="min-w-0">
            <p className="flex items-center gap-1.5 truncate font-medium text-fg">
              {t.sla_resolution_breached && <AlertTriangle className="size-3.5 shrink-0 text-tone-red" aria-label="SLA breached" />}
              <span className="truncate">{t.subject}</span>
            </p>
            <p className="truncate text-xs text-fg-muted">{t.customer_name}{t.customer_company ? `, ${t.customer_company}` : ""}</p>
          </div>
        );
      },
    },
    { accessorKey: "priority", header: "Priority", cell: (c) => <Pill tone={PRIORITY_TONES[c.getValue<string>()] ?? "neutral"} className="capitalize">{c.getValue<string>()}</Pill> },
    { accessorKey: "team_name", header: "Team", meta: { hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{c.getValue<string>() ?? "-"}</span> },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="ticket" status={c.getValue<string>()} /> },
    { accessorKey: "created_at", header: "Created", meta: { numeric: true, hideOnMobile: true }, cell: (c) => <span className="text-fg-muted">{fmtDate(c.getValue<string>())}</span> },
  ], []);

  function renderDetails(row: Ticket) {
    const t = active?.id === row.id ? active : row;
    return (
      <div className="space-y-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0 text-xs text-fg-muted">
            <p>
              {t.customer_name} &lt;{t.customer_email}&gt;
              {t.customer_company ? `, ${t.customer_company}` : ""}
              {t.team_name ? `, ${t.team_name}` : ""}
              {t.category_name ? `, ${t.category_name}` : ""}
            </p>
            {(t.response_deadline || t.resolution_deadline) && (
              <div className="mt-1.5 flex flex-wrap gap-3">
                {t.response_deadline && (
                  <span className={cn("inline-flex items-center gap-1 tabular", t.sla_response_breached && "font-medium text-tone-red")}>
                    <Clock className="size-3" aria-hidden /> Response: {fmt(t.response_deadline)}
                    {t.first_responded_at && <CheckCircle2 className="size-3 text-tone-blue" aria-label="met" />}
                  </span>
                )}
                {t.resolution_deadline && (
                  <span className={cn("inline-flex items-center gap-1 tabular", t.sla_resolution_breached && "font-semibold text-tone-red")}>
                    <Clock className="size-3" aria-hidden /> Resolution: {fmt(t.resolution_deadline)}
                    {t.resolved_at && <CheckCircle2 className="size-3 text-tone-blue" aria-label="met" />}
                  </span>
                )}
                {t.sla_resolution_breached && <Pill tone="red" icon={AlertTriangle}>SLA breached</Pill>}
              </div>
            )}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {t.status === "new" && <Button size="sm" onClick={() => updateStatus(t.id, "open")}>Open</Button>}
            {["new", "open", "pending"].includes(t.status) && (
              <Button size="sm" onClick={() => updateStatus(t.id, "resolved")}><CheckCircle2 aria-hidden /> Resolve</Button>
            )}
            {t.status === "resolved" && <Button size="sm" onClick={() => updateStatus(t.id, "closed")}>Close</Button>}
          </div>
        </div>

        <div className="max-h-[420px] space-y-4 overflow-y-auto rounded-lg border border-line bg-surface p-4">
          {detailLoading && active?.id === row.id ? (
            <div className="flex justify-center py-6"><Loader2 className="size-5 animate-spin text-fg-muted" aria-label="Loading conversation" /></div>
          ) : (t.messages ?? []).length === 0 ? (
            <p className="text-center text-xs text-fg-muted">No messages yet.</p>
          ) : (t.messages ?? []).map(msg => {
            const isCustomer = !msg.author_name;
            return (
              <div key={msg.id} className={cn("flex gap-3", !isCustomer && "flex-row-reverse")}>
                <div className={cn("flex size-8 shrink-0 items-center justify-center rounded-full text-xs font-semibold",
                  isCustomer ? "bg-surface-2 text-fg-muted" : msg.is_internal ? "bg-tone-orange-soft text-tone-orange" : "bg-brand text-brand-on")}>
                  {isCustomer ? t.customer_name.charAt(0).toUpperCase() : (msg.author_name?.charAt(0).toUpperCase() ?? "A")}
                </div>
                <div className={cn("max-w-lg flex-1", !isCustomer && "flex flex-col items-end")}>
                  <div className={cn("mb-1 flex items-center gap-2 text-xs", !isCustomer && "flex-row-reverse")}>
                    <span className="font-medium text-fg">{isCustomer ? t.customer_name : msg.author_name}</span>
                    <span className="text-fg-muted tabular">{fmt(msg.created_at)}</span>
                    {msg.is_internal && <Pill tone="orange" icon={Lock}>Internal</Pill>}
                  </div>
                  <div className={cn("whitespace-pre-wrap rounded-lg px-4 py-3 text-sm text-fg",
                    isCustomer ? "bg-surface-2" : msg.is_internal ? "border border-tone-orange/30 bg-tone-orange-soft" : "bg-brand/[0.06]")}>
                    {msg.body}
                  </div>
                </div>
              </div>
            );
          })}
        </div>

        {t.status !== "closed" && active?.id === row.id && (
          <div>
            <div className="mb-2 flex items-center gap-2" role="radiogroup" aria-label="Message type">
              <Button size="sm" role="radio" aria-checked={!isInternal} variant={!isInternal ? "primary" : "secondary"} onClick={() => setIsInternal(false)}>
                Reply to customer
              </Button>
              <Button size="sm" role="radio" aria-checked={isInternal} variant="secondary"
                className={cn(isInternal && "border-tone-orange/40 bg-tone-orange-soft text-tone-orange")} onClick={() => setIsInternal(true)}>
                <Lock aria-hidden /> Internal note
              </Button>
            </div>
            <div className="flex gap-3">
              <Textarea aria-label={isInternal ? "Internal note" : "Reply"} value={replyBody} onChange={e => setReplyBody(e.target.value)}
                onKeyDown={e => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) sendReply(); }} rows={3}
                placeholder={isInternal ? "Write an internal note (not visible to customer)" : "Write a reply to the customer"}
                className={cn("flex-1", isInternal && "border-tone-orange/40 bg-tone-orange-soft/40")} />
              <Button variant="primary" className="self-end" onClick={sendReply} loading={sending} disabled={!replyBody.trim()}>
                {!sending && <Send aria-hidden />} Send
              </Button>
            </div>
            <p className="mt-1.5 text-xs text-fg-subtle">Ctrl+Enter to send</p>
          </div>
        )}
      </div>
    );
  }

  return (
    <ListFrame>
      <PageHeader flush title="Helpdesk"
        summary={summary ? <>{summary.new} new, <span className={cn(summary.sla_breached > 0 && "font-medium text-brand-ink")}>{summary.sla_breached} SLA breached</span></> : "Support tickets"} />
      <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setActive(null)}
        counts={summary ? { new: summary.new, open: summary.open, pending: summary.pending } : undefined} alertIds={["new"]} />
      <FilterChips chips={[]} onToggle={() => {}}>
        <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
          value={search} onChange={e => setSearch(e.target.value)} placeholder="Search tickets" aria-label="Search tickets" className="w-64" />
      </FilterChips>
      <DataTable label="Tickets" columns={columns} data={tickets} getRowId={(r) => r.id} loading={loading}
        expandedId={active?.id ?? null} onExpandedChange={onExpand}
        empty={{ icon: MessageCircle, title: "No tickets", description: "Nothing in this view." }}
        renderDetails={renderDetails} />
    </ListFrame>
  );
}
