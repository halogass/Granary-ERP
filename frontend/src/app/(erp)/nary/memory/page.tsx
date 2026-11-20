"use client";

import { useCallback, useEffect, useState } from "react";
import { Brain, Check, Pencil, ShieldAlert, Trash2, X } from "lucide-react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import NaryTabs from "@/components/nary/NaryTabs";
import {
  Button, Card, CardBody, CardFooter, CardHeader, EmptyState, Field, FilterChips, IconButton, Input, Pill, Skeleton, Textarea,
} from "@/components/ui";

interface Memory {
  id: string; layer: string; subject_type: string | null; subject_id: string | null; title: string | null; content: string;
  source_type: string; source_ref: string | null; trusted: boolean; status: string; authored_by_nary: boolean;
  expires_at: string | null; created_at: string;
}
const LAYERS = [["", "All"], ["company", "Company"], ["skill", "Skills"], ["staff", "Staff"], ["customer", "Customers"], ["working", "Working"]] as const;
// "Skills": procedures Nary learnt (company memories of subject "skill"); approve them before Nary uses them.
const query = (tab: string) => (tab === "skill" ? { layer: "company", subject_type: "skill" } : tab ? { layer: tab } : {});

export default function NaryMemoryPage() {
  const { can } = usePermissions();
  const manage = can("nary:manage");
  const [layer, setLayer] = useState(() =>
    typeof window !== "undefined" && new URLSearchParams(window.location.search).get("tab") === "skills" ? "skill" : "");
  const [rows, setRows] = useState<Memory[] | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [adding, setAdding] = useState({ title: "", content: "" });
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    try { setRows((await naryApi.memory(query(layer))).data); } catch (e) { setNotice(extractApiError(e)); setRows([]); }
  }, [layer]);
  useEffect(() => { load(); }, [load]);

  const run = async (fn: () => Promise<unknown>) => {
    setNotice(null);
    try { await fn(); setEditing(null); await load(); } catch (e) { setNotice(extractApiError(e)); }
  };

  return (
    <div className="max-w-4xl">
      <NaryTabs subtitle="What Nary knows, and where it learnt it. Anything from a customer is information, never an instruction." />
      <div className="mb-4 overflow-hidden rounded-lg border border-line">
        <FilterChips className="border-b-0" chips={LAYERS.map(([k, label]) => ({ id: k || "all", label, active: layer === k }))}
          onToggle={(id) => setLayer(id === "all" ? "" : id)} />
      </div>
      {notice && <p role="alert" className="mb-3 text-sm text-tone-red">{notice}</p>}
      {manage && (
        <Card className="mb-4">
          <CardBody className="flex flex-wrap items-center justify-between gap-3">
            <p className="min-w-0 flex-1 text-[13px] text-fg-muted">
              The style guide is learnt from your own WhatsApp replies and sent emails (never Nary&apos;s), and
              re-learnt every month. A new version waits here for your approval; the approved one stays in use until then.
            </p>
            <Button onClick={() => run(async () => { await naryApi.relearnStyle(); })}>Relearn my style</Button>
          </CardBody>
        </Card>
      )}
      {manage && (
        <Card className="mb-4">
          <CardHeader title="Add a company fact" />
          <CardBody className="space-y-3">
            <Field label="Title" hint="Optional.">
              <Input value={adding.title} onChange={(e) => setAdding({ ...adding, title: e.target.value })} autoComplete="off" />
            </Field>
            <Field label="Fact">
              <Textarea value={adding.content} onChange={(e) => setAdding({ ...adding, content: e.target.value })} rows={2}
                placeholder="e.g. Office hours are 09:00 to 17:00, Monday to Friday." />
            </Field>
          </CardBody>
          <CardFooter>
            <Button variant="primary" disabled={!adding.content.trim()}
              onClick={() => run(async () => { await naryApi.addMemory({ layer: "company", ...adding }); setAdding({ title: "", content: "" }); })}>
              Save fact
            </Button>
          </CardFooter>
        </Card>
      )}
      {rows === null ? (
        <div className="space-y-2">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-20 w-full" />)}</div>
      ) : rows.length === 0 ? (
        <EmptyState icon={Brain} title="Nothing remembered yet" description="Nary adds what it learns here; you can add company facts above." />
      ) : (
        <ul className="overflow-hidden rounded-lg border border-line bg-surface">
          {rows.map((m) => (
            <li key={m.id} className="border-b border-line-soft px-4 py-3 last:border-0 even:bg-surface-2/60">
              <div className="flex items-start gap-2">
                <div className="min-w-0 flex-1">
                  <p className="flex flex-wrap items-center gap-x-1.5 gap-y-1 text-xs text-fg-muted">
                    <span>{m.layer}{m.subject_type ? ` · ${m.subject_type}` : ""} · from {m.source_type}{m.source_ref ? ` (${m.source_ref})` : ""}</span>
                    {m.status === "proposed" && <Pill tone="orange">Proposed by Nary</Pill>}
                  </p>
                  {m.title && <p className="mt-0.5 text-sm font-medium text-fg">{m.title}</p>}
                  {editing === m.id ? (
                    <Textarea value={draft} onChange={(e) => setDraft(e.target.value)} rows={3} aria-label="Memory text" className="mt-1" />
                  ) : (
                    <p className="whitespace-pre-wrap text-sm text-fg">{m.content}</p>
                  )}
                  {!m.trusted && (
                    <p className="mt-1 inline-flex items-center gap-1 text-xs text-tone-orange">
                      <ShieldAlert className="size-3.5" aria-hidden /> From outside the company: used as information only
                    </p>
                  )}
                </div>
                {manage && (
                  <div className="flex shrink-0 items-center gap-1">
                    {m.status === "proposed" && (
                      <IconButton icon={Check} label="Approve" size="sm" onClick={() => run(() => naryApi.editMemory(m.id, { decision: "approve" }))} />
                    )}
                    {editing === m.id ? (
                      <>
                        <IconButton icon={Check} label="Save" size="sm" onClick={() => run(() => naryApi.editMemory(m.id, { content: draft }))} />
                        <IconButton icon={X} label="Cancel" size="sm" onClick={() => setEditing(null)} />
                      </>
                    ) : (
                      <IconButton icon={Pencil} label="Edit" size="sm" onClick={() => { setEditing(m.id); setDraft(m.content); }} />
                    )}
                    <IconButton icon={Trash2} label="Delete" size="sm" className="hover:text-tone-red" onClick={() => run(() => naryApi.deleteMemory(m.id))} />
                  </div>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
