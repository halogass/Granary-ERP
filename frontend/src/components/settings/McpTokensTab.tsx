"use client";

import { useCallback, useEffect, useState } from "react";
import { Plus, Trash2, Copy, Check, AlertTriangle, Terminal } from "lucide-react";
import {
  Button, Card, CardBody, CardHeader, Checkbox, DataTable, Field, IconButton, Input, Select, type ColumnDef,
} from "@/components/ui";
import { cn } from "@/lib/utils";
import { authApi, mcpApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { usePermissions } from "@/hooks/usePermissions";
import { FULL_HINT, LEVELS } from "@/components/mcp/levels";

/** A personal token or an agent logged in by browser (OAuth): either way it acts as one person, at one level. */
interface Token {
  id: string; name: string; token_prefix: string | null; kind?: "pat" | "oauth"; user?: string; scopes?: string[];
  last_used_at: string | null; expires_at: string | null;
  revoked: boolean; created_at: string | null;
}

export function levelText(scopes: string[] | undefined): string {
  if (scopes?.includes("full")) return "Full power (Owner)";
  const lvl = (["act", "propose", "read"] as const).find((l) => scopes?.includes(l)) ?? "act";
  // "owner" without "full": a connection made when Owner powers were a separate tick (still works as it did).
  return LEVELS[lvl].label + (scopes?.includes("owner") ? " + Owner powers (old)" : "");
}

/** Levels that can act (act, Owner powers, full power) need the password again. */
export function scopesNeedPassword(scopes: string[]): boolean {
  return scopes.some((s) => s === "act" || s === "owner" || s === "full");
}

export default function McpTokensTab() {
  const { isAdmin } = usePermissions();
  const [showAll, setShowAll] = useState(false);
  const [levels, setLevels] = useState<string[]>(["read", "propose", "act"]);
  const [level, setLevel] = useState("propose");
  const [full, setFull] = useState(false);
  const [tokens, setTokens] = useState<Token[]>([]);
  const [loading, setLoading] = useState(true);
  const [name, setName] = useState("");
  const [days, setDays] = useState("");
  // Asked again for levels that can act; kept only in this field and cleared after each try.
  const [password, setPassword] = useState("");
  // 2 Nov 2026: with two-factor sign-in on, a code from the app too (single use, never kept)
  const [totpOn, setTotpOn] = useState(false);
  const [totpCode, setTotpCode] = useState("");
  const [creating, setCreating] = useState(false);
  const [fresh, setFresh] = useState<{ token: string; name: string } | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setTokens((await mcpApi.connections(showAll)).data.items ?? []);
    } catch {
      setTokens([]);
    } finally { setLoading(false); }
  }, [showAll]);

  useEffect(() => { load(); }, [load]);
  useEffect(() => { mcpApi.levels().then((r) => setLevels(r.data.levels ?? [])).catch(() => {}); }, []);
  useEffect(() => { authApi.totpStatus().then((r) => setTotpOn(!!r.data.enabled)).catch(() => {}); }, []);

  const scopes = full ? ["full"] : [level];
  const needsPassword = scopesNeedPassword(scopes);

  async function create() {
    if (needsPassword && !password) {
      setError("Enter your password to create a token that can act (act or full power).");
      return;
    }
    if (needsPassword && totpOn && !totpCode.trim()) {
      setError("Enter the code from your authenticator app to create a token that can act.");
      return;
    }
    setCreating(true); setError("");
    try {
      const r = await mcpApi.createToken({
        name: name.trim() || "MCP token",
        expires_in_days: days ? Number(days) : null,
        scopes,
        ...(needsPassword ? { password } : {}),
        ...(needsPassword && totpOn ? { totp_code: totpCode.trim() } : {}),
      });
      setFresh({ token: r.data.token, name: r.data.name });
      setName("");
      await load();
    } catch (e) {
      setError(extractApiError(e, "Could not create the token."));
    } finally {
      setPassword("");
      setTotpCode("");
      setCreating(false);
    }
  }

  async function revoke(id: string) {
    try {
      await mcpApi.revokeConnection(id);
      await load();
    } catch (e) {
      setError(extractApiError(e, "Could not revoke the token."));
    }
  }

  async function copy() {
    if (!fresh) return;
    try {
      await navigator.clipboard.writeText(fresh.token);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch { /* clipboard blocked: the value is on screen to copy by hand */ }
  }

  const origin = typeof window !== "undefined" ? window.location.origin : "https://your-erp";

  const columns: ColumnDef<Token>[] = [
    { id: "connection", header: "Connection", cell: ({ row }) => {
      const t = row.original;
      return (
        <div className="min-w-0 py-0.5">
          <p className={cn("truncate font-medium", t.revoked ? "text-fg-subtle line-through" : "text-fg")}>{t.name}</p>
          <p className="text-xs text-fg-muted">
            {showAll && t.user ? `${t.user} · ` : ""}{t.kind === "oauth" ? "logged in by browser" : "personal token"} · {levelText(t.scopes)}
          </p>
          <p className="font-mono text-[11px] text-fg-subtle">
            {t.token_prefix ? `${t.token_prefix}... · ` : ""}
            {t.last_used_at ? `last used ${new Date(t.last_used_at).toLocaleDateString()}` : "never used"}
            {t.expires_at && ` · expires ${new Date(t.expires_at).toLocaleDateString()}`}
          </p>
        </div>
      );
    } },
    { id: "revoke", header: "", meta: { width: "48px" }, cell: ({ row }) =>
      row.original.revoked ? null : <IconButton label="Revoke" icon={Trash2} onClick={() => revoke(row.original.id)} /> },
  ];

  return (
    <div className="space-y-4">
      <Card className="overflow-hidden">
        <CardBody className="space-y-4">
          <p className="text-[13px] text-fg-muted">
            Let an AI agent (an MCP client) work in the ERP. A connection acts as <strong className="font-semibold text-fg">you</strong>,
            with your permissions, at the level you choose. The easiest way: give the agent the address below; it opens
            a browser where you log in and pick the level. A personal token is for agents that can&apos;t do that.
          </p>

          <div className="flex items-start gap-2 rounded-lg bg-tone-orange-soft px-3 py-2.5 text-xs text-tone-orange">
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
            <span>
              {Object.entries(LEVELS).map(([k, v]) => <span key={k} className="block"><strong className="font-semibold">{v.label}:</strong> {v.hint}</span>)}
              {levels.includes("full") && <span className="block"><strong className="font-semibold">Full power (Owner):</strong> {FULL_HINT}</span>}
              Everything an agent does is recorded as coming from MCP, with its name.
              <span className="mt-1 block">
                Tokens that can act (act or full power) need your password and expire after a limited number of days (the setting mcp.pat.max_days_power).
                Changing your password disconnects all your tokens and AI client connections.
              </span>
            </span>
          </div>

          <div className="flex flex-wrap items-end gap-3">
            <Field label="Token name" className="min-w-[200px] flex-1">
              <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. My AI client, laptop" />
            </Field>
            <Field label="Level" className="w-40">
              <Select value={full ? "act" : level} onChange={(e) => setLevel(e.target.value)} disabled={full} aria-label="Access level"
                options={(["read", "propose", "act"] as const).filter((l) => levels.includes(l)).map((l) => ({ value: l, label: LEVELS[l].label }))} />
            </Field>
            {levels.includes("full") && (
              <span className="pb-1.5" title="Work like Nary: decide its cards and topics, send as Nary, book meetings and draft invoices after your yes in the chat">
                <Checkbox label="Full power (Owner)" checked={full} onChange={(e) => setFull(e.target.checked)} />
              </span>
            )}
            <Field label="Expires (days)" className="w-32">
              <Input type="number" numeric min="1" value={days} onChange={(e) => setDays(e.target.value)} placeholder="default" />
            </Field>
            {needsPassword && (
              <Field label="Your password" className="w-48">
                <Input type="password" autoComplete="current-password" value={password}
                  onChange={(e) => setPassword(e.target.value)} aria-label="Your password" />
              </Field>
            )}
            {needsPassword && totpOn && (
              <Field label="Authenticator code" className="w-36">
                <Input value={totpCode} inputMode="numeric" autoComplete="one-time-code" maxLength={7}
                  onChange={(e) => setTotpCode(e.target.value)} aria-label="Authenticator code" placeholder="123456" />
              </Field>
            )}
            <Button variant="primary" onClick={create} loading={creating}>
              {!creating && <Plus aria-hidden />} Create
            </Button>
          </div>

          {fresh && (
            <div className="rounded-lg border border-line bg-surface-2 p-3">
              <p className="mb-1.5 text-xs font-semibold text-fg">Copy this now. It is never shown again.</p>
              <div className="flex items-center gap-2">
                <code className="flex-1 truncate rounded-lg border border-line bg-surface px-3 py-2 font-mono text-xs text-fg">
                  {fresh.token}
                </code>
                <Button size="sm" onClick={copy}>
                  {copied ? <Check aria-hidden /> : <Copy aria-hidden />}
                  {copied ? "Copied" : "Copy"}
                </Button>
              </div>
            </div>
          )}

          {error && (
            <p role="alert" className="rounded-lg border border-tone-red/30 bg-tone-red-soft px-4 py-2.5 text-sm text-tone-red">{error}</p>
          )}
        </CardBody>

        {isAdmin && (
          <div className="flex gap-1.5 border-t border-line px-4 pt-3 pb-1">
            {([[false, "My connections"], [true, "All connections"]] as const).map(([all, label]) => (
              <Button key={String(all)} size="sm" variant={showAll === all ? "secondary" : "ghost"} aria-pressed={showAll === all}
                onClick={() => setShowAll(all)}>
                {label}
              </Button>
            ))}
          </div>
        )}
        <div className="border-t border-line">
          <DataTable label="MCP connections" columns={columns} data={tokens} getRowId={(t) => t.id} loading={loading} maxHeight="none"
            empty={{ title: "No connections yet", compact: true }} />
        </div>
      </Card>

      <Card>
        <CardHeader title={<span className="inline-flex items-center gap-2"><Terminal className="size-3.5 text-fg-muted" aria-hidden /> Connecting an agent</span>} />
        <CardBody className="space-y-2">
          <p className="text-[13px] text-fg-muted">
            Agents that support MCP login (OAuth) need only the address: <code className="font-mono">{origin}/api/v1/mcp</code>. They open a
            browser where you log in and pick the level. For the others, send a personal token as a bearer header:
          </p>
          <pre className="overflow-x-auto rounded-lg border border-line bg-surface-2 p-4 font-mono text-[11px] leading-relaxed text-fg">{`{
  "mcpServers": {
    "granary": {
      "url": "${origin}/api/v1/mcp",
      "headers": { "Authorization": "Bearer gpat_..." }
    }
  }
}`}</pre>
        </CardBody>
      </Card>
    </div>
  );
}
