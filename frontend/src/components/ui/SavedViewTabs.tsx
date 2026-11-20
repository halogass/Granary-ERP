"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSession } from "next-auth/react";
import { Plus, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { CountBadge } from "./Pill";
import { noScrollbar } from "./FilterChips";
import { useScrollFade } from "./useScrollFade";

export interface SavedView<F> {
  id: string;
  label: string;
  filters: F;
  /** Built-in views come from the page and cannot be removed. */
  builtin?: boolean;
}

interface Stored<F> { views: SavedView<F>[]; active?: string }

export function savedViewsKey(userKey: string, pageKey: string) {
  return `granary:views:${userKey}:${pageKey}`;
}

function read<F>(key: string): Stored<F> {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return { views: [] };
    const parsed = JSON.parse(raw) as Stored<F>;
    return { views: Array.isArray(parsed.views) ? parsed.views : [], active: parsed.active };
  } catch {
    return { views: [] };
  }
}

function write<F>(key: string, value: Stored<F>) {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* storage full or blocked: views live for this visit */ }
}

/**
 * A list page's views: the page's built-ins plus the ones this user saved (localStorage, per
 * user and page). The last chosen view is remembered too.
 */
export function useSavedViews<F>(pageKey: string, builtins: SavedView<F>[]) {
  const { data: session } = useSession();
  const u = session?.user as { id?: string; email?: string | null } | undefined;
  const userKey = u?.id || u?.email || "me";
  const key = savedViewsKey(userKey, pageKey);

  const [custom, setCustom] = useState<SavedView<F>[]>([]);
  const [activeId, setActiveId] = useState<string>(builtins[0]?.id ?? "");
  const loaded = useRef<string | null>(null);

  useEffect(() => {
    const s = read<F>(key);
    setCustom(s.views);
    const all = [...builtins, ...s.views];
    if (s.active && all.some((v) => v.id === s.active)) setActiveId(s.active);
    loaded.current = key;
    // builtins are static per page
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const persist = useCallback((views: SavedView<F>[], active: string) => {
    if (loaded.current === key) write(key, { views, active });
  }, [key]);

  const views = useMemo(() => [...builtins.map((b) => ({ ...b, builtin: true })), ...custom], [builtins, custom]);
  const active = views.find((v) => v.id === activeId) ?? views[0];

  const select = useCallback((id: string) => {
    setActiveId(id);
    persist(custom, id);
  }, [custom, persist]);

  const save = useCallback((label: string, filters: F) => {
    const view: SavedView<F> = { id: `v${Date.now().toString(36)}`, label: label.trim(), filters };
    const next = [...custom, view];
    setCustom(next);
    setActiveId(view.id);
    persist(next, view.id);
    return view;
  }, [custom, persist]);

  const remove = useCallback((id: string) => {
    const next = custom.filter((v) => v.id !== id);
    const nextActive = activeId === id ? (builtins[0]?.id ?? "") : activeId;
    setCustom(next);
    setActiveId(nextActive);
    persist(next, nextActive);
  }, [custom, activeId, builtins, persist]);

  return { views, active, activeId: active?.id ?? "", select, save, remove };
}

export type SavedViewsState<F> = ReturnType<typeof useSavedViews<F>>;

export interface SavedViewTabsProps<F> {
  state: SavedViewsState<F>;
  /** The filters in effect now; "Save view" stores these. Omit to hide "Save view". */
  currentFilters?: F;
  /** Called with the chosen view; apply its filters. */
  onSelect?: (view: SavedView<F>) => void;
  /** Optional count per view id (shown muted; red for needs-you views via `alertIds`). */
  counts?: Record<string, number | undefined>;
  alertIds?: string[];
  className?: string;
}

/** Saved views as tabs under the page header: All open / Mine / Overdue / ... / + Save view. */
export function SavedViewTabs<F>({ state, currentFilters, onSelect, counts, alertIds, className }: SavedViewTabsProps<F>) {
  const [naming, setNaming] = useState(false);
  const [name, setName] = useState("");
  const fade = useScrollFade<HTMLDivElement>(state.activeId);

  function choose(v: SavedView<F>) {
    state.select(v.id);
    onSelect?.(v);
  }

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim() || currentFilters === undefined) return;
    const v = state.save(name, currentFilters);
    onSelect?.(v);
    setName("");
    setNaming(false);
  }

  return (
    <div ref={fade.ref} onScroll={fade.onScroll} style={fade.style}
      className={cn("flex items-center gap-1 overflow-x-auto bg-surface px-4 shadow-[inset_0_-1px_0_rgb(var(--line))]", noScrollbar, className)}>
      <div role="tablist" aria-label="Saved views" className="flex items-center gap-1">
        {state.views.map((v) => {
          const on = v.id === state.activeId;
          return (
            <div key={v.id} className="group relative flex shrink-0 items-center">
              <button type="button" role="tab" aria-selected={on} onClick={() => choose(v)}
                className={cn("inline-flex min-h-11 items-center gap-1.5 whitespace-nowrap border-b-2 px-3 text-sm transition-colors md:min-h-9",
                  "focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)]",
                  on ? "border-brand font-semibold text-fg" : "border-transparent text-fg-muted hover:text-fg",
                  !v.builtin && "pr-11 md:pr-7")}>
                {v.label}
                {counts?.[v.id] != null && (
                  <CountBadge count={counts[v.id]} tone={alertIds?.includes(v.id) ? "brand" : "muted"} />
                )}
              </button>
              {!v.builtin && (
                <button type="button" onClick={() => state.remove(v.id)} aria-label={`Remove view ${v.label}`}
                  className="absolute right-0 flex size-11 items-center justify-center rounded text-fg-subtle hover:bg-surface-2 hover:text-fg md:right-1 md:size-5 [@media(hover:hover)]:opacity-60 [@media(hover:hover)]:hover:opacity-100 [@media(hover:hover)]:focus-visible:opacity-100 [@media(hover:hover)]:group-hover:opacity-100">
                  <X className="size-3" aria-hidden />
                </button>
              )}
            </div>
          );
        })}
      </div>
      {currentFilters !== undefined && (naming ? (
        <form onSubmit={submit} className="flex shrink-0 items-center gap-1 py-1">
          <input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="View name" aria-label="View name"
            onKeyDown={(e) => { if (e.key === "Escape") { setNaming(false); setName(""); } }}
            className="h-11 w-36 rounded-lg border border-line-strong bg-surface px-2 text-base text-fg md:h-8 md:text-sm focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/20" />
          <button type="submit" disabled={!name.trim()} className="h-11 rounded-lg bg-brand px-3 text-[13px] font-medium text-brand-on disabled:opacity-50 md:h-8 md:px-2.5">Save</button>
          <button type="button" onClick={() => { setNaming(false); setName(""); }} className="h-11 rounded-lg px-2 text-[13px] text-fg-muted hover:bg-surface-2 md:h-8">Cancel</button>
        </form>
      ) : (
        <button type="button" onClick={() => setNaming(true)}
          className="inline-flex min-h-11 shrink-0 items-center gap-1 whitespace-nowrap px-3 text-sm text-fg-muted hover:text-fg md:min-h-9">
          <Plus className="size-3.5" aria-hidden /> Save view
        </button>
      ))}
    </div>
  );
}
