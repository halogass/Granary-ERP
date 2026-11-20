"use client";

import { useEffect, useState } from "react";
import { ShieldOff } from "lucide-react";
import { usersApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Pill } from "@/components/ui";

/**
 * Settings > Users, a user's details: their two-factor sign-in and, for the Owner or a system admin, "Reset
 * two-factor" (a lost phone with no recovery codes). The reset is audit-logged; the person sets it up again.
 */
export default function UserTwoFactorAdmin({ userId, name }: { userId: string; name: string }) {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");

  useEffect(() => {
    let alive = true;
    usersApi.totpStatus(userId)
      .then((r) => { if (alive) setEnabled(!!r.data.enabled); })
      .catch(() => { if (alive) setEnabled(null); });
    return () => { alive = false; };
  }, [userId]);

  async function reset() {
    if (!confirm(`Reset two-factor sign-in for ${name}? They sign in with the password alone until they set it up again.`)) return;
    setBusy(true); setMsg("");
    try {
      await usersApi.resetTotp(userId);
      setEnabled(false);
      setMsg("Two-factor sign-in reset. Ask them to set it up again in Profile > Security.");
    } catch (e) {
      setMsg(extractApiError(e, "Could not reset two-factor sign-in."));
    } finally { setBusy(false); }
  }

  if (enabled === null && !msg) return null;
  return (
    <div className="flex flex-wrap items-center gap-2 text-sm">
      <span className="text-fg-muted">Two-factor sign-in</span>
      <Pill tone={enabled ? "blue" : "neutral"}>{enabled ? "On" : "Off"}</Pill>
      {enabled && (
        <Button size="sm" variant="danger" onClick={reset} loading={busy}><ShieldOff aria-hidden /> Reset two-factor</Button>
      )}
      {msg && <span role="status" className="text-xs text-fg-muted">{msg}</span>}
    </div>
  );
}
