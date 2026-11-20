"use client";

import { useState } from "react";
import Link from "next/link";
import { X, Check, Loader2, FileText, AlertTriangle, ExternalLink, BellPlus, Globe2, Send, Link2, Paperclip, CalendarPlus, Film } from "lucide-react";
import { salesApi, financeApi, kbApi, remindersApi, researchApi, naryApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { isStillWorking, LongWorkError, settleCard } from "@/lib/longWork";
import { cardZone, fromZonedInput, toZonedInput, zoneLabel } from "@/lib/zonedTime";
import type {
  Proposal, DocProposal, FollowUpProposal, ProposalItem, ProposalMeta, ResearchProposal, ResolveProposal, SopStep,
  CustomerMessageProposal, ChatLinkProposal, ReminderChangeProposal, MeetingProposal, NaryActionProposal, CardFile,
} from "./types";
import RecordChangeCard from "./RecordChangeCard";
import AuthImage from "@/components/ui/AuthImage";
import ProposalVisualCard, { isProposalCardAction } from "./ProposalVisualCard";

function fmtMoney(currency: string, n: number) {
  try { return new Intl.NumberFormat("en-US", { style: "currency", currency, maximumFractionDigits: 0 }).format(n); }
  catch { return `${currency} ${Math.round(n).toLocaleString()}`; }
}

function lineAmount(it: ProposalItem): number {
  if (it.amount != null) return Number(it.amount);
  const gross = (Number(it.quantity) || 0) * (Number(it.unit_price) || 0);
  return gross * (1 - (Number(it.discount_percentage) || 0) / 100);
}

/** Line total after the line discount; optional lines are offered, not counted. */
export function proposalTotal(items: ProposalItem[]): number {
  return items.reduce((sum, it) => (it.is_optional ? sum : sum + lineAmount(it)), 0);
}

/** 4 Nov 2026: the optional lines' own subtotal (shown under the total, never in it). */
export function proposalOptionalSubtotal(items: ProposalItem[]): number {
  return items.reduce((sum, it) => (it.is_optional ? sum + lineAmount(it) : sum), 0);
}

type CardProps<P> = { proposal: P & ProposalMeta; onResolved?: ResolveProposal };

/** A card reopened from history starts in the state the user left it in. */
const wasApplied = (p: ProposalMeta) => p.status === "applied";
const wasDiscarded = (p: ProposalMeta) => p.status === "discarded";

export default function ProposalCard({ proposal, onResolved }: CardProps<Proposal>) {
  if (proposal.status === "withdrawn" || proposal.status === "superseded") {
    return (
      <div className="mt-2 text-xs text-fg-subtle italic">
        {proposal.status === "withdrawn" ? "Nary took this card back." : "Replaced by a newer card below."}
      </div>
    );
  }
  if (proposal.kind === "chat_link" || proposal.kind === "reminder_change")
    return <ActionCard proposal={proposal} onResolved={onResolved} />;
  if (proposal.kind === "record_change") return <RecordChangeCard proposal={proposal} onResolved={onResolved} />;
  if (proposal.kind === "sop") return <SopProposalCard proposal={proposal} onResolved={onResolved} />;
  if (proposal.kind === "followup_reminder") return <FollowUpProposalCard proposal={proposal} onResolved={onResolved} />;
  if (proposal.kind === "research") return <ResearchProposalCard proposal={proposal} onResolved={onResolved} />;
  if (proposal.kind === "customer_message") return <CustomerMessageCard proposal={proposal} onResolved={onResolved} />;
  if (proposal.kind === "meeting") return <MeetingCard proposal={proposal} onResolved={onResolved} />;
  // visuals V4: proposal figures and "Ask Nary to change" a proposal have their own card (the figure, where it
  // goes, the warnings), never the quotation form or the plain action line.
  const k = proposal.kind as string;
  if ((k === "nary_action" && isProposalCardAction((proposal as NaryActionProposal).action)) || k === "proposal_visual" || k === "proposal_edit") {
    const p = proposal as NaryActionProposal & ProposalMeta;
    const action = p.action || (k === "proposal_edit" ? "proposal.edit" : "proposal.visual");
    return <ProposalVisualCard proposal={{ ...p, action }} onResolved={onResolved} />;
  }
  if (proposal.kind === "nary_action") return <NaryActionCard proposal={proposal} onResolved={onResolved} />;
  if ((proposal.kind === "quotation" || proposal.kind === "invoice") && (proposal as DocProposal).payload)
    return <SalesProposalCard proposal={proposal as DocProposal & ProposalMeta} onResolved={onResolved} />;
  // Records of what Nary already did over the Owner tools (nary_decision, nary_topic_answer) and any kind this
  // screen does not know yet: a short note, never a form built for another kind.
  return <NoteCard proposal={proposal} />;
}

/** A card with no form of its own: its summary and what happened. */
function NoteCard({ proposal }: { proposal: Proposal }) {
  const p = proposal as Proposal & { summary?: string; label?: string; reason?: string; answer?: string };
  const text = p.summary || p.answer || p.label || "Nary recorded an action here.";
  const done = p.status === "applied";
  return (
    <div className="mt-2 rounded-lg border border-line bg-surface-2 px-3 py-2 text-xs text-fg-muted">
      <div className="flex items-start gap-1.5">
        {done ? <Check size={13} className="mt-0.5 shrink-0 text-tone-blue" aria-hidden /> : <FileText size={13} className="mt-0.5 shrink-0" aria-hidden />}
        <span className="whitespace-pre-line">{text}</span>
      </div>
      {p.status === "discarded" && <p className="mt-1 italic">Discarded.</p>}
    </div>
  );
}

/** Deep research the assistant offered; nothing runs until the user starts it here. */
function ResearchProposalCard({ proposal, onResolved }: CardProps<ResearchProposal>) {
  const [busy, setBusy] = useState(false);
  const [started, setStarted] = useState<null | { failed: boolean; text: string }>(
    wasApplied(proposal) ? { failed: false, text: "Research was started from this card." } : null);
  const [cancelled, setCancelled] = useState(wasDiscarded(proposal));
  const [error, setError] = useState("");
  const href = `/crm/${proposal.object_type === "organization" ? "organizations" : "contacts"}/${proposal.record_id}`;

  const confirm = async () => {
    setBusy(true); setError("");
    try {
      const r = await researchApi.start(proposal.object_type, proposal.record_id);
      const run = r.data.run;
      if (run.status !== "failed") onResolved?.("applied", { id: proposal.record_id, label: proposal.label });
      setStarted(run.status === "failed"
        ? { failed: true, text: run.error || "Research could not run." }
        : { failed: false, text: run.status === "done"
            ? "Research finished: the results are on the record."
            : "Research started: you'll get a notification when it's done." });
    } catch (e) {
      setError(extractApiError(e, "Research could not be started."));
    } finally { setBusy(false); }
  };

  if (cancelled) return <div className="mt-2 text-xs text-fg-subtle italic">Research not started.</div>;
  if (started) {
    return (
      <div className={`mt-2 rounded-lg border p-3 text-sm not-prose ${started.failed ? "border-tone-orange/30 bg-tone-orange-soft" : "border-tone-blue/30 bg-tone-blue-soft"}`}>
        <p className={`font-medium flex items-center gap-1.5 ${started.failed ? "text-tone-orange" : "text-tone-blue"}`}>
          {started.failed ? <AlertTriangle size={15} /> : <Check size={15} />} {started.text}
        </p>
        <Link href={href} className="mt-1.5 inline-flex items-center gap-1 text-xs text-fg-muted hover:underline">
          Open {proposal.label} <ExternalLink size={12} />
        </Link>
      </div>
    );
  }
  return (
    <div className="mt-2 rounded-lg border border-line bg-surface p-3 not-prose">
      <div className="flex items-center gap-1.5 text-fg mb-1.5">
        <Globe2 size={14} /><span className="text-xs font-semibold">Deep research · {proposal.label}</span>
      </div>
      {proposal.summary && <p className="text-[11px] text-fg-muted">{proposal.summary}</p>}
      {error && <p className="text-[11px] text-tone-red mt-1.5">{error}</p>}
      <div className="flex items-center gap-2 mt-2.5">
        <button onClick={confirm} disabled={busy}
          className="flex-1 inline-flex items-center justify-center gap-1.5 rounded-lg bg-brand text-brand-on px-3 py-1.5 text-xs font-medium hover:bg-brand-hover disabled:opacity-50">
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Globe2 size={13} />} Start research
        </button>
        <button onClick={() => { setCancelled(true); onResolved?.("discarded"); }} disabled={busy}
          className="rounded-lg border border-line px-3 py-1.5 text-xs text-fg-muted hover:bg-surface-2">Cancel</button>
      </div>
    </div>
  );
}

