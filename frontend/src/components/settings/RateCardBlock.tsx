"use client";

import { useCallback, useEffect, useState } from "react";
import { Search } from "lucide-react";
import { Button, Card, CardBody, CardHeader, toast } from "@/components/ui";
import { salesDocumentsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import RateCard, { type Rate } from "@/components/proposals/RateCard";

/** Settings, Sales & CRM, Proposals: rate card (internal; sales:view_costs). */
export default function RateCardBlock() {
  const { can, loaded } = usePermissions();
  const [rates, setRates] = useState<Rate[] | null>(null);
  const load = useCallback(async () => { setRates((await salesDocumentsApi.rateCard()).data); }, []);
  useEffect(() => { if (loaded && can("sales:view_costs")) load().catch(() => setRates(null)); }, [load, loaded, can]);
  if (!rates) return null;
  return (
    <Card>
      <CardHeader title="Rate card: cost per man-day (internal)"
        actions={can("sales:manage_costs") && (
          <Button size="sm" variant="ghost" onClick={async () => {
            try { const r = await salesDocumentsApi.researchRates(); toast.success(`Nary proposed ${r.data.proposed} change(s) for your approval.`); }
            catch (e) { toast.error(extractApiError(e)); }
          }}><Search aria-hidden /> Ask Nary to research current rates</Button>
        )} />
      <CardBody>
        <RateCard rates={rates} canManage={can("sales:manage_costs")}
          onAdd={async (r) => { try { await salesDocumentsApi.addRate(r); await load(); toast.success("Rate set."); } catch (e) { toast.error(extractApiError(e)); } }} />
      </CardBody>
    </Card>
  );
}
