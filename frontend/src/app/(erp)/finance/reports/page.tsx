"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { financeApi, systemApi } from "@/lib/api";
import { BarChart3, TrendingUp, AlertCircle, Scale, BookOpen, List, AlertTriangle } from "lucide-react";
import {
  Button, Card, CardHeader, Checkbox, DataTable, EmptyState, Field, Input, PageHeader, Pill, Select, Skeleton,
  Stat, StatGroup, Tabs, TabsList, TabsTrigger, type ColumnDef,
} from "@/components/ui";
import { cn } from "@/lib/utils";

type Tab = "pl" | "balance_sheet" | "trial_balance" | "aged_ar" | "gl";

interface Account { id: string; code: string; name: string; category: string; }

// Minor-unit digits per currency from the currency.decimals setting (GET /system/currencies), the
// same table the backend rounds with. Filled once on load; until then Intl uses the currency's own
// convention rather than a forced 0.
let currencyDecimals: Record<string, number> = {};
let functionalCode = "";

const digits = (cur: string) => {
  const d = currencyDecimals[cur.toUpperCase()];
  return d === undefined ? {} : { minimumFractionDigits: d, maximumFractionDigits: d };
};

// Report totals are in the functional currency the backend returns; USD only as a fallback.
const fmt = (n: number, cur?: string) => {
  const code = (cur || functionalCode || "USD").toUpperCase();
  return new Intl.NumberFormat("en-US", { style: "currency", currency: code, ...digits(code) }).format(n);
};

// Trial balance / ledger columns: plain numbers in the functional currency.
const fmtNum = (n: number) =>
  new Intl.NumberFormat("en-US", digits(functionalCode || "USD")).format(n);

type ArItem = { id: string; number: string; customer_name: string; currency: string; amount_due: number; due_date: string | null; days_overdue: number };
type TbLine = { code: string; name: string; category: string; total_debit: number; total_credit: number; balance: number };
type GlLine = { date: string; entry_number: string; narration: string; description: string | null; debit: number; credit: number; balance: number };

