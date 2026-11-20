"use client";

import { useEffect, useState } from "react";
import { CheckCircle2, XCircle } from "lucide-react";
import { Button, Card, CardBody, CardFooter, Field, Input, Textarea } from "@/components/ui";
import { systemApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { useDraftSource } from "@/components/settings/SettingsDraft";

/** Nary's acknowledgement texts (setting nary.ack.templates), one card per language. {name} and
 *  {reply_window} are filled in when it is sent; the late prefix starts a late acknowledgement. */
export type AckTemplate = {
  in_hours: string; after_hours: string; late_prefix: string; name_fallback: string; reply_window: string; email_subject: string;
};
export const ACK_FIELDS: { key: keyof AckTemplate; label: string; multiline?: boolean }[] = [
  { key: "in_hours", label: "In working hours", multiline: true },
  { key: "after_hours", label: "After working hours", multiline: true },
  { key: "late_prefix", label: "Late prefix (an apology)" },
  { key: "name_fallback", label: "When the name is unknown" },
  { key: "reply_window", label: "{reply_window}" },
  { key: "email_subject", label: "Email subject (web forms)" },
];
const LANG: Record<string, string> = { en: "English", fr: "French" };

export function parseTemplates(raw: string | undefined): Record<string, AckTemplate> | null {
  try {
    const v = JSON.parse(raw || "");
    return v && typeof v === "object" && !Array.isArray(v) ? v : null;
  } catch {
    return null;
  }
}

function countChanges(a: Record<string, AckTemplate> | null, b: Record<string, AckTemplate> | null) {
  if (!a || !b) return 0;
  let n = 0;
  for (const lang of Object.keys(a)) {
    for (const f of ACK_FIELDS) if ((a[lang]?.[f.key] ?? "") !== (b[lang]?.[f.key] ?? "")) n++;
  }
  return n;
}

export default function NaryAckTemplatesBlock({ value, onSaved }: { value: string | undefined; onSaved: () => void }) {
  const [tpl, setTpl] = useState<Record<string, AckTemplate> | null>(parseTemplates(value));
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  useEffect(() => { setTpl(parseTemplates(value)); }, [value]);

  const missing = tpl
    ? Object.entries(tpl).flatMap(([lang, t]) =>
      ACK_FIELDS.filter((f) => !(t?.[f.key] || "").trim()).map((f) => `${LANG[lang] ?? lang}: ${f.label}`))
    : [];

  const persist = async () => {
    await systemApi.updateSetting("nary.ack.templates", JSON.stringify(tpl));
    onSaved();
  };

  const { inSaveBar } = useDraftSource("nary-ack-templates", {
    count: countChanges(tpl, parseTemplates(value)),
    save: persist,
    discard: () => setTpl(parseTemplates(value)),
    validate: () => (missing.length ? `Acknowledgement texts: fill in ${missing.join("; ")}.` : null),
  });

  if (!tpl) {
    return <p className="text-sm text-tone-red">nary.ack.templates is not valid JSON; fix it in the list above.</p>;
  }

  const save = async () => {
    setSaving(true);
    setMsg(null);
    try {
      await persist();
      setMsg({ ok: true, text: "Saved." });
    } catch (e) {
      setMsg({ ok: false, text: extractApiError(e) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <Card>
      <CardBody className="space-y-4">
        <p className="text-[13px] text-fg-muted">
          Sent once to a new business conversation, in the customer&apos;s language, signed by Nary. {"{name}"} becomes
          their first name, {"{reply_window}"} the line below. Turn it on with nary.ack.enabled when WhatsApp&apos;s own
          greeting is switched off.
        </p>
        <div className="grid gap-4 md:grid-cols-2">
          {Object.entries(tpl).map(([lang, t]) => (
            <fieldset key={lang} className="space-y-3 rounded-lg border border-line p-3">
              <legend className="px-1 text-xs font-semibold text-fg-muted">{LANG[lang] ?? lang}</legend>
              {ACK_FIELDS.map((f) => (
                <Field key={f.key} label={f.label}>
                  {f.multiline ? (
                    <Textarea rows={3} value={t[f.key] ?? ""} aria-label={`${LANG[lang] ?? lang} ${f.label}`}
                      onChange={(e) => setTpl({ ...tpl, [lang]: { ...t, [f.key]: e.target.value } })} />
                  ) : (
                    <Input value={t[f.key] ?? ""} aria-label={`${LANG[lang] ?? lang} ${f.label}`}
                      onChange={(e) => setTpl({ ...tpl, [lang]: { ...t, [f.key]: e.target.value } })} />
                  )}
                </Field>
              ))}
            </fieldset>
          ))}
        </div>
        {missing.length > 0 && <p className="text-xs text-tone-red">Fill in: {missing.join("; ")}.</p>}
      </CardBody>
      {!inSaveBar && (
        <CardFooter className="justify-start">
          <Button variant="primary" onClick={save} loading={saving} disabled={missing.length > 0}>Save texts</Button>
          {msg && (
            <span className={`inline-flex items-center gap-1 text-xs ${msg.ok ? "text-fg-muted" : "text-tone-red"}`}>
              {msg.ok ? <CheckCircle2 className="size-3.5" aria-hidden /> : <XCircle className="size-3.5" aria-hidden />} {msg.text}
            </span>
          )}
        </CardFooter>
      )}
    </Card>
  );
}
