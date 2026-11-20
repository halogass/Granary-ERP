"use client";

import { useCallback, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Components } from "react-markdown";
import {
  Bold, Code2, Eye, Heading1, Heading2, Heading3, Italic, List, ListOrdered, Minus, Plus, Quote, Table2, X,
} from "lucide-react";
import { Button, Card, IconButton, Input, Pill, Switch } from "@/components/ui";
import { cn } from "@/lib/utils";

/** Shared pieces of the knowledge base's new and edit article pages. */

export interface SopStep {
  step: number;
  title: string;
  description: string;
  responsible: string;
}

export const ACCESS_OPTIONS = [
  { value: "all_staff", label: "All staff" },
  { value: "department", label: "Department only" },
  { value: "management", label: "Management only" },
  { value: "restricted", label: "Restricted" },
];

/** Markdown rendering for previews and article pages, in the theme's tokens. */
export const MD_PREVIEW: Components = {
  h1: ({ children }) => <h1 className="mb-3 mt-6 border-b border-line pb-2 text-xl font-semibold text-fg first:mt-0">{children}</h1>,
  h2: ({ children }) => <h2 className="mb-2 mt-5 text-lg font-semibold text-fg">{children}</h2>,
  h3: ({ children }) => <h3 className="mb-2 mt-4 text-base font-semibold text-fg">{children}</h3>,
  p: ({ children }) => <p className="mb-3 text-sm leading-relaxed text-fg">{children}</p>,
  ul: ({ children }) => <ul className="mb-3 list-outside list-disc space-y-1 pl-5 text-sm">{children}</ul>,
  ol: ({ children }) => <ol className="mb-3 list-outside list-decimal space-y-1 pl-5 text-sm">{children}</ol>,
  li: ({ children }) => <li className="leading-relaxed text-fg">{children}</li>,
  blockquote: ({ children }) => <blockquote className="my-3 rounded-r-lg border-l-4 border-brand/40 bg-surface-2 py-2 pl-4 text-sm italic text-fg-muted">{children}</blockquote>,
  pre: ({ children }) => <pre className="mb-3 overflow-x-auto rounded-lg border border-line bg-surface-2 p-4 text-xs leading-relaxed text-fg">{children}</pre>,
  code: ({ children, className }) => className
    ? <code className="font-mono text-xs">{children}</code>
    : <code className="rounded border border-line bg-surface-2 px-1.5 py-0.5 font-mono text-xs text-brand-ink">{children}</code>,
  table: ({ children }) => <div className="mb-3 overflow-x-auto"><table className="w-full border-collapse text-xs">{children}</table></div>,
  th: ({ children }) => <th className="border border-line bg-surface-2 px-3 py-2 text-left font-semibold text-fg">{children}</th>,
  td: ({ children }) => <td className="border border-line px-3 py-2 text-fg">{children}</td>,
  hr: () => <hr className="my-4 border-0 border-t border-line" />,
  strong: ({ children }) => <strong className="font-semibold text-fg">{children}</strong>,
  em: ({ children }) => <em className="italic">{children}</em>,
  a: ({ href, children }) => <a href={href} target="_blank" rel="noopener noreferrer" className="text-sm text-brand-ink hover:underline">{children}</a>,
};

