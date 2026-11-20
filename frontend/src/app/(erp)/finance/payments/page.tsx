"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { financeApi, systemApi } from "@/lib/api";
import { formatCurrency } from "@/lib/currency";
import { Banknote, Search, CreditCard, Building, CheckCircle2 } from "lucide-react";
import Link from "next/link";
import {
  Button, DataTable, FilterChips, Input, KeyValues, ListFrame, PageHeader, Pill, SavedViewTabs,
  Stat, StatGroup, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Payment {
  id: string;
  amount: number;
  currency?: string;
  payment_date: string;
  method: string;
  reference?: string;
  notes?: string;
  invoice?: { id: string; number: string };
  customer_name?: string | null;
}

// Shape returned by GET /finance/payments.
interface PaymentRow {
  id: string;
  invoice_id: string;
  invoice_number?: string | null;
  customer_name?: string | null;
  amount: number;
  currency?: string;
  payment_date: string;
  payment_method?: string | null;
  payment_reference?: string | null;
}

// payment_method is free text (default "Bank Transfer (T/T)"); bucket it for the chips.
function methodKey(m?: string | null): string {
  const v = (m ?? "").toLowerCase();
  if (v.includes("bank") || v.includes("transfer")) return "bank_transfer";
  if (v.includes("cash")) return "cash";
  if (v.includes("check") || v.includes("cheque")) return "check";
  if (v.includes("card")) return "credit_card";
  return "other";
}

function toPayment(r: PaymentRow): Payment {
  return {
    id: r.id,
    amount: r.amount,
    currency: r.currency,
    payment_date: r.payment_date,
    method: methodKey(r.payment_method),
    reference: r.payment_reference ?? undefined,
    invoice: r.invoice_id ? { id: r.invoice_id, number: r.invoice_number ?? "-" } : undefined,
    customer_name: r.customer_name,
  };
}

const METHOD_META: Record<string, { label: string; icon: React.ElementType }> = {
  bank_transfer: { label: "Bank transfer", icon: Building },
  cash:          { label: "Cash",          icon: Banknote },
  check:         { label: "Check",         icon: CheckCircle2 },
  credit_card:   { label: "Credit card",   icon: CreditCard },
  other:         { label: "Other",         icon: Banknote },
};

type Filters = { range?: "month" | "30d" };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All payments", filters: {} },
  { id: "month", label: "This month", filters: { range: "month" } },
  { id: "30d", label: "Last 30 days", filters: { range: "30d" } },
];

const fmtDay = (iso: string) =>
  new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

const fmt = (n: number, cur = "USD") => formatCurrency(n, cur);

const fmtK = (n: number) => {
  if (n >= 1e9) return `${(n/1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n/1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n/1e3).toFixed(0)}K`;
  return `${n}`;
};

