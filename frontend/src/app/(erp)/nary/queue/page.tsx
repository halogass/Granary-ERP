"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import NaryTabs from "@/components/nary/NaryTabs";
import { KIND_LABELS, workQueueApi, type QueueItem, type QueueResponse, type Urgency } from "@/components/nary/workQueue";
import { Button, DataTable, FilterChips, Pill, Select, type ColumnDef, type Tone } from "@/components/ui";

const URGENCY: { id: Urgency; label: string; tone: Tone }[] = [
  { id: "urgent", label: "Urgent", tone: "red" },
  { id: "today", label: "Today", tone: "orange" },
  { id: "normal", label: "Later", tone: "neutral" },
];

const fmtTime = (iso: string) =>
  new Date(iso).toLocaleString("en-GB", { weekday: "short", day: "2-digit", month: "short", hour: "2-digit", minute: "2-digit" });

/** Who or what an item is about: its name, then the record ids it carries. */
function Subject({ s }: { s: Record<string, unknown> }) {
  const name = typeof s.name === "string" ? s.name : "";
  const ids = Object.entries(s).filter(([k, v]) => k !== "name" && k !== "last_message" && typeof v === "string" && v);
  return (
    <div className="min-w-0">
      {name && <p className="truncate text-fg">{name}</p>}
      <p className="truncate font-mono text-[11px] text-fg-muted">{ids.map(([k, v]) => `${k}: ${v}`).join(" · ")}</p>
    </div>
  );
}

/**
 * Nary > Work queue (3 Nov 2026, Owner and system admins): what is waiting while internal Nary is paused, and the
 * tool your AI client uses next. The same list your AI client reads with nary_work_queue.
 */
export default function WorkQueuePage() {
  const [data, setData] = useState<QueueResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [urgency, setUrgency] = useState<Urgency | null>(null);
  const [kind, setKind] = useState("");
  const [copied, setCopied] = useState(false);

  const load = useCallback(async () => {
    try {
      const r = await workQueueApi.list({ urgency: urgency ?? undefined, kinds: kind || undefined, limit: 500 });
      setData(r.data);
      setError(null);
    } catch (e) {
      const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      setError(typeof detail === "string" ? detail : "The work queue could not be loaded.");
      setData({ total: 0, matching: 0, shown: 0, by_urgency: { urgent: 0, today: 0, normal: 0 }, by_kind: {},
                nary_paused: false, scheduled_task_prompt: "", items: [] });
    }
  }, [urgency, kind]);
  useEffect(() => { load(); }, [load]);

  const columns = useMemo<ColumnDef<QueueItem>[]>(() => [
    {
      accessorKey: "urgency", header: "When",
      cell: (c) => {
        const u = URGENCY.find((x) => x.id === c.getValue<Urgency>());
        return <Pill tone={u?.tone ?? "neutral"}>{u?.label ?? c.getValue<string>()}</Pill>;
      },
    },
    {
      accessorKey: "title", header: "What is waiting",
      cell: (c) => (
        <div className="min-w-0">
          <p className="font-medium text-fg">{c.row.original.title}</p>
          <p className="text-xs text-fg-muted">{KIND_LABELS[c.row.original.kind] ?? c.row.original.kind} · {c.row.original.hint}</p>
        </div>
      ),
    },
    { id: "subject", header: "Who or what", meta: { hideOnMobile: true }, cell: (c) => <Subject s={c.row.original.subject} /> },
    { accessorKey: "since", header: "Since", meta: { numeric: true, hideOnMobile: true }, cell: (c) => fmtTime(c.getValue<string>()) },
    {
      id: "next", header: "Next tool", meta: { hideOnMobile: true },
      cell: (c) => c.row.original.suggested_tool
        ? <code className="text-xs">{c.row.original.suggested_tool}</code>
        : <span className="text-fg-subtle">-</span>,
    },
    {
      id: "open", header: "",
      cell: (c) => <Link href={c.row.original.link} className="text-xs font-medium text-brand hover:underline">Open</Link>,
    },
  ], []);

  const chips = URGENCY.map((u) => ({ id: u.id, label: u.label, active: urgency === u.id, count: data?.by_urgency?.[u.id] ?? 0 }));
  const kindOptions = [{ value: "", label: "All kinds" },
    ...Object.entries(KIND_LABELS).map(([value, label]) => ({ value, label: `${label}${data?.by_kind?.[value] ? ` (${data.by_kind[value]})` : ""}` }))];

  const copyPrompt = async () => {
    if (!data?.scheduled_task_prompt) return;
    try { await navigator.clipboard.writeText(data.scheduled_task_prompt); setCopied(true); } catch { setCopied(false); }
  };

  return (
    <div className="max-w-6xl">
      <NaryTabs
        subtitle={data?.nary_paused
          ? "Nary is paused: this is what waits for you and your AI client, with the next tool for each item."
          : "What waits for you and your AI client, with the next tool for each item."}
        actions={<Button variant="secondary" size="sm" onClick={load}>Refresh</Button>}
      />
      {error && <p role="alert" className="mb-3 text-sm text-tone-red">{error}</p>}
      <div className="overflow-hidden rounded-lg border border-line bg-surface">
        <FilterChips chips={chips} onToggle={(id) => setUrgency((u) => (u === id ? null : (id as Urgency)))}
          onClear={() => { setUrgency(null); setKind(""); }}>
          <Select aria-label="Kind" value={kind} onChange={(e) => setKind(e.target.value)} options={kindOptions} />
        </FilterChips>
        <DataTable label="Work queue" columns={columns} data={data?.items ?? []} getRowId={(r) => r.id} loading={data === null}
          empty={{ title: "Nothing waiting", description: "New customer messages, leads, due follow-ups and cards appear here." }} />
      </div>
      {data?.scheduled_task_prompt && (
        <section className="mt-4 rounded-lg border border-line bg-surface p-4">
          <h2 className="text-sm font-semibold text-fg">Scheduled task in your AI client</h2>
          <p className="mt-1 text-xs text-fg-muted">Paste this prompt into a scheduled task (for example every working day at 08:15, 13:15 and 17:15).</p>
          <pre className="mt-2 whitespace-pre-wrap break-words rounded-lg bg-surface-2 p-3 text-xs text-fg">{data.scheduled_task_prompt}</pre>
          <Button className="mt-2" variant="secondary" size="sm" onClick={copyPrompt}>{copied ? "Copied" : "Copy prompt"}</Button>
        </section>
      )}
    </div>
  );
}