/** The Markdown body: a formatting toolbar, Write / Preview. The textarea keeps id "md-editor". */
export function MarkdownEditor({ value, onChange, placeholder, emptyPreview = "Nothing to preview" }: {
  value: string; onChange: (v: string) => void; placeholder?: string; emptyPreview?: string;
}) {
  const [tab, setTab] = useState<"write" | "preview">("write");

  const applyMarkdown = useCallback((before: string, after = "") => {
    const ta = document.getElementById("md-editor") as HTMLTextAreaElement | null;
    if (!ta) return;
    const start = ta.selectionStart;
    const end = ta.selectionEnd;
    const selected = ta.value.slice(start, end);
    const replacement = `${before}${selected || "text"}${after}`;
    onChange(ta.value.slice(0, start) + replacement + ta.value.slice(end));
    requestAnimationFrame(() => {
      ta.focus();
      ta.setSelectionRange(start + before.length, start + before.length + (selected || "text").length);
    });
  }, [onChange]);

  const toolbarItems = [
    { icon: Heading1, label: "Heading 1", action: () => applyMarkdown("# ") },
    { icon: Heading2, label: "Heading 2", action: () => applyMarkdown("## ") },
    { icon: Heading3, label: "Heading 3", action: () => applyMarkdown("### ") },
    { divider: true },
    { icon: Bold, label: "Bold", action: () => applyMarkdown("**", "**") },
    { icon: Italic, label: "Italic", action: () => applyMarkdown("*", "*") },
    { divider: true },
    { icon: List, label: "Bullet list", action: () => applyMarkdown("- ") },
    { icon: ListOrdered, label: "Numbered list", action: () => applyMarkdown("1. ") },
    { icon: Quote, label: "Quote", action: () => applyMarkdown("> ") },
    { divider: true },
    { icon: Code2, label: "Code", action: () => applyMarkdown("`", "`") },
    { icon: Table2, label: "Table", action: () => applyMarkdown("| Col 1 | Col 2 |\n|---|---|\n| ", " | |\n") },
    { icon: Minus, label: "Horizontal rule", action: () => applyMarkdown("\n---\n") },
  ] as const;

  return (
    <Card className="flex min-h-[420px] flex-1 flex-col">
      <div className="flex flex-shrink-0 flex-wrap items-center gap-0.5 border-b border-line px-2 py-1.5">
        {tab === "write" && toolbarItems.map((item, idx) =>
          "divider" in item ? (
            <span key={idx} className="mx-1 h-5 w-px bg-line" aria-hidden />
          ) : (
            <IconButton key={idx} icon={item.icon} label={item.label} size="sm" onClick={item.action} className="text-fg-muted" />
          ),
        )}
        <div className="ml-auto flex items-center gap-1" role="group" aria-label="Editor mode">
          {(["write", "preview"] as const).map((t) => (
            <Button key={t} size="sm" variant="ghost" aria-pressed={tab === t} onClick={() => setTab(t)}
              className={cn(tab === t ? "bg-surface-2 text-fg" : "text-fg-muted")}>
              {t === "write" ? <Code2 aria-hidden /> : <Eye aria-hidden />}
              {t === "write" ? "Write" : "Preview"}
            </Button>
          ))}
        </div>
      </div>
      {tab === "write" ? (
        <textarea
          id="md-editor"
          aria-label="Article body (Markdown)"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          className="min-h-[360px] w-full flex-1 resize-none bg-transparent px-5 py-4 font-mono text-sm leading-relaxed text-fg placeholder:text-fg-subtle focus:outline-none"
        />
      ) : (
        <div className="flex-1 overflow-auto px-6 py-5">
          {value ? (
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_PREVIEW}>{value}</ReactMarkdown>
          ) : (
            <p className="text-sm italic text-fg-muted">{emptyPreview}</p>
          )}
        </div>
      )}
    </Card>
  );
}

/** Structured SOP steps: title, responsible party, description. */
export function SopStepsEditor({ steps, onChange, emptyText }: {
  steps: SopStep[]; onChange: (fn: (s: SopStep[]) => SopStep[]) => void; emptyText: string;
}) {
  const set = (idx: number, k: keyof SopStep, v: string) => onChange((s) => s.map((st, i) => (i === idx ? { ...st, [k]: v } : st)));
  return (
    <div>
      <div className="mb-2 flex items-center justify-between">
        <p className="text-[13px] font-medium text-fg">Procedure steps</p>
        <Button size="sm" variant="ghost" onClick={() => onChange((s) => [...s, { step: s.length + 1, title: "", description: "", responsible: "" }])}>
          <Plus aria-hidden /> Add step
        </Button>
      </div>
      {steps.length === 0 ? (
        <p className="text-xs italic text-fg-muted">{emptyText}</p>
      ) : (
        <ol className="space-y-2">
          {steps.map((step, idx) => (
            <li key={idx} className="flex items-start gap-2 rounded-lg bg-surface-2 p-2.5">
              <span className="mt-2 flex size-6 shrink-0 items-center justify-center rounded-full bg-surface text-xs font-semibold text-fg tabular">
                {step.step}
              </span>
              <div className="grid flex-1 grid-cols-1 gap-2 md:grid-cols-3">
                <Input value={step.title} onChange={(e) => set(idx, "title", e.target.value)} placeholder="Step title" aria-label={`Step ${step.step} title`} />
                <Input value={step.responsible} onChange={(e) => set(idx, "responsible", e.target.value)} placeholder="Responsible party" aria-label={`Step ${step.step} responsible`} />
                <Input value={step.description} onChange={(e) => set(idx, "description", e.target.value)} placeholder="Description" aria-label={`Step ${step.step} description`} />
              </div>
              <IconButton icon={X} size="sm" label={`Remove step ${step.step}`} className="mt-0.5 hover:text-tone-red"
                onClick={() => onChange((s) => s.filter((_, i) => i !== idx).map((st, i) => ({ ...st, step: i + 1 })))} />
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

/** "Only visible to me". */
export function PrivateSwitch({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center gap-3">
      <Switch label="Only visible to me" checked={checked} onCheckedChange={onChange} />
      {checked && <Pill tone="orange">Private</Pill>}
    </div>
  );
}
