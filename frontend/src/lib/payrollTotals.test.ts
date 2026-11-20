import { describe, expect, it } from "vitest";
import { paidThisYear } from "./payrollTotals";

describe("paidThisYear (payroll 'Paid this year' tile)", () => {
  const runs = [
    { status: "paid", year: 2026, total_net: 3_000_000, currency: "JPY" },
    { status: "paid", year: 2026, total_net: 539_105, currency: "JPY" },
    { status: "draft", year: 2026, total_net: 999, currency: "JPY" },
    { status: "paid", year: 2025, total_net: 1_000, currency: "JPY" },
  ];

  it("sums this year's paid runs in their currency, not USD", () => {
    expect(paidThisYear(runs, 2026, "JPY")).toEqual({ total: 3_539_105, currency: "JPY", mixed: false });
  });

  it("shows zero in the functional currency when nothing is paid", () => {
    expect(paidThisYear([], 2026, "JPY")).toEqual({ total: 0, currency: "JPY", mixed: false });
  });

  it("treats a run without a currency as the functional currency", () => {
    expect(paidThisYear([{ status: "paid", year: 2026, total_net: 5, currency: null }], 2026, "EUR").currency).toBe("EUR");
  });
});
