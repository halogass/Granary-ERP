import { describe, it, expect } from "vitest";
import { sanitizeEmailHtml } from "./sanitize";

describe("email bodies with inline images", () => {
  it("turns an unresolvable cid: image into a note instead of a failed request", () => {
    const out = sanitizeEmailHtml('<p>Hi</p><img src="cid:_Foxmail.1@a2be" alt="logo" width="80">');
    expect(out).not.toContain("cid:");
    expect(out).toContain('alt="[inline image]"');
    expect(out).toContain('width="80"');
  });

  it("keeps embedded and normal images, and still strips scripts", () => {
    const html = '<img src="data:image/png;base64,iVBORw0KGgo="><img src="https://x.test/a.png"><script>alert(1)</script>';
    const out = sanitizeEmailHtml(html);
    expect(out).toContain("data:image/png;base64");
    expect(out).toContain("https://x.test/a.png");
    expect(out).not.toContain("<script");
  });
});