/** 1 Nov 2026: a video on a message card: "name (size, length, type)". */
export function videoLabel(f: CardFile): string {
  const parts = [f.size_label, f.duration_label, f.type].filter(Boolean);
  return `${f.name} (${parts.join(", ")})`;
}

/** 1 Nov 2026: how a video or an emailed file goes, in a few words ("" when it is attached as usual). */
export function sendAsNote(f: CardFile): string {
  switch (f.send_as) {
    case "video": return "plays in the WhatsApp chat";
    case "document": return "goes as a WhatsApp document";
    case "link": return "goes as a download link in the email";
    default: return "";
  }
}

/** A message Nary sends as itself (its signature is added) once the user confirms this exact text. */
export function CustomerMessageCard({ proposal, onResolved }: CardProps<CustomerMessageProposal>) {
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(wasApplied(proposal));
  const [cancelled, setCancelled] = useState(wasDiscarded(proposal));
  const [error, setError] = useState("");
  const isEmail = proposal.channel === "email";
  const pictures = (proposal.files ?? []).filter((f) => f.picture && f.thumbnail_url);
  const videos = (proposal.files ?? []).filter((f) => f.video);

  const send = async () => {
    setBusy(true); setError("");
    try {
      const res = await naryApi.send({
        channel: proposal.channel, text: proposal.text, chat_id: proposal.chat_id, thread_id: proposal.thread_id,
        contact_id: proposal.contact_id, to: proposal.to, subject: proposal.subject, attach: proposal.attach,
        ...(isEmail ? { cc: proposal.cc ?? [], bcc: proposal.bcc ?? [], from_inbox: proposal.from_inbox ?? null } : {}),
      });
      if (res?.data?.status === "failed") { setError(res.data.error || "It could not be sent."); return; }
      setSent(true);
      onResolved?.("applied", { id: res?.data?.id, label: res?.data?.label || proposal.summary });
    } catch (e) {
      setError(extractApiError(e, "It could not be sent."));
    } finally { setBusy(false); }
  };

  if (cancelled) return <div className="mt-2 text-xs text-fg-subtle italic">Not sent.</div>;
  if (sent) {
    return (
      <div className="mt-2 rounded-lg border border-tone-blue/30 bg-tone-blue-soft p-3 text-sm not-prose">
        <p className="text-tone-blue font-medium flex items-center gap-1.5">
          <Check size={15} /> Sent {isEmail ? "by email" : "on WhatsApp"} to {proposal.recipient}, as Nary
        </p>
      </div>
    );
  }
  return (
    <div className="mt-2 rounded-lg border border-line bg-surface p-3 not-prose">
      <div className="flex items-center gap-1.5 text-fg mb-1.5">
        <Send size={14} />
        <span className="text-xs font-semibold">{isEmail ? "Email" : "WhatsApp"} to {proposal.recipient}, as Nary</span>
      </div>
      {isEmail && (
        <div className="text-[11px] text-fg-muted mb-1 space-y-0.5" aria-label="Email header">
          {proposal.from_inbox && <p className="break-all">From: {proposal.from_inbox}</p>}
          <p className="break-all">To: {proposal.to || proposal.recipient}</p>
          {!!proposal.cc?.length && <p className="break-all">Cc: {proposal.cc.join(", ")}</p>}
          {!!proposal.bcc?.length && <p className="break-all">Bcc: {proposal.bcc.join(", ")}</p>}
          {proposal.subject && <p>Subject: {proposal.subject}</p>}
        </div>
      )}
      <p className="whitespace-pre-wrap text-xs text-fg bg-surface-2 rounded-lg p-2 max-h-72 overflow-y-auto">{proposal.text}</p>
      {!!proposal.attachments?.length && (
        <ul className="mt-1 space-y-0.5 text-[11px] text-fg-muted" aria-label="Attachments">
          {proposal.attachments.map((a, i) => (
            <li key={i} className="flex items-start gap-1"><Paperclip size={11} className="mt-0.5 shrink-0" aria-hidden /> {a}</li>
          ))}
        </ul>
      )}
      {/* 29 Oct 2026: pictures stored on a record (upload_file, a saved preview) show as thumbnails */}
      {!!pictures.length && (
        <div className="mt-1.5 flex flex-wrap gap-1.5" aria-label="Pictures">
          {pictures.map((f) => (
            <figure key={f.id} className="w-24">
              <AuthImage id={f.id} alt={f.name} mime={f.mime_type}
                className="h-16 w-24 rounded-md border border-line object-cover" />
              <figcaption className="truncate text-[10px] text-fg-muted"
                title={`${f.name} (${f.size_label}, ${f.type})${isEmail ? "" : ", sent as a photo"}`}>
                {f.name}
              </figcaption>
            </figure>
          ))}
        </div>
      )}
      {/* 1 Nov 2026: videos with size and length, and how each goes */}
      {!!videos.length && (
        <ul className="mt-1.5 space-y-0.5 text-[11px] text-fg-muted" aria-label="Videos">
          {videos.map((f) => (
            <li key={f.id} className="flex items-start gap-1">
              <Film size={11} className="mt-0.5 shrink-0" aria-hidden />
              <span>{videoLabel(f)}{sendAsNote(f) && <span className="text-fg-subtle">: {sendAsNote(f)}</span>}</span>
            </li>
          ))}
        </ul>
      )}
      <p className="text-[11px] text-fg-subtle mt-1">Nary&apos;s signature is added when it is sent.</p>
      {proposal.warning && <p className="text-[11px] text-tone-orange mt-1">{proposal.warning}</p>}
      {error && <p className="text-[11px] text-tone-red mt-1.5">{error}</p>}
      <div className="flex items-center gap-2 mt-2.5">
        <button onClick={send} disabled={busy}
          className="flex-1 inline-flex items-center justify-center gap-1.5 rounded-lg bg-brand text-brand-on px-3 py-1.5 text-xs font-medium hover:bg-brand-hover disabled:opacity-50">
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Send size={13} />} Send as Nary
        </button>
        <button onClick={() => { setCancelled(true); onResolved?.("discarded"); }} disabled={busy}
          className="rounded-lg border border-line px-3 py-1.5 text-xs text-fg-muted hover:bg-surface-2">Cancel</button>
      </div>
    </div>
  );
}

