"use client";

import { useCallback, useEffect, useState } from "react";
import { Plus } from "lucide-react";
import { Button, Card, CardBody, CardHeader, Field, FieldGrid, Input, Pill, Select, Skeleton, Switch, toast } from "@/components/ui";
import { systemApi, type CrmMailbox } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { CrmImapBlock, CrmSmtpBlock, useReloadEmailConfig } from "./EmailServerBlocks";

const PURPOSES = [
  { value: "general", label: "General" },
  { value: "sales", label: "Sales" },
];

/** hello@ until the inbox list loads (or when the server has no list yet). */
const HELLO: CrmMailbox = {
  id: "", prefix: "crm", address: "hello@example.com", display_name: "Your Company", purpose: "general",
  signature_html: null, roles: null, active: true, is_default: true, sort: 0, connected_at: null, backfill_days: 0, backfill_done: true,
};

function InboxRow({ box, onChange }: { box: CrmMailbox; onChange: (m: CrmMailbox) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function patch(data: Parameters<typeof systemApi.updateCrmMailbox>[1]) {
    setBusy(true); setError("");
    try {
      onChange((await systemApi.updateCrmMailbox(box.id, data)).data.mailbox);
    } catch (e) {
      setError(extractApiError(e, "Could not change the inbox."));
    } finally {
      setBusy(false);
    }
  }
  return (
    <li className="py-2.5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium text-fg">{box.address}</p>
          <p className="truncate text-xs text-fg-muted">
            {box.display_name} · history {box.backfill_days} days
            {box.active && (box.backfill_done ? " · history imported" : " · importing history")}
          </p>
        </div>
        <Pill tone={box.purpose === "sales" ? "violet" : "neutral"}>{box.purpose === "sales" ? "Sales" : "General"}</Pill>
        {box.is_default && <Pill tone="blue">Default</Pill>}
        {!box.is_default && box.active && (
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => patch({ is_default: true })}>Make default</Button>
        )}
        <Switch label={box.active ? "On" : "Off"} checked={box.active} disabled={busy || (box.is_default && box.active)}
          aria-label={`${box.address} on`} onCheckedChange={(v) => patch({ active: v })} />
      </div>
      {error && <p role="alert" className="mt-1 text-xs text-tone-red">{error}</p>}
    </li>
  );
}

function AddInbox({ onAdded }: { onAdded: (m: CrmMailbox) => void }) {
  const [address, setAddress] = useState("");
  const [name, setName] = useState("");
  const [purpose, setPurpose] = useState<"general" | "sales">("sales");
  const [days, setDays] = useState("90");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function add() {
    setError("");
    const addr = address.trim().toLowerCase();
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(addr)) { setError("Enter the inbox's email address."); return; }
    const n = parseInt(days, 10);
    setBusy(true);
    try {
      const r = await systemApi.createCrmMailbox({
        address: addr, display_name: name.trim() || "Your Company", purpose, backfill_days: Number.isFinite(n) && n >= 0 ? n : 90,
      });
      onAdded(r.data.mailbox);
      setAddress(""); setName(""); setDays("90");
      toast.success(`${addr} added. Set its servers below, then switch it on.`);
    } catch (e) {
      setError(extractApiError(e, "Could not add the inbox."));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="space-y-3 border-t border-line pt-3">
      <p className="text-sm font-semibold text-fg">Add inbox</p>
      <p className="text-[13px] text-fg-muted">
        A new inbox starts switched off. Set its outgoing and incoming servers below and test them, then switch it on.
        It then imports its last {days || "N"} days of mail quietly: no alerts and no reminders for that history.
      </p>
      <FieldGrid cols={2}>
        <Field label="Address"><Input type="email" value={address} onChange={(e) => setAddress(e.target.value)} placeholder="sales@example.com" /></Field>
        <Field label="Display name"><Input value={name} onChange={(e) => setName(e.target.value)} placeholder="Acme Sales" /></Field>
        <Field label="Purpose">
          <Select value={purpose} onChange={(e) => setPurpose(e.target.value as "general" | "sales")} options={PURPOSES} />
        </Field>
        <Field label="History to import" hint="Days of past mail, read once when it is switched on.">
          <Input numeric value={days} onChange={(e) => setDays(e.target.value.replace(/[^0-9]/g, ""))} />
        </Field>
      </FieldGrid>
      {error && <p role="alert" className="text-xs text-tone-red">{error}</p>}
      <Button onClick={add} loading={busy}><Plus aria-hidden /> Add inbox</Button>
    </div>
  );
}

/** Settings, CRM inboxes: the shared inboxes (hello@, sales@...) and each one's outgoing and incoming servers. */
export default function CrmInboxesBlock() {
  const reloadConfig = useReloadEmailConfig();
  const [boxes, setBoxes] = useState<CrmMailbox[] | null>(null);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    try {
      setBoxes([...(await systemApi.listCrmMailboxes()).data.mailboxes].sort((a, b) => a.sort - b.sort));
    } catch (e) {
      setBoxes([]);
      setError(extractApiError(e, "Could not load the inboxes."));
    }
  }, []);
  useEffect(() => { void load(); }, [load]);

  const replace = (m: CrmMailbox) => {
    // A new default takes the flag from the old one.
    setBoxes((list) => (list ?? []).map((b) => (b.id === m.id ? m : m.is_default ? { ...b, is_default: false } : b)));
  };

  const shown = boxes && boxes.length ? boxes : [HELLO];
  return (
    <div className="space-y-4">
      <Card>
        <CardHeader title="Inboxes" description="Mail to these addresses lands in CRM, Email. The default one sends deals without an inbox of their own." />
        <CardBody className="space-y-3">
          {boxes === null ? <Skeleton className="h-16 w-full" /> : (
            <ul className="divide-y divide-line-soft">
              {shown.map((b) => <InboxRow key={b.id || b.prefix} box={b} onChange={replace} />)}
            </ul>
          )}
          {error && <p role="alert" className="text-xs text-tone-red">{error}</p>}
          <AddInbox onAdded={(m) => { setBoxes((list) => [...(list ?? []), m]); void reloadConfig(); }} />
        </CardBody>
      </Card>
      {shown.map((b) => (
        <section key={`servers-${b.prefix}`} className="space-y-2" aria-label={`${b.address} servers`}>
          <h3 className="text-sm font-semibold text-fg">{b.address}, outgoing (SMTP)</h3>
          <CrmSmtpBlock prefix={b.prefix} address={b.address} />
          <h3 className="text-sm font-semibold text-fg">{b.address}, incoming (IMAP)</h3>
          <CrmImapBlock prefix={b.prefix} address={b.address} />
        </section>
      ))}
    </div>
  );
}
