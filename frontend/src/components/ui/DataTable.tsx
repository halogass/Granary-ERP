"use client";

import { Fragment, useState } from "react";
import { useRouter } from "next/navigation";
import {
  flexRender, getCoreRowModel, getSortedRowModel, useReactTable,
  type Column, type ColumnDef, type Header, type Row, type RowData, type SortingState, type OnChangeFn, type Table,
} from "@tanstack/react-table";
import { ArrowDown, ArrowUp, ChevronRight, ChevronsUpDown } from "lucide-react";
import { cn } from "@/lib/utils";
import { EmptyState, Skeleton, type EmptyStateProps } from "./Feedback";
import { Select } from "./Input";
import { useIsPhone } from "./useMediaQuery";

declare module "@tanstack/react-table" {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  interface ColumnMeta<TData extends RowData, TValue> {
    /** Right-aligns the column with tabular figures (amounts, counts, dates). */
    numeric?: boolean;
    align?: "left" | "center" | "right";
    /** CSS width of the column (e.g. "120px", "30%"). */
    width?: string;
    /** Extra classes for the body cells. */
    className?: string;
    /** A secondary column: on phones it goes in the card body like every other column (never hidden). */
    hideOnMobile?: boolean;
    /**
     * Its place in the phone card. Guessed when unset: a "select" column is the checkbox, "status" the
     * pill top right, "actions" (or a column without a header) the buttons row, the first other column
     * the title, everything else a label: value pair.
     */
    card?: CardRole;
    /** The column's name where its header is not plain text (card labels, the "Sort by" select). */
    label?: string;
  }
}

export type { ColumnDef, SortingState };

/** Where a column goes in a phone card (see ColumnMeta.card). */
export type CardRole = "title" | "status" | "actions" | "select" | "body";

export interface DataTableProps<T> {
  columns: ColumnDef<T, any>[]; // eslint-disable-line @typescript-eslint/no-explicit-any
  data: T[];
  /** Stable id per row (record id). Defaults to the row index. */
  getRowId?: (row: T, index: number) => string;

  /** Content unfolded under a clicked row, full table width. Makes rows expandable. */
  renderDetails?: (row: T) => React.ReactNode;
  /** Controlled expanded row (one at a time). */
  expandedId?: string | null;
  onExpandedChange?: (id: string | null) => void;

  /** Called on a row click (after expanding, when renderDetails is set). */
  onRowClick?: (row: T) => void;
  /** Without renderDetails: clicking or pressing Enter on a row opens this URL. */
  rowHref?: (row: T) => string | undefined;
  /** Tints a row as selected. */
  selectedId?: string | null;

  /** Server-side sorting: pass the state and handle changes yourself. Client-side otherwise. */
  sorting?: SortingState;
  onSortingChange?: OnChangeFn<SortingState>;
  manualSorting?: boolean;

  loading?: boolean;
  /** Shown when there are no rows and not loading. */
  empty?: EmptyStateProps;
  /** Under the table, inside the frame (totals, pagination). */
  footer?: React.ReactNode;
  striped?: boolean;
  /** The scroll area's max height; the header sticks within it. "none" lets the page scroll. */
  maxHeight?: string;
  /** Accessible name of the table. */
  label?: string;
  className?: string;
}

const INTERACTIVE = "a,button,input,select,textarea,label,[role=menuitem],[role=checkbox],[data-row-click=ignore]";
const DEFAULT_MAX_HEIGHT = "calc(100dvh - 12rem)";

/**
 * The list table: sortable headers, striped comfortable rows (38 px, 14 px text), a sticky
 * header, numbers right-aligned, and rows that unfold their details underneath.
 */
