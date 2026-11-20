// Server-safe part of the theme switch (app/layout.tsx imports it); hooks live in theme.ts.

export type ThemePref = "system" | "light" | "dark";
export const THEME_KEY = "granary-theme";

/**
 * Runs in <head> before the first paint (app/layout.tsx), so a dark page never flashes white.
 * Keep it tiny and dependency-free; it must match applyTheme in theme.ts.
 */
export const THEME_SCRIPT = `(function(){try{var p=localStorage.getItem("${THEME_KEY}");var d=p==="dark"||((p!=="light")&&window.matchMedia("(prefers-color-scheme: dark)").matches);document.documentElement.classList.toggle("dark",d);}catch(e){}})();`;
