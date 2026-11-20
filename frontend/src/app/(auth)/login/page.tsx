"use client";

import { useState } from "react";
import Link from "next/link";
import { signIn } from "next-auth/react";
import { useRouter } from "next/navigation";
import { Building2, LifeBuoy, Users } from "lucide-react";
import { Button, Field, Input } from "@/components/ui";
import { AuthLayout, FormError, PasswordInput } from "@/components/portal/AuthLayout";
import { useBranding } from "@/hooks/useBranding";
import { TotpStep } from "@/components/portal/TotpStep";
import { afterLoginPath, challengeFromError, firstStepMessage } from "@/lib/totpLogin";

export default function LoginPage() {
  const router = useRouter();
  const branding = useBranding();
  const features = branding.login_features
    .split("\n")
    .map((f) => f.trim())
    .filter(Boolean);

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  // Two-factor sign-in: the challenge after a right password, while the code is asked
  const [challenge, setChallenge] = useState<string | null>(null);

  async function goOn() {
    // Route to the portal by portal_type, or back to the page that sent the user here (an agent's consent
    // page for the MCP browser login), same-site paths only (never "//host" or a full URL).
    const { getSession } = await import("next-auth/react");
    const session = await getSession();
    router.push(afterLoginPath(session?.user?.portal_type, window.location.search));
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setLoading(true);
    setError("");
    const result = await signIn("credentials", {
      email,
      password,
      redirect: false,
    });
    setLoading(false);
    const totp = challengeFromError(result?.error);
    if (totp) {
      setPassword("");
      setChallenge(totp);
    } else if (result?.error) {
      // "CredentialsSignin" is a plain wrong email/password; a server sentence (the login rate limit) shows
      // as-is; anything else (a bare code, "undefined") becomes a plain "did not complete" (lib/totpLogin.ts).
      setError(firstStepMessage(result.error));
    } else {
      await goOn();
    }
  }

  const quick = "flex min-h-11 items-center justify-center gap-1.5 rounded-lg border border-line bg-surface px-3 text-[13px] font-medium text-fg-muted transition-colors hover:bg-surface-2 hover:text-fg md:min-h-10";

  return (
    <AuthLayout eyebrow={branding.login_badge} headline={branding.login_headline} accent={branding.login_headline_accent}
      tagline={branding.login_tagline} features={features} panelFooter={branding.login_footer}>
      <div className="mb-8">
        <h1 className="text-2xl font-semibold tracking-tight text-fg">Granary access</h1>
        <p className="mt-1 text-sm text-fg-muted">{branding.login_subtitle}</p>
      </div>

      {challenge ? (
        <TotpStep challenge={challenge} onSignedIn={goOn}
          onRestart={(msg) => { setChallenge(null); setError(msg); }} />
      ) : (
      <form onSubmit={handleSubmit} className="space-y-4">
        <Field label="Email address">
          <Input type="email" value={email} onChange={(e) => setEmail(e.target.value)} required autoComplete="email"
            placeholder="you@example.com" className="md:h-10" />
        </Field>
        <Field label="Password">
          <PasswordInput value={password} onChange={(e) => setPassword(e.target.value)} required autoComplete="current-password" className="md:h-10" />
        </Field>

        {error && <FormError>{error}</FormError>}

        <Button type="submit" variant="primary" size="lg" className="w-full" loading={loading}>
          {loading ? "Signing in..." : "Sign in"}
        </Button>
      </form>
      )}

      <div className="mt-8">
        <div className="mb-3 flex items-center gap-3">
          <div className="h-px flex-1 bg-line" />
          <span className="text-xs text-fg-subtle">Quick access</span>
          <div className="h-px flex-1 bg-line" />
        </div>
        <div className="mb-2 grid grid-cols-2 gap-2">
          <Link href="/partner/login" className={quick}><Users className="size-3.5 shrink-0" aria-hidden /> Partner sign in</Link>
          <Link href="/vendor/login" className={quick}><Building2 className="size-3.5 shrink-0" aria-hidden /> Vendor sign in</Link>
        </div>
        <Link href="/support" className={quick}><LifeBuoy className="size-3.5 shrink-0" aria-hidden /> Submit a support ticket</Link>
      </div>

      <p className="mt-8 text-center text-xs text-fg-subtle">{branding.login_copyright}</p>
    </AuthLayout>
  );
}
