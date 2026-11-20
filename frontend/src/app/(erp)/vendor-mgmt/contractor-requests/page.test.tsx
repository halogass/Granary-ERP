import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import ContractorRequestsPage from "./page";

const createContractorRequest = vi.fn();
vi.mock("@/lib/api", () => ({
  vendorApi: {
    listContractorRequests: () => Promise.resolve({ data: [] }),
    createContractorRequest: (...a: unknown[]) => createContractorRequest(...a),
    approveContractorRequest: vi.fn(), deleteContractorRequest: vi.fn(),
  },
  systemApi: { getCurrencySettings: () => Promise.resolve({ data: { functional: "SGD", active: ["SGD", "JPY"] } }) },
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/vendor-mgmt/contractor-requests", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

describe("contractor requests: currency list", () => {
  it("offers the active currencies and defaults to the functional one, not USD/EUR", async () => {
    createContractorRequest.mockResolvedValue({ data: {} });
    render(<ContractorRequestsPage />);
    // the currency settings load once and are shared
    await new Promise((r) => setTimeout(r, 0));
    fireEvent.click(await screen.findByRole("button", { name: /New request/ }));
    const select = await waitFor(() => {
      const el = screen.getAllByRole("combobox").find((s) =>
        Array.from((s as HTMLSelectElement).options).some((o) => o.value === "JPY")) as HTMLSelectElement;
      expect(el).toBeTruthy();
      return el;
    });
    expect(Array.from(select.options).map((o) => o.value)).toEqual(["SGD", "JPY"]);
    expect(select.value).toBe("SGD");
  });
});
