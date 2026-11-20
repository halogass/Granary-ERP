"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useState, useCallback, useRef, useMemo } from "react";
import { useSession } from "next-auth/react";
import { authApi, financeApi, systemApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import BankAccountSelect from "@/components/finance/BankAccountSelect";
import JournalLink from "@/components/finance/JournalLink";
import {
  Plus, Search, CheckCircle2, XCircle, Wallet, Trash2, MessageSquare, RefreshCw,
} from "lucide-react";
import { formatCurrency, getCurrencySymbol, CURRENCY_META } from "@/lib/currency";
import {
  Button, Checkbox, DataTable, Dialog, ErrorToast, Field, FieldGrid, FilterChips, IconButton, Input, KeyValues,
  ListFrame, PageHeader, Pill, SavedViewTabs, Select, Stat, StatGroup, StatusPill, Textarea, useSavedViews,
  type ColumnDef, type SavedView,
} from "@/components/ui";

interface Expense {
  id: string;
  description?: string;
  amount: number;
  currency?: string;
  exchange_rate?: number;
  amount_functional?: number;
  category: string;
  status: string;
  expense_date: string;
  submitted_by_id?: string;
  submitted_by_name?: string;
  employee_name?: string;
  rejection_reason?: string;
  can_review?: boolean;
}

type Filters = { status?: string; mine?: boolean; review?: boolean };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "pending", label: "Pending", filters: { status: "pending" } },
  { id: "approved", label: "Approved", filters: { status: "approved" } },
  { id: "reimbursed", label: "Reimbursed", filters: { status: "reimbursed" } },
  { id: "rejected", label: "Rejected", filters: { status: "rejected" } },
];

const CATEGORIES = [
  "travel", "meals", "office_supplies", "software", "hardware",
  "marketing", "utilities", "professional_services", "other",
];

const CATEGORY_LABELS: Record<string, string> = {
  travel: "Travel", meals: "Meals", office_supplies: "Office Supplies",
  software: "Software", hardware: "Hardware", marketing: "Marketing",
  utilities: "Utilities", professional_services: "Professional Services", other: "Other",
};

const fmtK = (n: number) => {
  if (n >= 1e9) return `${(n/1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n/1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n/1e3).toFixed(0)}K`;
  return `${n}`;
};

