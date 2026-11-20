// @vitest-environment node
import { afterEach, describe, expect, it, vi } from "vitest";
import { GET } from "./route";

function branding(favicon_data: string) {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ favicon_data }), { status: 200 })));
}

const b64 = (s: string) => Buffer.from(s).toString("base64");

describe("GET /api/favicon", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("serves a PNG favicon with a sandboxing CSP and nosniff", async () => {
    branding(`data:image/png;base64,${b64("\x89PNG\r\n\x1a\nxx")}`);
    const res = await GET();
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("image/png");
    expect(res.headers.get("content-security-policy")).toBe("default-src 'none'; sandbox");
    expect(res.headers.get("x-content-type-options")).toBe("nosniff");
  });

  it.each(["image/x-icon", "image/vnd.microsoft.icon", "image/jpeg"])("serves %s", async (type) => {
    branding(`data:${type};base64,${b64("icon")}`);
    expect((await GET()).status).toBe(200);
  });

  it.each(["image/svg+xml", "text/html", "application/javascript", "image/svg+xml;charset=utf-8"])(
    "refuses a stored %s favicon (404, never served on this origin)", async (type) => {
      branding(`data:${type};base64,${b64("<svg onload=alert(1)>")}`);
      const res = await GET();
      expect(res.status).toBe(404);
    });
});
