import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1" } } }) }));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  usePathname: () => "/manufacturing/qc",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  manufacturingApi: {
    listQCRecords: () => Promise.resolve({ data: [] }),
    listUnits: () => Promise.resolve({ data: [
      { id: "u1", serial_number: "SN-1", status: "qc_pending" },
      { id: "u2", serial_number: "SN-2", status: "qc_pending" },
      { id: "u3", serial_number: "SN-3", status: "in_production" },
    ] }),
    createQCRecord: vi.fn(),
  },
}));

import QCPage from "./page";

/** The value shown under a KPI label. */
function kpi(label: string) {
  return screen.getByText(label).nextElementSibling?.textContent;
}

describe("quality control KPIs", () => {
  it("counts units waiting in QC pending, not only inspection records", async () => {
    render(<QCPage />);
    expect(await screen.findByText("units in QC pending")).toBeInTheDocument();
    expect(kpi("Awaiting QC")).toBe("2");
    expect(kpi("Inspections")).toBe("0");
    expect(kpi("Pass rate")).toBe("-");
  });
});
