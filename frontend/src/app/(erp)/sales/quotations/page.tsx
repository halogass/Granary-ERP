"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState, useCallback } from "react";
import { salesApi, systemApi } from "@/lib/api";
import { formatCurrency } from "@/lib/currency";
import { AlertTriangle, Download, Plus, Search, Trash2, TrendingUp } from "lucide-react";
import Link from "next/link";
import { csvName, downloadCsv } from "@/lib/csvExport";
import {
  Button, DataTable, FilterChips, IconButton, Input, KeyValues, ListFrame, PageHeader, SavedViewTabs,
  Stat, StatGroup, StatusPill, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";

interface Quotation {
  id: string;
  number: string;
  title?: string;
  status: string;
  /** 17 Nov 2026: "approved" for a draft whose approval still matches it; otherwise the status. */
  display_status?: string;
  total: number;
  currency?: string;
  issue_date: string;
  valid_until?: string;
  customer_org?: { name: string };
  customer_contact?: { full_name: string };
}

const STATUS_LABELS: Record<string, string> = {
  draft: "Draft", approved: "Approved", sent: "Sent", accepted: "Accepted", signed: "PO signed", rejected: "Rejected",
  expired: "Expired", superseded: "Superseded", cancelled: "Cancelled",
};

const fmt = (n: number, cur = "USD") => formatCurrency(n, cur);
const fmtDate = (d?: string | null) => (d ? new Date(d).toLocaleDateString("en-GB") : "-");

const fmtK = (n: number) => {
  if (n >= 1e9) return `${(n/1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n/1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n/1e3).toFixed(0)}K`;
  return `${n}`;
};

const isExpiringSoon = (q: Quotation) =>
  !!q.valid_until && q.status === "sent" && new Date(q.valid_until) < new Date(Date.now() + 3 * 86400000);

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
  { id: "sent", label: "Sent", filters: { status: "sent" } },
  { id: "accepted", label: "Accepted", filters: { status: "accepted" } },
  { id: "signed", label: "PO signed", filters: { status: "signed" } },
];

// 1 Nov 2026: a cancelled quotation is hidden from the other views; its chip shows them.
const CHIP_STATUSES = ["rejected", "expired", "superseded", "cancelled"];

