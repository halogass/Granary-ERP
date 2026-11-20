"use client";

import { useRef, useState } from "react";
import Link from "next/link";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
import { Check, Copy } from "lucide-react";
import "katex/dist/katex.min.css";

const FENCE_OR_CODE = /(```[\s\S]*?(?:```|$)|`[^`\n]*`)/g;

/**
 * Model output → Markdown this renderer understands.
 * - Drops `<think>` blocks some OpenAI-compatible models (Qwen, R1 via Ollama) put in the answer,
 *   including one still open while the answer streams.
 * - Turns `\( … \)` / `\[ … \]` math (how OpenAI and DeepSeek write it) into `$$` math. Single `$`
 *   stays plain text, so "$100 and $200" is never read as a formula.
 */
export function prepareMarkdown(text: string): string {
  // "[[next]]" separates the parts Nary sends one at a time on WhatsApp; on screen it's a divider.
  const withoutThink = (text || "").replace(/<think>[\s\S]*?(<\/think>|$)/g, "").trimStart()
    .replace(/\s*\[\[next\]\]\s*/gi, "\n\n---\n\n");
  return withoutThink
    .split(FENCE_OR_CODE)
    .map((part, i) => (i % 2 ? part : part
      .replace(/\\\[([\s\S]+?)\\\]/g, (_, m) => `\n$$\n${m.trim()}\n$$\n`)
      .replace(/\\\(([\s\S]+?)\\\)/g, (_, m) => `$$${m.trim()}$$`)))
    .join("");
}

function CodeBlock({ children }: { children?: React.ReactNode }) {
  const ref = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(ref.current?.innerText || "");
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch { /* clipboard unavailable */ }
  };
  return (
    <div className="relative my-2 group">
      <pre ref={ref}
        className="overflow-x-auto rounded-lg border border-line bg-surface text-fg text-xs leading-relaxed p-3 pr-10 [&_code]:bg-transparent [&_code]:p-0 [&_code]:text-inherit">
        {children}
      </pre>
      <button type="button" onClick={copy} title="Copy"
        className="absolute top-1.5 right-1.5 p-1.5 rounded-md text-fg-muted bg-surface-2 [@media(hover:hover)]:opacity-0 [@media(hover:hover)]:group-hover:opacity-100 [@media(hover:hover)]:focus:opacity-100 hover:bg-line hover:text-fg [@media(hover:none)]:p-2.5">
        {copied ? <Check size={13} /> : <Copy size={13} />}
      </button>
    </div>
  );
}

const components: Components = {
  h1: ({ children }) => <h3 className="text-base font-semibold text-fg mt-3 mb-1.5">{children}</h3>,
  h2: ({ children }) => <h4 className="text-[15px] font-semibold text-fg mt-3 mb-1.5">{children}</h4>,
  h3: ({ children }) => <h5 className="text-sm font-semibold text-fg mt-2.5 mb-1">{children}</h5>,
  h4: ({ children }) => <h6 className="text-sm font-semibold text-fg mt-2 mb-1">{children}</h6>,
  p: ({ children }) => <p className="my-1.5 leading-relaxed">{children}</p>,
  ul: ({ children }) => <ul className="my-1.5 pl-5 list-disc space-y-0.5 marker:text-fg-subtle">{children}</ul>,
  ol: ({ children, start }) => (
    <ol start={start} className="my-1.5 pl-5 list-decimal space-y-0.5 marker:text-fg-subtle">{children}</ol>
  ),
  li: ({ children }) => <li className="leading-relaxed [&>ul]:my-0.5 [&>ol]:my-0.5">{children}</li>,
  blockquote: ({ children }) => (
    <blockquote className="my-2 border-l-2 border-line-strong pl-3 text-fg-muted italic">{children}</blockquote>
  ),
  hr: () => <hr className="my-3 border-line" />,
  strong: ({ children }) => <strong className="font-semibold text-fg">{children}</strong>,
  a: ({ href, children }) => {
    const url = href || "";
    // In-app links stay in the app; anything else opens beside it so the chat isn't lost.
    if (url.startsWith("/") && !url.startsWith("//")) {
      return <Link href={url} className="text-brand-ink underline underline-offset-2">{children}</Link>;
    }
    return (
      <a href={url} target="_blank" rel="noopener noreferrer"
        className="text-brand-ink underline underline-offset-2 break-words">{children}</a>
    );
  },
  table: ({ children }) => (
    <div className="my-2 overflow-x-auto rounded-lg border border-line">
      <table className="w-full border-collapse text-xs">{children}</table>
    </div>
  ),
  thead: ({ children }) => <thead className="bg-surface-2">{children}</thead>,
  th: ({ children, style }) => (
    <th style={style} className="border-b border-line px-2.5 py-1.5 text-left font-semibold text-fg whitespace-nowrap">{children}</th>
  ),
  td: ({ children, style }) => (
    <td style={style} className="border-t border-line px-2.5 py-1.5 align-top text-fg">{children}</td>
  ),
  pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
  code: ({ className, children }) => (
    <code className={`rounded bg-surface-2 px-1 py-0.5 font-mono text-[0.85em] ${className || ""}`}>{children}</code>
  ),
};

/** An assistant answer, styled for the chat bubble (no typography plugin needed). */
export default function MarkdownMessage({ content }: { content: string }) {
  return (
    <div className="text-sm text-fg break-words [&>*:first-child]:mt-0 [&>*:last-child]:mb-0">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, [remarkMath, { singleDollarTextMath: false }]]}
        rehypePlugins={[rehypeKatex]}
        components={components}
      >
        {prepareMarkdown(content)}
      </ReactMarkdown>
    </div>
  );
}
