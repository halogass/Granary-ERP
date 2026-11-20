import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import PaymentSchedulesPage from "./page";

const raiseProforma = vi.fn();
const listSchedules = vi.fn();
let perms: string[] = [];
vi.mock("@/lib/api", () => ({
  financeApi: {
    listSchedules: (...a: unknown[]) => listSchedules(...a),
    raiseProforma: (...a: unknown[]) => raiseProforma(...a),
    deleteSchedule: vi.fn(),
    createSchedule: vi.fn(),
  },
  salesApi: { listQuotations: () => Promise.resolve({ data: [] }) },
  crmApi: {}, vendorApi: {}, hrApi: {}, rndApi: {},
}));
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: true, isAdmin: false, isSuperadmin: false,
    can: (c: string) => perms.includes(c), holds: (c: string) => perms.includes(c),
  }),
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/finance/schedules", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

const MILESTONE = {
  id: "s1", name: "Deposit", sequence: 1, percentage: 30, amount: 3000, currency: "USD", due_trigger: "on_date",
  due_date: "2099-01-01", status: "pending", quotation_id: "q1", quotation_number: "Q-1", invoice_id: null,
  proforma_invoice_id: null,
};

describe("payment schedules: raise proforma", () => {
  beforeEach(() => {
    raiseProforma.mockReset(); listSchedules.mockReset();
    listSchedules.mockResolvedValue({ data: [MILESTONE] });
  });

  it("raises the milestone's proforma through POST /finance/payment-schedules/{id}/raise-proforma", async () => {
    perms = ["finance:view_invoices", "finance:create_invoice"];
    raiseProforma.mockResolvedValue({ data: { id: "inv1", number: "PI-0001", amount: 3000 } });
    render(<PaymentSchedulesPage />);
    fireEvent.click(await screen.findByRole("button", { name: /Raise proforma/ }));
    await waitFor(() => expect(raiseProforma).toHaveBeenCalledWith("s1"));
  });

  it("does not offer it once a proforma exists, nor to someone who cannot invoice", async () => {
    perms = ["finance:view_invoices", "finance:create_invoice"];
    listSchedules.mockResolvedValue({ data: [{ ...MILESTONE, proforma_invoice_id: "inv1" }] });
    const { unmount } = render(<PaymentSchedulesPage />);
    expect((await screen.findAllByText("Deposit")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /Raise proforma/ })).toBeNull();
    unmount();
    perms = ["finance:view_invoices"];
    listSchedules.mockResolvedValue({ data: [MILESTONE] });
    render(<PaymentSchedulesPage />);
    expect((await screen.findAllByText("Deposit")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /Raise proforma/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Add milestone/ })).toBeNull();
  });
});
