// Shared helpers for the unit serial registry (serial search, the API's error text, copy and the unit label).

/** The API's `{detail}` error as a message (a fallback when there is none). */
export function apiErrorMessage(err: unknown, fallback = "Something went wrong. Please try again."): string {
  const data = (err as { response?: { data?: { detail?: unknown } } })?.response?.data;
  return typeof data?.detail === "string" && data.detail ? data.detail : fallback;
}

/**
 * A typed or scanned unit serial as the ERP compares it: upper case, no dashes or spaces, I and L read
 * as 1, O as 0. The server does the same (and checks a generated serial's check character).
 */
export function normalizeSerial(raw: string): string {
  return raw.toUpperCase().replace(/[-\s]/g, "").replace(/[IL]/g, "1").replace(/O/g, "0");
}

/** Copies a value; true when it reached the clipboard. */
export async function copyText(value: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(value);
    return true;
  } catch { return false; /* clipboard blocked: the value is on screen to copy by hand */ }
}