/** A small change Nary prepared (link a chat to a contact, move or close a reminder): one line and a confirm. */
export function ActionCard({ proposal, onResolved }: CardProps<ChatLinkProposal | ReminderChangeProposal>) {
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(wasApplied(proposal));
  const [cancelled, setCancelled] = useState(wasDiscarded(proposal));
  const [error, setError] = useState("");

  const confirm = async () => {
    setBusy(true); setError("");
    try {
      if (proposal.kind === "chat_link") {
        await naryApi.linkChat(proposal.chat_id, proposal.contact_id);
      } else if (proposal.action === "snooze") {
        await remindersApi.snooze(proposal.reminder_id, { until: proposal.until || undefined });
      } else if (proposal.action === "done") {
        await remindersApi.done(proposal.reminder_id);
      } else {
        await remindersApi.cancel(proposal.reminder_id);
      }
      setDone(true);
      onResolved?.("applied", { label: proposal.summary });
    } catch (e) {
      setError(extractApiError(e, "It could not be done."));
    } finally { setBusy(false); }
  };

  if (cancelled) return <div className="mt-2 text-xs text-fg-subtle italic">Not done.</div>;
  if (done) {
    return (
      <div className="mt-2 rounded-lg border border-tone-blue/30 bg-tone-blue-soft p-3 text-sm not-prose">
        <p className="text-tone-blue font-medium flex items-center gap-1.5"><Check size={15} /> Done: {proposal.summary}</p>
      </div>
    );
  }
  const Icon = proposal.kind === "chat_link" ? Link2 : BellPlus;
  return (
    <div className="mt-2 rounded-lg border border-line bg-surface p-3 not-prose">
      <p className="text-xs text-fg flex items-center gap-1.5"><Icon size={14} /> {proposal.summary}</p>
      {error && <p className="text-[11px] text-tone-red mt-1.5">{error}</p>}
      <div className="flex items-center gap-2 mt-2.5">
        <button onClick={confirm} disabled={busy}
          className="flex-1 inline-flex items-center justify-center gap-1.5 rounded-lg bg-brand text-brand-on px-3 py-1.5 text-xs font-medium hover:bg-brand-hover disabled:opacity-50">
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Check size={13} />} Confirm
        </button>
        <button onClick={() => { setCancelled(true); onResolved?.("discarded"); }} disabled={busy}
          className="rounded-lg border border-line px-3 py-1.5 text-xs text-fg-muted hover:bg-surface-2">Cancel</button>
      </div>
    </div>
  );
}

