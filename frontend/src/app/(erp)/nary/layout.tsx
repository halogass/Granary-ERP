"use client";

import { useEffect } from "react";
import { usePathname, useRouter } from "next/navigation";
import { usePermissions } from "@/hooks/usePermissions";
import { CHAT_HREF, NARY_APPROVE, NARY_VIEW, QUEUE_HREF } from "@/components/nary/NaryTabs";

/** Staff without nary:view only have Nary's chat; any other Nary page sends them there. */
export default function NaryLayout({ children }: { children: React.ReactNode }) {
  const path = usePathname();
  const router = useRouter();
  const { loaded, holds, isAdmin } = usePermissions();
  // 3 Nov 2026: the work queue also opens for system admins (the Owner and system admins only, as the API)
  const allowed = path === CHAT_HREF || (loaded && holds(NARY_VIEW))
    || (path === QUEUE_HREF && loaded && (isAdmin || holds(NARY_APPROVE)));
  useEffect(() => {
    if (loaded && !allowed) router.replace(CHAT_HREF);
  }, [loaded, allowed, router]);
  if (path !== CHAT_HREF && !allowed) return null;   // nothing (no 403s) while checking or redirecting
  return <>{children}</>;
}
