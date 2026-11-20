"use client";
/**
 * 4 Nov 2026: the quotation editor's house notes: "Fill notes from template" (the template of the lines'
 * product family, Settings > Sales & CRM > Quotation notes templates) and a panel with what the notes check finds
 * (missing clauses, not numbered, an accuracy figure, a delay penalty, a discount, tax). The approval refuses
 * notes with a blocking finding; this panel shows them before saving.
 */
import { useEffect, useRef, useState } from "react";
import { AlertTriangle, CheckCircle2, Loader2, Wand2 } from "lucide-react";
import { salesApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import { Button, Select } from "@/components/ui";

export interface NotesLine {
  product_id?: string | null;
  name?: string | null;
  description?: string | null;
  quantity?: number | null;
  unit_price?: number | null;
  is_optional?: boolean;
  tax_id?: string | null;
  tax_pct?: number | null;
}

export interface NotesContext {
  /** The saved quotation being edited: the check fills what the form leaves out from it, as the approval does. */
  quotation_id?: string;
  notes: string;
  items: NotesLine[];
  currency: string;
  subject?: string;
  customer_org_id?: string;
  validity_days?: number;
  discount_type?: string;
  discount_value?: number;
  discount_display?: string;
}

export interface NotesIssue { code: string; message: string; blocking: boolean }
export interface NotesCheck { family: string; family_name: string; missing: string[]; ok: boolean; issues: NotesIssue[] }
export interface NotesTemplate { family: string; family_name: string; preview_html: string | null; payment_terms?: string | null }

/** The notes the template fills when the form's notes are empty or the user agrees to replace them. */
export function shouldReplace(current: string): boolean {
  const plain = (current ?? "").replace(/<[^>]*>/g, "").replace(/&nbsp;/g, " ").trim();
  return !plain || window.confirm("Replace the current notes with the house template? The deal clauses (scope, delivery...) still need to be written.");
}

export default function QuotationNotesHelper({ ctx, onFill, delayMs = 600 }: {
  ctx: NotesContext;
  /** The template's notes and its Payment Terms text. */
  onFill: (html: string, paymentTerms: string | null) => void;
  delayMs?: number;
}) {
  const [check, setCheck] = useState<NotesCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const [filling, setFilling] = useState(false);
  const [language, setLanguage] = useState("en");
  const [error, setError] = useState("");
  const key = JSON.stringify(ctx);
  const seq = useRef(0);

  useEffect(() => {
    if (!ctx.items.length) { setCheck(null); return; }
    const n = ++seq.current;
    const t = setTimeout(() => {
      setChecking(true);
      salesApi.quotationNotesCheck(ctx)
        .then((r) => { if (n === seq.current) setCheck(r.data as NotesCheck); })
        .catch(() => { if (n === seq.current) setCheck(null); })
        .finally(() => { if (n === seq.current) setChecking(false); });
    }, delayMs);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, delayMs]);

  async function fill() {
    setError("");
    if (!ctx.items.length) { setError("Add the lines first: the template follows their product family."); return; }
    if (!shouldReplace(ctx.notes)) return;
    setFilling(true);
    try {
      const r = await salesApi.quotationNotesTemplate({ ...ctx, language });
      const t = r.data as NotesTemplate;
      if (t.preview_html) onFill(t.preview_html, t.payment_terms ?? null);
    } catch (e) {
      setError(extractApiError(e, "Could not load the notes template."));
    } finally {
      setFilling(false);
    }
  }

  const issues = check?.issues ?? [];
  return (
    <div className="space-y-2" data-testid="notes-helper">
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" variant="secondary" size="sm" onClick={fill} disabled={filling}>
          {filling ? <Loader2 className="size-3.5 animate-spin" aria-hidden /> : <Wand2 className="size-3.5" aria-hidden />}
          Fill notes from template
        </Button>
        <Select aria-label="Notes language" value={language} onChange={(e) => setLanguage(e.target.value)}
          options={[{ value: "en", label: "English" }]} className="w-36" />
        {check && <span className="text-xs text-fg-muted">Template: {check.family_name}</span>}
        {checking && <Loader2 className="size-3.5 animate-spin text-fg-muted" aria-label="Checking the notes" />}
      </div>
      {error && <p className="text-xs text-tone-red">{error}</p>}
      {check && issues.length > 0 && (
        <div role="alert" className="rounded-md border border-tone-orange/40 bg-tone-orange-soft px-3 py-2 text-xs">
          <p className="mb-1 flex items-center gap-1.5 font-medium">
            <AlertTriangle className="size-3.5" aria-hidden />
            {issues.some((i) => i.blocking) ? "The approval will refuse these notes" : "Notes warnings"}
          </p>
          <ul className="list-disc space-y-0.5 pl-5">
            {issues.map((i, n) => <li key={n}>{i.message}</li>)}
          </ul>
          {check.missing.length > 0 && (
            <p className="mt-1">Missing clauses: {check.missing.join(", ")}</p>
          )}
        </div>
      )}
      {check && issues.length === 0 && (
        <p className="flex items-center gap-1.5 text-xs text-fg-muted">
          <CheckCircle2 className="size-3.5" aria-hidden /> The notes follow the house template.
        </p>
      )}
    </div>
  );
}
