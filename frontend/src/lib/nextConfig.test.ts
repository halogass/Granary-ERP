// @vitest-environment node
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// Security fix L5: the image optimiser (/_next/image) must not fetch internal hosts for anyone.
describe("next.config.mjs images", () => {
  const text = readFileSync(fileURLToPath(new URL("../../next.config.mjs", import.meta.url)), "utf8");
  const images = /images:\s*\{([\s\S]*?)\n {2}\},/.exec(text)?.[1] ?? "";

  it("has an images block", () => {
    expect(images).toContain("remotePatterns");
  });

  it("allows no internal or plain-http remote image host", () => {
    expect(images).not.toMatch(/localhost|minio|127\.0\.0\.1|protocol:\s*"http"/);
  });
});
