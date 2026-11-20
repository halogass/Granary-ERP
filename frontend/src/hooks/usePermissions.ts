"use client";

import { useCallback, useEffect, useState } from "react";
import { authApi } from "@/lib/api";

interface Me { is_superadmin?: boolean; is_system_admin?: boolean; permissions?: string[] }

// One /auth/me per page load, shared by every component that asks.
let cached: Promise<Me> | null = null;

function loadMe(): Promise<Me> {
  if (!cached) {
    cached = authApi.me().then((r) => r.data as Me).catch(() => {
      cached = null;          // a failed load may be retried by the next caller
      return {};
    });
  }
  return cached;
}

/**
 * The signed-in user's permissions, for hiding actions they can't take. The API checks every
 * permission itself: this only keeps buttons that would fail out of the way.
 */
export function usePermissions() {
  const [me, setMe] = useState<Me | null>(null);
  useEffect(() => {
    let alive = true;
    loadMe().then((m) => { if (alive) setMe(m); });
    return () => { alive = false; };
  }, []);
  const isAdmin = !!(me?.is_superadmin || me?.is_system_admin);
  // Stable between renders (they change only when `me` loads): pages put `can` in useCallback/useEffect
  // deps, and a new function every render reloaded such a page in a loop (21 Oct: the proposal page hit
  // nginx's rate limit and got 503s).
  const can = useCallback((code: string) => isAdmin || !!me?.permissions?.includes(code), [isAdmin, me]);
  /** As the API checks most codes: only a superadmin is exempt (a system admin needs the code). */
  const holds = useCallback((code: string) => !!me?.is_superadmin || !!me?.permissions?.includes(code), [me]);
  return { loaded: me !== null, isAdmin, isSuperadmin: !!me?.is_superadmin, can, holds };
}
