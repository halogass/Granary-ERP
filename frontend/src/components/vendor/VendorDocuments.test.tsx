import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import VendorDocuments from "./VendorDocuments";

const listVendorDocuments = vi.fn();
const verifyVendorDocument = vi.fn();
vi.mock("@/lib/api", () => ({
  vendorApi: {
    listVendorDocuments: (...a: unknown[]) => listVendorDocuments(...a),
    verifyVendorDocument: (...a: unknown[]) => verifyVendorDocument(...a),
    downloadVendorDocument: vi.fn(),
  },
}));

describe("VendorDocuments", () => {
  beforeEach(() => {
    listVendorDocuments.mockReset(); verifyVendorDocument.mockReset();
  });

  it("lists the vendor's documents and verifies one through PATCH /vendor/documents/{id}/verify", async () => {
    listVendorDocuments.mockResolvedValue({ data: [
      { id: "d1", document_type: "insurance", name: "Liability cover", signed_at: null, expires_at: null,
        is_verified: false, notes: null, created_at: "2026-11-01T00:00:00Z" },
    ] });
    verifyVendorDocument.mockResolvedValue({ data: { id: "d1", is_verified: true } });
    render(<VendorDocuments vendorId="v1" />);
    await waitFor(() => expect(listVendorDocuments).toHaveBeenCalledWith("v1"));
    expect(await screen.findByText("Not verified")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Verify Liability cover" }));
    await waitFor(() => expect(verifyVendorDocument).toHaveBeenCalledWith("d1"));
    expect(await screen.findByText("Verified")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Verify Liability cover" })).toBeNull();
  });

  it("says when there are none", async () => {
    listVendorDocuments.mockResolvedValue({ data: [] });
    render(<VendorDocuments vendorId="v1" />);
    expect(await screen.findByText(/No documents uploaded/)).toBeTruthy();
  });
});
