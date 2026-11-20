import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const getQuotation = vi.fn();
const forEntity = vi.fn();
const withdrawQuotation = vi.fn();

vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock("@/lib/api", () => ({
  salesApi: {
    getQuotation: (...a: unknown[]) => getQuotation(...a),
    withdrawQuotation: (...a: unknown[]) => withdrawQuotation(...a),
  },
  approvalsApi: { forEntity: (...a: unknown[]) => forEntity(...a) },
}));
// The form itself is tested on its own: here only what the page hands it.
vi.mock("@/components/sales/QuotationForm", () => ({
  default: ({ lockedReason }: { lockedReason?: string }) => (
    <div>form{lockedReason ? `: ${lockedReason}` : ", can save"}</div>
  ),
  savedItemToLine: (i: unknown) => i,
}));

import EditQuotationPage from "./page";

beforeEach(() => {
  getQuotation.mockResolvedValue({ data: {
    id: "q1", number: "Q-1", full_number: "Q-2026-11-00001-1", status: "draft", currency: "JPY", items: [],
    issue_date: "2026-11-02", valid_until: "2026-11-16",
  } });
  withdrawQuotation.mockResolvedValue({ data: { status: "draft" } });
});

describe("Edit quotation (2 Nov 2026)", () => {
  it("says a draft waiting for approval must be withdrawn first, with Withdraw right there", async () => {
    forEntity.mockResolvedValue({ data: { request: { status: "pending" } } });
    render(<EditQuotationPage params={{ id: "q1" }} />);
    expect(await screen.findByText("Waiting for approval: withdraw it first")).toBeInTheDocument();
    expect(screen.getByText(/Q-2026-11-00001-1 is waiting for approval: withdraw it first/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /withdraw from approval/i }));
    await waitFor(() => expect(withdrawQuotation).toHaveBeenCalledWith("q1", expect.any(String)));
    await waitFor(() => expect(screen.queryByText("Waiting for approval: withdraw it first")).toBeNull());
    expect(screen.getByText("form, can save")).toBeInTheDocument();
  });

  it("edits a draft nobody is approving as before", async () => {
    forEntity.mockResolvedValue({ data: { request: null } });
    render(<EditQuotationPage params={{ id: "q1" }} />);
    expect(await screen.findByText("form, can save")).toBeInTheDocument();
    expect(screen.queryByText(/withdraw it first/i)).toBeNull();
  });
});
