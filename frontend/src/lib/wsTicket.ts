import { authApi } from "@/lib/api";

/**
 * Opens a WebSocket at `url` with a single-use ticket from the API (stage 8): the access token never goes in
 * the URL, where proxies and access logs would keep it. `setup` gets the socket as soon as it exists (attach
 * the handlers there). Returns the cleanup for a useEffect: it closes the socket, or stops it from opening
 * when the ticket is still on its way.
 */
export function openTicketedSocket(url: string, setup: (ws: WebSocket) => void): () => void {
  let stopped = false;
  let ws: WebSocket | null = null;
  authApi.wsTicket()
    .then((r) => {
      if (stopped) return;
      const sep = url.includes("?") ? "&" : "?";
      ws = new WebSocket(`${url}${sep}ticket=${encodeURIComponent(r.data.ticket)}`);
      setup(ws);
    })
    .catch(() => { /* no ticket (signed out, server down): no live updates, the page still loads */ });
  return () => {
    stopped = true;
    ws?.close();
  };
}
