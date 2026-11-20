"use client";

import * as DialogPrimitive from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * A modal (Radix: focus trap, Escape, aria). Controlled with open/onOpenChange.
 *
 *   <Dialog open={open} onOpenChange={setOpen} title="New deal" description="..." footer={<Buttons/>}>
 *     form fields
 *   </Dialog>
 */
export interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: React.ReactNode;
  description?: React.ReactNode;
  footer?: React.ReactNode;
  size?: "sm" | "md" | "lg" | "xl";
  /** Blocks closing by Escape or outside click (e.g. while saving). */
  dismissable?: boolean;
  className?: string;
  children?: React.ReactNode;
}

const widths = { sm: "md:max-w-sm", md: "md:max-w-lg", lg: "md:max-w-2xl", xl: "md:max-w-4xl" };

// Phones: small and medium dialogs are bottom sheets (up to 92dvh), large ones take the whole screen.
const phoneShape = {
  sm: "max-md:max-h-[92dvh] max-md:rounded-t-xl",
  md: "max-md:max-h-[92dvh] max-md:rounded-t-xl",
  lg: "max-md:top-0 max-md:h-dvh max-md:border-0",
  xl: "max-md:top-0 max-md:h-dvh max-md:border-0",
};

// Sticky footer; on phones the buttons share the width and clear the home indicator.
const footerClass = "flex flex-wrap items-center justify-end gap-2 border-t border-line px-5 py-3 " +
  "max-md:pb-[max(0.75rem,env(safe-area-inset-bottom))] max-md:[&>button]:flex-1";

const overlayClass = "fixed inset-0 z-50 bg-black/40 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0";

function Header({ title, description }: { title: React.ReactNode; description?: React.ReactNode }) {
  return (
    <div className="flex items-start gap-3 border-b border-line px-5 py-3.5">
      <div className="min-w-0 flex-1">
        <DialogPrimitive.Title className="text-base font-semibold text-fg">{title}</DialogPrimitive.Title>
        {description ? (
          <DialogPrimitive.Description className="mt-0.5 text-[13px] text-fg-muted">{description}</DialogPrimitive.Description>
        ) : (
          <DialogPrimitive.Description className="sr-only">{title}</DialogPrimitive.Description>
        )}
      </div>
      <DialogPrimitive.Close aria-label="Close"
        className="-mr-1 flex size-11 shrink-0 items-center justify-center rounded-lg text-fg-muted hover:bg-surface-2 hover:text-fg md:size-8">
        <X className="size-4" aria-hidden />
      </DialogPrimitive.Close>
    </div>
  );
}

export function Dialog({ open, onOpenChange, title, description, footer, size = "md", dismissable = true, className, children }: DialogProps) {
  const block = (e: Event) => { if (!dismissable) e.preventDefault(); };
  return (
    <DialogPrimitive.Root open={open} onOpenChange={onOpenChange}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className={overlayClass} />
        <DialogPrimitive.Content onEscapeKeyDown={block} onPointerDownOutside={block} onInteractOutside={block}
          className={cn("fixed z-50 flex flex-col border-line bg-surface-raised text-fg shadow-pop focus:outline-none",
            "inset-x-0 bottom-0 w-full border-t max-md:data-[state=open]:animate-in max-md:data-[state=open]:slide-in-from-bottom",
            phoneShape[size],
            "md:inset-auto md:left-1/2 md:top-1/2 md:max-h-[calc(100dvh-2rem)] md:w-[calc(100vw-2rem)] md:-translate-x-1/2 md:-translate-y-1/2 md:rounded-lg md:border",
            "md:data-[state=open]:animate-in md:data-[state=open]:fade-in-0 md:data-[state=open]:zoom-in-[0.98]",
            widths[size], className)}>
          <Header title={title} description={description} />
          <div className={cn("min-h-0 flex-1 overflow-y-auto px-5 py-4", !footer && "max-md:pb-[max(1rem,env(safe-area-inset-bottom))]")}>{children}</div>
          {footer && <div className={footerClass}>{footer}</div>}
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

export interface SheetProps extends Omit<DialogProps, "size"> {
  /** right: a side panel on desktop; bottom: a sheet from the bottom (mobile menus). */
  side?: "right" | "bottom";
  width?: string;
}

/** A panel that slides in from the right (or up from the bottom), for long forms and pickers. */
export function Sheet({ open, onOpenChange, title, description, footer, side = "right", width = "min(560px, 100vw)", dismissable = true, className, children }: SheetProps) {
  const block = (e: Event) => { if (!dismissable) e.preventDefault(); };
  return (
    <DialogPrimitive.Root open={open} onOpenChange={onOpenChange}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className={overlayClass} />
        <DialogPrimitive.Content onEscapeKeyDown={block} onPointerDownOutside={block} onInteractOutside={block}
          style={side === "right" ? { width } : undefined}
          className={cn("fixed z-50 flex flex-col bg-surface-raised text-fg shadow-pop focus:outline-none",
            side === "right"
              ? "inset-y-0 right-0 border-l border-line data-[state=open]:animate-in data-[state=open]:slide-in-from-right"
              : "inset-x-0 bottom-0 max-h-[92dvh] rounded-t-xl border-t border-line data-[state=open]:animate-in data-[state=open]:slide-in-from-bottom",
            className)}>
          <Header title={title} description={description} />
          <div className={cn("min-h-0 flex-1 overflow-y-auto px-5 py-4", !footer && "max-md:pb-[max(1rem,env(safe-area-inset-bottom))]")}>{children}</div>
          {footer && <div className={footerClass}>{footer}</div>}
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

export const DialogClose = DialogPrimitive.Close;
