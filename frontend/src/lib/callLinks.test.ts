import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { callPageUrl } from "./callLinks";

describe("call links", () => {
  it("send staff to the member call page, joining the room, with audio-only kept", () => {
    expect(callPageUrl("abc_123")).toBe("/calls?join=abc_123");
    expect(callPageUrl("abc_123", true)).toBe("/calls?join=abc_123&audio=1");
  });

  it("the Messages call buttons no longer open the guest page", () => {
    const src = readFileSync(join(__dirname, "..", "app", "(erp)", "messaging", "page.tsx"), "utf8");
    expect(src).toContain("callPageUrl(roomId, audioOnly)");
    expect(src).not.toMatch(/window\.open\(`\/meet\//);
  });

  it("the call page reads ?audio=1 and takes the room size from the server", () => {
    const src = readFileSync(join(__dirname, "..", "app", "(erp)", "calls", "page.tsx"), "utf8");
    expect(src).toContain('searchParams.get("audio") === "1"');
    expect(src).not.toMatch(/max_participants:\s*10/);
  });
});
