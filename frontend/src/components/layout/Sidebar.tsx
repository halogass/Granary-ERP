"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { cn } from "@/lib/utils";
import { useBranding } from "@/hooks/useBranding";
import { usePermissions } from "@/hooks/usePermissions";
import { NAV, activeHref, isGroup, permAllowed, type BadgeKey, type NavGroup, type NavLink, type NavSection } from "./nav";
import { useNavBadges } from "./useNavBadges";

interface SidebarProps {
  user: { is_superadmin: boolean; is_system_admin: boolean };
}

const OPEN_KEY = "sidebar-open";        // groups and sections the user opened
const COLLAPSED_KEY = "sidebar-collapsed";

function readSet(key: string): Set<string> {
  try { return new Set(JSON.parse(localStorage.getItem(key) ?? "[]") as string[]); } catch { return new Set(); }
}

const itemBase =
  "relative flex min-h-8 w-full items-center gap-2.5 rounded-lg px-2 text-sm text-nav-fg transition-colors " +
  "hover:bg-nav-hover hover:text-fg focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)]";
const itemActive = "bg-nav-active font-semibold text-fg shadow-[inset_2px_0_0_rgb(var(--brand))]";

function Count({ n }: { n?: number }) {
  if (!n) return null;
  return <span className="ml-auto text-xs font-medium text-brand-ink tabular" aria-label={`${n} waiting`}>{n > 99 ? "99+" : n}</span>;
}

