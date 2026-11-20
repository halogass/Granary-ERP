"use client";

import { isMyLead } from "@/lib/leadOwner";
import { fetchAllPages } from "@/lib/fetchAll";
import { formatCurrency, sumByCurrency } from "@/lib/currency";
import { useCurrencySettings } from "@/hooks/useCurrencySettings";
import { useEffect, useMemo, useState, useCallback } from "react";
import { useRouter } from "next/navigation";
import { useSession } from "next-auth/react";
import { crmApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import {
  Plus, Search, X, StickyNote, Clock, User, Trash2, Building2, Sparkles, MessageSquare, Mail, AlertCircle,
  ArrowUpRight, KanbanSquare, List, Phone, Users, Monitor, FileText, PenLine, Pin, Pencil, TrendingUp,
} from "lucide-react";
import { useCrmStages } from "@/hooks/useCrmStages";
import { canMoveLead, stageStyle } from "@/lib/crmStages";
import ReasonDialog from "@/components/crm/ReasonDialog";
import Link from "next/link";
import AiSelectionBar from "@/components/crm/AiSelectionBar";
import OrgContactPicker from "@/components/crm/OrgContactPicker";
import AttachmentPanel from "@/components/ui/AttachmentPanel";
import { DndContext, type DragEndEvent } from "@dnd-kit/core";
import { useBoardSensors, DroppableColumn, DraggableCard } from "@/components/dnd/board";
import { MoveToMenu } from "@/components/dnd/MoveToMenu";
import { stageDotClass } from "@/components/crm/stageTone";
import {
  Button, Checkbox, DataTable, Dialog, EmptyState, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues,
  ListFrame, PageHeader, SavedViewTabs, Select, Sheet, Skeleton, Stat, StatGroup, StatusPill, Textarea, toast,
  useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { segmentFilterOptions, segmentFilterParams, segmentLabel, segmentOf, segmentPatch, useSegments, type WithSegment } from "@/lib/segments";
import SegmentInput from "@/components/crm/SegmentInput";

const TEMP_LABEL: Record<string, string> = { hot: "Hot", warm: "Warm", cold: "Cold" };

function ScoreChip({ ai }: { ai: AiScore }) {
  return (
    <StatusPill family="temperature" status={ai.temperature}
      label={<span className="tabular">{TEMP_LABEL[ai.temperature] ?? "Cold"}, {ai.score}</span>} />
  );
}

interface AiScore {
  score: number;
  temperature: "hot" | "warm" | "cold";
  rationale?: string;
  recommended_next_action?: string;
  signals?: string[];
  scored_at?: string;
}

interface Lead extends WithSegment {
  id: string;
  title: string;
  status: string;
  source?: string;
  priority?: string;
  estimated_value?: number;
  currency?: string;
  created_at?: string;
  created_by_id?: string;
  assigned_to_id?: string | null;
  contact_id?: string;
  organization_id?: string;
  contact_name?: string;
  contact_email?: string;
  contact_phone?: string;
  contact_whatsapp?: string;
  organization_name?: string;
  ai_score?: AiScore | null;
  needs_next_action?: boolean;
  converted_to_deal_id?: string | null;
  disqualification_reason?: string | null;
}

interface LeadDetail extends Lead {
  description?: string;
  source_detail?: string;
  created_by_name?: string;
}

interface Activity {
  id: string;
  activity_type: string;
  subject?: string;
  content?: string;
  scheduled_at?: string;
  created_at?: string;
  created_by_name?: string;
}

const fmtCompact = (n?: number) => {
  if (!n) return null;
  if (n >= 1_000_000_000) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1_000_000) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1e3).toFixed(0)}K`;
  return `${n}`;
};
const fmtFull = (n: number | undefined, currency: string) =>
  n == null ? "-" : formatCurrency(n, currency);

const fmtDate = (iso?: string) => {
  if (!iso) return "";
  return new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });
};

const ACTIVITY_ICONS: Record<string, React.ElementType> = {
  note: StickyNote, call: Phone, email: Mail, whatsapp: MessageSquare,
  meeting: Users, demo: Monitor, site_visit: Building2,
  proposal_sent: FileText, contract_sent: PenLine,
};

type Scope = "open" | "closed" | "all";
type Filters = { scope?: Scope; mine?: boolean; hot?: boolean; needsAction?: boolean; status?: string; segment?: string };

// Built-in views of the list; the status chips refine them (and filter on the server, as before).
const VIEWS: SavedView<Filters>[] = [
  { id: "open", label: "All open", filters: { scope: "open" } },
  { id: "mine", label: "Mine", filters: { scope: "open", mine: true } },
  { id: "hot", label: "Hot", filters: { scope: "open", hot: true } },
  { id: "next", label: "No next action", filters: { scope: "open", needsAction: true } },
  { id: "closed", label: "Closed", filters: { scope: "closed" } },
  { id: "all", label: "All leads", filters: { scope: "all" } },
];

const alertClass = "flex items-center gap-2 rounded-lg border border-tone-red/30 bg-tone-red-soft px-4 py-2.5 text-sm text-tone-red";

export default function LeadsPage() {
  const [leads, setLeads] = useState<Lead[]>([]);
  const [loading, setLoading] = useState(true);
  const router = useRouter();
  const [search, setSearch] = useState("");
  const [view, setView] = useState<"kanban" | "list">("kanban");
  const [aiSelected, setAiSelected] = useState<string[]>([]);
  const [showForm, setShowForm] = useState(false);
  const [form, setForm] = useState({ title: "", status: "new", source: "founder_network", estimated_value: "", currency: "", organization_id: "", contact_id: "", segment: "" });
  const [saving, setSaving] = useState(false);

  const { data: session } = useSession();
  const myId = (session?.user as { id?: string } | undefined)?.id ?? "";
  const views = useSavedViews<Filters>("crm.leads", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [segmentFilter, setSegmentFilter] = useState("");
  const segments = useSegments();
  // The list's views apply to the list; the board keeps every column and only takes the status chip.
  const listFilters: Filters = {
    ...views.active?.filters,
    ...(chipStatus ? { status: chipStatus } : {}),
    ...(segmentFilter ? { segment: segmentFilter } : {}),
  };
  const statusFilter = view === "list" ? (listFilters.status ?? "") : chipStatus;
  // the segment filters on the server, on the board and the list alike
  const serverSegment = view === "list" ? (listFilters.segment ?? "") : segmentFilter;

  // The funnel vocabulary comes from the backend (GET /crm/stages)
  const { reg } = useCrmStages();
  const STAGES = reg.lead_statuses.map((st) => ({ key: st.key, label: st.label }));
  const BOARD = STAGES.filter((st) => st.key !== "promoted");   // promoted leads live on as deals
  const SOURCES = reg.lead_sources.map((src) => src.key);
  const SOURCE_LABEL: Record<string, string> = Object.fromEntries(reg.lead_sources.map((src) => [src.key, src.label]));
  const label = (key: string) => stageStyle(reg, key).label;
  const [notice, setNotice] = useState<string | null>(null);
  const [disqualify, setDisqualify] = useState<{ leadId: string; from: string } | null>(null);
  const [reasonError, setReasonError] = useState<string | null>(null);
  const [promoting, setPromoting] = useState(false);

  // Drag
  const sensors = useBoardSensors();

  // Open the WhatsApp / Email conversation for a lead's contact
  const openWhatsApp = (lead: Lead, e?: React.MouseEvent) => {
    e?.stopPropagation();
    const phone = lead.contact_whatsapp || lead.contact_phone || "";
    const p = new URLSearchParams();
    if (lead.contact_id) p.set("contact", lead.contact_id);
    if (phone) p.set("phone", phone);
    router.push(`/crm/whatsapp?${p.toString()}`);
  };
  const openEmail = (lead: Lead, e?: React.MouseEvent) => {
    e?.stopPropagation();
    const p = new URLSearchParams();
    p.set("lead", lead.id);
    if (lead.contact_id) p.set("contact", lead.contact_id);
    if (lead.contact_email) p.set("email", lead.contact_email);
    router.push(`/crm/email?${p.toString()}`);
  };
  const ContactActions = (lead: Lead) => {
    const hasWa = !!(lead.contact_whatsapp || lead.contact_phone);
    const hasEmail = !!lead.contact_email;
    if (!hasWa && !hasEmail) return null;
    return (
      <div className="flex items-center gap-0.5">
        {hasWa && <IconButton size="sm" icon={MessageSquare} label="Open WhatsApp chat" onClick={(e) => openWhatsApp(lead, e)} />}
        {hasEmail && <IconButton size="sm" icon={Mail} label="Open email thread" onClick={(e) => openEmail(lead, e)} />}
      </div>
    );
  };

  // Drawer
  const [selected, setSelected] = useState<LeadDetail | null>(null);
  const [activities, setActivities] = useState<Activity[]>([]);
  const [drawerLoading, setDrawerLoading] = useState(false);
  const [noteText, setNoteText] = useState("");
  const [savingNote, setSavingNote] = useState(false);
  const { functional, active: activeCurrencies } = useCurrencySettings();
  const [editForm, setEditForm] = useState<Partial<LeadDetail & { estimated_value_str: string; segment: string }>>({});
  const [savingEdit, setSavingEdit] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [scoring, setScoring] = useState(false);

  async function runScore() {
    if (!selected) return;
    setScoring(true);
    try {
      const res = await crmApi.scoreLead(selected.id);
      const ai = res.data as AiScore;
      setSelected((s) => (s ? { ...s, ai_score: ai } : s));
      setLeads((ls) => ls.map((l) => (l.id === selected.id ? { ...l, ai_score: ai } : l)));
    } catch (e) {
      const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(detail || "Scoring failed. Check Settings, AI.");
    } finally {
      setScoring(false);
    }
  }

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = { ...segmentFilterParams(serverSegment) };
      if (statusFilter) params.status = statusFilter;
      const { rows } = await fetchAllPages<Lead>(crmApi.listLeads, params);
      setLeads(rows);
    } catch {
      setLeads([]);
    } finally {
      setLoading(false);
    }
  }, [statusFilter, serverSegment]);

  useEffect(() => { load(); }, [load]);

  async function openDrawer(lead: Lead) {
    setSelected(lead as LeadDetail);
    setEditForm({
      title: lead.title, status: lead.status,
      source: lead.source, priority: lead.priority,
      estimated_value_str: lead.estimated_value ? String(lead.estimated_value) : "",
      currency: lead.currency || "",
      organization_id: lead.organization_id ?? "",
      contact_id: lead.contact_id ?? "",
    });
    setActivities([]);
    setNoteText("");
    setDrawerLoading(true);
    try {
      const [detailRes, actRes] = await Promise.all([
        crmApi.getLead(lead.id),
        crmApi.listActivities({ entity_type: "lead", entity_id: lead.id }),
      ]);
      const detail = detailRes.data;
      setSelected(detail);
      setEditForm({
        title: detail.title, status: detail.status,
        source: detail.source, priority: detail.priority,
        description: detail.description,
        segment: segmentOf(detail),
        estimated_value_str: detail.estimated_value ? String(detail.estimated_value) : "",
        currency: detail.currency || "",
        organization_id: detail.organization_id ?? "",
        contact_id: detail.contact_id ?? "",
      });
      setActivities(actRes.data.items ?? actRes.data);
    } finally {
      setDrawerLoading(false);
    }
  }

  async function saveEdit() {
    if (!selected) return;
    setSavingEdit(true);
    try {
      const payload: Record<string, unknown> = {
        title: editForm.title,
        status: editForm.status,
        source: editForm.source,
        priority: editForm.priority,
        description: editForm.description,
        estimated_value: editForm.estimated_value_str ? Number(editForm.estimated_value_str) : null,
        currency: editForm.currency || null,      // none: Nary quotes in the customer's country currency
        organization_id: editForm.organization_id || null,
        contact_id: editForm.contact_id || null,
      };
      if ((editForm.segment ?? "") !== segmentOf(selected)) Object.assign(payload, segmentPatch(editForm.segment ?? ""));
      if (editForm.status === "disqualified" && selected.status !== "disqualified") {
        payload.disqualification_reason = editForm.disqualification_reason;
      }
      try {
        await crmApi.updateLead(selected.id, payload);
      } catch (err) {
        setNotice(extractApiError(err));
        return;
      }
      setNotice(null);
      const updated = {
        ...selected,
        ...payload,
        estimated_value: payload.estimated_value as number | undefined ?? undefined,
      };
      setSelected(updated as LeadDetail);
      setLeads((prev) => prev.map((l) => l.id === selected.id ? { ...l, ...payload, estimated_value: payload.estimated_value as number | undefined } : l));
    } finally {
      setSavingEdit(false);
    }
  }

  async function addNote() {
    if (!selected || !noteText.trim()) return;
    setSavingNote(true);
    try {
      await crmApi.createActivity({
        entity_type: "lead",
        entity_id: selected.id,
        lead_id: selected.id,
        activity_type: "note",
        direction: "internal",
        content: noteText.trim(),
      });
      setNoteText("");
      const res = await crmApi.listActivities({ entity_type: "lead", entity_id: selected.id });
      setActivities(res.data.items ?? res.data);
    } finally {
      setSavingNote(false);
    }
  }

  async function deleteLead(id: string) {
    setDeleting(true);
    try {
      await crmApi.deleteLead(id);
      setLeads((prev) => prev.filter((l) => l.id !== id));
      setSelected(null);
      setConfirmDeleteId(null);
    } finally {
      setDeleting(false);
    }
  }

  function onDragEnd(e: DragEndEvent) {
    if (e.over) moveLead(String(e.active.id), String(e.over.id));
  }

  /** Move a lead to another status: by dragging, or with the card's "Move to" menu. */
  function moveLead(id: string, targetStatus: string) {
    const lead = leads.find((l) => l.id === id);
    if (!lead || lead.status === targetStatus) return;
    if (!canMoveLead(reg, lead.status, targetStatus)) {
      setNotice(`A ${lead.status} lead can't move to ${targetStatus}.`);
      return;
    }
    if (targetStatus === "disqualified") {
      setReasonError(null);
      setDisqualify({ leadId: id, from: lead.status });   // continues in the reason dialog
      return;
    }
    setNotice(null);
    setLeads((prev) => prev.map((l) => l.id === id ? { ...l, status: targetStatus } : l));
    crmApi.updateLead(id, { status: targetStatus }).catch((err) => {
      setLeads((prev) => prev.map((l) => l.id === id ? { ...l, status: lead.status } : l));
      setNotice(extractApiError(err));
    });
  }

  async function confirmDisqualify(reason: string) {
    if (!disqualify) return;
    setSaving(true);
    try {
      await crmApi.updateLead(disqualify.leadId, { status: "disqualified", disqualification_reason: reason });
      setLeads((prev) => prev.map((l) => l.id === disqualify.leadId ? { ...l, status: "disqualified" } : l));
      setDisqualify(null);
    } catch (err) {
      setReasonError(extractApiError(err));
    } finally { setSaving(false); }
  }

  async function promote(lead: Lead) {
    setPromoting(true);
    try {
      const res = await crmApi.promoteLead(lead.id);
      router.push(`/crm/deals/${res.data.deal_id}`);
    } catch (err) {
      setNotice(extractApiError(err));
    } finally { setPromoting(false); }
  }

  async function handleCreate(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    setNotice(null);
    try {
      const { segment, ...rest } = form;
      await crmApi.createLead({
        ...rest,
        ...(segment.trim() ? segmentPatch(segment) : {}),
        estimated_value: form.estimated_value ? Number(form.estimated_value) : undefined,
        currency: form.currency || undefined,     // none: Nary quotes in the customer's country currency
        organization_id: form.organization_id || undefined,
        contact_id: form.contact_id || undefined,
      });
      setShowForm(false);
      setForm({ title: "", status: "new", source: "founder_network", estimated_value: "", currency: "", organization_id: "", contact_id: "", segment: "" });
      load();
    } catch (err) {
      setNotice(extractApiError(err));
    } finally {
      setSaving(false);
    }
  }

  const filteredLeads = leads.filter((l) =>
    !search || l.title?.toLowerCase().includes(search.toLowerCase())
  );
  // The list view also applies the saved view's scope and flags
  const listedLeads = useMemo(() => filteredLeads.filter((l) => {
    const st = reg.lead_statuses.find((s) => s.key === l.status);
    if (listFilters.scope === "open" && st?.is_closed) return false;
    if (listFilters.scope === "closed" && !st?.is_closed) return false;
    if (listFilters.mine && !isMyLead(l, myId)) return false;
    if (listFilters.hot && l.ai_score?.temperature !== "hot") return false;
    if (listFilters.needsAction && !l.needs_next_action) return false;
    return true;
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }), [filteredLeads, reg, listFilters.scope, listFilters.mine, listFilters.hot, listFilters.needsAction, myId]);

  // Leads carry their own currency, so these are grouped rather than blindly summed.
  const openLeads = leads.filter((l) => reg.lead_statuses.find((st) => st.key === l.status)?.is_active);
  const promoted = leads.filter((l) => l.status === "promoted");
  const decided = leads.filter((l) => reg.lead_statuses.find((st) => st.key === l.status)?.is_closed);
  const pipelineValue = sumByCurrency(openLeads, (l) => l.estimated_value, (l) => l.currency, functional);
  const promotedValue = sumByCurrency(promoted, (l) => l.estimated_value, (l) => l.currency, functional);

  const statusChips = STAGES.map((s) => ({
    id: s.key,
    label: <><span className={cn("size-1.5 rounded-full", stageDotClass("lead", s.key))} aria-hidden />{s.label}</>,
    active: chipStatus === s.key,
    count: leads.filter((l) => l.status === s.key).length,
  }));

  const searchBox = (
    <>
      {segments.length > 0 && (
        <Select aria-label="Segment filter" className="w-48" value={segmentFilter} placeholder="All segments"
          onChange={(e) => setSegmentFilter(e.target.value)} options={segmentFilterOptions(segments)} />
      )}
      <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
        value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search leads" aria-label="Search leads" className="w-56" />
    </>
  );

  function deleteConfirm(lead: Lead) {
    return confirmDeleteId === lead.id ? (
      <span className="inline-flex items-center gap-1.5">
        <span className="text-xs font-medium text-tone-red">Delete?</span>
        <Button size="sm" variant="danger" loading={deleting} onClick={() => deleteLead(lead.id)}>Yes, delete</Button>
        <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
      </span>
    ) : (
      <IconButton size="sm" icon={Trash2} label={`Delete ${lead.title}`} onClick={() => setConfirmDeleteId(lead.id)} />
    );
  }

  function promoteAction(lead: Lead) {
    if (lead.converted_to_deal_id) {
      return (
        <Button size="sm" asChild>
          <Link href={`/crm/deals/${lead.converted_to_deal_id}`}>Open deal <ArrowUpRight aria-hidden /></Link>
        </Button>
      );
    }
    if (["promoted", "disqualified"].includes(lead.status)) return null;
    return (
      <Button size="sm" variant="primary" loading={promoting} onClick={() => promote(lead)} title="Create a deal from this lead">
        {!promoting && <ArrowUpRight aria-hidden />} Promote
      </Button>
    );
  }

  const columns: ColumnDef<Lead>[] = [
    {
      id: "select",
      enableSorting: false,
      meta: { width: "40px" },
      header: () => (
        <Checkbox aria-label="Select every lead shown"
          checked={listedLeads.length > 0 && listedLeads.every((l) => aiSelected.includes(l.id))}
          onChange={(e) => setAiSelected(e.target.checked ? listedLeads.map((l) => l.id) : [])} />
      ),
      cell: ({ row }) => (
        <Checkbox aria-label={`Select ${row.original.title}`} checked={aiSelected.includes(row.original.id)}
          onChange={(e) => setAiSelected((prev) =>
            e.target.checked ? [...prev, row.original.id] : prev.filter((id) => id !== row.original.id))} />
      ),
    },
    {
      accessorKey: "title",
      header: "Lead",
      cell: ({ row }) => (
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
          <Link href={`/crm/leads/${row.original.id}`} className="font-medium text-fg hover:text-brand-ink">{row.original.title}</Link>
          {row.original.needs_next_action && (
            <span className="inline-flex items-center gap-1 text-xs font-medium text-tone-orange">
              <AlertCircle className="size-3" aria-hidden /> No next action
            </span>
          )}
        </div>
      ),
    },
    {
      id: "organization",
      header: "Organization",
      accessorFn: (l) => l.organization_name ?? l.contact_name ?? "",
      meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string>() || "-"}</span>,
    },
    {
      accessorKey: "status",
      header: "Status",
      sortingFn: (a, b) => (reg.lead_statuses.find((s) => s.key === a.original.status)?.order ?? 99)
        - (reg.lead_statuses.find((s) => s.key === b.original.status)?.order ?? 99),
      cell: ({ row }) => <StatusPill family="lead" status={row.original.status} label={label(row.original.status)} />,
    },
    {
      id: "score",
      header: "AI score",
      accessorFn: (l) => l.ai_score?.score ?? -1,
      meta: { hideOnMobile: true },
      cell: ({ row }) => row.original.ai_score ? <ScoreChip ai={row.original.ai_score} /> : <span className="text-fg-subtle">-</span>,
    },
    {
      id: "source",
      header: "Source",
      accessorFn: (l) => SOURCE_LABEL[l.source ?? ""] ?? l.source ?? "",
      meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string>() || "-"}</span>,
    },
    {
      id: "segment",
      header: "Segment",
      accessorFn: (l) => segmentLabel(segments, segmentOf(l)),
      meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string>()}</span>,
    },
    {
      accessorKey: "estimated_value",
      header: "Est. value",
      meta: { numeric: true },
      cell: ({ row }) => fmtFull(row.original.estimated_value, row.original.currency || functional),
    },
    {
      accessorKey: "created_at",
      header: "Created",
      meta: { numeric: true, hideOnMobile: true },
      cell: ({ row }) => fmtDate(row.original.created_at) || "-",
    },
  ];

  return (
    <div className="space-y-4">
      <PageHeader className="mb-0" title="Leads"
        summary={`${leads.length} ${leads.length === 1 ? "lead" : "leads"}, ${openLeads.length} open`}
        actions={<>
          <div role="group" aria-label="Layout" className="inline-flex rounded-lg border border-line-strong bg-surface p-0.5">
            {(["kanban", "list"] as const).map((v) => {
              const Icon = v === "kanban" ? KanbanSquare : List;
              return (
                <button key={v} type="button" aria-pressed={view === v} onClick={() => setView(v)}
                  className={cn("inline-flex h-10 items-center gap-1.5 rounded-md px-2.5 text-[13px] font-medium capitalize transition-colors md:h-7",
                    "focus-visible:outline focus-visible:outline-2 focus-visible:outline-[rgb(var(--focus)/0.7)]",
                    view === v ? "bg-surface-2 text-fg" : "text-fg-muted hover:text-fg")}>
                  <Icon className="size-3.5" aria-hidden />{v}
                </button>
              );
            })}
          </div>
          <Button variant="primary" onClick={() => setShowForm(true)}><Plus aria-hidden /> Add lead</Button>
        </>} />

      <StatGroup>
        <Stat label="Total leads" value={leads.length} />
        <Stat label="Pipeline" value={`${pipelineValue.currency} ${fmtCompact(pipelineValue.total) ?? "0"}${pipelineValue.mixed ? " *" : ""}`}
          note={pipelineValue.mixed ? `Mixed currencies: showing the ${pipelineValue.currency} subtotal` : undefined} />
        <Stat label="Promoted" value={`${promotedValue.currency} ${fmtCompact(promotedValue.total) ?? "0"}${promotedValue.mixed ? " *" : ""}`}
          note={promotedValue.mixed ? `Mixed currencies: showing the ${promotedValue.currency} subtotal` : undefined} />
        <Stat label="Conversion" value={decided.length > 0 ? `${Math.round(promoted.length / decided.length * 100)}%` : "-"}
          note="Promoted out of promoted + disqualified" />
      </StatGroup>

      {notice && (
        <div role="alert" className={alertClass}>
          <AlertCircle className="size-4 shrink-0" aria-hidden />{notice}
          <IconButton size="sm" icon={X} label="Dismiss" className="ml-auto text-tone-red" onClick={() => setNotice(null)} />
        </div>
      )}

      {view === "kanban" ? (
        <>
          <div className="overflow-hidden rounded-lg border border-line">
            <FilterChips className="border-b-0" chips={statusChips}
              onToggle={(id) => setChipStatus((s) => (s === id ? "" : id))} onClear={() => setChipStatus("")}>
              {searchBox}
            </FilterChips>
          </div>
          {loading ? (
            <div className="flex gap-3 overflow-x-auto pb-4">
              {[0, 1, 2, 3].map((i) => <Skeleton key={i} className="h-48 w-60 shrink-0" />)}
            </div>
          ) : (
            <DndContext sensors={sensors} onDragEnd={onDragEnd}>
              {/* Phones: one column at a time (85% wide, the next one peeking), snapping as you swipe */}
              <div className="-mx-1 flex snap-x snap-mandatory gap-3 overflow-x-auto scroll-px-1 px-1 pb-4 md:snap-none">
                {BOARD.map((stage) => {
                  const stageLeads = filteredLeads.filter((l) => l.status === stage.key);
                  const stageValue = sumByCurrency(stageLeads, (l) => l.estimated_value, (l) => l.currency, functional);
                  return (
                    <DroppableColumn key={stage.key} id={stage.key} className="w-[85%] flex-shrink-0 snap-start rounded-lg md:w-60">
                      {(isOver) => (
                        <div className={cn("rounded-lg border border-line bg-surface-2/60 p-2 transition-colors", isOver && "bg-brand/[0.05] ring-2 ring-brand/20")}>
                          <div className="mb-1 flex items-center gap-2 px-1">
                            <span className={cn("size-2 rounded-full", stageDotClass("lead", stage.key))} aria-hidden />
                            <span className="text-[13px] font-semibold text-fg">{stage.label}</span>
                            <span className="ml-auto rounded-full bg-surface px-2 text-[11px] font-medium text-fg-muted tabular">{stageLeads.length}</span>
                          </div>
                          <p className="mb-2 px-1 text-xs text-fg-muted tabular"
                            title={stageValue.mixed ? `Mixed currencies: showing the ${stageValue.currency} subtotal` : undefined}>
                            {stageValue.total > 0 ? `${stageValue.currency} ${fmtCompact(stageValue.total)}${stageValue.mixed ? " *" : ""}` : " "}
                          </p>

                          <div className="min-h-[80px] space-y-2">
                            {stageLeads.length === 0 && (
                              <div className={cn("rounded-lg border-2 border-dashed p-4 text-center text-xs text-fg-subtle transition-colors",
                                isOver ? "border-brand/30" : "border-line")}>
                                Drop here
                              </div>
                            )}
                            {stageLeads.map((lead) => (
                              <DraggableCard key={lead.id} id={lead.id} onClick={() => openDrawer(lead)}
                                className="group cursor-pointer select-none rounded-lg border border-line bg-surface p-3 shadow-card transition-shadow hover:shadow-pop">
                                <div className="mb-1.5 flex items-start justify-between gap-1.5">
                                  <p className="text-sm font-medium leading-snug text-fg group-hover:text-brand-ink">{lead.title}</p>
                                  {lead.ai_score && <span className="shrink-0"><ScoreChip ai={lead.ai_score} /></span>}
                                </div>
                                {(lead.organization_name || lead.contact_name) && (
                                  <div className="mb-1.5 flex flex-col gap-0.5">
                                    {lead.organization_name && (
                                      <span className="inline-flex items-center gap-1 truncate text-xs text-fg-muted">
                                        <Building2 className="size-3 shrink-0 text-fg-subtle" aria-hidden />{lead.organization_name}
                                      </span>
                                    )}
                                    {lead.contact_name && (
                                      <span className="inline-flex items-center gap-1 truncate text-xs text-fg-muted">
                                        <User className="size-3 shrink-0 text-fg-subtle" aria-hidden />{lead.contact_name}
                                      </span>
                                    )}
                                  </div>
                                )}
                                <div className="flex items-center justify-between gap-2">
                                  {lead.estimated_value ? (
                                    <span className="text-xs font-semibold text-fg tabular">{lead.currency || functional} {fmtCompact(lead.estimated_value)}</span>
                                  ) : <span />}
                                  {lead.source && (
                                    <span className="max-w-[110px] truncate rounded-full bg-surface-2 px-2 py-0.5 text-[11px] text-fg-muted">
                                      {SOURCE_LABEL[lead.source] ?? lead.source}
                                    </span>
                                  )}
                                </div>
                                <div className="mt-1.5 flex items-center justify-between">
                                  {lead.created_at ? (
                                    <p className="text-[11px] text-fg-subtle tabular">{fmtDate(lead.created_at)}</p>
                                  ) : <span />}
                                  <span className="flex items-center gap-0.5" onClick={(e) => e.stopPropagation()}
                                    onMouseDown={(e) => e.stopPropagation()} onTouchStart={(e) => e.stopPropagation()}>
                                    {ContactActions(lead)}
                                    <MoveToMenu itemLabel={lead.title} current={lead.status} onMove={(to) => moveLead(lead.id, to)}
                                      options={BOARD.filter((b) => b.key === lead.status || canMoveLead(reg, lead.status, b.key))
                                        .map((b) => ({ value: b.key, label: b.label }))} />
                                  </span>
                                </div>
                                {lead.needs_next_action && (
                                  <p className="mt-1.5 inline-flex items-center gap-1 text-[11px] font-medium text-tone-orange">
                                    <AlertCircle className="size-3" aria-hidden /> No next action
                                  </p>
                                )}
                              </DraggableCard>
                            ))}
                          </div>
                        </div>
                      )}
                    </DroppableColumn>
                  );
                })}
              </div>
            </DndContext>
          )}
        </>
      ) : (
        <>
          <ListFrame>
            <SavedViewTabs state={views} currentFilters={listFilters} onSelect={() => setChipStatus("")} />
            <FilterChips chips={statusChips}
              onToggle={(id) => setChipStatus((s) => (s === id ? "" : id))} onClear={() => setChipStatus("")}>
              {searchBox}
            </FilterChips>
            <AiSelectionBar objectType="lead" selected={aiSelected}
              onClear={() => setAiSelected([])}
              onMessage={(kind, text) => (kind === "error" ? toast.error(text) : kind === "success" ? toast.success(text) : toast(text))} />
            <DataTable label="Leads" columns={columns} data={listedLeads} getRowId={(l) => l.id} loading={loading}
              empty={{ icon: TrendingUp, title: "No leads in this view", description: "Pick another view, clear the filters, or add a lead.",
                action: <Button size="sm" variant="primary" onClick={() => setShowForm(true)}><Plus aria-hidden /> Add lead</Button> }}
              renderDetails={(lead) => (
                <div className="flex flex-wrap items-start gap-6">
                  <KeyValues className="min-w-[260px] flex-1" items={[
                    { label: "Status", value: <StatusPill family="lead" status={lead.status} label={label(lead.status)} /> },
                    { label: "Estimated value", value: fmtFull(lead.estimated_value, lead.currency || functional), numeric: true },
                    { label: "Organization", value: lead.organization_name || "-" },
                    { label: "Contact", value: lead.contact_name || "-" },
                    { label: "Source", value: SOURCE_LABEL[lead.source ?? ""] ?? lead.source ?? "-" },
                    { label: "Created", value: fmtDate(lead.created_at) || "-", numeric: true },
                    ...(lead.ai_score ? [{ label: "AI score", value: <ScoreChip ai={lead.ai_score} /> }] : []),
                    ...(lead.ai_score?.recommended_next_action ? [{ label: "Next", value: lead.ai_score.recommended_next_action }] : []),
                  ]} />
                  <div className="flex flex-wrap items-center gap-2">
                    <Button size="sm" asChild>
                      <Link href={`/crm/leads/${lead.id}`}>Open lead <ArrowUpRight aria-hidden /></Link>
                    </Button>
                    <Button size="sm" onClick={() => openDrawer(lead)}><Pencil aria-hidden /> Edit</Button>
                    {promoteAction(lead)}
                    {ContactActions(lead)}
                    {deleteConfirm(lead)}
                  </div>
                </div>
              )} />
          </ListFrame>
        </>
      )}

      {/* Lead details and quick edit */}
      <Sheet open={!!selected} onOpenChange={(o) => { if (!o) { setSelected(null); setConfirmDeleteId(null); } }}
        title={selected?.title ?? "Lead"}
        description={selected?.created_by_name
          ? `Registered by ${selected.created_by_name}${selected.created_at ? `, ${fmtDate(selected.created_at)}` : ""}`
          : undefined}>
        {selected && (
          <div className="space-y-6">
            <div className="flex flex-wrap items-center gap-2">
              <StatusPill family="lead" status={selected.status} label={label(selected.status)} />
              <span className="ml-auto" />
              <Button size="sm" asChild>
                <Link href={`/crm/leads/${selected.id}`}>Open lead <ArrowUpRight aria-hidden /></Link>
              </Button>
              {promoteAction(selected)}
              {deleteConfirm(selected)}
            </div>

            <section className="space-y-2">
              <div className="flex items-center gap-2">
                <h3 className="text-sm font-semibold text-fg">AI score</h3>
                {selected.ai_score && <ScoreChip ai={selected.ai_score} />}
                <Button size="sm" className="ml-auto" loading={scoring} onClick={runScore}>
                  {!scoring && <Sparkles aria-hidden />}
                  {selected.ai_score ? "Re-score" : "Score with AI"}
                </Button>
              </div>
              {selected.ai_score ? (
                <div className="space-y-2 text-sm">
                  <p className="text-fg">{selected.ai_score.rationale}</p>
                  {selected.ai_score.recommended_next_action && (
                    <p className="text-[13px] text-fg-muted"><span className="font-medium text-fg">Next:</span> {selected.ai_score.recommended_next_action}</p>
                  )}
                  {selected.ai_score.signals && selected.ai_score.signals.length > 0 && (
                    <ul className="list-disc space-y-0.5 pl-4 text-xs text-fg-muted">
                      {selected.ai_score.signals.map((sig, i) => <li key={i}>{sig}</li>)}
                    </ul>
                  )}
                </div>
              ) : (
                <p className="text-[13px] text-fg-muted">Analyze this lead&apos;s WhatsApp and email history and details to gauge how hot it is.</p>
              )}
            </section>

            {(selected.contact_whatsapp || selected.contact_phone || selected.contact_email) && (
              <section className="space-y-2">
                <h3 className="text-sm font-semibold text-fg">Conversations</h3>
                <div className="flex flex-wrap gap-2">
                  {(selected.contact_whatsapp || selected.contact_phone) && (
                    <Button size="sm" onClick={() => openWhatsApp(selected)}><MessageSquare aria-hidden /> Open WhatsApp</Button>
                  )}
                  {selected.contact_email && (
                    <Button size="sm" onClick={() => openEmail(selected)}><Mail aria-hidden /> Open email</Button>
                  )}
                </div>
              </section>
            )}

            <section className="space-y-3 border-t border-line pt-4">
              <h3 className="text-sm font-semibold text-fg">Details</h3>
              <Field label="Title">
                <Input value={editForm.title ?? ""} onChange={(e) => setEditForm({ ...editForm, title: e.target.value })} />
              </Field>
              <FieldGrid cols={2}>
                <Field label="Stage">
                  <Select value={editForm.status ?? "new"} onChange={(e) => setEditForm({ ...editForm, status: e.target.value })}>
                    {/* the current status and the moves allowed from it (stages.py LEAD_TRANSITIONS) */}
                    {STAGES.filter((s) => s.key === selected.status || canMoveLead(reg, selected.status, s.key))
                      .map((s) => <option key={s.key} value={s.key}>{s.label}</option>)}
                  </Select>
                </Field>
                {editForm.status === "disqualified" && selected.status !== "disqualified" && (
                  <Field label="Disqualification reason">
                    <Select value={editForm.disqualification_reason ?? ""} placeholder="Select..."
                      onChange={(e) => setEditForm({ ...editForm, disqualification_reason: e.target.value })}
                      options={reg.disqualify_reasons.map((r) => ({ value: r.key, label: r.label }))} />
                  </Field>
                )}
                <Field label="Priority">
                  <Select value={editForm.priority ?? "medium"} onChange={(e) => setEditForm({ ...editForm, priority: e.target.value })}
                    options={["low", "medium", "high", "urgent"].map((p) => ({ value: p, label: p.charAt(0).toUpperCase() + p.slice(1) }))} />
                </Field>
                <Field label="Source">
                  <Select value={editForm.source ?? ""} placeholder="Select..." onChange={(e) => setEditForm({ ...editForm, source: e.target.value })}
                    options={SOURCES.map((s) => ({ value: s, label: SOURCE_LABEL[s] }))} />
                </Field>
                <Field label="Segment">
                  <SegmentInput segments={segments} value={editForm.segment ?? ""}
                    onChange={(v) => setEditForm({ ...editForm, segment: v })} />
                </Field>
                <Field label="Est. value">
                  <div className="flex gap-2">
                    <Select aria-label="Currency" className="w-28" value={editForm.currency || ""}
                      onChange={(e) => setEditForm({ ...editForm, currency: e.target.value })}>
                      <option value="" title="The customer's country decides">Country</option>
                      {activeCurrencies.map((c) => <option key={c} value={c}>{c}</option>)}
                    </Select>
                    <div className="flex-1">
                      <Input numeric type="number" min="0" value={editForm.estimated_value_str ?? ""}
                        onChange={(e) => setEditForm({ ...editForm, estimated_value_str: e.target.value })} />
                    </div>
                  </div>
                </Field>
                <OrgContactPicker
                  organizationId={editForm.organization_id ?? ""}
                  contactId={editForm.contact_id ?? ""}
                  onChange={({ organization_id, contact_id }) => setEditForm({ ...editForm, organization_id, contact_id })}
                />
              </FieldGrid>
              <Field label="Notes / description">
                <Textarea rows={3} value={editForm.description ?? ""} placeholder="Add context, requirements, constraints..."
                  onChange={(e) => setEditForm({ ...editForm, description: e.target.value })} />
              </Field>
              <div className="flex justify-end">
                <Button variant="primary" loading={savingEdit} onClick={saveEdit}>Save lead</Button>
              </div>
            </section>

            <section className="space-y-2 border-t border-line pt-4">
              <Field label={<span className="inline-flex items-center gap-1.5"><StickyNote className="size-3.5 text-fg-muted" aria-hidden /> Add note</span>}>
                <Textarea rows={2} value={noteText} onChange={(e) => setNoteText(e.target.value)} placeholder="Write a note about this lead..." />
              </Field>
              <div className="flex justify-end">
                <Button size="sm" variant="primary" loading={savingNote} disabled={!noteText.trim()} onClick={addNote}>Add note</Button>
              </div>
            </section>

            <section className="space-y-2 border-t border-line pt-4">
              <h3 className="text-sm font-semibold text-fg">Attachments</h3>
              <AttachmentPanel entityType="crm_lead" entityId={selected.id} />
            </section>

            <section className="space-y-3 border-t border-line pt-4">
              <div className="flex items-center gap-2">
                <Clock className="size-3.5 text-fg-muted" aria-hidden />
                <h3 className="text-sm font-semibold text-fg">Activity log</h3>
              </div>
              {drawerLoading && activities.length === 0 && <Skeleton className="h-10 w-full" />}
              {activities.length === 0 && !drawerLoading && (
                <EmptyState compact title="No activity recorded yet" />
              )}
              <ul className="space-y-3">
                {activities.map((act) => {
                  const Icon = ACTIVITY_ICONS[act.activity_type] ?? Pin;
                  return (
                    <li key={act.id} className="flex gap-3">
                      <span className="flex size-7 shrink-0 items-center justify-center rounded-lg border border-line bg-surface-2 text-fg-muted">
                        <Icon className="size-3.5" aria-hidden />
                      </span>
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="text-[13px] font-medium capitalize text-fg">{act.activity_type.replace(/_/g, " ")}</span>
                          {act.created_by_name && <span className="text-xs text-fg-muted">by {act.created_by_name}</span>}
                          {(act.scheduled_at ?? act.created_at) && (
                            <span className="ml-auto text-xs text-fg-subtle tabular">{fmtDate(act.scheduled_at ?? act.created_at)}</span>
                          )}
                        </div>
                        {act.subject && <p className="mt-0.5 text-[13px] font-medium text-fg">{act.subject}</p>}
                        {act.content && <p className="mt-0.5 text-[13px] leading-relaxed text-fg-muted">{act.content}</p>}
                      </div>
                    </li>
                  );
                })}
              </ul>
            </section>
          </div>
        )}
      </Sheet>

      {/* Create */}
      <Dialog open={showForm} onOpenChange={setShowForm} dismissable={!saving}
        title="New lead" description="Track a prospect or opportunity."
        footer={<>
          <Button onClick={() => setShowForm(false)}>Cancel</Button>
          <Button type="submit" form="lead-form" variant="primary" loading={saving}>Create lead</Button>
        </>}>
        <form id="lead-form" onSubmit={handleCreate} className="space-y-4">
          <Field label="Lead title" required>
            <Input required value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })}
              placeholder="e.g. ERP system for Acme Ltd" />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Stage">
              <Select value={form.status} onChange={(e) => setForm({ ...form, status: e.target.value })}
                options={STAGES.filter((s) => reg.lead_start_statuses.includes(s.key)).map((s) => ({ value: s.key, label: s.label }))} />
            </Field>
            <Field label="Source">
              <Select value={form.source} onChange={(e) => setForm({ ...form, source: e.target.value })}
                options={SOURCES.map((s) => ({ value: s, label: SOURCE_LABEL[s] }))} />
            </Field>
            <Field label="Segment">
              <SegmentInput segments={segments} value={form.segment} onChange={(v) => setForm({ ...form, segment: v })} />
            </Field>
            <OrgContactPicker
              organizationId={form.organization_id}
              contactId={form.contact_id}
              onChange={({ organization_id, contact_id }) => setForm({ ...form, organization_id, contact_id })}
            />
          </FieldGrid>
          <Field label="Estimated value">
            <div className="flex gap-2">
              <Select aria-label="Currency" className="w-28" value={form.currency || ""} onChange={(e) => setForm({ ...form, currency: e.target.value })}>
                <option value="" title="The customer's country decides">Country</option>
                {activeCurrencies.map((c) => <option key={c} value={c}>{c}</option>)}
              </Select>
              <div className="flex-1">
                <Input numeric type="number" min="0" value={form.estimated_value} placeholder="0"
                  onChange={(e) => setForm({ ...form, estimated_value: e.target.value })} />
              </div>
            </div>
          </Field>
          {notice && showForm && <p role="alert" className="text-xs text-tone-red">{notice}</p>}
        </form>
      </Dialog>

      {disqualify && (
        <ReasonDialog title="Why is this lead disqualified?" reasons={reg.disqualify_reasons}
          saving={saving} error={reasonError}
          onCancel={() => setDisqualify(null)}
          onConfirm={(reasons) => confirmDisqualify(reasons[0])} />
      )}
    </div>
  );
}
