import { describe, it, expect } from "vitest";
import { csvCell, csvName, toCsv } from "./csvExport";

// Batch E: the Export buttons on the quotation and invoice lists save the current list as CSV.
describe("CSV export", () => {
  it("quotes cells that need it and keeps numbers as numbers", () => {
    expect(csvCell("Acme, Ltd")).toBe('"Acme, Ltd"');
    expect(csvCell('Say "hi"')).toBe('"Say ""hi"""');
    expect(csvCell("two\nlines")).toBe('"two\nlines"');
    expect(csvCell(1250.5)).toBe("1250.5");
    expect(csvCell(-3)).toBe("-3");
    expect(csvCell(null)).toBe("");
    expect(csvCell(undefined)).toBe("");
  });

  it("never lets a text cell start a spreadsheet formula", () => {
    expect(csvCell("=HYPERLINK(\"x\")")).toBe("\"'=HYPERLINK(\"\"x\"\")\"");
    expect(csvCell("+1")).toBe("'+1");
    expect(csvCell("-1")).toBe("'-1");
    expect(csvCell("@SUM(A1)")).toBe("'@SUM(A1)");
  });

  it("writes a header row and one line per row", () => {
    const csv = toCsv([{ n: "Q-1", t: 10 }, { n: "Q-2", t: 20 }], [
      { header: "Number", value: (r) => r.n },
      { header: "Total", value: (r) => r.t },
    ]);
    expect(csv).toBe("Number,Total\r\nQ-1,10\r\nQ-2,20\r\n");
  });

  it("names the file by date", () => {
    expect(csvName("invoices", new Date("2026-11-20T10:00:00Z"))).toBe("invoices-2026-11-20.csv");
  });
});
