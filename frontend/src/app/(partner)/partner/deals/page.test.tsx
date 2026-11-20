import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import PartnerDealsPage from "./page";

const listMyDeals = vi.fn();
const createDeal = vi.fn();
vi.mock("@/lib/api", () => ({
  portalPartnerApi: {
    listMyDeals: (...a: unknown[]) => listMyDeals(...a),
    createDeal: (...a: unknown[]) => createDeal(...a),
  },
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/partner/deals", useSearchParams: () => new URLSearchParams() }));

// What GET /partner/portal/deals returns (portal_list_deals)
const DEALS = [
  { id: "d1", title: "Shibuya fit-out cameras", customer_org_name: "Kanda Logistics KK", customer_contact_name: null,
    customer_contact_email: null, estimated_value: 1200000, currency: "JPY", expected_close_date: "2026-12-20",
    status: "pending", created_at: "2026-11-01T00:00:00Z" },
  { id: "d2", title: "Zero value pilot", customer_org_name: null, customer_contact_name: null, customer_contact_email: null,
    estimated_value: null, currency: "USD", expected_close_date: null, status: "approved", created_at: "2026-11-02T00:00:00Z" },
  { id: "d3", title: "Turned down rollout", customer_org_name: null, customer_contact_name: null, customer_contact_email: null,
    estimated_value: null, currency: "USD", expected_close_date: null, status: "rejected",
    rejection_reason: "Already registered by another partner", created_at: "2026-11-03T00:00:00Z" },
];

describe("partner portal deal registrations", () => {
  beforeEach(() => {
    listMyDeals.mockReset().mockResolvedValue({ data: DEALS });
    createDeal.mockReset().mockResolvedValue({ data: { id: "d3" } });
  });

  it("shows the opportunity and end customer the API returns", async () => {
    render(<PartnerDealsPage />);
    expect((await screen.findAllByText("Shibuya fit-out cameras")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("Kanda Logistics KK").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Zero value pilot").length).toBeGreaterThan(0);
  });

  it("registers a deal with the API's field names", async () => {
    render(<PartnerDealsPage />);
    await screen.findAllByText("Shibuya fit-out cameras");
    fireEvent.click(screen.getAllByRole("button", { name: /register deal/i })[0]);
    fireEvent.change(screen.getByPlaceholderText(/support plan rollout/i), { target: { value: "Osaka rollout" } });
    fireEvent.change(screen.getByPlaceholderText(/final buyer/i), { target: { value: "Umeda Retail" } });
    fireEvent.click(screen.getByRole("button", { name: /submit registration/i }));
    await waitFor(() => expect(createDeal).toHaveBeenCalled());
    expect(createDeal.mock.calls[0][0]).toMatchObject({ title: "Osaka rollout", customer_org_name: "Umeda Retail" });
  });

  // Batch E: the partner sees why its own registration was rejected.
  it("shows the rejection reason on a rejected registration", async () => {
    render(<PartnerDealsPage />);
    const rows = await screen.findAllByText("Turned down rollout");
    fireEvent.click(rows[0]);
    expect(await screen.findByText("Already registered by another partner")).toBeInTheDocument();
    expect(screen.getByText(/why this registration was not approved/i)).toBeInTheDocument();
  });
});
