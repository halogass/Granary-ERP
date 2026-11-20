"use client";

import { useCallback, useEffect, useState } from "react";
import { Send } from "lucide-react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import NaryTabs from "@/components/nary/NaryTabs";
import { ReportView, SopRevisions, type ReportData, type SopBrief, type SopDraft } from "@/components/nary/WeeklyReport";
import { Button, Skeleton } from "@/components/ui";

export default function NaryReportPage() {
  const { can } = usePermissions();
  const manage = can("nary:manage");
  const [data, setData] = useState<ReportData | null>(null);
  const [sops, setSops] = useState<{ briefs: SopBrief[]; drafts: SopDraft[] } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [r, s] = await Promise.all([naryApi.report(), naryApi.sopRevisions()]);
      setData(r.data);
      setSops(s.data);
    } catch (e) { setNotice(extractApiError(e)); }
  }, []);
  useEffect(() => { load(); }, [load]);

  const run = async (fn: () => Promise<unknown>, done: string) => {
    setNotice(null);
    try { await fn(); setNotice(done); await load(); } catch (e) { setNotice(extractApiError(e)); }
  };

  return (
    <div className="max-w-5xl space-y-4">
      <NaryTabs subtitle="The weekly report (it also comes with Monday's morning brief), and SOP revisions for working with Nary."
        actions={manage ? (
          <Button size="sm" onClick={() => run(() => naryApi.sendReport(), "The report was sent to you.")}>
            <Send aria-hidden /> Send it to me now
          </Button>
        ) : undefined} />
      {notice && <p role="status" className="rounded-lg border border-line bg-surface-2 px-3 py-2 text-sm text-fg">{notice}</p>}
      {data === null ? <Skeleton className="h-48 w-full" /> : <ReportView data={data} />}
      {sops && (
        <SopRevisions briefs={sops.briefs} drafts={sops.drafts} canManage={manage}
          onDraft={(codes) => run(() => naryApi.draftSops(codes), "Nary is drafting them; you'll be told when they're ready (a few minutes each).")} />
      )}
    </div>
  );
}
