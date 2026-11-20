"use client";

import Link from "next/link";
import type { LucideIcon } from "lucide-react";
import { safeHref } from "@/lib/sanitize";

/** One read-only built-in field in a record page's identity card. Renders nothing when empty. */
export default function Fact({ icon: Icon, label, value, href }: {
  icon: LucideIcon;
  label: string;
  value?: string | null;
  href?: string;
}) {
  if (!value) return null;
  // Values are user-entered (website, LinkedIn): never render a javascript: link.
  const link = safeHref(href);
  return (
    <div className="flex items-start gap-2.5 py-1.5">
      <Icon className="mt-0.5 size-3.5 shrink-0 text-fg-subtle" aria-hidden />
      <div className="min-w-0 flex-1">
        <p className="text-xs text-fg-muted">{label}</p>
        {link
          ? <Link href={link} target={link.startsWith("http") ? "_blank" : undefined}
              rel={link.startsWith("http") ? "noopener noreferrer" : undefined}
              className="block truncate text-sm text-brand-ink hover:underline">{value}</Link>
          : <span className="block truncate text-sm text-fg">{value}</span>}
      </div>
    </div>
  );
}