export function DataTable<T>({
  columns, data, getRowId, renderDetails, expandedId, onExpandedChange, onRowClick, rowHref, selectedId,
  sorting, onSortingChange, manualSorting, loading, empty, footer, striped = true,
  maxHeight = DEFAULT_MAX_HEIGHT, label, className,
}: DataTableProps<T>) {
  const router = useRouter();
  const phone = useIsPhone();
  const [innerSorting, setInnerSorting] = useState<SortingState>([]);
  const [innerExpanded, setInnerExpanded] = useState<string | null>(null);
  const expanded = expandedId !== undefined ? expandedId : innerExpanded;

  const table = useReactTable({
    data,
    columns,
    getRowId: getRowId ? (row, i) => getRowId(row, i) : undefined,
    state: { sorting: sorting ?? innerSorting },
    onSortingChange: onSortingChange ?? setInnerSorting,
    manualSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });

  const expandable = !!renderDetails;
  const colCount = table.getVisibleLeafColumns().length + (expandable ? 1 : 0);

  function toggle(id: string) {
    const next = expanded === id ? null : id;
    if (expandedId === undefined) setInnerExpanded(next);
    onExpandedChange?.(next);
  }

  function activate(row: T, id: string) {
    if (expandable) toggle(id);
    onRowClick?.(row);
    if (!expandable) {
      const href = rowHref?.(row);
      if (href) router.push(href);
    }
  }

  function onClick(e: React.MouseEvent, row: T, id: string) {
    const target = e.target as HTMLElement;
    const hit = target.closest(INTERACTIVE);
    if (hit && hit !== e.currentTarget && e.currentTarget.contains(hit)) return;
    if (window.getSelection()?.toString()) return;   // selecting text is not a click
    activate(row, id);
  }

  const clickable = expandable || !!onRowClick || !!rowHref;
  const rows = table.getRowModel().rows;

  if (phone) {
    return (
      <PhoneCards table={table} expandable={expandable} expanded={expanded} renderDetails={renderDetails}
        activate={activate} onClick={onClick} clickable={clickable} rowHref={rowHref} onRowClick={onRowClick}
        selectedId={selectedId} loading={loading} empty={empty} footer={footer} label={label} className={className}
        maxHeight={maxHeight === "none" || maxHeight === DEFAULT_MAX_HEIGHT ? undefined : maxHeight} />
    );
  }

  return (
    <div className={cn("bg-surface", className)}>
      <div className="overflow-auto" style={maxHeight === "none" ? undefined : { maxHeight }}>
        <table className="w-full border-separate border-spacing-0 text-sm" aria-label={label} aria-busy={loading || undefined}>
          <thead>
            {table.getHeaderGroups().map((hg) => (
              <tr key={hg.id}>
                {expandable && <th aria-hidden className="sticky top-0 z-10 w-8 border-b border-line bg-surface-2" />}
                {hg.headers.map((h) => {
                  const meta = h.column.columnDef.meta;
                  const right = meta?.numeric || meta?.align === "right";
                  const sortable = h.column.getCanSort();
                  const dir = h.column.getIsSorted();
                  return (
                    <th key={h.id} scope="col" style={{ width: meta?.width }}
                      aria-sort={dir === "asc" ? "ascending" : dir === "desc" ? "descending" : sortable ? "none" : undefined}
                      className={cn("sticky top-0 z-10 h-9 whitespace-nowrap border-b border-line bg-surface-2 px-3 text-[11px] font-medium uppercase tracking-[0.04em] text-fg-muted",
                        right ? "text-right" : meta?.align === "center" ? "text-center" : "text-left",
                        meta?.hideOnMobile && "hidden md:table-cell")}>
                      {h.isPlaceholder ? null : sortable ? (
                        <button type="button" onClick={h.column.getToggleSortingHandler()}
                          className={cn("group inline-flex items-center gap-1 uppercase hover:text-fg", right && "flex-row-reverse")}>
                          {flexRender(h.column.columnDef.header, h.getContext())}
                          {dir === "asc" ? <ArrowUp className="size-3" aria-hidden />
                            : dir === "desc" ? <ArrowDown className="size-3" aria-hidden />
                            : <ChevronsUpDown className="size-3 opacity-0 group-hover:opacity-60" aria-hidden />}
                        </button>
                      ) : flexRender(h.column.columnDef.header, h.getContext())}
                    </th>
                  );
                })}
              </tr>
            ))}
          </thead>
          <tbody>
            {loading && rows.length === 0 ? (
              Array.from({ length: 6 }).map((_, i) => (
                <tr key={`sk${i}`}>
                  {Array.from({ length: colCount }).map((__, j) => (
                    <td key={j} className="h-row border-b border-line-soft px-3"><Skeleton className="h-3.5 w-3/4" /></td>
                  ))}
                </tr>
              ))
            ) : rows.length === 0 ? (
              <tr>
                <td colSpan={colCount}>
                  <EmptyState title="Nothing here yet" {...empty} />
                </td>
              </tr>
            ) : rows.map((row, i) => {
              const open = expandable && expanded === row.id;
              const tinted = open || (selectedId != null && selectedId === row.id);
              const detailsId = `row-${row.id}-details`;
              const href = !expandable ? rowHref?.(row.original) : undefined;
              return (
                <Fragment key={row.id}>
                  <tr data-row-id={row.id} data-state={open ? "open" : undefined}
                    onClick={clickable ? (e) => onClick(e, row.original, row.id) : undefined}
                    tabIndex={!expandable && (href || onRowClick) ? 0 : undefined}
                    onKeyDown={!expandable && (href || onRowClick) ? (e) => {
                      if (e.key === "Enter" && e.target === e.currentTarget) { e.preventDefault(); activate(row.original, row.id); }
                    } : undefined}
                    className={cn("group/row focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)]",
                      clickable && "cursor-pointer",
                      tinted ? "bg-brand/[0.07]" : striped && i % 2 === 1 ? "bg-surface-2/70" : "bg-surface",
                      !tinted && clickable && "hover:bg-brand/[0.035]")}>
                    {expandable && (
                      <td className="h-row w-8 border-b border-line-soft pl-2">
                        <button type="button" aria-expanded={open} aria-controls={open ? detailsId : undefined}
                          aria-label={open ? "Hide details" : "Show details"} onClick={() => activate(row.original, row.id)}
                          className="flex size-6 items-center justify-center rounded text-fg-subtle hover:bg-surface-2 hover:text-fg">
                          <ChevronRight className={cn("size-4 transition-transform", open && "rotate-90")} aria-hidden />
                        </button>
                      </td>
                    )}
                    {row.getVisibleCells().map((cell) => {
                      const meta = cell.column.columnDef.meta;
                      const right = meta?.numeric || meta?.align === "right";
                      return (
                        <td key={cell.id}
                          className={cn("h-row border-b border-line-soft px-3 py-1.5 align-middle text-fg",
                            right ? "text-right tabular" : meta?.align === "center" ? "text-center" : "text-left",
                            meta?.hideOnMobile && "hidden md:table-cell", meta?.className)}>
                          {flexRender(cell.column.columnDef.cell, cell.getContext())}
                        </td>
                      );
                    })}
                  </tr>
                  {open && (
                    <tr id={detailsId} data-details-for={row.id}>
                      <td colSpan={colCount} className="border-b border-line bg-surface-2 px-4 py-3 md:pl-11">
                        {renderDetails!(row.original)}
                      </td>
                    </tr>
                  )}
                </Fragment>
              );
            })}
          </tbody>
        </table>
      </div>
      {footer && <div className="flex flex-wrap items-center gap-3 border-t border-line px-4 py-2 text-[13px] text-fg-muted">{footer}</div>}
    </div>
  );
}

