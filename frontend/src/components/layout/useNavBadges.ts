"use client";

import { createContext, createElement, useContext, useEffect, useState } from "react";
import { usePathname } from "next/navigation";
import { approvalsApi, crmInboxApi, naryApi } from "@/lib/api";
import { usePermissions } from "@/hooks/usePermissions";
import { NARY_VIEW, type BadgeKey } from "./nav";

type Badges = Record<BadgeKey, number>;
const ZERO: Badges = { approvals: 0, nary: 0, crm_email: 0 };
const BadgeContext = createContext<Badges>(ZERO);

/** The "needs you" counts in the menu (from NavBadgesProvider in the ERP layout). */
export function useNavBadges(): Badges {
  return useContext(BadgeContext);
}

/** Fetches the menu counts once for the sidebar, bottom nav and anything else that shows them. */
export function NavBadgesProvider({ children }: { children: React.ReactNode }) {
  const badges = useFetchBadges();
  return createElement(BadgeContext.Provider, { value: badges }, children);
}

/** Approvals awaiting me, Nary cards pending, unread CRM email; polled every minute. */
function useFetchBadges(): Badges {
  const pathname = usePathname();
  const { loaded, holds } = usePermissions();
  const naryViewer = loaded && holds(NARY_VIEW);
  const [approvals, setApprovals] = useState(0);
  const [crmEmail, setCrmEmail] = useState(0);
  const [nary, setNary] = useState(0);

  useEffect(() => {
    let alive = true;
    const refresh = () => {
      crmInboxApi.getUnreadCount()
        .then((res) => { if (alive) setCrmEmail(res.data?.count ?? 0); })
        .catch(() => { /* no CRM access, or offline: no badge */ });
    };
    refresh();
    const timer = setInterval(refresh, 60_000);
    return () => { alive = false; clearInterval(timer); };
  }, [pathname]);

  useEffect(() => {
    let alive = true;
    const refresh = () => {
      approvalsApi.count()
        .then((res) => { if (alive) setApprovals(res.data?.awaiting_me ?? 0); })
        .catch(() => { /* not critical */ });
    };
    refresh();
    const timer = setInterval(refresh, 60_000);
    window.addEventListener("approvals:changed", refresh);
    return () => { alive = false; clearInterval(timer); window.removeEventListener("approvals:changed", refresh); };
  }, []);

  useEffect(() => {
    if (!naryViewer) return;
    let alive = true;
    const refresh = () => {
      naryApi.status()
        .then((res) => { if (alive) setNary(res.data?.pending_cards ?? 0); })
        .catch(() => { /* not critical */ });
    };
    refresh();
    const timer = setInterval(refresh, 60_000);
    return () => { alive = false; clearInterval(timer); };
  }, [naryViewer]);

  return { approvals, crm_email: crmEmail, nary };
}
