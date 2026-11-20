"use client";

import { useCallback, useSyncExternalStore } from "react";

/** True while the media query matches. False on the server and where matchMedia is missing. */
export function useMediaQuery(query: string): boolean {
  const subscribe = useCallback((onChange: () => void) => {
    if (typeof window === "undefined" || !window.matchMedia) return () => {};
    const mq = window.matchMedia(query);
    mq.addEventListener?.("change", onChange);
    return () => mq.removeEventListener?.("change", onChange);
  }, [query]);
  return useSyncExternalStore(
    subscribe,
    () => typeof window !== "undefined" && !!window.matchMedia?.(query).matches,
    () => false,
  );
}

/** Phones: below the md breakpoint (768 px), where the shell shows the BottomNav. */
export const PHONE_QUERY = "(max-width: 767.98px)";

export function useIsPhone(): boolean {
  return useMediaQuery(PHONE_QUERY);
}
