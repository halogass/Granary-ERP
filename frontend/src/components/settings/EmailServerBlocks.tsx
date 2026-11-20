"use client";

import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import { CheckCircle, Eye, EyeOff, Mail, Send, Wifi, XCircle } from "lucide-react";
import { systemApi } from "@/lib/api";
import { Button, Card, CardBody, CardFooter, Field, FieldGrid, IconButton, Input, Skeleton, Switch, toast } from "@/components/ui";
import { extractApiError } from "@/lib/apiError";
import { useDraftSource } from "./SettingsDraft";

interface ServerField { key: string; label: string; type: "text" | "email" | "number" | "password" | "toggle"; placeholder: string }

const SYSTEM_SMTP: ServerField[] = [
  { key: "email_smtp_from_name", label: "From name", type: "text", placeholder: "Granary" },
  { key: "email_smtp_from_email", label: "From email", type: "email", placeholder: "granary.erp@example.com" },
  { key: "email_smtp_host", label: "SMTP host", type: "text", placeholder: "smtp.gmail.com" },
  { key: "email_smtp_port", label: "SMTP port", type: "number", placeholder: "587" },
  { key: "email_smtp_user", label: "Username", type: "text", placeholder: "granary.erp@example.com" },
  { key: "email_smtp_password", label: "Password or app password", type: "password", placeholder: "" },
  { key: "email_smtp_tls", label: "Use STARTTLS", type: "toggle", placeholder: "" },
];
/** An inbox's outgoing server keys: `${prefix}_smtp_*` (hello@ is prefix "crm"). */
export function smtpFields(prefix: string, address: string, name = "Your Company"): ServerField[] {
  return [
    { key: `${prefix}_smtp_from_name`, label: "From name", type: "text", placeholder: name },
    { key: `${prefix}_smtp_from_email`, label: "From email", type: "email", placeholder: address },
    { key: `${prefix}_smtp_host`, label: "SMTP host", type: "text", placeholder: "smtp.gmail.com" },
    { key: `${prefix}_smtp_port`, label: "SMTP port", type: "number", placeholder: "587" },
    { key: `${prefix}_smtp_user`, label: "Username", type: "text", placeholder: address },
    { key: `${prefix}_smtp_password`, label: "Password or app password", type: "password", placeholder: "" },
    { key: `${prefix}_smtp_tls`, label: "Use STARTTLS", type: "toggle", placeholder: "" },
  ];
}
/** An inbox's incoming server keys: `${prefix}_imap_*`. */
export function imapFields(prefix: string, address: string): ServerField[] {
  return [
    { key: `${prefix}_imap_host`, label: "IMAP host", type: "text", placeholder: "imap.gmail.com" },
    { key: `${prefix}_imap_port`, label: "IMAP port", type: "number", placeholder: "993" },
    { key: `${prefix}_imap_user`, label: "Username", type: "text", placeholder: address },
    { key: `${prefix}_imap_password`, label: "Password or app password", type: "password", placeholder: "" },
    { key: `${prefix}_imap_ssl`, label: "Use SSL/TLS", type: "toggle", placeholder: "" },
  ];
}

const isPassword = (k: string) => /_(smtp|imap)_password$/.test(k);
const MASK = "••••••••";

interface EmailCtx {
  loaded: boolean;
  config: Record<string, string>;
  form: Record<string, string>;
  set: (key: string, value: string) => void;
  reset: (keys: string[]) => void;
  reload: () => Promise<void>;
  testEmail: string;
  setTestEmail: (v: string) => void;
}

const Ctx = createContext<EmailCtx | null>(null);
function useEmail() {
  const c = useContext(Ctx);
  if (!c) throw new Error("Email server blocks need <EmailConfigProvider>");
  return c;
}

function formFrom(config: Record<string, string>) {
  // Password fields start blank: typing one replaces it, blank keeps it.
  return Object.fromEntries(Object.keys(config).map((k) => [k, isPassword(k) ? "" : (config[k] ?? "")]));
}