export default function ExpensesPage() {
  const { data: session } = useSession();
  const myId = (session?.user as { id?: string })?.id ?? "";
  // Mirrors the backend exactly: finance:confirm_payment or superadmin. usePermissions().can() also
  // lets system admins through, who would get a 403 from /reimburse.
  const [canReimburse, setCanReimburse] = useState(false);
  useEffect(() => {
    authApi.me().then((r) => {
      const me = r.data as { is_superadmin?: boolean; permissions?: string[] };
      setCanReimburse(!!me?.is_superadmin || !!me?.permissions?.includes("finance:confirm_payment"));
    }).catch(() => setCanReimburse(false));
  }, []);

  const views = useSavedViews<Filters>("finance.expenses", VIEWS);
  const [chips, setChips] = useState<{ mine?: boolean; review?: boolean }>({});
  const [expenses, setExpenses] = useState<Expense[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [activeCurrencies, setActiveCurrencies] = useState<string[]>(["USD", "EUR"]);
  const [functionalCurrency, setFunctionalCurrency] = useState("USD");
  const currencyInitialized = useRef(false);

  // A view's own chips (saved views may carry them) apply when it is chosen or restored.
  useEffect(() => {
    const f = views.active?.filters ?? {};
    setChips({ mine: !!f.mine, review: !!f.review });
  }, [views.activeId]); // eslint-disable-line react-hooks/exhaustive-deps

  const statusFilter = views.active?.filters.status ?? "";
  const filters: Filters = {
    ...(statusFilter ? { status: statusFilter } : {}),
    ...(chips.mine ? { mine: true } : {}),
    ...(chips.review ? { review: true } : {}),
  };

  // Submit modal
  const [showModal, setShowModal] = useState(false);
  const [form, setForm] = useState({
    description: "", amount: "", category: "other",
    expense_date: new Date().toISOString().slice(0, 10),
    currency: "USD", exchange_rate: "", pre_approved: false,
  });
  const [fetchingRate, setFetchingRate] = useState(false);
  const [saving, setSaving] = useState(false);

  // Reject reason modal
  const [rejectTarget, setRejectTarget] = useState<Expense | null>(null);
  const [rejectReason, setRejectReason] = useState("");
  const [rejecting, setRejecting] = useState(false);
  const [actionError, setActionError] = useState("");

  // Mark reimbursed modal
  const [reimburseTarget, setReimburseTarget] = useState<Expense | null>(null);
  const [reimburseForm, setReimburseForm] = useState({ paid_date: "", bank_account_id: "" });
  const [reimbursing, setReimbursing] = useState(false);

  // Delete confirm
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [expRes, currRes] = await Promise.all([
        fetchAllPages<Expense>(financeApi.listExpenses),
        systemApi.getCurrencySettings().catch(() => null),
      ]);
      setExpenses(expRes.rows);
      if (currRes?.data) {
        const active: string[] = currRes.data.active ?? ["USD", "EUR"];
        const functional: string = currRes.data.functional ?? "USD";
        setActiveCurrencies(active);
        setFunctionalCurrency(functional);
        if (!currencyInitialized.current) {
          setForm(f => ({ ...f, currency: functional }));
          currencyInitialized.current = true;
        }
      }
    } catch { setExpenses([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const fetchRate = useCallback(async (currency: string, expDate: string, funcCur: string) => {
    if (currency === funcCur) { setForm(f => ({ ...f, exchange_rate: "" })); return; }
    setFetchingRate(true);
    try {
      const res = await financeApi.getSpotRate(currency, funcCur, expDate);
      setForm(f => ({ ...f, exchange_rate: res.data?.rate ? String(res.data.rate) : "" }));
    } catch {
      setForm(f => ({ ...f, exchange_rate: "" }));
    } finally { setFetchingRate(false); }
  }, []);

  // Auto-fetch rate when currency/date changes in modal
  useEffect(() => {
    if (showModal && form.currency !== functionalCurrency) {
      fetchRate(form.currency, form.expense_date, functionalCurrency);
    }
  }, [form.currency, form.expense_date, functionalCurrency, fetchRate, showModal]);

  const filtered = expenses.filter((e) => {
    const name = e.employee_name ?? e.submitted_by_name ?? "";
    const matchSearch = !search ||
      (e.description ?? "").toLowerCase().includes(search.toLowerCase()) ||
      name.toLowerCase().includes(search.toLowerCase());
    const matchStatus = !statusFilter || e.status === statusFilter;
    const matchMine = !chips.mine || e.submitted_by_id === myId;
    const matchReview = !chips.review || (!!e.can_review && e.status === "pending");
    return matchSearch && matchStatus && matchMine && matchReview;
  });

  // Approved includes reimbursed: an expense paid back is still an approved one.
  const totalApproved = expenses.filter((e) => e.status === "approved" || e.status === "reimbursed")
    .reduce((s, e) => s + (e.amount_functional ?? e.amount), 0);
  const totalPending  = expenses.filter((e) => e.status === "pending")
    .reduce((s, e) => s + (e.amount_functional ?? e.amount), 0);
  const pendingCount  = expenses.filter((e) => e.status === "pending").length;
  const reviewCount   = expenses.filter((e) => e.can_review && e.status === "pending").length;

  const isForeign = form.currency !== functionalCurrency;
  const rateNum = parseFloat(form.exchange_rate) || 0;
  const amountNum = parseFloat(form.amount) || 0;

  async function submitExpense() {
    if (!form.description || !form.amount) return;
    setSaving(true);
    try {
      await financeApi.submitExpense({
        description: form.description,
        amount: amountNum,
        category: form.category,
        expense_date: form.expense_date,
        currency: form.currency,
        exchange_rate: isForeign && form.exchange_rate ? rateNum : undefined,
        pre_approved: form.pre_approved,
      });
      setShowModal(false);
      setForm({ description: "", amount: "", category: "other",
        expense_date: new Date().toISOString().slice(0, 10),
        currency: functionalCurrency, exchange_rate: "", pre_approved: false });
      await load();
    } catch (e) {
      // e.g. no exchange rate on record for a foreign-currency expense
      setActionError(extractApiError(e, "Could not submit the expense."));
    } finally { setSaving(false); }
  }

  async function approveExpense(id: string) {
    setActionError("");
    try {
      await financeApi.reviewExpense(id, { approved: true });
      window.dispatchEvent(new Event("approvals:changed"));
    } catch (e) {
      setActionError(extractApiError(e, "Could not approve the expense."));
    }
    await load();
  }

  async function confirmReject() {
    if (!rejectTarget) return;
    setRejecting(true);
    try {
      await financeApi.reviewExpense(rejectTarget.id, { approved: false, reason: rejectReason });
      setRejectTarget(null);
      setRejectReason("");
      window.dispatchEvent(new Event("approvals:changed"));
      await load();
    } catch (e) {
      setActionError(extractApiError(e, "Could not reject the expense."));
      setRejectTarget(null);
    } finally { setRejecting(false); }
  }

  function openReimburse(exp: Expense) {
    setReimburseForm({ paid_date: new Date().toISOString().slice(0, 10), bank_account_id: "" });
    setReimburseTarget(exp);
  }

  async function confirmReimburse() {
    if (!reimburseTarget) return;
    setReimbursing(true);
    try {
      await financeApi.reimburseExpense(reimburseTarget.id, {
        paid_date: reimburseForm.paid_date || undefined,
        bank_account_id: reimburseForm.bank_account_id || undefined,
      });
      setReimburseTarget(null);
      await load();
    } catch (e) {
      setActionError(extractApiError(e, "Could not mark the expense reimbursed."));
    } finally { setReimbursing(false); }
  }

  async function deleteExpense(id: string) {
    setDeleting(true);
    try {
      await financeApi.deleteExpense(id);
      setConfirmDeleteId(null);
      setExpenses((prev) => prev.filter((e) => e.id !== id));
    } catch { /* keep */ } finally { setDeleting(false); }
  }

  function rowActions(exp: Expense) {
    const isOwn = exp.submitted_by_id === myId;
    const canReview = exp.can_review && exp.status === "pending";
    if (confirmDeleteId === exp.id) {
      return (
        <div className="flex items-center justify-end gap-1.5">
          <span className="text-xs font-semibold text-tone-red">Delete?</span>
          <Button size="sm" variant="danger" onClick={() => deleteExpense(exp.id)} loading={deleting}>Yes</Button>
          <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
        </div>
      );
    }
    return (
      <div className="flex justify-end gap-1">
        {canReview && (
          <>
            <IconButton size="sm" icon={CheckCircle2} label="Approve" onClick={() => approveExpense(exp.id)} />
            <IconButton size="sm" icon={XCircle} label="Reject" onClick={() => { setRejectTarget(exp); setRejectReason(""); }} />
          </>
        )}
        {canReimburse && exp.status === "approved" && (
          <IconButton size="sm" icon={Wallet} label="Mark reimbursed" onClick={() => openReimburse(exp)} />
        )}
        {isOwn && ["pending", "rejected"].includes(exp.status) && (
          <IconButton size="sm" icon={Trash2} label="Delete" onClick={() => setConfirmDeleteId(exp.id)} />
        )}
      </div>
    );
  }

  const columns = useMemo<ColumnDef<Expense>[]>(() => [
    {
      accessorKey: "expense_date", header: "Date", meta: { width: "90px" },
      cell: (c) => <span className="tabular text-fg-muted">{new Date(c.getValue<string>()).toLocaleDateString("en-GB", { day: "numeric", month: "short" })}</span>,
    },
    {
      accessorKey: "description", header: "Description",
      cell: (c) => <span className="block max-w-[260px] truncate font-medium">{c.row.original.description ?? "-"}</span>,
    },
    {
      accessorKey: "category", header: "Category", meta: { hideOnMobile: true },
      cell: (c) => <Pill>{CATEGORY_LABELS[c.getValue<string>()] ?? c.getValue<string>()}</Pill>,
    },
    {
      id: "employee", header: "Employee", meta: { hideOnMobile: true },
      accessorFn: (e) => e.employee_name ?? e.submitted_by_name ?? "",
      cell: (c) => {
        const exp = c.row.original;
        const isOwn = exp.submitted_by_id === myId;
        return (
          <span className={isOwn ? "font-medium" : "text-fg-muted"}>
            {exp.employee_name ?? exp.submitted_by_name ?? "-"}
            {isOwn && <span className="ml-1 text-xs font-normal text-fg-muted">(you)</span>}
          </span>
        );
      },
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="expense" status={c.getValue<string>()} /> },
    {
      accessorKey: "amount", header: "Amount", meta: { numeric: true },
      cell: (c) => {
        const exp = c.row.original;
        const expCurrency = exp.currency ?? functionalCurrency;
        return (
          <div>
            <span className="font-medium">{formatCurrency(exp.amount, expCurrency)}</span>
            {expCurrency !== functionalCurrency && exp.amount_functional && (
              <div className="text-xs text-fg-muted">≈ {formatCurrency(exp.amount_functional, functionalCurrency)}</div>
            )}
          </div>
        );
      },
    },
    { id: "actions", header: "", enableSorting: false, meta: { align: "right" }, cell: (c) => rowActions(c.row.original) },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [myId, functionalCurrency, canReimburse, confirmDeleteId, deleting]);

  const filteredApproved = filtered.filter(e => e.status === "approved" || e.status === "reimbursed")
    .reduce((s, e) => s + (e.amount_functional ?? e.amount), 0);
  const anyFilter = !!(search || statusFilter || chips.mine || chips.review);

  return (
    <div className="animate-fade-up">
      <ListFrame>
        <PageHeader flush title="Expenses" summary={`${filtered.length} expense${filtered.length !== 1 ? "s" : ""}. Submit and track expense reimbursements.`}
          actions={<Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Submit expense</Button>}>
          <StatGroup className="mt-3">
            <Stat label="Approved" value={`${functionalCurrency} ${fmtK(totalApproved)}`} />
            <Stat label="Pending review" value={`${functionalCurrency} ${fmtK(totalPending)}`} />
            <Stat label="Pending count" value={pendingCount.toString()} alert={reviewCount > 0}
              note={reviewCount > 0 ? `${reviewCount} waiting for you` : undefined} />
            <Stat label="Total" value={expenses.length.toString()} />
          </StatGroup>
        </PageHeader>
        <SavedViewTabs state={views} currentFilters={filters} />
        <FilterChips
          chips={[
            { id: "mine", label: "Mine", active: !!chips.mine },
            { id: "review", label: "Needs my review", active: !!chips.review, count: reviewCount },
          ]}
          onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id as keyof typeof c] }))}
          onClear={() => setChips({})}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search expenses" aria-label="Search expenses" className="w-56" />
        </FilterChips>
        <DataTable label="Expenses" columns={columns} data={filtered} getRowId={(e) => e.id} loading={loading}
          empty={{
            icon: Wallet,
            title: anyFilter ? "No matching expenses" : "No expenses submitted",
            description: anyFilter ? "Try another view or clear the filters." : "Submit your first expense for reimbursement.",
            action: !anyFilter ? <Button variant="primary" onClick={() => setShowModal(true)}><Plus aria-hidden /> Submit expense</Button> : undefined,
          }}
          renderDetails={(exp) => {
            const expCurrency = exp.currency ?? functionalCurrency;
            return (
              <div className="flex flex-wrap items-start gap-6">
                <KeyValues className="min-w-[260px] flex-1" items={[
                  { label: "Description", value: exp.description ?? "-" },
                  { label: "Employee", value: exp.employee_name ?? exp.submitted_by_name ?? "-" },
                  { label: "Date", value: new Date(exp.expense_date).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }), numeric: true },
                  { label: "Category", value: CATEGORY_LABELS[exp.category] ?? exp.category },
                  { label: "Amount", value: formatCurrency(exp.amount, expCurrency), numeric: true },
                  ...(expCurrency !== functionalCurrency && exp.amount_functional
                    ? [{ label: `In ${functionalCurrency}`, value: formatCurrency(exp.amount_functional, functionalCurrency), numeric: true }]
                    : []),
                ]} />
                <div className="flex min-w-[200px] flex-col gap-2">
                  {["approved", "reimbursed"].includes(exp.status) && (
                    <JournalLink source="expense" extraSources={["expense_reimbursement"]} sourceId={exp.id} refreshKey={exp.status} />
                  )}
                  {exp.status === "rejected" && exp.rejection_reason && (
                    <p className="flex items-start gap-1.5 text-[13px] text-tone-red">
                      <MessageSquare className="mt-0.5 size-3.5 shrink-0" aria-hidden />
                      <span>{exp.rejection_reason}</span>
                    </p>
                  )}
                </div>
              </div>
            );
          }}
          footer={
            <div className="flex w-full items-center justify-between">
              <span className="tabular">{filtered.length} expense{filtered.length !== 1 ? "s" : ""}</span>
              <span>Approved: <strong className="font-semibold text-fg tabular">{formatCurrency(filteredApproved, functionalCurrency)}</strong></span>
            </div>
          } />
      </ListFrame>

      {/* Submit Expense */}
      <Dialog open={showModal} onOpenChange={setShowModal} title="Submit expense"
        footer={<>
          <Button onClick={() => setShowModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={submitExpense} disabled={!form.description || !form.amount} loading={saving}>
            {saving ? "Submitting…" : "Submit expense"}
          </Button>
        </>}>
        <FieldGrid cols={2}>
          <Field label="Description" required className="md:col-span-2">
            <Input value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })}
              placeholder="e.g. Taxi to client site" />
          </Field>
          <Field label={`Amount (${getCurrencySymbol(form.currency)})`} required>
            <Input type="number" min="0" numeric value={form.amount} onChange={(e) => setForm({ ...form, amount: e.target.value })} placeholder="0" />
          </Field>
          <Field label="Date" required>
            <Input type="date" value={form.expense_date} onChange={(e) => setForm({ ...form, expense_date: e.target.value })} />
          </Field>
          <Field label="Currency" className="md:col-span-2">
            <Select value={form.currency} className="font-mono"
              onChange={(e) => setForm(f => ({ ...f, currency: e.target.value, exchange_rate: "" }))}>
              {activeCurrencies.map(c => (
                <option key={c} value={c}>{c}: {CURRENCY_META[c as keyof typeof CURRENCY_META]?.name ?? c}</option>
              ))}
            </Select>
          </Field>
          {/* Exchange rate, foreign currency only */}
          {isForeign && (
            <Field label={`Rate (1 ${form.currency} = ? ${functionalCurrency})`} className="md:col-span-2"
              hint={amountNum > 0 && rateNum > 0 ? `≈ ${formatCurrency(amountNum * rateNum, functionalCurrency)}` : undefined}>
              <div className="flex items-center gap-2">
                <Input type="number" step="0.000001" min="0" numeric className="flex-1"
                  value={form.exchange_rate}
                  onChange={(e) => setForm(f => ({ ...f, exchange_rate: e.target.value }))}
                  placeholder="e.g. 15900" />
                <IconButton variant="secondary" icon={RefreshCw} label="Fetch the rate" disabled={fetchingRate}
                  className={fetchingRate ? "[&_svg]:animate-spin" : undefined}
                  onClick={() => fetchRate(form.currency, form.expense_date, functionalCurrency)} />
              </div>
            </Field>
          )}
          <Field label="Category" className="md:col-span-2">
            <Select value={form.category} onChange={(e) => setForm({ ...form, category: e.target.value })}
              options={CATEGORIES.map((c) => ({ value: c, label: CATEGORY_LABELS[c] }))} />
          </Field>
        </FieldGrid>
        <Checkbox className="mt-4 text-xs text-fg-muted" checked={form.pre_approved}
          onChange={(e) => setForm({ ...form, pre_approved: e.target.checked })}
          label={<>My manager pre-approved this expense before I incurred it (needed above the pre-approval limit in the expense policy).
            It then goes to the approval chain: your manager, Finance Admin, and the Owner for larger claims.</>} />
      </Dialog>

      {/* Reject reason */}
      <Dialog open={!!rejectTarget} onOpenChange={(o) => { if (!o) { setRejectTarget(null); setRejectReason(""); } }}
        title="Reject expense"
        description={rejectTarget && (
          <>
            Rejecting <strong>{rejectTarget.description ?? "this expense"}</strong>,{" "}
            {formatCurrency(rejectTarget.amount, rejectTarget.currency ?? functionalCurrency)}
            {(rejectTarget.employee_name ?? rejectTarget.submitted_by_name) && (
              <> by <strong>{rejectTarget.employee_name ?? rejectTarget.submitted_by_name}</strong></>
            )}
          </>
        )}
        footer={<>
          <Button onClick={() => { setRejectTarget(null); setRejectReason(""); }}>Cancel</Button>
          <Button variant="danger" onClick={confirmReject} disabled={!rejectReason.trim()} loading={rejecting}>
            {rejecting ? "Rejecting…" : "Reject expense"}
          </Button>
        </>}>
        <Field label="Reason for rejection" required>
          <Textarea value={rejectReason} onChange={(e) => setRejectReason(e.target.value)} rows={3}
            placeholder="Explain why this expense is being rejected" />
        </Field>
      </Dialog>

      {/* Mark reimbursed */}
      <Dialog open={!!reimburseTarget} onOpenChange={(o) => { if (!o) setReimburseTarget(null); }}
        title="Mark reimbursed"
        description={reimburseTarget && (
          <>
            Paying back <strong>{reimburseTarget.description ?? "this expense"}</strong>,{" "}
            {formatCurrency(reimburseTarget.amount, reimburseTarget.currency ?? functionalCurrency)}
            {(reimburseTarget.employee_name ?? reimburseTarget.submitted_by_name) && (
              <> to <strong>{reimburseTarget.employee_name ?? reimburseTarget.submitted_by_name}</strong></>
            )}
          </>
        )}
        footer={<>
          <Button onClick={() => setReimburseTarget(null)}>Cancel</Button>
          <Button variant="primary" onClick={confirmReimburse} loading={reimbursing}>{reimbursing ? "Saving…" : "Mark reimbursed"}</Button>
        </>}>
        {reimburseTarget && (
          <FieldGrid cols={1}>
            <Field label="Paid from">
              <BankAccountSelect value={reimburseForm.bank_account_id}
                currency={reimburseTarget.currency ?? functionalCurrency}
                onChange={(v) => setReimburseForm({ ...reimburseForm, bank_account_id: v })} />
            </Field>
            <Field label="Paid date">
              <Input type="date" value={reimburseForm.paid_date}
                onChange={(e) => setReimburseForm({ ...reimburseForm, paid_date: e.target.value })} />
            </Field>
          </FieldGrid>
        )}
      </Dialog>
      <ErrorToast message={actionError} onClose={() => setActionError("")} />
    </div>
  );
}
