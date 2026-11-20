import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import CommissionsPage from "./page";

const post = vi.fn(() => Promise.resolve({ data: {} }));
const patch = vi.fn(() => Promise.resolve({ data: {} }));
let rows: unknown[] = [];
vi.mock("@/lib/api", () => ({
  default: { post: (...a: unknown[]) => post(...(a as [])), patch: (...a: unknown[]) => patch(...(a as [])) },
  partnerApi: {
    listCommissions: () => Promise.resolve({ data: rows }),
    listAccounts: () => Promise.resolve({ data: [{ id: "p1", company_name: "Harbor Resellers" }] }),
    listDealRegs: () => Promise.resolve({ data: [] }),
  },
}));
vi.mock("@/hooks/usePermissions", () => ({ usePermissions: () => ({ holds: () => true, can: () => true, loaded: true }) }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/partner-mgmt/commissions", useSearchParams: () => new URLSearchParams() }));

// Batch E: nothing creates a commission when a deal is won; the page says so and lets staff record one.
describe("partner commissions", () => {
  it("says honestly how commissions are created", async () => {
    rows = [];
    render(<CommissionsPage />);
    expect(await screen.findByText(/winning a deal does not create one on its own/i)).toBeInTheDocument();
    expect(screen.queryByText(/generated when deal registrations are approved and won/i)).toBeNull();
  });

  it("records a commission with the rate as a fraction", async () => {
    rows = [];
    render(<CommissionsPage />);
    fireEvent.click((await screen.findAllByRole("button", { name: /record commission/i }))[0]);
    await waitFor(() => expect(screen.getByRole("option", { name: "Harbor Resellers" })).toBeInTheDocument());
    fireEvent.change(screen.getByRole("combobox", { name: /^partner/i }), { target: { value: "p1" } });
    fireEvent.change(screen.getByLabelText(/rate \(%\)/i), { target: { value: "12.5" } });
    fireEvent.change(screen.getByLabelText(/base amount/i), { target: { value: "1000" } });
    fireEvent.click(screen.getByRole("button", { name: /^record$/i }));
    await waitFor(() => expect(post).toHaveBeenCalled());
    expect(post.mock.calls[0]).toEqual(["/partner/commissions", expect.objectContaining({ partner_id: "p1", rate: "0.125", base_amount: "1000" })]);
  });

  it("offers approve on a pending commission", async () => {
    rows = [{ id: "c1", partner_id: "p1", partner_name: "Harbor Resellers", amount: 100, currency: "USD", rate: 0.1, status: "pending", created_at: "2026-11-01T00:00:00Z" }];
    render(<CommissionsPage />);
    fireEvent.click((await screen.findAllByRole("button", { name: /^approve$/i }))[0]);
    await waitFor(() => expect(patch).toHaveBeenCalledWith("/partner/commissions/c1/status", { status: "approved" }));
  });
});
