"use client";

import { useCallback, useEffect, useState } from "react";
import { emailSignatureApi } from "@/lib/api";
import { Save, AlertTriangle, Check, Code2, Eye } from "lucide-react";
import { Button, Card, CardBody, CardFooter, Pill, Skeleton, Switch, Tabs, TabsList, TabsTrigger } from "@/components/ui";
import { useDraftSource } from "@/components/settings/SettingsDraft";

type Field = { name: string; label: string };

/**
 * The company-wide signature template. Admins own the HTML; everyone else only fills in
 * the {{placeholders}} it declares (see Profile, Email signature).
 */
export default function EmailSignatureTemplate() {
  const [html, setHtml] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [saved, setSaved] = useState({ html: "", enabled: true });
  const [fields, setFields] = useState<Field[]>([]);
  const [assetCount, setAssetCount] = useState(0);
  const [tab, setTab] = useState<"html" | "preview">("html");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; msg: string; problems?: string[] } | null>(null);

  const load = useCallback(async () => {
    try {
      const res = await emailSignatureApi.getTemplate();
      setHtml(res.data.html || "");
      setEnabled(res.data.enabled !== false);
      setSaved({ html: res.data.html || "", enabled: res.data.enabled !== false });
      setFields(res.data.fields || []);
      setAssetCount(res.data.asset_count || 0);
    } catch { /* non-admins never see this component */ }
    finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  async function persist() {
    setResult(null);
    const res = await emailSignatureApi.saveTemplate(html, enabled);
    setHtml(res.data.html || "");
    setFields(res.data.fields || []);
    const imported = res.data.images_imported || 0;
    const problems: string[] = res.data.problems || [];
    setResult({
      ok: problems.length === 0,
      msg: imported
        ? `Saved. ${imported} image${imported === 1 ? "" : "s"} copied onto our server.`
        : "Saved.",
      problems,
    });
    await load();
  }

  const { inSaveBar } = useDraftSource("email-signature-template", {
    count: (html !== saved.html ? 1 : 0) + (enabled !== saved.enabled ? 1 : 0),
    save: persist,
    discard: () => { setHtml(saved.html); setEnabled(saved.enabled); },
  });

  async function save() {
    setSaving(true);
    try {
      await persist();
    } catch (e) {
      const detail = (e as { response?: { data?: { detail?: string } } })?.response?.data?.detail;
      setResult({ ok: false, msg: detail || "Could not save the template" });
    } finally { setSaving(false); }
  }

  if (loading) {
    return <Card><CardBody className="space-y-2"><Skeleton className="h-4 w-1/3" /><Skeleton className="h-40 w-full" /></CardBody></Card>;
  }

  return (
    <Card>
      <CardBody className="space-y-4">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <p className="max-w-2xl text-[13px] leading-relaxed text-fg-muted">
            Paste the signature HTML once. Use <code className="font-mono text-brand-ink">{"{{name}}"}</code>,{" "}
            <code className="font-mono text-brand-ink">{"{{title}}"}</code>,{" "}
            <code className="font-mono text-brand-ink">{"{{mobile}}"}</code> and so on wherever a personal
            detail belongs; each one becomes a field staff fill in on their profile. Images are
            copied onto our server when you save and travel inside every email.
          </p>
          <Switch label={enabled ? "Enabled" : "Disabled"} checked={enabled} onCheckedChange={setEnabled} />
        </div>

        <div className="flex items-center gap-2">
          <Tabs value={tab} onValueChange={(v) => setTab(v as "html" | "preview")}>
            <TabsList>
              <TabsTrigger value="html"><Code2 className="size-3.5" aria-hidden /> HTML</TabsTrigger>
              <TabsTrigger value="preview"><Eye className="size-3.5" aria-hidden /> Preview</TabsTrigger>
            </TabsList>
          </Tabs>
          {assetCount > 0 && (
            <span className="ml-auto text-xs text-fg-muted">{assetCount} image(s) hosted here</span>
          )}
        </div>

        {tab === "html" ? (
          <textarea value={html} onChange={(e) => setHtml(e.target.value)} rows={16} spellCheck={false}
            aria-label="Signature HTML" placeholder="<table>...</table>"
            className="w-full rounded-lg border border-line-strong bg-surface px-3 py-2 font-mono text-xs text-fg focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20" />
        ) : (
          <div className="overflow-x-auto rounded-lg border border-line p-4" style={{ background: "white", color: "black" }}>
            {/* Admin-authored markup, shown as the recipient sees it (on white, as in a mail client).
                Placeholders stay literal. The marker opts this block out of the app's image reset so the
                preview matches a mail client rather than our Tailwind styling. */}
            <div data-granary-signature="1" dangerouslySetInnerHTML={{ __html: html }} />
          </div>
        )}

        {fields.length > 0 && (
          <div>
            <p className="mb-1.5 text-xs font-medium text-fg-muted">Fields staff will fill in</p>
            <div className="flex flex-wrap gap-1.5">
              {fields.map((f) => <Pill key={f.name} tone="neutral">{f.label}</Pill>)}
            </div>
          </div>
        )}

        {result && (
          <div className={`rounded-lg border px-4 py-3 text-sm ${result.ok
            ? "border-line bg-surface-2 text-fg" : "border-tone-orange/30 bg-tone-orange-soft text-tone-orange"}`}>
            <p className="flex items-center gap-1.5 font-medium">
              {result.ok ? <Check className="size-4" aria-hidden /> : <AlertTriangle className="size-4" aria-hidden />}
              {result.msg}
            </p>
            {result.problems && result.problems.length > 0 && (
              <ul className="mt-1.5 list-inside list-disc space-y-0.5 text-xs">
                {result.problems.map((p, i) => <li key={i}>{p}</li>)}
              </ul>
            )}
          </div>
        )}
      </CardBody>
      {!inSaveBar && (
        <CardFooter>
          <Button variant="primary" onClick={save} loading={saving}>
            {!saving && <Save aria-hidden />}
            {saving ? "Saving and copying images..." : "Save template"}
          </Button>
        </CardFooter>
      )}
    </Card>
  );
}
