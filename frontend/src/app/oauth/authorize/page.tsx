"use client";

import { Suspense, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "next/navigation";
import { Bot, Loader2, ShieldCheck } from "lucide-react";
import { authApi, mcpApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { FULL_HINT, LEVELS, initialLevel } from "@/components/mcp/levels";
import { Button, Card, Field, Input } from "@/components/ui";
import { PublicPage } from "@/components/portal/PublicPage";
import { FormError } from "@/components/portal/AuthLayout";
import { cn } from "@/lib/utils";

/** An agent (an MCP client) asks to connect to Granary as the logged-in person (MCP browser login).
 *  The person sees which agent, where it will be sent, and picks the access level; Allow returns a one-time code
 *  to the agent (backend/app/routers/mcp_oauth.py). */
function Consent() {
  const q = useSearchParams();
  const clientId = q.get("client_id") ?? "";
  const redirectUri = q.get("redirect_uri") ?? "";
  const requested = useMemo(() => (q.get("scope") ?? "").split(/\s+/).filter(Boolean), [q]);
  const [info, setInfo] = useState<{ client_name: string; redirect_host: string; levels: string[] } | null>(null);
  const [level, setLevel] = useState("propose");
  const [full, setFull] = useState(false);
  const [password, setPassword] = useState("");
  const [totpOn, setTotpOn] = useState(false);
  const [totpCode, setTotpCode] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  // Stage 8: a level that can act (act, full power) needs the password again, and the authenticator code
  const canAct = full || level === "act";

  useEffect(() => {
    if (q.get("response_type") !== "code" || !clientId || !redirectUri || !q.get("code_challenge")) {
      setError("This sign-in link is incomplete. Start the connection again from the agent.");
      return;
    }
    mcpApi.oauthClient(clientId, redirectUri)
      .then((r) => {
        setInfo(r.data);
        // Stage 8: nothing that can act is picked for the person, whatever the agent asked for
        setLevel(initialLevel(requested, r.data.levels));
      })
      .catch((e) => setError(extractApiError(e, "This agent can't be connected.")));
    authApi.totpStatus().then((r) => setTotpOn(!!r.data.enabled)).catch(() => {});
  }, [clientId, redirectUri, q, requested]);

  const back = (params: Record<string, string>) => {
    const u = new URL(redirectUri);
    Object.entries(params).forEach(([k, v]) => u.searchParams.set(k, v));
    const state = q.get("state");
    if (state) u.searchParams.set("state", state);
    window.location.assign(u.toString());
  };

  const allow = async () => {
    if (canAct && !password) { setError("Enter your password to give an agent a level that can act."); return; }
    if (canAct && totpOn && !totpCode.trim()) {
      setError("Enter the code from your authenticator app to give an agent a level that can act.");
      return;
    }
    setBusy(true); setError("");
    try {
      const r = await mcpApi.oauthAuthorize({
        client_id: clientId, redirect_uri: redirectUri, code_challenge: q.get("code_challenge") ?? "",
        code_challenge_method: q.get("code_challenge_method") ?? "S256",
        scopes: full ? ["full"] : [level], state: q.get("state"),
        ...(canAct ? { password } : {}),
        ...(canAct && totpOn ? { totp_code: totpCode.trim() } : {}),
      });
      window.location.assign(r.data.redirect_to);
    } catch (e) {
      setError(extractApiError(e, "Couldn't connect the agent."));
      setBusy(false);
    }
  };

  const option = (on: boolean) => cn("flex cursor-pointer gap-3 rounded-lg border px-3 py-2.5 transition-colors",
    on ? "border-brand bg-brand/[0.07]" : "border-line hover:bg-surface-2");

  return (
    <PublicPage subtitle="Granary" width="sm" center>
      <Card className="p-5 sm:p-6">
        <div className="flex items-center gap-2.5 text-fg">
          <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-brand/10 text-brand-ink">
            <Bot className="size-5" aria-hidden />
          </span>
          <h1 className="text-base font-semibold">Connect an agent to Granary</h1>
        </div>
        {!info && !error && <Loader2 className="mt-6 size-5 animate-spin text-fg-subtle" aria-label="Loading" />}
        {error && <div className="mt-4"><FormError>{error}</FormError></div>}
        {info && (
          <>
            <p className="mt-4 text-sm text-fg">
              <b className="font-semibold">{info.client_name}</b> wants to work in the ERP <b className="font-semibold">as you</b>, with your permissions. It will be sent
              back to <span className="font-mono text-xs">{info.redirect_host}</span>.
            </p>
            <fieldset className="mt-5 space-y-2">
              <legend className="mb-2 text-[13px] font-medium text-fg">Access level</legend>
              {(["read", "propose", "act"] as const).filter((l) => info.levels.includes(l)).map((l) => (
                <label key={l} className={cn(option(!full && level === l), full && "cursor-not-allowed opacity-60")}>
                  <input type="radio" name="level" value={l} checked={!full && level === l} disabled={full} onChange={() => setLevel(l)}
                    className="mt-1 size-4 shrink-0 accent-[rgb(var(--brand))]" />
                  <span>
                    <span className="block text-sm font-medium text-fg">{LEVELS[l].label}</span>
                    <span className="block text-xs text-fg-muted">{LEVELS[l].hint}</span>
                  </span>
                </label>
              ))}
              {info.levels.includes("full") && (
                <label className={option(full)}>
                  <input type="checkbox" checked={full} onChange={(e) => setFull(e.target.checked)}
                    className="mt-1 size-4 shrink-0 accent-[rgb(var(--brand))]" />
                  <span>
                    <span className="block text-sm font-medium text-fg">Full power (Owner)</span>
                    <span className="block text-xs text-fg-muted">{FULL_HINT}</span>
                  </span>
                </label>
              )}
            </fieldset>
            {canAct && (
              <div className="mt-4 space-y-3">
                <p className="text-xs text-fg-muted">
                  A level that can act needs your password{totpOn ? " and the code from your authenticator app" : ""}.
                  The connection ends on its own after a fixed number of days, however often the agent uses it.
                </p>
                <Field label="Your password">
                  <Input type="password" autoComplete="current-password" value={password}
                    onChange={(e) => setPassword(e.target.value)} aria-label="Your password" />
                </Field>
                {totpOn && (
                  <Field label="Authenticator code">
                    <Input value={totpCode} inputMode="numeric" autoComplete="one-time-code" maxLength={7}
                      onChange={(e) => setTotpCode(e.target.value)} aria-label="Authenticator code" />
                  </Field>
                )}
              </div>
            )}
            <p className="mt-4 flex items-center gap-1.5 text-xs text-fg-muted">
              <ShieldCheck className="size-3.5 shrink-0" aria-hidden /> You can revoke it any time in Settings, MCP access.
            </p>
            <div className="mt-5 flex flex-wrap justify-end gap-2 border-t border-line pt-4">
              <Button onClick={() => back({ error: "access_denied" })} disabled={busy}>Deny</Button>
              <Button variant="primary" onClick={allow} loading={busy}>Allow</Button>
            </div>
          </>
        )}
      </Card>
    </PublicPage>
  );
}

export default function OAuthAuthorizePage() {
  return <Suspense fallback={null}><Consent /></Suspense>;
}
