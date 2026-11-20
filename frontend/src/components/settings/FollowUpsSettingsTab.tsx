"use client";

import { useCallback, useEffect, useState } from "react";
import { Save } from "lucide-react";
import { Button, Card, CardBody, CardFooter, Checkbox, Field, FieldGrid, Input, Select, Skeleton, toast } from "@/components/ui";
import { systemApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { useDraftSource } from "@/components/settings/SettingsDraft";

interface Setting { key: string; value: string }

const NUMBERS: { key: string; label: string; hint: string; min: number; max: number }[] = [
  { key: "default_delay_days", label: "Default follow-up delay", hint: "Business days ahead when someone taps “Remind me”", min: 1, max: 30 },
  { key: "no_reply_days", label: "No reply after", hint: "Days of silence after our last WhatsApp or email before a reminder is raised", min: 1, max: 60 },
  { key: "lead_idle_days", label: "Lead idle after", hint: "Days with no activity on an open lead", min: 1, max: 120 },
  { key: "deal_stage_idle_days", label: "Deal stuck after", hint: "Days a deal may sit in one stage", min: 1, max: 180 },
  { key: "quotation_followup_days", label: "Quotation unanswered after", hint: "Days a sent quotation may go without an answer", min: 1, max: 90 },
  { key: "due_hour", label: "Due hour", hint: "Hour of the day (company time, 0 to 23) follow-ups fall due", min: 0, max: 23 },
  { key: "escalate_after_hours", label: "Email if still open after", hint: "Hours after the due notice before the owner is emailed", min: 1, max: 168 },
  { key: "digest_max_items", label: "Follow-ups per WhatsApp", hint: "Follow-ups that fall due together arrive as one WhatsApp; this many are listed, the rest as “+N more”", min: 1, max: 50 },
];

const RULES: { key: string; label: string }[] = [
  { key: "rule_no_reply", label: "Remind when a customer has not replied" },
  { key: "rule_stale_lead", label: "Remind about idle leads" },
  { key: "rule_stale_deal", label: "Remind about deals stuck in a stage" },
  { key: "rule_quotation", label: "Remind about sent quotations with no answer" },
  { key: "rule_next_action", label: "Remind when a planned next action falls due" },
];

const WEEKDAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/** Settings, Sales & CRM, Follow-ups: the thresholds the reminder rules use, the delivery window and the Owner digest. */
export default function FollowUpsSettingsTab() {
  const [values, setValues] = useState<Record<string, string>>({});
  const [saved, setSaved] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await systemApi.listSettings();
      const all: Setting[] = res.data.items ?? res.data ?? [];
      const v = Object.fromEntries(all
        .filter((s) => s.key.startsWith("crm.followup."))
        .map((s) => [s.key.replace("crm.followup.", ""), s.value ?? ""]));
      setValues(v);
      setSaved(v);
    } catch {
      setValues({});
      setSaved({});
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => { load(); }, [load]);

  const set = (key: string, value: string) => setValues((v) => ({ ...v, [key]: value }));
  const bool = (key: string) => (values[key] ?? "true").toLowerCase() !== "false";
  const changed = Object.keys(values).filter((k) => (values[k] ?? "") !== (saved[k] ?? "")).length;

  async function persist() {
    await Promise.all(Object.entries(values).map(([key, value]) =>
      systemApi.updateSetting(`crm.followup.${key}`, value)));
    await load();
  }

  const { inSaveBar } = useDraftSource("crm-followups", {
    count: changed,
    save: persist,
    discard: () => setValues(saved),
  });

  async function save() {
    setSaving(true);
    try {
      await persist();
      toast.success("Follow-up settings saved");
    } catch (e) {
      toast.error(extractApiError(e, "Could not save the settings."));
    } finally {
      setSaving(false);
    }
  }

  if (loading) {
    return (
      <Card><CardBody className="space-y-3">
        <Skeleton className="h-4 w-1/2" /><Skeleton className="h-4 w-2/3" /><Skeleton className="h-4 w-1/3" />
      </CardBody></Card>
    );
  }

  return (
    <Card>
      <CardBody className="space-y-6">
        <p className="text-[13px] text-fg-muted">
          When a quiet conversation or record should raise a reminder, and when reminders may be delivered.
        </p>
        <FieldGrid cols={2}>
          {NUMBERS.map((n) => (
            <Field key={n.key} label={n.label} hint={n.hint}>
              <Input type="number" numeric min={n.min} max={n.max} value={values[n.key] ?? ""}
                onChange={(e) => set(n.key, e.target.value)} className="md:w-32" />
            </Field>
          ))}
        </FieldGrid>

        <fieldset className="space-y-1">
          <legend className="mb-1 text-sm font-semibold text-fg">Rules</legend>
          {RULES.map((r) => (
            <div key={r.key}>
              <Checkbox label={r.label} checked={bool(r.key)}
                onChange={(e) => set(r.key, e.target.checked ? "true" : "false")} />
            </div>
          ))}
        </fieldset>

        <FieldGrid cols={2}>
          <div className="flex flex-col gap-1.5">
            <span className="text-[13px] font-medium text-fg">Delivery window</span>
            <div className="flex items-center gap-2">
              <Input type="time" aria-label="Reminders delivered from" value={values.quiet_from ?? "08:00"}
                onChange={(e) => set("quiet_from", e.target.value)} className="w-32" />
              <span className="text-xs text-fg-muted">to</span>
              <Input type="time" aria-label="Reminders delivered until" value={values.quiet_to ?? "20:00"}
                onChange={(e) => set("quiet_to", e.target.value)} className="w-32" />
            </div>
            <p className="text-xs text-fg-muted">Reminders wait outside these hours, so nobody is pinged at night (company time zone)</p>
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[13px] font-medium text-fg">Weekly digest to the Owner</span>
            <div className="flex items-center gap-2">
              <Select aria-label="Digest weekday" value={values.digest_weekday ?? "1"}
                onChange={(e) => set("digest_weekday", e.target.value)} className="w-40"
                options={WEEKDAYS.map((d, i) => ({ value: String(i + 1), label: d }))} />
              <Input type="time" aria-label="Digest time" value={values.digest_time ?? "08:00"}
                onChange={(e) => set("digest_time", e.target.value)} className="w-32" />
            </div>
            <p className="text-xs text-fg-muted">Everything overdue across the team, in one email</p>
          </div>
        </FieldGrid>
      </CardBody>
      {!inSaveBar && (
        <CardFooter>
          <Button variant="primary" onClick={save} loading={saving}>
            <Save aria-hidden /> Save follow-up settings
          </Button>
        </CardFooter>
      )}
    </Card>
  );
}
