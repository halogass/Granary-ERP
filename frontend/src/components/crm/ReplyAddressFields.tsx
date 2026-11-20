"use client";

import { useEffect, useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import { Select } from "@/components/ui";
import { cn } from "@/lib/utils";
import { crmInboxApi, type InboxMailbox, type ReplyDefaults } from "@/lib/api";

/** Who an email reply goes from and to, as the user leaves it. Address lists are comma separated. */
export interface ReplyAddressing {
  /** False until the thread's reply defaults arrived; nothing below is sent before. */
  loaded: boolean;
  from_inbox: string;
  to: string;
  cc: string;
  bcc: string;
  subject: string;
}

export const EMPTY_ADDRESSING: ReplyAddressing = { loaded: false, from_inbox: "", to: "", cc: "", bcc: "", subject: "" };

export function splitAddresses(text: string): string[] {
  return text.split(/[,;\s]+/).map((s) => s.trim()).filter(Boolean);
}

/** The first address in the lists that is not an email address, if any. */
export function invalidAddress(a: ReplyAddressing): string | null {
  for (const e of [...splitAddresses(a.to), ...splitAddresses(a.cc), ...splitAddresses(a.bcc)]) {
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(e)) return e;
  }
  return null;
}

/** Adds to, cc, bcc, from_inbox and subject to a reply's form data (only once the defaults loaded). */
export function appendAddressing(fd: FormData, a: ReplyAddressing) {
  if (!a.loaded) return;
  fd.append("to", splitAddresses(a.to).join(","));
  fd.append("cc", splitAddresses(a.cc).join(","));
  fd.append("bcc", splitAddresses(a.bcc).join(","));
  if (a.from_inbox) fd.append("from_inbox", a.from_inbox);
  if (a.subject.trim()) fd.append("subject", a.subject.trim());
}

export type ReplyMode = "reply" | "all" | "forward";

/** The addressing a reply mode starts with: Reply all keeps the defaults' Cc, Reply goes to the last
 *  sender only, Forward starts with nobody and a "Fwd:" subject. */
export function addressingFor(mode: ReplyMode, d: ReplyDefaults | null, threadSubject: string): ReplyAddressing {
  const from_inbox = d?.from_inbox ?? "";
  if (mode === "forward") {
    const base = (threadSubject || "").replace(/^\s*(re|fwd?)\s*:\s*/i, "");
    return { loaded: true, from_inbox, to: "", cc: "", bcc: "", subject: `Fwd: ${base}` };
  }
  // Without defaults the server's own apply (the last sender plus everyone else on the thread)
  if (!d) return EMPTY_ADDRESSING;
  if (mode === "reply") {
    return { loaded: true, from_inbox, to: (d.to ?? []).slice(0, 1).join(", "), cc: "", bcc: "", subject: d.subject ?? "" };
  }
  return { loaded: true, from_inbox, to: (d.to ?? []).join(", "), cc: (d.cc ?? []).join(", "), bcc: "", subject: d.subject ?? "" };
}

/** One line for the folded header: "To Robert Smith +2 cc · Re: ERP rollout". */
export function addressingSummary(a: ReplyAddressing, names: Record<string, string> = {}): string {
  const to = splitAddresses(a.to);
  const cc = splitAddresses(a.cc).length;
  const bcc = splitAddresses(a.bcc).length;
  const who = to.length ? (names[to[0].toLowerCase()] || to[0]) : "nobody yet";
  let line = `To ${who}`;
  if (to.length > 1) line += ` +${to.length - 1}`;
  if (cc) line += ` +${cc} cc`;
  if (bcc) line += ` +${bcc} bcc`;
  if (a.subject.trim()) line += ` · ${a.subject.trim()}`;
  return line;
}

const inputClass = "h-8 min-w-0 flex-1 bg-transparent text-[13px] text-fg outline-none placeholder:text-fg-subtle";

function Row({ label, children, className }: { label: string; children: React.ReactNode; className?: string }) {
  return (
    <label className={cn("flex min-h-9 items-center gap-2 border-b border-line-soft px-4", className)}>
      <span className="w-14 shrink-0 text-xs text-fg-muted">{label}</span>
      {children}
    </label>
  );
}

/**
 * From, To, Cc, Bcc and Subject of an email reply, prefilled from the thread's reply defaults
 * (GET /crm/inbox/threads/{id}/reply-defaults). Everyone can be removed or added before sending.
 */
