"use client";

import { usePathname } from "next/navigation";
import { useEffect } from "react";

// Sets the browser tab title from the current route so each page reflects where
// the user is (instead of a static "Granary" everywhere). Rendered once
// from the (erp) layout. Most labels are derived from the last meaningful path
// segment; a few are overridden for nicer wording.
const OVERRIDES: Record<string, string> = {
  "/dashboard": "Dashboard",
  "/nary": "Nary",
  "/nary/chat": "Nary · Chat",
  "/crm/email": "CRM Email Inbox",
  "/crm/whatsapp": "WhatsApp Inbox",
  "/messaging": "Messaging",
  "/sales/catalog": "Product Catalog",
  "/finance/fx": "Exchange Rates",
  "/finance/accounts": "Chart of Accounts",
  "/hr/org-chart": "Org Chart",
  "/settings": "Settings",
  "/settings/users": "User Management",
  "/profile": "My Profile",
};

const ID_SEGMENT = /^(?:[0-9a-f]{8,}|\d+|[0-9a-f-]{20,})$/i;

function titleCase(segment: string): string {
  return segment.replace(/-/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

function deriveTitle(path: string): string {
  if (OVERRIDES[path]) return OVERRIDES[path];
  const segments = path.split("/").filter(Boolean);
  // Walk back past id-like / route-group segments to the first readable label.
  for (let i = segments.length - 1; i >= 0; i--) {
    const seg = segments[i];
    if (!seg || seg.startsWith("(") || seg.startsWith("[") || ID_SEGMENT.test(seg)) continue;
    return titleCase(seg);
  }
  return "Granary";
}

export default function DocumentTitle() {
  const pathname = usePathname();
  useEffect(() => {
    document.title = `${deriveTitle(pathname)} · Granary`;
  }, [pathname]);
  return null;
}
