import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  DOWNLOAD_TYPE, canOpenInTab, isSafeImageType, isSafeVideoType, openOrSaveFile, safeBlob, safeType, saveFile,
} from "./safeFile";

describe("safeType", () => {
  it("keeps only allowlisted types", () => {
    expect(safeType("application/pdf")).toBe("application/pdf");
    expect(safeType("IMAGE/PNG; charset=binary")).toBe("image/png");
    expect(safeType("image/jpg")).toBe("image/jpeg");
    expect(safeType("text/plain")).toBe("text/plain");
    expect(safeType("audio/mpeg")).toBe("audio/mpeg");
    expect(safeType("video/mp4")).toBe("video/mp4");
  });

  it("turns script-capable and unknown types into a download", () => {
    for (const t of ["image/svg+xml", "text/html", "application/xhtml+xml", "text/xml", "application/xml",
      "application/javascript", "video/x-matroska", "", null, undefined]) {
      expect(safeType(t)).toBe(DOWNLOAD_TYPE);
    }
  });

  it("never treats SVG as an image", () => {
    expect(isSafeImageType("image/svg+xml")).toBe(false);
    expect(isSafeImageType("image/webp")).toBe(true);
    expect(isSafeVideoType("video/webm")).toBe(true);
    expect(isSafeVideoType("video/quicktime")).toBe(true);
    expect(isSafeVideoType("video/x-msvideo")).toBe(false);
    expect(canOpenInTab("text/html")).toBe(false);
    expect(canOpenInTab("application/pdf")).toBe(true);
  });
});

describe("safeBlob", () => {
  it("rebuilds the blob with the declared type, else the blob's own, through the allowlist", () => {
    const html = new Blob(["<script>alert(1)</script>"], { type: "text/html" });
    expect(safeBlob(html).type).toBe(DOWNLOAD_TYPE);
    expect(safeBlob(html, "text/plain").type).toBe("text/plain");
    const pdf = new Blob(["%PDF"], { type: "application/pdf" });
    const rebuilt = safeBlob(pdf);
    expect(rebuilt).not.toBe(pdf);
    expect(rebuilt.type).toBe("application/pdf");
    expect(rebuilt.size).toBe(pdf.size);
  });
});

describe("openOrSaveFile", () => {
  let open: ReturnType<typeof vi.fn>;
  let created: Blob[];
  let clicks: { href: string; download: string }[];

  beforeEach(() => {
    vi.useFakeTimers();
    created = [];
    clicks = [];
    open = vi.fn();
    vi.stubGlobal("open", open);
    URL.createObjectURL = vi.fn((b: Blob) => { created.push(b); return `blob:test/${created.length}`; });
    URL.revokeObjectURL = vi.fn();
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      clicks.push({ href: this.href, download: this.download });
    });
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("opens a PDF in a new tab", () => {
    expect(openOrSaveFile(new Blob(["%PDF"], { type: "application/pdf" }), "a.pdf")).toBe("opened");
    expect(open).toHaveBeenCalledWith("blob:test/1", "_blank", "noopener");
    expect(created[0].type).toBe("application/pdf");
    expect(clicks).toHaveLength(0);
  });

  it("downloads SVG and HTML instead of opening them", () => {
    const svg = new Blob(['<svg onload="alert(1)"/>'], { type: "image/svg+xml" });
    expect(openOrSaveFile(svg, "x.svg")).toBe("saved");
    const html = new Blob(["<h1>x</h1>"], { type: "text/plain" });
    // the record says HTML: the declared type wins and is refused
    expect(openOrSaveFile(html, "x.html", "text/html")).toBe("saved");
    expect(open).not.toHaveBeenCalled();
    expect(created.map((b) => b.type)).toEqual([DOWNLOAD_TYPE, DOWNLOAD_TYPE]);
    expect(clicks.map((c) => c.download)).toEqual(["x.svg", "x.html"]);
  });

  it("saveFile never keeps a dangerous type", () => {
    saveFile(new Blob(["<x/>"], { type: "text/html" }), "page.html");
    expect(created[0].type).toBe(DOWNLOAD_TYPE);
    expect(clicks[0].download).toBe("page.html");
    expect(open).not.toHaveBeenCalled();
  });
});
