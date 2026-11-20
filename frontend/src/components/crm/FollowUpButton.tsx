"use client";

import { useEffect, useState } from "react";
import { BellPlus, BellRing, Check } from "lucide-react";
import { Button, Dialog, Field, IconButton, Input, Textarea, toast } from "@/components/ui";
import { cn } from "@/lib/utils";
import { remindersApi, type CrmReminder, type ReminderDefaults } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";

interface Props {
  /** contact | lead | deal | quotation | email_thread | whatsapp_chat */
  entityType: string;
  entityId?: string;
  chatId?: string;
  /** Pre-fills the reminder title, e.g. the customer or quotation number. */
  subject: string;
  compact?: boolean;
  /** Trigger styled for a dark header (the WhatsApp chat). */
  dark?: boolean;
}

function whenLabel(iso: string) {
  return new Date(iso).toLocaleString("en-GB", { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
}

function toLocalInput(iso: string) {
  const d = new Date(iso);
  return new Date(d.getTime() - d.getTimezoneOffset() * 60000).toISOString().slice(0, 16);
}

/** "Remind me" on a chat, thread, lead, deal or quotation: a private follow-up for the person who sets it. */
export default function FollowUpButton({ entityType, entityId, chatId, subject, compact, dark }: Props) {
  const [open, setOpen] = useState(false);
  const [defaults, setDefaults] = useState<ReminderDefaults | null>(null);
  const [existing, setExisting] = useState<CrmReminder[]>([]);
  const [title, setTitle] = useState("");
  const [note, setNote] = useState("");
  const [due, setDue] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const key = `${entityType}:${entityId ?? chatId ?? ""}`;
  const load = () => {
    if (!entityId && !chatId) return;
    remindersApi.forRecord({ entity_type: entityType, entity_id: entityId, chat_id: chatId })
      .then((r) => setExisting(r.data)).catch(() => setExisting([]));
  };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { load(); }, [key]);

  async function openDialog() {
    setError("");
    setTitle(`Follow up: ${subject}`.slice(0, 120));
    setNote("");
    setOpen(true);
    try {
      const res = await remindersApi.defaults();
      setDefaults(res.data);
      setDue(toLocalInput(res.data.default_due_at));
    } catch {
      setDue(toLocalInput(new Date(Date.now() + 3 * 864e5).toISOString()));
    }
  }

  async function save() {
    setSaving(true);
    setError("");
    try {
      await remindersApi.create({
        entity_type: entityType, entity_id: entityId, chat_id: chatId,
        title: title.trim() || `Follow up: ${subject}`, note: note.trim() || undefined,
        due_at: new Date(due).toISOString(),
      });
      toast.success(`Reminder set for ${whenLabel(new Date(due).toISOString())}`);
      setOpen(false);
      load();
    } catch (e) {
      setError(extractApiError(e, "The reminder could not be set."));
    } finally {
      setSaving(false);
    }
  }

  async function complete(id: string) {
    try {
      await remindersApi.done(id);
      toast.success("Follow-up marked done");
      load();
    } catch (e) {
      toast.error(extractApiError(e, "Could not update the reminder."));
    }
  }

  const next = existing[0];
  const hint = next ? `Reminder set for ${whenLabel(next.due_at)}` : "Set a follow-up reminder";
  return (
    <>
      {dark ? (
        <IconButton onClick={openDialog} label={hint} icon={next ? BellRing : BellPlus}
          className={next ? "bg-tone-orange-soft text-tone-orange hover:bg-tone-orange-soft" : undefined} />
      ) : (
        <Button onClick={openDialog} title={hint} size={compact ? "sm" : "md"}
          className={next ? "border-tone-orange/40 bg-tone-orange-soft text-tone-orange hover:bg-tone-orange-soft" : undefined}>
          {next ? <BellRing aria-hidden /> : <BellPlus aria-hidden />}
          <span className="tabular">{next ? whenLabel(next.due_at) : "Remind me"}</span>
        </Button>
      )}

      <Dialog open={open} onOpenChange={setOpen} title="Remind me to follow up"
        description="Only you see this. You will get it in the bell and on WhatsApp; if it is still open the next day, by email too."
        footer={(
          <>
            <Button onClick={() => setOpen(false)}>Cancel</Button>
            <Button variant="primary" onClick={save} loading={saving} disabled={!due}>
              {!saving && <BellPlus aria-hidden />} Set reminder
            </Button>
          </>
        )}>
        <div className="space-y-4">
          {existing.length > 0 && (
            <div className="space-y-1.5 rounded-lg border border-tone-orange/30 bg-tone-orange-soft p-3">
              <p className="text-xs font-semibold text-tone-orange">Already set</p>
              {existing.map((r) => (
                <div key={r.id} className="flex items-center gap-2 text-xs text-fg">
                  <span className="flex-1 tabular">{whenLabel(r.due_at)}, {r.title}</span>
                  <Button size="sm" variant="ghost" onClick={() => complete(r.id)}>
                    <Check aria-hidden /> Done
                  </Button>
                </div>
              ))}
            </div>
          )}
          <Field label="What for">
            <Input value={title} onChange={(e) => setTitle(e.target.value)} />
          </Field>
          <Field label="When">
            <div className="space-y-2">
              {(defaults?.presets ?? []).length > 0 && (
                <div className="flex flex-wrap gap-1.5">
                  {(defaults?.presets ?? []).map((p) => {
                    const on = due === toLocalInput(p.due_at);
                    return (
                      <button key={p.key} type="button" aria-pressed={on} onClick={() => setDue(toLocalInput(p.due_at))}
                        className={cn("inline-flex min-h-9 items-center rounded-lg border px-2.5 text-xs font-medium transition-colors md:min-h-7",
                          on ? "border-brand bg-brand/[0.07] text-brand-ink" : "border-line text-fg hover:bg-surface-2")}>
                        {p.label}
                      </button>
                    );
                  })}
                </div>
              )}
              <Input type="datetime-local" value={due} onChange={(e) => setDue(e.target.value)} aria-label="Remind me at" />
            </div>
          </Field>
          <Field label="Note (optional)" error={error || undefined}>
            <Textarea value={note} onChange={(e) => setNote(e.target.value)} rows={2}
              placeholder="e.g. ask about the revised budget" />
          </Field>
        </div>
      </Dialog>
    </>
  );
}
