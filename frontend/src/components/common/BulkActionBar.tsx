"use client";

import { useEffect, useRef, useState } from "react";
import { ChevronDown, Loader2, X } from "lucide-react";
import { Button, IconButton, Input } from "@/components/ui";
import { cn } from "@/lib/utils";

export type BulkOption = { value: string; label: string };

export type BulkAction = {
  key: string;
  label: string;
  icon?: React.ElementType;
  /** Destructive: asks "<confirm>" with the count before running. */
  confirm?: (count: number) => string;
  /** A small menu of choices (e.g. statuses) instead of a plain button. */
  options?: BulkOption[];
  /** A searchable picker (e.g. people): returns options for the typed text. */
  search?: (q: string) => Promise<BulkOption[]>;
  searchPlaceholder?: string;
  /** Extra first entry in a search picker, e.g. "Unassign". */
  clearOption?: BulkOption;
};

type Props = {
  /** Ticked items, or the whole match when `allMatching` is on. */
  count: number;
  /** How many match the current filters on the server (to offer "select all matching"). */
  matchingTotal?: number;
  allMatching: boolean;
  onSelectAllMatching?: () => void;
  onClear: () => void;
  actions: BulkAction[];
  onAction: (key: string, value?: string | null) => Promise<void> | void;
  busy: boolean;
  /** Short outcome of the last action ("37 deleted"). */
  result?: string | null;
  dark?: boolean;
};

/**
 * The bar that replaces a list's header while items are ticked: the count, "select all matching",
 * the actions, and a confirmation for destructive ones. Esc clears the selection.
 */
// Phones: docked to the bottom of the screen, above the BottomNav, opaque.
const phoneDock = "max-md:fixed max-md:inset-x-2 max-md:bottom-[calc(var(--shell-bottom)+0.5rem)] max-md:z-40 " +
  "max-md:border-line max-md:bg-surface-raised max-md:text-sm max-md:shadow-pop";

