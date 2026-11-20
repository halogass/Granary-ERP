"use client";

import { useState } from "react";
import { signIn } from "next-auth/react";
import { ArrowLeft, KeyRound, ShieldCheck } from "lucide-react";
import { Button, Field, Input } from "@/components/ui";
import { FormError } from "@/components/portal/AuthLayout";
import { TOTP_EXPIRED, secondStepMessage } from "@/lib/totpLogin";

/**
 * The login's second step for an account with two-factor sign-in on (lib/totpLogin.ts): the 6-digit code
 * from the authenticator app, or one of the recovery codes. Used by the ERP, partner and vendor logins (and
 * so by an agent's MCP browser login, which goes through the ERP login).
 */
export function TotpStep({ challenge, onSignedIn, onRestart }: {
  challenge: string;
  onSignedIn: () => void | Promise<void>;
  /** Back to the password (with a message when the challenge ran out). */
  onRestart: (message: string) => void;
}) {
  const [recovery, setRecovery] = useState(false);
  const [value, setValue] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const v = value.trim();
    if (!v) return;
    setLoading(true);
    setError("");
    const result = await signIn("credentials", {
      challenge,
      ...(recovery ? { recovery_code: v } : { code: v.replace(/\s+/g, "") }),
      redirect: false,
    });
    if (result?.error) {
      setLoading(false);
      if (result.error === TOTP_EXPIRED) { onRestart(secondStepMessage(result.error, recovery)); return; }
      setError(secondStepMessage(result.error, recovery));
      setValue("");
      return;
    }
    await onSignedIn();
    setLoading(false);
  }

  return (
    <form onSubmit={submit} className="space-y-4" aria-label="Two-factor sign-in">
      <div className="flex items-start gap-2.5 rounded-lg border border-line bg-surface-2 px-3.5 py-3 text-sm text-fg-muted">
        <ShieldCheck className="mt-0.5 size-4 shrink-0 text-brand-ink" aria-hidden />
        <span>
          {recovery
            ? "Enter one of the recovery codes you saved when you turned on two-factor sign-in. Each code works once."
            : "Enter the 6-digit code from your authenticator app."}
        </span>
      </div>
      <Field label={recovery ? "Recovery code" : "Authentication code"}>
        <Input key={recovery ? "r" : "c"} value={value} onChange={(e) => setValue(e.target.value)} required autoFocus
          inputMode={recovery ? "text" : "numeric"} autoComplete="one-time-code" maxLength={recovery ? 24 : 7}
          placeholder={recovery ? "xxxxx-xxxxx" : "123456"} className="font-mono tracking-widest md:h-10" />
      </Field>

      {error && <FormError>{error}</FormError>}

      <Button type="submit" variant="primary" size="lg" className="w-full" loading={loading}>
        {loading ? "Checking..." : "Verify and sign in"}
      </Button>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Button type="button" size="sm" variant="ghost" onClick={() => { setRecovery(!recovery); setValue(""); setError(""); }}>
          <KeyRound aria-hidden /> {recovery ? "Use the authenticator app" : "Use a recovery code"}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={() => onRestart("")}>
          <ArrowLeft aria-hidden /> Back
        </Button>
      </div>
    </form>
  );
}