export default function Sidebar({ user: _user }: SidebarProps) {
  const pathname = usePathname();
  const branding = useBranding();
  const { loaded, holds } = usePermissions();
  const badges = useNavBadges();
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [collapsed, setCollapsed] = useState(false);

  useEffect(() => {
    setOpen(readSet(OPEN_KEY));
    setCollapsed(localStorage.getItem(COLLAPSED_KEY) === "true");
  }, []);

  const current = useMemo(() => activeHref(pathname), [pathname]);
  const allowed = (l: { perm?: string }) => !l.perm || (loaded && permAllowed(l.perm, holds));

  function toggle(id: string) {
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      try { localStorage.setItem(OPEN_KEY, JSON.stringify(Array.from(next))); } catch { /* ignore */ }
      return next;
    });
  }

  function toggleCollapsed() {
    setCollapsed((prev) => {
      try { localStorage.setItem(COLLAPSED_KEY, String(!prev)); } catch { /* ignore */ }
      return !prev;
    });
  }

  const count = (b?: BadgeKey) => (b ? badges[b] : 0);
  const groupCount = (g: NavGroup) => g.links.reduce((n, l) => n + (allowed(l) ? count(l.badge) : 0), 0);
  const groupActive = (g: NavGroup) => g.links.some((l) => l.href === current);

  function renderLink(l: NavLink, opts: { indent?: boolean; icon?: boolean } = {}) {
    if (!allowed(l)) return null;
    const active = l.href === current;
    const Icon = l.icon;
    return (
      <Link key={l.href} href={l.href} aria-current={active ? "page" : undefined}
        className={cn(itemBase, opts.indent && (Icon && opts.icon !== false ? "pl-[18px]" : "pl-[34px]"), active && itemActive)}>
        {opts.icon !== false && Icon && <Icon className="size-4 shrink-0 opacity-80" aria-hidden />}
        <span className="truncate">{l.label}</span>
        <Count n={count(l.badge)} />
      </Link>
    );
  }

  /** A group that opens in place (Inbox, Settings, the modules under More). */
  function renderGroup(g: NavGroup, id: string) {
    const links = g.links.filter(allowed);
    if (!links.length) return null;
    const isOpen = open.has(id) || groupActive(g);
    const n = groupCount(g);
    return (
      <div key={id}>
        <button type="button" onClick={() => toggle(id)} aria-expanded={isOpen}
          className={cn(itemBase, !isOpen && groupActive(g) && itemActive)}>
          <g.icon className="size-4 shrink-0 opacity-80" aria-hidden />
          <span className="truncate">{g.label}</span>
          {!isOpen && <Count n={n} />}
          <ChevronDown className={cn("size-3.5 shrink-0 opacity-60 transition-transform", !n || isOpen ? "ml-auto" : "ml-1", !isOpen && "-rotate-90")} aria-hidden />
        </button>
        {isOpen && <div className="mt-px space-y-px">{links.map((l) => renderLink(l, { indent: true, icon: !!l.channel }))}</div>}
      </div>
    );
  }

  /** Sales / Money / Operate: a heading, the main links, then "Show more" for the rest. */
  function renderFlatSection(s: NavSection, g: NavGroup) {
    const main = g.links.filter((l) => !l.extra && allowed(l));
    const extra = g.links.filter((l) => l.extra && allowed(l));
    const id = `more:${s.id}`;
    const showExtra = open.has(id) || extra.some((l) => l.href === current);
    return (
      <div key={s.id} className="pt-3">
        <div className="px-2 pb-1 text-[11px] font-semibold text-fg-muted">{s.label}</div>
        <div className="space-y-px">
          {main.map((l) => renderLink(l, { icon: false }))}
          {showExtra && extra.map((l) => renderLink(l, { icon: false }))}
          {extra.length > 0 && (
            <button type="button" onClick={() => toggle(id)} aria-expanded={showExtra}
              className={cn(itemBase, "min-h-8 text-[13px] text-fg-muted")}>
              {showExtra ? "Show less" : `Show ${extra.length} more`}
            </button>
          )}
        </div>
      </div>
    );
  }

  function renderSection(s: NavSection) {
    if (s.label && !s.collapsible && s.items.length === 1 && isGroup(s.items[0])) {
      return renderFlatSection(s, s.items[0]);
    }
    const body = (
      <div className="space-y-px">
        {s.items.map((item) => isGroup(item) ? renderGroup(item, `${s.id}:${item.label}`) : renderLink(item))}
      </div>
    );
    if (!s.collapsible) return <div key={s.id} className={s.id === "top" ? "" : "pt-3"}>{body}</div>;

    const id = `section:${s.id}`;
    const anyActive = s.items.some((it) => isGroup(it) ? groupActive(it) : it.href === current);
    const isOpen = open.has(id) || anyActive;
    const n = s.items.reduce((acc, it) => acc + (isGroup(it) ? groupCount(it) : allowed(it) ? count(it.badge) : 0), 0);
    return (
      <div key={s.id} className="pt-3">
        <button type="button" onClick={() => toggle(id)} aria-expanded={isOpen}
          className="flex min-h-8 w-full items-center gap-1 rounded-lg px-2 text-[11px] font-semibold text-fg-muted hover:text-fg">
          <ChevronRight className={cn("size-3 transition-transform", isOpen && "rotate-90")} aria-hidden />
          {s.label}
          {!isOpen && <Count n={n} />}
        </button>
        {isOpen && body}
      </div>
    );
  }

  /* Icon rail */
  function renderRail() {
    const entries: { key: string; label: string; href: string; icon: React.ElementType; active: boolean; n: number }[] = [];
    for (const s of NAV) {
      for (const item of s.items) {
        if (isGroup(item)) {
          const first = item.links.find(allowed);
          if (!first) continue;
          entries.push({ key: `${s.id}:${item.label}`, label: item.label, href: first.href, icon: item.icon, active: groupActive(item), n: groupCount(item) });
        } else if (allowed(item) && item.icon) {
          entries.push({ key: item.href, label: item.label, href: item.href, icon: item.icon, active: item.href === current, n: count(item.badge) });
        }
      }
    }
    return entries.map((e) => (
      <Link key={e.key} href={e.href} title={e.label} aria-label={e.label} aria-current={e.active ? "page" : undefined}
        className={cn(itemBase, "justify-center px-0", e.active && itemActive)}>
        <e.icon className="size-[18px]" aria-hidden />
        {e.n > 0 && <span className="absolute right-2 top-1.5 size-2 rounded-full bg-brand" aria-hidden />}
      </Link>
    ));
  }

  const logo = branding.logo_type === "image" && branding.logo_image_data ? (
    // eslint-disable-next-line @next/next/no-img-element
    <img src={branding.logo_image_data} alt={branding.logo_text} className={cn("object-contain", collapsed ? "size-7 rounded-lg" : "h-7 max-w-[160px]")} />
  ) : (
    <>
      <span className="flex size-6 shrink-0 items-center justify-center rounded-lg bg-brand text-xs font-semibold text-brand-on">
        {(branding.logo_text || "A").charAt(0)}
      </span>
      {!collapsed && (
        <span className="min-w-0 truncate text-sm font-semibold text-fg">
          {branding.logo_text} <span className="font-normal text-fg-muted">{branding.logo_subtitle}</span>
        </span>
      )}
    </>
  );

  const main = NAV.filter((s) => s.id !== "bottom");
  const bottom = NAV.find((s) => s.id === "bottom");

  return (
    <aside aria-label="Main menu"
      className={cn("hidden h-full shrink-0 flex-col border-r border-line bg-nav transition-[width] duration-150 md:flex",
        collapsed ? "w-[60px]" : "w-56")}>
      <Link href="/dashboard" className={cn("flex h-12 shrink-0 items-center gap-2 border-b border-line", collapsed ? "justify-center" : "px-4")}>
        {logo}
      </Link>

      <nav aria-label="Main" className="flex-1 overflow-y-auto overscroll-contain px-2 py-2">
        {collapsed ? <div className="space-y-px">{renderRail()}</div> : main.map(renderSection)}
      </nav>

      <div className="shrink-0 space-y-px border-t border-line px-2 py-2">
        {!collapsed && bottom && renderSection(bottom)}
        <button type="button" onClick={toggleCollapsed} aria-label={collapsed ? "Expand menu" : "Collapse menu"} title={collapsed ? "Expand menu" : "Collapse menu"}
          className={cn(itemBase, "text-fg-muted", collapsed && "justify-center px-0")}>
          {collapsed ? <PanelLeftOpen className="size-4" aria-hidden /> : <PanelLeftClose className="size-4" aria-hidden />}
          {!collapsed && <span>Collapse menu</span>}
        </button>
      </div>
    </aside>
  );
}
