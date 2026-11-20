"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { formatCurrency, sumByCurrency } from "@/lib/currency";
import { useCurrencySettings } from "@/hooks/useCurrencySettings";
import { useEffect, useMemo, useState, useCallback } from "react";
import { useSession } from "next-auth/react";
import { crmApi, peopleApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Plus, Pencil, Trash2, X, Paperclip, Building2, User, AlertCircle, Search, KanbanSquare, List, ArrowUpRight } from "lucide-react";
import { useCrmStages } from "@/hooks/useCrmStages";
import { usePermissions } from "@/hooks/usePermissions";
import { REASON_STAGES, boardStages, isActiveStage, isWonStage, stageStyle, winRate as computeWinRate } from "@/lib/crmStages";
import ReasonDialog from "@/components/crm/ReasonDialog";
import FollowUpButton from "@/components/crm/FollowUpButton";
import Link from "next/link";
import AiSelectionBar from "@/components/crm/AiSelectionBar";
import OrgContactPicker from "@/components/crm/OrgContactPicker";
import AttachmentPanel from "@/components/ui/AttachmentPanel";
import { ProductLineMultiSelect } from "@/components/catalog/ProductLineSelect";
import { DndContext, type DragEndEvent } from "@dnd-kit/core";
import { useBoardSensors, DroppableColumn, DraggableCard } from "@/components/dnd/board";
import { MoveToMenu } from "@/components/dnd/MoveToMenu";
import { stageDotClass } from "@/components/crm/stageTone";
import {
  Button, Checkbox, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues, ListFrame,
  PageHeader, SavedViewTabs, Select, Sheet, Skeleton, Stat, StatGroup, StatusPill, Tabs, TabsContent, TabsList,
  TabsTrigger, toast, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { segmentFilterOptions, segmentFilterParams, segmentLabel, segmentOf, segmentPatch, useSegments, type WithSegment } from "@/lib/segments";
import SegmentInput from "@/components/crm/SegmentInput";

interface Deal extends WithSegment {
  currency?: string;
  id: string;
  title: string;
  stage: string;
  value?: number;
  probability?: number;
  expected_close_date?: string;
  product_line_ids?: string[];
  contact?: { first_name: string; last_name?: string };
  organization?: { name: string };
  assigned_to?: { name: string };
  contact_name?: string;
  organization_name?: string;
  contact_id?: string;
  organization_id?: string;
  assigned_to_id?: string | null;
  needs_next_action?: boolean;
  loss_reasons?: string[];
}

// Probability "" = the stage's default (crm.funnel.stage_probabilities, set by the backend).
// segment "" on a new deal = automatic (it follows the lead and the accepted quotation); a segment picked here is kept.
const EMPTY_FORM = { title: "", stage: "new", value: "", currency: "", probability: "", expected_close_date: "", product_line_ids: [] as string[], organization_id: "", contact_id: "", assigned_to_id: "", segment: "" };

const fmtCompact = (n?: number) => {
  if (!n) return "-";
  if (n >= 1e9) return `${(n/1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n/1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n/1e3).toFixed(0)}K`;
  return `${n}`;
};

const fmtFull = (n: number | undefined, currency: string) =>
  n == null ? "-" : formatCurrency(n, currency);

const fmtDay = (iso?: string) => iso ? new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-";

type Scope = "open" | "won" | "lost" | "all";
type Filters = { scope?: Scope; mine?: boolean; needsAction?: boolean; closingSoon?: boolean; segment?: string };

// Built-in views of the list (the pipeline board always shows every open stage).
const VIEWS: SavedView<Filters>[] = [
  { id: "open", label: "All open", filters: { scope: "open" } },
  { id: "mine", label: "Mine", filters: { scope: "open", mine: true } },
  { id: "next", label: "No next action", filters: { scope: "open", needsAction: true } },
  { id: "won", label: "Won", filters: { scope: "won" } },
  { id: "lost", label: "Lost", filters: { scope: "lost" } },
  { id: "all", label: "All deals", filters: { scope: "all" } },
];

const CHIPS: { id: "mine" | "needsAction" | "closingSoon"; label: string }[] = [
  { id: "mine", label: "Mine" },
  { id: "needsAction", label: "No next action" },
  { id: "closingSoon", label: "Closing in 30 days" },
];

const contactName = (d: Deal) =>
  d.contact_name || (d.contact ? [d.contact.first_name, d.contact.last_name].filter(Boolean).join(" ") : "");

function ProbabilityBar({ value, stage, wide }: { value: number; stage: string; wide?: boolean }) {
  return (
    <span className="inline-flex items-center justify-end gap-2">
      <span className={cn("h-1.5 overflow-hidden rounded-full bg-surface-2", wide ? "w-20" : "w-12")}>
        <span className={cn("block h-full rounded-full", stageDotClass("deal", stage))} style={{ width: `${value}%` }} />
      </span>
      <span className="w-9 text-right text-xs text-fg-muted tabular">{value}%</span>
    </span>
  );
}

export default function DealsPage() {
  const [deals, setDeals] = useState<Deal[]>([]);
  const [aiSelected, setAiSelected] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [view, setView] = useState<"pipeline" | "list">("pipeline");
  const [showForm, setShowForm] = useState(false);
  const [editingDeal, setEditingDeal] = useState<Deal | null>(null);
  const [form, setForm] = useState(EMPTY_FORM);
  const { functional, active: activeCurrencies } = useCurrencySettings();
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const sensors = useBoardSensors();
  const [selectedDeal, setSelectedDeal] = useState<Deal | null>(null);
  const [drawerTab, setDrawerTab] = useState<"details" | "files">("details");
  const { reg } = useCrmStages();
  const { can } = usePermissions();
  const canAssign = can("crm:assign_leads");
  const [users, setUsers] = useState<{ id: string; name: string }[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  // A move to lost / disqualified waits here for its reasons
  const [askReasons, setAskReasons] = useState<{ stage: string; dealId?: string } | null>(null);
  const columns = boardStages(reg);
  const style = (key: string) => stageStyle(reg, key);

  const { data: session } = useSession();
  const myId = (session?.user as { id?: string } | undefined)?.id ?? "";
  const views = useSavedViews<Filters>("crm.deals", VIEWS);
  const [chips, setChips] = useState<Partial<Record<(typeof CHIPS)[number]["id"], boolean>>>({});
  const [q, setQ] = useState("");
  // the segment filter (on the server; a saved view keeps it)
  const [segmentFilter, setSegmentFilter] = useState("");
  const segments = useSegments();
  const filters: Filters = {
    ...views.active?.filters,
    ...(chips.mine ? { mine: true } : {}),
    ...(chips.needsAction ? { needsAction: true } : {}),
    ...(chips.closingSoon ? { closingSoon: true } : {}),
    ...(segmentFilter ? { segment: segmentFilter } : {}),
  };
  const serverSegment = filters.segment ?? "";

  useEffect(() => {
    if (!canAssign) return;
    // The staff directory (any internal user may read it), not the admin users list (403 for staff).
    peopleApi.directory().then((r) => {
      setUsers((r.data ?? []).map((u) => ({ id: String(u.id), name: u.name || "User" })));
    }).catch(() => setUsers([]));
  }, [canAssign]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = { ...segmentFilterParams(serverSegment) };
      const { rows } = await fetchAllPages<Deal>(crmApi.listDeals, params);
      setDeals(rows);
    } catch { setDeals([]); } finally { setLoading(false); }
  }, [serverSegment]);

  useEffect(() => { load(); }, [load]);

  function openCreate() {
    setEditingDeal(null);
    setForm(EMPTY_FORM);
    setFormError(null);
    setShowForm(true);
  }

  function openEdit(deal: Deal) {
    setEditingDeal(deal);
    setForm({
      title: deal.title,
      stage: deal.stage,
      value: deal.value != null ? String(deal.value) : "",
      currency: deal.currency || "",
      probability: deal.probability != null ? String(deal.probability) : "50",
      expected_close_date: deal.expected_close_date ?? "",
      product_line_ids: deal.product_line_ids ?? [],
      organization_id: deal.organization_id ?? "",
      contact_id: deal.contact_id ?? "",
      assigned_to_id: deal.assigned_to_id ?? "",
      segment: segmentOf(deal),
    });
    setFormError(null);
    setShowForm(true);
  }

  function closeForm() {
    setShowForm(false);
    setEditingDeal(null);
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const stageChanged = !editingDeal || editingDeal.stage !== form.stage;
    if (stageChanged && REASON_STAGES[form.stage]) {
      setAskReasons({ stage: form.stage });   // the save continues in the reason dialog
      return;
    }
    void submitForm();
  }

  async function submitForm(reasons?: string[], notes?: string) {
    setSaving(true);
    setFormError(null);
    try {
      const payload: Record<string, unknown> = {
        title: form.title,
        stage: form.stage,
        value: form.value ? Number(form.value) : undefined,
        currency: form.currency || functional,
        probability: form.probability === "" ? undefined : Number(form.probability),
        expected_close_date: form.expected_close_date || undefined,
        product_line_ids: form.product_line_ids,
        organization_id: form.organization_id || null,
        contact_id: form.contact_id || null,
        ...(reasons ? { loss_reasons: reasons, loss_notes: notes || undefined } : {}),
      };
      // a segment picked by hand (new deal) or changed (edit) is kept; otherwise it stays automatic
      if (form.segment.trim() !== segmentOf(editingDeal) && (editingDeal || form.segment.trim())) {
        Object.assign(payload, segmentPatch(form.segment));
      }
      if (editingDeal && canAssign && form.assigned_to_id && form.assigned_to_id !== (editingDeal.assigned_to_id ?? "")) {
        payload.assigned_to_id = form.assigned_to_id;
      }
      if (editingDeal) {
        await crmApi.updateDeal(editingDeal.id, payload);
      } else {
        await crmApi.createDeal(payload);
      }
      setAskReasons(null);
      setShowForm(false);
      setEditingDeal(null);
      setForm(EMPTY_FORM);
      load();
    } catch (err) {
      setFormError(extractApiError(err));
    } finally { setSaving(false); }
  }

  async function moveWithReasons(dealId: string, stage: string, reasons: string[], notes: string) {
    setSaving(true);
    try {
      await crmApi.updateDealStage(dealId, { stage, loss_reasons: reasons, loss_notes: notes || undefined });
      setAskReasons(null);
      load();
    } catch (err) {
      setFormError(extractApiError(err));
    } finally { setSaving(false); }
  }

  async function handleDelete(id: string) {
    setDeleting(true);
    try {
      await crmApi.deleteDeal(id);
      setConfirmDeleteId(null);
      setDeals((prev) => prev.filter((d) => d.id !== id));
      setSelectedDeal((s) => (s?.id === id ? null : s));
    } catch { /* keep showing */ } finally { setDeleting(false); }
  }

  function onDragEnd(e: DragEndEvent) {
    if (e.over) moveDeal(String(e.active.id), String(e.over.id));
  }

  /** Move a deal to another stage: by dragging, or with the card's "Move to" menu. */
  function moveDeal(id: string, targetStage: string) {
    const deal = deals.find((d) => d.id === id);
    if (!deal || deal.stage === targetStage) return;
    if (REASON_STAGES[targetStage]) {
      setFormError(null);
      setAskReasons({ stage: targetStage, dealId: id });
      return;
    }
    setNotice(null);
    setDeals((prev) => prev.map((d) => d.id === id ? { ...d, stage: targetStage } : d));
    crmApi.updateDealStage(id, { stage: targetStage }).then(() => load()).catch((err) => {
      setDeals((prev) => prev.map((d) => d.id === id ? { ...d, stage: deal.stage } : d));
      setNotice(extractApiError(err));
    });
  }

  // Deals carry their own currency, so these are grouped rather than blindly summed.
  const activeDeals = deals.filter((d) => isActiveStage(reg, d.stage));
  const wonDeals = deals.filter((d) => isWonStage(reg, d.stage));
  const pipeline = sumByCurrency(activeDeals, (d) => d.value, (d) => d.currency, functional);
  const won = sumByCurrency(wonDeals, (d) => d.value, (d) => d.currency, functional);
  const winRate = computeWinRate(reg, deals.map((d) => d.stage));
  const decided = deals.filter((d) => reg.deal_stages.find((s) => s.key === d.stage)?.is_closed).length;

  // The list view: the active saved view, the chips and the search, all on the client.
  const listed = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const soon = Date.now() + 30 * 86_400_000;
    return deals.filter((d) => {
      const st = reg.deal_stages.find((s) => s.key === d.stage);
      if (filters.scope === "open" && st?.is_closed) return false;
      if (filters.scope === "won" && !st?.is_won) return false;
      if (filters.scope === "lost" && !(st?.is_closed && !st.is_won)) return false;
      if (filters.mine && (!myId || d.assigned_to_id !== myId)) return false;
      if (filters.needsAction && !d.needs_next_action) return false;
      if (filters.closingSoon) {
        const t = d.expected_close_date ? new Date(d.expected_close_date).getTime() : NaN;
        if (!(t <= soon)) return false;
      }
      if (needle && ![d.title, d.organization_name, d.organization?.name, contactName(d)]
        .some((s) => s?.toLowerCase().includes(needle))) return false;
      return true;
    });
  }, [deals, reg, filters.scope, filters.mine, filters.needsAction, filters.closingSoon, myId, q]);
  const listedTotal = sumByCurrency(listed, (d) => d.value, (d) => d.currency, functional);

  function deleteConfirm(deal: Deal) {
    return confirmDeleteId === deal.id ? (
      <span className="inline-flex items-center gap-1.5">
        <span className="text-xs font-medium text-tone-red">Delete?</span>
        <Button size="sm" variant="danger" loading={deleting} onClick={() => handleDelete(deal.id)}>Yes, delete</Button>
        <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
      </span>
    ) : (
      <IconButton size="sm" icon={Trash2} label={`Delete ${deal.title}`} onClick={() => setConfirmDeleteId(deal.id)} />
    );
  }

  const tableColumns: ColumnDef<Deal>[] = [
    {
      id: "select",
      enableSorting: false,
      meta: { width: "40px" },
      header: () => (
        <Checkbox aria-label="Select every deal shown"
          checked={listed.length > 0 && listed.every((d) => aiSelected.includes(d.id))}
          onChange={(e) => setAiSelected(e.target.checked ? listed.map((d) => d.id) : [])} />
      ),
      cell: ({ row }) => (
        <Checkbox aria-label={`Select ${row.original.title}`} checked={aiSelected.includes(row.original.id)}
          onChange={(e) => setAiSelected((prev) =>
            e.target.checked ? [...prev, row.original.id] : prev.filter((id) => id !== row.original.id))} />
      ),
    },
    {
      accessorKey: "title",
      header: "Deal",
      cell: ({ row }) => (
        <div className="min-w-0">
          <Link href={`/crm/deals/${row.original.id}`} className="font-medium text-fg hover:text-brand-ink">{row.original.title}</Link>
          {row.original.needs_next_action && (
            <span className="ml-2 inline-flex items-center gap-1 text-xs font-medium text-tone-orange">
              <AlertCircle className="size-3" aria-hidden /> No next action
            </span>
          )}
        </div>
      ),
    },
    {
      id: "organization",
      header: "Organization",
      accessorFn: (d) => d.organization?.name ?? d.organization_name ?? contactName(d),
      meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{c.getValue<string>() || "-"}</span>,
    },
    {
      accessorKey: "stage",
      header: "Stage",
      sortingFn: (a, b) => (reg.deal_stages.find((s) => s.key === a.original.stage)?.order ?? 99)
        - (reg.deal_stages.find((s) => s.key === b.original.stage)?.order ?? 99),
      cell: ({ row }) => <StatusPill family="deal" status={row.original.stage} label={style(row.original.stage).label} />,
    },
    {
      accessorKey: "value",
      header: "Value",
      meta: { numeric: true },
      cell: ({ row }) => fmtFull(row.original.value, row.original.currency || functional),
    },
    {
      accessorKey: "probability",
      header: "Probability",
      meta: { numeric: true, hideOnMobile: true },
      cell: ({ row }) => row.original.probability != null
        ? <ProbabilityBar value={row.original.probability} stage={row.original.stage} wide /> : "-",
    },
    {
      accessorKey: "expected_close_date",
      header: "Close date",
      meta: { numeric: true, hideOnMobile: true },
      cell: ({ row }) => fmtDay(row.original.expected_close_date),
    },
    {
      id: "segment",
      header: "Segment",
      accessorFn: (d) => segmentLabel(segments, segmentOf(d)),
      meta: { hideOnMobile: true },
      cell: ({ row }) => <span className="text-fg-muted">{segmentLabel(segments, segmentOf(row.original))}</span>,
    },
  ];

  function lossLabelsOf(deal: Deal) {
    return (deal.loss_reasons ?? []).map((k) =>
      [...reg.loss_reasons, ...reg.disqualify_reasons].find((r) => r.key === k)?.label ?? k);
  }

  function dealFacts(deal: Deal) {
    const lossLabels = lossLabelsOf(deal);
    return [
      { label: "Stage", value: <StatusPill family="deal" status={deal.stage} label={style(deal.stage).label} /> },
      { label: "Value", value: deal.value != null ? fmtFull(deal.value, deal.currency || functional) : "-", numeric: true },
      ...(deal.probability != null ? [{ label: "Probability", value: <ProbabilityBar value={deal.probability} stage={deal.stage} /> }] : []),
      ...(deal.expected_close_date ? [{ label: "Close date", value: fmtDay(deal.expected_close_date), numeric: true }] : []),
      ...(deal.organization?.name || deal.organization_name ? [{ label: "Organization", value: deal.organization?.name ?? deal.organization_name }] : []),
      ...(contactName(deal) ? [{ label: "Contact", value: contactName(deal) }] : []),
      ...(deal.assigned_to ? [{ label: "Assigned to", value: deal.assigned_to.name }] : []),
      ...(lossLabels.length > 0 ? [{ label: "Reasons", value: lossLabels.join(", ") }] : []),
    ];
  }

  const viewToggle = (
    <div role="group" aria-label="Layout" className="inline-flex rounded-lg border border-line-strong bg-surface p-0.5">
      {(["pipeline", "list"] as const).map((v) => {
        const Icon = v === "pipeline" ? KanbanSquare : List;
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
  );

  return (
    <div className="space-y-4">
      <PageHeader className="mb-0" title="Deals"
        summary={view === "pipeline"
          ? `${activeDeals.length} open, ${pipeline.currency} ${fmtCompact(pipeline.total)} in the pipeline`
          : `${listed.length} ${listed.length === 1 ? "deal" : "deals"} in this view, ${listedTotal.currency} ${fmtCompact(listedTotal.total)}${listedTotal.mixed ? " *" : ""}`}
        actions={<>
          {viewToggle}
          <Button variant="primary" onClick={openCreate}><Plus aria-hidden /> New deal</Button>
        </>} />

      <StatGroup>
        <Stat label="Pipeline" value={`${pipeline.currency} ${fmtCompact(pipeline.total)}`}
          note={pipeline.mixed ? `${activeDeals.length} active, mixed currencies` : `${activeDeals.length} active`} />
        <Stat label="Won" value={`${won.currency} ${fmtCompact(won.total)}`}
          note={won.mixed ? `${wonDeals.length} won, mixed currencies` : `${wonDeals.length} won`} />
        <Stat label="Win rate" value={winRate == null ? "-" : `${winRate}%`} note={`of ${decided} decided deals`} />
      </StatGroup>

      {notice && (
        <div role="alert" className="flex items-center gap-2 rounded-lg border border-tone-red/30 bg-tone-red-soft px-4 py-2.5 text-sm text-tone-red">
          <AlertCircle className="size-4 shrink-0" aria-hidden />{notice}
          <IconButton size="sm" icon={X} label="Dismiss" className="ml-auto text-tone-red" onClick={() => setNotice(null)} />
        </div>
      )}

      {view === "pipeline" ? (
        loading ? (
          <div className="flex gap-4 overflow-x-auto pb-4">
            {[0, 1, 2, 3].map((i) => <Skeleton key={i} className="h-48 w-56 shrink-0" />)}
          </div>
        ) : (
          <DndContext sensors={sensors} onDragEnd={onDragEnd}>
            {/* Phones: one column at a time (85% wide, the next one peeking), snapping as you swipe */}
            <div className="-mx-1 flex snap-x snap-mandatory gap-3 overflow-x-auto scroll-px-1 px-1 pb-4 md:snap-none">
              {columns.map((col) => {
                const stage = style(col.key);
                const stageDeals = deals.filter((d) => d.stage === stage.key);
                const stageTotal = sumByCurrency(stageDeals, (d) => d.value, (d) => d.currency, functional);
                return (
                  <DroppableColumn key={stage.key} id={stage.key} className="w-[85%] flex-shrink-0 snap-start rounded-lg md:w-60">
                    {(isOver) => (
                      <div className={cn("rounded-lg border border-line bg-surface-2/60 p-2 transition-colors", isOver && "bg-brand/[0.05] ring-2 ring-brand/20")}>
                        <div className="mb-1 flex items-center gap-2 px-1">
                          <span className={cn("size-2 rounded-full", stageDotClass("deal", stage.key))} aria-hidden />
                          <span className="text-[13px] font-semibold text-fg">{stage.label}</span>
                          <span className="ml-auto rounded-full bg-surface px-2 text-[11px] font-medium text-fg-muted tabular">{stageDeals.length}</span>
                        </div>
                        <p className="mb-2 px-1 text-xs text-fg-muted tabular"
                          title={stageTotal.mixed ? `Mixed currencies: showing the ${stageTotal.currency} subtotal` : undefined}>
                          {stageTotal.total > 0 ? `${stageTotal.currency} ${fmtCompact(stageTotal.total)}${stageTotal.mixed ? " *" : ""}` : " "}
                        </p>

                        <div className="min-h-16 space-y-2">
                          {stageDeals.length === 0 && (
                            <div className={cn("rounded-lg border-2 border-dashed p-4 text-center text-xs text-fg-subtle transition-colors",
                              isOver ? "border-brand/30" : "border-line")}>
                              Drop here
                            </div>
                          )}
                          {stageDeals.map((deal) => (
                            <DraggableCard key={deal.id} id={deal.id}
                              onClick={() => { setSelectedDeal(deal); setDrawerTab("details"); }}
                              className="group relative cursor-grab rounded-lg border border-line bg-surface p-3 shadow-card transition-shadow hover:shadow-pop active:cursor-grabbing">
                              <div className={"flex justify-end gap-0.5 transition-opacity max-md:-mr-1.5 max-md:-mt-1.5 max-md:mb-1 md:absolute md:right-1.5 md:top-1.5 " +
                                "focus-within:opacity-100 group-hover:opacity-100 [@media(hover:hover)]:opacity-0"}
                                onClick={(e) => e.stopPropagation()} onMouseDown={(e) => e.stopPropagation()} onTouchStart={(e) => e.stopPropagation()}>
                                {confirmDeleteId === deal.id ? (
                                  <span className="flex items-center gap-1 rounded-lg border border-tone-red/30 bg-surface-raised px-2 py-1 shadow-card">
                                    <span className="text-[11px] font-medium text-tone-red">Delete?</span>
                                    <button type="button" onClick={() => handleDelete(deal.id)} disabled={deleting}
                                      className="text-[11px] font-semibold text-tone-red hover:underline disabled:opacity-50">
                                      {deleting ? "..." : "Yes"}
                                    </button>
                                    <span className="text-[11px] text-fg-subtle">/</span>
                                    <button type="button" onClick={() => setConfirmDeleteId(null)}
                                      className="text-[11px] font-semibold text-fg-muted hover:text-fg">No</button>
                                  </span>
                                ) : (
                                  <>
                                    <MoveToMenu itemLabel={deal.title} current={stage.key} onMove={(to) => moveDeal(deal.id, to)}
                                      options={columns.map((c) => ({ value: style(c.key).key, label: style(c.key).label }))} />
                                    <IconButton size="sm" icon={Pencil} label={`Edit ${deal.title}`} className="bg-surface-raised" onClick={() => openEdit(deal)} />
                                    <IconButton size="sm" icon={Trash2} label={`Delete ${deal.title}`} className="bg-surface-raised" onClick={() => setConfirmDeleteId(deal.id)} />
                                  </>
                                )}
                              </div>
                              <p className="mb-1.5 text-sm font-medium md:pr-20 leading-snug text-fg group-hover:text-brand-ink">{deal.title}</p>
                              {(deal.organization_name || deal.contact_name) && (
                                <div className="mb-2 flex flex-col gap-0.5">
                                  {deal.organization_name && (
                                    <span className="inline-flex items-center gap-1 truncate text-xs text-fg-muted">
                                      <Building2 className="size-3 shrink-0 text-fg-subtle" aria-hidden />{deal.organization_name}
                                    </span>
                                  )}
                                  {deal.contact_name && (
                                    <span className="inline-flex items-center gap-1 truncate text-xs text-fg-muted">
                                      <User className="size-3 shrink-0 text-fg-subtle" aria-hidden />{deal.contact_name}
                                    </span>
                                  )}
                                </div>
                              )}
                              <div className="flex items-end justify-between gap-2">
                                <span className="text-sm font-semibold text-fg tabular">{deal.currency || functional} {fmtCompact(deal.value)}</span>
                                {deal.probability != null && <ProbabilityBar value={deal.probability} stage={stage.key} />}
                              </div>
                              {deal.expected_close_date && (
                                <p className="mt-2 text-[11px] text-fg-muted tabular">
                                  Close {new Date(deal.expected_close_date).toLocaleDateString("en-GB", { day: "numeric", month: "short" })}
                                </p>
                              )}
                              {deal.needs_next_action && (
                                <p className="mt-2 inline-flex items-center gap-1 text-[11px] font-medium text-tone-orange">
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
        )
      ) : (
        <>
          <ListFrame>
            <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChips({})} />
            <FilterChips
              chips={CHIPS.map((c) => ({ id: c.id, label: c.label, active: !!chips[c.id] }))}
              onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id as keyof typeof c] }))}
              onClear={() => setChips({})}>
              {segments.length > 0 && (
                <Select aria-label="Segment filter" className="w-48" value={segmentFilter} placeholder="All segments"
                  onChange={(e) => setSegmentFilter(e.target.value)} options={segmentFilterOptions(segments)} />
              )}
              <Input type="search" leading={Search} value={q} onChange={(e) => setQ(e.target.value)}
                placeholder="Search deals" aria-label="Search deals" className="w-56" autoComplete="off" />
            </FilterChips>
            <AiSelectionBar objectType="deal" selected={aiSelected}
              onClear={() => setAiSelected([])}
              onMessage={(kind, text) => (kind === "error" ? toast.error(text) : kind === "success" ? toast.success(text) : toast(text))} />
            <DataTable label="Deals" columns={tableColumns} data={listed} getRowId={(d) => d.id} loading={loading}
              empty={{ title: "No deals in this view", description: "Pick another view, clear the filters, or create a deal.",
                action: <Button variant="primary" size="sm" onClick={openCreate}><Plus aria-hidden /> New deal</Button> }}
              renderDetails={(deal) => (
                <div className="flex flex-wrap items-start gap-6">
                  <KeyValues className="min-w-[260px] flex-1" items={dealFacts(deal)} />
                  <div className="flex flex-wrap items-center gap-2">
                    <Button size="sm" variant="primary" asChild>
                      <Link href={`/crm/deals/${deal.id}`}>Open deal <ArrowUpRight aria-hidden /></Link>
                    </Button>
                    <Button size="sm" onClick={() => openEdit(deal)}><Pencil aria-hidden /> Edit</Button>
                    <Button size="sm" onClick={() => { setSelectedDeal(deal); setDrawerTab("files"); }}><Paperclip aria-hidden /> Files</Button>
                    <FollowUpButton compact entityType="deal" entityId={deal.id} subject={deal.title} />
                    {deleteConfirm(deal)}
                  </div>
                </div>
              )} />
          </ListFrame>
        </>
      )}

      {/* Deal details, opened from a pipeline card (or Files in the list) */}
      <Sheet open={!!selectedDeal} onOpenChange={(o) => { if (!o) setSelectedDeal(null); }}
        title={selectedDeal?.title ?? "Deal"}
        description={selectedDeal?.organization?.name ?? selectedDeal?.organization_name}>
        {selectedDeal && (
          <div className="space-y-4">
            <div className="flex flex-wrap items-center gap-2">
              <Button size="sm" variant="primary" asChild>
                <Link href={`/crm/deals/${selectedDeal.id}`}>Open deal <ArrowUpRight aria-hidden /></Link>
              </Button>
              <Button size="sm" onClick={() => openEdit(selectedDeal)}><Pencil aria-hidden /> Edit</Button>
              <FollowUpButton compact entityType="deal" entityId={selectedDeal.id} subject={selectedDeal.title} />
              {deleteConfirm(selectedDeal)}
            </div>
            <Tabs value={drawerTab} onValueChange={(v) => setDrawerTab(v as "details" | "files")}>
              <TabsList>
                <TabsTrigger value="details">Details</TabsTrigger>
                <TabsTrigger value="files"><Paperclip className="size-3.5" aria-hidden /> Files</TabsTrigger>
              </TabsList>
              <TabsContent value="details" className="pt-4">
                <KeyValues items={dealFacts(selectedDeal)} />
              </TabsContent>
              <TabsContent value="files" className="pt-4">
                <AttachmentPanel entityType="crm_deal" entityId={selectedDeal.id} />
              </TabsContent>
            </Tabs>
          </div>
        )}
      </Sheet>

      {/* Create / edit */}
      <Dialog open={showForm} onOpenChange={(o) => { if (!o) closeForm(); }} dismissable={!saving}
        title={editingDeal ? "Edit deal" : "New deal"}
        description={editingDeal ? "Update the opportunity's details." : "Add an opportunity to the pipeline."}
        footer={<>
          <Button onClick={closeForm}>Cancel</Button>
          <Button type="submit" form="deal-form" variant="primary" loading={saving}>
            {editingDeal ? "Save deal" : "Create deal"}
          </Button>
        </>}>
        <form id="deal-form" onSubmit={handleSubmit} className="space-y-4">
          <Field label="Title" required>
            <Input required value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })}
              placeholder="e.g. ERP implementation, Acme Ltd" />
          </Field>
          <FieldGrid cols={2}>
            <Field label="Stage">
              <Select value={form.stage} onChange={(e) => {
                // Changing the stage resets the probability to that stage's default
                const p = reg.deal_stages.find((s) => s.key === e.target.value)?.probability;
                setForm({ ...form, stage: e.target.value, probability: p != null ? String(p) : "" });
              }}>
                {reg.deal_stages.map((s) => <option key={s.key} value={s.key}>{s.label}</option>)}
                {!reg.deal_stages.some((s) => s.key === form.stage) && <option value={form.stage}>{form.stage}</option>}
              </Select>
            </Field>
            <Field label="Probability">
              <Input numeric type="number" min="0" max="100" value={form.probability} placeholder="Stage default" trailing="%"
                onChange={(e) => setForm({ ...form, probability: e.target.value })} />
            </Field>
            <Field label="Value">
              <div className="flex gap-2">
                <Select aria-label="Currency" className="w-24" value={form.currency || functional}
                  onChange={(e) => setForm({ ...form, currency: e.target.value })}
                  options={activeCurrencies.map((c) => ({ value: c, label: c }))} />
                <div className="flex-1">
                  <Input numeric type="number" min="0" value={form.value} onChange={(e) => setForm({ ...form, value: e.target.value })} />
                </div>
              </div>
            </Field>
            <Field label="Close date">
              <Input type="date" value={form.expected_close_date} onChange={(e) => setForm({ ...form, expected_close_date: e.target.value })} />
            </Field>
            <OrgContactPicker
              organizationId={form.organization_id}
              contactId={form.contact_id}
              onChange={({ organization_id, contact_id }) => setForm({ ...form, organization_id, contact_id })}
            />
          </FieldGrid>
          <Field label="Product lines">
            <ProductLineMultiSelect value={form.product_line_ids} onChange={(v) => setForm({ ...form, product_line_ids: v })} valueKey="code" />
          </Field>
          <Field label="Segment" hint={editingDeal ? undefined : "Leave it empty to follow the lead and the accepted quotation."}>
            <SegmentInput segments={segments} value={form.segment} placeholder={editingDeal ? "None" : "Automatic"}
              onChange={(v) => setForm({ ...form, segment: v })} />
          </Field>
          {editingDeal && canAssign && users.length > 0 && (
            <Field label="Owner">
              <Select value={form.assigned_to_id} onChange={(e) => setForm({ ...form, assigned_to_id: e.target.value })}>
                {!form.assigned_to_id && <option value="">Unassigned</option>}
                {users.map((u) => <option key={u.id} value={u.id}>{u.name}</option>)}
              </Select>
            </Field>
          )}
          {formError && !askReasons && <p role="alert" className="text-xs text-tone-red">{formError}</p>}
        </form>
      </Dialog>

      {askReasons && (
        <ReasonDialog
          title={askReasons.stage === "lost" ? "Why was this deal lost?" : "Why is this deal disqualified?"}
          reasons={reg[REASON_STAGES[askReasons.stage]]}
          multiple withNotes saving={saving} error={formError}
          onCancel={() => { setAskReasons(null); setFormError(null); }}
          onConfirm={(reasons, notes) => askReasons.dealId
            ? moveWithReasons(askReasons.dealId, askReasons.stage, reasons, notes)
            : submitForm(reasons, notes)} />
      )}
    </div>
  );
}
