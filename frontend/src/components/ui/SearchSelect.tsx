"use client";

import { useCallback, useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, Search, X } from "lucide-react";

export interface Option {
  id: string;
  label: string;
  sublabel?: string;
  /** Extra fields the caller needs on selection (e.g. a contact's organization_id). */
  meta?: Record<string, unknown>;
}

/**
 * Where a SearchSelect gets its options.
 *
 * `fetch` runs on open and on every (debounced) keystroke (server-side search), so the
 * list is never capped by the endpoint's default `limit`. `resolve` looks up a single
 * option by id: edit forms are handed only an id (see sales/quotations/[id]/edit/page.tsx),
 * and that record is often outside the first page, so without it the trigger would render
 * an empty box for a client that is in fact set.
 */
export interface OptionSource {
  fetch: (query: string) => Promise<Option[]>;
  resolve?: (id: string) => Promise<Option | null>;
}

const DEBOUNCE_MS = 250;

interface Props {
  value: string;
  onChange: (id: string, option: Option | null) => void;
  source: OptionSource;
  placeholder?: string;
  emptyLabel?: string;
  disabled?: boolean;
  allowClear?: boolean;
  /** Overrides the trigger button styling. Defaults to the standard form-field look. */
  className?: string;
  /** Compact variant for line-item tables. */
  size?: "md" | "sm";
  /** Ids to drop from the results, e.g. an employee cannot report to themselves. */
  excludeIds?: string[];
  /** For a <label htmlFor> (Combobox inside a Field passes it). */
  id?: string;
  "aria-describedby"?: string;
  "aria-invalid"?: boolean;
}

const TRIGGER_BASE =
  "w-full flex items-center gap-2 border border-line-strong rounded-lg bg-surface text-left " +
  "focus:outline-none focus:ring-2 focus:ring-brand/20 focus:border-brand " +
  "transition-all disabled:opacity-50 disabled:cursor-not-allowed";

