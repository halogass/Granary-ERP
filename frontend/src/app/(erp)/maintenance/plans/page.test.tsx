import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

import MaintenancePlansPage from "./page";

const plans = vi.fn();
vi.mock("@/lib/api", () => ({
  default: {
    get: (url: string) => (url === "/maintenance/plans" ? plans() : Promise.resolve({ data: [] })),
    post: vi.fn(), delete: vi.fn(),
  },
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/maintenance/plans", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

const PLAN = {
  id: "p1", equipment_id: "e1", equipment_name: "Press", name: "Oil change", description: null, frequency_type: "days",
  frequency_value: 30, last_performed_at: null, next_due_at: null, next_due_days: 5, overdue: false, due_soon: true,
  due_soon_days: 12, is_active: true, created_at: "2026-11-01T00:00:00Z",
};

describe("maintenance plans: due-soon window", () => {
  it("labels the due-soon figure with the configured number of days, not a fixed 7", async () => {
    plans.mockResolvedValue({ data: [PLAN] });
    render(<MaintenancePlansPage />);
    expect(await screen.findByText("Due within 12 days")).toBeTruthy();
    expect(screen.queryByText("Due within 7 days")).toBeNull();
  });

  it("falls back to Due soon when no plan says the window", async () => {
    plans.mockResolvedValue({ data: [] });
    render(<MaintenancePlansPage />);
    expect((await screen.findAllByText("Due soon")).length).toBeGreaterThan(0);
  });
});
