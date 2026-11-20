"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { RotateCcw } from "lucide-react";
import { Button, Card, CardBody, CardHeader, Pill, Skeleton, toast } from "@/components/ui";
import { crmApi, type DeletedActivity } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import { formatDate, formatDatetime } from "@/lib/utils";
import { statusLabel } from "@/components/ui/status";

const RECORD_PATHS: Record<string, string> = {
  lead: "/crm/leads", deal: "/crm/deals", contact: "/crm/contacts",
  organization: "/crm/organizations", org: "/crm/organizations",
};

/** Where the activity was logged, as a link to that record. */
function recordHref(a: DeletedActivity): string | null {
  const base = a.entity_type ? RECORD_PATHS[a.entity_type] : undefined;
  return base && a.entity_id ? `${base}/${a.entity_id}` : null;
}

/** Settings, Sales & CRM: the activities people deleted, which the Owner or an admin can restore. */
export default function DeletedActivitiesBlock() {
  const perms = usePermissions();
  const [rows, setRows] = useState<DeletedActivity[] | null>(null);
  const [error, setError] = useState("");
  const [restoring, setRestoring] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const r = await crmApi.listDeletedActivities();
      setRows(r.data?.items ?? []);
      setError("");
    } catch (e) {
      setRows([]);
      setError(extractApiError(e, "The deleted activities could not be loaded."));
    }
  }, []);

  useEffect(() => { if (perms.isAdmin) void load(); }, [perms.isAdmin, load]);

  async function restore(a: DeletedActivity) {
    setRestoring(a.id);
    try {
      await crmApi.restoreDeletedActivity(a.id);
      setRows((rs) => rs && rs.map((r) => (r.id === a.id ? { ...r, restored_at: new Date().toISOString() } : r)));
      toast.success("Activity restored");
    } catch (e) {
      toast.error(extractApiError(e, "The activity could not be restored."));
      void load();
    } finally {
      setRestoring(null);
    }
  }

  if (perms.loaded && !perms.isAdmin) {
    return (
      <Card>
        <CardBody>
          <p className="text-[13px] text-fg-muted">Only the Owner and admins see deleted activities.</p>
        </CardBody>
      </Card>
    );
  }

  return (
    <Card>
      <CardHeader title="Deleted activities"
        description="Activities deleted from a lead, deal, contact or organization are kept here and can be put back." />
      <CardBody className="p-0">
        {rows === null ? (
          <div className="p-4"><Skeleton className="h-20 w-full" /></div>
        ) : error ? (
          <p role="alert" className="p-4 text-[13px] text-tone-red">{error}</p>
        ) : rows.length === 0 ? (
          <p className="p-4 text-[13px] text-fg-muted">No deleted activities.</p>
        ) : (
          <ul className="divide-y divide-line-soft">
            {rows.map((a) => {
              const href = recordHref(a);
              return (
                <li key={a.id} className="flex flex-wrap items-start gap-x-3 gap-y-1.5 px-4 py-2.5">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="min-w-0 truncate text-sm font-medium text-fg">{a.subject || "(No subject)"}</span>
                      {a.type && <Pill className="h-5 px-2 text-[11px]">{statusLabel(a.type)}</Pill>}
                    </div>
                    <p className="mt-0.5 text-xs text-fg-muted">
                      {href ? (
                        <Link href={href} className="text-brand-ink underline-offset-2 hover:underline">
                          {statusLabel(a.entity_type ?? "")}
                        </Link>
                      ) : a.entity_type ? statusLabel(a.entity_type) : "No record"}
                      {" · "}Deleted by {a.voided_by || "unknown"}, <span className="tabular">{formatDatetime(a.voided_at)}</span>
                    </p>
                    {a.reason && <p className="mt-0.5 text-xs text-fg">Reason: {a.reason}</p>}
                  </div>
                  {a.restored_at ? (
                    <span className="shrink-0 text-xs text-fg-muted">Restored {formatDate(a.restored_at)}</span>
                  ) : (
                    <Button size="sm" className="shrink-0" loading={restoring === a.id} disabled={!!restoring}
                      onClick={() => restore(a)}>
                      {restoring !== a.id && <RotateCcw aria-hidden />} Restore
                    </Button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </CardBody>
    </Card>
  );
}
