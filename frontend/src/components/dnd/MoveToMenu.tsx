"use client";

import { ArrowRightLeft, Check } from "lucide-react";
import { DropdownMenu, IconButton, MenuItem, MenuLabel } from "@/components/ui";

/**
 * "Move to" on a board card: the same move as dragging it to another column, for touch screens
 * (and keyboards) where dragging is unreliable. Events stop here, so opening the menu or picking a
 * column never opens or drags the card underneath.
 */
export function MoveToMenu({ itemLabel, current, options, onMove, className }: {
  /** The card's name, for the button's accessible label. */
  itemLabel: string;
  /** The column the card is in now (shown checked, not selectable). */
  current: string;
  options: { value: string; label: string }[];
  onMove: (value: string) => void;
  className?: string;
}) {
  const stop = (e: React.SyntheticEvent) => e.stopPropagation();
  return (
    <span className={className} onClick={stop} onMouseDown={stop} onTouchStart={stop} onPointerDown={stop} onKeyDown={stop}>
      <DropdownMenu trigger={<IconButton size="sm" icon={ArrowRightLeft} label={`Move ${itemLabel} to`} className="bg-surface-raised" />}>
        <MenuLabel>Move to</MenuLabel>
        {options.map((o) => (
          <MenuItem key={o.value} disabled={o.value === current} onSelect={() => onMove(o.value)}>
            <span className="flex-1">{o.label}</span>
            {o.value === current && <Check aria-hidden className="text-brand-ink" />}
          </MenuItem>
        ))}
      </DropdownMenu>
    </span>
  );
}
