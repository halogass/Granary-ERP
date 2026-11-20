"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { hrApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import BankAccountSelect from "@/components/finance/BankAccountSelect";
import JournalLink from "@/components/finance/JournalLink";
import { DollarSign, Plus } from "lucide-react";
import { stack } from "@/lib/stackTable";
import { cn } from "@/lib/utils";
import { formatCurrency } from "@/lib/currency";
import { paidThisYear } from "@/lib/payrollTotals";
import { useCurrencySettings } from "@/hooks/useCurrencySettings";
import {
  Button, DataTable, Dialog, ErrorToast, Field, FieldGrid, Input, ListFrame, PageHeader, Select, Skeleton, Stat, StatGroup,
  StatusPill, type ColumnDef,
} from "@/components/ui";

const MONTH_NAMES = ["","Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];

interface PayrollRun {
  id: string;
  month: number;
  year: number;
  status: string;
  total_gross: number;
  total_income_tax: number;
  total_contributions: number;
  total_net: number;
  currency: string;
  approved_at?: string;
  paid_at?: string;
  created_at?: string;
}

interface PayrollLine {
  id: string;
  employee_id: string;
  employee_name: string;
  gross_salary: number;
  allowances: number;
  overtime_pay: number;
  deductions: number;
  income_tax: number;
  /** Each configured contribution: its label and the employee and employer shares (amounts as text). */
  contributions?: Record<string, { label?: string; employee?: string | number; employer?: string | number }>;
  employee_contributions: number;
  employer_contributions: number;
  net_salary: number;
  days_worked?: number;
  notes?: string;
}

interface PayrollDetail extends PayrollRun {
  lines: PayrollLine[];
}

const fmt = (n: number, currency: string) => formatCurrency(n, currency);

export default function PayrollPage() {
  const [actionError, setActionError] = useState("");
  const { functional } = useCurrencySettings();
  const now = new Date();
  const [runs, setRuns] = useState<PayrollRun[]>([]);
  const [loading, setLoading] = useState(true);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<PayrollDetail | null>(null);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [form, setForm] = useState({ month: now.getMonth() + 1, year: now.getFullYear(), notes: "" });
  const [saving, setSaving] = useState(false);
  const [acting, setActing] = useState<string | null>(null);
  const [payRun, setPayRun] = useState<PayrollRun | null>(null);
  const [payForm, setPayForm] = useState({ paid_date: "", bank_account_id: "" });

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await hrApi.listPayrollRuns();
      setRuns(res.data ?? []);
    } catch { setRuns([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function expand(id: string | null) {
    if (!id || expandedId === id) { setExpandedId(null); setDetail(null); return; }
    setExpandedId(id);
    setDetail(null);
    setLoadingDetail(true);
    try {
      const res = await hrApi.getPayrollRun(id);
      setDetail(res.data);
    } catch (e) { setActionError(extractApiError(e, "Could not load the payroll run.")); }
    finally { setLoadingDetail(false); }
  }

  async function create() {
    setSaving(true);
    try {
      await hrApi.createPayrollRun({ month: form.month, year: form.year, notes: form.notes || undefined });
      setShowCreate(false);
      load();
    } catch (e) {
      // e.g. 409 (a run for that month already exists) or 400 (a payroll setting is missing).
      setActionError(extractApiError(e, "Could not create the payroll run."));
    } finally { setSaving(false); }
  }

  async function approve(id: string) {
    setActing(id);
    setActionError("");
    try { await hrApi.approvePayrollRun(id); load(); window.dispatchEvent(new Event("approvals:changed")); }
    catch (e) { setActionError(extractApiError(e, "Could not approve the payroll run.")); }
    finally { setActing(null); }
  }

  function openMarkPaid(run: PayrollRun) {
    setPayForm({ paid_date: new Date().toISOString().slice(0, 10), bank_account_id: "" });
    setPayRun(run);
  }

  async function markPaid() {
    if (!payRun) return;
    const id = payRun.id;
    setActing(id);
    setActionError("");
    try {
      await hrApi.markPayrollPaid(id, {
        paid_date: payForm.paid_date || undefined,
        bank_account_id: payForm.bank_account_id || undefined,
      });
      setPayRun(null);
      load();
    }
    catch (e) { setActionError(extractApiError(e, "Could not mark the payroll run paid.")); }
    finally { setActing(null); }
  }

  const paid = paidThisYear(runs, now.getFullYear(), functional);
  const drafts = runs.filter(r => r.status === "draft").length;

  const columns = useMemo<ColumnDef<PayrollRun>[]>(() => [
    {
      id: "period", accessorFn: (r) => r.year * 100 + r.month, header: "Period",
      cell: (c) => <span className="font-medium text-fg">{MONTH_NAMES[c.row.original.month]} {c.row.original.year} payroll</span>,
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="payroll" status={c.row.original.status} /> },
    { accessorKey: "total_gross", header: "Gross", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmt(c.row.original.total_gross, c.row.original.currency) },
    { accessorKey: "total_income_tax", header: "Income tax", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmt(c.row.original.total_income_tax, c.row.original.currency) },
    { accessorKey: "total_contributions", header: "Contributions", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmt(c.row.original.total_contributions, c.row.original.currency) },
    {
      accessorKey: "total_net", header: "Net", meta: { numeric: true },
      cell: (c) => <span className="font-medium">{fmt(c.row.original.total_net, c.row.original.currency)}</span>,
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const run = c.row.original;
        return (
          <div className="flex items-center justify-end gap-2 whitespace-nowrap">
            {run.status === "paid" && <JournalLink source="payroll" sourceId={run.id} />}
            {run.status === "draft" && (
              <Button size="sm" variant="primary" loading={acting === run.id} onClick={() => approve(run.id)}>Approve</Button>
            )}
            {run.status === "approved" && (
              <Button size="sm" variant="primary" disabled={acting === run.id} onClick={() => openMarkPaid(run)}>Mark paid</Button>
            )}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [acting]);

  function renderLines(run: PayrollRun) {
    if (loadingDetail || !detail || detail.id !== run.id) {
      return <div className="space-y-2"><Skeleton className="h-6 w-full" /><Skeleton className="h-6 w-full" /><Skeleton className="h-6 w-2/3" /></div>;
    }
    if (!detail.lines?.length) return <p className="text-[13px] text-fg-muted">No payroll lines found.</p>;
    return (
      <div className="overflow-x-auto rounded-lg border border-line bg-surface max-md:overflow-visible max-md:border-0 max-md:bg-transparent">
        <table className={cn("w-full text-[13px]", stack.table)}>
          <thead className={cn("bg-surface-2 text-xs text-fg-muted", stack.thead)}>
            <tr>
              <th scope="col" className="px-3 py-2 text-left font-medium">Employee</th>
              <th scope="col" className="px-3 py-2 text-right font-medium">Gross</th>
              <th scope="col" className="px-3 py-2 text-right font-medium">Income tax</th>
              <th scope="col" className="px-3 py-2 text-right font-medium">Contributions</th>
              <th scope="col" className="px-3 py-2 text-right font-medium">Net salary</th>
            </tr>
          </thead>
          <tbody className={cn("tabular", stack.tbody, "max-md:p-0")}>
            {detail.lines.map(line => {
              const grossNote = `Base ${fmt(line.gross_salary - (line.allowances || 0) - (line.overtime_pay || 0), run.currency)}`
                + `, allowances ${fmt(line.allowances || 0, run.currency)}, overtime ${fmt(line.overtime_pay || 0, run.currency)}`
                + (line.days_worked != null ? `, ${line.days_worked} day(s) employed` : "");
              const parts = Object.values(line.contributions ?? {});
              const contribNote = parts.length === 0 ? "No contributions configured" :
                `Employee: ${parts.map((c) => `${c.label} ${fmt(Number(c.employee ?? 0), run.currency)}`).join(", ")}\n`
                + `Employer (company cost): ${parts.map((c) => `${c.label} ${fmt(Number(c.employer ?? 0), run.currency)}`).join(", ")}`;
              return (
                <tr key={line.id} className={cn("border-t border-line-soft", stack.tr)}>
                  <td className={cn("px-3 py-2 font-medium text-fg", stack.title)}>{line.employee_name}</td>
                  <td data-label="Gross" className={cn("px-3 py-2 text-right text-fg", stack.td)} title={grossNote}>
                    {fmt(line.gross_salary, run.currency)}
                    {/* Phones have no hover: the breakdown is shown under the amount. */}
                    <span className="block whitespace-pre-line text-[11px] text-fg-muted md:hidden">{grossNote}</span>
                  </td>
                  <td data-label="Income tax" className={cn("px-3 py-2 text-right text-fg-muted", stack.td)}>-{fmt(line.income_tax, run.currency)}</td>
                  <td data-label="Contributions" className={cn("px-3 py-2 text-right text-fg-muted", stack.td)} title={contribNote}>
                    -{fmt(line.employee_contributions, run.currency)}
                    <span className="block whitespace-pre-line text-[11px] md:hidden">{contribNote}</span>
                  </td>
                  <td data-label="Net salary" className={cn("px-3 py-2 text-right font-semibold text-fg", stack.td)}>{fmt(line.net_salary, run.currency)}</td>
                </tr>
              );
            })}
            <tr className={cn("border-t-2 border-line bg-surface-2 font-semibold text-fg", stack.tr, "max-md:bg-surface-2 max-md:even:bg-surface-2")}>
              <td className={cn("px-3 py-2", stack.title)}>Total</td>
              <td data-label="Gross" className={cn("px-3 py-2 text-right", stack.td)}>{fmt(run.total_gross, run.currency)}</td>
              <td data-label="Income tax" className={cn("px-3 py-2 text-right", stack.td)}>-{fmt(run.total_income_tax, run.currency)}</td>
              <td data-label="Contributions" className={cn("px-3 py-2 text-right", stack.td)}>-{fmt(run.total_contributions, run.currency)}</td>
              <td data-label="Net salary" className={cn("px-3 py-2 text-right", stack.td)}>{fmt(run.total_net, run.currency)}</td>
            </tr>
          </tbody>
        </table>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total runs" value={runs.length} />
        <Stat label="Paid this year" value={fmt(paid.total, paid.currency)}
          note={paid.mixed ? "net salaries, largest currency only" : "net salaries"} />
        <Stat label="Pending approval" value={drafts} alert={drafts > 0} note={drafts > 0 ? "draft runs to approve" : undefined} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Payroll" summary="Monthly payroll runs with income tax and contributions calculated from your settings"
          actions={<Button variant="primary" onClick={() => setShowCreate(true)}><Plus aria-hidden /> New payroll run</Button>} />
        <DataTable label="Payroll runs" columns={columns} data={runs} getRowId={(r) => r.id} loading={loading}
          expandedId={expandedId} onExpandedChange={expand}
          empty={{
            icon: DollarSign, title: "No payroll runs yet", description: "Create the first payroll run to calculate salaries.",
            action: <Button variant="primary" onClick={() => setShowCreate(true)}>New payroll run</Button>,
          }}
          renderDetails={renderLines} />
      </ListFrame>

      <Dialog open={showCreate} onOpenChange={setShowCreate} title="New payroll run"
        description="Salaries, income tax and contributions are calculated for all active employees from their base salary."
        footer={<>
          <Button onClick={() => setShowCreate(false)}>Cancel</Button>
          <Button variant="primary" onClick={create} loading={saving}>{saving ? "Calculating..." : "Create and calculate"}</Button>
        </>}>
        <div className="space-y-4">
          <FieldGrid cols={2}>
            <Field label="Month">
              <Select value={form.month} onChange={e => setForm(f => ({ ...f, month: Number(e.target.value) }))}
                options={MONTH_NAMES.slice(1).map((m, i) => ({ value: String(i + 1), label: m }))} />
            </Field>
            <Field label="Year">
              <Select value={form.year} onChange={e => setForm(f => ({ ...f, year: Number(e.target.value) }))}
                options={[now.getFullYear() - 1, now.getFullYear()].map(y => ({ value: String(y), label: String(y) }))} />
            </Field>
          </FieldGrid>
          <Field label="Notes">
            <Input placeholder="Optional notes..." value={form.notes} onChange={e => setForm(f => ({ ...f, notes: e.target.value }))} />
          </Field>
          <p className="text-xs leading-relaxed text-fg-muted">
            Income tax is withheld on the cumulative method: each month the year so far is annualised,
            taxed by your brackets after the employee&apos;s allowance, and what was already withheld is
            deducted, so the last month settles the year. Contributions and tax brackets come from
            Settings &gt; HR &amp; Attendance &gt; Payroll. Joiners and leavers are prorated; overtime and
            allowances are included. Salaries in another currency are converted at the month&apos;s
            exchange rate.
          </p>
        </div>
      </Dialog>

      <Dialog open={!!payRun} onOpenChange={(o) => { if (!o) setPayRun(null); }} title="Mark payroll paid"
        description={payRun ? `${MONTH_NAMES[payRun.month]} ${payRun.year}, net ${fmt(payRun.total_net, payRun.currency)}` : undefined}
        footer={payRun && <>
          <Button onClick={() => setPayRun(null)}>Cancel</Button>
          <Button variant="primary" onClick={markPaid} loading={acting === payRun.id}>{acting === payRun.id ? "Saving..." : "Mark paid"}</Button>
        </>}>
        {payRun && (
          <div className="space-y-4">
            <Field label="Paid from">
              <BankAccountSelect value={payForm.bank_account_id} currency={payRun.currency}
                onChange={v => setPayForm(f => ({ ...f, bank_account_id: v }))} />
            </Field>
            <Field label="Paid date">
              <Input type="date" value={payForm.paid_date} onChange={e => setPayForm(f => ({ ...f, paid_date: e.target.value }))} />
            </Field>
          </div>
        )}
      </Dialog>
      <ErrorToast message={actionError} onClose={() => setActionError("")} />
    </div>
  );
}
