import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  usePathname: () => "/knowledge/categories",
  useSearchParams: () => new URLSearchParams(),
}));
// The shape GET /kb/categories returns: no is_active field.
vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  kbApi: {
    listCategories: () => Promise.resolve({ data: [
      { id: "c1", name: "Operations", slug: "operations", access_level: "all", parent_id: null, icon: null },
      { id: "c2", name: "Shipping", slug: "shipping", access_level: "all", parent_id: "c1", icon: null, sort_order: 2 },
      { id: "c4", name: "Receiving", slug: "receiving", access_level: "all", parent_id: "c1", icon: null, sort_order: 1 },
      { id: "c3", name: "Finance", slug: "finance", access_level: "all", parent_id: null, icon: null },
    ] }),
    createCategory: vi.fn(),
  },
}));

import KBCategoriesPage from "./page";

describe("knowledge categories", () => {
  it("does not mark categories inactive when the API sends no active flag", async () => {
    render(<KBCategoriesPage />);
    expect(await screen.findByText("Operations")).toBeInTheDocument();
    expect(screen.getByText("Finance")).toBeInTheDocument();
    expect(screen.queryByText(/inactive/i)).toBeNull();
  });

  it("orders sub-categories by their saved sort order", async () => {
    render(<KBCategoriesPage />);
    await screen.findByText("Shipping");
    const names = screen.getAllByText(/^(Shipping|Receiving)$/).map((n) => n.textContent);
    expect(names).toEqual(["Receiving", "Shipping"]);
  });
});
