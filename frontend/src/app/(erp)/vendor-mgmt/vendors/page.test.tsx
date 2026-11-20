import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import VendorsPage from "./page";

const updateVendor = vi.fn();
const getVendor = vi.fn();
const listVendorDocuments = vi.fn((..._a: unknown[]) => Promise.resolve({ data: [] as unknown[] }));
let perms: string[] = [];

vi.mock("@/lib/api", () => ({
  vendorApi: {
    listVendors: () => Promise.resolve({ data: [
      { id: "v1", name: "Acme Supplies", vendor_type: "component_supplier", status: "active", contact_email: "a@acme.test" },
    ] }),
    getVendor: (...a: unknown[]) => getVendor(...a),
    updateVendor: (...a: unknown[]) => updateVendor(...a),
    createVendor: vi.fn(),
    deleteVendor: vi.fn(),
    inviteVendor: vi.fn(),
    listVendorDocuments: (...a: unknown[]) => listVendorDocuments(...a),
    verifyVendorDocument: vi.fn(),
    downloadVendorDocument: vi.fn(),
  },
}));
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: true, isAdmin: false, isSuperadmin: false,
    can: (c: string) => perms.includes(c), holds: (c: string) => perms.includes(c),
  }),
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/vendor-mgmt/vendors", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

describe("vendors: edit a vendor (20 Nov 2026)", () => {
  beforeEach(() => {
    updateVendor.mockReset();
    getVendor.mockReset();
    getVendor.mockResolvedValue({ data: {
      id: "v1", name: "Acme Supplies", vendor_type: "component_supplier", contact_email: "a@acme.test",
      tax_id: "T-1", withholding_applicable: false, withholding_rate: null,
    } });
    updateVendor.mockResolvedValue({ data: { id: "v1" } });
  });

  it("hides the edit action from people without vendor_mgmt:manage_vendors", async () => {
    perms = ["vendor_mgmt:view"];
    render(<VendorsPage />);
    expect((await screen.findAllByText("Acme Supplies")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: "Edit Acme Supplies" })).toBeNull();
  });

  it("shows Add vendor, Delete and portal invites only to people who manage vendors", async () => {
    perms = ["vendor_mgmt:view"];
    const { unmount } = render(<VendorsPage />);
    expect((await screen.findAllByText("Acme Supplies")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /Add vendor/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete Acme Supplies" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Grant portal access/ })).toBeNull();
    unmount();
    perms = ["vendor_mgmt:view", "vendor_mgmt:manage_vendors"];
    render(<VendorsPage />);
    expect(await screen.findByRole("button", { name: /Add vendor/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Delete Acme Supplies" })).toBeTruthy();
  });

  it("switches withholding on with a rate and saves through PATCH /vendor/vendors/{id}", async () => {
    perms = ["vendor_mgmt:view", "vendor_mgmt:manage_vendors"];
    render(<VendorsPage />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Acme Supplies" }));
    await waitFor(() => expect(getVendor).toHaveBeenCalledWith("v1"));
    fireEvent.click(await screen.findByLabelText(/Withhold tax/));
    const rate = await screen.findByRole("spinbutton");
    fireEvent.change(rate, { target: { value: "2.5" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(updateVendor).toHaveBeenCalledTimes(1));
    const [id, body] = updateVendor.mock.calls[0] as [string, Record<string, unknown>];
    expect(id).toBe("v1");
    expect(body).toEqual(expect.objectContaining({
      name: "Acme Supplies", tax_id: "T-1", withholding_applicable: true, withholding_rate: 2.5,
    }));
  });

  it("leaves the rate out when it is empty so the server takes the default", async () => {
    perms = ["vendor_mgmt:manage_vendors"];
    render(<VendorsPage />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Acme Supplies" }));
    fireEvent.click(await screen.findByLabelText(/Withhold tax/));
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(updateVendor).toHaveBeenCalledTimes(1));
    const body = updateVendor.mock.calls[0][1] as Record<string, unknown>;
    expect(body.withholding_applicable).toBe(true);
    expect("withholding_rate" in body).toBe(false);
  });
});

describe("vendors: documents in the expanded row (batch E)", () => {
  it("lists a vendor's documents for vendor managers only", async () => {
    perms = ["vendor_mgmt:view"];
    const { unmount } = render(<VendorsPage />);
    fireEvent.click((await screen.findAllByText("Acme Supplies"))[0]);
    expect(screen.queryByRole("region", { name: "Vendor documents" })).toBeNull();
    expect(listVendorDocuments).not.toHaveBeenCalled();
    unmount();
    perms = ["vendor_mgmt:view", "vendor_mgmt:manage_vendors"];
    render(<VendorsPage />);
    fireEvent.click((await screen.findAllByText("Acme Supplies"))[0]);
    await waitFor(() => expect(listVendorDocuments).toHaveBeenCalledWith("v1"));
    expect(await screen.findByRole("region", { name: "Vendor documents" })).toBeTruthy();
  });
});
