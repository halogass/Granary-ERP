"use client";
// Lightweight dependency-free WYSIWYG editor (contentEditable + execCommand).
// Emits sanitized HTML. Used for document notes (quotations, invoices, templates).
import { useEffect, useRef } from "react";
import { sanitizeHtml } from "@/lib/sanitize";
import { Bold, Italic, Underline, List, ListOrdered, Link2, Eraser } from "lucide-react";

export default function RichTextEditor({ value, onChange, placeholder, minHeight = 120 }: {
  value: string;
  onChange: (html: string) => void;
  placeholder?: string;
  minHeight?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);

  // Sync external value into the DOM only when it differs (avoids caret jumps while typing).
  // The value can come from a stored record or a shared note template, so it is sanitized
  // every time before it becomes live DOM (an <img onerror> would otherwise run here).
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const clean = sanitizeHtml(value || "");
    if (el.innerHTML !== clean) el.innerHTML = clean;
  }, [value]);

  function emit() {
    if (ref.current) onChange(sanitizeHtml(ref.current.innerHTML));
  }
  function cmd(command: string, arg?: string) {
    ref.current?.focus();
    document.execCommand(command, false, arg);
    emit();
  }
  function addLink() {
    const url = window.prompt("Link URL", "https://");
    if (url) cmd("createLink", url);
  }

  const Btn = ({ onClick, title, children }: { onClick: () => void; title: string; children: React.ReactNode }) => (
    <button type="button" onMouseDown={(e) => e.preventDefault()} onClick={onClick} title={title} aria-label={title}
      className="flex size-9 items-center justify-center rounded-md text-fg-muted transition-colors hover:bg-surface-2 hover:text-fg md:size-7">
      {children}
    </button>
  );

  return (
    <div className="overflow-hidden rounded-lg border border-line-strong bg-surface transition-colors focus-within:border-brand/50 focus-within:ring-2 focus-within:ring-brand/20">
      <div className="flex flex-wrap items-center gap-0.5 border-b border-line bg-surface-2 px-2 py-1">
        <Btn onClick={() => cmd("bold")} title="Bold"><Bold className="size-3.5" aria-hidden /></Btn>
        <Btn onClick={() => cmd("italic")} title="Italic"><Italic className="size-3.5" aria-hidden /></Btn>
        <Btn onClick={() => cmd("underline")} title="Underline"><Underline className="size-3.5" aria-hidden /></Btn>
        <span className="mx-1 h-4 w-px bg-line-strong" aria-hidden />
        <Btn onClick={() => cmd("insertUnorderedList")} title="Bullet list"><List className="size-3.5" aria-hidden /></Btn>
        <Btn onClick={() => cmd("insertOrderedList")} title="Numbered list"><ListOrdered className="size-3.5" aria-hidden /></Btn>
        <Btn onClick={addLink} title="Insert link"><Link2 className="size-3.5" aria-hidden /></Btn>
        <span className="mx-1 h-4 w-px bg-line-strong" aria-hidden />
        <Btn onClick={() => cmd("removeFormat")} title="Clear formatting"><Eraser className="size-3.5" aria-hidden /></Btn>
      </div>
      <div
        ref={ref}
        contentEditable
        suppressContentEditableWarning
        onInput={emit}
        onBlur={emit}
        data-placeholder={placeholder}
        role="textbox" aria-multiline="true" aria-label={placeholder || "Text"}
        className="rte-content px-4 py-3 text-sm leading-relaxed text-fg focus:outline-none [&_a]:text-brand-ink [&_a]:underline [&_ol]:list-decimal [&_ol]:pl-5 [&_ul]:list-disc [&_ul]:pl-5"
        style={{ minHeight }}
      />
      <style jsx>{`
        .rte-content:empty:before { content: attr(data-placeholder); color: rgb(var(--fg-subtle)); }
      `}</style>
    </div>
  );
}
