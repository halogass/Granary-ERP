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
  "Stay on top of every active engagement",
  "Get paid faster with digital invoicing",
  "Win more work through RFQ responses",
  "Keep your compliance documents up to date",
];

export default function VendorLoginPage() {
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
    <AuthLayout subtitle="Vendor portal" eyebrow="Vendor and contractor portal" headline="Work with us," accent="from one place."
      tagline="Everything you need to deliver, invoice, and collaborate with us."
      features={FEATURES} panelFooter="Secured by Granary">
      <div className="mb-8">
        <Pill tone="orange">Vendor or contractor</Pill>
        <h1 className="mt-3 text-2xl font-semibold tracking-tight text-fg">Vendor portal sign in</h1>
        <p className="mt-1 text-sm text-fg-muted">Enter your credentials to continue.</p>
      </div>

      {challenge ? (
        <TotpStep challenge={challenge} onSignedIn={goOn}
          onRestart={(msg) => { setChallenge(null); setError(msg); }} />
      ) : (
      <form onSubmit={handleSubmit} className="space-y-4">
        <Field label="Email address">
          <Input type="email" value={email} onChange={(e) => setEmail(e.target.value)} required autoComplete="email"
            placeholder="vendor@company.com" className="md:h-10" />
        </Field>
        <Field label="Password">
          <PasswordInput value={password} onChange={(e) => setPassword(e.target.value)} required autoComplete="current-password" className="md:h-10" />
        </Field>

        {error && <FormError>{error}</FormError>}

        <Button type="submit" variant="primary" size="lg" className="w-full" loading={loading}>
          {loading ? "Signing in..." : "Sign in to the vendor portal"}
        </Button>
      </form>
      )}

      <div className="mt-8 space-y-2 border-t border-line pt-6 text-center text-[13px] text-fg-muted">
        <p>
          New vendor?{" "}
          <Link href="/vendor/register" className="font-medium text-brand-ink hover:underline">Request portal access</Link>
        </p>
        <p>
          Staff member?{" "}
          <Link href="/login" className="text-fg hover:underline">Sign in to the ERP</Link>
        </p>
      </div>
    </AuthLayout>
  );
}
