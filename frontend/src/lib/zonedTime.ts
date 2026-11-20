/**
 * Wall-clock times in a named IANA time zone (the company's `timezone` setting), independent of the
 * browser's own zone. A `<input type="datetime-local">` holds a wall-clock "YYYY-MM-DDTHH:mm" with no
 * zone; these helpers read and write it in the company zone, so a reminder for "09:00" means 09:00 company
 * time whoever opens the card and wherever they are.
 */

function validZone(tz: string | null | undefined): string | null {
  if (!tz) return null;
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: tz });
    return tz;
  } catch {
    return null;
  }
}

/** The browser's zone (used only when a card carries no company zone). */
export function browserZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

/** The zone a card works in: the company zone it carries, else the browser's. */
export function cardZone(tz: string | null | undefined): string {
  return validZone(tz) ?? browserZone();
}

type Parts = { year: number; month: number; day: number; hour: number; minute: number; second: number };

function partsIn(ms: number, tz: string): Parts {
  const fmt = new Intl.DateTimeFormat("en-US", {
    timeZone: tz, hourCycle: "h23", year: "numeric", month: "2-digit", day: "2-digit",
    hour: "2-digit", minute: "2-digit", second: "2-digit",
  });
  const get = (t: string) => Number(fmt.formatToParts(new Date(ms)).find((p) => p.type === t)?.value ?? 0);
  return { year: get("year"), month: get("month"), day: get("day"), hour: get("hour") % 24, minute: get("minute"), second: get("second") };
}

/** Minutes the zone is ahead of UTC at the instant `ms` (Asia/Novosibirsk: 420). */
export function zoneOffsetMinutes(ms: number, tz: string): number {
  const p = partsIn(ms, tz);
  const asUtc = Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second);
  return Math.round((asUtc - Math.floor(ms / 1000) * 1000) / 60000);
}

const pad = (n: number) => String(n).padStart(2, "0");

/** An ISO instant as the "YYYY-MM-DDTHH:mm" wall clock of `tz` (for a datetime-local input). */
export function toZonedInput(iso: string, tz: string): string {
  const ms = new Date(iso).getTime();
  if (Number.isNaN(ms)) return "";
  const p = partsIn(ms, tz);
  return `${p.year}-${pad(p.month)}-${pad(p.day)}T${pad(p.hour)}:${pad(p.minute)}`;
}

/** A "YYYY-MM-DDTHH:mm" wall clock in `tz` as a UTC ISO instant ("" when it can't be read). */
export function fromZonedInput(local: string, tz: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(local);
  if (!m) return "";
  const wall = Date.UTC(+m[1], +m[2] - 1, +m[3], +m[4], +m[5], +(m[6] ?? 0));
  // the offset at the guess, then again at the result (a guess across a DST change is an hour off)
  let ms = wall - zoneOffsetMinutes(wall, tz) * 60000;
  const again = wall - zoneOffsetMinutes(ms, tz) * 60000;
  if (again !== ms) ms = again;
  return new Date(ms).toISOString();
}

/** "Asia/Novosibirsk (GMT+7)": the zone and its offset at `iso` (now when not given). */
export function zoneLabel(tz: string, iso?: string): string {
  const ms = iso ? new Date(iso).getTime() : Date.now();
  const off = zoneOffsetMinutes(Number.isNaN(ms) ? Date.now() : ms, tz);
  const sign = off < 0 ? "-" : "+";
  const h = Math.floor(Math.abs(off) / 60);
  const min = Math.abs(off) % 60;
  return `${tz} (GMT${sign}${h}${min ? `:${pad(min)}` : ""})`;
}
