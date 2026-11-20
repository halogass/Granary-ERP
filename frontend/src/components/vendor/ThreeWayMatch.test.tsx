import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

import ThreeWayMatch from "./ThreeWayMatch";

const getMatching = vi.fn();
vi.mock("@/lib/api", () => ({ p2pApi: { getMatching: (...a: unknown[]) => getMatching(...a) } }));

const RESULT = {
  po_id: "po1", po_number: "PO-0001", po_status: "received", po_total: 14000, po_currency: "USD",
  total_invoiced: { USD: 14000 }, overall_match: "over", invoice_count: 1, receipt_count: 2,
  lines: [
    { po_item_id: "i1", component_sku: "SKU-1", description: "Cable", quantity_ordered: 10, quantity_received: 10.5,
      quantity_invoiced: 10, unit_price: 1000, currency: "USD", match_status: "over", over_received: true,
      over_invoiced: false, receipt_pct: 105, invoice_pct: 100 },
    { po_item_id: "i2", component_sku: null, description: "Bracket", quantity_ordered: 4, quantity_received: 4,
      quantity_invoiced: 4, unit_price: 1000, currency: "USD", match_status: "matched", over_received: false,
      over_invoiced: false, receipt_pct: 100, invoice_pct: 100 },
  ],
};

describe("ThreeWayMatch", () => {
  beforeEach(() => { getMatching.mockReset(); });

  it("loads GET /vendor/purchase-orders/{id}/matching and flags the over-received line", async () => {
    getMatching.mockResolvedValue({ data: RESULT });
    render(<ThreeWayMatch poId="po1" />);
    await waitFor(() => expect(getMatching).toHaveBeenCalledWith("po1"));
    expect(await screen.findByText("Cable")).toBeTruthy();
    expect(screen.getByText("Over-received")).toBeTruthy();
    expect(screen.getByText("Matched")).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toContain("1 line received or invoiced beyond the ordered quantity");
  });

  it("shows the API's reason when the match cannot load", async () => {
    getMatching.mockRejectedValue({ response: { data: { detail: "Purchase order not found" } } });
    render(<ThreeWayMatch poId="nope" />);
    expect(await screen.findByText("Purchase order not found")).toBeTruthy();
  });
});
