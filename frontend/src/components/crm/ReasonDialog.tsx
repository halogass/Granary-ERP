"use client";

import { useState } from "react";
import { Button, Checkbox, Dialog, Field, Textarea } from "@/components/ui";
import type { FunnelReason } from "@/lib/crmStages";

interface Props {
  title: string;
  reasons: FunnelReason[];
  /** Several reasons (a lost deal) or exactly one (a disqualified lead). */
  multiple?: boolean;
  withNotes?: boolean;
  saving?: boolean;
  error?: string | null;
  onCancel: () => void;
  onConfirm: (reasons: string[], notes: string) => void;
}

/** Pick the reasons a lost / disqualified move needs, from the picklists in Settings. */
export default function ReasonDialog({ title, reasons, multiple = false, withNotes = false, saving, error,
                                       onCancel, onConfirm }: Props) {
  const [picked, setPicked] = useState<string[]>([]);
  const [notes, setNotes] = useState("");

  const toggle = (key: string) =>
    setPicked((prev) => multiple
      ? (prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key])
      : [key]);

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onCancel(); }} title={title}
      description={multiple ? "Select every reason that applies" : "Select the reason"}
      footer={<>
        <Button onClick={onCancel}>Cancel</Button>
        <Button variant="primary" disabled={!picked.length} loading={saving} onClick={() => onConfirm(picked, notes)}>
          {saving ? "Saving" : "Confirm"}
        </Button>
      </>}>
      <div className="space-y-1">
        <div role={multiple ? "group" : "radiogroup"} aria-label="Reasons" className="flex flex-col">
          {reasons.map((r) => (
            multiple ? (
              <div key={r.key} className="flex items-center gap-2 rounded-lg px-2 hover:bg-surface-2">
                <Checkbox className="flex-1" label={r.label} checked={picked.includes(r.key)} onChange={() => toggle(r.key)} />
                {r.recontact_months != null && <span className="text-xs text-fg-muted">re-contact in {r.recontact_months} mo</span>}
              </div>
            ) : (
              <label key={r.key} className="flex min-h-11 cursor-pointer items-center gap-2.5 rounded-lg px-2 text-sm text-fg hover:bg-surface-2 md:min-h-9">
                <input type="radio" name="reason" className="size-4 accent-[rgb(var(--brand))]"
                  checked={picked.includes(r.key)} onChange={() => toggle(r.key)} />
                <span className="flex-1">{r.label}</span>
                {r.recontact_months != null && <span className="text-xs text-fg-muted">re-contact in {r.recontact_months} mo</span>}
              </label>
            )
          ))}
        </div>
        {withNotes && (
          <Field label="Notes" className="pt-2">
            <Textarea value={notes} onChange={(e) => setNotes(e.target.value)} rows={3}
              placeholder="Loss analysis: who won, the deciding factor, what we could have done differently" />
          </Field>
        )}
        {error && <p className="text-xs text-tone-red">{error}</p>}
      </div>
    </Dialog>
  );
}
