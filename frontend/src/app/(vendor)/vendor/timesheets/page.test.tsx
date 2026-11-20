import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import VendorTimesheetsPage from "./page";

// Batch F: the form sent hours_worked/description and no work order; POST /vendor/portal/timesheets wants
// work_order_id and total_hours (and notes), so no vendor could submit a timesheet.
const listMyTimesheets = vi.fn();
const listMyWorkOrders = vi.fn();
const submitTimesheet = vi.fn();
vi.mock("@/lib/api", () => ({
  portalVendorApi: {
    listMyTimesheets: (...a: unknown[]) => listMyTimesheets(...a),
    listMyWorkOrders: (...a: unknown[]) => listMyWorkOrders(...a),
    submitTimesheet: (...a: unknown[]) => submitTimesheet(...a),
  },
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/vendor/timesheets", useSearchParams: () => new URLSearchParams() }));

const ORDERS = [
  { id: "w1", number: "WO-0001", title: "Site install", status: "active" },
  { id: "w2", number: "WO-0002", title: "Old survey", status: "completed" },
  { id: "w3", number: "WO-0003", title: "Cabling", status: "active" },
];

describe("vendor portal timesheets", () => {
  beforeEach(() => {
    listMyTimesheets.mockReset(); listMyWorkOrders.mockReset(); submitTimesheet.mockReset();
    listMyTimesheets.mockResolvedValue({ data: [
      { id: "t1", work_order_id: "w1", period_start: "2026-11-01", period_end: "2026-11-07", total_hours: 12.5,
        status: "rejected", notes: "Rack work", rejection_reason: "Split by week" },
    ] });
    listMyWorkOrders.mockResolvedValue({ data: ORDERS });
    submitTimesheet.mockResolvedValue({ data: { id: "t2", status: "submitted" } });
  });

  it("lists timesheets with their work order, hours and the rejection reason", async () => {
    render(<VendorTimesheetsPage />);
    expect((await screen.findAllByText(/WO-0001 · Site install/)).length).toBeGreaterThan(0);
    expect(screen.getAllByText("12.5h").length).toBeGreaterThan(0);
    expect(screen.getAllByText(/Rejected: Split by week/).length).toBeGreaterThan(0);
  });

  it("submits work_order_id, total_hours and notes, offering only active work orders", async () => {
    render(<VendorTimesheetsPage />);
    await screen.findAllByText(/WO-0001 · Site install/);
    fireEvent.click(screen.getAllByRole("button", { name: /Log hours/ })[0]);
    const select = await screen.findByLabelText(/Work order/) as HTMLSelectElement;
    const labels = Array.from(select.options).map((o) => o.textContent);
    expect(labels).toContain("WO-0003 · Cabling");
    expect(labels).not.toContain("WO-0002 · Old survey");

    const submit = screen.getByRole("button", { name: "Submit" }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    fireEvent.change(select, { target: { value: "w3" } });
    fireEvent.change(screen.getByLabelText(/Period start/), { target: { value: "2026-11-09" } });
    fireEvent.change(screen.getByLabelText(/Period end/), { target: { value: "2026-11-15" } });
    fireEvent.change(screen.getByLabelText(/Total hours/), { target: { value: "37.5" } });
    fireEvent.change(screen.getByLabelText(/Notes/), { target: { value: "  Pulled cable on floor 2 " } });
    expect(submit.disabled).toBe(false);
    fireEvent.click(submit);
    await waitFor(() => expect(submitTimesheet).toHaveBeenCalledWith({
      work_order_id: "w3", period_start: "2026-11-09", period_end: "2026-11-15", total_hours: 37.5,
      notes: "Pulled cable on floor 2",
    }));
    const sent = submitTimesheet.mock.calls[0][0];
    expect(sent).not.toHaveProperty("hours_worked");
    expect(sent).not.toHaveProperty("description");
  });

  it("refuses a period that ends before it starts, and shows the server's error", async () => {
    submitTimesheet.mockRejectedValue({ response: { data: { detail: "Work order not found or does not belong to your vendor account" } } });
    render(<VendorTimesheetsPage />);
    await screen.findAllByText(/WO-0001 · Site install/);
    fireEvent.click(screen.getAllByRole("button", { name: /Log hours/ })[0]);
    fireEvent.change(await screen.findByLabelText(/Work order/), { target: { value: "w1" } });
    fireEvent.change(screen.getByLabelText(/Period start/), { target: { value: "2026-11-15" } });
    fireEvent.change(screen.getByLabelText(/Period end/), { target: { value: "2026-11-09" } });
    fireEvent.change(screen.getByLabelText(/Total hours/), { target: { value: "8" } });
    const submit = screen.getByRole("button", { name: "Submit" }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    expect(screen.getByText("The period ends before it starts.")).toBeTruthy();
    fireEvent.change(screen.getByLabelText(/Period end/), { target: { value: "2026-11-20" } });
    fireEvent.click(submit);
    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(screen.getByText(/Work order not found/)).toBeTruthy();
  });
});
