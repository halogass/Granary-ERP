"use client";

import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { Mail, Send, Plus, ArrowRight } from "lucide-react";
import { emailMarketingApi } from "@/lib/api";
import {
  Button, Card, CardHeader, DataTable, PageHeader, Stat, StatGroup, StatusPill, type ColumnDef,
} from "@/components/ui";

interface MailingList { id: string; name: string; subscriber_count: number }
interface Campaign { id: string; name: string; subject: string; status: string; sent_count: number; open_count: number; total_recipients: number; created_at: string }

export default function EmailMarketingPage() {
  const [lists, setLists]         = useState<MailingList[]>([]);
  const [campaigns, setCampaigns] = useState<Campaign[]>([]);

  useEffect(() => {
    emailMarketingApi.listLists().then((r) => setLists(r.data.slice(0, 5)));
    emailMarketingApi.listCampaigns({ limit: 5 }).then((r) => setCampaigns(r.data));
  }, []);

  const totalSubscribers = lists.reduce((s, l) => s + l.subscriber_count, 0);
  const sentCampaigns    = campaigns.filter((c) => c.status === "sent");
  const totalSent        = sentCampaigns.reduce((s, c) => s + c.sent_count, 0);
  const totalOpens       = sentCampaigns.reduce((s, c) => s + c.open_count, 0);
  const avgOpenRate      = totalSent > 0 ? Math.round((totalOpens / totalSent) * 100) : 0;

  const listColumns = useMemo<ColumnDef<MailingList>[]>(() => [
    { accessorKey: "name", header: "List", cell: (c) => <span className="font-medium text-fg">{c.getValue<string>()}</span> },
    { accessorKey: "subscriber_count", header: "Subscribers", meta: { numeric: true }, cell: (c) => c.getValue<number>().toLocaleString() },
  ], []);

  const campaignColumns = useMemo<ColumnDef<Campaign>[]>(() => [
    {
      accessorKey: "name", header: "Campaign",
      cell: (c) => (
        <div className="min-w-0">
          <p className="truncate font-medium text-fg">{c.row.original.name}</p>
          <p className="truncate text-xs text-fg-muted">{c.row.original.subject}</p>
        </div>
      ),
    },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="email_campaign" status={c.getValue<string>()} /> },
  ], []);

  return (
    <div className="space-y-4">
      <PageHeader title="Email marketing" summary="Manage mailing lists and send campaigns"
        actions={<Button variant="primary" asChild><Link href="/marketing/email/campaigns/new"><Plus aria-hidden /> New campaign</Link></Button>} />

      <StatGroup>
        <Stat label="Total subscribers" value={totalSubscribers.toLocaleString()} href="/marketing/email/lists" />
        <Stat label="Emails sent" value={totalSent.toLocaleString()} href="/marketing/email/campaigns" />
        <Stat label="Avg open rate" value={`${avgOpenRate}%`} />
      </StatGroup>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <Card className="overflow-hidden">
          <CardHeader title={<span className="inline-flex items-center gap-2"><Mail className="size-4 text-fg-muted" aria-hidden /> Mailing lists</span>}
            actions={<>
              <Button size="sm" variant="ghost" asChild><Link href="/marketing/email/lists">View all <ArrowRight aria-hidden /></Link></Button>
              <Button size="sm" asChild><Link href="/marketing/email/lists"><Plus aria-hidden /> Create list</Link></Button>
            </>} />
          <DataTable label="Mailing lists" columns={listColumns} data={lists} getRowId={(r) => r.id} maxHeight="none"
            rowHref={(l) => `/marketing/email/lists/${l.id}`}
            empty={{ compact: true, title: "No mailing lists yet", description: "Create a list, then add subscribers." }} />
        </Card>

        <Card className="overflow-hidden">
          <CardHeader title={<span className="inline-flex items-center gap-2"><Send className="size-4 text-fg-muted" aria-hidden /> Recent campaigns</span>}
            actions={<Button size="sm" variant="ghost" asChild><Link href="/marketing/email/campaigns">View all <ArrowRight aria-hidden /></Link></Button>} />
          <DataTable label="Recent campaigns" columns={campaignColumns} data={campaigns} getRowId={(r) => r.id} maxHeight="none"
            rowHref={(c) => `/marketing/email/campaigns/${c.id}`}
            empty={{ compact: true, title: "No campaigns yet", description: "Write a campaign and send it to a list." }} />
        </Card>
      </div>
    </div>
  );
}
