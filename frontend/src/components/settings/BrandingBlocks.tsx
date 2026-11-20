"use client";

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { AlertCircle, ImageIcon, Trash2, Upload } from "lucide-react";
import { systemApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { invalidateLetterheadCache } from "@/lib/print";
import { invalidateBrandingCache } from "@/hooks/useBranding";
import { useDropToInput, isImage } from "@/hooks/useFileDrop";
import { Button, Card, CardBody, CardFooter, Field, FieldGrid, Input, Skeleton, Textarea, toast } from "@/components/ui";
import { cn } from "@/lib/utils";
import { useDraftSource } from "./SettingsDraft";

/** The text part of the branding, saved together with PATCH /system/branding (as before). */
const TEXT_FIELDS = [
  "logo_type", "logo_text", "logo_subtitle", "login_badge", "login_headline", "login_headline_accent",
  "login_tagline", "login_features", "login_footer", "login_subtitle", "login_copyright",
] as const;
type TextField = (typeof TEXT_FIELDS)[number];
type BrandingText = Record<TextField, string>;

const DEFAULTS: BrandingText = {
  logo_type: "text", logo_text: "Granary", logo_subtitle: "ERP",
  login_badge: "GRANARY",
  login_headline: "The ERP that thinks",
  login_headline_accent: "alongside your team.",
  login_tagline: "Nary, the built-in AI, works across sales, finance, people and operations: it drafts the next step and waits for your approval.",
  login_features: "AI-drafted quotations, proposals and replies\nLead enrichment, research and scoring\nEmail and WhatsApp triage with suggested replies\nEvery AI action approved by a person\nFinance, HR, projects and procurement in one place\nConnect your own AI client over MCP",
  login_footer: "Open-source ERP with built-in AI",
  login_subtitle: "Sign in to continue.",
  login_copyright: "Open-source ERP with built-in AI",
};

interface BrandingCtx {
  loaded: boolean;
  text: BrandingText;
  set: (k: TextField, v: string) => void;
  logo: string;
  favicon: string;
  letterhead: string | null;
  setLogo: (v: string) => void;
  setFavicon: (v: string) => void;
  setLetterhead: (v: string | null) => void;
  inSaveBar: boolean;
  saving: boolean;
  save: () => Promise<void>;
}

const Ctx = createContext<BrandingCtx | null>(null);

function useBrandingCtx() {
  const c = useContext(Ctx);
  if (!c) throw new Error("Branding blocks need <BrandingProvider>");
  return c;
}

/** Loads the branding and the letterhead once for the Company & branding area. */
export function BrandingProvider({ children }: { children: React.ReactNode }) {
  const [loaded, setLoaded] = useState(false);
  const [saved, setSaved] = useState<BrandingText>(DEFAULTS);
  const [text, setText] = useState<BrandingText>(DEFAULTS);
  const [logo, setLogo] = useState("");
  const [favicon, setFavicon] = useState("");
  const [letterhead, setLetterhead] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    const [br, lh] = await Promise.all([systemApi.getBranding(), systemApi.getLetterhead()]);
    const next = Object.fromEntries(TEXT_FIELDS.map((k) => [k, br.data[k] ?? DEFAULTS[k]])) as BrandingText;
    setSaved(next);
    setText(next);
    setLogo(br.data.logo_image_data ?? "");
    setFavicon(br.data.favicon_data ?? "");
    setLetterhead(lh.data.data_url ?? null);
    setLoaded(true);
  }, []);
  useEffect(() => { load().catch(() => setLoaded(true)); }, [load]);

  const count = TEXT_FIELDS.filter((k) => text[k] !== saved[k]).length;
  const save = useCallback(async () => {
    setSaving(true);
    try {
      await systemApi.updateBranding({ ...text });
      invalidateBrandingCache();
      await load();
    } finally {
      setSaving(false);
    }
  }, [text, load]);
  const { inSaveBar } = useDraftSource("branding", { count, save, discard: () => setText(saved) });

  const value = useMemo<BrandingCtx>(() => ({
    loaded, text, set: (k, v) => setText((t) => ({ ...t, [k]: v })), logo, favicon, letterhead,
    setLogo, setFavicon, setLetterhead, inSaveBar, saving, save,
  }), [loaded, text, logo, favicon, letterhead, inSaveBar, saving, save]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

function OwnSave({ label }: { label: string }) {
  const b = useBrandingCtx();
  if (b.inSaveBar) return null;
  return (
    <CardFooter>
      <Button variant="primary" loading={b.saving} onClick={() => b.save().then(() => toast.success("Branding saved"), (e) => toast.error(extractApiError(e)))}>
        {label}
      </Button>
    </CardFooter>
  );
}

function errorOf(e: unknown) {
  return extractApiError(e, "Upload failed");
}

/** An image upload area with drop support, preview, replace and remove. */
function ImageUpload({ current, accept, hint, emptyTitle, emptyHint, previewClass, onUpload, onRemove, uploadLabel, removeLabel }: {
  current: string | null; accept: string; hint: string; emptyTitle: string; emptyHint: string; previewClass: string;
  onUpload: (f: File) => Promise<void>; onRemove: () => Promise<void>; uploadLabel: [string, string]; removeLabel: string;
}) {
  const ref = useRef<HTMLInputElement>(null);
  const drop = useDropToInput(ref, { accept: isImage, single: true });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const run = async (fn: () => Promise<void>) => {
    setError(""); setBusy(true);
    try { await fn(); } catch (e) { setError(errorOf(e)); } finally {
      setBusy(false);
      if (ref.current) ref.current.value = "";
    }
  };
  return (
    <div className="space-y-3">
      {current ? (
        <div className="flex items-center justify-center rounded-lg border border-line bg-surface-2 p-3">
          <img src={current} alt="Current image" className={previewClass} />
        </div>
      ) : (
        <button type="button" onClick={() => ref.current?.click()} {...drop.dropHandlers}
          className={cn("flex w-full flex-col items-center gap-1 rounded-lg border-2 border-dashed p-6 text-center transition-colors",
            drop.dragActive ? "border-brand bg-brand/[0.07]" : "border-line bg-surface-2 hover:border-brand/40")}>
          <ImageIcon className="size-6 text-fg-subtle" aria-hidden />
          <span className="text-sm font-medium text-fg-muted">{drop.dragActive ? "Drop the image here" : emptyTitle}</span>
          <span className="text-xs text-fg-subtle">{emptyHint}</span>
        </button>
      )}
      <p className="text-xs text-fg-muted">{hint}</p>
      {error && (
        <p role="alert" className="flex items-center gap-2 text-sm text-tone-red"><AlertCircle className="size-4" aria-hidden /> {error}</p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button onClick={() => ref.current?.click()} loading={busy}>
          <Upload aria-hidden /> {current ? uploadLabel[1] : uploadLabel[0]}
        </Button>
        {current && (
          <Button variant="danger" onClick={() => run(onRemove)} disabled={busy}><Trash2 aria-hidden /> {removeLabel}</Button>
        )}
      </div>
      <input ref={ref} type="file" accept={accept} className="hidden"
        onChange={(e) => { const f = e.target.files?.[0]; if (f) void run(() => onUpload(f)); }} />
    </div>
  );
}

function Loading() {
  return <Card><CardBody className="space-y-3"><Skeleton className="h-4 w-40" /><Skeleton className="h-9 w-full" /></CardBody></Card>;
}

export function LogoBlock() {
  const b = useBrandingCtx();
  if (!b.loaded) return <Loading />;
  const isText = b.text.logo_type === "text";
  return (
    <Card>
      <CardBody className="space-y-4">
        <div role="radiogroup" aria-label="Logo type" className="grid grid-cols-2 gap-2">
          {(["text", "image"] as const).map((t) => (
            <button key={t} type="button" role="radio" aria-checked={b.text.logo_type === t} onClick={() => b.set("logo_type", t)}
              className={cn("min-h-11 rounded-lg border px-3 py-2 text-sm font-medium transition-colors md:min-h-9",
                b.text.logo_type === t ? "border-brand bg-brand/[0.07] text-brand-ink" : "border-line-strong text-fg-muted hover:bg-surface-2")}>
              {t === "text" ? "Text logo" : "Image logo"}
            </button>
          ))}
        </div>
        <FieldGrid cols={2}>
          {isText && (
            <Field label="Logo text"><Input value={b.text.logo_text} onChange={(e) => b.set("logo_text", e.target.value)} placeholder="Acme" /></Field>
          )}
          <Field label="Subtitle or app name" hint={isText ? "Under the logo text on the login page and sidebar." : "Next to the image in the header and sidebar."}>
            <Input value={b.text.logo_subtitle} onChange={(e) => b.set("logo_subtitle", e.target.value)} placeholder="Granary" />
          </Field>
        </FieldGrid>
        {isText ? (
          <div className="rounded-lg border border-line bg-surface-2 p-4">
            <p className="mb-2 text-xs text-fg-muted">Preview</p>
            <div className="text-2xl font-semibold tracking-widest text-brand-ink">{b.text.logo_text || "Granary"}</div>
            <div className="mt-0.5 text-xs text-fg-muted">{b.text.logo_subtitle || "Granary"}</div>
          </div>
        ) : (
          <ImageUpload current={b.logo || null} accept="image/png,image/jpeg,image/jpg,image/webp"
            hint="PNG, JPEG or WebP, up to 2 MB." emptyTitle="No logo uploaded" emptyHint="Click or drop an image." previewClass="h-16 w-auto object-contain"
            uploadLabel={["Upload logo", "Replace logo"]} removeLabel="Remove logo"
            onUpload={async (f) => {
              await systemApi.uploadLogo(f);
              invalidateBrandingCache();
              const res = await systemApi.getBranding();
              b.setLogo(res.data.logo_image_data ?? "");
            }}
            onRemove={async () => {
              await systemApi.deleteLogo();
              invalidateBrandingCache();
              b.set("logo_type", "text");
              b.setLogo("");
            }} />
        )}
      </CardBody>
      <OwnSave label="Save branding" />
    </Card>
  );
}

export function FaviconBlock() {
  const b = useBrandingCtx();
  if (!b.loaded) return <Loading />;
  return (
    <Card>
      <CardBody>
        <ImageUpload current={b.favicon || null} accept="image/png,image/jpeg,image/x-icon,image/vnd.microsoft.icon"
          hint="PNG, JPEG or ICO, up to 512 KB. 32 x 32 or 64 x 64 px works best. Without one the browser uses /favicon.ico."
          emptyTitle="No favicon uploaded" emptyHint="Click or drop an image." previewClass="size-12 object-contain"
          uploadLabel={["Upload favicon", "Replace favicon"]} removeLabel="Remove"
          onUpload={async (f) => {
            await systemApi.uploadFavicon(f);
            invalidateBrandingCache();
            const res = await systemApi.getBranding();
            b.setFavicon(res.data.favicon_data ?? "");
          }}
          onRemove={async () => {
            await systemApi.deleteFavicon();
            invalidateBrandingCache();
            b.setFavicon("");
          }} />
      </CardBody>
    </Card>
  );
}

export function LoginTextBlock() {
  const b = useBrandingCtx();
  if (!b.loaded) return <Loading />;
  const f = (k: TextField) => ({ value: b.text[k], onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => b.set(k, e.target.value) });
  return (
    <Card>
      <CardBody>
        <FieldGrid cols={2}>
          <Field label="Badge text" hint="Small label above the headline."><Input {...f("login_badge")} placeholder="Acme ERP" /></Field>
          <Field label="Form subtitle" hint='Below "Welcome back" on the sign-in form.'><Input {...f("login_subtitle")} placeholder="Sign in to continue" /></Field>
          <Field label="Headline, line 1"><Input {...f("login_headline")} placeholder="Your whole business," /></Field>
          <Field label="Headline, line 2" hint="Shown in the red accent."><Input {...f("login_headline_accent")} placeholder="one platform." /></Field>
          <Field label="Tagline" className="md:col-span-2"><Input {...f("login_tagline")} /></Field>
          <Field label="Feature bullets" hint="One per line; each gets a check mark." className="md:col-span-2">
            <Textarea {...f("login_features")} rows={5} />
          </Field>
          <Field label="Left panel footer"><Input {...f("login_footer")} /></Field>
          <Field label="Copyright line" hint="Under the sign-in form."><Input {...f("login_copyright")} /></Field>
        </FieldGrid>
      </CardBody>
      <OwnSave label="Save login text" />
    </Card>
  );
}

export function LetterheadBlock() {
  const b = useBrandingCtx();
  if (!b.loaded) return <Loading />;
  return (
    <Card>
      <CardBody>
        <ImageUpload current={b.letterhead} accept="image/png,image/jpeg,image/jpg,image/svg+xml,image/webp"
          hint="At the top of every printed quotation, invoice and knowledge base article. PNG, JPG, SVG or WebP, up to 5 MB."
          emptyTitle="No letterhead uploaded" emptyHint="Printed documents have no header image." previewClass="max-h-52 w-full object-contain"
          uploadLabel={["Upload image", "Replace image"]} removeLabel="Remove letterhead"
          onUpload={async (f) => {
            await systemApi.uploadLetterhead(f);
            invalidateLetterheadCache();
            const res = await systemApi.getLetterhead();
            b.setLetterhead(res.data.data_url ?? null);
          }}
          onRemove={async () => {
            await systemApi.deleteLetterhead();
            invalidateLetterheadCache();
            b.setLetterhead(null);
          }} />
      </CardBody>
    </Card>
  );
}

