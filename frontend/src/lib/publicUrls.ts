/**
 * WebSocket bases for the browser. A build may bake NEXT_PUBLIC_API_URL / NEXT_PUBLIC_SIGNAL_URL in; a prebuilt
 * image leaves them unset and uses the page's own origin, which the shipped nginx config proxies
 * (/api/v1/... to the backend, /ws/signal to the signalling service).
 */

type Loc = Pick<Location, "protocol" | "host"> | null;

/** The page's origin as ws(s)://host; empty while rendering on the server (no window there). */
function originWs(loc: Loc): string {
  if (!loc) return "";
  return `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}`;
}

const pageLocation = (): Loc => (typeof window === "undefined" ? null : window.location);

/** Base for the backend's sockets, e.g. `${apiWsBase()}/api/v1/whatsapp/ws`. */
export function apiWsBase(
  env: string | undefined = process.env.NEXT_PUBLIC_API_URL,
  loc: Loc = pageLocation(),
): string {
  return env ? env.replace(/^http/, "ws").replace(/\/+$/, "") : originWs(loc);
}

/** The signalling service URL for calls and meetings. */
export function signalWsUrl(
  env: string | undefined = process.env.NEXT_PUBLIC_SIGNAL_URL,
  loc: Loc = pageLocation(),
): string {
  return env || `${originWs(loc)}/ws/signal`;
}
