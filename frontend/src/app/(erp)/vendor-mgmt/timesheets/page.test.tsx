import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import VendorTimesheetsReviewPage from "./page";

const listTimesheets = vi.fn();
const reviewTimesheet = vi.fn();
let perms: string[] = [];
vi.mock("@/lib/api", () => ({
  vendorApi: {
    listTimesheets: (...a: unknown[]) => listTimesheets(...a),
    reviewTimesheet: (...a: unknown[]) => reviewTimesheet(...a),
  },
}));
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: true, isAdmin: false, isSuperadmin: false,
    can: (c: string) => perms.includes(c), holds: (c: string) => perms.includes(c),
  }),
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/vendor-mgmt/timesheets", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

const SHEET = {
  id: "t1", work_order_id: "w1", work_order_number: "WO-0007", work_order_title: "Site install", vendor_id: "v1",
  vendor_name: "Acme Installers", period_start: "2026-11-01", period_end: "2026-11-07", total_hours: 12.5,
  status: "submitted", notes: null, rejection_reason: null, reviewed_at: null, created_at: "2026-11-08T00:00:00Z",
};

describe("vendor timesheets review", () => {
  beforeEach(() => {
    listTimesheets.mockReset(); reviewTimesheet.mockReset();
    listTimesheets.mockResolvedValue({ data: [SHEET] });
    reviewTimesheet.mockResolvedValue({ data: {} });
    window.localStorage.clear();
  });

  it("lists submitted timesheets and approves one", async () => {
    perms = ["vendor_mgmt:view", "vendor_mgmt:approve_work_order"];
    render(<VendorTimesheetsReviewPage />);
    await waitFor(() => expect(listTimesheets).toHaveBeenCalledWith(expect.objectContaining({ status: "submitted" })));
    fireEvent.click((await screen.findAllByRole("button", { name: "Approve timesheet of Acme Installers" }))[0]);
    await waitFor(() => expect(reviewTimesheet).toHaveBeenCalledWith("t1", { status: "approved" }));
  });

  it("rejects with a reason", async () => {
    perms = ["vendor_mgmt:view", "vendor_mgmt:approve_work_order"];
    render(<VendorTimesheetsReviewPage />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Reject timesheet of Acme Installers" }))[0]);
    const reject = screen.getByRole("button", { name: "Reject" });
    expect((reject as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText(/Reason/), { target: { value: "Hours exceed the estimate" } });
    fireEvent.click(reject);
    await waitFor(() => expect(reviewTimesheet).toHaveBeenCalledWith("t1", { status: "rejected", rejection_reason: "Hours exceed the estimate" }));
  });

  it("shows no review buttons to someone who can only view", async () => {
    perms = ["vendor_mgmt:view"];
    render(<VendorTimesheetsReviewPage />);
    expect((await screen.findAllByText("Acme Installers")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /Approve timesheet/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Reject timesheet/ })).toBeNull();
  });
});
