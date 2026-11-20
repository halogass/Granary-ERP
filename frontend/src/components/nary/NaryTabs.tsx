"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { usePermissions } from "@/hooks/usePermissions";
import { PageHeader, tabTriggerClass } from "@/components/ui";

/** Everyone may chat with Nary; the agent's own pages need nary:view (the API checks it too). */
export const CHAT_HREF = "/nary/chat";
export const NARY_VIEW = "nary:view";
/** 3 Nov 2026: the work queue is for the Owner (Nary's approver) and system admins (the API checks it too). */
export const QUEUE_HREF = "/nary/queue";
export const NARY_APPROVE = "nary:approve";

const TABS = [
  { href: CHAT_HREF, label: "Chat" },
  { href: "/nary", label: "Overview" },
  { href: "/nary/approvals", label: "Approvals" },
  { href: "/nary/intake", label: "Intake" },
  { href: "/nary/history", label: "WhatsApp history" },
  { href: "/nary/meetings", label: "Meetings" },
  { href: "/nary/report", label: "Report" },
  { href: "/nary/runs", label: "Runs" },
  { href: "/nary/memory", label: "Memory" },
  { href: "/nary/autonomy", label: "Autonomy" },
  { href: QUEUE_HREF, label: "Work queue" },
];

export default function NaryTabs({ subtitle, actions }: { subtitle?: string; actions?: React.ReactNode }) {
  const path = usePathname();
  const { loaded, holds, isAdmin } = usePermissions();
  const tabs = TABS.filter((t) => t.href === CHAT_HREF
    || (t.href === QUEUE_HREF ? loaded && (isAdmin || holds(NARY_APPROVE)) : loaded && holds(NARY_VIEW)));
  return (
    <div className="mb-4">
      <PageHeader title="Nary" summary={subtitle} actions={actions} className="mb-2" />
      <nav className="flex gap-1 overflow-x-auto shadow-[inset_0_-1px_0_rgb(var(--line))]" aria-label="Nary sections">
        {tabs.map((t) => (
          <Link key={t.href} href={t.href} aria-current={path === t.href ? "page" : undefined} className={tabTriggerClass}>
            {t.label}
          </Link>
        ))}
      </nav>
    </div>
  );
}
