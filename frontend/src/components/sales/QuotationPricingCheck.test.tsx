import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const pricingOverride = vi.fn();
vi.mock("@/lib/api", () => ({ salesApi: { pricingOverride: (...a: unknown[]) => pricingOverride(...a) } }));

import QuotationPricingCheck, { type PricingCheck } from "./QuotationPricingCheck";

const BLOCKED: PricingCheck = {
  blocking: [{ code: "below_min_cost_multiple", message: "WIDGET-1 (Widget): USD 9,999.00 per unit is below 2x its cost; at least USD 10,000.00 per unit." }],
  warnings: [{ code: "cost_unknown", message: "Cost unknown for RULES-SW-MO." }],
  override: null,
  refused_at_approval: true,
};

describe("quotation pricing check (E7)", () => {
  beforeEach(() => pricingOverride.mockReset());

  it("shows nothing when the prices pass", () => {
    const { container } = render(
      <QuotationPricingCheck quotationId="q1" number="Q-1" status="draft" check={{ blocking: [], warnings: [] }} onChange={() => {}} />,
    );
    expect(container.textContent).toBe("");
  });

  it("lists what blocks the approval and the warnings, and lets the Owner override with a reason", async () => {
    pricingOverride.mockResolvedValue({ data: {} });
    const onChange = vi.fn();
    render(<QuotationPricingCheck quotationId="q1" number="Q-1" status="draft" check={BLOCKED} onChange={onChange} />);
    expect(screen.getByText(/approval refuses these prices/)).toBeTruthy();
    expect(screen.getByText(/is below 2x its cost/)).toBeTruthy();
    expect(screen.getByText(/Cost unknown/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: /Override \(Owner\)/ }));
    await userEvent.type(screen.getByRole("textbox"), "Strategic first deal in the region");
    await userEvent.click(screen.getByRole("button", { name: "Save override" }));
    expect(pricingOverride).toHaveBeenCalledWith("q1", "Strategic first deal in the region");
    expect(onChange).toHaveBeenCalled();
  });

  it("says when the Owner overrode, and offers no override on a sent quotation", () => {
    const { rerender } = render(
      <QuotationPricingCheck quotationId="q1" number="Q-1" status="draft"
        check={{ ...BLOCKED, override: { reason: "Agreed with the Owner", by: "Diana", valid: true } }} onChange={() => {}} />,
    );
    expect(screen.getByText(/overridden by the Owner/)).toBeTruthy();
    expect(screen.getByText(/Override by Diana: Agreed with the Owner/)).toBeTruthy();
    rerender(<QuotationPricingCheck quotationId="q1" number="Q-1" status="sent" check={BLOCKED} onChange={() => {}} />);
    expect(screen.queryByRole("button", { name: /Override/ })).toBeNull();
  });
});
