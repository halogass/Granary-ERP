import { describe, it, expect } from "vitest";
import { prepareEmailHtml } from "./emailFrame";

const body = (doc: string) => new DOMParser().parseFromString(doc, "text/html");

describe("prepareEmailHtml (security triage 18)", () => {
  it("holds back every kind of remote image by default and counts them", () => {
    const html =
      '<img src="https://track.example/p.gif?u=1" alt="pixel">' +
      '<img srcset="https://x.example/a.png 1x, https://x.example/b.png 2x">' +
      '<table background="https://x.example/bg.jpg"><tr><td style="background-image:url(\'https://x.example/c.png\')">x</td></tr></table>' +
      '<style>@import url(https://x.example/s.css); .h{background:url("https://x.example/d.png")}</style>' +
      '<img src="data:image/png;base64,iVBORw0KGgo=">';
    const { doc, blocked } = prepareEmailHtml(html, false);
    expect(blocked).toBe(6);
    expect(doc).not.toContain("x.example");
    expect(doc).not.toContain("track.example");
    expect(doc).toContain("data:image/png;base64,iVBORw0KGgo=");
    // The frame refuses remote loads on its own too.
    const csp = body(doc).querySelector('meta[http-equiv="Content-Security-Policy"]')?.getAttribute("content");
    expect(csp).toContain("img-src data:;");
    expect(csp).toContain("script-src 'none'");
  });

  it("keeps remote images when asked", () => {
    const { doc, blocked } = prepareEmailHtml('<img src="https://x.example/a.png">', true);
    expect(blocked).toBe(1);
    expect(doc).toContain('src="https://x.example/a.png"');
    expect(doc).toContain("img-src data: https: http:");
  });

  it("keeps the email's style tags inside the frame document, strips scripts, opens links in a new tab", () => {
    const html = '<style>body{display:none} .x{color:red}</style><p class="x">Hi <a href="https://example.com">site</a></p><script>alert(1)</script>';
    const { doc } = prepareEmailHtml(html, false);
    const d = body(doc);
    expect(d.querySelectorAll("body style")).toHaveLength(1);
    expect(d.querySelector("body style")?.textContent).toContain("display:none");
    expect(doc).not.toContain("<script");
    expect(d.querySelector("base")?.getAttribute("target")).toBe("_blank");
    const a = d.querySelector("a")!;
    expect(a.getAttribute("target")).toBe("_blank");
    expect(a.getAttribute("rel")).toBe("noopener noreferrer");
  });

  it("an email without remote content needs no image bar", () => {
    expect(prepareEmailHtml("<p>Plain</p>", false).blocked).toBe(0);
  });
});
