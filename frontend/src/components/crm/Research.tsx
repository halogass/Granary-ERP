"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import Link from "next/link";
import {
  Check, ExternalLink, Globe2, Loader2, Mail, MapPin, Newspaper, Phone, Search, UserPlus, X,
} from "lucide-react";
import { researchApi, type ResearchObject, type ResearchPerson, type ResearchRun } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { safeHref } from "@/lib/sanitize";
import { Button, Card, CardBody, CardHeader, IconButton, Input } from "@/components/ui";

const POLL_MS = 4000;
const FIELD_LABEL: Record<string, string> = {
  website: "website", industry: "industry", country: "country", city: "city",
  employee_count: "employees", annual_revenue: "revenue", description: "description",
  position: "position", department: "department", linkedin_url: "LinkedIn",
};

export function isBusy(run: ResearchRun | null | undefined): boolean {
  return !!run && (run.status === "queued" || run.status === "running");
}

/**
 * The research state of one record, shared by the header button and the results panel.
 * Polls while a run is queued or running, and calls `onFinished` once it ends, so the page can
 * reload the fields research may have filled.
 */
export function useResearch(objectType: ResearchObject, recordId: string, onFinished?: () => void) {
  const [run, setRun] = useState<ResearchRun | null>(null);
  const [people, setPeople] = useState<ResearchPerson[]>([]);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState("");
  const wasBusy = useRef(false);

  const load = useCallback(async () => {
    try {
      const r = await researchApi.state(objectType, recordId);
      setRun(r.data.run);
      setPeople(r.data.people ?? []);
      const busy = isBusy(r.data.run);
      if (wasBusy.current && !busy) onFinished?.();
      wasBusy.current = busy;
    } catch {
      /* no research yet, or no access: the panel stays empty */
    }
  }, [objectType, recordId, onFinished]);

  useEffect(() => { load(); }, [load]);
  useEffect(() => {
    if (!isBusy(run)) return;
    const t = setTimeout(load, POLL_MS);
    return () => clearTimeout(t);
  }, [run, load]);

  const start = useCallback(async () => {
    setStarting(true);
    setError("");
    try {
      const r = await researchApi.start(objectType, recordId);
      setRun(r.data.run);
      wasBusy.current = isBusy(r.data.run);
      if (!wasBusy.current) { onFinished?.(); load(); }
    } catch (e) {
      setError(extractApiError(e, "Research could not be started."));
    } finally {
      setStarting(false);
    }
  }, [objectType, recordId, onFinished, load]);

  return { run, people, setPeople, start, starting, error, reload: load };
}

export type ResearchState = ReturnType<typeof useResearch>;

/** The header trigger: "Deep research" on a company, "Research this contact" on a contact. */
export function ResearchButton({ state, label }: { state: ResearchState; label: string }) {
  const busy = state.starting || isBusy(state.run);
  return (
    <Button onClick={state.start} disabled={busy}>
      {busy ? <Loader2 className="animate-spin" aria-hidden /> : <Search aria-hidden />}
      {busy ? "Researching…" : label}
    </Button>
  );
}

function hostOf(url: string): string {
  try { return new URL(url).hostname; } catch { return url; }
}

function when(iso?: string | null): string {
  if (!iso) return "";
  return new Date(iso).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
}

