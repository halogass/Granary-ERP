"use client";

import { useBranding } from "@/hooks/useBranding";
import { cn } from "@/lib/utils";

/** The company mark for portals and public pages: logo (or a red letter mark) plus an optional subtitle. */
export function PortalBrand({ subtitle, size = "md", className }: { subtitle?: string; size?: "md" | "lg"; className?: string }) {
  const branding = useBranding();
  const image = branding.logo_type === "image" && branding.logo_image_data;
  const sub = subtitle ?? branding.logo_subtitle;
  const lg = size === "lg";
  return (
    <div className={cn("flex min-w-0 items-center gap-2.5", className)}>
      {image ? (
        // eslint-disable-next-line @next/next/no-img-element
        <img src={branding.logo_image_data} alt={branding.logo_text} className={cn("w-auto object-contain", lg ? "h-10 max-w-[200px]" : "h-7 max-w-[160px]")} />
      ) : (
        <>
          <span aria-hidden className={cn("flex shrink-0 items-center justify-center rounded-lg bg-brand font-semibold text-brand-on",
            lg ? "size-9 text-base" : "size-7 text-sm")}>
            {(branding.logo_text || "A").charAt(0)}
          </span>
          <span className={cn("font-semibold tracking-wide text-fg", lg ? "text-xl" : "text-base")}>{branding.logo_text}</span>
        </>
      )}
      {sub && (
        <span className={cn("truncate border-l border-line pl-2.5 text-fg-muted", lg ? "text-sm" : "text-[13px]")}>{sub}</span>
      )}
    </div>
  );
}
