import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import UnitsPage from "./page";

const listUnits = vi.fn();
const registerUnit = vi.fn();
const setUnitSerial = vi.fn();
const updateUnitDelivery = vi.fn();
let perms: string[] = ["manufacturing:view_products", "manufacturing:manage_production"];
vi.mock("@/lib/api", () => ({
  manufacturingApi: {
    listUnits: (...a: unknown[]) => listUnits(...a),
    listProducts: () => Promise.resolve({ data: [{ id: "p1", sku: "HW-LAP-T", name: "Laptop Pro 14" }] }),
    listProductionOrders: () => Promise.resolve({ data: [{ id: "o1", order_number: "PO-7" }] }),
    registerUnit: (...a: unknown[]) => registerUnit(...a),
    setUnitSerial: (...a: unknown[]) => setUnitSerial(...a),
    updateUnitDelivery: (...a: unknown[]) => updateUnitDelivery(...a),
  },
}));
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: true, isAdmin: false, isSuperadmin: false,
    can: (c: string) => perms.includes(c), holds: (c: string) => perms.includes(c),
  }),
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/manufacturing/units", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

const UNITS = [
  { id: "u1", serial_number: "DSKT-00000-016JD", product_id: "p1", production_order_id: "o1", status: "in_production", is_test: true },
  { id: "u2", serial_number: "DSK-TEST-000001", product_id: "p1", production_order_id: "o1", status: "in_production", is_test: false },
];

describe("unit registry (3 Nov 2026): generated serials", () => {
  beforeEach(() => {
    listUnits.mockReset();
    registerUnit.mockReset();
    listUnits.mockResolvedValue({ data: UNITS });
    window.history.replaceState({}, "", "/manufacturing/units");
  });

  it("shows a Test pill on test units", async () => {
    render(<UnitsPage />);
    expect((await screen.findAllByText("DSKT-00000-016JD")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("Test").length).toBeGreaterThan(0);
  });

  it("searches the label's ?serial= normalised (case, dashes, I/L/O)", async () => {
    window.history.replaceState({}, "", "/manufacturing/units?serial=dskt-ooooo-ol6jd");
    render(<UnitsPage />);
    await waitFor(() => expect(listUnits).toHaveBeenCalledWith(expect.objectContaining({ q: "DSKT00000016JD" })));
  });

  it("shows the server's typo message", async () => {
    listUnits.mockRejectedValue({ response: { data: { detail: "This serial's check character does not match: probably a typo" } } });
    render(<UnitsPage />);
    expect(await screen.findByText(/probably a typo/)).toBeTruthy();
  });

  it("registers a unit without asking for a serial and shows the generated one", async () => {
    registerUnit.mockResolvedValue({ data: { id: "u3", serial_number: "DSK-7K2M9-QXH48", status: "in_production", is_test: false } });
    render(<UnitsPage />);
    fireEvent.click(await screen.findByRole("button", { name: /Register unit/ }));
    expect(screen.queryByPlaceholderText(/HW-/)).toBeNull();
    const selects = screen.getAllByRole("combobox");
    const order = selects.find((s) => Array.from((s as HTMLSelectElement).options).some((o) => o.value === "o1")) as HTMLSelectElement;
    const product = selects.filter((s) => Array.from((s as HTMLSelectElement).options).some((o) => o.text.includes("(HW-LAP-T)")))[0] as HTMLSelectElement;
    fireEvent.change(order, { target: { value: "o1" } });
    fireEvent.change(product, { target: { value: "p1" } });
    const buttons = screen.getAllByRole("button", { name: "Register unit" });
    fireEvent.click(buttons[buttons.length - 1]);
    await waitFor(() => expect(registerUnit).toHaveBeenCalledTimes(1));
    const body = registerUnit.mock.calls[0][0] as Record<string, unknown>;
    expect(body.serial_number).toBeUndefined();
    expect(body).toEqual(expect.objectContaining({ production_order_id: "o1", product_id: "p1", is_test: false }));
    expect((await screen.findByTestId("generated-serial")).textContent).toBe("DSK-7K2M9-QXH48");
    expect(screen.getByRole("button", { name: "Copy serial DSK-7K2M9-QXH48" })).toBeTruthy();
  });
});

describe("unit registry: relabel, delivery and the label (batch E)", () => {
  beforeEach(() => {
    listUnits.mockReset(); setUnitSerial.mockReset(); updateUnitDelivery.mockReset();
    listUnits.mockResolvedValue({ data: [UNITS[1]] });
    perms = ["manufacturing:view_products", "manufacturing:manage_production"];
    window.history.replaceState({}, "", "/manufacturing/units");
  });

  async function openUnit() {
    render(<UnitsPage />);
    fireEvent.click((await screen.findAllByText("DSK-TEST-000001"))[0]);
  }

  it("changes a unit's serial through PUT /manufacturing/units/{id}/serial and shows a clash", async () => {
    setUnitSerial.mockRejectedValueOnce({ response: { data: { detail: "Serial DSK-NEW-1 is already used by another unit" } } });
    setUnitSerial.mockResolvedValueOnce({ data: { id: "u2", serial_number: "DSK-NEW-2" } });
    await openUnit();
    fireEvent.click(await screen.findByRole("button", { name: /Change serial/ }));
    const input = screen.getByLabelText(/New serial/);
    fireEvent.change(input, { target: { value: "DSK-NEW-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Save serial" }));
    expect(await screen.findByText(/already used by another unit/)).toBeTruthy();
    fireEvent.change(input, { target: { value: "DSK-NEW-2" } });
    fireEvent.click(screen.getByRole("button", { name: "Save serial" }));
    await waitFor(() => expect(setUnitSerial).toHaveBeenLastCalledWith("u2", "DSK-NEW-2"));
  });

  it("saves the tracking number and delivery date", async () => {
    updateUnitDelivery.mockResolvedValue({ data: {} });
    await openUnit();
    fireEvent.click(await screen.findByRole("button", { name: /Delivery details/ }));
    fireEvent.change(screen.getByLabelText(/Tracking number/), { target: { value: " TRK-9 " } });
    fireEvent.change(screen.getByLabelText(/Delivery date/), { target: { value: "2026-11-18" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateUnitDelivery).toHaveBeenCalledWith("u2", { tracking_number: "TRK-9", delivery_date: "2026-11-18" }));
  });

  it("links to the printable label, and shows edits only to production managers", async () => {
    perms = ["manufacturing:view_products"];
    await openUnit();
    const link = await screen.findByRole("link", { name: /Print label/ });
    expect(link.getAttribute("href")).toBe("/manufacturing/units/u2/label");
    expect(screen.queryByRole("button", { name: /Change serial/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Delivery details/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Register unit/ })).toBeNull();
  });
});
