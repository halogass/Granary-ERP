"use client";

import { forwardRef } from "react";
import * as Dropdown from "@radix-ui/react-dropdown-menu";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { Check } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * Menus (Radix dropdown): keyboard, typeahead, focus return built in.
 *
 *   <DropdownMenu trigger={<IconButton label="More" icon={MoreHorizontal} />}>
 *     <MenuItem onSelect={edit}>Edit</MenuItem>
 *     <MenuSeparator />
 *     <MenuItem danger onSelect={remove}>Delete</MenuItem>
 *   </DropdownMenu>
 */
export function DropdownMenu({ trigger, align = "end", className, children, open, onOpenChange }: {
  trigger: React.ReactNode; align?: "start" | "center" | "end"; className?: string; children: React.ReactNode;
  open?: boolean; onOpenChange?: (open: boolean) => void;
}) {
  return (
    <Dropdown.Root open={open} onOpenChange={onOpenChange} modal={false}>
      <Dropdown.Trigger asChild>{trigger}</Dropdown.Trigger>
      <Dropdown.Portal>
        <Dropdown.Content align={align} sideOffset={6} collisionPadding={8}
          className={cn("z-50 min-w-[180px] max-w-[calc(100vw-1rem)] overflow-y-auto overscroll-contain rounded-lg border border-line bg-surface-raised p-1 text-fg shadow-pop",
            "max-h-[var(--radix-dropdown-menu-content-available-height)]",
            "data-[state=open]:animate-in data-[state=open]:fade-in-0", className)}>
          {children}
        </Dropdown.Content>
      </Dropdown.Portal>
    </Dropdown.Root>
  );
}

const itemClass =
  "relative flex min-h-11 cursor-pointer select-none items-center gap-2 rounded-md px-2.5 text-sm outline-none md:min-h-8 " +
  "data-[highlighted]:bg-surface-2 data-[disabled]:pointer-events-none data-[disabled]:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0";

export const MenuItem = forwardRef<HTMLDivElement, React.ComponentPropsWithoutRef<typeof Dropdown.Item> & { danger?: boolean; icon?: React.ElementType }>(
  function MenuItem({ className, danger, icon: Icon, children, ...props }, ref) {
    return (
      <Dropdown.Item ref={ref} className={cn(itemClass, danger ? "text-tone-red" : "text-fg", className)} {...props}>
        {Icon && <Icon aria-hidden className={danger ? "" : "text-fg-muted"} />}
        {children}
      </Dropdown.Item>
    );
  },
);

/** A menu item that is a choice among several (theme: System / Light / Dark). */
export function MenuRadioGroup({ value, onValueChange, options, label }: {
  value: string; onValueChange: (v: string) => void; options: { value: string; label: string; icon?: React.ElementType }[]; label?: string;
}) {
  return (
    <Dropdown.Group>
      {label && <Dropdown.Label className="px-2.5 pb-1 pt-1.5 text-xs font-medium text-fg-muted">{label}</Dropdown.Label>}
      <Dropdown.RadioGroup value={value} onValueChange={onValueChange}>
        {options.map((o) => (
          <Dropdown.RadioItem key={o.value} value={o.value} className={cn(itemClass, "text-fg")}>
            {o.icon && <o.icon aria-hidden className="text-fg-muted" />}
            <span className="flex-1">{o.label}</span>
            <Dropdown.ItemIndicator><Check aria-hidden className="text-brand-ink" /></Dropdown.ItemIndicator>
          </Dropdown.RadioItem>
        ))}
      </Dropdown.RadioGroup>
    </Dropdown.Group>
  );
}

export function MenuSeparator() {
  return <Dropdown.Separator className="-mx-1 my-1 h-px bg-line" />;
}

export function MenuLabel({ children }: { children: React.ReactNode }) {
  return <Dropdown.Label className="px-2.5 pb-1 pt-1.5 text-xs font-medium text-fg-muted">{children}</Dropdown.Label>;
}

/** A hint on hover or focus. Wrap a single focusable child. Not for essential information. */
export function Tooltip({ content, side = "top", children }: { content: React.ReactNode; side?: "top" | "right" | "bottom" | "left"; children: React.ReactElement }) {
  return (
    <TooltipPrimitive.Provider delayDuration={300}>
      <TooltipPrimitive.Root>
        <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
        <TooltipPrimitive.Portal>
          <TooltipPrimitive.Content side={side} sideOffset={6}
            className="z-[60] max-w-xs rounded-md bg-fg px-2 py-1 text-xs text-surface shadow-pop data-[state=delayed-open]:animate-in data-[state=delayed-open]:fade-in-0">
            {content}
          </TooltipPrimitive.Content>
        </TooltipPrimitive.Portal>
      </TooltipPrimitive.Root>
    </TooltipPrimitive.Provider>
  );
}