export default function PaymentsPage() {
  const [payments, setPayments] = useState<Payment[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [methodFilter, setMethodFilter] = useState("");
  const views = useSavedViews<Filters>("finance.payments", VIEWS);
  const range = views.active?.filters.range;
  const [functionalCurrency, setFunctionalCurrency] = useState("USD");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [res, currRes] = await Promise.all([
        fetchAllPages<PaymentRow>(financeApi.listPayments),
        systemApi.getCurrencySettings().catch(() => null),
      ]);
      setPayments(res.rows.map(toPayment));
      if (currRes?.data) setFunctionalCurrency(currRes.data.functional ?? "USD");
    } catch { setPayments([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const filtered = payments.filter((p) => {
    const matchSearch = !search ||
      (p.invoice?.number ?? "").toLowerCase().includes(search.toLowerCase()) ||
      (p.reference ?? "").toLowerCase().includes(search.toLowerCase()) ||
      (p.customer_name ?? "").toLowerCase().includes(search.toLowerCase());
    const matchMethod = !methodFilter || p.method === methodFilter;
    let matchRange = true;
    if (range) {
      const d = new Date(p.payment_date);
      const now = new Date();
      matchRange = range === "month"
        ? d.getMonth() === now.getMonth() && d.getFullYear() === now.getFullYear()
        : now.getTime() - d.getTime() <= 30 * 86400000;
    }
    return matchSearch && matchMethod && matchRange;
  });

  const funcPayments = payments.filter(p => !p.currency || p.currency === functionalCurrency);
  const totalCollected = funcPayments.reduce((s, p) => s + p.amount, 0);
  const thisMonth = payments.filter((p) => {
    const d = new Date(p.payment_date);
    const now = new Date();
    return d.getMonth() === now.getMonth() && d.getFullYear() === now.getFullYear();
  });
  const thisMonthTotal = thisMonth
    .filter(p => !p.currency || p.currency === functionalCurrency)
    .reduce((s, p) => s + p.amount, 0);

  const filteredTotal = filtered.filter(p => !p.currency || p.currency === functionalCurrency).reduce((s, p) => s + p.amount, 0);

  const columns = useMemo<ColumnDef<Payment>[]>(() => [
    { accessorKey: "payment_date", header: "Date", meta: { width: "130px" }, cell: (c) => <span className="tabular">{fmtDay(c.getValue<string>())}</span> },
    {
      id: "invoice", header: "Invoice", accessorFn: (p) => p.invoice?.number ?? "",
      cell: (c) => {
        const inv = c.row.original.invoice;
        return inv
          ? <Link href={`/finance/invoices/${inv.id}`} className="font-mono text-[13px] font-medium text-fg hover:text-brand-ink hover:underline">{inv.number}</Link>
          : <span className="text-fg-subtle">-</span>;
      },
    },
    { accessorKey: "customer_name", header: "Client", cell: (c) => c.getValue<string | null>() ?? <span className="text-fg-subtle">-</span> },
    {
      accessorKey: "method", header: "Method", meta: { hideOnMobile: true },
      cell: (c) => {
        const m = METHOD_META[c.getValue<string>()] ?? METHOD_META.other;
        return <Pill icon={m.icon}>{m.label}</Pill>;
      },
    },
    {
      accessorKey: "reference", header: "Reference", meta: { hideOnMobile: true },
      cell: (c) => c.getValue<string>()
        ? <span className="font-mono text-xs text-fg-muted">{c.getValue<string>()}</span>
        : <span className="text-fg-subtle">-</span>,
    },
    { accessorKey: "amount", header: "Amount", meta: { numeric: true }, cell: (c) => <span className="font-medium">{fmt(c.getValue<number>(), c.row.original.currency)}</span> },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total collected" value={`${functionalCurrency} ${fmtK(totalCollected)}`} />
        <Stat label="This month" value={`${functionalCurrency} ${fmtK(thisMonthTotal)}`} />
        <Stat label="Payments" value={payments.length} />
        <Stat label="Payments this month" value={thisMonth.length} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Payments" summary={`${filtered.length} payment${filtered.length !== 1 ? "s" : ""}, ${functionalCurrency} ${fmtK(filteredTotal)}`} />
        <SavedViewTabs state={views} />
        <FilterChips
          chips={Object.entries(METHOD_META).map(([key, meta]) => ({ id: key, label: meta.label, active: methodFilter === key }))}
          onToggle={(id) => setMethodFilter(methodFilter === id ? "" : id)} onClear={() => setMethodFilter("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search payments" aria-label="Search payments" className="w-56" />
        </FilterChips>
        <DataTable label="Payments" columns={columns} data={filtered} getRowId={(p) => p.id} loading={loading}
          empty={{ icon: Banknote, title: "No payments found", description: "Payments are recorded from invoice detail pages." }}
          renderDetails={(pay) => {
            const m = METHOD_META[pay.method] ?? METHOD_META.other;
            return (
              <div className="flex flex-wrap items-start gap-6">
                <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                  { label: "Client", value: pay.customer_name ?? "-" },
                  { label: "Date", value: fmtDay(pay.payment_date), numeric: true },
                  { label: "Amount", value: fmt(pay.amount, pay.currency), numeric: true },
                  { label: "Method", value: m.label },
                  { label: "Reference", value: pay.reference ? <span className="font-mono text-xs">{pay.reference}</span> : "-" },
                  { label: "Invoice", value: pay.invoice?.number ?? "-" },
                ]} />
                {pay.invoice && (
                  <Button size="sm" asChild><Link href={`/finance/invoices/${pay.invoice.id}`}>Open invoice</Link></Button>
                )}
              </div>
            );
          }}
          footer={<><span className="tabular">{filtered.length} payment{filtered.length !== 1 ? "s" : ""}</span><span className="ml-auto tabular">Total: <strong className="font-semibold text-fg">{functionalCurrency} {fmtK(filteredTotal)}</strong></span></>} />
      </ListFrame>
    </div>
  );
}