/** A meeting at an agreed time: Confirm creates the Google Calendar event with Meet and, when the card has a
 * message, sends it with the link filled in. */
export function MeetingCard({ proposal, onResolved }: CardProps<MeetingProposal>) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<null | { label: string; meet_link: string | null; sent_error?: string | null }>(
    wasApplied(proposal) ? { label: proposal.summary || "Booked", meet_link: null } : null);
  const [cancelled, setCancelled] = useState(wasDiscarded(proposal));
  const [error, setError] = useState("");
  const msg = proposal.message;

  const confirm = async () => {
    setBusy(true); setError("");
    try {
      const res = await naryApi.bookMeeting({
        title: proposal.title, start: proposal.start, duration_minutes: proposal.duration_minutes,
        staff_ids: proposal.staff_ids, customer_email: proposal.customer_email, deal_id: proposal.deal_id,
        lead_id: proposal.lead_id, contact_id: proposal.contact_id, message: msg ?? null,
      });
      setResult({ label: res.data.label, meet_link: res.data.meet_link, sent_error: res.data.sent_error });
      onResolved?.("applied", { id: res.data.id, label: res.data.label });
    } catch (e) {
      setError(extractApiError(e, "The meeting could not be booked."));
    } finally { setBusy(false); }
  };

  if (cancelled) return <div className="mt-2 text-xs text-fg-subtle italic">Not booked.</div>;
  if (result) {
    // Booked, but the message to the customer failed: say why, and hand over the link to send by hand.
    const warn = !!result.sent_error;
    return (
      <div className={`mt-2 rounded-lg border p-3 text-sm not-prose ${warn ? "border-tone-orange/30 bg-tone-orange-soft" : "border-tone-blue/30 bg-tone-blue-soft"}`}>
        <p className={`font-medium flex items-center gap-1.5 ${warn ? "text-tone-orange" : "text-tone-blue"}`}>
          {warn ? <AlertTriangle size={15} /> : <Check size={15} />} {result.label}
        </p>
        {result.meet_link && (
          <div className="mt-1.5 flex items-center gap-3 text-xs">
            <a href={result.meet_link} target="_blank" rel="noreferrer"
              className={`inline-flex items-center gap-1 hover:underline ${warn ? "text-tone-orange" : "text-tone-blue"}`}>
              Open Google Meet <ExternalLink size={12} />
            </a>
            {warn && (
              <button type="button" onClick={() => navigator.clipboard?.writeText(result.meet_link || "")}
                className="text-tone-orange underline">Copy the link</button>
            )}
          </div>
        )}
      </div>
    );
  }
  return (
    <div className="mt-2 rounded-lg border border-line bg-surface p-3 not-prose">
      <p className="text-xs text-fg flex items-center gap-1.5"><CalendarPlus size={14} /> {proposal.summary}</p>
      {proposal.time_zone && (
        <p className="text-[11px] text-fg-muted mt-1">Company time: {zoneLabel(cardZone(proposal.time_zone), proposal.start)}</p>
      )}
      {msg && (
        <div className="mt-2 rounded-lg bg-surface-2 border border-line p-2">
          <p className="text-[11px] text-fg-muted mb-1">
            {msg.channel === "email" ? "Email" : "WhatsApp"} to {msg.recipient || msg.to}, as Nary:
          </p>
          <p className="text-xs text-fg whitespace-pre-wrap">{msg.text.replace(/\{link\}/g, "[Meet link]")}</p>
        </div>
      )}
      {error && <p className="text-[11px] text-tone-red mt-1.5">{error}</p>}
      <div className="flex items-center gap-2 mt-2.5">
        <button onClick={confirm} disabled={busy}
          className="flex-1 inline-flex items-center justify-center gap-1.5 rounded-lg bg-brand text-brand-on px-3 py-1.5 text-xs font-medium hover:bg-brand-hover disabled:opacity-50">
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Check size={13} />} {msg ? "Book and send" : "Book"}
        </button>
        <button onClick={() => { setCancelled(true); onResolved?.("discarded"); }} disabled={busy}
          className="rounded-lg border border-line px-3 py-1.5 text-xs text-fg-muted hover:bg-surface-2">Cancel</button>
      </div>
    </div>
  );
}

