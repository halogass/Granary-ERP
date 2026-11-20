# UI components

The look is dense and data-first: IBM Plex Sans, 8 px corners, comfortable 38 px striped rows, soft status pills,
the brand colour for the primary action and "needs you", light and dark. Everything below is in
`src/components/ui` and imported from one place:

```tsx
import { PageHeader, ListFrame, SavedViewTabs, useSavedViews, FilterChips, DataTable, StatusPill, Button } from "@/components/ui";
```

Open `/style-guide` (source: `src/app/(erp)/style-guide/page.tsx`) to see every component in both themes.

## Rules when converting a page

1. **No hex colors, no `bg-white`, no `text-gray-*`.** Use the tokens:

   | Use | Class |
   |---|---|
   | page background | `bg-canvas` (the shell already sets it) |
   | cards, tables, bars | `bg-surface` |
   | stripes, table head, soft fills, hover | `bg-surface-2` |
   | menus, dialogs | `bg-surface-raised` |
   | borders | `border-line` (inputs `border-line-strong`, faint `border-line-soft`) |
   | text | `text-fg`, `text-fg-muted` (hints, summaries), `text-fg-subtle` (placeholders, keys) |
   | primary action, active item | `bg-brand text-brand-on`, hover `bg-brand-hover` |
   | red text or icon | `text-brand-ink` (lighter in dark mode, keeps contrast) |
   | status colors | `bg-tone-{neutral,blue,violet,orange,red}-soft text-tone-...` (prefer `<StatusPill>`) |

   Old classes keep working (gray-* and slate-* now read the token scale and flip in dark mode; a dark-mode
   fallback softens `bg-white`, `bg-*-50/100` and `text-*-700`), but that fallback is temporary: convert.
2. **Numbers**: amounts and counts get `tabular` (tabular figures) and are right-aligned in tables
   (`meta: { numeric: true }`). IBM Plex Mono (`font-mono`) only for codes, ids and keys, never amounts.
3. **Weights** 400 / 500 / 600 only (`font-bold` already renders 600). Page title `text-lg font-semibold`
   (PageHeader does it), section title `text-sm font-semibold`, body `text-sm`, hints `text-[13px]` or `text-xs`.
4. **Red means needs you**: primary buttons, the active nav item, counts of things waiting for the user,
   overdue, failed, rejected. Not for "fine" states. Destructive buttons are `variant="danger"` (outlined).
5. **Statuses** always through `<StatusPill family="invoice" status={inv.status} />`. Families and colors live
   in `src/components/ui/status.ts`; add a status there, never color one in a page. Deal stage / lead status
   labels still come from `useCrmStages` (`stageStyle(reg, key).label`), pass it as `label`.
6. **Accessibility**: real `<button>`/`<Link>`, every icon-only button is `<IconButton label="...">`, every
   control inside a `<Field label>`. Controls are 44 px tall on phones and dense from `md` up (built in).
7. **Keep behaviour**: same API calls, same permission checks (`usePermissions`), same routes, same toasts.
   Only markup and classes change. No em dashes in copy; sentence case; buttons say what they do
   ("Send invoice", toast "Invoice sent").

## Components

