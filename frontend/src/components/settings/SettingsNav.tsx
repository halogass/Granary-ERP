"use client";

import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Search } from "lucide-react";
import { Input, Select } from "@/components/ui";
import { cn } from "@/lib/utils";
import { SETTINGS_AREAS, searchSettings, settingsHref, type SettingsArea, type SettingsSearchHit } from "@/lib/settings-registry";
import { useDraftSummary } from "./SettingsDraft";

/** Link for a search hit: settings also carry ?focus=<key> so the row is highlighted. */
export function hitHref(h: SettingsSearchHit) {
  if (h.kind === "setting" && h.key) {
    return `/settings?area=${encodeURIComponent(h.areaId)}&focus=${encodeURIComponent(h.key)}${h.sectionId ? `#${encodeURIComponent(h.sectionId)}` : ""}`;
  }
  return h.href;
}

function Highlight({ text, words }: { text: string; words: string[] }) {
  if (!words.length) return <>{text}</>;
  const re = new RegExp(`(${words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "ig");
  return <>{text.split(re).map((p, i) => (i % 2 ? <mark key={i} className="rounded-sm bg-tone-orange-soft px-0.5 text-fg">{p}</mark> : p))}</>;
}

/**
 * The settings sidebar: search on top, then the ten areas; the open one expands into its sections.
 * On phones it is a search box and an area picker.
 */
export function SettingsNav({ area, activeSection }: { area: SettingsArea; activeSection?: string }) {
  const router = useRouter();
  const { total } = useDraftSummary();
  const [q, setQ] = useState("");
  const hits = useMemo(() => searchSettings(q, 40), [q]);
  const words = q.trim().split(/\s+/).filter(Boolean);

  // A new area clears the search, so the sidebar shows where you landed.
  useEffect(() => { setQ(""); }, [area.id]);

  const go = (href: string) => {
    if (total > 0 && !window.confirm("You have unsaved changes. Leave without saving?")) return;
    router.push(href);
  };

  return (
    <nav aria-label="Settings" className="space-y-3">
      <Input type="search" autoComplete="off" leading={Search} value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search settings"
        aria-label="Search settings" aria-controls="settings-search-results"
        onKeyDown={(e) => { if (e.key === "Enter" && hits[0]) { e.preventDefault(); go(hitHref(hits[0])); } }} />

      {q.trim() ? (
        <div id="settings-search-results" role="region" aria-label="Search results" aria-live="polite">
          <p className="mb-1 px-2 text-xs text-fg-muted">
            {hits.length ? `${hits.length}${hits.length === 40 ? "+" : ""} result${hits.length === 1 ? "" : "s"}` : "No setting matches."}
          </p>
          <ul className="space-y-0.5">
            {hits.map((h) => (
              <li key={h.id}>
                <Link href={hitHref(h)} onClick={() => setQ("")}
                  className="block min-h-11 rounded-lg px-2 py-1.5 hover:bg-surface-2 md:min-h-0 focus-visible:outline focus-visible:outline-2 focus-visible:outline-[rgb(var(--focus)/0.7)]">
                  <span className="block text-sm text-fg"><Highlight text={h.label} words={words} /></span>
                  <span className="block truncate text-xs text-fg-muted">{h.trail}</span>
                  {h.key && <span className="block truncate font-mono text-[11px] text-fg-subtle"><Highlight text={h.key} words={words} /></span>}
                </Link>
              </li>
            ))}
          </ul>
        </div>
      ) : (
        <>
          {/* Phones and tablets: pick the area, then jump to one of its sections */}
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:hidden">
            <Select aria-label="Settings area" value={area.id} onChange={(e) => go(settingsHref(e.target.value))}
              options={SETTINGS_AREAS.map((a) => ({ value: a.id, label: a.label }))} />
            {area.sections.length > 1 && (
              <Select aria-label="Jump to section" value={activeSection ?? area.sections[0].id}
                onChange={(e) => { window.location.hash = e.target.value; }}
                options={area.sections.map((s) => ({ value: s.id, label: s.label }))} />
            )}
          </div>
          <ul className="hidden space-y-0.5 lg:block">
            {SETTINGS_AREAS.map((a) => {
              const open = a.id === area.id;
              return (
                <li key={a.id}>
                  <Link href={settingsHref(a.id)} aria-current={open ? "page" : undefined}
                    className={cn("relative flex min-h-9 items-center rounded-lg px-2 text-sm transition-colors",
                      open ? "bg-surface-2 font-semibold text-fg before:absolute before:inset-y-1.5 before:left-0 before:w-0.5 before:rounded-full before:bg-brand"
                        : "text-fg-muted hover:bg-surface-2 hover:text-fg")}>
                    {a.label}
                  </Link>
                  {open && (
                    <ul className="mb-1 mt-0.5 space-y-0.5">
                      {a.sections.map((s) => (
                        <li key={s.id}>
                          <a href={`#${s.id}`} aria-current={activeSection === s.id ? "location" : undefined}
                            className={cn("block rounded-lg py-1 pl-5 pr-2 text-[13px] transition-colors hover:bg-surface-2 hover:text-fg",
                              activeSection === s.id ? "font-medium text-brand-ink" : "text-fg-muted")}>
                            {s.label}
                          </a>
                        </li>
                      ))}
                    </ul>
                  )}
                </li>
              );
            })}
          </ul>
        </>
      )}
    </nav>
  );
}

/** "On this page" on the right: the area's sections, the visible one marked. */
export function OnThisPage({ area, activeSection }: { area: SettingsArea; activeSection?: string }) {
  return (
    <nav aria-label="On this page" className="space-y-1">
      <p className="px-2 text-[11px] font-medium uppercase tracking-wide text-fg-muted">On this page</p>
      <ul className="space-y-0.5">
        {area.sections.map((s) => (
          <li key={s.id}>
            <a href={`#${s.id}`} aria-current={activeSection === s.id ? "location" : undefined}
              className={cn("block border-l-2 py-1 pl-2.5 text-[13px] transition-colors hover:text-fg",
                activeSection === s.id ? "border-brand font-medium text-fg" : "border-transparent text-fg-muted")}>
              {s.label}
            </a>
          </li>
        ))}
      </ul>
    </nav>
  );
}