/** A reminder the assistant prepared; the user edits the time and confirms before it is saved. */
export function FollowUpProposalCard({ proposal, onResolved }: CardProps<FollowUpProposal>) {
  // company time, not the browser's: 09:00 on the card is 09:00 in the company's time zone
  const tz = cardZone(proposal.time_zone);
  const [title, setTitle] = useState(proposal.title);
  const [due, setDue] = useState(toZonedInput(proposal.due_at, tz));
  const dueIso = fromZonedInput(due, tz);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(wasApplied(proposal));
  const [cancelled, setCancelled] = useState(wasDiscarded(proposal));
  const [error, setError] = useState("");

  const confirm = async () => {
    setBusy(true); setError("");
    try {
      const res = await remindersApi.create({
        entity_type: proposal.entity_type, entity_id: proposal.entity_id ?? undefined,
        chat_id: proposal.chat_id ?? undefined, title, note: proposal.note ?? undefined,
        due_at: dueIso,
      });
      setSaved(true);
      onResolved?.("applied", { id: res?.data?.id, label: title });
    } catch (e) {
      setError(extractApiError(e, "Could not set the reminder."));
    } finally { setBusy(false); }
  };

  if (cancelled) return <div className="mt-2 text-xs text-fg-subtle italic">Reminder dismissed.</div>;
  if (saved) {
    return (
      <div className="mt-2 rounded-lg border border-tone-blue/30 bg-tone-blue-soft p-3 text-sm not-prose">
        <p className="text-tone-blue font-medium flex items-center gap-1.5"><Check size={15} /> Reminder set</p>
        <Link href="/crm/followups" className="mt-1.5 inline-flex items-center gap-1 text-tone-blue hover:underline text-xs">
          See your follow-ups <ExternalLink size={12} />
        </Link>
      </div>
    );
  }
  return (
    <div className="mt-2 rounded-lg border border-line bg-surface p-3 not-prose">
      <div className="flex items-center gap-1.5 text-fg mb-2">
        <BellPlus size={14} /><span className="text-xs font-semibold">Follow-up reminder</span>
      </div>
      <input value={title} onChange={(e) => setTitle(e.target.value)} aria-label="Reminder" autoComplete="off"
        className="w-full border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-2 py-1 text-xs font-semibold mb-1.5" />
      <input type="datetime-local" value={due} onChange={(e) => setDue(e.target.value)} aria-label="Due"
        className="w-full border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-2 py-1 text-xs" />
      <p className="text-[11px] text-fg-muted mt-1">
        {proposal.time_zone ? "Company time" : "Your time"}: {zoneLabel(tz, dueIso || proposal.due_at)}
      </p>
      {proposal.note && <p className="text-[11px] text-fg-muted mt-1.5">{proposal.note}</p>}
      {error && <p className="text-[11px] text-tone-red mt-1.5">{error}</p>}
      <div className="flex items-center gap-2 mt-2.5">
        <button onClick={confirm} disabled={busy || !title.trim() || !dueIso}
          className="flex-1 inline-flex items-center justify-center gap-1.5 rounded-lg bg-brand text-brand-on px-3 py-1.5 text-xs font-medium hover:bg-brand-hover disabled:opacity-50">
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Check size={13} />} Set reminder
        </button>
        <button onClick={() => { setCancelled(true); onResolved?.("discarded"); }} disabled={busy}
          className="rounded-lg border border-line px-3 py-1.5 text-xs text-fg-muted hover:bg-surface-2">Cancel</button>
      </div>
    </div>
  );
}

