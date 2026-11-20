"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState } from "react";
import { useParams } from "next/navigation";
import { Loader2, Search } from "lucide-react";
import { emailMarketingApi } from "@/lib/api";
import {
  Card, CardBody, DataTable, EmptyState, FilterChips, Input, ListFrame, PageHeader, Stat, StatGroup, StatusPill,
  type ColumnDef,
} from "@/components/ui";

interface Recipient {
  id: string; email: string; first_name?: string; full_name?: string;
  status: string; error_message?: string; sent_at?: string; opened_at?: string;
}
interface Report {
  campaign_id: string; name: string; status: string;
  total_recipients: number; sent_count: number; failed_count: number;
  open_count: number; open_rate: number;
  recipients: Recipient[];
}

// skipped: unsubscribed after it was queued
const FILTERS = ["sent", "opened", "failed", "pending", "skipped"];

export default function CampaignReportPage() {
  const { id }  = useParams<{ id: string }>();
  const [report, setReport]   = useState<Report | null>(null);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter]   = useState("");
  const [search, setSearch]   = useState("");

  useEffect(() => {
    setLoading(true);
    // The recipients come a page at a time; the campaign figures are the same on every page.
    let head: Report | null = null;
    fetchAllPages<Recipient>(async (p) => {
      const r = await emailMarketingApi.getReport(id, p);
      head = head ?? (r.data as Report);
      return { data: (r.data as Report).recipients ?? [] };
    }).then(({ rows }) => {
      if (head) setReport({ ...(head as Report), recipients: rows });
      setLoading(false);
    });
  }, [id]);

  const columns = useMemo<ColumnDef<Recipient>[]>(() => [
    {
      accessorKey: "email", header: "Recipient",
      cell: (c) => (
        <div>
          <p className="font-medium text-fg">{c.row.original.email}</p>
          {c.row.original.full_name && <p className="text-xs text-fg-muted">{c.row.original.full_name}</p>}
        </div>
      ),
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="email_recipient" status={c.getValue<string>()} /> },
    { accessorKey: "sent_at", header: "Sent at", meta: { numeric: true, hideOnMobile: true }, cell: (c) => <span className="text-xs text-fg-muted">{c.getValue<string>() ? new Date(c.getValue<string>()).toLocaleString() : "-"}</span> },
    { accessorKey: "opened_at", header: "Opened at", meta: { numeric: true, hideOnMobile: true }, cell: (c) => <span className="text-xs text-fg-muted">{c.getValue<string>() ? new Date(c.getValue<string>()).toLocaleString() : "-"}</span> },
    {
      accessorKey: "error_message", header: "Error", meta: { hideOnMobile: true },
      cell: (c) => c.getValue<string>() ? <span className="block break-words text-xs text-tone-red md:max-w-[200px] md:truncate" title={c.getValue<string>()}>{c.getValue<string>()}</span> : <span className="text-fg-subtle">-</span>,
    },
  ], []);

  if (loading) return <div className="flex justify-center py-20"><Loader2 className="size-6 animate-spin text-fg-muted" aria-label="Loading" /></div>;
  if (!report)  return <EmptyState title="Report not found." />;

  const filtered = report.recipients.filter((r) => {
    if (filter && r.status !== filter) return false;
    if (search && !r.email.toLowerCase().includes(search.toLowerCase()) &&
        !r.full_name?.toLowerCase().includes(search.toLowerCase())) return false;
    return true;
  });

  return (
    <div className="space-y-4">
      <PageHeader title={`${report.name}: report`} summary="Delivery and engagement summary"
        back={{ href: `/marketing/email/campaigns/${id}`, label: "Campaign" }} />

      <StatGroup>
        <Stat label="Total recipients" value={report.total_recipients.toLocaleString()} />
        <Stat label="Delivered" value={report.sent_count.toLocaleString()} />
        <Stat label="Failed" value={report.failed_count.toLocaleString()} alert={report.failed_count > 0} />
        <Stat label="Open rate" value={`${report.open_rate}%`} />
      </StatGroup>

      <Card>
        <CardBody>
          <div className="mb-2 flex items-center justify-between">
            <span className="text-sm font-medium text-fg">Open rate progress</span>
            <span className="text-sm font-semibold text-fg tabular">{report.open_rate}%</span>
          </div>
          <div className="h-2.5 overflow-hidden rounded-full bg-surface-2" role="progressbar" aria-label="Open rate"
            aria-valuenow={Math.min(report.open_rate, 100)} aria-valuemin={0} aria-valuemax={100}>
            <div className="h-full rounded-full bg-brand transition-all duration-500" style={{ width: `${Math.min(report.open_rate, 100)}%` }} />
          </div>
          <div className="mt-1 flex justify-between text-[11px] text-fg-muted">
            <span>0%</span><span>Industry avg ~22%</span><span>50%+</span>
          </div>
        </CardBody>
      </Card>

      <ListFrame>
        <FilterChips
          chips={FILTERS.map((s) => ({ id: s, label: s.charAt(0).toUpperCase() + s.slice(1), active: filter === s }))}
          onToggle={(s) => setFilter((cur) => (cur === s ? "" : s))} onClear={() => setFilter("")}>
          <span className="text-xs text-fg-muted tabular">{filtered.length} contacts</span>
          <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
            placeholder="Search recipients" aria-label="Search recipients" value={search} onChange={(e) => setSearch(e.target.value)} className="w-52" />
        </FilterChips>
        <DataTable label="Recipients" columns={columns} data={filtered} getRowId={(r) => r.id}
          empty={{ title: "No recipients match the filter." }} />
      </ListFrame>
    </div>
  );
}
