"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { usePathname, useRouter } from "next/navigation";
import { Command } from "cmdk";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { Clock, CornerDownLeft, FileText, Monitor, Moon, Search, Settings, Sparkles, Sun } from "lucide-react";
import { usePermissions } from "@/hooks/usePermissions";
import { useTheme } from "@/lib/theme";
import { recentRecords, recordListPath, rememberRecord, type RecentRecord } from "@/lib/recent";
import { settingsSearchEntries } from "@/lib/settings-registry";
import { Kbd } from "@/components/ui/Feedback";
import { PALETTE_EXTRAS, activeHref, allNavLinks, permAllowed } from "./nav";

const OPEN_EVENT = "palette:open";

/** Opens the command palette from anywhere (the topbar search box, a button). */
export function openCommandPalette() {
  window.dispatchEvent(new Event(OPEN_EVENT));
}

/** Notes record pages as they are opened, titled from the page's h1 once it has loaded. */
function useRecordTracking() {
  const pathname = usePathname();
  useEffect(() => {
    const list = recordListPath(pathname);
    if (!list) return;
    const listHref = activeHref(list);
    const kind = allNavLinks().find((l) => l.href === listHref)?.label ?? "Record";
    const save = () => {
      const h1 = document.querySelector("main h1")?.textContent?.trim();
      if (h1) rememberRecord({ href: pathname, title: h1.slice(0, 120), kind });
    };
    const t1 = setTimeout(save, 1200);
    const t2 = setTimeout(save, 4000);   // slow loads: the h1 may still be "Loading"
    return () => { clearTimeout(t1); clearTimeout(t2); };
  }, [pathname]);
}

const itemClass =
  "flex min-h-11 cursor-pointer select-none items-center gap-3 rounded-lg px-3 text-sm text-fg md:min-h-9 " +
  "data-[selected=true]:bg-surface-2 data-[disabled=true]:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0";
const groupClass =
  "[&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:pb-1 [&_[cmdk-group-heading]]:pt-2 " +
  "[&_[cmdk-group-heading]]:text-[11px] [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:text-fg-muted";