function SalesProposalCard({ proposal, onResolved }: CardProps<DocProposal>) {
  const currency = (proposal.currency || (proposal.payload.currency as string) || "USD").toUpperCase();
  const [items, setItems] = useState<ProposalItem[]>(proposal.payload.items || []);
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState<{ id: string; label: string } | null>(
    wasApplied(proposal) ? { id: proposal.result?.id || "", label: proposal.result?.label || "draft" } : null);
  const [cancelled, setCancelled] = useState(wasDiscarded(proposal));
  const [error, setError] = useState("");

  const isQuote = proposal.kind === "quotation";
  // 1 Nov 2026: a header discount (a Discount line) comes off the lines' total; notes only takes nothing off
  const discountAmount = isQuote ? Number(proposal.discount_amount) || 0 : 0;
  const total = proposalTotal(items) - discountAmount;
  const hasOptional = isQuote && items.some((it) => it.is_optional);
  const optionalSubtotal = proposalOptionalSubtotal(items);

  const setItem = (i: number, patch: Partial<ProposalItem>) =>
    setItems((arr) => arr.map((it, idx) => (idx === i ? { ...it, ...patch } : it)));
  const removeItem = (i: number) => setItems((arr) => arr.filter((_, idx) => idx !== i));
  const addItem = () => setItems((arr) => [...arr, { name: "", quantity: 1, unit_price: 0, unit: "Unit" }]);

  const confirm = async () => {
    setBusy(true); setError("");
    try {
      const payload = { ...proposal.payload, currency, items };
      const res = isQuote ? await salesApi.createQuotation(payload) : await financeApi.createInvoice(payload);
      const done = { id: res.data?.id, label: res.data?.full_number || res.data?.number || res.data?.invoice_number || "draft" };
      setCreated(done);
      onResolved?.("applied", done);
    } catch (e) {
      setError(extractApiError(e, "Failed to create draft"));
    } finally { setBusy(false); }
  };

  if (cancelled) return <div className="mt-2 text-xs text-fg-subtle italic">Draft dismissed.</div>;

  if (created) {
    const href = isQuote ? `/sales/quotations/${created.id}` : `/finance/invoices/${created.id}`;
    return (
      <div className="mt-2 rounded-lg border border-tone-blue/30 bg-tone-blue-soft p-3 text-sm not-prose">
        <p className="text-tone-blue font-medium flex items-center gap-1.5"><Check size={15} /> Draft created: {created.label}</p>
        {created.id && (
          <Link href={href} className="mt-1.5 inline-flex items-center gap-1 text-tone-blue hover:underline text-xs">
            Open in {isQuote ? "Sales" : "Finance"} <ExternalLink size={12} />
          </Link>
        )}
      </div>
    );
  }

  return (
    <div className="mt-2 rounded-lg border border-line bg-surface p-3 not-prose">
      <div className="flex items-center gap-1.5 text-fg mb-2">
        <FileText size={14} /><span className="text-xs font-semibold">Draft {isQuote ? "quotation" : "invoice"}{proposal.customer ? ` · ${proposal.customer}` : ""}</span>
      </div>
      <div className="space-y-1.5">
        {items.map((it, i) => (
          <div key={i} className="flex items-center gap-1.5">
            {it.is_optional && <span className="shrink-0 rounded bg-surface-2 px-1 text-[10px] text-fg-muted">Optional</span>}
            <input value={it.name} onChange={(e) => setItem(i, { name: e.target.value })} aria-label={`Line ${i + 1} item`}
              className="flex-1 min-w-0 border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-1.5 py-1 text-xs" />
            <input type="number" value={it.quantity} onChange={(e) => setItem(i, { quantity: Number(e.target.value) })} aria-label={`Line ${i + 1} quantity`}
              className="w-12 border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-1 py-1 text-xs text-right tabular" />
            <input type="number" value={it.unit_price} onChange={(e) => setItem(i, { unit_price: Number(e.target.value) })} aria-label={`Line ${i + 1} unit price`}
              className="w-24 border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-1.5 py-1 text-xs text-right tabular" />
            <button type="button" onClick={() => removeItem(i)} title="Remove line"
              className="text-fg-subtle hover:text-tone-red" aria-label="Remove line"><X size={13} /></button>
          </div>
        ))}
      </div>
      <button type="button" onClick={addItem} className="mt-1.5 text-[11px] text-brand-ink hover:underline">+ Add line</button>
      {isQuote && proposal.discount && (
        <p className="mt-2 flex justify-between gap-2 text-[11px] text-tone-orange" aria-label="Discount">
          <span>Discount: {proposal.discount}</span>
          {discountAmount > 0 && <span className="tabular">-{fmtMoney(currency, discountAmount)}</span>}
        </p>
      )}
      <div className="flex justify-between items-center mt-2 pt-2 border-t border-line text-sm">
        <span className="text-xs text-fg-muted">Total</span>
        <span className="font-semibold text-fg tabular">{fmtMoney(currency, total)}</span>
      </div>
      {hasOptional && (
        <div className="mt-1 space-y-0.5 text-[11px] text-fg-muted" aria-label="Optional items">
          <p className="flex justify-between gap-2"><span>Optional items (not in the total)</span>
            <span className="tabular">{fmtMoney(currency, optionalSubtotal)}</span></p>
          <p className="flex justify-between gap-2"><span>Total with optional items</span>
            <span className="tabular">{fmtMoney(currency, total + optionalSubtotal)}</span></p>
        </div>
      )}
      {/* 4 Nov 2026: what the house notes template filled, what was written for the deal, what is missing */}
      {isQuote && proposal.notes_summary && (
        <p className="mt-2 text-[11px] text-fg-muted" data-testid="notes-summary">{proposal.notes_summary}</p>
      )}
      {proposal.warnings && proposal.warnings.length > 0 && (
        <div className="mt-2 space-y-1">
          {proposal.warnings.map((w, i) => (
            <p key={i} className="flex items-start gap-1 text-[11px] text-tone-orange"><AlertTriangle size={11} className="mt-0.5 shrink-0" />{w}</p>
          ))}
        </div>
      )}
      {error && <p className="text-[11px] text-tone-red mt-1.5">{error}</p>}
      <div className="flex items-center gap-2 mt-2.5">
        <button onClick={confirm} disabled={busy || !items.some((it) => it.name.trim())}
          className="flex-1 inline-flex items-center justify-center gap-1.5 rounded-lg bg-brand text-brand-on px-3 py-1.5 text-xs font-medium hover:bg-brand-hover disabled:opacity-50">
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Check size={13} />} Confirm &amp; create draft
        </button>
        <button onClick={() => { setCancelled(true); onResolved?.("discarded"); }} disabled={busy}
          className="rounded-lg border border-line px-3 py-1.5 text-xs text-fg-muted hover:bg-surface-2">Cancel</button>
      </div>
    </div>
  );
}

