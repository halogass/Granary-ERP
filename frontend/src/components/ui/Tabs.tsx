"use client";

import { forwardRef, useCallback } from "react";
import * as TabsPrimitive from "@radix-ui/react-tabs";
import { cn } from "@/lib/utils";
import { useScrollFade } from "./useScrollFade";

/** Sections of one page (detail page tabs). For switching saved list filters use SavedViewTabs. */
export const Tabs = TabsPrimitive.Root;

/** Scrolls sideways when the tabs do not fit (phones), with a fade on the side where more tabs hide. */
export const TabsList = forwardRef<HTMLDivElement, React.ComponentPropsWithoutRef<typeof TabsPrimitive.List>>(
  function TabsList({ className, style, onScroll, ...props }, ref) {
    const fade = useScrollFade<HTMLDivElement>();
    const setRef = useCallback((el: HTMLDivElement | null) => {
      fade.ref.current = el;
      if (typeof ref === "function") ref(el);
      else if (ref) ref.current = el;
    }, [ref, fade.ref]);
    return (
      <TabsPrimitive.List ref={setRef} style={{ ...fade.style, ...style }}
        onScroll={(e) => { fade.onScroll(); onScroll?.(e); }}
        className={cn("flex gap-1 overflow-x-auto shadow-[inset_0_-1px_0_rgb(var(--line))] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden [&>*]:shrink-0", className)} {...props} />
    );
  },
);

export const tabTriggerClass =
  "relative inline-flex min-h-11 items-center gap-1.5 whitespace-nowrap border-b-2 border-transparent px-3 text-sm text-fg-muted " +
  "transition-colors hover:text-fg md:min-h-9 " +
  "focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[rgb(var(--focus)/0.7)] " +
  "data-[state=active]:border-brand data-[state=active]:font-semibold data-[state=active]:text-fg " +
  "aria-[current=page]:border-brand aria-[current=page]:font-semibold aria-[current=page]:text-fg";

export const TabsTrigger = forwardRef<HTMLButtonElement, React.ComponentPropsWithoutRef<typeof TabsPrimitive.Trigger>>(
  function TabsTrigger({ className, ...props }, ref) {
    return <TabsPrimitive.Trigger ref={ref} className={cn(tabTriggerClass, className)} {...props} />;
  },
);

export const TabsContent = forwardRef<HTMLDivElement, React.ComponentPropsWithoutRef<typeof TabsPrimitive.Content>>(
  function TabsContent({ className, ...props }, ref) {
    return <TabsPrimitive.Content ref={ref} className={cn("pt-4 focus-visible:outline-none", className)} {...props} />;
  },
);
