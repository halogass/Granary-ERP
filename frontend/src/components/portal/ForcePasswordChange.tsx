"use client";

import { useState } from "react";
import { signOut } from "next-auth/react";
import { usersApi } from "@/lib/api";
import { Eye, EyeOff, Lock, ShieldCheck } from "lucide-react";
import { Button, Field, Input } from "@/components/ui";

export default function ForcePasswordChange() {
  const [current, setCurrent] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [showPw, setShowPw] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  async function submit() {
    setError("");
    if (!current) { setError("Enter the temporary password you signed in with."); return; }
    if (password.length < 8) { setError("Password must be at least 8 characters."); return; }
    if (password !== confirm) { setError("Passwords do not match."); return; }
    setSaving(true);
    try {
      await usersApi.changeMyPassword(current, password);
      // Force re-login so the session reflects must_change_password = false
      await signOut({ callbackUrl: "/login" });
    } catch (err: unknown) {
      setError((err as { response?: { data?: { detail?: string } } })?.response?.data?.detail
        ?? "Failed to update password. Please try again.");
      setSaving(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
      <div role="dialog" aria-modal="true" aria-labelledby="fpc-title"
        className="w-full max-w-md rounded-lg border border-line bg-surface-raised text-fg shadow-pop">
        <div className="flex items-center gap-3 border-b border-line px-5 py-4">
          <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-brand/10 text-brand-ink">
            <Lock className="size-5" aria-hidden />
          </span>
          <div>
            <h2 id="fpc-title" className="text-base font-semibold text-fg">Set your password</h2>
            <p className="text-xs text-fg-muted">Required before you can continue</p>
          </div>
        </div>

        <form className="space-y-4 px-5 py-4" onSubmit={(e) => { e.preventDefault(); submit(); }}>
          <p className="text-sm text-fg-muted">
            Your account was created with a temporary password. Set a new password to protect your account.
          </p>
          <Field label="Temporary password">
            <Input type={showPw ? "text" : "password"} value={current} onChange={(e) => setCurrent(e.target.value)}
              placeholder="The password you signed in with" autoComplete="current-password" />
          </Field>
          <Field label="New password" hint="At least 8 characters.">
            <div className="relative">
              <Input type={showPw ? "text" : "password"} value={password} onChange={(e) => setPassword(e.target.value)}
                autoComplete="new-password" className="pr-11" />
              <button type="button" onClick={() => setShowPw((v) => !v)} aria-label={showPw ? "Hide passwords" : "Show passwords"}
                className="absolute right-1 top-1/2 flex size-9 -translate-y-1/2 items-center justify-center rounded-lg text-fg-subtle hover:text-fg">
                {showPw ? <EyeOff className="size-4" aria-hidden /> : <Eye className="size-4" aria-hidden />}
              </button>
            </div>
          </Field>
          <Field label="Confirm new password">
            <Input type={showPw ? "text" : "password"} value={confirm} onChange={(e) => setConfirm(e.target.value)}
              autoComplete="new-password" placeholder="Repeat your new password" />
          </Field>

          {error && <p role="alert" className="text-sm font-medium text-tone-red">{error}</p>}

          <Button type="submit" variant="primary" size="lg" className="w-full" loading={saving}
            disabled={!current || !password || !confirm}>
            {!saving && <ShieldCheck aria-hidden />} {saving ? "Saving..." : "Set password and continue"}
          </Button>
        </form>
      </div>
    </div>
  );
}
