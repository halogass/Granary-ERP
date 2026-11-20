"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import { portalVendorApi } from "@/lib/api";
import { ClipboardList } from "lucide-react";
import { DataTable, KeyValues, ListFrame, PageHeader, Stat, StatGroup, StatusPill, type ColumnDef } from "@/components/ui";

interface WorkOrder {
  id: string;
  wo_number: string;
  title: string;
  description?: string;
  status: string;
  total_amount?: number;
  currency: string;
  start_date?: string;
  due_date?: string;
  created_at: string;
}

const fmtDate = (d?: string) => d ? new Date(d).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : "-";

export default function VendorWorkOrdersPage() {
  const [orders, setOrders] = useState<WorkOrder[]>([]);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await portalVendorApi.listMyWorkOrders();
      setOrders(res.data.items ?? res.data ?? []);
    } catch { setOrders([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const active = orders.filter((o) => o.status === "in_progress").length;
  const totalValue = orders.reduce((s, o) => s + (o.total_amount ?? 0), 0);

  const columns = useMemo<ColumnDef<WorkOrder>[]>(() => [
    { accessorKey: "wo_number", header: "Number", meta: { hideOnMobile: true }, cell: (c) => <span className="font-mono text-[13px]">{c.getValue<string>()}</span> },
    { accessorKey: "title", header: "Title", meta: { card: "title" }, cell: (c) => <span className="font-medium">{c.getValue<string>()}</span> },
    { accessorKey: "start_date", header: "Start", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmtDate(c.getValue<string>()) },
    { accessorKey: "due_date", header: "Due", meta: { numeric: true }, cell: (c) => fmtDate(c.getValue<string>()) },
    { accessorKey: "total_amount", header: "Amount", meta: { numeric: true },
      cell: ({ row }) => row.original.total_amount
        ? <>{row.original.total_amount.toLocaleString()} <span className="text-xs text-fg-muted">{row.original.currency}</span></>
        : "-" },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill status={c.getValue<string>()} /> },
  ], []);

  return (
    <div className="space-y-4">
      <StatGroup>
        <Stat label="Total" value={orders.length} />
        <Stat label="In progress" value={active} />
        <Stat label="Total value" value={`$${totalValue.toLocaleString()}`} />
      </StatGroup>
      <ListFrame>
        <PageHeader flush title="My work orders" summary="Assignments from us." />
        <DataTable label="Work orders" columns={columns} data={orders} getRowId={(o) => o.id} loading={loading}
          empty={{ icon: ClipboardList, title: "No work orders assigned", description: "New assignments appear here." }}
          renderDetails={(o) => (
            <div className="space-y-3">
              <KeyValues cols={3} items={[
                { label: "Number", value: <span className="font-mono">{o.wo_number}</span> },
                { label: "Start", value: fmtDate(o.start_date) },
                { label: "Due", value: fmtDate(o.due_date) },
              ]} />
              {o.description && <p className="whitespace-pre-wrap text-[13px] text-fg">{o.description}</p>}
            </div>
          )} />
      </ListFrame>
    </div>
  );
}
