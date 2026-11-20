"use client";

import { useCallback, useEffect, useState } from "react";
import { authApi, whatsappApi } from "@/lib/api";
import { Save, Check, MessageCircle, Lock } from "lucide-react";
import { Button, Card, CardBody, CardHeader, Field, Input, Textarea } from "@/components/ui";

type SignatureInfo = {
  signature: string;
  name: string;
  auto_name: string;
  name_override: string | null;
  custom: string | null;
  can_customize: boolean;
};

/** Render WhatsApp's `_italic_` / `*bold*` the way the customer will see it. */
function WaText({ text }: { text: string }) {
  const parts = text.split(/(_[^_\n]+_|\*[^*\n]+\*)/g);
  return (
    <>
      {parts.map((p, i) =>
        /^_[^_\n]+_$/.test(p) ? <em key={i}>{p.slice(1, -1)}</em>
          : /^\*[^*\n]+\*$/.test(p) ? <strong key={i}>{p.slice(1, -1)}</strong>
          : <span key={i}>{p}</span>,
      )}
    </>
  );
}

/**
 * The sign-off added to every WhatsApp message this user sends from the ERP. Staff can shorten
 * the name; only the superadmin (Owner) can replace the whole line with their own text.
 * Hidden for users without access to the WhatsApp inbox.
 */
export default function MyWhatsAppSignature() {
  const [info, setInfo] = useState<SignatureInfo | null>(null);
  const [name, setName] = useState("");
  const [custom, setCustom] = useState("");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    try {
      const res = await whatsappApi.getSignature();
      const d: SignatureInfo = res.data;
      setInfo(d);
      setName(d.name_override ?? "");
      // Prefill the Owner's text with what they sign with today, so editing starts from it.
      setCustom(d.custom ?? d.signature);
    } catch { setInfo(null); }
  }, []);

  useEffect(() => { load(); }, [load]);

  if (!info) return null;

  const company = info.signature.match(/, (.+)_$/)?.[1];
  const previewName = name.trim() || info.auto_name;
  const preview = info.can_customize && custom.trim()
    ? custom.trim()
    : `_\u2014 ${previewName}${company ? `, ${company}` : ""}_`;

  async function save() {
    setSaving(true);
    setSaved(false);
    setError("");
    try {
      await authApi.updateMe({
        wa_signature_name: name.trim(),
        ...(info?.can_customize ? { wa_signature_custom: custom.trim() } : {}),
      });
      await load();
      setSaved(true);
      setTimeout(() => setSaved(false), 2500);
    } catch (e: unknown) {
      setError((e as { response?: { data?: { detail?: string } } })?.response?.data?.detail
        ?? "Could not save the signature");
    } finally { setSaving(false); }
  }

  return (
    <Card className="mt-5">
      <CardHeader
        title={<span className="inline-flex items-center gap-2"><MessageCircle className="size-4 text-brand-ink" aria-hidden /> WhatsApp signature</span>}
        description="Added to every WhatsApp message you send from the ERP, so customers know who is replying."
        actions={
          <Button variant="primary" size="sm" onClick={save} loading={saving}>
            {!saving && (saved ? <Check aria-hidden /> : <Save aria-hidden />)}
            {saved ? "Saved" : "Save"}
          </Button>
        } />
      <CardBody className="space-y-4">
        {info.can_customize ? (
          <Field label="Your signature text"
            hint={<span className="flex items-center gap-1.5"><Lock className="size-3 shrink-0" aria-hidden />
              Sent exactly as written. Only you can change it. Use _underscores_ for italics and *asterisks* for bold.
              Clear it to go back to the standard line.</span>}>
            <Textarea value={custom} onChange={(e) => setCustom(e.target.value)} rows={2} maxLength={300} className="font-mono" />
          </Field>
        ) : (
          <Field label="Name in the signature"
            hint={<>Leave empty to use &ldquo;{info.auto_name}&rdquo; (your first name and initials).</>}>
            <Input value={name} onChange={(e) => setName(e.target.value)} maxLength={60} placeholder={info.auto_name} />
          </Field>
        )}

        <div>
          <p className="mb-1.5 text-xs font-medium text-fg-muted">Preview</p>
          <div className="max-w-sm whitespace-pre-wrap rounded-lg border border-line bg-surface-2 px-4 py-3 text-sm text-fg">
            Hello, thank you for contacting us.{"\n\n"}<WaText text={preview} />
          </div>
        </div>

        {error && <p className="text-xs text-tone-red">{error}</p>}
      </CardBody>
    </Card>
  );
}