/** What research found: summary, what it filled, sources, news, published channels and people. */
export function ResearchPanel({ state, objectType }: { state: ResearchState; objectType: ResearchObject }) {
  const { run, people, error } = state;
  if (!run && !error) return null;

  return (
    <Card>
      <CardHeader
        title={<span className="inline-flex items-center gap-2"><Globe2 className="size-4 text-tone-violet" aria-hidden /> Web research</span>}
        actions={run && !isBusy(run) ? <span className="text-xs text-fg-muted tabular">{when(run.finished_at || run.created_at)}</span> : undefined} />

      <CardBody className="space-y-4 text-sm">
        {error && <p role="alert" className="rounded-lg bg-tone-red-soft px-3 py-2 text-xs text-tone-red">{error}</p>}
        {isBusy(run) && (
          <p className="flex items-center gap-2 text-xs text-fg-muted">
            <Loader2 className="size-3.5 animate-spin" aria-hidden />
            Searching the web and reading {objectType === "organization" ? "the company's website" : "public profiles"}…
            this usually takes under a minute.
          </p>
        )}
        {run?.status === "failed" && (
          <p className="rounded-lg bg-tone-red-soft px-3 py-2 text-xs text-tone-red">{run.error}</p>
        )}

        {run?.status === "done" && (
          <>
            {run.summary && <p className="whitespace-pre-line leading-relaxed text-fg">{run.summary}</p>}
            <FieldsLine written={run.written} suggested={run.suggested} />
            {objectType === "organization" && <Channels channels={run.channels} />}
            {objectType === "organization" && (
              <People people={people} setPeople={state.setPeople} />
            )}
            {run.news.length > 0 && (
              <div>
                <p className="mb-1.5 flex items-center gap-1.5 text-xs font-semibold text-fg-muted">
                  <Newspaper className="size-3.5" aria-hidden /> News
                </p>
                <ul className="space-y-1">
                  {run.news.map((n) => (
                    <li key={n.url} className="text-xs">
                      <a href={safeHref(n.url)} target="_blank" rel="noreferrer noopener" className="text-fg hover:text-brand-ink hover:underline">{n.title}</a>
                      {n.date && <span className="ml-1.5 text-fg-muted">{n.date}</span>}
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {run.sources.length > 0 && (
              <div className="flex flex-wrap gap-1.5">
                {run.sources.map((s) => (
                  <a key={s.url} href={safeHref(s.url)} target="_blank" rel="noreferrer noopener"
                    className="inline-flex max-w-full items-center gap-1 truncate rounded-lg bg-surface-2 px-2 py-1 text-xs text-fg-muted hover:text-brand-ink">
                    <ExternalLink className="size-3 shrink-0" aria-hidden />
                    <span className="truncate">{s.title || hostOf(s.url)}</span>
                  </a>
                ))}
              </div>
            )}
          </>
        )}
      </CardBody>
    </Card>
  );
}

function FieldsLine({ written, suggested }: { written: Record<string, unknown>; suggested: Record<string, unknown> }) {
  const w = Object.keys(written || {});
  const s = Object.keys(suggested || {});
  if (!w.length && !s.length) return <p className="text-xs text-fg-muted">No empty fields could be filled.</p>;
  return (
    <p className="text-xs text-fg-muted">
      {w.length > 0 && <>Filled <b className="font-medium text-fg">{w.map((f) => FIELD_LABEL[f] ?? f).join(", ")}</b>. </>}
      {s.length > 0 && <>Found but not sure enough to write: {s.map((f) => `${FIELD_LABEL[f] ?? f} (${String(suggested[f])})`).join(", ")}.</>}
    </p>
  );
}

function Channels({ channels }: { channels: ResearchRun["channels"] }) {
  if (!channels || (!channels.phone && !channels.email && !channels.address)) return null;
  return (
    <div className="space-y-1 rounded-lg bg-surface-2 px-3 py-2.5 text-xs text-fg">
      <p className="text-xs font-semibold text-fg-muted">Published contact details</p>
      {channels.phone && <p className="flex items-center gap-1.5"><Phone className="size-3 text-fg-subtle" aria-hidden />{channels.phone}</p>}
      {channels.email && <p className="flex items-center gap-1.5"><Mail className="size-3 text-fg-subtle" aria-hidden />{channels.email}</p>}
      {channels.address && <p className="flex items-start gap-1.5"><MapPin className="mt-0.5 size-3 shrink-0 text-fg-subtle" aria-hidden />{channels.address}</p>}
    </div>
  );
}

/** People found on the web: each one is only a suggestion until someone adds it. */
function People({ people, setPeople }: { people: ResearchPerson[]; setPeople: (p: ResearchPerson[]) => void }) {
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState({ name: "", title: "" });
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");

  if (people.length === 0) return null;

  async function add(p: ResearchPerson) {
    setBusy(p.id);
    setError("");
    try {
      const r = await researchApi.addPerson(p.id, { name: draft.name.trim(), title: draft.title.trim() });
      setPeople(people.map((x) => (x.id === p.id ? r.data.person : x)));
      setEditing(null);
    } catch (e) {
      setError(extractApiError(e, "Could not add this person."));
    } finally {
      setBusy(null);
    }
  }

  async function dismiss(p: ResearchPerson) {
    setBusy(p.id);
    setError("");
    try {
      await researchApi.dismissPerson(p.id);
      setPeople(people.filter((x) => x.id !== p.id));
    } catch (e) {
      setError(extractApiError(e, "Could not dismiss this person."));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div>
      <p className="mb-1.5 text-xs font-semibold text-fg-muted">
        People found · {people.filter((p) => p.status === "pending").length} to review
      </p>
      {error && <p className="mb-2 text-xs text-tone-red">{error}</p>}
      <ul className="divide-y divide-line-soft rounded-lg border border-line">
        {people.map((p) => (
          <li key={p.id} className="px-3 py-2">
            {editing === p.id ? (
              <div className="space-y-1.5">
                <Input aria-label="Name" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
                <Input aria-label="Title" value={draft.title} onChange={(e) => setDraft({ ...draft, title: e.target.value })}
                  placeholder="Job title" />
                <div className="flex gap-1.5">
                  <Button size="sm" variant="primary" onClick={() => add(p)} disabled={busy === p.id || !draft.name.trim()}>
                    {busy === p.id ? <Loader2 className="animate-spin" aria-hidden /> : <Check aria-hidden />} Add as contact
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setEditing(null)}>Cancel</Button>
                </div>
              </div>
            ) : (
              <div className="flex items-start gap-2">
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium text-fg">{p.name}</p>
                  <p className="truncate text-xs text-fg-muted">
                    {p.title || "-"}
                    {p.source_url && (
                      <> · <a href={safeHref(p.source_url)} target="_blank" rel="noreferrer noopener" className="hover:text-brand-ink hover:underline">source</a></>
                    )}
                  </p>
                </div>
                {p.status === "added" && p.contact_id ? (
                  <Link href={`/crm/contacts/${p.contact_id}`} className="shrink-0 text-xs font-medium text-tone-blue hover:underline">Added</Link>
                ) : p.matched_contact_id ? (
                  <Link href={`/crm/contacts/${p.matched_contact_id}`} className="shrink-0 text-xs text-fg-muted hover:underline">Already a contact</Link>
                ) : (
                  <div className="flex shrink-0 gap-1">
                    <IconButton size="sm" icon={UserPlus} label={`Add ${p.name}`} title="Add as contact"
                      onClick={() => { setEditing(p.id); setDraft({ name: p.name, title: p.title ?? "" }); }} />
                    <IconButton size="sm" icon={X} label={`Dismiss ${p.name}`} title="Not relevant"
                      onClick={() => dismiss(p)} disabled={busy === p.id} />
                  </div>
                )}
              </div>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
