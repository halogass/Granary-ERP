"use client";

import { useCallback, useEffect, useState } from "react";
import { THEME_KEY, type ThemePref } from "./theme-script";

export { THEME_KEY, THEME_SCRIPT, type ThemePref } from "./theme-script";

export function readThemePref(): ThemePref {
  try {
    const v = localStorage.getItem(THEME_KEY);
    return v === "light" || v === "dark" ? v : "system";
  } catch {
    return "system";
  }
}

function systemDark() {
  return typeof window !== "undefined" && !!window.matchMedia?.("(prefers-color-scheme: dark)").matches;
}

export function resolveTheme(pref: ThemePref): "light" | "dark" {
  return pref === "system" ? (systemDark() ? "dark" : "light") : pref;
}

export function applyTheme(pref: ThemePref) {
  document.documentElement.classList.toggle("dark", resolveTheme(pref) === "dark");
}

const EVENT = "granary-theme-change";

/** The stored preference, the theme in effect, and a setter. Follows the OS while on "system". */
export function useTheme() {
  const [pref, setPref] = useState<ThemePref>("system");
  const [resolved, setResolved] = useState<"light" | "dark">("light");

  useEffect(() => {
    const sync = () => {
      const p = readThemePref();
      setPref(p);
      setResolved(resolveTheme(p));
      applyTheme(p);
    };
    sync();
    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    mq?.addEventListener?.("change", sync);
    window.addEventListener(EVENT, sync);
    window.addEventListener("storage", sync);
    return () => {
      mq?.removeEventListener?.("change", sync);
      window.removeEventListener(EVENT, sync);
      window.removeEventListener("storage", sync);
    };
  }, []);

  const setTheme = useCallback((next: ThemePref) => {
    try {
      if (next === "system") localStorage.removeItem(THEME_KEY);
      else localStorage.setItem(THEME_KEY, next);
    } catch { /* private mode: still applies for this page */ }
    applyTheme(next);
    setPref(next);
    setResolved(resolveTheme(next));
    window.dispatchEvent(new Event(EVENT));
  }, []);

  return { theme: pref, resolved, setTheme };
}