function SopProposalCard({ proposal, onResolved }: CardProps<DocProposal>) {
  const p = proposal.payload;
  const [title, setTitle] = useState<string>((p.title as string) || proposal.title || "Untitled SOP");
  const [summary, setSummary] = useState<string>((p.summary as string) || "");
  const [content, setContent] = useState<string>((p.content as string) || "");
  const [steps, setSteps] = useState<SopStep[]>(p.sop_steps || []);
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState<{ id: string } | null>(
    wasApplied(proposal) ? { id: proposal.result?.id || "" } : null);
  const [cancelled, setCancelled] = useState(wasDiscarded(proposal));
  const [error, setError] = useState("");

  const setStep = (i: number, patch: Partial<SopStep>) =>
    setSteps((arr) => arr.map((s, idx) => (idx === i ? { ...s, ...patch } : s)));
  const removeStep = (i: number) => setSteps((arr) => arr.filter((_, idx) => idx !== i));
  const addStep = () => setSteps((arr) => [...arr, { title: `Step ${arr.length + 1}`, description: "", responsible: "" }]);

  const confirm = async () => {
    setBusy(true); setError("");
    try {
      const payload = { ...p, title, summary, content, sop_steps: steps, article_type: "sop", status: "draft" };
      const res = await kbApi.createArticle(payload);
      setCreated({ id: res.data?.id });
      onResolved?.("applied", { id: res.data?.id, label: title });
    } catch (e) {
      setError(extractApiError(e, "Failed to create SOP draft (need kb:manage_sops permission?)"));
    } finally { setBusy(false); }
  };

  if (cancelled) return <div className="mt-2 text-xs text-fg-subtle italic">SOP draft dismissed.</div>;

  if (created) {
    return (
      <div className="mt-2 rounded-lg border border-tone-blue/30 bg-tone-blue-soft p-3 text-sm not-prose">
        <p className="text-tone-blue font-medium flex items-center gap-1.5"><Check size={15} /> SOP draft created</p>
        {created.id && (
          <Link href={`/knowledge/articles/${created.id}`} className="mt-1.5 inline-flex items-center gap-1 text-tone-blue hover:underline text-xs">
            Review, publish &amp; sign in Knowledge Base <ExternalLink size={12} />
          </Link>
        )}
      </div>
    );
  }

  return (
    <div className="mt-2 rounded-lg border border-line bg-surface p-3 not-prose">
      <div className="flex items-center gap-1.5 text-fg mb-2">
        <FileText size={14} /><span className="text-xs font-semibold">Draft SOP</span>
      </div>
      <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="SOP title"
        className="w-full border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-2 py-1 text-xs font-semibold mb-1.5" />
      <textarea value={summary} onChange={(e) => setSummary(e.target.value)} placeholder="Short summary" rows={2}
        className="w-full border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-2 py-1 text-xs mb-2" />
      <p className="text-[11px] font-semibold text-fg-muted mb-1">Steps</p>
      <div className="space-y-1.5">
        {steps.map((s, i) => (
          <div key={i} className="flex items-start gap-1.5">
            <span className="text-[11px] text-fg-subtle mt-1.5 w-4 text-right">{i + 1}.</span>
            <div className="flex-1 min-w-0 space-y-1">
              <input value={s.title} onChange={(e) => setStep(i, { title: e.target.value })} placeholder="Step title"
                className="w-full border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-1.5 py-1 text-xs font-medium" />
              <textarea value={s.description} onChange={(e) => setStep(i, { description: e.target.value })} placeholder="What to do" rows={2}
                className="w-full border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-1.5 py-1 text-xs" />
              <input value={s.responsible || ""} onChange={(e) => setStep(i, { responsible: e.target.value })} placeholder="Responsible role (optional)"
                className="w-full border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-1.5 py-1 text-[11px] text-fg-muted" />
            </div>
            <button onClick={() => removeStep(i)} className="text-fg-subtle hover:text-tone-red mt-1.5" aria-label="Remove step"><X size={13} /></button>
          </div>
        ))}
      </div>
      <button onClick={addStep} className="mt-1.5 text-[11px] text-brand-ink hover:underline">+ Add step</button>
      <p className="text-[11px] font-semibold text-fg-muted mt-2 mb-1">Body (Markdown)</p>
      <textarea value={content} onChange={(e) => setContent(e.target.value)} placeholder="Purpose, scope, policy, notes" rows={4}
        className="w-full border border-line-strong bg-surface text-fg rounded-md focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20 px-2 py-1 text-xs" />
      {proposal.warnings && proposal.warnings.length > 0 && (
        <div className="mt-2 space-y-1">
          {proposal.warnings.map((w, i) => (
            <p key={i} className="flex items-start gap-1 text-[11px] text-tone-orange"><AlertTriangle size={11} className="mt-0.5 shrink-0" />{w}</p>
          ))}
        </div>
      )}
      {error && <p className="text-[11px] text-tone-red mt-1.5">{error}</p>}
      <div className="flex items-center gap-2 mt-2.5">
        <button onClick={confirm} disabled={busy || !title.trim()}
          className="flex-1 inline-flex items-center justify-center gap-1.5 rounded-lg bg-brand text-brand-on px-3 py-1.5 text-xs font-medium hover:bg-brand-hover disabled:opacity-50">
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Check size={13} />} Confirm &amp; create draft
        </button>
        <button onClick={() => { setCancelled(true); onResolved?.("discarded"); }} disabled={busy}
          className="rounded-lg border border-line px-3 py-1.5 text-xs text-fg-muted hover:bg-surface-2">Cancel</button>
      </div>
    </div>
  );
}