/** Loads the mail server configuration once for the Email & WhatsApp area. */
export function EmailConfigProvider({ children }: { children: React.ReactNode }) {
  const [loaded, setLoaded] = useState(false);
  const [config, setConfig] = useState<Record<string, string>>({});
  const [form, setForm] = useState<Record<string, string>>({});
  const [testEmail, setTestEmail] = useState("");

  const reload = useCallback(async () => {
    const res = await systemApi.getEmailConfig();
    const ec = res.data as Record<string, string>;
    setConfig(ec);
    setForm(formFrom(ec));
    setLoaded(true);
  }, []);
  useEffect(() => { reload().catch(() => setLoaded(true)); }, [reload]);

  const value = useMemo<EmailCtx>(() => ({
    loaded, config, form, reload, testEmail, setTestEmail,
    set: (key, v) => setForm((f) => ({ ...f, [key]: v })),
    reset: (keys) => setForm((f) => {
      const base = formFrom(config);
      return { ...f, ...Object.fromEntries(keys.map((k) => [k, base[k] ?? ""])) };
    }),
  }), [loaded, config, form, reload, testEmail]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

function changedKeys(fields: ServerField[], form: Record<string, string>, config: Record<string, string>) {
  return fields.filter((f) => {
    const v = form[f.key];
    if (v === undefined) return false;
    if (isPassword(f.key)) return v !== "";
    if (f.type === "toggle") return (v || "true") !== (config[f.key] || "true");
    return v !== (config[f.key] ?? "");
  }).map((f) => f.key);
}

function TestResult({ result }: { result: { ok: boolean; msg: string } | null }) {
  if (!result) return null;
  return (
    <span role="status" className={`inline-flex items-center gap-1.5 text-sm ${result.ok ? "text-tone-blue" : "text-tone-red"}`}>
      {result.ok ? <CheckCircle className="size-4" aria-hidden /> : <XCircle className="size-4" aria-hidden />}
      {result.msg}
    </span>
  );
}

function ServerFields({ fields }: { fields: ServerField[] }) {
  const { form, config, set } = useEmail();
  const [shown, setShown] = useState<Set<string>>(new Set());
  return (
    <FieldGrid cols={2}>
      {fields.map((f) => {
        if (f.type === "toggle") {
          const on = (form[f.key] ?? config[f.key] ?? "") !== "false";
          return (
            <div key={f.key} className="flex items-end md:col-span-2">
              <Switch label={f.label} checked={on} onCheckedChange={(v) => set(f.key, v ? "true" : "false")} />
            </div>
          );
        }
        if (f.type === "password") {
          const configured = (config[f.key] ?? "") === MASK;
          const visible = shown.has(f.key);
          return (
            <Field key={f.key} label={f.label} hint={configured ? "Configured. Leave blank to keep it." : undefined}>
              <div className="flex gap-1">
                <Input type={visible ? "text" : "password"} value={form[f.key] ?? ""} onChange={(e) => set(f.key, e.target.value)}
                  placeholder={configured ? "Leave blank to keep" : "Enter password"} autoComplete="new-password" />
                <IconButton label={visible ? "Hide password" : "Show password"} icon={visible ? EyeOff : Eye}
                  onClick={() => setShown((s) => { const n = new Set(s); if (n.has(f.key)) n.delete(f.key); else n.add(f.key); return n; })} />
              </div>
            </Field>
          );
        }
        return (
          <Field key={f.key} label={f.label}>
            <Input type={f.type} value={form[f.key] ?? ""} onChange={(e) => set(f.key, e.target.value)} placeholder={f.placeholder}
              numeric={f.type === "number"} />
          </Field>
        );
      })}
    </FieldGrid>
  );
}

/** One server's fields as a draft source: saved key by key with PATCH /system/email-config (as before). */
function useServerDraft(id: string, fields: ServerField[]) {
  const { form, config, reload, reset } = useEmail();
  const keys = changedKeys(fields, form, config);
  const [saving, setSaving] = useState(false);
  const save = async () => {
    setSaving(true);
    try {
      for (const f of fields) {
        const val = form[f.key] ?? "";
        if (isPassword(f.key) && !val) continue;
        await systemApi.updateEmailConfig(f.key, val);
      }
      await reload();
    } finally {
      setSaving(false);
    }
  };
  const { inSaveBar } = useDraftSource(id, { count: keys.length, save, discard: () => reset(fields.map((f) => f.key)) });
  return { inSaveBar, saving, save };
}

function OwnSave({ draft }: { draft: { inSaveBar: boolean; saving: boolean; save: () => Promise<void> } }) {
  if (draft.inSaveBar) return null;
  return (
    <Button variant="primary" loading={draft.saving}
      onClick={() => draft.save().then(() => toast.success("Configuration saved"), (e) => toast.error(extractApiError(e)))}>
      Save configuration
    </Button>
  );
}

function Loading() {
  return <Card><CardBody className="space-y-3"><Skeleton className="h-4 w-40" /><Skeleton className="h-9 w-full" /></CardBody></Card>;
}

export function TestAddressBlock() {
  const { testEmail, setTestEmail } = useEmail();
  return (
    <Card>
      <CardBody>
        <Field label="Test recipient address" hint="The Send test buttons below send to it.">
          <Input type="email" leading={Mail} value={testEmail} onChange={(e) => setTestEmail(e.target.value)} placeholder="your@email.com" className="md:max-w-sm" />
        </Field>
      </CardBody>
    </Card>
  );
}

/** Outgoing server of the system mail ("email") or of one CRM inbox (its prefix, also the test's scope). */
function SmtpBlock({ id, fields, prefix, fallbackName }: { id: string; fields: ServerField[]; prefix: string; fallbackName: string }) {
  const ctx = useEmail();
  const draft = useServerDraft(id, fields);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; msg: string } | null>(null);
  if (!ctx.loaded) return <Loading />;
  const p = `${prefix}_smtp_`;
  const val = (k: string, d = "") => ctx.form[p + k] || ctx.config[p + k] || d;
  const test = async () => {
    if (!ctx.testEmail.trim()) return;
    setTesting(true); setResult(null);
    try {
      await systemApi.testEmailConfig({
        to: ctx.testEmail,
        smtp_host: val("host"),
        smtp_port: parseInt(val("port", "587")),
        smtp_user: val("user"),
        smtp_password: ctx.form[`${p}password`] || "",
        smtp_from_name: val("from_name", fallbackName),
        smtp_from_email: val("from_email"),
        smtp_tls: (ctx.form[`${p}tls`] ?? ctx.config[`${p}tls`] ?? "") !== "false",
        scope: prefix === "email" ? "system" : prefix,
      });
      setResult({ ok: true, msg: `Test email sent to ${ctx.testEmail}` });
    } catch (e) {
      setResult({ ok: false, msg: extractApiError(e, "SMTP connection failed") });
    } finally {
      setTesting(false);
    }
  };
  return (
    <Card>
      <CardBody><ServerFields fields={fields} /></CardBody>
      <CardFooter className="flex-wrap justify-start">
        <OwnSave draft={draft} />
        <Button onClick={test} loading={testing} disabled={!ctx.testEmail.trim()}
          title={ctx.testEmail.trim() ? undefined : "Enter a test recipient first"}>
          <Send aria-hidden /> Send test email
        </Button>
        <TestResult result={result} />
      </CardFooter>
    </Card>
  );
}

export function SystemSmtpBlock() {
  return <SmtpBlock id="email-system-smtp" fields={SYSTEM_SMTP} prefix="email" fallbackName="Granary" />;
}

/** A CRM inbox's outgoing server; hello@ is prefix "crm" (the crm_smtp_* keys, as before). */
export function CrmSmtpBlock({ prefix = "crm", address = "hello@example.com" }: { prefix?: string; address?: string }) {
  const fields = useMemo(() => smtpFields(prefix, address), [prefix, address]);
  return <SmtpBlock id={`email-${prefix}-smtp`} fields={fields} prefix={prefix} fallbackName="Your Company" />;
}

/** A CRM inbox's incoming server; hello@ is prefix "crm" (the crm_imap_* keys, as before). */
export function CrmImapBlock({ prefix = "crm", address = "hello@example.com" }: { prefix?: string; address?: string }) {
  const fields = useMemo(() => imapFields(prefix, address), [prefix, address]);
  const ctx = useEmail();
  const draft = useServerDraft(`email-${prefix}-imap`, fields);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; msg: string } | null>(null);
  if (!ctx.loaded) return <Loading />;
  const p = `${prefix}_imap_`;
  const val = (k: string, d = "") => ctx.form[p + k] || ctx.config[p + k] || d;
  const test = async () => {
    setTesting(true); setResult(null);
    try {
      await systemApi.testImapConfig({
        imap_host: val("host"),
        imap_port: parseInt(val("port", "993")),
        imap_user: val("user"),
        imap_password: ctx.form[`${p}password`] || "",
        imap_ssl: (ctx.form[`${p}ssl`] ?? ctx.config[`${p}ssl`] ?? "") !== "false",
        scope: prefix,
      });
      setResult({ ok: true, msg: "IMAP connection successful" });
    } catch (e) {
      setResult({ ok: false, msg: extractApiError(e, "IMAP connection failed") });
    } finally {
      setTesting(false);
    }
  };
  return (
    <Card>
      <CardBody><ServerFields fields={fields} /></CardBody>
      <CardFooter className="flex-wrap justify-start">
        <OwnSave draft={draft} />
        <Button onClick={test} loading={testing}><Wifi aria-hidden /> Test IMAP connection</Button>
        <TestResult result={result} />
      </CardFooter>
    </Card>
  );
}

/** Reloads the server fields after an inbox is added (its empty keys now exist). */
export function useReloadEmailConfig() {
  return useEmail().reload;
}
