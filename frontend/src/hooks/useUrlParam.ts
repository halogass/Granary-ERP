"use client";

import { useCallback, useRef } from "react";
import { useSearchParams } from "next/navigation";

/**
 * One query parameter as state, for multi-pane screens on phones: opening a pane (a thread, a chat,
 * a compose sheet) adds `?key=value` as a new history entry, so the phone Back button closes it.
 * Changing the value while open replaces the entry; clearing it goes back when we pushed it, or
 * drops the parameter in place when the page was opened on it (a deep link).
 * Uses the native history API (Next 14.1+ syncs it with useSearchParams), so no server round trip.
 */
export function useUrlParam(key: string): [string | null, (next: string | null) => void] {
  const params = useSearchParams();
  const value = params?.get(key) ?? null;
  const pushed = useRef(false);

  const set = useCallback((next: string | null) => {
    if (typeof window === "undefined") return;
    const url = new URL(window.location.href);
    const cur = url.searchParams.get(key);
    if (cur === next) return;
    if (next === null) {
      if (pushed.current) {
        pushed.current = false;
        window.history.back();
        return;
      }
      url.searchParams.delete(key);
      window.history.replaceState(null, "", url);
      return;
    }
    url.searchParams.set(key, next);
    if (cur === null) {
      window.history.pushState(null, "", url);
      pushed.current = true;
    } else {
      window.history.replaceState(null, "", url);
    }
  }, [key]);

  return [value, set];
}

/** Changes several query parameters in one step without a new history entry (null removes one). */
export function replaceUrlParams(updates: Record<string, string | null>) {
  if (typeof window === "undefined") return;
  const url = new URL(window.location.href);
  for (const [k, v] of Object.entries(updates)) {
    if (v === null) url.searchParams.delete(k); else url.searchParams.set(k, v);
  }
  window.history.replaceState(null, "", url);
}