/** Cancel a meeting, add guests, merge two contacts: one line, Confirm applies it as the person (and sends the
 * customer message when there is one); a message that failed says why. */
export function NaryActionCard({ proposal, onResolved }: CardProps<NaryActionProposal>) {
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<null | { label: string; warn: boolean }>(
    wasApplied(proposal) ? { label: proposal.summary, warn: false } : null);
  const [cancelled, setCancelled] = useState(wasDiscarded(proposal));
  const [error, setError] = useState("");
  const [working, setWorking] = useState(false); // fix-longapply
  const msg = proposal.message;

  const confirm = async () => {
    setBusy(true); setError("");
    try {
      const res = await naryApi.runAction({ action: proposal.action, payload: proposal.payload, message: msg ?? null });
      // fix-longapply: still running after the wait: it finishes on its own, the card is polled
      const data = await settleCard(res, () => setWorking(true));
      if (data.status === "failed") throw new LongWorkError(String((data as { error?: string | null }).error || "It could not be done."));
      const polled = (data as { result?: { label?: string } | null }).result?.label;
      const label = (isStillWorking(res) ? polled : res.data.label) || proposal.summary;
      setDone({ label, warn: !!res.data.sent_error });
      onResolved?.("applied", { id: res.data.id, label });
    } catch (e) {
      setError(extractApiError(e, "It could not be done."));
    } finally { setBusy(false); setWorking(false); }
  };

  if (cancelled) return <div className="mt-2 text-xs text-fg-subtle italic">Not done.</div>;
  if (done) {
    return (
      <div className={`mt-2 rounded-lg border p-3 text-sm not-prose ${done.warn ? "border-tone-orange/30 bg-tone-orange-soft" : "border-tone-blue/30 bg-tone-blue-soft"}`}>
        <p className={`font-medium flex items-center gap-1.5 ${done.warn ? "text-tone-orange" : "text-tone-blue"}`}>
          {done.warn ? <AlertTriangle size={15} /> : <Check size={15} />} {done.label}
        </p>
      </div>
    );
  }
  return (
    <div className="mt-2 rounded-lg border border-line bg-surface p-3 not-prose">
      <p className="text-xs text-fg flex items-center gap-1.5"><Check size={14} /> {proposal.summary}</p>
      {!!proposal.details?.length && (
        <ul className="mt-1.5 space-y-0.5 text-[11px] text-fg-muted whitespace-pre-wrap" aria-label="What changes">
          {proposal.details.map((d, i) => <li key={i}>{d}</li>)}
        </ul>
      )}
      {msg && (
        <div className="mt-2 rounded-lg bg-surface-2 border border-line p-2">
          <p className="text-[11px] text-fg-muted mb-1">
            {msg.channel === "email" ? "Email" : "WhatsApp"} to {msg.recipient || msg.to}, as Nary:
          </p>
          <p className="text-xs text-fg whitespace-pre-wrap">{msg.text}</p>
        </div>
      )}
      {working && <p className="mt-1.5 flex items-center gap-1.5 text-[11px] text-fg-muted" role="status"><Loader2 size={12} className="animate-spin" aria-hidden /> Working... this can take a few minutes.</p>}
      {error && <p className="text-[11px] text-tone-red mt-1.5">{error}</p>}
      <div className="flex items-center gap-2 mt-2.5">
        <button onClick={confirm} disabled={busy}
          className="flex-1 inline-flex items-center justify-center gap-1.5 rounded-lg bg-brand text-brand-on px-3 py-1.5 text-xs font-medium hover:bg-brand-hover disabled:opacity-50">
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Check size={13} />} Confirm
        </button>
        <button onClick={() => { setCancelled(true); onResolved?.("discarded"); }} disabled={busy}
          className="rounded-lg border border-line px-3 py-1.5 text-xs text-fg-muted hover:bg-surface-2">Cancel</button>
      </div>
    </div>
  );
}
