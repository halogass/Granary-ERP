/**
 * Class strings for a hand-built <table> that turns into one card per row on phones (< md).
 * The desktop table is unchanged. On phones the header row hides, each row is a card and every
 * cell shows its column name from `data-label`, so no column is lost and nothing scrolls sideways.
 *
 *   <table className={cn("w-full text-sm", stack.table)}>
 *     <thead className={stack.thead}>...</thead>
 *     <tbody className={stack.tbody}>
 *       <tr className={stack.tr}>
 *         <td className={cn("px-3 py-2", stack.title)}>{name}</td>           // card title, no label
 *         <td data-label="Amount" className={cn("px-3 py-2 text-right", stack.td)}>{amount}</td>
 *         <td data-label="Tax" className={cn("px-3 py-2", stack.field)}><Select /></td>  // editable: label above
 *         <td className={cn("px-3 py-2", stack.actions)}><IconButton /></td>
 *
 * Put the stack classes last in `cn()` so they win over the desktop paddings on phones.
 */
const label =
  "max-md:before:text-xs max-md:before:font-medium max-md:before:text-fg-muted max-md:before:content-[attr(data-label)]";

export const stack = {
  /** On the <table>; drop any min-w-[...] to `md:min-w-[...]`. */
  table: "max-md:block max-md:min-w-0",
  thead: "max-md:hidden",
  tbody: "max-md:block max-md:space-y-2 max-md:divide-y-0 max-md:p-3",
  /** A row as a card. */
  tr: "max-md:block max-md:rounded-lg max-md:border max-md:border-line max-md:bg-surface max-md:px-3 max-md:py-2 max-md:even:bg-surface",
  /** A read-only cell: "Label    value" on one line, value right-aligned. */
  td: `max-md:flow-root max-md:border-0 max-md:px-0 max-md:py-1 max-md:text-right max-md:before:float-left max-md:before:mr-3 max-md:before:text-left ${label}`,
  /** An editable cell: the label above, the control full width. */
  field: `max-md:block max-md:w-full max-md:border-0 max-md:px-0 max-md:py-1.5 max-md:text-left max-md:before:mb-1 max-md:before:block ${label}`,
  /** The cell that names the row: the card's title line, no label. */
  title: "max-md:block max-md:border-0 max-md:px-0 max-md:py-1 max-md:text-left max-md:font-medium",
  /** Row buttons: a full-width row of visible buttons at the card's foot. */
  actions: "max-md:flex max-md:flex-wrap max-md:justify-end max-md:gap-2 max-md:border-0 max-md:px-0 max-md:pt-2",
  /** A cell with nothing useful on phones (a spacer, a drag handle that is duplicated elsewhere). */
  hide: "max-md:hidden",
  /** A <tfoot> or a totals row: shown as a card too. */
  tfoot: "max-md:block max-md:px-3 max-md:pb-3",
} as const;