// ---- Phones: one card per row ----

const SELECT_IDS = /^(_?select(ed|ion)?|check(ed)?)$/i;
const STATUS_IDS = /^(status|stage|state)$/i;
const ACTION_IDS = /^(_?actions?|row_?actions|menu|more|buttons|controls|remove|delete|edit|revoke|review|open)$/i;

function headerText<T>(c: Column<T, unknown>): string {
  const h = c.columnDef.header;
  if (c.columnDef.meta?.label) return c.columnDef.meta.label;
  if (typeof h === "string" && h.trim()) return h;
  const words = c.id.replace(/[_-]+/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** Sorts the visible columns into the parts of a card. Exported for tests. */
export function cardLayout<T>(columns: Column<T, unknown>[]) {
  const role = new Map<string, CardRole>();
  for (const c of columns) {
    const meta = c.columnDef.meta;
    const h = c.columnDef.header;
    if (meta?.card) role.set(c.id, meta.card);
    else if (SELECT_IDS.test(c.id)) role.set(c.id, "select");
    else if (ACTION_IDS.test(c.id) || h == null || (typeof h === "string" && !h.trim())) role.set(c.id, "actions");
  }
  if (![...role.values()].includes("status")) {
    const st = columns.find((c) => !role.has(c.id)
      && (STATUS_IDS.test(c.id) || (typeof c.columnDef.header === "string" && STATUS_IDS.test(c.columnDef.header.trim()))));
    if (st) role.set(st.id, "status");
  }
  if (![...role.values()].includes("title")) {
    const first = columns.find((c) => !role.has(c.id));
    if (first) role.set(first.id, "title");
  }
  const of = (r: CardRole) => columns.filter((c) => (role.get(c.id) ?? "body") === r);
  return { select: of("select"), title: of("title"), status: of("status"), body: of("body"), actions: of("actions") };
}

interface PhoneCardsProps<T> {
  table: Table<T>;
  expandable: boolean;
  expanded: string | null;
  renderDetails?: (row: T) => React.ReactNode;
  activate: (row: T, id: string) => void;
  onClick: (e: React.MouseEvent, row: T, id: string) => void;
  clickable: boolean;
  rowHref?: (row: T) => string | undefined;
  onRowClick?: (row: T) => void;
  selectedId?: string | null;
  loading?: boolean;
  empty?: EmptyStateProps;
  footer?: React.ReactNode;
  label?: string;
  className?: string;
  maxHeight?: string;
}

/**
 * The phone version of DataTable: every row is a card with its title, status pill, every other
 * column as label: value pairs, the row actions as buttons and the same unfolding details.
 */
function PhoneCards<T>({
  table, expandable, expanded, renderDetails, activate, onClick, clickable, rowHref, onRowClick, selectedId,
  loading, empty, footer, label, className, maxHeight,
}: PhoneCardsProps<T>) {
  const columns = table.getVisibleLeafColumns();
  const layout = cardLayout(columns);
  const headers = new Map<string, Header<T, unknown>>(table.getLeafHeaders().map((h) => [h.column.id, h]));
  const rows = table.getRowModel().rows;

  const sortable = columns.filter((c) => c.getCanSort() && !layout.select.includes(c) && !layout.actions.includes(c));
  const sorted = table.getState().sorting[0];
  const sortValue = sorted ? `${sorted.id}:${sorted.desc ? "desc" : "asc"}` : "";
  const selectAll = layout.select[0] && headers.get(layout.select[0].id);
  const showBar = rows.length > 0 && (sortable.length > 0 || !!selectAll);

  function sortBy(v: string) {
    if (!v) { table.setSorting([]); return; }
    const [id, dir] = v.split(":");
    table.setSorting([{ id, desc: dir === "desc" }]);
  }

  function renderCells(row: Row<T>, cols: Column<T, unknown>[]) {
    const cells = row.getVisibleCells();
    return cols.map((c) => {
      const cell = cells.find((x) => x.column.id === c.id);
      return cell ? <Fragment key={c.id}>{flexRender(c.columnDef.cell, cell.getContext())}</Fragment> : null;
    });
  }

  function labelOf(c: Column<T, unknown>) {
    const h = c.columnDef.header;
    if (c.columnDef.meta?.label || typeof h === "string") return headerText(c);
    const header = headers.get(c.id);
    return header ? flexRender(h, header.getContext()) : headerText(c);
  }

  return (
    <div className={cn("bg-surface", className)} data-layout="cards">
      {showBar && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-4 py-2">
          {selectAll && (
            <label className="flex min-h-11 items-center gap-2.5 text-[13px] text-fg-muted">
              {flexRender(selectAll.column.columnDef.header, selectAll.getContext())}
              Select all
            </label>
          )}
          {sortable.length > 0 && (
            <label className="ml-auto flex min-w-0 items-center gap-2 text-[13px] text-fg-muted">
              <span className="shrink-0">Sort by</span>
              <Select aria-label="Sort by" value={sortValue} onChange={(e) => sortBy(e.target.value)} className="min-w-0 max-w-[14rem]">
                <option value="">Default order</option>
                {sortable.map((c) => (
                  <Fragment key={c.id}>
                    <option value={`${c.id}:asc`}>{headerText(c)}, ascending</option>
                    <option value={`${c.id}:desc`}>{headerText(c)}, descending</option>
                  </Fragment>
                ))}
              </Select>
            </label>
          )}
        </div>
      )}
      <div className={maxHeight ? "overflow-auto" : undefined} style={maxHeight ? { maxHeight } : undefined}>
        {loading && rows.length === 0 ? (
          <ul aria-label={label} aria-busy className="divide-y divide-line-soft">
            {Array.from({ length: 4 }).map((_, i) => (
              <li key={i} className="space-y-2 px-4 py-3">
                <Skeleton className="h-4 w-2/3" />
                <Skeleton className="h-3 w-1/2" />
                <Skeleton className="h-3 w-1/3" />
              </li>
            ))}
          </ul>
        ) : rows.length === 0 ? (
          <EmptyState title="Nothing here yet" {...empty} />
        ) : (
          <ul aria-label={label} aria-busy={loading || undefined} className="divide-y divide-line-soft pb-[var(--bulk-bar-h,0px)]">
            {rows.map((row) => {
              const open = expandable && expanded === row.id;
              const tinted = open || (selectedId != null && selectedId === row.id);
              const detailsId = `row-${row.id}-details`;
              const keyable = !expandable && (!!rowHref?.(row.original) || !!onRowClick);
              return (
                <li key={row.id} data-row-id={row.id} data-state={open ? "open" : undefined}
                  onClick={clickable ? (e) => onClick(e, row.original, row.id) : undefined}
                  tabIndex={keyable ? 0 : undefined}
                  onKeyDown={keyable ? (e) => {
                    if (e.key === "Enter" && e.target === e.currentTarget) { e.preventDefault(); activate(row.original, row.id); }
                  } : undefined}
                  className={cn("px-4 py-3 text-sm text-fg focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)]",
                    clickable && "cursor-pointer", tinted ? "bg-brand/[0.07]" : "bg-surface")}>
                  <div className="flex items-start gap-2">
                    {layout.select.length > 0 && (
                      <label className="-my-2.5 -ml-3 flex size-11 shrink-0 items-center justify-center" data-row-click="ignore">
                        {renderCells(row, layout.select)}
                      </label>
                    )}
                    <div className="min-w-0 flex-1 space-y-0.5 break-words [&>*:first-child]:font-medium [&>*:not(:first-child)]:text-[13px] [&>*:not(:first-child)]:text-fg-muted">
                      {layout.title.map((c) => <div key={c.id}>{renderCells(row, [c])}</div>)}
                    </div>
                    {layout.status.length > 0 && (
                      <div className="flex shrink-0 flex-col items-end gap-1">{renderCells(row, layout.status)}</div>
                    )}
                  </div>
                  {layout.body.length > 0 && (
                    <dl className="mt-2 grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)] gap-x-3 gap-y-1.5">
                      {layout.body.map((c) => {
                        const meta = c.columnDef.meta;
                        return (
                          <div key={c.id} className="contents [&:has(>dd:empty)]:hidden">
                            <dt className="text-[13px] text-fg-muted">{labelOf(c)}</dt>
                            <dd className={cn("min-w-0 break-words", meta?.numeric && "tabular", meta?.className)}>{renderCells(row, [c])}</dd>
                          </div>
                        );
                      })}
                    </dl>
                  )}
                  {(layout.actions.length > 0 || expandable) && (
                    <div className="mt-2.5 hidden flex-wrap items-center justify-end gap-2 has-[>button]:flex has-[>div:not(:empty)]:flex">
                      {expandable && (
                        <button type="button" aria-expanded={open} aria-controls={open ? detailsId : undefined}
                          onClick={() => activate(row.original, row.id)}
                          className="mr-auto inline-flex min-h-11 items-center gap-1 rounded-lg pr-2 text-[13px] font-medium text-fg-muted hover:text-fg">
                          <ChevronRight className={cn("size-4 transition-transform", open && "rotate-90")} aria-hidden />
                          {open ? "Hide details" : "Show details"}
                        </button>
                      )}
                      {layout.actions.map((c) => (
                        <div key={c.id} className="flex flex-wrap items-center justify-end gap-2 empty:hidden">{renderCells(row, [c])}</div>
                      ))}
                    </div>
                  )}
                  {open && (
                    <div id={detailsId} data-details-for={row.id} className="-mx-4 -mb-3 mt-2 border-t border-line bg-surface-2 px-4 py-3">
                      {renderDetails!(row.original)}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
      {footer && <div className="flex flex-wrap items-center gap-3 border-t border-line px-4 py-2 text-[13px] text-fg-muted">{footer}</div>}
    </div>
  );
}

/** "41-60 of 212" with previous/next, for DataTable's footer. Pages are 1-based. */
export function Pager({ page, pageSize, total, onPage }: { page: number; pageSize: number; total: number; onPage: (page: number) => void }) {
  const last = Math.max(1, Math.ceil(total / pageSize));
  const from = total === 0 ? 0 : (page - 1) * pageSize + 1;
  const to = Math.min(total, page * pageSize);
  const btn = "inline-flex h-9 items-center rounded-lg border border-line px-2.5 text-[13px] text-fg hover:bg-surface-2 disabled:opacity-40 disabled:hover:bg-transparent md:h-7";
  return (
    <div className="ml-auto flex items-center gap-2">
      <span className="tabular">{from}-{to} of {total}</span>
      <button type="button" className={btn} disabled={page <= 1} onClick={() => onPage(page - 1)}>Previous</button>
      <button type="button" className={btn} disabled={page >= last} onClick={() => onPage(page + 1)}>Next</button>
    </div>
  );
}
