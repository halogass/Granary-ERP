"use client";

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { Button, toast } from "@/components/ui";
import { extractApiError } from "@/lib/apiError";

/**
 * The Settings page's unsaved changes. Every form on an area page (the registry's settings and
 * blocks with their own API, like branding or the AI settings) registers a draft source; the save
 * bar at the bottom sums them up and saves or discards them together. Outside the provider a block
 * keeps its own Save button (`inSaveBar` is false).
 */
export interface DraftSource {
  /** Number of changed fields; 0 means clean. */
  count: number;
  /** Persist the changes. Throw to stop the save bar (it shows the error). */
  save: () => Promise<void>;
  /** Throw away local changes. */
  discard: () => void;
  /** A message when the changes cannot be saved yet (invalid JSON, a required field). */
  validate?: () => string | null;
}

interface DraftCtx {
  set: (id: string, count: number) => void;
  remove: (id: string) => void;
  refs: Map<string, React.MutableRefObject<DraftSource>>;
}

const Ctx = createContext<DraftCtx | null>(null);
const SummaryCtx = createContext<{ total: number; saving: boolean; saveAll: () => Promise<boolean>; discardAll: () => void } | null>(null);

export function SettingsDraftProvider({ children }: { children: React.ReactNode }) {
  const [counts, setCounts] = useState<Record<string, number>>({});
  const [saving, setSaving] = useState(false);
  const refs = useRef(new Map<string, React.MutableRefObject<DraftSource>>()).current;

  const set = useCallback((id: string, count: number) => {
    setCounts((c) => (c[id] === count ? c : { ...c, [id]: count }));
  }, []);
  const remove = useCallback((id: string) => {
    refs.delete(id);
    setCounts((c) => {
      if (!(id in c)) return c;
      const next = { ...c };
      delete next[id];
      return next;
    });
  }, [refs]);

  const total = Object.values(counts).reduce((a, b) => a + b, 0);

  const saveAll = useCallback(async () => {
    const dirty = Array.from(refs.entries()).filter(([id]) => (counts[id] ?? 0) > 0);
    for (const [, r] of dirty) {
      const problem = r.current.validate?.();
      if (problem) {
        toast.error(problem);
        return false;
      }
    }
    setSaving(true);
    try {
      for (const [, r] of dirty) await r.current.save();
      toast.success("Settings saved");
      return true;
    } catch (e) {
      toast.error(extractApiError(e, "Could not save the settings."));
      return false;
    } finally {
      setSaving(false);
    }
  }, [refs, counts]);

  const discardAll = useCallback(() => {
    refs.forEach((r) => r.current.discard());
  }, [refs]);

  const ctx = useMemo(() => ({ set, remove, refs }), [set, remove, refs]);
  const summary = useMemo(() => ({ total, saving, saveAll, discardAll }), [total, saving, saveAll, discardAll]);
  return (
    <Ctx.Provider value={ctx}>
      <SummaryCtx.Provider value={summary}>{children}</SummaryCtx.Provider>
    </Ctx.Provider>
  );
}

/**
 * Register a form's unsaved changes with the save bar. Pass fresh closures every render; the bar
 * always calls the latest. Returns whether a save bar is there (else render your own Save button).
 */
export function useDraftSource(id: string, source: DraftSource): { inSaveBar: boolean } {
  const ctx = useContext(Ctx);
  const ref = useRef(source);
  ref.current = source;
  useEffect(() => {
    if (!ctx) return;
    ctx.refs.set(id, ref);
    return () => ctx.remove(id);
  }, [ctx, id]);
  useEffect(() => {
    ctx?.set(id, source.count);
  }, [ctx, id, source.count]);
  return { inSaveBar: !!ctx };
}

/** Total unsaved changes and the save/discard actions (for the save bar and the leave guard). */
export function useDraftSummary() {
  return useContext(SummaryCtx) ?? { total: 0, saving: false, saveAll: async () => true, discardAll: () => {} };
}

export function changesLabel(n: number) {
  return `${n} unsaved change${n === 1 ? "" : "s"}`;
}

/** The bar at the bottom: "N unsaved changes in <area>: Discard / Save". Hidden when clean. */
export function SaveBar({ areaLabel }: { areaLabel: string }) {
  const { total, saving, saveAll, discardAll } = useDraftSummary();
  if (total === 0) return null;
  return (
    <div role="region" aria-label="Unsaved changes"
      className="sticky bottom-[calc(var(--shell-bottom)-6rem)] md:bottom-[-1.5rem] z-20 -mx-4 mt-6 flex flex-wrap items-center gap-2 border-t border-line bg-surface-raised px-4 py-3 shadow-pop md:mx-0 md:rounded-lg md:border">
      <span className="text-sm font-medium text-fg" aria-live="polite">
        {changesLabel(total)} in {areaLabel}
      </span>
      <span className="ml-auto" />
      <Button variant="ghost" onClick={discardAll} disabled={saving}>Discard</Button>
      <Button variant="primary" onClick={() => { void saveAll(); }} loading={saving}>Save</Button>
    </div>
  );
}

/**
 * Asks before leaving with unsaved changes: closing or reloading the tab, and clicking a link to
 * another page or settings area (in-page #section links pass).
 */
export function useLeaveGuard(dirty: boolean, message = "You have unsaved changes. Leave without saving?") {
  useEffect(() => {
    if (!dirty) return;
    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = message;
      return message;
    };
    const onClick = (e: MouseEvent) => {
      if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
      const a = (e.target as Element | null)?.closest?.("a[href]") as HTMLAnchorElement | null;
      if (!a || a.target === "_blank" || a.hasAttribute("download")) return;
      const url = new URL(a.href, window.location.href);
      if (url.origin !== window.location.origin) return;
      if (url.pathname === window.location.pathname && url.search === window.location.search) return;
      if (!window.confirm(message)) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    document.addEventListener("click", onClick, true);
    return () => {
      window.removeEventListener("beforeunload", onBeforeUnload);
      document.removeEventListener("click", onClick, true);
    };
  }, [dirty, message]);
}
