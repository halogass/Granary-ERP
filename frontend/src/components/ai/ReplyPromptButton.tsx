"use client";

import { useEffect, useRef, useState } from "react";
import { Loader2, Sparkles, X } from "lucide-react";

/** One-tap directions for the reply; the user can also write their own. */
export const QUICK_PROMPTS = [
  "Friendly follow-up",
  "Ask for the PO / confirmation",
  "Share our pricing",
  "Propose a meeting",
  "Politely decline",
  "Keep it short",
];

/** What the model is told: the user's direction, plus their own draft when they want it improved. */
export function buildInstruction(prompt: string, draft: string, improve: boolean): string {
  const parts: string[] = [];
  if (prompt.trim()) parts.push(prompt.trim());
  if (improve && draft.trim()) {
    parts.push(`Start from the user's own draft below, keep its intent and facts, and improve it:\n"""\n${draft.trim().slice(0, 6000)}\n"""`);
  }
  return parts.join("\n\n");
}

/**
 * The AI reply button, with a small panel to say what the reply should do before it is drafted.
 * Leaving the panel empty and pressing Draft gives the plain suggestion, as before.
 */
export default function ReplyPromptButton({
  onGenerate, disabled, variant = "light", currentDraft = "",
}: {
  /** Draft a reply with this instruction ("" = no direction). Throw an Error to show its message. */
  onGenerate: (instruction: string) => Promise<void>;
  disabled?: boolean;
  variant?: "light" | "dark";
  /** What is already in the message box, offered as a starting point. */
  currentDraft?: string;
}) {
  const [open, setOpen] = useState(false);
  const [prompt, setPrompt] = useState("");
  const [improve, setImprove] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLTextAreaElement>(null);
  const dark = variant === "dark";
  const hasDraft = currentDraft.trim().length > 0;

  useEffect(() => {
    if (!open) return;
    field.current?.focus();
    setImprove(hasDraft);
    const close = (e: MouseEvent) => { if (box.current && !box.current.contains(e.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  async function generate() {
    setBusy(true);
    setError("");
    try {
      await onGenerate(buildInstruction(prompt, currentDraft, improve && hasDraft));
      setOpen(false);
      setPrompt("");
    } catch (e) {
      setError((e as Error).message || "Couldn't draft a reply.");
    } finally {
      setBusy(false);
    }
  }

  const panel = dark
    ? "bg-surface-raised border-line text-fg"
    : "bg-surface border-line text-fg";
  const chip = dark
    ? "bg-surface-2 text-fg-muted hover:bg-line hover:text-fg"
    : "bg-surface-2 text-fg-muted hover:bg-line";

  return (
    <div className="relative" ref={box}>
      {dark ? (
        <button type="button" onClick={() => setOpen((v) => !v)} disabled={disabled || busy}
          title="Draft a reply with AI" aria-label="Draft a reply with AI" aria-expanded={open}
          className="flex size-11 items-center justify-center rounded-lg bg-surface-2 transition-colors hover:bg-line disabled:cursor-not-allowed disabled:opacity-30 md:size-10">
          <Sparkles aria-hidden className={`w-4 h-4 text-tone-violet ${busy ? "animate-pulse" : ""}`} />
        </button>
      ) : (
        <button type="button" onClick={() => setOpen((v) => !v)} disabled={disabled || busy}
          title="Draft a reply with AI" aria-expanded={open}
          className="flex items-center gap-1.5 text-xs text-fg-muted hover:text-fg transition-colors px-2 py-1 rounded-lg hover:bg-surface-2 disabled:opacity-50">
          <Sparkles aria-hidden className={`w-3.5 h-3.5 text-tone-violet ${busy ? "animate-pulse" : ""}`} />
          Suggest
        </button>
      )}

      {open && (
        <div role="dialog" aria-label="Draft a reply with AI"
          className={`absolute bottom-full mb-2 z-30 w-80 rounded-lg border p-3 shadow-pop ${dark ? "right-0" : "left-0"} ${panel}`}>
          <div className="mb-2 flex items-center justify-between">
            <p className="text-xs font-semibold">What should the reply say?</p>
            <button type="button" onClick={() => setOpen(false)} aria-label="Close"
              className={`rounded p-0.5 text-fg-subtle hover:text-fg`}>
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
          <textarea ref={field} value={prompt} onChange={(e) => setPrompt(e.target.value)} rows={3}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); if (!busy) generate(); }
              if (e.key === "Escape") setOpen(false);
            }}
            placeholder="e.g. Confirm delivery next Tuesday and ask for the signed PO. Optional."
            className={`w-full resize-none rounded-lg px-2.5 py-2 text-xs focus:outline-none ${dark
              ? "bg-surface-2 text-fg placeholder:text-fg-subtle focus:ring-2 focus:ring-brand/20"
              : "border border-line-strong bg-surface text-fg placeholder:text-fg-subtle focus:border-brand focus:ring-2 focus:ring-brand/20"}`} />
          <div className="mt-2 flex flex-wrap gap-1">
            {QUICK_PROMPTS.map((q) => (
              <button key={q} type="button" onClick={() => setPrompt((p) => (p.trim() ? `${p.trim()}. ${q}` : q))}
                className={`rounded-full px-2 py-0.5 text-[11px] transition-colors ${chip}`}>{q}</button>
            ))}
          </div>
          {hasDraft && (
            <label className={`mt-2 flex items-center gap-1.5 text-[11px] text-fg-muted`}>
              <input type="checkbox" checked={improve} onChange={(e) => setImprove(e.target.checked)}
                className="h-3.5 w-3.5 accent-brand" />
              Start from what I&apos;ve typed
            </label>
          )}
          {error && <p className="mt-2 text-[11px] text-tone-red">{error}</p>}
          <div className="mt-2.5 flex items-center justify-between gap-2">
            <p className="text-[11px] text-fg-subtle">
              Replaces the message box. Nothing is sent.
            </p>
            <button type="button" onClick={generate} disabled={busy}
              className="inline-flex shrink-0 items-center gap-1.5 rounded-lg bg-brand px-3 py-1.5 text-xs font-semibold text-brand-on hover:bg-brand-hover disabled:opacity-50">
              {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Sparkles className="h-3.5 w-3.5" />}
              {busy ? "Drafting…" : "Draft reply"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
