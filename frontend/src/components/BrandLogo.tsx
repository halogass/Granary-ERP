"use client";

import { useBranding } from "@/hooks/useBranding";

interface BrandLogoProps {
  /** Override subtitle (e.g. "Partner Portal", "Vendor Portal") */
  subtitleOverride?: string;
  /** Layout context: adjusts sizing */
  variant?: "sidebar" | "login" | "header";
  className?: string;
}

export default function BrandLogo({ subtitleOverride, variant = "sidebar", className = "" }: BrandLogoProps) {
  const branding = useBranding();
  const subtitle = subtitleOverride ?? branding.logo_subtitle;

  const showImage = branding.logo_type === "image" && branding.logo_image_data;

  if (variant === "login") {
    return (
      <div className={`text-center ${className}`}>
        {showImage ? (
          <div className="flex justify-center mb-1">
            <img
              src={branding.logo_image_data}
              alt={branding.logo_text}
              className="h-14 w-auto object-contain"
            />
          </div>
        ) : (
          <>
            <div className="text-4xl font-semibold tracking-widest text-brand-ink mb-1">
              {branding.logo_text}
            </div>
            <div className="text-sm font-medium text-fg-muted">{subtitle}</div>
          </>
        )}
        {showImage && (
          <div className="text-sm font-medium text-fg-muted mt-1">{subtitle}</div>
        )}
      </div>
    );
  }

  if (variant === "header") {
    return (
      <div className={`flex items-center gap-3 ${className}`}>
        {showImage ? (
          <img
            src={branding.logo_image_data}
            alt={branding.logo_text}
            className="h-7 w-auto object-contain"
          />
        ) : (
          <div className="text-xl font-semibold tracking-widest text-brand-ink">
            {branding.logo_text}
          </div>
        )}
        <div className="border-l border-line pl-3 text-xs font-medium text-fg-muted">
          {subtitle}
        </div>
      </div>
    );
  }

  // variant === "sidebar" (default)
  return (
    <div className={className}>
      {showImage ? (
        <img
          src={branding.logo_image_data}
          alt={branding.logo_text}
          className="h-8 w-auto object-contain max-w-[140px]"
        />
      ) : (
        <>
          <div className="text-xl font-semibold tracking-widest text-brand-ink">
            {branding.logo_text}
          </div>
          <div className="mt-0.5 text-xs font-medium text-fg-muted">{subtitle}</div>
        </>
      )}
    </div>
  );
}
