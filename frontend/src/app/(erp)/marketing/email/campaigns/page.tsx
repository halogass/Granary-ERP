"use client";

import { fetchAllPages } from "@/lib/fetchAll";
import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { Plus, Send, BarChart2, Trash2 } from "lucide-react";
import { emailMarketingApi } from "@/lib/api";
import {
  Button, DataTable, IconButton, ListFrame, PageHeader, SavedViewTabs, StatusPill, useSavedViews,
  type ColumnDef, type SavedView,
} from "@/components/ui";

interface Campaign {
  id: string; name: string; subject: string; status: string;
  total_recipients: number; sent_count: number; failed_count: number; open_count: number;
  scheduled_at?: string; sent_at?: string; created_at: string;
}

type Filters = { status?: string };

const VIEWS: SavedView<Filters>[] = [
  { id: "all", label: "All", filters: {} },
  { id: "draft", label: "Draft", filters: { status: "draft" } },
  { id: "scheduled", label: "Scheduled", filters: { status: "scheduled" } },
  { id: "sending", label: "Sending", filters: { status: "sending" } },
  { id: "sent", label: "Sent", filters: { status: "sent" } },
  { id: "failed", label: "Failed", filters: { status: "failed" } },
];

export default function EmailCampaignsPage() {
  const views = useSavedViews<Filters>("marketing.email.campaigns", VIEWS);
  const [campaigns, setCampaigns] = useState<Campaign[]>([]);
  const [loading, setLoading]     = useState(true);

  async function load() {
    setLoading(true);
    const { rows } = await fetchAllPages<Campaign>(emailMarketingApi.listCampaigns);
    setCampaigns(rows);
    setLoading(false);
  }
  useEffect(() => { load(); }, []);

  async function del(id: string, name: string) {
    if (!confirm(`Delete campaign "${name}"?`)) return;
    await emailMarketingApi.deleteCampaign(id).catch(() => {});
    load();
  }

  const filter = views.active?.filters.status ?? "";
  const filters: Filters = filter ? { status: filter } : {};
  const shown = filter ? campaigns.filter((c) => c.status === filter) : campaigns;
  const failed = campaigns.filter((c) => c.status === "failed").length;

  const columns = useMemo<ColumnDef<Campaign>[]>(() => [
    {
      accessorKey: "name", header: "Campaign",
      cell: (c) => (
        <div className="min-w-0">
          <Link href={`/marketing/email/campaigns/${c.row.original.id}`} className="font-medium text-fg hover:text-brand-ink">{c.row.original.name}</Link>
          <p className="max-w-[280px] truncate text-xs text-fg-muted">{c.row.original.subject}</p>
        </div>
      ),
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="email_campaign" status={c.getValue<string>()} /> },
    { accessorKey: "total_recipients", header: "Recipients", meta: { numeric: true }, cell: (c) => c.getValue<number>().toLocaleString() },
    { accessorKey: "sent_count", header: "Sent", meta: { numeric: true, hideOnMobile: true }, cell: (c) => c.getValue<number>().toLocaleString() },
    { accessorKey: "open_count", header: "Opens", meta: { numeric: true, hideOnMobile: true }, cell: (c) => c.getValue<number>().toLocaleString() },
    {
      id: "open_rate", accessorFn: (c) => (c.sent_count > 0 ? c.open_count / c.sent_count : 0), header: "Open rate", meta: { numeric: true },
      cell: (c) => {
        const cp = c.row.original;
        if (cp.status !== "sent") return <span className="text-fg-subtle">-</span>;
        const openRate = cp.sent_count > 0 ? Math.round((cp.open_count / cp.sent_count) * 100) : 0;
        return <span className={openRate >= 10 ? "font-semibold text-fg" : "text-fg-muted"}>{openRate}%</span>;
      },
    },
    {
      id: "actions", header: "", enableSorting: false, meta: { align: "right", width: "1%" },
      cell: (c) => {
        const cp = c.row.original;
        return (
          <div className="flex items-center justify-end gap-1">
            {cp.status === "sent" && (
              <Button size="sm" variant="ghost" asChild>
                <Link href={`/marketing/email/campaigns/${cp.id}/report`} aria-label={`View report for ${cp.name}`} title="View report"><BarChart2 aria-hidden /> Report</Link>
              </Button>
            )}
            {cp.status === "draft" && <IconButton size="sm" icon={Trash2} label={`Delete ${cp.name}`} onClick={() => del(cp.id, cp.name)} />}
          </div>
        );
      },
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ], []);

  return (
    <ListFrame>
      <PageHeader flush title="Email campaigns" summary="Create and send email campaigns to your lists"
        back={{ href: "/marketing/email", label: "Email marketing" }}
        actions={<Button variant="primary" asChild><Link href="/marketing/email/campaigns/new"><Plus aria-hidden /> New campaign</Link></Button>} />
      <SavedViewTabs state={views} currentFilters={filters} counts={{ failed: failed || undefined }} alertIds={["failed"]} />
      <DataTable label="Email campaigns" columns={columns} data={shown} getRowId={(r) => r.id} loading={loading}
        rowHref={(c) => `/marketing/email/campaigns/${c.id}`}
        empty={{
          icon: Send, title: "No campaigns yet", description: "Write a campaign and send it to a mailing list.",
          action: <Button variant="primary" asChild><Link href="/marketing/email/campaigns/new"><Plus aria-hidden /> Create campaign</Link></Button>,
        }} />
    </ListFrame>
  );
}
