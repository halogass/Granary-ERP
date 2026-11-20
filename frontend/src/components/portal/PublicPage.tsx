"use client";

import { cn } from "@/lib/utils";
import { PortalBrand } from "./PortalBrand";
import { ThemeMenu } from "./ThemeMenu";

const widths = { sm: "max-w-md", md: "max-w-2xl", lg: "max-w-4xl", xl: "max-w-6xl" };

/**
 * The frame of every page a customer or partner opens without the ERP: brand bar on top,
 * a calm centred column, a quiet footer. Follows light and dark.
 */
export function PublicPage({ subtitle, width = "md", center, aside, footer, className, children }: {
  subtitle?: string;
  width?: keyof typeof widths;
  /** Vertically centre the content (short pages: a result, a confirmation). */
  center?: boolean;
  /** Extra controls on the right of the brand bar. */
  aside?: React.ReactNode;
  footer?: React.ReactNode;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex min-h-screen flex-col bg-canvas text-fg">
      <header className="border-b border-line bg-surface">
        <div className={cn("mx-auto flex h-14 items-center gap-3 px-4", widths[width === "sm" ? "md" : width])}>
          <PortalBrand subtitle={subtitle} />
          <div className="ml-auto flex items-center gap-1">
            {aside}
            <ThemeMenu />
          </div>
        </div>
      </header>
      <main className={cn("mx-auto w-full flex-1 px-4 py-8", widths[width], center && "flex flex-col justify-center", className)}>
        {children}
      </main>
      <footer className="px-4 pb-6 text-center text-xs text-fg-subtle">{footer ?? "Granary"}</footer>
    </div>
  );
}

/** A centred result block (success, error, expired) with an icon in a tinted square. */
export function PublicNotice({ icon: Icon, tone = "neutral", title, children, action }: {
  icon: React.ElementType;
  tone?: "neutral" | "blue" | "red" | "orange";
  title: React.ReactNode;
  children?: React.ReactNode;
  action?: React.ReactNode;
}) {
  const tones = {
    neutral: "bg-tone-neutral-soft text-tone-neutral",
    blue: "bg-tone-blue-soft text-tone-blue",
    red: "bg-tone-red-soft text-tone-red",
    orange: "bg-tone-orange-soft text-tone-orange",
  };
  return (
    <div className="rounded-lg border border-line bg-surface px-6 py-10 text-center shadow-card">
      <span className={cn("mx-auto mb-4 flex size-12 items-center justify-center rounded-lg", tones[tone])}>
        <Icon className="size-6" aria-hidden />
      </span>
      <h1 className="text-lg font-semibold text-fg">{title}</h1>
      {children && <div className="mx-auto mt-2 max-w-sm text-sm text-fg-muted">{children}</div>}
      {action && <div className="mt-6 flex justify-center">{action}</div>}
    </div>
  );
}
