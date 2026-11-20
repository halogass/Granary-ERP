import { describe, it, expect } from "vitest";
import { allNavLinks, permAllowed } from "./nav";

// Batch E: the API checks kb:view on the knowledge read routes, so the menu hides them without it.
describe("knowledge base menu links", () => {
  const link = (href: string) => allNavLinks().find((l) => l.href === href)!;

  it("need kb:view or an editing permission", () => {
    for (const href of ["/knowledge/articles", "/knowledge/categories"]) {
      const perm = link(href).perm;
      expect(permAllowed(perm, () => false)).toBe(false);
      expect(permAllowed(perm, (c) => c === "kb:view")).toBe(true);
      expect(permAllowed(perm, (c) => c === "kb:create")).toBe(true);
    }
    expect(permAllowed(link("/knowledge/categories").perm, (c) => c === "kb:manage_categories")).toBe(true);
    expect(permAllowed(link("/knowledge/articles").perm, (c) => c === "kb:manage_categories")).toBe(false);
  });
});
