import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// The Radix switch inside a form measures its hidden input (jsdom has no ResizeObserver).
globalThis.ResizeObserver ??= class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;

const NOTE = "Prices exclude VAT and withholding tax.";
const updateInvoice = vi.fn();

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock("@/lib/api", () => ({
  financeApi: {
    listTaxes: () => Promise.resolve({ data: [{ id: "t-vat", name: "VAT", code: "VAT11", rate: 11, applies_to_sales: true }] }),
    getSpotRate: () => Promise.resolve({ data: { rate: null } }),
    updateInvoice: (...a: unknown[]) => updateInvoice(...a),
    createInvoice: vi.fn(),
  },
  systemApi: { getCurrencySettings: () => Promise.resolve({ data: { active: ["JPY"], functional: "JPY", decimals: { JPY: 0 } } }) },
  salesApi: {
    taxNotes: () => Promise.resolve({ data: { note: NOTE, exclusive_phrases: ["exclude vat"], inclusive_phrases: ["including vat"] } }),
    pricingPreview: vi.fn(),
  },
}));
vi.mock("@/components/ui/RichTextEditor", () => ({ default: () => null }));
vi.mock("@/components/notes/NoteTemplatePicker", () => ({ default: () => null }));
vi.mock("@/components/crm/OrgContactPicker", () => ({ default: () => null }));
vi.mock("@/lib/entitySources", () => ({ catalogSource: { fetch: () => Promise.resolve([]) } }));

import InvoiceForm from "./InvoiceForm";

describe("InvoiceForm (2 Nov 2026)", () => {
  it("Prices exclude tax clears every line's tax and puts the sentence in the notes", async () => {
    updateInvoice.mockResolvedValue({ data: {} });
    render(<InvoiceForm mode="edit" invoiceId="i1"
      initialForm={{ currency: "JPY", due_date: "2026-11-30", notes: "<p>Bank: First National</p>" }}
      initialLines={[
        { key: "a", name: "Gateway", description: "", product_id: "", qty: 1, unit_price: 1000000, discount_pct: 0, tax_id: "t-vat", tax_pct: 11 },
        { key: "b", name: "Installation", description: "", product_id: "", qty: 1, unit_price: 500000, discount_pct: 0, tax_id: "t-vat", tax_pct: 11 },
      ]} />);
    const toggle = await screen.findByRole("switch", { name: "Prices exclude tax" });
    await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "false"));
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
    await userEvent.click(screen.getAllByRole("button", { name: /save/i })[0]);
    await waitFor(() => expect(updateInvoice).toHaveBeenCalled());
    const payload = updateInvoice.mock.calls[0][1];
    for (const it of payload.items) {
      expect(it.tax_id).toBeUndefined();
      expect(it.tax_pct).toBeUndefined();
    }
    expect(payload.notes).toBe(`<p>Bank: First National</p><p>${NOTE}</p>`);
  });

  it("warns when the notes say prices exclude tax but a line carries it", async () => {
    render(<InvoiceForm mode="edit" invoiceId="i1"
      initialForm={{ currency: "JPY", due_date: "2026-11-30", notes: `<p>${NOTE}</p>` }}
      initialLines={[
        { key: "a", name: "Gateway", description: "", product_id: "", qty: 1, unit_price: 1000000, discount_pct: 0, tax_id: "t-vat", tax_pct: 11 },
      ]} />);
    expect(await screen.findByText(/notes say prices exclude tax, but 1 of 1 line carries tax/i)).toBeInTheDocument();
  });
});
