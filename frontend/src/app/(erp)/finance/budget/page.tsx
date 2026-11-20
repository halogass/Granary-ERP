"use client";

import { useState, useEffect, useCallback, useMemo } from "react";
import { Plus, BarChart3, Trash2, CheckCircle2, AlertTriangle } from "lucide-react";
import api from "@/lib/api";
import { usePermissions } from "@/hooks/usePermissions";
import {
  Button, DataTable, Dialog, Field, FieldGrid, FilterChips, IconButton, Input, ListFrame, PageHeader, Pill,
  SavedViewTabs, Select, Skeleton, Stat, StatGroup, StatusPill, Textarea, toast, useSavedViews,
  type ColumnDef, type SavedView,
} from "@/components/ui";
import { cn } from "@/lib/utils";

interface Budget {
  id: string;
  name: string;
  fiscal_year: number;
  department: string | null;
  period_type: string;
  status: string;
  total_budgeted: number;
  totals_by_currency?: Record<string, number>;
  approved_at: string | null;
  notes: string | null;
}

interface BudgetLine {
  line_id: string;
  account_id: string;
  account_code: string;
  account_name: string;
  account_category: string;
  label: string | null;
  period_month: number | null;
  currency: string;
  foreign_currency?: boolean;   // saved in another currency before budgets were functional-only
  budgeted: number;
  actual: number;
  variance: number | null;
  variance_pct: number;
  is_over_budget: boolean;
}

interface VsActual {
  budget_id: string;
  budget_name: string;
  fiscal_year: number;
  period_type: string;
  status: string;
  total_budgeted: number;
  total_actual: number;
  total_variance: number;
  functional_currency?: string;
  foreign_currency_lines?: number;
  lines: BudgetLine[];
}

interface Account { id: string; code: string; name: string; category: string; }

type Filters = { status?: string; year?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "active", label: "Active", filters: { status: "active" } },
  { id: "closed", label: "Closed", filters: { status: "closed" } },
];

const PERIOD_LABELS: Record<string, string> = {
  annual: "Annual", quarterly: "Quarterly", monthly: "Monthly",
};

