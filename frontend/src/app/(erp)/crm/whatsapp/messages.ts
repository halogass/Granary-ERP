/** Pure helpers for the WhatsApp message list (kept out of page.tsx so they can be tested). */

export interface ListMessage {
  id: string;
  timestamp: number;
  fromMe: boolean;
  /** 0 pending, 1 sent, 2 delivered, 3 read, 4 played. Own messages only. */
  ack?: number | null;
}

const byTime = <T extends ListMessage>(a: T, b: T) => a.timestamp - b.timestamp;

/** "Load older": put a page of older messages in front of what is shown. Only messages older
 *  than the oldest one already on screen are taken, and nothing is shown twice. */
export function mergeOlderMessages<T extends ListMessage>(current: T[], older: T[]): T[] {
  const shown = new Set(current.map((m) => m.id));
  const oldest = current.length ? Math.min(...current.map((m) => m.timestamp)) : Infinity;
  const fresh = older.filter((m) => !shown.has(m.id) && m.timestamp <= oldest).sort(byTime);
  return [...fresh, ...current];
}

/** A refresh of the latest page: replace the newest part and keep the older pages the user
 *  already loaded. Optimistic bubbles are dropped (the refresh carries the real message). */
export function mergeLatestMessages<T extends ListMessage>(current: T[], latest: T[]): T[] {
  if (!latest.length) return current.filter((m) => !m.id.startsWith("opt-"));
  const newestPageStart = Math.min(...latest.map((m) => m.timestamp));
  const ids = new Set(latest.map((m) => m.id));
  const kept = current.filter((m) => m.timestamp < newestPageStart && !ids.has(m.id) && !m.id.startsWith("opt-"));
  return [...kept, ...[...latest].sort(byTime)];
}

/** Same list, same order and same ticks, so a background refresh can skip a re-render. */
export function sameMessages<T extends ListMessage>(a: T[], b: T[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i].id !== b[i].id || (a[i].ack ?? null) !== (b[i].ack ?? null)) return false;
  }
  return true;
}

/** Record a tick update. Acks only move forward, so a late "delivered" never undoes "read". */
export function applyAck<T extends ListMessage>(list: T[], messageId: string, ack: number): T[] {
  let changed = false;
  const next = list.map((m) => {
    if (m.id !== messageId || (m.ack ?? -1) >= ack) return m;
    changed = true;
    return { ...m, ack };
  });
  return changed ? next : list;
}

export type TickState = "none" | "pending" | "sent" | "delivered" | "read";

/** Which tick to show under a message, WhatsApp-style. */
export function tickState(m: ListMessage): TickState {
  if (!m.fromMe) return "none";
  if (m.id.startsWith("opt-")) return "pending";
  const ack = m.ack ?? 1;          // older payloads carry no ack: show a plain "sent" tick
  if (ack <= 0) return "pending";
  if (ack === 1) return "sent";
  if (ack === 2) return "delivered";
  return "read";
}

/** Whether more items exist beyond this page, from the server's `x-has-more` header. A page can
 *  come back short (the chat list drops hidden chats), so its length only decides when the
 *  header is missing (an older backend). */
export function hasMoreFromHeaders(res: { headers?: Record<string, unknown> }, count: number, limit: number): boolean {
  const more = res.headers?.["x-has-more"];
  if (more === undefined) return count >= limit;
  return more === "1";
}

// fix-walist: the chat list never looks empty because WhatsApp is slow. The server serves its last good
// list (header `x-wa-stale`, the ISO time of that copy) or a 503; the page says so and retries.

/** Auto-retry delays while the list is stale or failed; the last one repeats while the page is open. */
export const CHAT_RETRY_DELAYS_MS = [5000, 15000, 30000];

export function chatRetryDelay(attempt: number): number {
  return CHAT_RETRY_DELAYS_MS[Math.min(Math.max(attempt, 0), CHAT_RETRY_DELAYS_MS.length - 1)];
}

export const CHATS_SLOW = "WhatsApp is slow to answer; try again in a moment";

export type ChatListProblem = { kind: "stale" | "failed"; text: string; retry: boolean };

/** "HH:MM" (24 h, local time) of an ISO time, or null when it does not parse. */
export function clockTime(iso: string): string | null {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** The notice for a list the server served from its copy, or null for a fresh list. */
export function staleChatList(res: { headers?: Record<string, unknown> }): ChatListProblem | null {
  const at = res.headers?.["x-wa-stale"];
  if (typeof at !== "string" || !at) return null;
  const time = clockTime(at);
  return {
    kind: "stale",
    text: time ? `WhatsApp is slow to answer; showing the list from ${time}` : "WhatsApp is slow to answer; showing an older list",
    retry: true,
  };
}

/** The notice for a failed load. Slow or unreachable service (503, 502, 504, no answer): retried by
 *  itself; anything else (no access...) only on Retry. */
export function failedChatList(err: unknown): ChatListProblem {
  const r = (err as { response?: { status?: number; data?: { detail?: unknown } } })?.response;
  const status = r?.status;
  const detail = typeof r?.data?.detail === "string" ? r.data.detail : "";
  if (status === undefined || status === 502 || status === 503 || status === 504) {
    return { kind: "failed", text: status === 503 && detail ? detail : CHATS_SLOW, retry: true };
  }
  return { kind: "failed", text: detail || "The chat list could not be loaded", retry: false };
}
