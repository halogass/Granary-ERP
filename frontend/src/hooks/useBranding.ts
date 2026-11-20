"use client";

import { useEffect, useState } from "react";

export interface Branding {
  logo_type: "text" | "image";
  logo_text: string;
  logo_subtitle: string;
  logo_image_data: string;
  // Login page text
  login_badge: string;
  login_headline: string;
  login_headline_accent: string;
  login_tagline: string;
  login_features: string;
  login_footer: string;
  login_subtitle: string;
  login_copyright: string;
}

const DEFAULT: Branding = {
  logo_type: "text",
  logo_text: "Granary",
  logo_subtitle: "ERP",
  logo_image_data: "",
  login_badge: "GRANARY",
  login_headline: "The ERP that thinks",
  login_headline_accent: "alongside your team.",
  login_tagline: "Nary, the built-in AI, works across sales, finance, people and operations: it drafts the next step and waits for your approval.",
  login_features: "AI-drafted quotations, proposals and replies\nLead enrichment, research and scoring\nEmail and WhatsApp triage with suggested replies\nEvery AI action approved by a person\nFinance, HR, projects and procurement in one place\nConnect your own AI client over MCP",
  login_footer: "Open-source ERP with built-in AI",
  login_subtitle: "Sign in to continue.",
  login_copyright: "Open-source ERP with built-in AI",
};

// Module-level cache so multiple components share one fetch
let _cache: Branding | null = null;
let _promise: Promise<Branding> | null = null;

async function fetchBranding(): Promise<Branding> {
  if (_cache) return _cache;
  if (!_promise) {
    _promise = fetch("/api/v1/system/branding", { cache: "no-store" })
      .then((r) => r.json())
      .then((data) => {
        _cache = { ...DEFAULT, ...data };
        return _cache!;
      })
      .catch(() => DEFAULT);
  }
  return _promise;
}

export function invalidateBrandingCache() {
  _cache = null;
  _promise = null;
}

export function useBranding() {
  const [branding, setBranding] = useState<Branding>(_cache ?? DEFAULT);

  useEffect(() => {
    fetchBranding().then(setBranding);
  }, []);

  return branding;
}
