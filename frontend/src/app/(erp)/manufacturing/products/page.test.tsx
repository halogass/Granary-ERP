import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import ManufacturingProductsPage from "./page";

const updateProduct = vi.fn((_id: string, body: { serial_prefix?: string | null }) =>
  Promise.resolve({ data: { id: "p1", serial_prefix: body.serial_prefix ?? null } }));
vi.mock("@/lib/api", () => ({
  manufacturingApi: {
    listProducts: () => Promise.resolve({ data: [
      { id: "p1", product_code: "LAP-T", name: "Laptop Pro 14", category: "hardware", currency: "USD", is_active: true,
        serial_prefix: null },
    ] }),
    getBOM: () => Promise.reject(new Error("none")),
    updateProduct: (...a: unknown[]) => updateProduct(...(a as [string, { serial_prefix?: string | null }])),
  },
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/manufacturing/products", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));
vi.mock("@/components/approvals/ApprovalPanel", () => ({ default: () => null }));
vi.mock("@/components/catalog/ProductLineSelect", () => ({
  ProductLineSelect: () => null,
  useProductLines: () => [],
  lineLookup: () => () => null,
}));

describe("hardware products: the serial prefix (3 Nov 2026)", () => {
  it("edits the prefix in the list: letters only, upper case, saved on blur", async () => {
    render(<ManufacturingProductsPage />);
    // look the input up each time: on a slow machine the list re-renders after the data loads
    const prefix = () => screen.getAllByLabelText("Serial prefix of Laptop Pro 14")[0] as HTMLInputElement;
    await screen.findAllByLabelText("Serial prefix of Laptop Pro 14");
    expect(prefix().value).toBe("");
    await waitFor(() => {
      fireEvent.change(prefix(), { target: { value: "ds-k1" } });
      expect(prefix().value).toBe("DSK");
    });
    fireEvent.blur(prefix());
    await waitFor(() => expect(updateProduct).toHaveBeenCalledWith("p1", { serial_prefix: "DSK" }));
  });

  it("asks for it when a product is created", async () => {
    render(<ManufacturingProductsPage />);
    await screen.findAllByLabelText("Serial prefix of Laptop Pro 14");
    fireEvent.click(screen.getAllByRole("button", { name: /new product|add product/i })[0]);
    expect(await screen.findByText(/its units' serials are generated as DSK-XXXXX-XXXXX/)).toBeTruthy();
  });
});
