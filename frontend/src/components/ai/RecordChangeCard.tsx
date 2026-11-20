"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { Check, Loader2, X, ArrowRight, Database, ExternalLink } from "lucide-react";
import { crmRecordsApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import type { ProposalMeta, RecordChange, ResolveProposal } from "./types";

const OBJECT_LABEL: Record<string, string> = {
  organization: "Organization",
  contact: "Contact",
  lead: "Lead",
  deal: "Deal",
  activity: "Activity",
};

const RECORD_PATH: Record<string, string> = {
  organization: "/crm/organizations",
  contact: "/crm/contacts",
  lead: "/crm/leads",
  deal: "/crm/deals",
};

function fmt(v: unknown): string {
  if (v === null || v === undefined || v === "") return "-";
  if (typeof v === "boolean") return v ? "Yes" : "No";
  if (Array.isArray(v)) return v.join(", ");
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

function humanize(field: string): string {
  return field.replace(/_id$/, "").replace(/_/g, " ").replace(/^\w/, (c) => c.toUpperCase());
}

/**
 * Confirm card for an AI-proposed CRM change.
 *
 * The assistant only ever *prepares* this; the change is written when the user clicks
 * Apply, which posts to /crm/ai/apply where permissions are re-checked server-side.
 * A card reopened from history shows its saved outcome instead of a live Apply button.
 */
export default function RecordChangeCard({ proposal, onResolved }: {
  proposal: RecordChange & ProposalMeta;
  onResolved?: ResolveProposal;
}) {
  const [busy, setBusy] = useState(false);
  const [applied, setApplied] = useState<{ id: string; label: string } | null>(
    proposal.status === "applied"
      ? { id: proposal.result?.id || proposal.record_id || "", label: proposal.result?.label || proposal.label || "record" }
      : null);
  const [cancelled, setCancelled] = useState(proposal.status === "discarded");
  const [error, setError] = useState("");
  // The diff was taken when the assistant answered; refresh "before" so an old card isn't stale.
  const [before, setBefore] = useState<Record<string, unknown>>(proposal.before || {});

  const isCreate = proposal.action === "create";
  // An id shows the record's name; "@org" means the record another card of this answer creates.
  const show = (f: string, v: unknown) => proposal.display?.[f] ?? fmt(v);
  const waitsFor = Object.values(proposal.after).some((v) => typeof v === "string" && v.startsWith("@"));
  const changedFields = Object.keys(proposal.after);
  const pending = !applied && !cancelled;

  useEffect(() => {
    if (!pending || isCreate || !proposal.record_id) return;
    let live = true;
    (async () => {
      try {
        const res = await crmRecordsApi.previewChange({
          object_type: proposal.object_type, action: proposal.action,
          record_id: proposal.record_id, fields: proposal.after,
        });
        if (live && res?.data?.before) setBefore(res.data.before);
      } catch { /* keep the stored diff */ }
    })();
    return () => { live = false; };
    // Only on mount: the card's proposal never changes while it is pending.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function apply() {
    setBusy(true); setError("");
    try {
      const res = await crmRecordsApi.applyChange({
        object_type: proposal.object_type,
        action: proposal.action,
        record_id: proposal.record_id ?? undefined,
        fields: proposal.after,
        proposal_id: proposal.proposal_id,
      });
      const done = { id: res.data.id, label: res.data.label || "record" };
      setApplied(done);
      onResolved?.("applied", done);
    } catch (e) {
      setError(extractApiError(e, "Could not apply the change."));
    } finally { setBusy(false); }
  }

  const href = RECORD_PATH[proposal.object_type];

  if (cancelled) {
    return (
      <div className="mt-2 rounded-lg border border-line bg-surface-2 px-4 py-2.5 text-xs text-fg-subtle">
        Change discarded: nothing was saved.
      </div>
    );
  }

  if (applied) {
    return (
      <div className="mt-2 rounded-lg border border-tone-blue/30 bg-tone-blue-soft px-4 py-3">
        <p className="flex items-center gap-1.5 text-sm font-semibold text-tone-blue">
          <Check className="h-4 w-4" />
          {isCreate ? "Created" : "Updated"} {OBJECT_LABEL[proposal.object_type] ?? "record"}
        </p>
        <p className="mt-0.5 text-xs text-tone-blue">{applied.label}</p>
        {href && applied.id && (
          <Link href={`${href}/${applied.id}`}
            className="mt-1.5 inline-flex items-center gap-1 text-xs font-semibold text-tone-blue hover:underline">
            Open record <ExternalLink className="h-3 w-3" />
          </Link>
        )}
      </div>
    );
  }

  return (
    <div className="mt-2 overflow-hidden rounded-lg border border-line bg-surface">
      <div className="flex items-center gap-2 border-b border-line px-4 py-2.5">
        <div className="flex h-6 w-6 items-center justify-center rounded-lg bg-brand/10">
          <Database className="h-3.5 w-3.5 text-brand-ink" />
        </div>
        <p className="text-sm font-semibold text-fg">
          {isCreate ? "Create" : "Update"} {OBJECT_LABEL[proposal.object_type] ?? proposal.object_type}
        </p>
        {proposal.label && (
          <span className="truncate text-xs text-fg-subtle">· {proposal.label}</span>
        )}
      </div>

      <div className="px-4 py-3">
        {changedFields.length === 0 ? (
          <p className="text-xs text-fg-subtle">No fields to change.</p>
        ) : (
          <table className="w-full text-xs">
            <tbody className="divide-y divide-line-soft">
              {changedFields.map((f) => (
                <tr key={f}>
                  <td className="py-1.5 pr-3 align-top font-semibold text-fg-subtle">{humanize(f)}</td>
                  {isCreate ? (
                    <td className="py-1.5 text-fg break-words">{show(f, proposal.after[f])}</td>
                  ) : (
                    <td className="py-1.5">
                      <span className="text-fg-subtle line-through break-words">
                        {fmt(before[f])}
                      </span>
                      <ArrowRight className="mx-1.5 inline h-3 w-3 text-fg-subtle" />
                      <span className="font-semibold text-fg break-words">
                        {show(f, proposal.after[f])}
                      </span>
                    </td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        )}

        {waitsFor && (
          <p className="mt-2 text-[11px] text-fg-muted">Links to a record created by a card above: confirm that one first.</p>
        )}
        {error && (
          <div className="mt-3 rounded-lg border border-tone-red/30 bg-tone-red-soft px-3 py-2 text-xs text-tone-red">
            {error}
          </div>
        )}

        <div className="mt-3 flex items-center justify-end gap-2">
          <button type="button" onClick={() => { setCancelled(true); onResolved?.("discarded"); }} disabled={busy}
            className="flex items-center gap-1 rounded-lg border border-line px-3 py-1.5 text-xs font-semibold text-fg-muted transition-all hover:bg-surface-2 disabled:opacity-40">
            <X className="h-3 w-3" /> Discard
          </button>
          <button type="button" onClick={apply} disabled={busy || changedFields.length === 0}
            className="flex items-center gap-1.5 rounded-lg bg-brand px-4 py-1.5 text-xs font-semibold text-brand-on transition-colors hover:bg-brand-hover disabled:opacity-50">
            {busy ? <Loader2 className="h-3 w-3 animate-spin" /> : <Check className="h-3 w-3" />}
            {isCreate ? "Create" : "Apply"}
          </button>
        </div>
      </div>
    </div>
  );
}
