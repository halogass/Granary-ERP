"use client";

import { useState } from "react";
import { Clock, Send } from "lucide-react";
import { Button, Dialog, Field, Input } from "@/components/ui";

export interface ScheduledMsg {
  id: string;
  channel: string;
  chat_id: string;
  recipient: string;
  subject: string | null;
  body: string;
  scheduled_at: string;
  sent_at: string | null;
  status: string;
  retry_count: number;
  last_error: string | null;
  contact_id: string | null;
  created_by_id: string;
  created_at: string;
}

interface Props {
  channel: "whatsapp" | "email";
  chatId: string;
  recipient: string;
  body: string;
  subject?: string;
  accessToken: string;
  editingId?: string;
  onScheduled: (msg: ScheduledMsg) => void;
  onClose: () => void;
}

function toLocalDatetimeString(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function getPresets(): { label: string; date: Date }[] {
  const now = new Date();
  const tomorrow9 = new Date(now);
  tomorrow9.setDate(now.getDate() + 1);
  tomorrow9.setHours(9, 0, 0, 0);

  const afternoon2 = new Date(now);
  afternoon2.setHours(14, 0, 0, 0);
  if (afternoon2 <= now) afternoon2.setDate(afternoon2.getDate() + 1);

  const nextMonday9 = new Date(now);
  const daysUntilMonday = (8 - now.getDay()) % 7 || 7;
  nextMonday9.setDate(now.getDate() + daysUntilMonday);
  nextMonday9.setHours(9, 0, 0, 0);

  return [
    { label: "Tomorrow 9am", date: tomorrow9 },
    { label: "This afternoon 2pm", date: afternoon2 },
    { label: "Next Monday 9am", date: nextMonday9 },
  ];
}

export default function ScheduleSendModal({
  channel, chatId, recipient, body, subject, accessToken, editingId, onScheduled, onClose,
}: Props) {
  const presets = getPresets();
  const minDatetime = toLocalDatetimeString(new Date(Date.now() + 5 * 60 * 1000));

  const [customDatetime, setCustomDatetime] = useState(presets[0] ? toLocalDatetimeString(presets[0].date) : "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  async function schedule(dt: Date) {
    setSaving(true);
    setError("");
    try {
      let res: Response;
      if (editingId) {
        res = await fetch(`/api/v1/crm/scheduled/${editingId}`, {
          method: "PATCH",
          headers: { "Content-Type": "application/json", Authorization: `Bearer ${accessToken}` },
          body: JSON.stringify({ body, subject: subject || undefined, scheduled_at: dt.toISOString() }),
        });
      } else {
        res = await fetch("/api/v1/crm/scheduled", {
          method: "POST",
          headers: { "Content-Type": "application/json", Authorization: `Bearer ${accessToken}` },
          body: JSON.stringify({
            channel, chat_id: chatId, recipient, body,
            subject: subject || undefined, scheduled_at: dt.toISOString(),
          }),
        });
      }
      if (!res.ok) {
        const data = await res.json().catch(() => ({}));
        throw new Error(data.detail || `Error ${res.status}`);
      }
      const msg: ScheduledMsg = await res.json();
      onScheduled(msg);
      onClose();
    } catch (e: any) {
      setError(e.message || "Failed to schedule message");
    } finally {
      setSaving(false);
    }
  }

  function scheduleCustom() {
    if (!customDatetime) return;
    schedule(new Date(customDatetime));
  }

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }} size="sm"
      title={editingId ? "Edit schedule" : "Schedule send"}
      description="The message will be sent automatically.">
      <div className="space-y-4">
        <div className="rounded-lg border border-line bg-surface-2 px-3.5 py-2.5">
          <p className="mb-1 text-xs font-medium text-fg-muted">Message preview</p>
          <p className="line-clamp-2 text-sm text-fg">{body || "(empty)"}</p>
        </div>

        <div>
          <p className="mb-2 text-[13px] font-medium text-fg">Quick presets</p>
          <div className="grid grid-cols-1 gap-2">
            {presets.map((p) => (
              <button
                key={p.label}
                type="button"
                onClick={() => schedule(p.date)}
                disabled={saving}
                className="flex min-h-11 items-center justify-between rounded-lg border border-line px-3.5 py-2 text-left text-sm font-medium text-fg transition-colors hover:bg-surface-2 disabled:opacity-50 md:min-h-9"
              >
                <span className="inline-flex items-center gap-2"><Clock className="size-4 text-fg-subtle" aria-hidden />{p.label}</span>
                <span className="text-xs text-fg-muted tabular">{p.date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })} {p.date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}</span>
              </button>
            ))}
          </div>
        </div>

        <Field label="Custom time" error={error || undefined}>
          <div className="flex gap-2">
            <Input
              type="datetime-local"
              value={customDatetime}
              min={minDatetime}
              onChange={(e) => setCustomDatetime(e.target.value)}
              className="flex-1"
            />
            <Button variant="primary" onClick={scheduleCustom} loading={saving} disabled={!customDatetime}>
              {!saving && <Send aria-hidden />}
              Schedule
            </Button>
          </div>
        </Field>
      </div>
    </Dialog>
  );
}
