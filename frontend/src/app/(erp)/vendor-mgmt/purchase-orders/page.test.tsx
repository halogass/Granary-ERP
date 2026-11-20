import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import PurchaseOrdersPage from "./page";

const getMatching = vi.fn();
vi.mock("@/lib/api", () => ({
  vendorApi: {
    listPOs: () => Promise.resolve({ data: [
      { id: "po1", number: "PO-0001", vendor_id: "v1", vendor_name: "Acme", status: "received", total: 100,
        currency: "USD", created_at: "2026-11-01T00:00:00Z" },
    ] }),
    listVendors: () => Promise.resolve({ data: [{ id: "v1", name: "Acme" }] }),
    deletePO: vi.fn(), createPO: vi.fn(),
  },
  p2pApi: { getMatching: (...a: unknown[]) => getMatching(...a) },
  systemApi: { getCurrencySettings: () => Promise.resolve({ data: { functional: "SGD", active: ["SGD", "JPY"] } }) },
  default: { get: vi.fn(), post: vi.fn() },
}));
vi.mock("@/lib/entitySources", () => ({ vendorSource: {} }));
vi.mock("@/components/approvals/useSendDocument", () => ({ useSendDocument: () => ({ modal: null, preparing: null, send: vi.fn() }) }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/vendor-mgmt/purchase-orders", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

describe("purchase orders: Matching tab", () => {
  it("opens the three-way match of the expanded order", async () => {
    getMatching.mockResolvedValue({ data: {
      po_id: "po1", po_number: "PO-0001", po_total: 100, po_currency: "USD", total_invoiced: {},
      overall_match: "partial", invoice_count: 0, receipt_count: 1, lines: [],
    } });
    render(<PurchaseOrdersPage />);
    fireEvent.click((await screen.findAllByText("PO-0001"))[0]);
    const tab = await screen.findByRole("tab", { name: "Matching" });
    fireEvent.mouseDown(tab);
    fireEvent.click(tab);
    await waitFor(() => expect(getMatching).toHaveBeenCalledWith("po1"));
    expect(await screen.findByText("This order has no lines.")).toBeTruthy();
  });
});
