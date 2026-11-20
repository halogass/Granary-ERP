"use client";

import { useCallback, useEffect, useState } from "react";
import { Check, Copy, Download, KeyRound, RefreshCw, ShieldCheck, ShieldOff } from "lucide-react";
import { authApi, type TotpSetup, type TotpStatus } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Card, CardBody, CardFooter, CardHeader, Field, Input, Pill, Skeleton } from "@/components/ui";

/** "ABCD EFGH ..." for typing the secret by hand. */
export function groupSecret(secret: string): string {
  return secret.replace(/(.{4})/g, "$1 ").trim();
}

function Note({ tone, children }: { tone: "red" | "blue" | "orange"; children: React.ReactNode }) {
  const cls = { red: "bg-tone-red-soft text-tone-red", blue: "bg-tone-blue-soft text-tone-blue", orange: "bg-tone-orange-soft text-tone-orange" }[tone];
  return <div role={tone === "red" ? "alert" : "status"} className={`rounded-lg px-3 py-2.5 text-sm ${cls}`}>{children}</div>;
}

/** The recovery codes, shown once (after setup or a regenerate), with copy and download. */
function RecoveryCodes({ codes, onDone }: { codes: string[]; onDone: () => void }) {
  const [copied, setCopied] = useState(false);
  const text = codes.join("\n");
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch { /* clipboard blocked: the codes are on screen */ }
  }
  function download() {
    const blob = new Blob([`Granary recovery codes (each works once)\n\n${text}\n`], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "granary-recovery-codes.txt";
    a.click();
    URL.revokeObjectURL(url);
  }
  return (
    <div className="space-y-3">
      <Note tone="orange">
        Save these recovery codes somewhere safe now: they are not shown again. Each one signs you in once if you lose
        your phone.
      </Note>
      <ul aria-label="Recovery codes" className="grid grid-cols-2 gap-1.5 rounded-lg border border-line bg-surface-2 p-3 font-mono text-sm text-fg">
        {codes.map((c) => <li key={c}>{c}</li>)}
      </ul>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" onClick={copy}>{copied ? <Check aria-hidden /> : <Copy aria-hidden />} {copied ? "Copied" : "Copy"}</Button>
        <Button size="sm" onClick={download}><Download aria-hidden /> Download</Button>
        <Button size="sm" variant="primary" onClick={onDone}>I saved them</Button>
      </div>
    </div>
  );
}

/**
 * Profile > Security: two-factor sign-in (TOTP). Turn it on (QR or the key by hand, confirmed with a code),
 * recovery codes shown once, new recovery codes (needs a code), turn it off (a code or a recovery code).
 */
