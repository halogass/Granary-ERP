"use client";

import { useCallback, useEffect, useState } from "react";
import { Ban } from "lucide-react";
import { Button, Card, CardHeader, DataTable, StatusPill, toast, type ColumnDef } from "@/components/ui";
import { hrApi, type KioskDevice } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";

function when(iso: string | null) {
  return iso ? new Date(iso).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" }) : "-";
}

/** Registered clock-in tablets. A tablet is registered on the tablet itself (/clock?kiosk=1, signed in as HR). */
export default function KiosksPanel({ offices }: { offices: { id: string; name: string }[] }) {
  const [kiosks, setKiosks] = useState<KioskDevice[] | null>(null);
  const [revoking, setRevoking] = useState<string | null>(null);

  const load = useCallback(() => {
    hrApi.listKiosks().then((r) => setKiosks(r.data)).catch(() => setKiosks([]));
  }, []);
  useEffect(() => { load(); }, [load]);

  async function revoke(k: KioskDevice) {
    if (!window.confirm(`Revoke "${k.name}"? It stops working immediately and must be registered again.`)) return;
    setRevoking(k.id);
    try {
      await hrApi.revokeKiosk(k.id);
      toast.success(`${k.name} revoked`);
      load();
    } catch (e) {
      toast.error(extractApiError(e, "Could not revoke the kiosk."));
    } finally {
      setRevoking(null);
    }
  }

  const columns: ColumnDef<KioskDevice>[] = [
    { accessorKey: "name", header: "Tablet", cell: ({ row }) => <span className="font-medium">{row.original.name}</span> },
    { id: "office", header: "Office", cell: ({ row }) => row.original.office_location ?? "-" },
    { id: "token", header: "Token", meta: { hideOnMobile: true }, cell: ({ row }) => (
      <span className="font-mono text-[13px] text-fg-muted">{row.original.token_prefix}...</span>
    ) },
    { id: "seen", header: "Last seen", meta: { numeric: true, hideOnMobile: true }, cell: ({ row }) => when(row.original.last_seen_at) },
    { id: "action", header: "", meta: { align: "right" }, cell: ({ row }) => {
      const k = row.original;
      return k.revoked_at ? (
        <StatusPill status="revoked" label={`Revoked ${when(k.revoked_at)}`} />
      ) : (
        <Button size="sm" variant="danger" onClick={() => revoke(k)} loading={revoking === k.id}>
          {revoking !== k.id && <Ban aria-hidden />} Revoke
        </Button>
      );
    } },
  ];

  return (
    <Card className="overflow-hidden">
      <CardHeader title="Kiosk tablets"
        description={<>
          Dedicated clock-in tablets. To add one, sign in as HR on the tablet, open <code className="font-mono">/clock?kiosk=1</code> and
          tap &ldquo;Register this device&rdquo;{offices.length ? "" : " (add an office location first)"}.
        </>} />
      <DataTable label="Kiosk tablets" columns={columns} data={kiosks ?? []} getRowId={(k) => k.id}
        loading={kiosks === null} maxHeight="none"
        empty={{ title: "No kiosks registered", compact: true }} />
    </Card>
  );
}