export default function ReportsPage() {
  const [tab, setTab] = useState<Tab>("pl");
  const [year, setYear] = useState(new Date().getFullYear());
  const [dateFrom, setDateFrom] = useState(`${new Date().getFullYear()}-01-01`);
  const [dateTo, setDateTo] = useState(new Date().toISOString().slice(0, 10));
  const [asOf, setAsOf] = useState(new Date().toISOString().slice(0, 10));
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [selectedAccount, setSelectedAccount] = useState<string>("");

  // Report data
  const [pl, setPl] = useState<{
    revenue: { code: string; name: string; amount: number }[];
    expenses: { code: string; name: string; amount: number }[];
    total_revenue: number; total_expenses: number; net_income: number;
    date_from?: string; date_to?: string; functional_currency?: string;
  } | null>(null);

  const [bs, setBs] = useState<{
    assets: { code: string; name: string; balance: number }[];
    liabilities: { code: string; name: string; balance: number }[];
    equity: { code: string | null; name: string; balance: number; computed?: boolean }[];
    total_assets: number; total_liabilities: number; total_equity: number;
    balanced: boolean; as_of: string; functional_currency?: string;
  } | null>(null);

  const [tb, setTb] = useState<{
    lines: TbLine[];
    total_debit: number; total_credit: number; balanced: boolean;
  } | null>(null);

  const [ar, setAr] = useState<{
    current: { count: number; total: number; items: ArItem[] };
    "1_30_days": { count: number; total: number; items: unknown[] };
    "31_60_days": { count: number; total: number; items: unknown[] };
    "61_90_days": { count: number; total: number; items: unknown[] };
    over_90_days: { count: number; total: number; items: unknown[] };
    total_outstanding: number;
    functional_currency?: string;
  } | null>(null);

  const [gl, setGl] = useState<{
    account: Account;
    lines: GlLine[];
    closing_balance: number;
    functional_currency?: string;
  } | null>(null);

  const [loading, setLoading] = useState(false);
  // AR aging is commercial invoices (what the ledger's AR holds); proformas are payment requests.
  const [includeProforma, setIncludeProforma] = useState(false);

  const [, setCurrencyLoaded] = useState(false);

  useEffect(() => {
    financeApi.listAccounts().then(r => setAccounts(r.data ?? []));
    systemApi.getCurrencySettings().then((r) => {
      currencyDecimals = r.data?.decimals ?? {};
      functionalCode = r.data?.functional ?? "";
      setCurrencyLoaded(true);   // re-render with the configured decimals
    }).catch(() => { /* Intl's own per-currency defaults stay */ });
  }, []);

  const loadReport = useCallback(async () => {
    setLoading(true);
    try {
      if (tab === "pl") {
        const res = await financeApi.getProfitAndLoss({ date_from: dateFrom, date_to: dateTo });
        setPl(res.data);
      } else if (tab === "balance_sheet") {
        const res = await financeApi.getBalanceSheet({ as_of: asOf });
        setBs(res.data);
      } else if (tab === "trial_balance") {
        const res = await financeApi.getTrialBalance({ date_from: dateFrom, date_to: dateTo });
        setTb(res.data);
      } else if (tab === "aged_ar") {
        const res = await financeApi.getAgedReceivables({ as_of: asOf, include_proforma: includeProforma });
        setAr(res.data);
      } else if (tab === "gl" && selectedAccount) {
        const res = await financeApi.getGeneralLedger(selectedAccount, { date_from: dateFrom, date_to: dateTo });
        setGl(res.data);
      }
    } catch { /* silent */ } finally { setLoading(false); }
  }, [tab, dateFrom, dateTo, asOf, selectedAccount, includeProforma]);

  useEffect(() => { loadReport(); }, [loadReport]);

  const tabs = [
    { key: "pl",            label: "P&L",            icon: TrendingUp },
    { key: "balance_sheet", label: "Balance sheet",  icon: Scale },
    { key: "trial_balance", label: "Trial balance",  icon: List },
    { key: "aged_ar",       label: "AR aging",       icon: AlertCircle },
    { key: "gl",            label: "General ledger", icon: BookOpen },
  ];

  const tbColumns = useMemo<ColumnDef<TbLine>[]>(() => [
    { accessorKey: "code", header: "Code", meta: { width: "90px" }, cell: (c) => <span className="font-mono text-[13px] text-fg-muted">{c.getValue<string>()}</span> },
    { accessorKey: "name", header: "Account" },
    { accessorKey: "category", header: "Category", meta: { hideOnMobile: true }, cell: (c) => <span className="text-xs capitalize text-fg-muted">{c.getValue<string>()}</span> },
    { accessorKey: "total_debit", header: "Debit", meta: { numeric: true }, cell: (c) => c.getValue<number>() > 0 ? fmtNum(c.getValue<number>()) : "" },
    { accessorKey: "total_credit", header: "Credit", meta: { numeric: true }, cell: (c) => c.getValue<number>() > 0 ? fmtNum(c.getValue<number>()) : "" },
    {
      accessorKey: "balance", header: "Balance", meta: { numeric: true },
      cell: (c) => {
        const b = c.getValue<number>();
        return <span className={cn("font-medium", b < 0 && "text-tone-red")}>{fmtNum(Math.abs(b))}{b < 0 ? " Cr" : ""}</span>;
      },
    },
  ], []);

  const glColumns = useMemo<ColumnDef<GlLine>[]>(() => [
    { accessorKey: "date", header: "Date", meta: { width: "100px" }, cell: (c) => <span className="tabular text-fg-muted">{new Date(c.getValue<string>()).toLocaleDateString("en-GB", { day: "numeric", month: "short" })}</span> },
    { accessorKey: "entry_number", header: "Entry", meta: { width: "130px", hideOnMobile: true }, cell: (c) => <span className="font-mono text-[13px] text-fg-muted">{c.getValue<string>()}</span> },
    {
      accessorKey: "narration", header: "Narration",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate">{c.row.original.narration}</p>
          {c.row.original.description && <p className="truncate text-xs text-fg-muted">{c.row.original.description}</p>}
        </div>
      ),
    },
    { accessorKey: "debit", header: "Debit", meta: { numeric: true }, cell: (c) => c.getValue<number>() > 0 ? fmtNum(c.getValue<number>()) : "" },
    { accessorKey: "credit", header: "Credit", meta: { numeric: true }, cell: (c) => c.getValue<number>() > 0 ? fmtNum(c.getValue<number>()) : "" },
    {
      accessorKey: "balance", header: "Balance", meta: { numeric: true },
      cell: (c) => {
        const b = c.getValue<number>();
        return <span className={cn("font-medium", b < 0 && "text-tone-red")}>{fmtNum(Math.abs(b))}{b < 0 ? " Cr" : ""}</span>;
      },
    },
  ], []);

  const arColumns = useMemo<ColumnDef<ArItem>[]>(() => [
    {
      accessorKey: "customer_name", header: "Customer / invoice",
      cell: (c) => (
        <div>
          <p className="font-medium">{c.row.original.customer_name}</p>
          <p className="font-mono text-xs text-fg-muted">{c.row.original.number}</p>
        </div>
      ),
    },
    {
      accessorKey: "due_date", header: "Due date", meta: { hideOnMobile: true },
      cell: (c) => <span className="tabular text-fg-muted">{c.getValue<string | null>() ? new Date(c.getValue<string>()).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "No due date"}</span>,
    },
    { accessorKey: "amount_due", header: "Outstanding", meta: { numeric: true }, cell: (c) => <span className="font-medium">{fmt(c.getValue<number>(), c.row.original.currency)}</span> },
    {
      accessorKey: "days_overdue", header: "Overdue", meta: { numeric: true },
      cell: (c) => {
        const d = c.getValue<number>();
        if (d <= 0) return <Pill tone="neutral">Current</Pill>;
        return <Pill tone={d > 60 ? "red" : "orange"}>{d}d</Pill>;
      },
    },
  ], []);

  const arItems: ArItem[] = ar ? [
    ...ar.current.items,
    ...(ar["1_30_days"].items as ArItem[]),
    ...(ar["31_60_days"].items as ArItem[]),
    ...(ar["61_90_days"].items as ArItem[]),
    ...(ar.over_90_days.items as ArItem[]),
  ] : [];

  return (
    <div className="animate-fade-up">
      <PageHeader title="Financial reports" summary="P&L, balance sheet, trial balance, AR aging, general ledger" />

      <Tabs value={tab} onValueChange={(v) => setTab(v as Tab)}>
        <TabsList className="mb-4">
          {tabs.map(t => (
            <TabsTrigger key={t.key} value={t.key}>
              <t.icon className="size-3.5" aria-hidden />
              {t.label}
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>

      <div className="mb-4 flex flex-wrap items-end gap-3">
        {tab === "balance_sheet" || tab === "aged_ar" ? (
          <>
            <Field label="As of date" className="w-44">
              <Input type="date" value={asOf} onChange={e => setAsOf(e.target.value)} />
            </Field>
            {tab === "aged_ar" && (
              <Checkbox label="Include proformas (collections view)" checked={includeProforma}
                onChange={e => setIncludeProforma(e.target.checked)} />
            )}
          </>
        ) : (
          <>
            <Field label="From" className="w-44">
              <Input type="date" value={dateFrom} onChange={e => setDateFrom(e.target.value)} />
            </Field>
            <Field label="To" className="w-44">
              <Input type="date" value={dateTo} onChange={e => setDateTo(e.target.value)} />
            </Field>
            <div className="flex gap-1.5">
              {[new Date().getFullYear(), new Date().getFullYear() - 1].map(y => {
                const on = year === y && dateFrom === `${y}-01-01`;
                return (
                  <Button key={y} variant={on ? "primary" : "secondary"} aria-pressed={on} className="tabular"
                    onClick={() => { setDateFrom(`${y}-01-01`); setDateTo(`${y}-12-31`); setYear(y); }}>
                    {y}
                  </Button>
                );
              })}
            </div>
          </>
        )}
        {tab === "gl" && (
          <Field label="Account" className="min-w-[260px]">
            <Select value={selectedAccount} onChange={e => setSelectedAccount(e.target.value)} placeholder="Select account">
              {accounts.map(a => <option key={a.id} value={a.id}>{a.code} {a.name}</option>)}
            </Select>
          </Field>
        )}
      </div>

      {loading ? (
        <Card className="space-y-2 p-4">
          {[0, 1, 2, 3].map(i => <Skeleton key={i} className="h-5 w-full" />)}
        </Card>
      ) : (
        <>
          {/* P&L */}
          {tab === "pl" && pl && (
            <div className="space-y-4">
              <StatGroup>
                <Stat label="Total revenue" value={fmt(pl.total_revenue, pl.functional_currency)} />
                <Stat label="Total expenses" value={fmt(pl.total_expenses, pl.functional_currency)} />
                <Stat label="Net income" value={fmt(pl.net_income, pl.functional_currency)}
                  alert={pl.net_income < 0} note={pl.net_income < 0 ? "Loss in period" : undefined} />
              </StatGroup>
              <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
                <StatementCard title="Revenue" empty="No revenue entries in period"
                  rows={pl.revenue.map(r => ({ code: r.code, name: r.name, value: r.amount }))}
                  total={pl.total_revenue} format={(n) => fmt(n, pl.functional_currency)} />
                <StatementCard title="Expenses" empty="No expense entries in period"
                  rows={pl.expenses.map(e => ({ code: e.code, name: e.name, value: e.amount }))}
                  total={pl.total_expenses} format={(n) => fmt(n, pl.functional_currency)} />
              </div>
              <Card className="flex flex-wrap items-center justify-between gap-2 px-4 py-3">
                <p className="text-sm font-semibold text-fg">Net income <span className="font-normal text-fg-muted tabular">({dateFrom} to {dateTo})</span></p>
                <p className={cn("text-xl font-semibold tabular", pl.net_income >= 0 ? "text-fg" : "text-tone-red")}>
                  {pl.net_income >= 0 ? "+" : ""}{fmt(pl.net_income, pl.functional_currency)}
                </p>
              </Card>
            </div>
          )}

          {/* Balance Sheet */}
          {tab === "balance_sheet" && bs && (
            <div className="space-y-4">
              <StatGroup>
                <Stat label="Total assets" value={fmt(bs.total_assets, bs.functional_currency)} />
                <Stat label="Total liabilities" value={fmt(bs.total_liabilities, bs.functional_currency)} />
                <Stat label="Total equity" value={fmt(bs.total_equity, bs.functional_currency)} />
              </StatGroup>
              {!bs.balanced && (
                <div role="alert" className="flex items-center gap-2 rounded-lg bg-tone-red-soft px-4 py-3 text-sm font-semibold text-tone-red">
                  <AlertTriangle className="size-4 shrink-0" aria-hidden />
                  Balance sheet does not balance. Check for unposted entries or missing equity adjustments.
                </div>
              )}
              <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
                <div className="space-y-4">
                  <StatementCard title="Assets" empty="No balances"
                    rows={bs.assets.map(r => ({ code: r.code, name: r.name, value: r.balance }))}
                    total={bs.total_assets} format={plainNum} />
                </div>
                <div className="space-y-4">
                  <StatementCard title="Liabilities" empty="No balances"
                    rows={bs.liabilities.map(r => ({ code: r.code, name: r.name, value: r.balance }))}
                    total={bs.total_liabilities} format={plainNum} />
                  <StatementCard title="Equity" empty="No balances"
                    rows={bs.equity.map(r => ({ code: r.code, name: r.name, value: r.balance, computed: r.computed }))}
                    total={bs.total_equity} format={plainNum} />
                </div>
              </div>
              <Card className="flex items-center justify-between px-4 py-3">
                <span className="text-sm font-semibold text-fg-muted">Liabilities + equity</span>
                <span className="text-sm font-semibold text-fg tabular">{fmt(bs.total_liabilities + bs.total_equity, bs.functional_currency)}</span>
              </Card>
            </div>
          )}

          {/* Trial Balance */}
          {tab === "trial_balance" && tb && (
            <Card className="overflow-hidden">
              <CardHeader title="Trial balance" description={<span className="tabular">{dateFrom} to {dateTo}</span>}
                actions={tb.balanced ? <Pill tone="blue">Balanced</Pill> : <Pill tone="red">Out of balance</Pill>} />
              <DataTable label="Trial balance" columns={tbColumns} data={tb.lines} getRowId={(l) => l.code} maxHeight="500px"
                empty={{ title: "No accounts with activity", compact: true }}
                footer={
                  <div className="flex w-full flex-wrap items-center justify-end gap-x-6 gap-y-1 font-semibold text-fg tabular">
                    <span className="mr-auto">Totals</span>
                    <span>Debit {fmtNum(tb.total_debit)}</span>
                    <span>Credit {fmtNum(tb.total_credit)}</span>
                  </div>
                } />
            </Card>
          )}

          {/* AR Aging */}
          {tab === "aged_ar" && ar && (
            <div className="space-y-4">
              <StatGroup>
                {[
                  { label: "Current",    data: ar.current,       alert: false },
                  { label: "1-30 days",  data: ar["1_30_days"],  alert: false },
                  { label: "31-60 days", data: ar["31_60_days"], alert: false },
                  { label: "61-90 days", data: ar["61_90_days"], alert: ar["61_90_days"].count > 0 },
                  { label: "90+ days",   data: ar.over_90_days,  alert: ar.over_90_days.count > 0 },
                ].map(b => (
                  <Stat key={b.label} label={b.label} value={fmt(b.data.total, ar.functional_currency)} alert={b.alert}
                    note={`${b.data.count} invoice${b.data.count !== 1 ? "s" : ""}`} />
                ))}
              </StatGroup>
              <Card className="overflow-hidden">
                <CardHeader title="Outstanding invoices"
                  actions={<span className="text-sm font-semibold text-fg tabular">Total: {fmt(ar.total_outstanding, ar.functional_currency)}</span>} />
                <DataTable label="Outstanding invoices" columns={arColumns} data={arItems} getRowId={(i) => i.id} maxHeight="24rem"
                  empty={{ title: "Nothing outstanding", compact: true }} />
              </Card>
            </div>
          )}

          {/* General Ledger */}
          {tab === "gl" && (
            gl ? (
              <Card className="overflow-hidden">
                <CardHeader
                  title={<><span className="font-mono">{gl.account.code}</span> {gl.account.name}</>}
                  description={<span className="capitalize">{gl.account.category}</span>}
                  actions={
                    <div className="text-right">
                      <p className="text-xs text-fg-muted">Closing balance</p>
                      <p className={cn("text-lg font-semibold tabular", gl.closing_balance >= 0 ? "text-fg" : "text-tone-red")}>
                        {fmt(gl.closing_balance, gl.functional_currency)}
                      </p>
                    </div>
                  } />
                <DataTable label="General ledger" columns={glColumns} data={gl.lines} maxHeight="500px"
                  empty={{ title: "No transactions in this period", compact: true }} />
              </Card>
            ) : (
              <Card>
                <EmptyState icon={BookOpen} title="Select an account to view its general ledger" />
              </Card>
            )
          )}

          {!pl && tab === "pl" && !loading && (
            <Card>
              <EmptyState icon={BarChart3} title="No data" description="Post journal entries to see reports." />
            </Card>
          )}
        </>
      )}
    </div>
  );
}

const plainNum = (n: number) => new Intl.NumberFormat("en-US", { minimumFractionDigits: 0 }).format(n);

/** One section of a financial statement: code, account, amount, and a total row. */
function StatementCard({
  title, rows, total, format, empty,
}: {
  title: string;
  rows: { code: string | null; name: string; value: number; computed?: boolean }[];
  total: number;
  format: (n: number) => string;
  empty: string;
}) {
  return (
    <Card className="overflow-hidden">
      <CardHeader title={title} />
      <table className="w-full border-separate border-spacing-0 text-sm" aria-label={title}>
        <thead className="sr-only">
          <tr><th scope="col">Code</th><th scope="col">Account</th><th scope="col">Amount</th></tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr><td colSpan={3} className="px-4 py-3 text-[13px] text-fg-muted">{empty}</td></tr>
          ) : rows.map((r, i) => (
            <tr key={r.code ?? r.name} className={cn(i % 2 === 1 ? "bg-surface-2/70" : "bg-surface", r.computed && "italic")}>
              <td className="h-row w-16 border-b border-line-soft pl-4 pr-2 font-mono text-xs text-fg-subtle">{r.code ?? ""}</td>
              <td className="h-row border-b border-line-soft px-2 text-fg">{r.name}</td>
              <td className="h-row border-b border-line-soft pl-2 pr-4 text-right tabular text-fg">{format(r.value)}</td>
            </tr>
          ))}
          <tr className="bg-surface-2">
            <td colSpan={2} className="h-row pl-4 pr-2 font-semibold text-fg">Total {title.toLowerCase()}</td>
            <td className="h-row pl-2 pr-4 text-right font-semibold tabular text-fg">{format(total)}</td>
          </tr>
        </tbody>
      </table>
    </Card>
  );
}