export default function TwoFactorSection() {
  const [status, setStatus] = useState<TotpStatus | null>(null);
  const [setup, setSetup] = useState<TotpSetup | null>(null);
  const [codes, setCodes] = useState<string[] | null>(null);
  const [code, setCode] = useState("");
  // Stage 8: setting it up asks the current password again
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [done, setDone] = useState("");

  const load = useCallback(async () => {
    try { setStatus((await authApi.totpStatus()).data); }
    catch (e) { setError(extractApiError(e, "Could not load two-factor sign-in.")); }
  }, []);
  useEffect(() => { load(); }, [load]);

  async function run(what: string, fn: () => Promise<void>) {
    setBusy(what); setError(""); setDone("");
    try { await fn(); }
    catch (e) { setError(extractApiError(e, "That did not work.")); }
    finally { setBusy(null); setCode(""); }
  }

  const start = () => run("setup", async () => {
    setSetup((await authApi.totpSetup(password)).data);
    setPassword("");
  });
  const confirm = () => run("confirm", async () => {
    const r = await authApi.totpConfirm(code.trim());
    setSetup(null);
    setCodes(r.data.recovery_codes);
    await load();
  });
  const regenerate = () => run("regenerate", async () => {
    const r = await authApi.totpRegenerate(code.trim());
    setCodes(r.data.recovery_codes);
    await load();
  });
  const disable = () => run("disable", async () => {
    await authApi.totpDisable(code.trim());
    setDone("Two-factor sign-in is off. Your password alone signs you in now.");
    await load();
  });

  if (!status && !error) {
    return <Card><CardBody><Skeleton className="h-24 w-full" /></CardBody></Card>;
  }
  const on = !!status?.enabled;

  return (
    <Card>
      <CardHeader title="Two-factor sign-in" description="A code from an authenticator app on your phone, asked after your password at every login."
        actions={<Pill tone={on ? "blue" : "neutral"}>{on ? "On" : "Off"}</Pill>} />
      <CardBody className="space-y-4">
        {status?.required && !on && (
          <Note tone="orange">Your role approves sensitive actions: turn on two-factor sign-in first, each approval asks a code.</Note>
        )}

        {codes && <RecoveryCodes codes={codes} onDone={() => setCodes(null)} />}

        {!codes && !on && !setup && (<>
          <p className="text-sm text-fg-muted">
            Use Google Authenticator, Microsoft Authenticator, 1Password or any app that shows 6-digit codes.
          </p>
          <Field label="Your password" hint="Asked again before a new key is made.">
            <Input type="password" value={password} onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password" className="w-60" />
          </Field>
        </>)}

        {!codes && setup && (
          <div className="space-y-4">
            <p className="text-sm text-fg">1. Scan this QR code with your authenticator app.</p>
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src={setup.qr_data_uri} alt="QR code for your authenticator app" className="size-44 rounded-lg border border-line bg-white p-1" />
            <p className="text-sm text-fg-muted">
              Can&apos;t scan? Enter this key in the app ({setup.issuer}, {setup.account}):{" "}
              <code aria-label="Setup key" className="break-all rounded bg-surface-2 px-1.5 py-0.5 font-mono text-xs text-fg">{groupSecret(setup.secret)}</code>
            </p>
            <Field label="2. Code from the app">
              <Input value={code} onChange={(e) => setCode(e.target.value)} inputMode="numeric" autoComplete="one-time-code"
                maxLength={7} placeholder="123456" className="w-40 font-mono tracking-widest" />
            </Field>
          </div>
        )}

        {!codes && on && (
          <div className="space-y-3">
            <p className="text-sm text-fg-muted">
              {status?.recovery_codes_left ?? 0} recovery codes left.
              {status?.confirmed_at && ` On since ${new Date(status.confirmed_at).toLocaleDateString()}.`}
            </p>
            <Field label="Code from your app" hint="To turn it off you can also use a recovery code.">
              <Input value={code} onChange={(e) => setCode(e.target.value)} autoComplete="one-time-code"
                maxLength={24} placeholder="123456" className="w-48 font-mono tracking-widest" />
            </Field>
          </div>
        )}

        {error && <Note tone="red">{error}</Note>}
        {done && <Note tone="blue">{done}</Note>}
      </CardBody>
      {!codes && (
        <CardFooter className="justify-start">
          {!on && !setup && (
            <Button variant="primary" onClick={start} loading={busy === "setup"} disabled={!password}>
              <ShieldCheck aria-hidden /> Turn on
            </Button>
          )}
          {!on && setup && (<>
            <Button variant="primary" onClick={confirm} loading={busy === "confirm"} disabled={!code.trim()}>
              <Check aria-hidden /> Confirm and turn on
            </Button>
            <Button onClick={() => { setSetup(null); setCode(""); setError(""); }}>Cancel</Button>
          </>)}
          {on && (<>
            <Button onClick={regenerate} loading={busy === "regenerate"} disabled={!code.trim()}>
              <RefreshCw aria-hidden /> New recovery codes
            </Button>
            <Button variant="danger" onClick={disable} loading={busy === "disable"} disabled={!code.trim()}>
              <ShieldOff aria-hidden /> Turn off
            </Button>
          </>)}
          {!on && !setup && <span className="flex items-center gap-1 text-xs text-fg-subtle"><KeyRound className="size-3" aria-hidden /> You also get one-time recovery codes.</span>}
        </CardFooter>
      )}
    </Card>
  );
}
