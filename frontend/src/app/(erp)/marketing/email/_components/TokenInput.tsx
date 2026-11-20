"use client";

import { useEffect, useRef, useState } from "react";
import { ChevronDown, UserRound } from "lucide-react";
import { useFieldControl } from "@/components/ui";

/** Recipient placeholders the sender fills in per email (see services/email_marketing.py). */
export const PLACEHOLDERS = [
  { token: "{{first_name}}", label: "First name" },
  { token: "{{full_name}}", label: "Full name" },
  { token: "{{company}}", label: "Company (recipient's)" },
  { token: "{{email}}", label: "Email address" },
];

interface Props {
  value: string;
  onChange: (value: string) => void;
  className: string;
  multiline?: boolean;
  rows?: number;
  placeholder?: string;
  disabled?: boolean;
}

/** An input / textarea with a "Personalize" menu that inserts a placeholder at the cursor. */
export default function TokenInput({ value, onChange, className, multiline, rows = 4, placeholder, disabled }: Props) {
  const fieldRef = useRef<HTMLInputElement & HTMLTextAreaElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const selection = useRef<[number, number]>([value.length, value.length]);
  const [open, setOpen] = useState(false);
  const ctl = useFieldControl({});

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => { if (!menuRef.current?.contains(e.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);

  function remember() {
    const el = fieldRef.current;
    if (el) selection.current = [el.selectionStart ?? value.length, el.selectionEnd ?? value.length];
  }

  function insert(token: string) {
    const [start, end] = selection.current;
    const from = Math.min(start, value.length);
    const to = Math.min(Math.max(end, from), value.length);
    onChange(value.slice(0, from) + token + value.slice(to));
    setOpen(false);
    const caret = from + token.length;
    selection.current = [caret, caret];
    requestAnimationFrame(() => {
      const el = fieldRef.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(caret, caret);
    });
  }

  const common = {
    ref: fieldRef, value, placeholder, disabled, className, id: ctl.id, "aria-describedby": ctl["aria-describedby"],
    onChange: (e: React.ChangeEvent<HTMLInputElement & HTMLTextAreaElement>) => { onChange(e.target.value); remember(); },
    onSelect: remember, onKeyUp: remember, onClick: remember, onBlur: remember,
  };

  return (
    <div>
      {multiline ? <textarea rows={rows} {...common} /> : <input {...common} />}
      {!disabled && (
        <div ref={menuRef} className="relative mt-1">
          <button type="button" onClick={() => setOpen((o) => !o)} aria-expanded={open}
            className="inline-flex min-h-8 items-center gap-1 text-xs font-medium text-fg-muted hover:text-brand-ink md:min-h-0">
            <UserRound className="size-3" aria-hidden /> Personalize <ChevronDown className="size-3" aria-hidden />
          </button>
          {open && (
            <div className="absolute z-20 mt-1 w-56 rounded-lg border border-line bg-surface-raised py-1 shadow-pop">
              {PLACEHOLDERS.map((p) => (
                <button key={p.token} type="button" onClick={() => insert(p.token)}
                  className="flex w-full items-center justify-between gap-2 px-3 py-1.5 text-left text-xs hover:bg-surface-2">
                  <span className="text-fg">{p.label}</span>
                  <code className="font-mono text-[11px] text-fg-subtle">{p.token}</code>
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
