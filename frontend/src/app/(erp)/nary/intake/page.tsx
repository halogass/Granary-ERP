"use client";

import { useCallback, useEffect, useState } from "react";
import { Lock } from "lucide-react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import NaryTabs from "@/components/nary/NaryTabs";
import { HeldChats, IdentityLinks, FilteredList, type Held, type Link, type Verdict } from "@/components/nary/IntakeLists";
import {
  Button, Card, CardBody, EmptyState, Field, Input, Skeleton, StatusPill, Tabs, TabsList, TabsTrigger,
} from "@/components/ui";

interface Arrival {
  id: string; lead_id: string | null; name: string | null; company: string | null; phone: string | null;
  status: string; due_at: string; arrived_at: string | null; created_at: string;
}
interface Exclusion { chat_id: string; phone: string | null; reason: string | null; excluded_at: string | null }

const TABS = [["held", "Business or personal?"], ["filtered", "Filtered"], ["links", "Linked chats"],
              ["arrivals", "Web forms → WhatsApp"], ["personal", "Personal chats"]] as const;
type Tab = typeof TABS[number][0];
const fmt = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "");

export default function NaryIntakePage() {
  const { can } = usePermissions();
  const manage = can("nary:manage");
  const approve = can("nary:approve");
  const [tab, setTab] = useState<Tab>("held");
  const [held, setHeld] = useState<Held[] | null>(null);
  const [verdicts, setVerdicts] = useState<Verdict[] | null>(null);
  const [links, setLinks] = useState<Link[] | null>(null);
  const [arrivals, setArrivals] = useState<Arrival[] | null>(null);
  const [exclusions, setExclusions] = useState<Exclusion[] | null>(null);
  const [newExclusion, setNewExclusion] = useState("");
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    const t = new URLSearchParams(window.location.search).get("tab");
    if (t && TABS.some(([k]) => k === t)) setTab(t as Tab);
  }, []);

  const load = useCallback(async () => {
    setNotice(null);
    try {
      if (tab === "held") setHeld((await naryApi.heldChats()).data);
      if (tab === "filtered") setVerdicts((await naryApi.verdicts(true)).data);
      if (tab === "links") setLinks((await naryApi.identityLinks()).data);
      if (tab === "arrivals") setArrivals((await naryApi.arrivals()).data);
      if (tab === "personal" && manage) setExclusions((await naryApi.exclusions()).data);
    } catch (e) {
      setNotice(extractApiError(e));
    }
  }, [tab, manage]);
  useEffect(() => { load(); }, [load]);

  const run = async (fn: () => Promise<unknown>) => {
    setNotice(null);
    try { await fn(); await load(); } catch (e) { setNotice(extractApiError(e)); }
  };

  return (
    <div className="max-w-5xl">
      <NaryTabs subtitle="Every new conversation, sorted: what Nary filtered out, who it linked to whom, and chats it won't read until you say so." />
      <Tabs value={tab} onValueChange={(v) => setTab(v as Tab)} className="mb-4">
        <TabsList aria-label="Intake lists">
          {TABS.filter(([k]) => k !== "personal" || manage).map(([k, label]) => (
            <TabsTrigger key={k} value={k}>{label}</TabsTrigger>
          ))}
        </TabsList>
      </Tabs>
      {notice && <p role="alert" className="mb-3 text-sm text-tone-red">{notice}</p>}

      {tab === "held" && (held === null ? <Spinner /> : (
        <HeldChats rows={held} canDecide={manage}
          onDecide={(chatId, business) => run(() => naryApi.decideHeld(chatId, business))} />
      ))}
      {tab === "filtered" && (verdicts === null ? <Spinner /> : (
        <FilteredList rows={verdicts} canCorrect={approve}
          onCorrect={(id, verdict) => run(() => naryApi.correctVerdict(id, verdict))} />
      ))}
      {tab === "links" && (links === null ? <Spinner /> : (
        <IdentityLinks rows={links} canUndo={approve} onUndo={(id) => run(() => naryApi.undoLink(id))} />
      ))}
      {tab === "arrivals" && (arrivals === null ? <Spinner /> : arrivals.length === 0 ? (
        <Empty text="No [whatsapp] web forms yet." />
      ) : (
        <ul className="overflow-hidden rounded-lg border border-line bg-surface">
          {arrivals.map((a) => (
            <li key={a.id} className="flex flex-wrap items-center gap-3 border-b border-line-soft px-4 py-2.5 last:border-0 even:bg-surface-2/60">
              <div className="min-w-0 flex-1">
                <p className="text-sm font-medium text-fg">{a.name || "?"}{a.company ? ` · ${a.company}` : ""}</p>
                <p className="text-xs text-fg-muted tabular">{a.phone || "no number"} · form {fmt(a.created_at)}</p>
              </div>
              <StatusPill status={a.status} tone={a.status === "arrived" ? "blue" : a.status === "missed" ? "orange" : "neutral"}
                label={a.status === "arrived" ? `Chat came ${fmt(a.arrived_at)}` : a.status === "missed" ? "No chat came (you were told)" : `Waiting until ${fmt(a.due_at)}`} />
              {a.lead_id && <a href={`/crm/leads/${a.lead_id}`} className="text-[13px] font-medium text-brand-ink hover:underline">Lead</a>}
            </li>
          ))}
        </ul>
      ))}
      {tab === "personal" && manage && (exclusions === null ? <Spinner /> : (
        <div className="space-y-3">
          <Card>
            <CardBody className="flex flex-wrap items-end gap-2">
              <Field label="Mark a number personal" className="min-w-[14rem] flex-1">
                <Input value={newExclusion} onChange={(e) => setNewExclusion(e.target.value)} placeholder="090... or a chat id" autoComplete="off" />
              </Field>
              <Button variant="primary" disabled={!newExclusion.trim()}
                onClick={() => run(async () => { await naryApi.addExclusion(newExclusion.trim()); setNewExclusion(""); })}>
                Mark personal
              </Button>
            </CardBody>
          </Card>
          <p className="inline-flex items-center gap-1 text-xs text-fg-muted">
            <Lock className="size-3.5" aria-hidden /> Messages from these numbers are never stored, read by the AI or answered. Only the number is kept.
          </p>
          {exclusions.length === 0 ? <Empty text="No personal chats." /> : (
            <ul className="overflow-hidden rounded-lg border border-line bg-surface">
              {exclusions.map((x) => (
                <li key={x.chat_id} className="flex flex-wrap items-center justify-between gap-3 border-b border-line-soft px-4 py-2 text-sm last:border-0 even:bg-surface-2/60">
                  <span className="text-fg tabular">{x.phone || x.chat_id}</span>
                  <span className="flex items-center gap-3">
                    <span className="text-xs text-fg-muted">{x.reason?.replace("_", " ")} · {fmt(x.excluded_at)}</span>
                    <Button size="sm"
                      title="Nary reads and answers it again. Its old messages were deleted; they come back only through the WhatsApp history import."
                      onClick={() => run(async () => { await naryApi.includeAgain(x.chat_id); })}>
                      Include again
                    </Button>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      ))}
    </div>
  );
}

function Spinner() {
  return <div className="space-y-2">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-14 w-full" />)}</div>;
}

function Empty({ text }: { text: string }) {
  return <EmptyState title={text} compact />;
}