| Component | What for |
|---|---|
| `Button` | `variant`: primary / secondary (default) / ghost / danger; `size`: sm / md / lg; `loading`; `asChild` to style a `<Link>` |
| `IconButton` | icon-only, `label` required, optional `badge` count |
| `Field`, `FieldGrid`, `Label` | label + control + hint or error; `FieldGrid cols={2}` for forms |
| `Input`, `Textarea`, `Select`, `Checkbox`, `Switch` | form controls; `Input numeric`, `leading={Search}`, `trailing="USD"`; `Select options=[...]` |
| `Combobox` | searchable select with server search (wraps `SearchSelect`; sources in `lib/entitySources.ts`) |
| `Card`, `CardHeader`, `CardBody`, `CardFooter`, `KeyValues` | detail sections and form blocks; `KeyValues` for label/value pairs |
| `Pill`, `StatusPill`, `CountBadge` | soft colored pills; red count for needs-you |
| `Tabs`, `TabsList`, `TabsTrigger`, `TabsContent` | sections inside one record page |
| `SavedViewTabs` + `useSavedViews` | saved views under a list header, stored per user and page in localStorage |
| `FilterChips` | quick filters under the views; `children` for a search box or extra controls on the right |
| `DataTable`, `Pager` | the list table (react-table): sorting, stripes, sticky header, expandable rows, empty/loading, `rowHref`; one card per row on phones (see "Phones") |
| `PageHeader`, `ListFrame`, `Toolbar`, `ToolbarSpacer` | page top (title, summary, status, actions, back link) |
| `FullHeightPage` | inboxes, chats, calls: fills the space between the Topbar and the phone BottomNav exactly (shell heights are `--shell-top` / `--shell-bottom` in globals.css); panes scroll inside, the page never does |
| `Dialog`, `Sheet` | modal and side/bottom panel (Radix: focus trap, Escape); bottom sheet or full screen on phones |
| `DropdownMenu`, `MenuItem`, `MenuSeparator`, `MenuLabel`, `MenuRadioGroup` | row actions, "More" menus |
| `Tooltip` | hover/focus hint for icon buttons (never essential info) |
| `toast`, `ErrorToast` | `toast.success("Saved")`, `toast.error(extractApiError(e))`; ErrorToast for list rows |
| `EmptyState`, `Skeleton`, `Kbd` | empty lists (say what to do next), loading placeholders, key hints |
| `Stat`, `StatGroup` | dashboard numbers; `alert` marks a number that needs attention |
| `useIsPhone`, `useMediaQuery` | `true` below md (768 px); false on the server and in tests without a `matchMedia` mock |
| `useScrollFade` | a row that scrolls sideways: fade on the side with hidden items, active item scrolled into view |

The shell (`src/components/layout`) is done: `Sidebar` (menu in `nav.ts`), `Topbar`, `CommandPalette` (Ctrl K;
`openCommandPalette()`), `BottomNav`. Pages do not render their own outer chrome. A record page appears under
"Recent records" in the palette automatically (its `<h1>` is the title), so give record pages a PageHeader whose
title is the record's name. The Nary assistant panel moved from Ctrl K to Ctrl J.

## A list page

```tsx
"use client";

import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Plus, Search } from "lucide-react";
import Link from "next/link";
import {
  Button, DataTable, FilterChips, Input, KeyValues, ListFrame, PageHeader, Pager, SavedViewTabs,
  StatusPill, useSavedViews, type ColumnDef, type SavedView,
} from "@/components/ui";
import { financeApi } from "@/lib/api";   // use the page's existing API call and params
import { formatCurrency, formatDate } from "@/lib/utils";

interface Invoice { id: string; number: string; client_name: string; status: string; total: number; currency: string; due_date: string }
type Filters = { status?: string; mine?: boolean };

// Built-in views: stable ids, defined outside the component.
const VIEWS: SavedView<Filters>[] = [
  { id: "open", label: "All open", filters: {} },
  { id: "overdue", label: "Overdue", filters: { status: "overdue" } },
  { id: "draft", label: "Drafts", filters: { status: "draft" } },
];

export default function InvoicesPage() {
  const views = useSavedViews<Filters>("finance.invoices", VIEWS);
  const [chips, setChips] = useState<Record<string, boolean>>({});
  const [q, setQ] = useState("");
  const [page, setPage] = useState(1);
  // The view's filters plus the chips in effect; this is what "Save view" stores.
  const filters: Filters = { ...views.active?.filters, ...(chips.mine ? { mine: true } : {}) };

  const { data, isLoading } = useQuery({
    queryKey: ["invoices", filters, q, page],
    queryFn: () => financeApi.listInvoices({ ...filters, search: q || undefined, page }).then((r) => r.data),
  });
  const rows: Invoice[] = data?.items ?? [];

  const columns = useMemo<ColumnDef<Invoice>[]>(() => [
    { accessorKey: "number", header: "Invoice", cell: (c) => <span className="font-mono text-[13px]">{c.getValue<string>()}</span> },
    { accessorKey: "client_name", header: "Client" },
    { accessorKey: "status", header: "Status", cell: (c) => <StatusPill family="invoice" status={c.getValue<string>()} /> },
    { accessorKey: "due_date", header: "Due", meta: { numeric: true, hideOnMobile: true }, cell: (c) => formatDate(c.getValue<string>()) },
    { accessorKey: "total", header: "Total", meta: { numeric: true }, cell: (c) => formatCurrency(c.getValue<number>(), c.row.original.currency) },
  ], []);

  return (
    <ListFrame>
      <PageHeader flush title="Invoices" summary={`${data?.total ?? 0} invoices`}
        actions={<Button variant="primary" asChild><Link href="/finance/invoices/new"><Plus aria-hidden /> New invoice</Link></Button>} />
      <SavedViewTabs state={views} currentFilters={filters} onSelect={() => setPage(1)} />
      <FilterChips chips={[{ id: "mine", label: "Created by me", active: !!chips.mine }]}
        onToggle={(id) => setChips((c) => ({ ...c, [id]: !c[id] }))} onClear={() => setChips({})}>
        <Input type="search" leading={Search} value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search invoices" aria-label="Search invoices" className="w-56" />
      </FilterChips>
      <DataTable label="Invoices" columns={columns} data={rows} getRowId={(r) => r.id} loading={isLoading}
        empty={{ title: "No invoices in this view", description: "Pick another view, or create an invoice." }}
        renderDetails={(inv) => (
          <div className="flex flex-wrap items-start gap-6">
            <KeyValues className="min-w-[260px] flex-1" items={[
              { label: "Client", value: inv.client_name },
              { label: "Due", value: formatDate(inv.due_date) },
              { label: "Total", value: formatCurrency(inv.total, inv.currency), numeric: true },
            ]} />
            <Button size="sm" asChild><Link href={`/finance/invoices/${inv.id}`}>Open invoice</Link></Button>
          </div>
        )}
        footer={<Pager page={page} pageSize={20} total={data?.total ?? 0} onPage={setPage} />} />
    </ListFrame>
  );
}
```

