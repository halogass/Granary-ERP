import { crmApi, salesApi, vendorApi, hrApi, rndApi } from "@/lib/api";
import type { Option, OptionSource } from "@/components/ui/SearchSelect";

/**
 * Option sources for SearchSelect: one per entity kind.
 *
 * Two shapes, picked per endpoint:
 *
 *  - `serverSource`: the endpoint supports `?q=`, so typing filters in Postgres and the
 *                     endpoint's default `limit` never hides a record. Use this for any
 *                     list that grows without bound (orgs, contacts, vendors, employees…).
 *  - `localSource`: the caller already holds the options in state, often derived or
 *                     filtered client-side; filter them in the browser.
 */

type Row = Record<string, unknown>;

/** Endpoints return either a bare array or `{ items: [...] }`. */
function rows(data: unknown): Row[] {
  if (Array.isArray(data)) return data as Row[];
  const items = (data as { items?: unknown } | null)?.items;
  return Array.isArray(items) ? (items as Row[]) : [];
}

const str = (v: unknown): string => (typeof v === "string" ? v : v == null ? "" : String(v));

const PAGE = 50;

function serverSource(
  list: (params: Record<string, unknown>) => Promise<{ data: unknown }>,
  toOption: (r: Row) => Option,
  getOne?: (id: string) => Promise<{ data: unknown }>,
  extraParams: Record<string, unknown> = {},
): OptionSource {
  return {
    fetch: (query) =>
      list({ ...extraParams, ...(query ? { q: query } : {}), limit: PAGE })
        .then((r) => rows(r.data).map(toOption))
        .catch(() => []),
    resolve: getOne
      ? (id) =>
          getOne(id)
            .then((r) => (r.data ? toOption(r.data as Row) : null))
            .catch(() => null)
      : undefined,
  };
}

/**
 * A source over options the caller already holds in state, for lists that are derived or
 * filtered client-side (project members, unlinked employees, a flattened location tree),
 * where a server query would throw that logic away. Memoize the result on the array with
 * useMemo so the identity is stable across renders.
 */
export function localSource(options: Option[]): OptionSource {
  const match = (o: Option, q: string) =>
    o.label.toLowerCase().includes(q) || (o.sublabel ?? "").toLowerCase().includes(q);
  return {
    fetch: (query) => {
      const q = query.trim().toLowerCase();
      return Promise.resolve(q ? options.filter((o) => match(o, q)) : options);
    },
    resolve: (id) => Promise.resolve(options.find((o) => o.id === id) ?? null),
  };
}

// ─── CRM ──────────────────────────────────────────────────────────────────────

const orgOption = (r: Row): Option => ({
  id: str(r.id),
  label: str(r.name),
  sublabel: [str(r.industry), str(r.city)].filter(Boolean).join(" · ") || undefined,
});

export const orgSource: OptionSource = serverSource(
  crmApi.listOrgs, orgOption, crmApi.getOrg,
);

const contactOption = (r: Row): Option => ({
  id: str(r.id),
  label: str(r.full_name) || `${str(r.first_name)} ${str(r.last_name)}`.trim(),
  sublabel: [str(r.position), str(r.email)].filter(Boolean).join(" · ") || undefined,
  // Lets a caller back-fill the organization when the contact is picked first.
  meta: { organization_id: r.organization_id ? str(r.organization_id) : "" },
});

/**
 * Contacts, optionally narrowed to one organization. Built per-org rather than exported
 * as a constant so the caller can rebuild it when the selected org changes, memoize it
 * with useMemo on `orgId`.
 */
export function contactSource(orgId?: string): OptionSource {
  return serverSource(
    crmApi.listContacts, contactOption, crmApi.getContact,
    orgId ? { org_id: orgId } : {},
  );
}

// ─── Sales / products ─────────────────────────────────────────────────────────

const catalogOption = (r: Row): Option => ({
  id: str(r.id),
  label: str(r.name),
  sublabel: str(r.sku) || undefined,
});

/** Retired items (inactive or no longer orderable) are never offered on a new line. */
const isOrderable = (r: Row) => r.is_active !== false && r.is_orderable !== false;

/**
 * Orderable catalog items. The API already leaves retired items out unless include_retired=true;
 * the client filter keeps that true whatever the endpoint returns. `resolve` still shows the name
 * of a retired item a saved line points at.
 */
export const catalogSource: OptionSource = {
  fetch: (query) =>
    salesApi.listCatalog({ include_retired: false, ...(query ? { q: query } : {}), limit: PAGE })
      .then((r) => rows(r.data).filter(isOrderable).map(catalogOption))
      .catch(() => []),
  resolve: (id) =>
    salesApi.getProduct(id)
      .then((r) => (r.data ? catalogOption(r.data as Row) : null))
      .catch(() => null),
};

// ─── Vendors ──────────────────────────────────────────────────────────────────
// NB: the API returns `name` (backend/app/routers/vendor.py), not `company_name`.

export const vendorSource: OptionSource = serverSource(
  vendorApi.listVendors,
  (r) => ({
    id: str(r.id),
    label: str(r.name),
    sublabel: [str(r.vendor_type).replace(/_/g, " "), str(r.contact_name)].filter(Boolean).join(" · ") || undefined,
  }),
  vendorApi.getVendor,
);

// ─── People ───────────────────────────────────────────────────────────────────

export const employeeSource: OptionSource = serverSource(
  hrApi.listEmployees,
  (r) => ({
    id: str(r.id),
    label: str(r.full_name) || `${str(r.first_name)} ${str(r.last_name)}`.trim(),
    sublabel: [str(r.job_title), str(r.department)].filter(Boolean).join(" · ") || undefined,
  }),
  hrApi.getEmployee,
);

// ─── R&D ──────────────────────────────────────────────────────────────────────

export const rndProjectSource: OptionSource = serverSource(
  rndApi.listProjects,
  (r) => ({ id: str(r.id), label: str(r.name), sublabel: str(r.code) || undefined }),
  rndApi.getProject,
);
