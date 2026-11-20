import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import UnitLabelPage from "./page";

const getUnitLabel = vi.fn();
vi.mock("@/lib/api", () => ({ manufacturingApi: { getUnitLabel: (...a: unknown[]) => getUnitLabel(...a) } }));
vi.mock("next/navigation", () => ({ useParams: () => ({ id: "u1" }), useRouter: () => ({ push: vi.fn() }), usePathname: () => "/manufacturing/units/u1/label" }));

describe("unit label page", () => {
  beforeEach(() => { getUnitLabel.mockReset(); });

  it("draws the serial and QR at the configured label size and prints", async () => {
    getUnitLabel.mockResolvedValue({ data: {
      id: "u1", serial_number: "DSK-7K2M9-QXH48", is_test: true, hardware_revision: "1.2",
      product_sku: "HW-DSK", product_name: "Desk unit", qr_url: "https://erp.example/manufacturing/units?serial=DSK-7K2M9-QXH48",
      qr: "data:image/png;base64,AAAA", width_mm: 62, height_mm: 29,
    } });
    const print = vi.spyOn(window, "print").mockImplementation(() => {});
    render(<UnitLabelPage />);
    await waitFor(() => expect(getUnitLabel).toHaveBeenCalledWith("u1"));
    const label = await screen.findByTestId("unit-label");
    expect(label.style.width).toBe("62mm");
    expect(label.style.height).toBe("29mm");
    expect(screen.getByText("DSK-7K2M9-QXH48")).toBeTruthy();
    expect(screen.getByText("TEST UNIT")).toBeTruthy();
    expect(screen.getByAltText("QR code for DSK-7K2M9-QXH48").getAttribute("src")).toBe("data:image/png;base64,AAAA");
    fireEvent.click(screen.getByRole("button", { name: /Print/ }));
    expect(print).toHaveBeenCalled();
    print.mockRestore();
  });

  it("shows why the label cannot load (e.g. the label size is not set)", async () => {
    getUnitLabel.mockRejectedValue({ response: { data: { detail: "Setting manufacturing.label.width_mm is not set" } } });
    render(<UnitLabelPage />);
    expect(await screen.findByText(/label.width_mm is not set/)).toBeTruthy();
  });
});
