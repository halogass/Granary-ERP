/**
 * Video-call peer naming. The signalling server only knows peer ids (`erp_xxxxxxxx` / `guest_xxxxxxxx`), so a
 * tile's name comes from the `display_name` a peer puts on its offer/answer, or, for a guest the host has just
 * admitted, from the name the waiting room recorded.
 */

/** The signalling peer id of an admitted guest (calls.rs `_guest_peer_id`): `guest_` + the waiting id's first 8 chars. */
export function guestPeerId(waitingId: string): string {
  return `guest_${waitingId.slice(0, 8)}`;
}

/** `prev` with `peerId` named `name`; the same map when the name is missing or blank. */
export function withPeerName(prev: Map<string, string>, peerId: string, name: unknown): Map<string, string> {
  if (typeof name !== "string" || !name.trim() || !peerId) return prev;
  if (prev.get(peerId) === name) return prev;
  return new Map(prev).set(peerId, name);
}
