"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import { portalPartnerApi } from "@/lib/api";
import { DollarSign } from "lucide-react";
import { DataTable, ListFrame, PageHeader, Stat, StatGroup, StatusPill, type ColumnDef } from "@/components/ui";
import { formatCurrency, sumByCurrency } from "@/lib/currency";

interface Commission {
  id: string;
  deal_registration_id?: string;
  commission_type?: string;
  amount: number;
  currency: string;
  rate: number;          // a fraction from the server: 0.1 = 10%
  status: string;
  paid_at?: string | null;
  created_at?: string | null;
}

const fmtDate = (iso?: string | null) =>
  iso ? new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-";
const fmtRate = (fraction: number) =>
  `${(Number(fraction) * 100).toLocaleString("en-US", { maximumFractionDigits: 2 })}%`;
const TYPE_LABEL: Record<string, string> = {
  product_discount: "Product discount", saas_recurring: "SaaS recurring",
  project_onetime: "Project (one-time)", finder_fee: "Finder fee",
};

export default function PartnerCommissionsPage() {
  const [commissions, setCommissions] = useState<Commission[]>([]);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await portalPartnerApi.listMyCommissions();
      setCommissions(res.data.items ?? res.data ?? []);
    } catch { setCommissions([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  // Commissions carry their own currency: grouped by currency, never added across them.
  const byStatus = (status: string) => sumByCurrency(commissions.filter((c) => c.status === status), (c) => c.amount, (c) => c.currency);
  const totalPaid = byStatus("paid");
  const totalPending = byStatus("pending");
  const totalApproved = byStatus("approved");
  const show = (t: { total: number; currency: string }) => formatCurrency(t.total, t.currency);
  const mixedNote = (t: { mixed: boolean; currency: string }) => t.mixed ? `Mixed currencies: ${t.currency} total shown` : undefined;

  const columns = useMemo<ColumnDef<Commission>[]>(() => [
    { id: "type", header: "Commission", accessorFn: (c) => (c.commission_type && (TYPE_LABEL[c.commission_type] ?? c.commission_type)) || "Commission",
      cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
    { accessorKey: "rate", header: "Rate", meta: { numeric: true }, cell: (c) => fmtRate(c.getValue<number>()) },
    { id: "date", header: "Date", meta: { numeric: true, hideOnMobile: true }, accessorFn: (c) => c.paid_at ?? c.created_at ?? "",
      cell: ({ row }) => row.original.paid_at ? `Paid ${fmtDate(row.original.paid_at)}` : `Created ${fmtDate(row.original.created_at)}` },
    { accessorKey: "amount", header: "Amount", meta: { numeric: true },
      cell: ({ row }) => <>{row.original.amount.toLocaleString()} <span className="text-xs text-fg-muted">{row.original.currency}</span></> },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill status={c.getValue<string>()} /> },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Paid out" value={show(totalPaid)} note={mixedNote(totalPaid)} />
        <Stat label="Approved" value={show(totalApproved)} note={mixedNote(totalApproved)} />
        <Stat label="Pending" value={show(totalPending)} note={mixedNote(totalPending)} />
      </StatGroup>
      <ListFrame>
        <PageHeader flush title="My commissions" summary="Commissions the partnership team has recorded for your deals." />
        <DataTable label="Commissions" columns={columns} data={commissions} getRowId={(c) => c.id} loading={loading}
          empty={{ icon: DollarSign, title: "No commissions yet", description: "Commissions appear here once the partnership team records them, usually after a registered deal is won." }} />
      </ListFrame>
    </div>
  );
}
