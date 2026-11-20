import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { OptionalTotals } from "./QuotationOptionalTotals";
import { proposalOptionalSubtotal, proposalTotal } from "@/components/ai/ProposalCard";

const fmt = (n: number) => `JPY ${n.toLocaleString("en-US")}`;
const items = [{ is_optional: false }, { is_optional: false }, { is_optional: true }];

describe("OptionalTotals (quotation page)", () => {
  it("shows the optional items' subtotal, their tax and the total with them under the total", () => {
    render(<OptionalTotals fmt={fmt} quote={{ total: 218300000, items, optional_subtotal: 31200000, optional_tax: 3432000,
      total_with_optional: 252932000, optional_in_total: false }} />);
    const block = screen.getByTestId("optional-totals");
    expect(block).toHaveTextContent("Optional items (not in the total)JPY 31,200,000");
    expect(block).toHaveTextContent("Their taxJPY 3,432,000");
    expect(block).toHaveTextContent("Total with optional itemsJPY 252,932,000");
  });

  it("says an older quotation's total includes them", () => {
    render(<OptionalTotals fmt={fmt} quote={{ total: 249500000, items, optional_subtotal: 31200000, optional_tax: 0,
      total_with_optional: 249500000, optional_in_total: true }} />);
    expect(screen.getByTestId("optional-totals")).toHaveTextContent(/this total includes them/);
  });

  it("renders nothing without optional lines", () => {
    const { container } = render(<OptionalTotals fmt={fmt} quote={{ total: 1, items: [{ is_optional: false }] }} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("chat quotation card totals", () => {
  it("keeps optional lines out of the total and sums them apart", () => {
    const lines = [
      { name: "Laptop set", quantity: 1, unit_price: 150000000 },
      { name: "Installation", quantity: 1, unit_price: 68300000 },
      { name: "Analytics dashboard", quantity: 1, unit_price: 31200000, is_optional: true },
    ];
    expect(proposalTotal(lines)).toBe(218300000);
    expect(proposalOptionalSubtotal(lines)).toBe(31200000);
  });
});
