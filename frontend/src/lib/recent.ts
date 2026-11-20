/** Recently opened records (deal, invoice, contact...), for the command palette. Per browser. */
export interface RecentRecord {
  href: string;
  title: string;
  /** What it is, from its list page ("Deals"). */
  kind: string;
  at: number;
}

const KEY = "granary:recent-records";
const MAX = 12;
const ID_SEGMENT = /^(?:[0-9a-f]{8,}|\d+|[0-9a-f-]{20,})$/i;

/** The list path of a record page (/crm/deals/<id> -> /crm/deals), or null for other pages. */
export function recordListPath(pathname: string): string | null {
  const segs = pathname.split("/").filter(Boolean);
  const idAt = segs.findIndex((s) => ID_SEGMENT.test(s));
  if (idAt < 1) return null;
  // /x/<id>/edit and /x/<id>/report count as the record itself
  return "/" + segs.slice(0, idAt).join("/");
}

export function recentRecords(): RecentRecord[] {
  try {
    const list = JSON.parse(localStorage.getItem(KEY) ?? "[]") as RecentRecord[];
    return Array.isArray(list) ? list.filter((r) => r && typeof r.href === "string") : [];
  } catch {
    return [];
  }
}

export function rememberRecord(rec: Omit<RecentRecord, "at">) {
  const base = rec.href.split("?")[0];
  const rest = recentRecords().filter((r) => r.href.split("?")[0] !== base);
  const next = [{ ...rec, at: Date.now() }, ...rest].slice(0, MAX);
  try { localStorage.setItem(KEY, JSON.stringify(next)); } catch { /* ignore */ }
}
