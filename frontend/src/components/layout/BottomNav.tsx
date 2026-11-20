"use client";

import { useEffect, useMemo, useState } from "react";
import { usePathname } from "next/navigation";
import Link from "next/link";
import { FolderKanban, LayoutDashboard, MessageSquare, MoreHorizontal, Search, Users } from "lucide-react";
import { cn } from "@/lib/utils";
import { usePermissions } from "@/hooks/usePermissions";
import { Sheet } from "@/components/ui/Dialog";
import { NAV, activeHref, isGroup, permAllowed, type NavLink } from "./nav";
import { useNavBadges } from "./useNavBadges";
import { openCommandPalette } from "./CommandPalette";

/** Phones: four main places and More, which holds the whole menu. */
export default function BottomNav() {
  const pathname = usePathname();
  const [moreOpen, setMoreOpen] = useState(false);
  const { loaded, holds } = usePermissions();
  const badges = useNavBadges();
  const current = useMemo(() => activeHref(pathname), [pathname]);

  useEffect(() => { setMoreOpen(false); }, [pathname]);

  // Opening More shows where you are in the long menu.
  useEffect(() => {
    if (!moreOpen) return;
    const t = setTimeout(() => {
      document.querySelector<HTMLElement>('[role="dialog"] a[aria-current="page"]')?.scrollIntoView?.({ block: "center" });
    }, 50);
    return () => clearTimeout(t);
  }, [moreOpen]);

  const allowed = (l: { perm?: string }) => !l.perm || (loaded && permAllowed(l.perm, holds));
  const under = (prefixes: string[]) => prefixes.some((p) => pathname === p || pathname.startsWith(p + "/"));

  // The same four tabs as before the redesign; "More" opens the whole menu tree.
  const tabs = [
    { label: "Dashboard", href: "/dashboard", icon: LayoutDashboard, active: under(["/dashboard"]), n: 0 },
    { label: "Messages", href: "/messaging", icon: MessageSquare, active: under(["/messaging"]), n: 0 },
    { label: "CRM", href: "/crm/contacts", icon: Users, active: under(["/crm"]), n: badges.crm_email },
    { label: "Projects", href: "/projects", icon: FolderKanban, active: under(["/projects"]), n: 0 },
  ];
  const moreCount = badges.approvals;

  function linkRow(l: NavLink) {
    if (!allowed(l)) return null;
    const active = l.href === current;
    const n = l.badge ? badges[l.badge] : 0;
    return (
      <Link key={l.href} href={l.href} aria-current={active ? "page" : undefined}
        className={cn("flex min-h-11 items-center gap-2 rounded-lg px-3 text-sm",
          active ? "bg-nav-active font-semibold text-fg shadow-[inset_2px_0_0_rgb(var(--brand))]" : "text-fg hover:bg-surface-2")}>
        <span className="flex-1 truncate">{l.label}</span>
        {n > 0 && <span className="text-xs font-medium text-brand-ink tabular">{n}</span>}
      </Link>
    );
  }

  return (
    <>
      <Sheet open={moreOpen} onOpenChange={setMoreOpen} side="bottom" title="All pages">
        <div className="-mx-2 space-y-4">
          <button type="button" onClick={() => { setMoreOpen(false); openCommandPalette(); }}
            className="mx-2 flex min-h-11 w-[calc(100%-1rem)] items-center gap-2 rounded-lg border border-line bg-canvas px-3 text-left text-sm text-fg-subtle">
            <Search className="size-4 shrink-0" aria-hidden />
            Go to a page, record or setting
          </button>
          {NAV.map((s) => {
            const groups = s.items.map((item) => isGroup(item)
              ? { label: item.label, links: item.links.filter(allowed) }
              : { label: "", links: allowed(item) ? [item] : [] }).filter((g) => g.links.length);
            if (!groups.length) return null;
            return (
              <section key={s.id}>
                {s.label && <h3 className="px-3 pb-1 text-[11px] font-semibold text-fg-muted">{s.label}</h3>}
                {groups.map((g, i) => (
                  <div key={`${g.label}${i}`}>
                    {g.label && g.label !== s.label && <div className="px-3 pb-0.5 pt-2 text-xs font-medium text-fg-muted">{g.label}</div>}
                    {g.links.map(linkRow)}
                  </div>
                ))}
              </section>
            );
          })}
          {linkRow({ label: "My profile", href: "/profile" })}
        </div>
      </Sheet>

      <nav aria-label="Main" className="fixed inset-x-0 bottom-0 z-30 border-t border-line bg-surface pb-[env(safe-area-inset-bottom)] md:hidden">
        <div className="flex h-[var(--shell-nav)] items-stretch">
          {tabs.map((tab) => (
            <Link key={tab.label} href={tab.href} aria-current={tab.active ? "page" : undefined}
              className={cn("relative flex flex-1 flex-col items-center justify-center gap-0.5 text-[11px] font-medium",
                tab.active ? "text-brand-ink" : "text-fg-muted")}>
              <tab.icon className="size-5" aria-hidden />
              {tab.label}
              {tab.n > 0 && (
                <span className="absolute left-1/2 top-1.5 ml-2 flex h-4 min-w-4 items-center justify-center rounded-full bg-brand px-1 text-[10px] font-semibold leading-none text-brand-on tabular">
                  {tab.n > 99 ? "99+" : tab.n}
                </span>
              )}
            </Link>
          ))}
          <button type="button" onClick={() => setMoreOpen(true)} aria-expanded={moreOpen}
            className={cn("relative flex flex-1 flex-col items-center justify-center gap-0.5 text-[11px] font-medium",
              moreOpen ? "text-brand-ink" : "text-fg-muted")}>
            <MoreHorizontal className="size-5" aria-hidden />
            More
            {moreCount > 0 && <span className="absolute left-1/2 top-2 ml-2 size-2 rounded-full bg-brand" aria-label={`${moreCount} approvals waiting`} />}
          </button>
        </div>
      </nav>
    </>
  );
}