export default function ReplyAddressFields({
  threadId, mailboxes, value, onChange, className, autoLoad = true, summary = false, names, expanded, onExpandedChange,
}: {
  threadId: string;
  /** The CRM inboxes; the From select lists the active ones. */
  mailboxes: InboxMailbox[];
  value: ReplyAddressing;
  onChange: (next: ReplyAddressing) => void;
  className?: string;
  /** False when the parent loads the reply defaults itself (it also picks Reply, Reply all or Forward). */
  autoLoad?: boolean;
  /** Fold the fields into one summary line that opens on click; Bcc stays hidden until used. */
  summary?: boolean;
  /** Display names by lower-case address, for the summary line. */
  names?: Record<string, string>;
  expanded?: boolean;
  onExpandedChange?: (open: boolean) => void;
}) {
  const [ownOpen, setOwnOpen] = useState(false);
  const open = !summary || (expanded ?? ownOpen);
  const setOpen = (o: boolean) => { setOwnOpen(o); onExpandedChange?.(o); };
  const [bccShown, setBccShown] = useState(false);
  useEffect(() => { setBccShown(false); }, [threadId]);

  useEffect(() => {
    if (!autoLoad) return;
    let live = true;
    onChange(EMPTY_ADDRESSING);
    crmInboxApi.replyDefaults(threadId).then((r) => {
      if (!live) return;
      const d = r.data;
      onChange({
        loaded: true, from_inbox: d.from_inbox ?? "", to: (d.to ?? []).join(", "), cc: (d.cc ?? []).join(", "),
        bcc: "", subject: d.subject ?? "",
      });
    }).catch(() => { /* the server's own defaults apply */ });
    return () => { live = false; };
    // onChange is the parent's setter; reload only for another thread
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadId, autoLoad]);

  if (!value.loaded) return null;
  const set = (k: keyof ReplyAddressing) => (e: React.ChangeEvent<HTMLInputElement | HTMLSelectElement>) =>
    onChange({ ...value, [k]: e.target.value });
  const active = mailboxes.filter((m) => m.active);
  const fromOptions = active.some((m) => m.address === value.from_inbox) || !value.from_inbox
    ? active : [...active, { id: value.from_inbox, address: value.from_inbox, name: "", purpose: "", active: true, default: false }];

  if (!open) {
    return (
      <div className={cn("border-b border-line", className)}>
        <button type="button" aria-expanded={false} onClick={() => setOpen(true)}
          title="Show From, To, Cc, Bcc and Subject"
          className="flex min-h-9 w-full min-w-0 items-center gap-2 px-4 text-left text-[13px] text-fg transition-colors hover:bg-surface-2">
          <span className="min-w-0 flex-1 truncate">{addressingSummary(value, names)}</span>
          <ChevronDown className="size-3.5 shrink-0 text-fg-muted" aria-hidden />
        </button>
      </div>
    );
  }
  const showBcc = !summary || bccShown || !!value.bcc.trim();

  return (
    <div className={cn("border-b border-line", className)}>
      <div className="flex items-center border-b border-line-soft">
        <Row label="From" className="min-w-0 flex-1 border-b-0">
          {fromOptions.length > 1 ? (
            <div className="min-w-0 flex-1">
              <Select aria-label="From inbox" value={value.from_inbox} onChange={set("from_inbox")} className="h-8 border-0 bg-transparent px-0 text-[13px] md:h-8"
                options={fromOptions.map((m) => ({ value: m.address, label: m.name ? `${m.name} <${m.address}>` : m.address }))} />
            </div>
          ) : <span className="min-w-0 flex-1 truncate text-[13px] text-fg">{value.from_inbox}</span>}
        </Row>
        {summary && (
          <button type="button" aria-expanded onClick={() => setOpen(false)}
            aria-label="Fold the address fields" title="Fold the address fields"
            className="mr-3 flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted transition-colors hover:bg-surface-2 hover:text-fg">
            <ChevronUp className="size-3.5" aria-hidden />
          </button>
        )}
      </div>
      <Row label="To">
        <input aria-label="To" className={inputClass} value={value.to} onChange={set("to")} placeholder="name@company.com" />
        {!showBcc && (
          <button type="button" onClick={(e) => { e.preventDefault(); setBccShown(true); }}
            className="shrink-0 rounded-md px-1.5 py-0.5 text-xs text-fg-muted transition-colors hover:bg-surface-2 hover:text-fg">
            Bcc
          </button>
        )}
      </Row>
      <Row label="Cc">
        <input aria-label="Cc" className={inputClass} value={value.cc} onChange={set("cc")} placeholder="Separate addresses with commas" />
      </Row>
      {showBcc && (
        <Row label="Bcc">
          <input aria-label="Bcc" className={inputClass} value={value.bcc} onChange={set("bcc")} autoFocus={summary && bccShown && !value.bcc} />
        </Row>
      )}
      <Row label="Subject">
        <input aria-label="Subject" className={inputClass} value={value.subject} onChange={set("subject")} />
      </Row>
    </div>
  );
}
