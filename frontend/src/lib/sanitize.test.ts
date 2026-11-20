import { describe, expect, it } from "vitest";
import { safeHref } from "./sanitize";

describe("safeHref", () => {
  it("allows http(s), mailto and tel", () => {
    expect(safeHref("https://example.com/x")).toBe("https://example.com/x");
    expect(safeHref("mailto:hello@example.com")).toBe("mailto:hello@example.com");
    expect(safeHref("tel:+15550100199")).toBe("tel:+15550100199");
  });

  it("keeps same-origin relative paths", () => {
    expect(safeHref("/api/v1/attachments/1")).toBe("/api/v1/attachments/1");
  });

  it("upgrades a bare domain to https", () => {
    expect(safeHref("example.com")).toBe("https://example.com");
    expect(safeHref("www.example.com/pricing")).toBe("https://www.example.com/pricing");
  });

  it("blocks script-bearing and protocol-relative URLs", () => {
    expect(safeHref("javascript:alert(1)")).toBeUndefined();
    expect(safeHref("  javascript:alert(1)")).toBeUndefined();
    expect(safeHref("java\tscript:alert(1)")).toBeUndefined();
    expect(safeHref("data:text/html;base64,PHNjcmlwdD4=")).toBeUndefined();
    expect(safeHref("vbscript:msgbox(1)")).toBeUndefined();
    expect(safeHref("//evil.example.com")).toBeUndefined();
  });

  it("returns undefined for empty input", () => {
    expect(safeHref(undefined)).toBeUndefined();
    expect(safeHref(null)).toBeUndefined();
    expect(safeHref("   ")).toBeUndefined();
  });
});