export default function QuotationsPage() {
  const views = useSavedViews<Filters>("sales.quotations", VIEWS);
  const [chipStatus, setChipStatus] = useState("");
  const [quotes, setQuotes] = useState<Quotation[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [functionalCurrency, setFunctionalCurrency] = useState("USD");

  const statusFilter = chipStatus || views.active?.filters.status || "";
  const filters: Filters = statusFilter ? { status: statusFilter } : {};

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const params: Record<string, string> = {};
      if (search) params.search = search;
      if (statusFilter) params.status = statusFilter;
      const [res, currRes] = await Promise.all([
        fetchAllPages<Quotation>(salesApi.listQuotations, params),
        systemApi.getCurrencySettings().catch(() => null),
      ]);
      setQuotes(res.rows);
      if (currRes?.data) setFunctionalCurrency(currRes.data.functional ?? "USD");
    } catch { setQuotes([]); } finally { setLoading(false); }
  }, [search, statusFilter]);

  useEffect(() => { load(); }, [load]);

  async function deleteQuotation(id: string) {
    setDeleting(true);
    try {
      await salesApi.deleteQuotation(id);
      setConfirmDeleteId(null);
      setQuotes((prev) => prev.filter((q) => q.id !== id));
    } catch { /* keep */ } finally { setDeleting(false); }
  }

  /** The list as shown (current view, chip and search) as a CSV file. */
  function exportCsv() {
    downloadCsv(csvName("quotations"), quotes, [
      { header: "Number", value: (q) => q.number },
      { header: "Title", value: (q) => q.title },
      { header: "Client", value: (q) => q.customer_org?.name ?? q.customer_contact?.full_name },
      { header: "Status", value: (q) => STATUS_LABELS[q.display_status ?? q.status] ?? q.display_status ?? q.status },
      { header: "Total", value: (q) => q.total },
      { header: "Currency", value: (q) => q.currency },
      { header: "Issued", value: (q) => q.issue_date },
      { header: "Valid until", value: (q) => q.valid_until },
    ]);
  }

  const inFunctional = (q: Quotation) => !q.currency || q.currency === functionalCurrency;
  const totalSent = quotes.filter((q) => q.status === "sent" && inFunctional(q)).reduce((s, q) => s + q.total, 0);
  const totalWon = quotes.filter((q) => (q.status === "accepted" || q.status === "signed") && inFunctional(q)).reduce((s, q) => s + q.total, 0);
  const sentCount = quotes.filter((q) => q.status === "sent").length;
  const wonCount = quotes.filter((q) => q.status === "accepted" || q.status === "signed").length;
  const draftCount = quotes.filter((q) => q.status === "draft").length;
  const expiringSoon = quotes.filter(isExpiringSoon).length;

  const columns = useMemo<ColumnDef<Quotation>[]>(() => [
    {
      accessorKey: "number", header: "Number",
      cell: (c) => (
        <div>
          <Link href={`/sales/quotations/${c.row.original.id}`} className="font-mono text-[13px] font-medium text-fg hover:text-brand-ink">
            {c.row.original.number}
          </Link>
          {c.row.original.title && <p className="max-w-36 truncate text-xs text-fg-muted">{c.row.original.title}</p>}
        </div>
      ),
    },
    {
      id: "client", accessorFn: (q) => q.customer_org?.name ?? q.customer_contact?.full_name ?? "", header: "Client",
      cell: (c) => <p className="max-w-44 truncate font-medium text-fg">{c.getValue<string>() || "-"}</p>,
    },
    {
      accessorKey: "status", header: "Status",
      cell: (c) => {
        const s = c.row.original.display_status ?? c.row.original.status;
        return <StatusPill family="quotation" status={s} label={STATUS_LABELS[s]} tone={s === "signed" ? "blue" : undefined} />;
      },
    },
    {
      accessorKey: "total", header: "Total", meta: { numeric: true },
      cell: (c) => <span className="font-medium">{fmt(c.row.original.total, c.row.original.currency)}</span>,
    },
    {
      accessorKey: "issue_date", header: "Issued", meta: { hideOnMobile: true },
      cell: (c) => <span className="text-fg-muted">{fmtDate(c.row.original.issue_date)}</span>,
    },
    {
      accessorKey: "valid_until", header: "Valid until", meta: { hideOnMobile: true },
      cell: (c) => {
        const q = c.row.original;
        if (!q.valid_until) return <span className="text-fg-subtle">-</span>;
        return isExpiringSoon(q)
          ? <span className="inline-flex items-center gap-1 font-medium text-tone-orange"><AlertTriangle className="size-3" aria-hidden />{fmtDate(q.valid_until)}</span>
          : <span className="text-fg-muted">{fmtDate(q.valid_until)}</span>;
      },
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const q = c.row.original;
        if (confirmDeleteId === q.id) {
          return (
            <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
              <span className="text-xs font-semibold text-tone-red">Delete?</span>
              <Button size="sm" variant="danger" loading={deleting} onClick={() => deleteQuotation(q.id)}>Yes</Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirmDeleteId(null)}>No</Button>
            </div>
          );
        }
        return ["draft", "rejected", "expired"].includes(q.status)
          ? <IconButton size="sm" variant="ghost" icon={Trash2} label={`Delete ${q.number}`} onClick={() => setConfirmDeleteId(q.id)} />
          : null;
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], [confirmDeleteId, deleting]);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Sent, pending" value={`${functionalCurrency} ${fmtK(totalSent)}`} note={`${sentCount} quotation${sentCount !== 1 ? "s" : ""}`} />
        <Stat label="Won, accepted" value={`${functionalCurrency} ${fmtK(totalWon)}`} note={`${wonCount} quotation${wonCount !== 1 ? "s" : ""}`} />
        <Stat label="Draft" value={draftCount} note="not sent yet" />
        <Stat label="Expiring soon" value={expiringSoon} note="sent, valid under 3 days" alert={expiringSoon > 0} />
      </StatGroup>

      <ListFrame>
        <PageHeader flush title="Quotations"
          summary={`${quotes.length} quotation${quotes.length !== 1 ? "s" : ""}, won ${functionalCurrency} ${fmtK(totalWon)}`}
          actions={<>
            <Button disabled={loading || quotes.length === 0} onClick={exportCsv}><Download aria-hidden /> Export</Button>
            <Button variant="primary" asChild><Link href="/sales/quotations/new"><Plus aria-hidden /> New quotation</Link></Button>
          </>} />
        <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setChipStatus("")} />
        <FilterChips
          chips={CHIP_STATUSES.map((s) => ({ id: s, label: STATUS_LABELS[s], active: chipStatus === s }))}
          onToggle={(id) => setChipStatus((cur) => (cur === id ? "" : id))}
          onClear={() => setChipStatus("")}>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search quotations" aria-label="Search quotations" className="w-56" />
        </FilterChips>
        <DataTable label="Quotations" columns={columns} data={quotes} getRowId={(r) => r.id} loading={loading}
          empty={{
            icon: TrendingUp, title: "No quotations yet", description: "Create your first quotation to start winning deals.",
            action: <Button variant="primary" asChild><Link href="/sales/quotations/new"><Plus aria-hidden /> Create quotation</Link></Button>,
          }}
          renderDetails={(q) => (
            <div className="flex flex-wrap items-start gap-6">
              <KeyValues className="min-w-[260px] flex-1" cols={3} items={[
                { label: "Client", value: q.customer_org?.name ?? q.customer_contact?.full_name ?? "-" },
                { label: "Contact", value: q.customer_contact?.full_name ?? "-" },
                { label: "Title", value: q.title ?? "-" },
                { label: "Total", value: fmt(q.total, q.currency), numeric: true },
                { label: "Issued", value: fmtDate(q.issue_date) },
                { label: "Valid until", value: fmtDate(q.valid_until) },
              ]} />
              <Button size="sm" asChild><Link href={`/sales/quotations/${q.id}`}>Open quotation</Link></Button>
            </div>
          )} />
      </ListFrame>
    </div>
  );
}
