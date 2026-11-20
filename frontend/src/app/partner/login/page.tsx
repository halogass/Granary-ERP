"use client";

import { useState } from "react";
import { signIn } from "next-auth/react";
import { useRouter } from "next/navigation";
import Link from "next/link";
import { Button, Field, Input, Pill } from "@/components/ui";
import { AuthLayout, FormError, PasswordInput } from "@/components/portal/AuthLayout";
import { TotpStep } from "@/components/portal/TotpStep";
import { afterLoginPath, challengeFromError, firstStepMessage } from "@/lib/totpLogin";

const FEATURES = [
  "Co-build your pipeline with us",
  "Receive and act on warm leads from us",
  "Fund joint marketing through MDF",
  "Level up your team with our training",
  "See exactly what you've earned",
];

export default function PartnerLoginPage() {
  const router = useRouter();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  // Two-factor sign-in: the challenge after a right password, while the code is asked
  const [challenge, setChallenge] = useState<string | null>(null);

  async function goOn() {
    const { getSession } = await import("next-auth/react");
    const session = await getSession();
    router.push(afterLoginPath(session?.user?.portal_type, ""));
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setLoading(true);
    setError("");
    const result = await signIn("credentials", { email, password, redirect: false });
    setLoading(false);
    const totp = challengeFromError(result?.error);
    if (totp) {
      setPassword("");
      setChallenge(totp);
    } else if (result?.error) {
      setError(firstStepMessage(result.error));
    } else {
      await goOn();
    }
  }

  return (
    <AuthLayout subtitle="Partner portal" eyebrow="Partner Program" headline="Built to grow" accent="together."
      tagline="Your dedicated workspace to co-sell, co-market, and grow alongside us."
      features={FEATURES} panelFooter="Secured by Granary">
      <div className="mb-8">
        <Pill tone="blue">Partner program</Pill>
        <h1 className="mt-3 text-2xl font-semibold tracking-tight text-fg">Partner portal sign in</h1>
        <p className="mt-1 text-sm text-fg-muted">Enter your credentials to continue.</p>
      </div>

      {challenge ? (
        <TotpStep challenge={challenge} onSignedIn={goOn}
          onRestart={(msg) => { setChallenge(null); setError(msg); }} />
      ) : (
      <form onSubmit={handleSubmit} className="space-y-4">
        <Field label="Email address">
          <Input type="email" value={email} onChange={(e) => setEmail(e.target.value)} required autoComplete="email"
            placeholder="partner@company.com" className="md:h-10" />
        </Field>
        <Field label="Password">
          <PasswordInput value={password} onChange={(e) => setPassword(e.target.value)} required autoComplete="current-password" className="md:h-10" />
        </Field>

        {error && <FormError>{error}</FormError>}

        <Button type="submit" variant="primary" size="lg" className="w-full" loading={loading}>
          {loading ? "Signing in..." : "Sign in to the partner portal"}
        </Button>
      </form>
      )}

      <div className="mt-8 space-y-2 border-t border-line pt-6 text-center text-[13px] text-fg-muted">
        <p>
          Not a partner yet?{" "}
          <Link href="/partner/register" className="font-medium text-brand-ink hover:underline">Apply to join</Link>
        </p>
        <p>
          Staff member?{" "}
          <Link href="/login" className="text-fg hover:underline">Sign in to the ERP</Link>
        </p>
      </div>
    </AuthLayout>
  );
}
