"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { signOut } from "next-auth/react";
import { ChevronDown, LogOut } from "lucide-react";
import { DropdownMenu, MenuItem, MenuRadioGroup, MenuSeparator, useScrollFade } from "@/components/ui";
import { useTheme, type ThemePref } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { PortalBrand } from "./PortalBrand";
import { THEME_OPTIONS } from "./ThemeMenu";

/**
 * The frame of the partner and vendor portals: brand, a tab row of sections, the user menu.
 * Separate from the ERP shell, same look.
 */
export function PortalShell({ subtitle, nav, user, loginPath, children }: {
  subtitle: string;
  nav: { label: string; href: string }[];
  user: { name?: string | null; email?: string | null };
  /** Where "Sign out" lands. */
  loginPath: string;
  children: React.ReactNode;
}) {
  const pathname = usePathname() ?? "";
  const { theme, setTheme } = useTheme();
  const who = user.name || user.email || "Account";
  // Phones: the section row scrolls sideways, fades where more tabs hide, keeps the current one in view.
  const fade = useScrollFade<HTMLElement>(pathname);
  return (
    <div className="min-h-screen bg-canvas text-fg">
      <header className="sticky top-0 z-40 border-b border-line bg-surface">
        <div className="mx-auto flex h-14 max-w-6xl items-center gap-3 px-4 md:px-6">
          <Link href={nav[0]?.href ?? "/"} className="min-w-0 rounded-lg focus-visible:outline focus-visible:outline-2 focus-visible:outline-[rgb(var(--focus)/0.7)]">
            <PortalBrand subtitle={subtitle} />
          </Link>
          <div className="ml-auto flex items-center gap-1">
            <DropdownMenu className="w-60" trigger={
              <button type="button" className="flex h-11 max-w-[220px] items-center gap-2 rounded-lg px-2 text-sm text-fg hover:bg-surface-2 md:h-9">
                <span aria-hidden className="flex size-7 shrink-0 items-center justify-center rounded-full bg-surface-2 text-xs font-semibold text-fg-muted">
                  {who.charAt(0).toUpperCase()}
                </span>
                <span className="hidden truncate sm:block">{who}</span>
                <ChevronDown className="hidden size-3.5 shrink-0 text-fg-subtle sm:block" aria-hidden />
              </button>
            }>
              <div className="px-2.5 py-2">
                <div className="truncate text-sm font-semibold text-fg">{user.name ?? who}</div>
                {user.email && <div className="truncate text-xs text-fg-muted">{user.email}</div>}
              </div>
              <MenuSeparator />
              <MenuRadioGroup label="Theme" value={theme} onValueChange={(v) => setTheme(v as ThemePref)} options={THEME_OPTIONS} />
              <MenuSeparator />
              <MenuItem icon={LogOut} onSelect={() => signOut({ callbackUrl: loginPath })}>Sign out</MenuItem>
            </DropdownMenu>
          </div>
        </div>
        <nav ref={fade.ref} onScroll={fade.onScroll} style={fade.style} aria-label={subtitle}
          className="mx-auto max-w-6xl overflow-x-auto px-2 [scrollbar-width:none] md:px-4">
          <ul className="flex min-w-max gap-0.5">
            {nav.map((item) => {
              const active = pathname === item.href || pathname.startsWith(item.href + "/");
              return (
                <li key={item.href}>
                  <Link href={item.href} aria-current={active ? "page" : undefined}
                    className={cn("flex h-11 items-center border-b-2 px-3 text-sm transition-colors md:h-10",
                      active ? "border-brand font-semibold text-fg" : "border-transparent text-fg-muted hover:text-fg")}>
                    {item.label}
                  </Link>
                </li>
              );
            })}
          </ul>
        </nav>
      </header>
      <main className="mx-auto max-w-6xl px-4 py-6 md:px-6">{children}</main>
    </div>
  );
}