const MONTH_NAMES = ["", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

const EMPTY_FORM = {
  name: "", fiscal_year: new Date().getFullYear().toString(),
  department: "", period_type: "annual", notes: "",
};

const EMPTY_LINE = { account_id: "", label: "", budgeted_amount: "", period_month: "" };

const millions = (n: number) => `${(n / 1e6).toFixed(1)}M`;

export default function BudgetPage() {
  const views = useSavedViews<Filters>("finance.budgets", VIEWS);
  const [budgets, setBudgets] = useState<Budget[]>([]);
  const [loading, setLoading] = useState(true);
  const [yearFilter, setYearFilter] = useState("");
  const statusFilter = views.active?.filters.status ?? "";

  // Create budget
  const [showCreate, setShowCreate] = useState(false);
  const [form, setForm] = useState(EMPTY_FORM);
  const [saving, setSaving] = useState(false);

  // Expanded row (vs-actual detail)
  const [activeId, setActiveId] = useState<string | null>(null);
  const [vsActual, setVsActual] = useState<VsActual | null>(null);
  const [vsLoading, setVsLoading] = useState(false);

  // Add line
  const [showAddLine, setShowAddLine] = useState(false);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [lineForm, setLineForm] = useState(EMPTY_LINE);
  const [lineSaving, setLineSaving] = useState(false);

  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [confirmCloseId, setConfirmCloseId] = useState<string | null>(null);
  const [movingId, setMovingId] = useState<string | null>(null);
  const { holds } = usePermissions();
  const canManage = holds("finance:manage_budget");

  // Budgets are in the functional currency (Settings > Currencies), compared with ledger actuals.
  const [functional, setFunctional] = useState("");
  useEffect(() => {
    api.get("/system/currencies").then(res => setFunctional(res.data?.functional ?? "")).catch(() => {});
  }, []);

  // A saved view may carry a year; restore it when the view is chosen or restored.
  useEffect(() => {
    setYearFilter(views.active?.filters.year ?? "");
  }, [views.activeId]); // eslint-disable-line react-hooks/exhaustive-deps

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (yearFilter) params.fiscal_year = yearFilter;
      if (statusFilter) params.status = statusFilter;
      const res = await api.get("/finance/budgets", { params });
      setBudgets(res.data ?? []);
    } catch {
      setBudgets([]);
    } finally {
      setLoading(false);
    }
  }, [yearFilter, statusFilter]);

  useEffect(() => { load(); }, [load]);

  async function openDetail(id: string) {
    setActiveId(id);
    setVsLoading(true);
    try {
      const res = await api.get(`/finance/budgets/${id}/vs-actual`);
      setVsActual(res.data);
    } catch {
      setVsActual(null);
    } finally {
      setVsLoading(false);
    }
  }

  function closeDetail() {
    setActiveId(null);
    setVsActual(null);
    setConfirmDeleteId(null);
  }

  async function createBudget() {
    if (!form.name.trim()) return toast.error("Budget name is required");
    setSaving(true);
    try {
      await api.post("/finance/budgets", {
        name: form.name.trim(),
        fiscal_year: parseInt(form.fiscal_year),
        department: form.department || null,
        period_type: form.period_type,
        notes: form.notes || null,
      });
      toast.success("Budget created");
      setShowCreate(false);
      setForm(EMPTY_FORM);
      load();
    } catch {
      toast.error("Failed to create budget");
    } finally {
      setSaving(false);
    }
  }

  async function approveBudget(id: string) {
    try {
      await api.post(`/finance/budgets/${id}/approve`);
      toast.success("Budget approved");
      load();
      if (activeId === id) openDetail(id);
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Failed to approve");
    }
  }

  // Approved -> active (being tracked) -> closed (the year is over); PATCH /finance/budgets/{id} {status}.
  async function moveBudget(id: string, status: "active" | "closed") {
    setMovingId(id);
    try {
      await api.patch(`/finance/budgets/${id}`, { status });
      toast.success(status === "active" ? "Budget activated" : "Budget closed");
      setConfirmCloseId(null);
      load();
      if (activeId === id) openDetail(id);
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? (status === "active" ? "Failed to activate" : "Failed to close"));
    } finally { setMovingId(null); }
  }

  async function deleteBudget(id: string) {
    try {
      await api.delete(`/finance/budgets/${id}`);
      toast.success("Budget deleted");
      setConfirmDeleteId(null);
      if (activeId === id) { setActiveId(null); setVsActual(null); }
      load();
    } catch (err: unknown) {
      const msg = (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      toast.error(msg ?? "Cannot delete approved/active budget");
    }
  }

  async function openAddLine() {
    if (!accounts.length) {
      try {
        const res = await api.get("/finance/accounts", { params: { limit: 200 } });
        setAccounts(res.data ?? []);
      } catch { setAccounts([]); }
    }
    setLineForm(EMPTY_LINE);
    setShowAddLine(true);
  }

  async function addLine() {
    if (!activeId || !lineForm.account_id || !lineForm.budgeted_amount) {
      return toast.error("Account and amount are required");
    }
    setLineSaving(true);
    try {
      await api.post(`/finance/budgets/${activeId}/lines`, {
        account_id: lineForm.account_id,
        label: lineForm.label || null,
        budgeted_amount: lineForm.budgeted_amount,
        period_month: lineForm.period_month ? parseInt(lineForm.period_month) : null,
      });
      toast.success("Line added");
      setShowAddLine(false);
      openDetail(activeId);
    } catch (err) {
      const detail = (err as { response?: { data?: { detail?: unknown } } })?.response?.data?.detail;
      toast.error(typeof detail === "string" ? detail : "Failed to add line");
    } finally {
      setLineSaving(false);
    }
  }

  async function deleteLine(budgetId: string, lineId: string) {
    try {
      await api.delete(`/finance/budgets/${budgetId}/lines/${lineId}`);
      toast.success("Line removed");
      openDetail(budgetId);
    } catch {
      toast.error("Failed to remove line");
    }
  }

  const years = Array.from(
    new Set(budgets.map((b) => b.fiscal_year))
  ).sort((a, b) => b - a);
  if (yearFilter && !years.includes(Number(yearFilter))) years.unshift(Number(yearFilter));

  const currentFilters: Filters = {
    ...(statusFilter ? { status: statusFilter } : {}),
    ...(yearFilter ? { year: yearFilter } : {}),
  };

  const columns = useMemo<ColumnDef<Budget>[]>(() => [
    { accessorKey: "name", header: "Budget", cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
    { accessorKey: "fiscal_year", header: "Year", meta: { numeric: true, width: "80px" }, cell: (c) => `FY${c.getValue<number>()}` },
    { accessorKey: "period_type", header: "Period", meta: { hideOnMobile: true }, cell: (c) => PERIOD_LABELS[c.getValue<string>()] ?? c.getValue<string>() },
    { accessorKey: "department", header: "Department", meta: { hideOnMobile: true }, cell: (c) => c.getValue<string | null>() ?? <span className="text-fg-subtle">-</span> },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="budget" status={c.getValue<string>()} /> },
    {
      accessorKey: "total_budgeted", header: "Budgeted", meta: { numeric: true },
      cell: (c) => (
        <div className="flex flex-col items-end">
          {Object.entries(c.row.original.totals_by_currency ?? { [functional]: c.row.original.total_budgeted }).map(([cur, amt]) => (
            <span key={cur}>{cur} {millions(amt)}</span>
          ))}
        </div>
      ),
    },
  ], [functional]);

  function renderDetails(b: Budget) {
    const isDraft = b.status === "draft";
    const loadingThis = vsLoading || (!!vsActual && vsActual.budget_id !== b.id);
    return (
      <div className="space-y-3">
        <div className="flex flex-wrap items-center gap-2">
          <p className="text-[13px] text-fg-muted">
            FY{vsActual?.fiscal_year ?? b.fiscal_year} · {PERIOD_LABELS[vsActual?.period_type ?? b.period_type ?? "annual"]}
            {b.department ? ` · ${b.department}` : ""}
          </p>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            {canManage && isDraft && <Button size="sm" variant="primary" onClick={() => approveBudget(b.id)}>Approve</Button>}
            {canManage && b.status === "approved" && (
              <Button size="sm" variant="primary" loading={movingId === b.id} onClick={() => moveBudget(b.id, "active")}>Activate</Button>
            )}
            {canManage && (b.status === "approved" || b.status === "active") && (confirmCloseId === b.id ? (
              <div className="flex items-center gap-1.5">
                <span className="text-xs font-semibold text-fg">Close budget? It cannot be reopened.</span>
                <Button size="sm" variant="primary" loading={movingId === b.id} onClick={() => moveBudget(b.id, "closed")}>Yes, close</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmCloseId(null)}>No</Button>
              </div>
            ) : (
              <Button size="sm" onClick={() => setConfirmCloseId(b.id)}>Close budget</Button>
            ))}
            {/* Lines of an approved budget are read-only, and only drafts can be deleted. */}
            {canManage && isDraft && <Button size="sm" onClick={openAddLine}><Plus aria-hidden /> Add line</Button>}
            {!(canManage && isDraft) ? null : confirmDeleteId === b.id ? (
              <div className="flex items-center gap-1.5">
                <span className="text-xs font-semibold text-tone-red">Delete budget?</span>
                <Button size="sm" variant="danger" onClick={() => deleteBudget(b.id)}>Yes, delete</Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
              </div>
            ) : (
              <IconButton size="sm" icon={Trash2} label="Delete budget" onClick={() => setConfirmDeleteId(b.id)} />
            )}
          </div>
        </div>

        {vsActual && (vsActual.foreign_currency_lines ?? 0) > 0 && (
          <div className="flex items-center gap-2 rounded-lg bg-tone-orange-soft px-3 py-2 text-xs text-tone-orange">
            <AlertTriangle className="size-3.5 shrink-0" aria-hidden />
            {vsActual.foreign_currency_lines} line(s) are not in {vsActual.functional_currency} and are left out of the totals.
            Re-enter them in {vsActual.functional_currency}.
          </div>
        )}

        {loadingThis ? (
          <div className="space-y-2">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-5 w-full" />)}</div>
        ) : (
          <>
            {vsActual && (
              <StatGroup>
                <Stat label="Total budget" value={`${vsActual.functional_currency} ${(vsActual.total_budgeted / 1e6).toFixed(2)}M`} />
                <Stat label="Total actual" value={`${vsActual.functional_currency} ${(vsActual.total_actual / 1e6).toFixed(2)}M`} />
                <Stat label="Variance" alert={vsActual.total_variance < 0}
                  value={`${vsActual.total_variance >= 0 ? "+" : ""}${vsActual.functional_currency} ${(vsActual.total_variance / 1e6).toFixed(2)}M`}
                  note={vsActual.total_variance < 0 ? "Over budget" : undefined} />
              </StatGroup>
            )}
            <div className="overflow-hidden rounded-lg border border-line">
              <DataTable label={`${b.name} lines`} columns={lineColumns} data={vsActual?.lines ?? []}
                getRowId={(l) => l.line_id} maxHeight="none"
                empty={{ title: "No budget lines yet", description: isDraft ? "Click Add line to start." : undefined, compact: true }} />
            </div>
          </>
        )}
      </div>
    );
  }

  const lineColumns: ColumnDef<BudgetLine>[] = [
    {
      id: "account", header: "Account",
      cell: (c) => {
        const line = c.row.original;
        return (
          <div>
            <p className="font-medium"><span className="font-mono text-[13px]">{line.account_code}</span> · {line.account_name}</p>
            {line.label && <p className="text-xs text-fg-muted">{line.label}</p>}
            {line.foreign_currency && <p className="text-xs font-semibold text-tone-orange">{line.currency} line, not compared</p>}
          </div>
        );
      },
    },
    {
      accessorKey: "period_month", header: "Period",
      cell: (c) => { const m = c.getValue<number | null>(); return <span className="text-fg-muted">{m ? MONTH_NAMES[m] || `Q${m}` : "Annual"}</span>; },
    },
    { accessorKey: "budgeted", header: "Budget", meta: { numeric: true }, cell: (c) => <span className="font-medium">{c.getValue<number>().toLocaleString()}</span> },
    { accessorKey: "actual", header: "Actual", meta: { numeric: true }, cell: (c) => c.getValue<number>().toLocaleString() },
    {
      accessorKey: "variance", header: "Variance", meta: { numeric: true },
      cell: (c) => {
        const line = c.row.original;
        if (line.variance === null) return <span className="text-fg-subtle">-</span>;
        return (
          <span className={cn("font-medium", line.variance < 0 && "text-tone-red")}>
            {line.variance >= 0 ? "+" : ""}{line.variance.toLocaleString()}
            <span className="ml-1 font-normal text-fg-muted">({line.variance_pct}%)</span>
          </span>
        );
      },
    },
    {
      id: "state", header: "Status", meta: { align: "center" },
      cell: (c) => {
        const line = c.row.original;
        if (line.is_over_budget) return <Pill tone="red" icon={AlertTriangle}>Over</Pill>;
        if (line.actual > 0) return <Pill tone="blue" icon={CheckCircle2}>On track</Pill>;
        return <span className="text-fg-subtle">-</span>;
      },
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "48px" },
      cell: (c) => {
        const b = budgets.find((x) => x.id === activeId);
        return canManage && b?.status === "draft" && activeId
          ? <IconButton size="sm" icon={Trash2} label="Remove line" onClick={() => deleteLine(activeId, c.row.original.line_id)} />
          : null;
      },
    },
  ];

  return (
    <div className="animate-fade-up">
      <ListFrame>
        <PageHeader flush title="Budgets" summary={`${budgets.length} budget${budgets.length !== 1 ? "s" : ""}. Plan and track spending by account.`}
          actions={canManage ? <Button variant="primary" onClick={() => { setForm(EMPTY_FORM); setShowCreate(true); }}><Plus aria-hidden /> New budget</Button> : undefined} />
        <SavedViewTabs state={views} currentFilters={currentFilters} onSelect={() => closeDetail()} />
        <FilterChips
          chips={years.map((y) => ({ id: String(y), label: `FY${y}`, active: yearFilter === String(y) }))}
          onToggle={(id) => { setYearFilter((cur) => (cur === id ? "" : id)); closeDetail(); }}
          onClear={() => setYearFilter("")} />
        <DataTable label="Budgets" columns={columns} data={budgets} getRowId={(b) => b.id} loading={loading}
          expandedId={activeId}
          onExpandedChange={(id) => { if (id) { setConfirmDeleteId(null); openDetail(id); } else closeDetail(); }}
          renderDetails={renderDetails}
          empty={{ icon: BarChart3, title: "No budgets in this view", description: "Pick another view, or create a budget." }} />
      </ListFrame>

      <Dialog open={showCreate} onOpenChange={setShowCreate} title="New budget"
        footer={<>
          <Button onClick={() => setShowCreate(false)}>Cancel</Button>
          <Button variant="primary" onClick={createBudget} loading={saving}>{saving ? "Creating…" : "Create budget"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Budget name" required className="md:col-span-2">
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="e.g. Engineering Dept 2026, Q1 Marketing" />
          </Field>
          <Field label="Fiscal year" required>
            <Input type="number" numeric value={form.fiscal_year} onChange={(e) => setForm({ ...form, fiscal_year: e.target.value })} />
          </Field>
          <Field label="Period type">
            <Select value={form.period_type} onChange={(e) => setForm({ ...form, period_type: e.target.value })}
              options={[{ value: "annual", label: "Annual" }, { value: "quarterly", label: "Quarterly" }, { value: "monthly", label: "Monthly" }]} />
          </Field>
          <Field label="Department" className="md:col-span-2">
            <Input value={form.department} onChange={(e) => setForm({ ...form, department: e.target.value })}
              placeholder="e.g. Engineering, Finance, Marketing" />
          </Field>
          <Field label="Notes" className="md:col-span-2">
            <Textarea value={form.notes} onChange={(e) => setForm({ ...form, notes: e.target.value })} rows={2} />
          </Field>
        </FieldGrid>
      </Dialog>

      <Dialog open={showAddLine && !!activeId} onOpenChange={setShowAddLine} title="Add budget line"
        footer={<>
          <Button onClick={() => setShowAddLine(false)}>Cancel</Button>
          <Button variant="primary" onClick={addLine} loading={lineSaving}>{lineSaving ? "Adding…" : "Add line"}</Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Account" required className="md:col-span-2">
            <Select value={lineForm.account_id} onChange={(e) => setLineForm({ ...lineForm, account_id: e.target.value })} placeholder="Select account">
              {["expenses", "revenue", "assets", "liabilities"].map((cat) => {
                const group = accounts.filter((a) => a.category === cat);
                if (!group.length) return null;
                return (
                  <optgroup key={cat} label={cat.charAt(0).toUpperCase() + cat.slice(1)}>
                    {group.map((a) => (
                      <option key={a.id} value={a.id}>{a.code} {a.name}</option>
                    ))}
                  </optgroup>
                );
              })}
            </Select>
          </Field>
          <Field label="Label" className="md:col-span-2">
            <Input value={lineForm.label} onChange={(e) => setLineForm({ ...lineForm, label: e.target.value })}
              placeholder="Optional description for this line" />
          </Field>
          <Field label="Amount" required>
            <Input type="number" min="0" numeric value={lineForm.budgeted_amount}
              onChange={(e) => setLineForm({ ...lineForm, budgeted_amount: e.target.value })} placeholder="0" />
          </Field>
          <Field label="Currency" hint="Budgets are compared with the ledger, which reports in the functional currency.">
            <p className="flex h-11 items-center rounded-lg border border-line bg-surface-2 px-3 text-sm text-fg-muted md:h-9">
              {vsActual?.functional_currency || functional || "Functional currency"}
            </p>
          </Field>
          <Field className="md:col-span-2"
            label={`Period ${vsActual?.period_type === "monthly" ? "(1-12 = Jan-Dec)" : vsActual?.period_type === "quarterly" ? "(1-4 = Q1-Q4)" : "(leave blank for annual)"}`}>
            <Input type="number" min="1" max={vsActual?.period_type === "monthly" ? 12 : 4} numeric
              value={lineForm.period_month} onChange={(e) => setLineForm({ ...lineForm, period_month: e.target.value })}
              placeholder="Leave blank for full-period budget" />
          </Field>
        </FieldGrid>
      </Dialog>
    </div>
  );
}