Notes:
- Filters come from the active view (restored on return, per user). Chips refine it; "Save view" stores the
  combination. If the page filters on the server, keep the same API params it uses today.
- The details under a row show what you would otherwise open the record for (key facts, the next action, a
  waiting Nary card), plus a link or button to the full record. Rows without details use `rowHref` instead.
- `DataTable` sorts on the client by default. For server sorting pass `sorting`, `onSortingChange`, `manualSorting`.
- Bulk selection: add a first column with `<Checkbox aria-label="Select ...">`; clicks on controls in a row do
  not toggle the row. Keep `components/common/BulkActionBar` for the actions.
- A list that is not the main subject of the page (related records on a detail page) uses `DataTable` inside
  a `Card` with `maxHeight="none"`.

## A form page (or a detail page)

```tsx
"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import {
  Button, Card, CardBody, CardFooter, CardHeader, Combobox, Field, FieldGrid, Input, PageHeader, Select,
  StatusPill, Switch, Textarea, toast,
} from "@/components/ui";
import { extractApiError } from "@/lib/apiError";
import { contactSource } from "@/lib/entitySources";

export default function EditVendorPage({ vendor }: { vendor: Vendor }) {
  const router = useRouter();
  const [form, setForm] = useState(vendor);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const set = (k: keyof Vendor) => (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>) =>
    setForm((f) => ({ ...f, [k]: e.target.value }));

  async function save() {
    setSaving(true);
    try {
      await vendorApi.update(vendor.id, form);
      toast.success("Vendor saved");
      router.push(`/vendor-mgmt/vendors`);
    } catch (e) {
      toast.error(extractApiError(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="mx-auto max-w-4xl">
      <PageHeader title={vendor.name} back={{ href: "/vendor-mgmt/vendors", label: "Vendors" }}
        status={<StatusPill family="generic" status={vendor.status} />}
        actions={<><Button onClick={() => router.back()}>Cancel</Button><Button variant="primary" loading={saving} onClick={save}>Save vendor</Button></>} />

      <div className="space-y-4">
        <Card>
          <CardHeader title="Company" description="As it appears on purchase orders." />
          <CardBody>
            <FieldGrid cols={2}>
              <Field label="Legal name" required error={errors.name}><Input value={form.name} onChange={set("name")} /></Field>
              <Field label="Tax ID" hint="As registered."><Input value={form.tax_id ?? ""} onChange={set("tax_id")} className="font-mono" /></Field>
              <Field label="Payment terms"><Select value={form.terms} onChange={set("terms")} options={[{ value: "14", label: "14 days" }, { value: "30", label: "30 days" }]} /></Field>
              <Field label="Main contact"><Combobox value={form.contact_id ?? ""} onChange={(id) => setForm((f) => ({ ...f, contact_id: id }))} source={contactSource()} /></Field>
              <Field label="Notes" className="md:col-span-2"><Textarea value={form.notes ?? ""} onChange={set("notes")} /></Field>
            </FieldGrid>
          </CardBody>
        </Card>
        <Card>
          <CardHeader title="Purchasing" />
          <CardBody><Switch label="Allow new purchase orders" checked={form.active} onCheckedChange={(v) => setForm((f) => ({ ...f, active: v }))} /></CardBody>
          <CardFooter><Button variant="primary" loading={saving} onClick={save}>Save vendor</Button></CardFooter>
        </Card>
      </div>
    </div>
  );
}
```

