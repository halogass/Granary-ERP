"use client";

import { Suspense, useCallback, useEffect, useState } from "react";
import { useSearchParams } from "next/navigation";
import { CheckCheck, Inbox } from "lucide-react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import NaryTabs from "@/components/nary/NaryTabs";
import CardView from "@/components/nary/CardView";
import type { NaryCard } from "@/components/nary/types";
import { Button, Checkbox, EmptyState, FilterChips, Skeleton } from "@/components/ui";

const STATUSES = [["pending", "Waiting"], ["scheduled", "Scheduled for the morning"], ["shadow", "Shadow drafts"], ["applied", "Applied"], ["rejected", "Rejected"],
                  ["failed", "Failed"], ["expired", "Expired"], ["cancelled", "Cancelled before sending"]] as const;

function Approvals() {
  const params = useSearchParams();
  const [status, setStatus] = useState<string>(params.get("status") || "pending");
  const [cards, setCards] = useState<NaryCard[]>([]);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const focus = params.get("card");

  const load = useCallback(async () => {
    setLoading(true);
    try { setCards((await naryApi.cards({ status })).data); }
    catch (e) { setNotice(extractApiError(e)); }
    finally { setLoading(false); }
  }, [status]);
  useEffect(() => { load(); setSelected([]); }, [load]);

  const replace = (c: NaryCard) => setCards((cs) => cs.map((x) => (x.id === c.id ? c : x)));

  const batch = async () => {
    setBusy(true); setNotice(null);
    try {
      const res = await naryApi.batchApply(selected);
      setNotice(`Applied ${res.data.applied.length}` + (res.data.failed.length ? `; ${res.data.failed.length} could not be applied` : ""));
      setSelected([]);
      await load();
    } catch (e) { setNotice(extractApiError(e)); }
    finally { setBusy(false); }
  };

  const ordered = focus ? [...cards].sort((a, b) => (a.id === focus ? -1 : b.id === focus ? 1 : 0)) : cards;
  const selectable = status === "pending" ? ordered.filter((c) => c.status === "pending") : [];
  const allOn = selectable.length > 0 && selectable.every((c) => selected.includes(c.id));

  return (
    <div className="max-w-4xl">
      <NaryTabs subtitle="What Nary wants to do. Apply, edit or reject; each decision teaches it." />
      <div className="mb-3 overflow-hidden rounded-lg border border-line">
        <FilterChips className="border-b-0"
          chips={STATUSES.map(([k, label]) => ({ id: k, label, active: status === k, count: status === k && !loading ? cards.length : undefined }))}
          onToggle={(id) => setStatus(id)}>
          {selectable.length > 1 && (
            <Checkbox label="Select all" checked={allOn}
              onChange={(e) => setSelected(e.target.checked ? selectable.map((c) => c.id) : [])} />
          )}
        </FilterChips>
      </div>
      {notice && <p role="status" className="mb-3 rounded-lg border border-line bg-surface-2 px-3 py-2 text-sm text-fg">{notice}</p>}
      {status === "pending" && selected.length > 0 && (
        <div className={"z-20 mb-3 flex items-center justify-between gap-3 rounded-lg border border-line bg-surface-raised px-4 py-2 shadow-card " +
          "max-md:fixed max-md:inset-x-3 max-md:bottom-[calc(var(--shell-bottom)+0.5rem)] max-md:mb-0 max-md:shadow-pop md:sticky md:top-2"}>
          <span className="text-sm text-fg tabular">{selected.length} selected</span>
          <Button variant="primary" size="sm" loading={busy} onClick={batch}>
            {!busy && <CheckCheck aria-hidden />} Apply all
          </Button>
        </div>
      )}
      {loading ? (
        <div className="space-y-3">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-28 w-full" />)}</div>
      ) : ordered.length === 0 ? (
        <EmptyState icon={Inbox} title="Nothing here" description="Nary has nothing in this list. Pick another status above." />
      ) : (
        <div className={status === "pending" && selected.length > 0 ? "space-y-3 max-md:pb-20" : "space-y-3"}>
          {ordered.map((c) => (
            <CardView key={c.id} card={c} selected={selected.includes(c.id)}
              onSelect={(on) => setSelected((s) => (on ? [...s, c.id] : s.filter((x) => x !== c.id)))}
              onDone={replace} />
          ))}
        </div>
      )}
    </div>
  );
}

export default function NaryApprovalsPage() {
  return <Suspense fallback={null}><Approvals /></Suspense>;
}
