"use client";

import { X } from "lucide-react";
import { cn } from "@/lib/utils";
import { useScrollFade } from "./useScrollFade";

export interface FilterChip {
  id: string;
  label: React.ReactNode;
  active: boolean;
  /** A count shown after the label. */
  count?: number;
}

export interface FilterChipsProps {
  chips: FilterChip[];
  onToggle: (id: string) => void;
  /** Shows "Clear" when any chip is on. */
  onClear?: () => void;
  /** Extra controls at the end (a search box, "+ Filter" menu, a date range). */
  children?: React.ReactNode;
  className?: string;
}

const chipBase =
  "inline-flex min-h-11 items-center gap-1.5 whitespace-nowrap rounded-lg border px-2.5 text-xs transition-colors md:min-h-7 " +
  "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-[rgb(var(--focus)/0.7)]";

/** Hides the scrollbar of a row that scrolls sideways (the fade edge shows there is more). */
export const noScrollbar = "[scrollbar-width:none] [&::-webkit-scrollbar]:hidden";

/**
 * The row of quick filters under the saved views. An active chip is filled (dark on light, light on dark).
 * Phones: the chips scroll sideways in one line with a fade edge; the extra controls take the next line.
 */
export function FilterChips({ chips, onToggle, onClear, children, className }: FilterChipsProps) {
  const anyOn = chips.some((c) => c.active);
  const fade = useScrollFade<HTMLDivElement>();
  return (
    <div className={cn("flex flex-col gap-2 border-b border-line bg-surface px-4 py-2 md:flex-row md:flex-wrap md:items-center md:gap-1.5", className)} role="group" aria-label="Filters">
      {(chips.length > 0 || (anyOn && onClear)) && (
        <div ref={fade.ref} onScroll={fade.onScroll} style={fade.style}
          className={cn("-mx-4 flex items-center gap-1.5 overflow-x-auto px-4 md:mx-0 md:flex-wrap md:overflow-visible md:px-0", noScrollbar)}>
          {chips.map((c) => (
            <button key={c.id} type="button" aria-pressed={c.active} onClick={() => onToggle(c.id)}
              className={cn(chipBase, "shrink-0", c.active
                ? "border-fg bg-fg text-surface"
                : "border-line bg-surface text-fg hover:bg-surface-2")}>
              {c.label}
              {c.count != null && <span className={cn("tabular", c.active ? "opacity-80" : "text-fg-muted")}>{c.count}</span>}
            </button>
          ))}
          {anyOn && onClear && (
            <button type="button" onClick={onClear} className={cn(chipBase, "shrink-0 border-transparent text-fg-muted hover:text-fg")}>
              <X className="size-3" aria-hidden /> Clear
            </button>
          )}
        </div>
      )}
      {children && (
        <div className="flex flex-wrap items-center gap-2 md:ml-auto max-md:[&>*]:min-w-0 max-md:[&>*]:flex-1 max-md:[&_input]:w-full max-md:[&_select]:w-full">
          {children}
        </div>
      )}
    </div>
  );
}
