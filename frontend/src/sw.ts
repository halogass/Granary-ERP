import { defaultCache } from "@serwist/next/worker";
import type { PrecacheEntry, SerwistGlobalConfig } from "serwist";
import { Serwist, NetworkOnly } from "serwist";

declare global {
  interface ServiceWorkerGlobalScope extends SerwistGlobalConfig {
    __SW_MANIFEST: (PrecacheEntry | string)[] | undefined;
  }
}

declare const self: ServiceWorkerGlobalScope;

const serwist = new Serwist({
  precacheEntries: self.__SW_MANIFEST,
  skipWaiting: true,
  clientsClaim: true,
  navigationPreload: true,
  runtimeCaching: [
    // Web app manifest: always fetch fresh JSON. If it falls through to the
    // default cache it can be served the HTML app-shell ("manifest syntax error").
    { matcher: ({ url }) => url.pathname === "/manifest.webmanifest", handler: new NetworkOnly() },
    // Page navigations: never cache, prevents stale pages on hard refresh.
    { matcher: ({ request }) => request.mode === "navigate", handler: new NetworkOnly() },
    // API requests: always go to the network; never cache or redirect.
    // Caching these causes Mixed Content errors when the backend returns HTTP redirects
    // (trailing-slash 307) that get exposed to the browser through the SW.
    { matcher: ({ url }) => url.pathname.startsWith("/api/"), handler: new NetworkOnly() },
    ...defaultCache,
  ],
});

serwist.addEventListeners();
