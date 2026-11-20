"use client";

import { useCallback, useEffect, useRef, useState } from "react";

const FADE = "24px";

/**
 * For a row that scrolls sideways (tabs, chips): fades the edge where more items hide, and brings
 * the active item (aria-selected, aria-current, data-state=active, aria-pressed) into view.
 *
 *   const fade = useScrollFade<HTMLDivElement>(activeKey);
 *   <div ref={fade.ref} onScroll={fade.onScroll} style={fade.style} className="overflow-x-auto">...</div>
 */
export function useScrollFade<E extends HTMLElement>(activeKey?: unknown) {
  const ref = useRef<E | null>(null);
  const [edges, setEdges] = useState({ start: false, end: false });

  const measure = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const start = el.scrollLeft > 1;
    const end = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
    setEdges((e) => (e.start === start && e.end === end ? e : { start, end }));
  }, []);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [measure]);

  useEffect(() => {
    const el = ref.current;
    const active = el?.querySelector<HTMLElement>(
      '[aria-selected="true"],[aria-current="page"],[data-state="active"]',
    );
    if (!el || !active || el.scrollWidth <= el.clientWidth) return;
    // scroll only this row, never the page
    const box = el.getBoundingClientRect();
    const a = active.getBoundingClientRect();
    const left = a.left - box.left + el.scrollLeft;
    if (left < el.scrollLeft || left + a.width > el.scrollLeft + el.clientWidth) {
      el.scrollTo?.({ left: Math.max(0, left - 24), behavior: "smooth" });
    }
    measure();
  }, [activeKey, measure]);

  const mask = edges.start || edges.end
    ? `linear-gradient(to right, ${edges.start ? "transparent" : "black"}, black ${FADE}, black calc(100% - ${FADE}), ${edges.end ? "transparent" : "black"})`
    : undefined;
  return {
    ref,
    onScroll: measure,
    style: mask ? { maskImage: mask, WebkitMaskImage: mask } as React.CSSProperties : undefined,
    edges,
  };
}
