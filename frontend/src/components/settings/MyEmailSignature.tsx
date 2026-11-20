"use client";

import { useCallback, useEffect, useState } from "react";
import { emailSignatureApi } from "@/lib/api";
import { Save, Check, Mail } from "lucide-react";
import { Button, Card, CardBody, CardFooter, CardHeader, Field, FieldGrid, Input, Skeleton, Switch } from "@/components/ui";

type SigField = { name: string; label: string; value: string };

const NOTICE = "rounded-lg border border-tone-orange/30 bg-tone-orange-soft px-3 py-2 text-xs text-tone-orange";

/**
 * The staff-facing half of the signature: fill in your own details, see the result.
 * The layout itself is fixed by the company template.
 */
export default function MyEmailSignature() {
  const [fields, setFields] = useState<SigField[]>([]);
  const [enabled, setEnabled] = useState(true);
  const [configured, setConfigured] = useState(false);
  const [companyEnabled, setCompanyEnabled] = useState(true);
  const [html, setHtml] = useState("");
  const [missing, setMissing] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  const load = useCallback(async () => {
    try {
      const res = await emailSignatureApi.getMine();
      setFields(res.data.fields || []);
      setEnabled(res.data.enabled !== false);
      setConfigured(!!res.data.configured);
      setCompanyEnabled(res.data.company_enabled !== false);
      setMissing(res.data.missing || []);
      setHtml(res.data.html || "");
    } catch { /* ignore */ }
    finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const setField = (name: string, value: string) =>
    setFields((prev) => prev.map((f) => (f.name === name ? { ...f, value } : f)));

  async function save() {
    setSaving(true);
    setSaved(false);
    try {
      const payload = Object.fromEntries(fields.map((f) => [f.name, f.value]));
      const res = await emailSignatureApi.saveMine(payload, enabled);
      setHtml(res.data.html || "");
      setMissing(fields.filter((f) => !f.value.trim()).map((f) => f.label));
      setSaved(true);
      setTimeout(() => setSaved(false), 2500);
    } catch { /* ignore */ }
    finally { setSaving(false); }
  }

  const title = <span className="inline-flex items-center gap-2"><Mail className="size-4 text-brand-ink" aria-hidden /> Email signature</span>;

  if (loading) return <Card><CardBody className="space-y-2"><Skeleton className="h-4 w-1/3" /><Skeleton className="h-9 w-full" /></CardBody></Card>;

  if (!configured) {
    return (
      <Card>
        <CardHeader title={title} />
        <CardBody>
          <p className="text-sm text-fg-muted">
            No company signature template has been set up yet. An administrator can add one in
            Settings, Email &amp; WhatsApp.
          </p>
        </CardBody>
      </Card>
    );
  }

  return (
    <Card>
      <CardHeader title={title}
        description="Your details in the company signature. It is added to messages you write in Messaging, Email and to CRM inbox replies."
        actions={<Switch label={enabled ? "On" : "Off"} checked={enabled} onCheckedChange={setEnabled} />} />
      <CardBody className="space-y-4">
        {missing.length > 0 && (
          <p className={NOTICE}>
            Your signature is not being added to emails yet. Fill in{" "}
            <strong className="font-semibold">{missing.join(", ")}</strong> so it never goes out with a gap where your
            details belong.
          </p>
        )}

        {!companyEnabled && (
          <p className={NOTICE}>
            Signatures are currently switched off company-wide, so nothing will be added to your
            messages.
          </p>
        )}

        <FieldGrid cols={2}>
          {fields.map((f) => (
            <Field key={f.name} label={f.label}>
              <Input value={f.value} onChange={(e) => setField(f.name, e.target.value)} />
            </Field>
          ))}
        </FieldGrid>

        {!html && missing.length === 0 && enabled && (
          <p className="text-xs text-fg-muted">Save to see the preview.</p>
        )}

        {html && (
          <div>
            <p className="mb-1.5 text-xs font-medium text-fg-muted">Preview</p>
            <div className="overflow-x-auto rounded-lg border border-line p-4" style={{ background: "white", color: "black" }}>
              {/* Company-authored template with this user's values already substituted, on white as in a mail client. */}
              <div dangerouslySetInnerHTML={{ __html: html }} />
            </div>
          </div>
        )}
      </CardBody>
      <CardFooter>
        {saved && (
          <span className="flex items-center gap-1 text-xs text-fg-muted">
            <Check className="size-3.5" aria-hidden /> Saved
          </span>
        )}
        <Button variant="primary" onClick={save} loading={saving}>
          {!saving && <Save aria-hidden />} Save signature
        </Button>
      </CardFooter>
    </Card>
  );
}
