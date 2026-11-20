import { describe, it, expect } from "vitest";
import { taxClaim, taxConflict, withTaxNote, withoutTaxNote, hasTaxNote, type TaxNotes } from "./taxNotes";

const NOTE = "Prices exclude VAT and withholding tax.";
const cfg: TaxNotes = {
  note: NOTE,
  exclusive_phrases: ["exclude vat", "excluding vat"],
  inclusive_phrases: ["including vat", "vat included"],
};

describe("tax notes (2 Nov 2026)", () => {
  it("reads what the notes say, as the server does", () => {
    expect(taxClaim(`<p>${NOTE}</p>`, cfg)).toBe("excludes");
    expect(taxClaim("Prices excluding VAT", cfg)).toBe("excludes");
    expect(taxClaim("Prices including VAT", cfg)).toBe("includes");
    expect(taxClaim("Delivery in 4 weeks", cfg)).toBeNull();
  });

  it("adds the sentence once and takes it out again", () => {
    const on = withTaxNote("<p>Delivery in 4 weeks.</p>", NOTE);
    expect(on).toBe(`<p>Delivery in 4 weeks.</p><p>${NOTE}</p>`);
    expect(withTaxNote(on, NOTE)).toBe(on);
    expect(hasTaxNote(on, NOTE)).toBe(true);
    expect(withoutTaxNote(on, NOTE)).toBe("<p>Delivery in 4 weeks.</p>");
    expect(withTaxNote("", NOTE)).toBe(`<p>${NOTE}</p>`);
  });

  it("flags notes and taxes that disagree", () => {
    expect(taxConflict(NOTE, 2, 3, cfg)).toMatch(/2 of 3 lines carry tax/);
    expect(taxConflict(NOTE, 0, 3, cfg)).toBeNull();
    expect(taxConflict("Prices including VAT", 0, 2, cfg)).toMatch(/include tax/);
    expect(taxConflict(NOTE, 2, 3, null)).toBeNull();
    expect(taxConflict(NOTE, 1, 1, cfg)).not.toContain("—");
  });
});
