/**
 * 2 Nov 2026: what a document's notes say about tax, as the server's notes/tax gate reads them
 * (services/src/quotation_tax.rs). The sentence and both phrase lists come from Settings
 * (GET /sales/tax-notes); the default sentence below is only for when they can't be loaded.
 */

export interface TaxNotes {
  note: string;
  exclusive_phrases: string[];
  inclusive_phrases: string[];
}

export const DEFAULT_TAX_NOTE = "Prices exclude sales tax and withholding tax.";

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Lower case, tags and entities out, one space between words. */
export function normNotes(s: string | null | undefined): string {
  return (s ?? "")
    .replace(/<[^>]*>/g, " ")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/ /g, " ")
    .toLowerCase()
    .replace(/\s+/g, " ")
    .trim();
}

export function hasTaxNote(notes: string | null | undefined, note: string): boolean {
  const n = normNotes(note);
  return !!n && normNotes(notes).includes(n);
}

/** The notes with the sentence added at the end (a paragraph in rich-text notes), once. */
export function withTaxNote(notes: string | null | undefined, note: string): string {
  const cur = (notes ?? "").trim();
  if (hasTaxNote(cur, note)) return cur;
  if (!cur || cur === "<p></p>") return `<p>${note}</p>`;
  return cur.includes("<") ? `${cur}<p>${note}</p>` : `${cur}\n\n${note}`;
}

/** The notes without the sentence (and the empty paragraph it leaves). */
export function withoutTaxNote(notes: string | null | undefined, note: string): string {
  const words = note.trim().split(/\s+/).filter(Boolean).map(escapeRe);
  if (!words.length) return notes ?? "";
  const re = new RegExp(words.join("(?:\\s|&nbsp;)+"), "gi");
  return (notes ?? "")
    .replace(re, "")
    .replace(/<p>(?:\s|&nbsp;|<br\s*\/?>)*<\/p>/g, "")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

export type TaxClaim = "excludes" | "includes" | null;

/** What the notes say: prices exclude tax, include it, or nothing (also when they say both). */
export function taxClaim(notes: string | null | undefined, cfg: TaxNotes): TaxClaim {
  let rest = normNotes(notes);
  if (!rest) return null;
  let excludes = false;
  for (const p of [normNotes(cfg.note), ...cfg.exclusive_phrases.map(normNotes)]) {
    if (p && rest.includes(p)) {
      excludes = true;
      rest = rest.split(p).join(" ");
    }
  }
  const includes = cfg.inclusive_phrases.map(normNotes).some((p) => !!p && rest.includes(p));
  if (excludes && !includes) return "excludes";
  if (includes && !excludes) return "includes";
  return null;
}

/** Why the notes and the line taxes disagree (the approval and the send refuse it), or null. */
export function taxConflict(notes: string | null | undefined, taxedLines: number, lines: number, cfg: TaxNotes | null): string | null {
  if (!cfg) return null;
  const claim = taxClaim(notes, cfg);
  if (claim === "excludes" && taxedLines > 0) {
    return `The notes say prices exclude tax, but ${taxedLines} of ${lines} line${lines === 1 ? "" : "s"} carr${taxedLines === 1 ? "ies" : "y"} tax. Clear the tax (Prices exclude tax) or change the notes: it can't go to approval or be sent like this.`;
  }
  if (claim === "includes" && taxedLines === 0 && lines > 0) {
    return "The notes say prices include tax, but no line carries tax. Give the lines their tax or change the notes: it can't go to approval or be sent like this.";
  }
  return null;
}