export default function SearchSelect({
  value,
  onChange,
  source,
  placeholder = "Select…",
  emptyLabel = "No matches found",
  disabled = false,
  allowClear = true,
  className,
  size = "md",
  excludeIds,
  id,
  "aria-describedby": describedBy,
  "aria-invalid": invalid,
}: Props) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [options, setOptions] = useState<Option[]>([]);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<Option | null>(null);
  const [highlight, setHighlight] = useState(0);

  const rootRef = useRef<HTMLDivElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const listboxId = useId();

  // The popover is portalled to <body>: several call sites sit inside an
  // `overflow-x-auto` table or a scrolling modal, which would otherwise clip it.
  // Position is therefore fixed and measured from the trigger.
  // Inside a modal dialog the popover is portalled into the dialog instead (17 Nov 2026): Radix's modal blocks
  // pointer events and focus outside its content, so a list on <body> could be seen but not clicked or typed in.
  // The dialog is transformed, which makes it the containing block of `position: fixed`, so the position is
  // measured relative to it (ox/oy, and its height for a list that opens upwards).
  const [rect, setRect] = useState<{ top: number; left: number; width: number; below: boolean; ox: number; oy: number; vh: number } | null>(null);
  const [host, setHost] = useState<HTMLElement | null>(null);

  const measure = useCallback(() => {
    const el = rootRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const spaceBelow = window.innerHeight - r.bottom;
    const below = spaceBelow > 280 || spaceBelow > r.top;
    const dialog = el.closest<HTMLElement>('[role="dialog"]');
    const d = dialog?.getBoundingClientRect();
    setHost(dialog ?? document.body);
    setRect({
      top: below ? r.bottom + 6 : r.top - 6,
      left: r.left,
      width: r.width,
      below,
      ox: d?.left ?? 0,
      oy: d?.top ?? 0,
      vh: d?.height ?? window.innerHeight,
    });
  }, []);

  // Keep the trigger label in step with `value`, including ids that were never in a
  // fetched page (edit mode). Falls back to whatever the last fetch returned.
  useEffect(() => {
    let cancelled = false;
    if (!value) { setSelected(null); return; }
    if (selected?.id === value) return;
    const known = options.find((o) => o.id === value);
    if (known) { setSelected(known); return; }
    if (!source.resolve) return;
    source.resolve(value)
      .then((o) => { if (!cancelled && o) setSelected(o); })
      .catch(() => {});
    return () => { cancelled = true; };
    // `selected`/`options` are read but must not retrigger the resolve.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value, source]);

  const excludeKey = excludeIds?.join(",") ?? "";
  const runFetch = useCallback((q: string) => {
    setLoading(true);
    const drop = new Set(excludeKey ? excludeKey.split(",") : []);
    source.fetch(q)
      .then((res) => { setOptions(drop.size ? res.filter((o) => !drop.has(o.id)) : res); setHighlight(0); })
      .catch(() => setOptions([]))
      .finally(() => setLoading(false));
  }, [source, excludeKey]);

  // Debounced search while open. The leading fetch (query "") fills the list the moment
  // the popover opens, so it never shows an empty panel.
  useEffect(() => {
    if (!open) return;
    const t = setTimeout(() => runFetch(query), query ? DEBOUNCE_MS : 0);
    return () => clearTimeout(t);
  }, [open, query, runFetch]);

  useEffect(() => {
    if (open) inputRef.current?.focus();
    else setQuery("");
  }, [open]);

  useEffect(() => {
    if (!open) { setRect(null); return; }
    measure();
    function onDocMouseDown(e: MouseEvent) {
      const t = e.target as Node;
      if (!rootRef.current?.contains(t) && !popRef.current?.contains(t)) setOpen(false);
    }
    document.addEventListener("mousedown", onDocMouseDown);
    window.addEventListener("scroll", measure, true);
    window.addEventListener("resize", measure);
    return () => {
      document.removeEventListener("mousedown", onDocMouseDown);
      window.removeEventListener("scroll", measure, true);
      window.removeEventListener("resize", measure);
    };
  }, [open, measure]);

  // Keep the highlighted row inside the scroll viewport when navigating by keyboard.
  useEffect(() => {
    if (!open) return;
    listRef.current?.querySelector<HTMLElement>(`[data-idx="${highlight}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [highlight, open]);

  function pick(o: Option) {
    setSelected(o);
    onChange(o.id, o);
    setOpen(false);
  }

  function clear(e: React.MouseEvent) {
    e.stopPropagation();
    setSelected(null);
    onChange("", null);
  }

  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === "ArrowDown" || (e.key === "Enter" && !open)) {
      e.preventDefault();
      if (!open) { setOpen(true); return; }
      setHighlight((h) => Math.min(h + 1, options.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlight((h) => Math.max(h - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const o = options[highlight];
      if (o) pick(o);
    } else if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
    }
  }

  const pad = size === "sm" ? "px-2.5 py-2 text-xs" : "px-3 py-2.5 md:py-2 text-sm";
  const showClear = allowClear && !!selected && !disabled;

  return (
    <div ref={rootRef} className="relative" onKeyDown={onKeyDown}>
      <button
        type="button"
        id={id}
        aria-describedby={describedBy}
        aria-invalid={invalid}
        role="combobox"
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-controls={open ? listboxId : undefined}
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
        className={className ?? `${TRIGGER_BASE} ${pad}`}
      >
        <span className={`flex-1 truncate ${selected ? "text-fg" : "text-fg-subtle"}`}>
          {selected?.label ?? placeholder}
        </span>
        <ChevronDown className={`w-3.5 h-3.5 text-fg-subtle shrink-0 ${showClear ? "ml-5" : ""}`} />
      </button>

      {showClear && (
        <button
          type="button"
          aria-label="Clear selection"
          onClick={clear}
          className="absolute right-7 top-1/2 -translate-y-1/2 flex size-9 md:right-8 md:size-6 items-center justify-center rounded text-fg-subtle hover:text-fg transition-colors"
        >
          <X className="w-3.5 h-3.5" />
        </button>
      )}

      {open && rect && host && createPortal(
        <div
          ref={popRef}
          style={{
            position: "fixed",
            top: rect.below ? rect.top - rect.oy : undefined,
            bottom: rect.below ? undefined : rect.vh - (rect.top - rect.oy),
            left: rect.left - rect.ox,
            width: Math.max(rect.width, 240),
          }}
          className="z-[60] bg-surface-raised border border-line rounded-lg shadow-pop overflow-hidden">
          <div className="relative border-b border-line">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-fg-subtle" aria-hidden />
            <input type="search" autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
              ref={inputRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search…"
              className="w-full pl-9 pr-3 py-2.5 text-base md:text-sm bg-transparent text-fg focus:outline-none"
            />
          </div>

          <div ref={listRef} id={listboxId} role="listbox" className="max-h-60 overflow-y-auto py-1">
            {loading && options.length === 0 && (
              <p className="px-3 py-2.5 text-xs text-fg-muted">Searching…</p>
            )}
            {!loading && options.length === 0 && (
              <p className="px-3 py-2.5 text-xs text-fg-muted">{emptyLabel}</p>
            )}
            {options.map((o, i) => (
              <button
                key={o.id}
                type="button"
                data-idx={i}
                role="option"
                aria-selected={o.id === value}
                onMouseEnter={() => setHighlight(i)}
                onClick={() => pick(o)}
                className={`w-full min-h-11 md:min-h-0 text-left px-3 py-2 flex items-center gap-2 transition-colors ${
                  i === highlight ? "bg-surface-2" : ""
                }`}
              >
                <span className="flex-1 min-w-0">
                  <span className="block text-sm text-fg truncate">{o.label}</span>
                  {o.sublabel && (
                    <span className="block text-xs text-fg-muted truncate">{o.sublabel}</span>
                  )}
                </span>
                {o.id === value && <Check className="w-3.5 h-3.5 text-brand-ink shrink-0" />}
              </button>
            ))}
          </div>
        </div>,
        host,
      )}
    </div>
  );
}
