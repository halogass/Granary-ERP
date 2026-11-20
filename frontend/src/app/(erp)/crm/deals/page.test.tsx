import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";

const listDeals = vi.fn();
const getStages = vi.fn();

vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1" } } }) }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock("next/link", () => ({ default: ({ children }: { children: React.ReactNode }) => <a>{children}</a> }));
vi.mock("@/lib/api", () => ({
  crmApi: {
    listDeals: (...a: unknown[]) => listDeals(...a),
    getStages: (...a: unknown[]) => getStages(...a),
    updateDealStage: vi.fn(), updateDeal: vi.fn(), createDeal: vi.fn(), deleteDeal: vi.fn(),
  },
  peopleApi: { directory: () => Promise.resolve({ data: [] }) },
  authApi: { me: () => Promise.resolve({ data: { permissions: [] } }) },
  systemApi: { getCurrencySettings: () => Promise.resolve({ data: { active: ["JPY"], functional: "JPY" } }) },
}));
vi.mock("@/components/crm/AiSelectionBar", () => ({ default: () => null }));
vi.mock("@/components/crm/FollowUpButton", () => ({ default: () => null }));
vi.mock("@/components/crm/OrgContactPicker", () => ({ default: () => null }));
vi.mock("@/components/ui/AttachmentPanel", () => ({ default: () => null }));
vi.mock("@/components/catalog/ProductLineSelect", () => ({ ProductLineMultiSelect: () => null }));

import DealsPage from "./page";
import { resetCrmStagesCache } from "@/hooks/useCrmStages";

const stage = (key: string, label: string, order: number, over: Record<string, unknown> = {}) =>
  ({ key, label, order, is_closed: false, is_won: false, side: false, probability: 10, ...over });

const REGISTRY = {
  deal_stages: [
    stage("new", "New", 1), stage("qualifying", "Qualifying", 2), stage("proposal", "Proposal", 5),
    stage("won", "Won", 7, { is_closed: true, is_won: true }),
    stage("nurture", "Nurture", 10, { side: true }),
    stage("lost", "Lost", 11, { is_closed: true, side: true }),
  ],
  lead_statuses: [], lead_transitions: {}, lead_start_statuses: [], lead_sources: [],
  loss_reasons: [], disqualify_reasons: [],
};

beforeEach(() => {
  resetCrmStagesCache();
  getStages.mockResolvedValue({ data: REGISTRY });
  listDeals.mockResolvedValue({ data: [
    { id: "d1", title: "Pilot at Acme Ltd", stage: "proposal", value: 100, currency: "JPY", needs_next_action: true },
    { id: "d2", title: "Signed rollout", stage: "won", value: 300, currency: "JPY" },
    { id: "d3", title: "Too pricey", stage: "lost", value: 50, currency: "JPY" },
  ] });
});

describe("Deals kanban", () => {
  it("draws its columns from the backend's stage registry", async () => {
    render(<DealsPage />);
    expect(await screen.findByText("Pilot at Acme Ltd")).toBeInTheDocument();
    // The ladder plus nurture; lost is reached from the form, never a column
    for (const label of ["New", "Qualifying", "Proposal", "Won", "Nurture"]) {
      expect(screen.getAllByText(label).length).toBeGreaterThan(0);
    }
    expect(screen.queryByText("Too pricey")).not.toBeInTheDocument();
    expect(screen.queryByText("Prospect")).not.toBeInTheDocument();
    expect(screen.queryByText("Negotiation")).not.toBeInTheDocument();
    expect(screen.getByText("No next action")).toBeInTheDocument();
  });

  it("computes the win rate over decided deals only", async () => {
    render(<DealsPage />);
    await screen.findByText("Pilot at Acme Ltd");
    const card = screen.getByText("Win rate").parentElement!;
    // 1 won out of 2 decided (won + lost); the open deal doesn't count
    expect(within(card).getByText("50%")).toBeInTheDocument();
    expect(within(card).getByText("of 2 decided deals")).toBeInTheDocument();
  });

  it("filters the list by segment on the server and shows the segment", async () => {
    getStages.mockResolvedValue({ data: { ...REGISTRY, segments: [{ key: "retail", label: "Retail" }, "Public sector"] } });
    listDeals.mockResolvedValue({ data: [
      { id: "d1", title: "Pilot at Acme Ltd", stage: "proposal", value: 100, currency: "JPY", segment: "retail" },
    ] });
    render(<DealsPage />);
    await screen.findByText("Pilot at Acme Ltd");
    fireEvent.click(screen.getByRole("button", { name: /list/i }));
    fireEvent.change(await screen.findByLabelText("Segment filter"), { target: { value: "retail" } });
    await waitFor(() => expect(listDeals).toHaveBeenLastCalledWith({ segment: "retail", limit: 200, offset: 0 }));
    expect(screen.getAllByText("Retail").length).toBeGreaterThan(0);
  });

  it("hides the segment filter when no segments are configured", async () => {
    render(<DealsPage />);
    await screen.findByText("Pilot at Acme Ltd");
    fireEvent.click(screen.getByRole("button", { name: /list/i }));
    expect(screen.queryByLabelText("Segment filter")).toBeNull();
  });
});