export default function BulkActionBar({
  count, matchingTotal, allMatching, onSelectAllMatching, onClear, actions, onAction, busy, result, dark,
}: Props) {
  const [confirming, setConfirming] = useState<BulkAction | null>(null);
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [found, setFound] = useState<BulkOption[]>([]);
  const barRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (menuFor || confirming) { setMenuFor(null); setConfirming(null); }
      else onClear();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menuFor, confirming, onClear]);

  // Close an open menu on a click elsewhere.
  useEffect(() => {
    if (!menuFor) return;
    const onDown = (e: MouseEvent) => { if (!barRef.current?.contains(e.target as Node)) setMenuFor(null); };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [menuFor]);

  // Phones: the bar floats above the BottomNav; its height is published as --bulk-bar-h so a list
  // can leave room for it at the end (padding-bottom: var(--bulk-bar-h, 0px)).
  useEffect(() => {
    const el = barRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const root = document.documentElement;
    const ro = new ResizeObserver(() => {
      const phone = window.matchMedia?.("(max-width: 767.98px)").matches;
      root.style.setProperty("--bulk-bar-h", phone ? `${el.offsetHeight + 16}px` : "0px");
    });
    ro.observe(el);
    return () => { ro.disconnect(); root.style.removeProperty("--bulk-bar-h"); };
  }, [confirming]);

  // Searchable pickers: look up as the user types.
  const searching = actions.find((a) => a.key === menuFor && a.search);
  useEffect(() => {
    if (!searching?.search) return;
    let alive = true;
    const t = setTimeout(() => {
      searching.search!(query).then((r) => { if (alive) setFound(r); }).catch(() => { if (alive) setFound([]); });
    }, 200);
    return () => { alive = false; clearTimeout(t); };
  }, [searching, query]);

  const run = async (a: BulkAction, value?: string | null) => {
    setMenuFor(null);
    setConfirming(null);
    await onAction(a.key, value);
  };

  // `dark` is kept for callers; the bar now follows the theme tokens in both modes.
  void dark;
  const btn = cn(
    "inline-flex min-h-11 items-center gap-1 rounded-lg px-2 font-medium transition-colors disabled:opacity-40 md:min-h-7",
    "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-[rgb(var(--focus)/0.7)]",
  );

  if (confirming) {
    return (
      <div ref={barRef} role="alertdialog" aria-label="Confirm bulk action"
        className={cn("flex flex-wrap items-center gap-2 rounded-lg border border-brand/20 bg-brand/[0.05] px-3 py-2 text-xs text-fg", phoneDock)}>
        <span className="min-w-[160px] flex-1 font-medium">{confirming.confirm!(count)}</span>
        <Button size="sm" variant="danger" loading={busy} onClick={() => run(confirming)}>{confirming.label}</Button>
        <Button size="sm" variant="ghost" onClick={() => setConfirming(null)}>Cancel</Button>
      </div>
    );
  }

  return (
    <div ref={barRef} className={cn("rounded-lg border border-brand/20 bg-brand/[0.05] px-2 py-1.5 text-xs text-fg", phoneDock)}>
      <div className="flex flex-wrap items-center gap-1">
        <IconButton size="sm" icon={X} label="Clear selection (Esc)" onClick={onClear} />
        <span className="mr-1 font-semibold tabular">{count} selected</span>
        {actions.map((a) => {
          const Icon = a.icon;
          const hasMenu = !!(a.options || a.search);
          return (
            <div key={a.key} className="relative">
              <button type="button" disabled={busy} aria-haspopup={hasMenu ? "menu" : undefined}
                aria-expanded={hasMenu ? menuFor === a.key : undefined}
                onClick={() => hasMenu ? (setQuery(""), setFound([]), setMenuFor(menuFor === a.key ? null : a.key))
                  : a.confirm ? setConfirming(a) : run(a)}
                className={cn(btn, a.confirm ? "text-tone-red hover:bg-tone-red-soft" : "text-fg hover:bg-surface")}>
                {Icon && <Icon className="size-3.5" aria-hidden />}
                {a.label}
                {hasMenu && <ChevronDown className="size-3" aria-hidden />}
              </button>
              {menuFor === a.key && (
                <div role="menu" className={cn("absolute left-0 top-full z-30 mt-1 w-56 rounded-lg border border-line bg-surface-raised p-1 text-fg shadow-pop",
                  // phones: a full-width menu right above the docked bar
                  "max-md:fixed max-md:inset-x-2 max-md:bottom-[calc(var(--shell-bottom)+var(--bulk-bar-h,4rem)+0.25rem)] max-md:top-auto max-md:mt-0 max-md:max-h-[50dvh] max-md:w-auto max-md:overflow-y-auto")}>
                  {a.search && (
                    <Input autoFocus autoComplete="off" value={query} onChange={(e) => setQuery(e.target.value)}
                      placeholder={a.searchPlaceholder ?? "Search"} aria-label={a.searchPlaceholder ?? "Search"}
                      className="mb-1" />
                  )}
                  {[...(a.clearOption ? [a.clearOption] : []), ...(a.options ?? found)].map((o) => (
                    <button key={o.value || "clear"} type="button" role="menuitem" onClick={() => run(a, o.value || null)}
                      className="flex min-h-11 w-full items-center rounded-md px-2 text-left hover:bg-surface-2 md:min-h-8">{o.label}</button>
                  ))}
                  {a.search && query && !found.length && <p className="px-2 py-1.5 text-fg-muted">No one found</p>}
                </div>
              )}
            </div>
          );
        })}
        {busy && <Loader2 className="ml-1 size-3.5 animate-spin text-fg-muted" aria-label="Working" />}
      </div>
      {(onSelectAllMatching && matchingTotal && matchingTotal > count && !allMatching) ? (
        <button type="button" onClick={onSelectAllMatching} className="ml-1 mt-1 font-medium text-brand-ink hover:underline">
          Select all {matchingTotal} matching this view
        </button>
      ) : allMatching ? (
        <p className="ml-1 mt-1 text-fg-muted">All {count} matching this view are selected.</p>
      ) : null}
      {result && <p role="status" className="ml-1 mt-1 text-fg-muted">{result}</p>}
    </div>
  );
}
