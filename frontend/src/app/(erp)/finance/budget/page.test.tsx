import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import BudgetPage from "./page";

const get = vi.fn();
const patch = vi.fn();
let perms: string[] = [];
vi.mock("@/lib/api", () => ({
  default: {
    get: (...a: unknown[]) => get(...a),
    patch: (...a: unknown[]) => patch(...a),
    post: vi.fn(),
    delete: vi.fn(),
  },
}));
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: true, isAdmin: false, isSuperadmin: false,
    can: (c: string) => perms.includes(c), holds: (c: string) => perms.includes(c),
  }),
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/finance/budget", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

function budget(status: string) {
  return { id: "b1", name: "Operations 2026", fiscal_year: 2026, department: null, period_type: "annual", status,
    total_budgeted: 1000, approved_at: null, notes: null };
}

function serve(status: string) {
  get.mockImplementation((url: string) => {
    if (url === "/finance/budgets") return Promise.resolve({ data: [budget(status)] });
    if (url === "/finance/budgets/b1/vs-actual") return Promise.resolve({ data: {
      budget_id: "b1", budget_name: "Operations 2026", fiscal_year: 2026, period_type: "annual", status,
      total_budgeted: 1000, total_actual: 0, total_variance: 1000, lines: [],
    } });
    return Promise.resolve({ data: {} });
  });
}

describe("budgets: activate and close", () => {
  beforeEach(() => { get.mockReset(); patch.mockReset(); patch.mockResolvedValue({ data: {} }); });

  it("activates an approved budget through PATCH /finance/budgets/{id}", async () => {
    perms = ["finance:manage_budget"];
    serve("approved");
    render(<BudgetPage />);
    fireEvent.click((await screen.findAllByText("Operations 2026"))[0]);
    fireEvent.click(await screen.findByRole("button", { name: "Activate" }));
    await waitFor(() => expect(patch).toHaveBeenCalledWith("/finance/budgets/b1", { status: "active" }));
  });

  it("closes an active budget after a confirmation", async () => {
    perms = ["finance:manage_budget"];
    serve("active");
    render(<BudgetPage />);
    fireEvent.click((await screen.findAllByText("Operations 2026"))[0]);
    expect(screen.queryByRole("button", { name: "Activate" })).toBeNull();
    fireEvent.click(await screen.findByRole("button", { name: "Close budget" }));
    fireEvent.click(screen.getByRole("button", { name: "Yes, close" }));
    await waitFor(() => expect(patch).toHaveBeenCalledWith("/finance/budgets/b1", { status: "closed" }));
  });

  it("shows no budget actions to someone who may only view budgets", async () => {
    perms = ["finance:view_budget"];
    serve("approved");
    render(<BudgetPage />);
    fireEvent.click((await screen.findAllByText("Operations 2026"))[0]);
    await waitFor(() => expect(get).toHaveBeenCalledWith("/finance/budgets/b1/vs-actual"));
    expect(screen.queryByRole("button", { name: "Activate" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Close budget" })).toBeNull();
    expect(screen.queryByRole("button", { name: /New budget/ })).toBeNull();
  });
});
