/**
 * Load every row of a list endpoint that pages with `limit` / `offset` and answers a bare array (or `{ items }`).
 * Lists used to show only the first page (the API's default limit), so a table silently stopped at 50 rows.
 * `max` stops a runaway loop; the caller can tell the user when it was reached (`capped`).
 */
export async function fetchAllPages<T>(
  get: (params: Record<string, unknown>) => Promise<{ data: unknown }>,
  params: Record<string, unknown> = {},
  pageSize = 200,
  max = 10000,
): Promise<{ rows: T[]; capped: boolean }> {
  const rows: T[] = [];
  const seen = new Set<unknown>();
  for (let offset = 0; offset < max; offset += pageSize) {
    const res = await get({ ...params, limit: pageSize, offset });
    const d = res.data as { items?: T[] } | T[];
    const page: T[] = Array.isArray(d) ? d : (d?.items ?? []);
    // An endpoint that ignores `offset` answers the same page again: stop instead of repeating rows.
    const firstId = (page[0] as { id?: unknown } | undefined)?.id;
    if (offset > 0 && firstId !== undefined && seen.has(firstId)) return { rows, capped: true };
    for (const r of page) {
      const id = (r as { id?: unknown }).id;
      if (id !== undefined) seen.add(id);
    }
    rows.push(...page);
    if (page.length < pageSize) return { rows, capped: false };
  }
  return { rows, capped: true };
}

/**
 * A `get` for `fetchAllPages` over plain `fetch` with a bearer token, for pages that do not use the axios client.
 * A failed request answers an empty page, as those pages did before.
 */
export function bearerGet(url: string, token: string) {
  return async (params: Record<string, unknown>): Promise<{ data: unknown }> => {
    const qs = new URLSearchParams();
    for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null) qs.set(k, String(v));
    const r = await fetch(`${url}?${qs}`, { headers: { Authorization: `Bearer ${token}` } });
    return { data: r.ok ? await r.json() : [] };
  };
}
