import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

import ProposalCard from "./ProposalCard";
import type { Proposal } from "./types";

vi.mock("@/lib/api", () => ({ naryApi: {}, remindersApi: {}, researchApi: {}, salesApi: {}, financeApi: {}, kbApi: {} }));
vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a>,
}));

// 1 Nov 2026: an Owner-approved discount on a draft quotation card, and the reprice / withdraw / cancel cards.
describe("quotation cards", () => {
  it("shows the discount and takes it off the total", () => {
    const card = {
      kind: "quotation", currency: "JPY", customer: "Northwind Traders", discount: "10% off, Discount line on the PDF",
      discount_amount: 200000,
      payload: { currency: "JPY", discount_type: "percentage", discount_value: "10",
        items: [{ name: "Laptop", quantity: 2, unit_price: 1000000, amount: 2000000 }] },
    } as unknown as Proposal;
    render(<ProposalCard proposal={card} />);
    const line = screen.getByLabelText("Discount");
    expect(line.textContent).toContain("10% off");
    expect(line.textContent).toContain("200,000");
    expect(screen.getByText(/1,800,000/)).toBeInTheDocument();
  });

  it("lists a reprice card's prices before and after", () => {
    const card = {
      kind: "nary_action", action: "quotation.reprice", summary: "Reprice quotation Q-1 to today (2026-11-01)",
      payload: { quotation_id: "q1" },
      details: ["Issue date: 2026-10-01 → 2026-11-01", "Unit prices, before → after (JPY):",
        "  Laptop: 1,000,000 → 1,250,000 (catalog price of the new date)"],
    } as unknown as Proposal;
    render(<ProposalCard proposal={card} />);
    expect(screen.getByText("Reprice quotation Q-1 to today (2026-11-01)")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "What changes" }).textContent).toContain("1,250,000");
  });
});