/** Ctrl K / Cmd K: jump to any page, a recent record, a setting by name, or switch the theme. */
export default function CommandPalette() {
  const router = useRouter();
  const { loaded, holds } = usePermissions();
  const { theme, setTheme } = useTheme();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [recent, setRecent] = useState<RecentRecord[]>([]);

  useRecordTracking();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((o) => !o);
      }
    };
    const onOpen = () => setOpen(true);
    document.addEventListener("keydown", onKey);
    window.addEventListener(OPEN_EVENT, onOpen);
    return () => { document.removeEventListener("keydown", onKey); window.removeEventListener(OPEN_EVENT, onOpen); };
  }, []);

  useEffect(() => {
    if (open) { setRecent(recentRecords()); setQuery(""); }
  }, [open]);

  const pages = useMemo(() => {
    const seen = new Set<string>();
    return [...allNavLinks(), ...PALETTE_EXTRAS.map((l) => ({ ...l, trail: "" }))].filter((l) => {
      if (seen.has(l.href)) return false;
      seen.add(l.href);
      return !l.perm || (loaded && permAllowed(l.perm, holds));
    });
  }, [loaded, holds]);

  const settings = useMemo(() => settingsSearchEntries(), []);

  const go = useCallback((href: string) => {
    setOpen(false);
    router.push(href);
  }, [router]);

  const themeItems = [
    { value: "system" as const, label: "Use the computer's theme", icon: Monitor },
    { value: "light" as const, label: "Light theme", icon: Sun },
    { value: "dark" as const, label: "Dark theme", icon: Moon },
  ];

  return (
    <DialogPrimitive.Root open={open} onOpenChange={setOpen}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-[70] bg-black/40" />
        <DialogPrimitive.Content aria-describedby={undefined}
          className="fixed left-1/2 top-3 z-[70] md:top-[12vh] w-[calc(100vw-2rem)] max-w-xl -translate-x-1/2 overflow-hidden rounded-lg border border-line bg-surface-raised text-fg shadow-pop focus:outline-none">
          <DialogPrimitive.Title className="sr-only">Go to</DialogPrimitive.Title>
          <Command label="Go to a page, record or setting" loop>
            <div className="flex items-center gap-2 border-b border-line px-3">
              <Search className="size-4 shrink-0 text-fg-subtle" aria-hidden />
              <Command.Input value={query} onValueChange={setQuery} placeholder="Go to a page, record or setting"
                className="h-12 w-full bg-transparent text-base text-fg md:text-[15px] placeholder:text-fg-subtle focus:outline-none" />
              <Kbd>Esc</Kbd>
            </div>
            <Command.List className="max-h-[min(60vh,440px)] overflow-y-auto overscroll-contain p-1.5">
              <Command.Empty className="px-3 py-8 text-center text-sm text-fg-muted">
                Nothing matches &quot;{query}&quot;. Try a module name like Invoices or Payroll.
              </Command.Empty>

              {recent.length > 0 && (
                <Command.Group heading="Recent records" className={groupClass}>
                  {recent.map((r) => (
                    <Command.Item key={r.href} value={`recent ${r.title} ${r.kind} ${r.href}`} onSelect={() => go(r.href)} className={itemClass}>
                      <Clock className="text-fg-subtle" aria-hidden />
                      <span className="min-w-0 flex-1 truncate">{r.title}</span>
                      <span className="shrink-0 text-xs text-fg-muted">{r.kind}</span>
                    </Command.Item>
                  ))}
                </Command.Group>
              )}

              <Command.Group heading="Pages" className={groupClass}>
                {pages.map((p) => {
                  const Icon = p.icon ?? FileText;
                  return (
                    <Command.Item key={p.href} value={`${p.label} ${p.trail} ${p.href}`} keywords={p.keywords}
                      onSelect={() => go(p.href)} className={itemClass}>
                      <Icon className="text-fg-subtle" aria-hidden />
                      <span className="min-w-0 flex-1 truncate">{p.label}</span>
                      {p.trail && <span className="shrink-0 text-xs text-fg-muted">{p.trail}</span>}
                    </Command.Item>
                  );
                })}
              </Command.Group>

              {query.trim().length > 0 && (
                <Command.Group heading="Settings" className={groupClass}>
                  {settings.map((s) => (
                    <Command.Item key={s.id} value={`setting ${s.label} ${s.trail} ${s.hint ?? ""} ${s.key ?? ""}`}
                      onSelect={() => go(s.href)} className={itemClass}>
                      <Settings className="text-fg-subtle" aria-hidden />
                      <span className="min-w-0 flex-1 truncate">{s.label}</span>
                      <span className="shrink-0 text-xs text-fg-muted">{s.trail}</span>
                    </Command.Item>
                  ))}
                </Command.Group>
              )}

              <Command.Group heading="Actions" className={groupClass}>
                <Command.Item value="ask nary ai assistant question" onSelect={() => { setOpen(false); window.dispatchEvent(new Event("assistant:open")); }} className={itemClass}>
                  <Sparkles className="text-fg-subtle" aria-hidden />
                  <span className="flex-1">Ask Nary</span>
                  <span className="hidden items-center gap-1 md:flex"><Kbd>Ctrl</Kbd><Kbd>J</Kbd></span>
                </Command.Item>
              </Command.Group>

              <Command.Group heading="Theme" className={groupClass}>
                {themeItems.map((t) => (
                  <Command.Item key={t.value} value={`theme ${t.label} appearance mode`} onSelect={() => { setTheme(t.value); setOpen(false); }} className={itemClass}>
                    <t.icon className="text-fg-subtle" aria-hidden />
                    <span className="flex-1">{t.label}</span>
                    {theme === t.value && <span className="text-xs text-fg-muted">Current</span>}
                  </Command.Item>
                ))}
              </Command.Group>
            </Command.List>
            <div className="hidden items-center gap-3 border-t border-line px-3 py-2 text-xs text-fg-muted md:flex">
              <span className="flex items-center gap-1"><Kbd>↑</Kbd><Kbd>↓</Kbd> move</span>
              <span className="flex items-center gap-1"><Kbd><CornerDownLeft className="size-3" aria-hidden /></Kbd> open</span>
              <span className="ml-auto flex items-center gap-1"><Kbd>Ctrl</Kbd><Kbd>K</Kbd> anywhere</span>
            </div>
          </Command>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
