import { describe, it, expect } from "vitest";
import {
  DOC_VIEWS, fileKind, filenameFromDisposition, formatFileSize, languageLabel, listParams, needsApproval, nextLabel,
  productLabel, productsPayload, uploadProblem, versionStatusLabel,
} from "./documentLibrary";

const MB = 1024 * 1024;

describe("document library helpers", () => {
  it("suggests the next label like the server: last number + 1, rest kept", () => {
    const table: [string | null, string][] = [
      [null, "v1"], ["", "v1"], ["v33", "v34"], ["v2.1", "v2.2"], ["3", "4"], ["Rev B2", "Rev B3"],
      ["Final", "Final 2"], ["v09", "v10"], ["v2.9", "v2.10"], ["2024-03 draft", "2024-04 draft"],
    ];
    for (const [prev, next] of table) expect(nextLabel(prev), String(prev)).toBe(next);
  });

  it("maps every saved view to the API's view and leaves empty filters out", () => {
    expect(DOC_VIEWS.map((v) => v.id)).toEqual(["all", "mine", "customer", "partner", "internal", "needs_review", "pending"]);
    expect(listParams({ view: "pending" })).toEqual({ view: "pending", limit: 50, offset: 0 });
    expect(listParams({ view: "customer", category: "products", language: "de", product_id: "p1" }, "  brochure ", 3, 20)).toEqual({
      view: "customer", category: "products", language: "de", product_id: "p1", q: "brochure", limit: 20, offset: 40,
    });
    expect(listParams({}).view).toBe("all");
  });

  it("asks for executive approval only when customers or partners see it", () => {
    expect(needsApproval(["internal"])).toBe(false);
    expect(needsApproval(["internal", "customer"])).toBe(true);
    expect(needsApproval(["partner"])).toBe(true);
    expect(needsApproval([])).toBe(false);
  });

  it("checks uploads: PDF or image, a PDF with a source, the size limit", () => {
    const pdf = { name: "brochure.pdf", type: "application/pdf", size: 2 * MB };
    const png = { name: "logo.png", type: "image/png", size: MB };
    const docx = { name: "brochure.docx", type: "", size: MB };
    expect(uploadProblem(null, null, 100)).toMatch(/Choose/);
    expect(uploadProblem(pdf, null, 100)).toBeNull();
    expect(uploadProblem(png, null, 100)).toBeNull();
    expect(uploadProblem(docx, null, 100)).toMatch(/source/);
    expect(uploadProblem({ name: "a.zip", size: 1 }, null, 100)).toMatch(/PDF, an image .* or a video/);
    // 5 Nov 2026: videos, on their own cap
    expect(uploadProblem({ name: "demo.mp4", type: "video/mp4", size: 150 * MB }, null, 100, 200)).toBeNull();
    expect(uploadProblem({ name: "demo.mov", size: 201 * MB }, null, 100, 200)).toMatch(/video is larger than 200 MB/);
    expect(uploadProblem({ name: "demo.webm", type: "video/webm", size: 1 }, docx, 100, 200)).toMatch(/needs its PDF/);
    expect(uploadProblem(pdf, docx, 100)).toBeNull();
    expect(uploadProblem(png, docx, 100)).toMatch(/needs its PDF/);
    expect(uploadProblem(pdf, png, 100)).toMatch(/DOCX, XLSX or PPTX/);
    expect(uploadProblem({ ...pdf, size: 101 * MB }, null, 100)).toMatch(/larger than 100 MB/);
    expect(uploadProblem(pdf, { ...docx, size: 6 * MB }, 5)).toMatch(/source file is larger than 5 MB/);
  });

  it("tells a PDF from an image and an office source", () => {
    expect(fileKind("x.PDF")).toBe("pdf");
    expect(fileKind("scan", "image/jpeg")).toBe("image");
    expect(fileKind("deck.pptx")).toBe("office");
    expect(fileKind("notes.txt")).toBe("other");
  });

  it("reads the file name from Content-Disposition", () => {
    expect(filenameFromDisposition('attachment; filename="Company Profile v3.pdf"', "x.pdf")).toBe("Company Profile v3.pdf");
    expect(filenameFromDisposition("attachment; filename*=UTF-8''Brochure%20Laptop.pdf", "x.pdf")).toBe("Brochure Laptop.pdf");
    expect(filenameFromDisposition(null, "x.pdf")).toBe("x.pdf");
  });

  it("formats sizes, languages, products and version statuses", () => {
    expect(formatFileSize(512)).toBe("512 B");
    expect(formatFileSize(2048)).toBe("2 KB");
    expect(formatFileSize(3.5 * MB)).toBe("3.5 MB");
    expect(formatFileSize(null)).toBe("-");
    expect(languageLabel("ms")).toBe("Malay");
    expect(languageLabel("pt")).toBe("PT");
    expect(productLabel({ id: "1", name: "Laptop Pro 14", sku: "LAP-P14" })).toBe("Laptop Pro 14 (LAP-P14)");
    expect(productLabel({ family: "Laptop" })).toBe("Family: Laptop");
    expect(productsPayload([{ id: "1", name: "x" }, { family: "Laptop" }, {}])).toEqual([{ product_id: "1" }, { family: "Laptop" }]);
    expect(versionStatusLabel("pending_approval")).toBe("Waiting for approval");
    expect(versionStatusLabel("live")).toBe("Live");
  });

  it("keeps em dashes out of the copy", () => {
    const copy = [...DOC_VIEWS.map((v) => v.label), uploadProblem(null, null, 1) ?? ""];
    for (const t of copy) expect(t).not.toMatch(/—/);
  });
});
