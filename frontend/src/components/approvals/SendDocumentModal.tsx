"use client";

import { useEffect, useMemo, useState } from "react";
import { Loader2, Mail, MessageCircle, Search, Send } from "lucide-react";
import { Button, Dialog, Field, Input, Select, Textarea } from "@/components/ui";
import { cn } from "@/lib/utils";
import { crmInboxApi, documentsApi, whatsappApi, type InboxMailbox, type SendDefaults } from "@/lib/api";
import { apiErrorCode, extractApiError } from "@/lib/apiError";

interface Chat {
  id: string;
  name?: string;
  isGroup?: boolean;
  phone?: string | null;
  crm_contact_name?: string | null;
}

interface Props {
  entityType: string;
  entityId: string;
  /** "Quotation", "Invoice", "Purchase order", "Work order" */
  label: string;
  defaults: SendDefaults;
  onClose: () => void;
  onSent: (message: string) => void;
}

function sentTime(iso?: string): string {
  if (!iso) return "a few minutes ago";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "a few minutes ago"
    : `at ${d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}

/** No usable answer from the server (timeout, proxy 502/504 page): the send may still have gone through. */
function noVerdict(e: unknown): boolean {
  const res = (e as { response?: { status?: number; data?: unknown } })?.response;
  if (!res) return true;
  return (res.status === 502 || res.status === 504) && (typeof res.data !== "object" || res.data === null);
}

function splitEmails(text: string): string[] {
  return text.split(/[,;\s]+/).map((s) => s.trim()).filter(Boolean);
}

/** Send an approved document's verified PDF by email (CRM mailbox) or WhatsApp: recipient editable, confirmed. */
export default function SendDocumentModal({ entityType, entityId, label, defaults, onClose, onSent }: Props) {
  const [channel, setChannel] = useState<"email" | "whatsapp">(defaults.email || !defaults.whatsapp ? "email" : "whatsapp");
  const [to, setTo] = useState(defaults.email);
  const [cc, setCc] = useState("");
  const [bcc, setBcc] = useState("");
  // The CRM inbox it goes from; "" is the deal's inbox (the server picks it).
  const [fromInbox, setFromInbox] = useState("");
  const [mailboxes, setMailboxes] = useState<InboxMailbox[]>([]);
  const [waTo, setWaTo] = useState(defaults.whatsapp);
  const [waChatName, setWaChatName] = useState("");
  const [toName, setToName] = useState(defaults.name);
  const [message, setMessage] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  // The same document just went to the same recipient: sending again needs a deliberate click.
  const [recentlySentAt, setRecentlySentAt] = useState<string | null>(null);

  const [chats, setChats] = useState<Chat[] | null>(null);
  const [chatQuery, setChatQuery] = useState("");
  const [chatsError, setChatsError] = useState("");

  useEffect(() => {
    crmInboxApi.mailboxes().then((r) => setMailboxes((r.data?.mailboxes ?? []).filter((m) => m.active))).catch(() => { /* one inbox */ });
  }, []);

  useEffect(() => {
    if (channel !== "whatsapp" || chats !== null) return;
    whatsappApi.getConversations(200)
      .then((res) => setChats(Array.isArray(res.data) ? res.data : []))
      .catch((e) => { setChats([]); setChatsError(extractApiError(e, "WhatsApp chats could not be loaded.")); });
  }, [channel, chats]);

  const filteredChats = useMemo(() => {
    const q = chatQuery.trim().toLowerCase();
    return (chats ?? [])
      .filter((c) => !q || [c.name, c.crm_contact_name, c.phone, c.id].some((v) => v?.toLowerCase().includes(q)))
      .slice(0, 30);
  }, [chats, chatQuery]);

  const recipients = channel === "email" ? splitEmails(to) : waTo.trim() ? [waTo.trim()] : [];
  const ccList = channel === "email" ? splitEmails(cc) : [];
  const bccList = channel === "email" ? splitEmails(bcc) : [];
  const recipientText = channel === "email"
    ? recipients.join(", ") + (ccList.length ? ` (cc ${ccList.join(", ")})` : "") + (bccList.length ? ` (bcc ${bccList.join(", ")})` : "")
      + (fromInbox ? ` from ${fromInbox}` : "")
    : waChatName ? `${waChatName} (${waTo})` : waTo;

  function review() {
    setError("");
    if (!recipients.length) {
      setError(channel === "email" ? "Enter at least one email address." : "Enter a WhatsApp number or pick a chat.");
      return;
    }
    if (channel === "email") {
      const bad = [...recipients, ...ccList, ...bccList].find((e) => !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(e));
      if (bad) { setError(`"${bad}" is not a valid email address.`); return; }
      if (bccList.length > 10) { setError("Bcc takes at most 10 addresses."); return; }
    }
    setConfirming(true);
  }

  async function send(force = false) {
    setBusy(true);
    setError("");
    try {
      await documentsApi.send(entityType, entityId, {
        channel, to: recipients, cc: ccList, to_name: toName.trim() || undefined, message: message.trim() || undefined,
        ...(channel === "email" ? { bcc: bccList, from_inbox: fromInbox || null } : {}),
        ...(force ? { force: true } : {}),
      });
      setRecentlySentAt(null);
      onSent(`${label} ${defaults.number} sent to ${recipientText}.`);
    } catch (e) {
      const code = apiErrorCode(e);
      if (code === "recently_sent") {
        const detail = (e as { response?: { data?: { detail?: { sent_at?: string } } } }).response?.data?.detail;
        setRecentlySentAt(detail?.sent_at || "");
      } else if (noVerdict(e)) {
        setError(`The server took too long to answer. The ${label.toLowerCase()} may still have been sent; `
          + "check the deal's activity or the WhatsApp chat before sending again.");
      } else {
        setError(extractApiError(e, `Could not send the ${label.toLowerCase()}.`));
      }
      setConfirming(false);
    } finally {
      setBusy(false);
    }
  }

  const tab = (value: "email" | "whatsapp", Icon: typeof Mail, text: string) => (
    <button type="button" aria-pressed={channel === value}
      onClick={() => { setChannel(value); setConfirming(false); setError(""); setRecentlySentAt(null); }}
      className={cn("inline-flex min-h-11 flex-1 items-center justify-center gap-2 rounded-lg border px-3 text-sm font-medium transition-colors md:min-h-9",
        "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)]",
        channel === value ? "border-brand bg-brand/[0.07] text-brand-ink" : "border-line-strong text-fg hover:bg-surface-2")}>
      <Icon className="size-4" aria-hidden /> {text}
    </button>
  );

  function requestClose() {
    if (busy) return;
    if (confirming) setConfirming(false); else onClose();
  }

  return (
    <Dialog open onOpenChange={(o) => { if (!o) requestClose(); }}
      title={`Send ${label.toLowerCase()} ${defaults.number}`}
      description="The approved PDF with its verification QR is attached."
      footer={<>
        <Button onClick={confirming ? () => setConfirming(false) : onClose} disabled={busy}>
          {confirming ? "Back" : "Cancel"}
        </Button>
        <Button variant="primary" onClick={confirming ? () => send() : review} disabled={busy || recentlySentAt !== null}>
          {busy ? <Loader2 className="animate-spin" aria-hidden /> : <Send aria-hidden />}
          {confirming ? "Confirm & send" : "Review"}
        </Button>
      </>}>
      <div className="space-y-4">
        <div className="flex gap-2">
          {tab("email", Mail, "Email")}
          {tab("whatsapp", MessageCircle, "WhatsApp")}
        </div>

        {channel === "email" ? (
          <>
            {mailboxes.length > 1 && (
              <Field label="From">
                <Select value={fromInbox} onChange={(e) => { setFromInbox(e.target.value); setConfirming(false); }}
                  options={[{ value: "", label: "Deal's inbox" }, ...mailboxes.map((m) => ({ value: m.address, label: m.address }))]} />
              </Field>
            )}
            <Field label="To" hint={`Separate several addresses with commas. Sent from ${mailboxes.length > 1 ? "the inbox above" : "the CRM mailbox"}.`}>
              <Input value={to} onChange={(e) => { setTo(e.target.value); setConfirming(false); setRecentlySentAt(null); }}
                placeholder="name@company.com" />
            </Field>
            <Field label="Cc">
              <Input value={cc} onChange={(e) => { setCc(e.target.value); setConfirming(false); }} />
            </Field>
            <Field label="Bcc" hint="Optional, up to 10 addresses. Recipients don't see them.">
              <Input value={bcc} onChange={(e) => { setBcc(e.target.value); setConfirming(false); }} />
            </Field>
            <Field label="Greeting name">
              <Input value={toName} onChange={(e) => setToName(e.target.value)} />
            </Field>
          </>
        ) : (
          <>
            <Field label="WhatsApp number or chat">
              <Input value={waTo} onChange={(e) => { setWaTo(e.target.value); setWaChatName(""); setConfirming(false); setRecentlySentAt(null); }}
                placeholder="+1 555 010 0199" />
            </Field>
            <div className="space-y-1.5">
              <p className="text-[13px] font-medium text-fg">Or pick a chat</p>
              <Input type="search" leading={Search} autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
                value={chatQuery} onChange={(e) => setChatQuery(e.target.value)} placeholder="Search chats" aria-label="Search chats" />
              <div className="max-h-44 divide-y divide-line-soft overflow-y-auto rounded-lg border border-line">
                {chats === null && <p className="flex items-center gap-2 p-3 text-xs text-fg-muted"><Loader2 className="size-3.5 animate-spin" aria-hidden /> Loading chats...</p>}
                {chatsError && <p className="p-3 text-xs text-tone-red">{chatsError}</p>}
                {chats !== null && !chatsError && filteredChats.length === 0 && <p className="p-3 text-xs text-fg-muted">No chats found.</p>}
                {filteredChats.map((c) => {
                  const name = c.crm_contact_name || c.name || c.phone || c.id;
                  const selected = waTo === c.id;
                  return (
                    <button key={c.id} type="button" aria-pressed={selected}
                      onClick={() => { setWaTo(c.id); setWaChatName(name); setConfirming(false); setRecentlySentAt(null); }}
                      className={cn("w-full px-3 py-2 text-left text-sm hover:bg-surface-2", selected && "bg-brand/[0.07]")}>
                      <span className="font-semibold text-fg">{name}</span>
                      {c.isGroup ? <span className="ml-2 text-xs text-fg-muted">group</span>
                        : c.phone && name !== c.phone ? <span className="ml-2 text-xs text-fg-muted">{c.phone}</span> : null}
                    </button>
                  );
                })}
              </div>
            </div>
          </>
        )}

        <Field label="Message (optional)">
          <Textarea value={message} onChange={(e) => setMessage(e.target.value)} rows={3} />
        </Field>

        {confirming && (
          <div className="rounded-lg border border-tone-orange/30 bg-tone-orange-soft p-3 text-sm text-fg">
            Send {label.toLowerCase()} <strong>{defaults.number}</strong> by {channel === "email" ? "email" : "WhatsApp"} to{" "}
            <strong className="break-all">{recipientText}</strong>?
          </div>
        )}
        {recentlySentAt !== null && (
          <div className="rounded-lg border border-tone-orange/30 bg-tone-orange-soft p-3 text-sm text-fg">
            <p>
              {label} <strong>{defaults.number}</strong> was already sent to{" "}
              <strong className="break-all">{recipientText}</strong> {sentTime(recentlySentAt)}. Send it again?
            </p>
            <div className="mt-2 flex justify-end gap-2">
              <Button size="sm" onClick={() => setRecentlySentAt(null)} disabled={busy}>Don&apos;t send</Button>
              <Button size="sm" variant="primary" onClick={() => send(true)} disabled={busy}>
                {busy ? <Loader2 className="animate-spin" aria-hidden /> : <Send aria-hidden />} Send again
              </Button>
            </div>
          </div>
        )}
        {error && <p className="text-xs text-tone-red" role="alert">{error}</p>}
      </div>
    </Dialog>
  );
}
