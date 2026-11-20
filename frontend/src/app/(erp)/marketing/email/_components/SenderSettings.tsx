"use client";

import { useEffect, useState } from "react";
import { emailMarketingApi, type CampaignSender } from "@/lib/api";
import { Checkbox, Field, Select } from "@/components/ui";

interface Props {
  senderUserId: string | null;
  includeSignature: boolean;
  disabled?: boolean;
  onChange: (patch: { sender_user_id?: string | null; include_signature?: boolean }) => void;
}

/** Who the campaign comes from: their email signature (the CRM / inbox one) closes every email. */
export default function SenderSettings({ senderUserId, includeSignature, disabled, onChange }: Props) {
  const [senders, setSenders] = useState<CampaignSender[]>([]);

  useEffect(() => {
    emailMarketingApi.senders().then((r) => {
      setSenders(r.data);
      if (!senderUserId) {
        const me = r.data.find((s) => s.is_me);
        if (me) onChange({ sender_user_id: me.id });
      }
    }).catch(() => setSenders([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const sender = senders.find((s) => s.id === senderUserId);

  return (
    <div className="space-y-3">
      <Field label="Sender">
        <Select value={senderUserId ?? ""} disabled={disabled} onChange={(e) => onChange({ sender_user_id: e.target.value || null })}
          placeholder={!senderUserId ? "Select a sender" : undefined}
          options={senders.map((s) => ({ value: s.id, label: `${s.name}${s.is_me ? " (me)" : ""}, ${s.email}` }))} />
      </Field>
      <Checkbox label="Close the email with the sender's signature" checked={includeSignature} disabled={disabled}
        onChange={(e) => onChange({ include_signature: e.target.checked })} />
      {includeSignature && sender && !sender.has_signature && (
        <p className="rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-3 py-2 text-xs text-tone-orange">
          {sender.name} has no complete email signature yet (Settings, Email signature), so the company name and
          email are shown instead.
        </p>
      )}
      <p className="text-xs text-fg-muted">
        Every email ends with an unsubscribe link, which opts the recipient out of all marketing email.
      </p>
    </div>
  );
}
