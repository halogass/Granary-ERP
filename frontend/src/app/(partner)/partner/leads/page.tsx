"use client";

import { useState, useEffect, useMemo } from "react";
import Link from "next/link";
import { useSession } from "next-auth/react";
import { Inbox } from "lucide-react";
import {
  Button, DataTable, FilterChips, KeyValues, ListFrame, PageHeader, Pill, StatusPill, toast, type ColumnDef,
} from "@/components/ui";

interface Lead {
  id: string;
  customer_name: string;
  customer_email: string | null;
  customer_company: string | null;
  customer_country: string | null;
  product_interest: string[];
  notes: string | null;
  status: string;
  assigned_at: string | null;
  expires_at: string | null;
  created_at: string;
}

const STATUS_LABEL: Record<string, string> = {
  new: "New", contacted: "Contacted", qualified: "Qualified", converted: "Converted", dead: "Dead",
};
const STATUSES = ["new", "contacted", "qualified", "converted", "dead"];

function fmt(d: string) {
  return new Date(d).toLocaleDateString("en-GB", { day: "2-digit", month: "short", year: "numeric" });
}

export default function PartnerLeadsPage() {
  const { data: session } = useSession();
  const [leads, setLeads] = useState<Lead[]>([]);
  const [loading, setLoading] = useState(true);
  const [statusFilter, setStatusFilter] = useState("all");

  useEffect(() => {
    if (!session?.access_token) return;
    fetch("/api/v1/partner/portal/leads", {
      headers: { Authorization: `Bearer ${session.access_token}` },
    })
      .then(r => r.json())
      .then(setLeads)
      .catch(() => toast.error("Failed to load leads"))
      .finally(() => setLoading(false));
  }, [session?.access_token]);

  const filtered = statusFilter === "all" ? leads : leads.filter(l => l.status === statusFilter);

  const columns = useMemo<ColumnDef<Lead>[]>(() => [
    { accessorKey: "customer_name", header: "Customer", cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
    { accessorKey: "customer_company", header: "Company", meta: { hideOnMobile: true }, cell: (c) => c.getValue<string>() || "-" },
    { id: "products", header: "Products", enableSorting: false, meta: { hideOnMobile: true },
      cell: ({ row }) => {
        const p = row.original.product_interest || [];
        return (
          <span className="flex flex-wrap items-center gap-1">
            {p.slice(0, 2).map((x) => <Pill key={x}>{x}</Pill>)}
            {p.length > 2 && <span className="text-xs text-fg-muted">+{p.length - 2}</span>}
          </span>
        );
      } },
    { accessorKey: "assigned_at", header: "Assigned", meta: { numeric: true, hideOnMobile: true },
      cell: (c) => c.getValue<string>() ? fmt(c.getValue<string>()) : "-" },
    { accessorKey: "status", header: "Status",
      cell: (c) => <StatusPill family="partner_lead" status={c.getValue<string>()} label={STATUS_LABEL[c.getValue<string>()]} /> },
  ], []);

  return (
    <ListFrame>
      <PageHeader flush title="Assigned leads" summary="Warm leads assigned to you to follow up on." />
      <FilterChips
        chips={STATUSES.map((s) => ({ id: s, label: STATUS_LABEL[s], active: statusFilter === s }))}
        onToggle={(id) => setStatusFilter((cur) => (cur === id ? "all" : id))}
        onClear={() => setStatusFilter("all")} />
      <DataTable label="Assigned leads" columns={columns} data={filtered} getRowId={(l) => l.id} loading={loading}
        empty={{ icon: Inbox, title: "No leads found", description: "Warm leads assigned to you appear here when available." }}
        renderDetails={(lead) => (
          <div className="flex flex-wrap items-start gap-6">
            <div className="min-w-[260px] flex-1 space-y-3">
              <KeyValues items={[
                { label: "Company", value: lead.customer_company || "-" },
                { label: "Email", value: lead.customer_email || "-" },
                { label: "Country", value: lead.customer_country || "-" },
                { label: "Product interest", value: lead.product_interest.length > 0 ? lead.product_interest.join(", ") : "-" },
                ...(lead.expires_at ? [{ label: "Lead expires", value: <span className="font-medium text-tone-orange">{fmt(lead.expires_at)}</span> }] : []),
              ]} />
              {lead.notes && (
                <div className="rounded-lg border border-line bg-surface p-3 text-[13px]">
                  <p className="text-xs text-fg-muted">Notes from us</p>
                  <p className="mt-1 whitespace-pre-wrap text-fg">{lead.notes}</p>
                </div>
              )}
            </div>
            <Button variant="primary" size="sm" asChild><Link href="/partner/deals">Register as deal</Link></Button>
          </div>
        )} />
    </ListFrame>
  );
}
