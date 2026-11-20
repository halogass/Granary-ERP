import { describe, expect, it } from "vitest";
import { invoiceTypeLabel } from "./invoiceTypes";

describe("invoiceTypeLabel", () => {
  it("names the known invoice types", () => {
    expect(invoiceTypeLabel("credit_note")).toBe("Credit note");
    expect(invoiceTypeLabel("proforma")).toBe("Proforma");
    expect(invoiceTypeLabel("commercial")).toBe("Commercial");
  });

  it("never shows a raw snake_case code", () => {
    expect(invoiceTypeLabel("down_payment")).toBe("Down payment");
    expect(invoiceTypeLabel(null)).toBe("-");
  });
});