Notes:
- One `Card` per section; the header row is title left, actions right. Detail pages: `PageHeader` with the
  record name, `status` pill and actions, then Cards (`KeyValues` for facts, `DataTable` for related lists).
- Dialogs that hold a form: `<Dialog open onOpenChange title footer={buttons}>` with Fields inside.
  A required reason before an action (reject, revoke, revise): `components/ui/ReasonModal` (a Dialog).
- Dashboards: `StatGroup` of `Stat`s on top, then compact `DataTable`s in Cards.

## Phones (below md, 768 px)

Everything a desktop user can see and do works at 390 px, without page code for the common cases:

- **DataTable** renders a card per row (`data-layout="cards"`): the title line, the status pill top right,
  then every other column as a label: value pair, the row actions as buttons, and "Show details" (or a tap
  on the card) unfolds the same `renderDetails` as desktop. `hideOnMobile` no longer hides anything: on
  phones the column simply sits in the card body; on desktop the table is unchanged. A "Sort by" select
  above the cards lists every sortable column (ascending and descending), and the select column's header
  checkbox shows there as "Select all". Pairs whose cell renders nothing are left out. `maxHeight` only
  applies on phones when you set it yourself (panes); otherwise the page scrolls.
  The card parts are guessed from the columns; set `meta.card` when the guess is wrong:

  | Part | Guessed from | `meta.card` |
  |---|---|---|
  | checkbox (44 px tap area) | id `select` | `"select"` |
  | title (first line bold, more title columns muted under it) | the first other column | `"title"` |
  | pill top right | id or header `status`, `stage`, `state` | `"status"` |
  | buttons row | id `actions`/`action`/`remove`/`edit`/`menu`..., or an empty header | `"actions"` |
  | label: value pair | everything else | `"body"` |

  The label is the header text; give `meta.label` when the header is a component.
- **Dialog**: `sm`/`md` are bottom sheets up to 92dvh, `lg`/`xl` take the whole screen; header and footer
  stay put while the body scrolls, footer buttons share the width. **Sheet** `side="right"` is full width,
  `side="bottom"` goes up to 92dvh. Keep dialog forms in one `FieldGrid` (it is one column on phones).
- **SavedViewTabs, FilterChips, TabsList** scroll sideways in one line with a fade edge on the side that has
  more, and scroll the active tab into view. FilterChips puts its `children` (search, selects) on their
  own full-width line under the chips.
- **Sizes**: Button, IconButton, inputs, selects, chips, tabs and menu items are 44 px tall on phones;
  text controls use a 16 px font there so iOS does not zoom (do not override with `text-sm` on an `Input`).
- **Hover-only controls** must be reachable by touch: hide them with `[@media(hover:hover)]:opacity-0
  [@media(hover:hover)]:group-hover:opacity-100 [@media(hover:hover)]:focus-within:opacity-100`, never a plain
  `opacity-0 group-hover:opacity-100`.
- **BulkActionBar** docks to the bottom of the screen above the BottomNav and publishes its height as
  `--bulk-bar-h`; a scrolling list under it adds `pb-[var(--bulk-bar-h,0px)]` (DataTable cards do).
- **Shell**: the Topbar keeps search, clock in/out, notifications (a full-width panel) and the account menu
  (profile, theme, sign out) on phones; the BottomNav "More" sheet holds the whole menu plus a search entry.

