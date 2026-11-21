"use client";

import { useState } from "react";
import { Check, Loader2, Pencil, RotateCcw, X, ShieldAlert } from "lucide-react";
import { naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { settleCard } from "@/lib/longWork";
import { Button, Checkbox, Input, StatusPill, Textarea } from "@/components/ui";
import { cn } from "@/lib/utils";
import { LEVEL_LABELS, type NaryCard } from "./types";
import {
  cardCurrency, cardDiff, cardPayload, cardText, cardTitle, DiffValue, editedMessagePayload, fieldLabel, KickoffDetails,
  messageEdit, shown,
} from "./cardDetails";

const STATUS_LABEL: Record<string, string> = {
  pending: "Waiting for you", scheduled: "Scheduled", shadow: "Shadow draft", applying: "Applying",
};

/** One approval card: what Nary wants to do, why, and the change it makes. Apply / Edit / Reject. */
export default function CardView({ card, selected, onSelect, onDone }: {
  card: NaryCard; selected?: boolean; onSelect?: (on: boolean) => void; onDone: (c: NaryCard) => void;
}) {
  const [mode, setMode] = useState<"view" | "edit" | "reject">("view");
  // a message card (WhatsApp/email reply, an agent's customer message) is edited as text; other kinds as JSON
  const message = messageEdit(card);
  const [msgText, setMsgText] = useState(message?.text ?? "");
  const [msgSubject, setMsgSubject] = useState(message?.subject ?? "");
  const [draft, setDraft] = useState(JSON.stringify(cardPayload(card), null, 2));
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // fix-longapply: the apply outlasted the request; it finishes on its own and the card is polled
  const [working, setWorking] = useState(card.status === "applying");
  // a failed proposal card (its work stopped half-way) can be applied again; the retry reuses what it made
  const retryable = card.status === "failed" && card.action.startsWith("proposal.");
  // A scheduled card is Nary's own WhatsApp message waiting for working hours: "Send now" or "Drop".
  const scheduled = card.status === "scheduled";
  const pending = card.status === "pending" || scheduled;
  const when = scheduled && typeof card.result?.when === "string" ? (card.result.when as string) : null;
  const text = cardText(card);
  const diff = cardDiff(card);
  const currency = cardCurrency(card, diff);
  const title = cardTitle(card);
  // 21 Nov 2026: the kickoff has its own layout (project, PM, customer, tasks, handoff package)
  const kickoff = card.action === "handoff.kickoff";

  // the editor starts from what the card shows now (its last edit, if any)
  const startEdit = () => {
    setMsgText(message?.text ?? "");
    setMsgSubject(message?.subject ?? "");
    setDraft(JSON.stringify(cardPayload(card), null, 2));
    setError(null);
    setMode("edit");
  };

  const decide = async (decision: "apply" | "reject") => {
    setBusy(true); setError(null);
    try {
      let edited: unknown;
      if (decision === "apply" && mode === "edit") {
        if (message) {
          if (!msgText.trim()) { setError("The message can't be empty."); setBusy(false); return; }
          edited = editedMessagePayload(card, { text: msgText, subject: message.subject !== null ? msgSubject : null });
        } else {
          try { edited = JSON.parse(draft); } catch { setError("The edited payload isn't valid JSON."); setBusy(false); return; }
        }
      }
      const res = await naryApi.decide(card.id, { decision, edited_payload: edited, reason: decision === "reject" ? reason : undefined });
      const final = await settleCard(res, () => setWorking(true));
      if (final.status === "failed") setError(final.detail || final.error || "It couldn't be applied.");
      onDone(final);
      setMode("view");
      // the Waiting badge in the menu counts again now rather than at its next minute
      window.dispatchEvent(new Event("nary:changed"));
    } catch (e) {
      setError(extractApiError(e));
    } finally { setBusy(false); setWorking(false); }
  };

  return (
    <article className={cn("rounded-lg border border-line bg-surface p-3 shadow-card md:p-4", selected && "bg-brand/[0.07]")}>
      <header className="flex items-start gap-3">
        {pending && onSelect && (
          <span className="mt-0.5">
            <Checkbox aria-label={`Select ${title}`} checked={!!selected} onChange={(e) => onSelect(e.target.checked)} />
          </span>
        )}
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold text-fg">{title}</p>
          <p className="text-xs text-fg-muted">
            {card.origin === "mcp" ? "Connected agent" : "Nary"} · {LEVEL_LABELS[card.level] ?? card.level}
            {card.sop_ref && <> · <span className="font-mono">{card.sop_ref}</span></>} · <span className="tabular">{new Date(card.created_at).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" })}</span>
          </p>
        </div>
        <StatusPill family="nary_card" status={card.status} label={STATUS_LABEL[card.status]} />
      </header>

      {scheduled ? (
        <p className="mt-2 inline-flex items-center gap-1 text-xs text-tone-orange">
          <ShieldAlert className="size-3.5" aria-hidden /> {card.result?.pacing ? "Over Nary's WhatsApp limit" : "Outside working hours"}: goes out {when ?? "at the start of the next working day"}, or send it now.
        </p>
      ) : card.customer_facing && (
        <p className="mt-2 inline-flex items-center gap-1 text-xs text-tone-orange">
          <ShieldAlert className="size-3.5" aria-hidden /> Reaches a customer.{card.status === "shadow" ? " A draft only: nothing was sent." : ""}
        </p>
      )}
      {card.rationale && <p className="mt-2 whitespace-pre-wrap text-sm text-fg">{card.rationale}</p>}
      {text && <blockquote className="mt-2 whitespace-pre-wrap rounded-r-lg border-l-2 border-line-strong bg-surface-2 px-3 py-2 text-sm text-fg">{text}</blockquote>}

      {kickoff ? <KickoffDetails card={card} /> : diff?.after && (
        <dl className="mt-3 grid grid-cols-1 gap-x-3 gap-y-1 text-[13px] sm:grid-cols-[auto,1fr]">
          {Object.entries(diff.after).filter(([k, v]) => shown(v) || shown(diff.before?.[k])).map(([k, v]) => (
            <div key={k} className="contents">
              <dt className="text-xs text-fg-subtle">{fieldLabel(k)}</dt>
              <dd className="min-w-0 break-words text-fg">
                {diff.before && k in diff.before && (
                  <span className="mr-1.5 text-fg-subtle line-through"><DiffValue value={diff.before[k]} field={k} currency={currency} /></span>
                )}
                <DiffValue value={v} field={k} currency={currency} />
              </dd>
            </div>
          ))}
        </dl>
      )}
      {card.diff?.error && <p className="mt-2 text-xs text-tone-red">{card.diff.error}</p>}
      {card.reject_reason && <p className="mt-2 text-xs text-fg-muted">Rejected: {card.reject_reason}</p>}
      {card.error && <p className="mt-2 text-xs text-tone-red">{card.error}</p>}
      {card.actual_text && (
        <p className="mt-2 text-xs text-fg-muted">Actually sent ({card.diff_ratio}% different): {card.actual_text}</p>
      )}

      {mode === "edit" && message && (
        <div className="mt-3 space-y-2">
          {message.subject !== null && (
            <Input value={msgSubject} onChange={(e) => setMsgSubject(e.target.value)} aria-label="Subject"
              placeholder="Subject" autoComplete="off" />
          )}
          <Textarea value={msgText} onChange={(e) => setMsgText(e.target.value)} rows={6} aria-label="Message" />
        </div>
      )}
      {mode === "edit" && !message && (
        <div className="mt-3">
          <p className="mb-1 text-xs text-fg-muted">Advanced: this kind of card has no form, so its payload is edited as JSON.</p>
          <Textarea value={draft} onChange={(e) => setDraft(e.target.value)} rows={8} aria-label="Edited payload"
            className="font-mono md:text-xs" />
        </div>
      )}
      {mode === "reject" && (
        <Input value={reason} onChange={(e) => setReason(e.target.value)} placeholder="Why? Nary learns from it"
          aria-label="Reason for rejecting" className="mt-3" autoComplete="off" />
      )}
      {working && (
        <p className="mt-2 inline-flex items-center gap-1.5 text-xs text-fg-muted" role="status">
          <Loader2 className="size-3.5 animate-spin" aria-hidden /> Working... this can take a few minutes.
        </p>
      )}
      {error && <p className="mt-2 text-xs text-tone-red">{error}</p>}
      {retryable && !working && (
        <footer className="mt-3 flex flex-wrap items-center gap-2">
          <Button variant="primary" loading={busy} onClick={() => decide("apply")}>
            {!busy && <RotateCcw aria-hidden />} Try again
          </Button>
        </footer>
      )}

      {pending && (
        <footer className="mt-3 flex flex-wrap items-center gap-2 max-sm:[&>button]:flex-1">
          {mode !== "reject" && (
            <Button variant="primary" loading={busy} onClick={() => decide("apply")}>
              {!busy && <Check aria-hidden />} {mode === "edit" ? (scheduled ? "Send edited now" : "Apply edited") : (scheduled ? "Send now" : "Apply")}
            </Button>
          )}
          {mode === "view" && (
            <Button onClick={startEdit}><Pencil aria-hidden /> Edit</Button>
          )}
          {mode === "reject" ? (
            <Button variant="danger" disabled={busy || !reason.trim()} onClick={() => decide("reject")}>
              <X aria-hidden /> {scheduled ? "Drop" : "Reject"}
            </Button>
          ) : (
            <Button variant="ghost" onClick={() => setMode("reject")}>
              <X aria-hidden /> {scheduled ? "Drop" : "Reject"}
            </Button>
          )}
          {mode !== "view" && (
            <Button variant="ghost" onClick={() => { setMode("view"); setError(null); }}>Cancel</Button>
          )}
        </footer>
      )}
    </article>
  );
}
