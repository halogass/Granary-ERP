"use client";

import { forwardRef, useState } from "react";
import { AlertTriangle, Check, Eye, EyeOff } from "lucide-react";
import { Input, type InputProps } from "@/components/ui";
import { PortalBrand } from "./PortalBrand";
import { ThemeMenu } from "./ThemeMenu";

/**
 * Sign-in and sign-up pages: a brand panel on the left (from lg up) and the form on the right.
 * Calm, token-colored, light and dark.
 */
export function AuthLayout({ subtitle, eyebrow, headline, accent, tagline, features, panelFooter, children }: {
  subtitle?: string;
  eyebrow?: React.ReactNode;
  headline: React.ReactNode;
  accent?: React.ReactNode;
  tagline?: React.ReactNode;
  features?: string[];
  panelFooter?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex min-h-screen bg-canvas text-fg">
      <aside className="relative hidden flex-col justify-between overflow-hidden border-r border-line bg-surface p-12 lg:flex lg:w-[52%]">
        <div aria-hidden className="pointer-events-none absolute inset-y-0 left-0 w-1 bg-brand" />
        <PortalBrand subtitle={subtitle} size="lg" />
        <div className="max-w-md py-10">
          {eyebrow && <p className="mb-3 text-xs font-semibold uppercase tracking-[0.16em] text-brand-ink">{eyebrow}</p>}
          <h2 className="text-[2.1rem] font-semibold leading-tight text-fg">
            {headline}
            {accent && <><br /><span className="text-brand-ink">{accent}</span></>}
          </h2>
          {tagline && <p className="mt-4 text-base leading-relaxed text-fg-muted">{tagline}</p>}
          {features && features.length > 0 && (
            <ul className="mt-8 space-y-3">
              {features.map((f) => (
                <li key={f} className="flex items-center gap-3 text-sm text-fg">
                  <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-brand/10 text-brand-ink">
                    <Check className="size-3" aria-hidden />
                  </span>
                  {f}
                </li>
              ))}
            </ul>
          )}
        </div>
        <p className="text-xs text-fg-subtle">{panelFooter}</p>
      </aside>

      <main className="relative flex w-full flex-col items-center justify-center px-4 py-12 sm:px-8 lg:w-[48%]">
        <div className="absolute right-3 top-3"><ThemeMenu /></div>
        <div className="w-full max-w-[380px]">
          <div className="mb-8 flex justify-center lg:hidden"><PortalBrand subtitle={subtitle} size="lg" /></div>
          {children}
        </div>
      </main>
    </div>
  );
}

/** A password field with a show/hide toggle. */
export const PasswordInput = forwardRef<HTMLInputElement, Omit<InputProps, "type">>(function PasswordInput({ className, ...props }, ref) {
  const [show, setShow] = useState(false);
  return (
    <div className="relative">
      <Input ref={ref} type={show ? "text" : "password"} className={`pr-11 ${className ?? ""}`} {...props} />
      <button type="button" onClick={() => setShow((v) => !v)} aria-label={show ? "Hide password" : "Show password"} aria-pressed={show}
        className="absolute right-0.5 top-1/2 flex size-10 -translate-y-1/2 items-center justify-center rounded-lg text-fg-subtle hover:text-fg md:size-8">
        {show ? <EyeOff className="size-4" aria-hidden /> : <Eye className="size-4" aria-hidden />}
      </button>
    </div>
  );
});

/** An inline error under a sign-in form. */
export function FormError({ children }: { children: React.ReactNode }) {
  return (
    <div role="alert" className="flex items-start gap-2.5 rounded-lg border border-tone-red/30 bg-tone-red-soft px-3.5 py-3 text-sm text-tone-red">
      <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
      <span>{children}</span>
    </div>
  );
}
