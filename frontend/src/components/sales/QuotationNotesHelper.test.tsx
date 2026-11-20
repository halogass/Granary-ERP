import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const check = vi.fn();
const template = vi.fn();
vi.mock("@/lib/api", () => ({
  salesApi: {
    quotationNotesCheck: (...a: unknown[]) => check(...a),
    quotationNotesTemplate: (...a: unknown[]) => template(...a),
  },
}));

import QuotationNotesHelper, { type NotesContext } from "./QuotationNotesHelper";

const HTML = "<div><ol><li><b>Validity: </b>This quotation is valid for 30 days from the issue date.</li></ol></div>";
const ctx = (over: Partial<NotesContext> = {}): NotesContext => ({
  notes: "", currency: "JPY", items: [{ product_id: "p-box", name: "Laptop", quantity: 2 }], ...over,
});

describe("QuotationNotesHelper (4 Nov 2026: house notes template)", () => {
  beforeEach(() => {
    check.mockReset();
    template.mockReset();
  });

  it("lists what the notes check finds and says the approval will refuse it", async () => {
    check.mockResolvedValue({ data: {
      family: "hardware", family_name: "Hardware", ok: false, missing: ["Scope", "Payment Terms"],
      issues: [
        { code: "missing", message: "Required clauses missing (Hardware template): Scope, Payment Terms.", blocking: true },
        { code: "accuracy_figure", message: "The notes state an accuracy figure.", blocking: true },
      ],
    } });
    render(<QuotationNotesHelper ctx={ctx({ notes: "Prices exclude VAT." })} onFill={vi.fn()} delayMs={0} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("The approval will refuse these notes");
    expect(screen.getByRole("alert")).toHaveTextContent("Missing clauses: Scope, Payment Terms");
    expect(screen.getByRole("alert")).toHaveTextContent("accuracy figure");
    expect(screen.getByText("Template: Hardware")).toBeInTheDocument();
    expect(check).toHaveBeenCalledWith(expect.objectContaining({ notes: "Prices exclude VAT.", currency: "JPY" }));
  });

  it("says when the notes follow the house template", async () => {
    check.mockResolvedValue({ data: { family: "hardware", family_name: "Hardware", ok: true, missing: [], issues: [] } });
    render(<QuotationNotesHelper ctx={ctx({ notes: HTML })} onFill={vi.fn()} delayMs={0} />);
    expect(await screen.findByText("The notes follow the house template.")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("fills the notes and the payment terms from the template, in the chosen language", async () => {
    check.mockResolvedValue({ data: { family: "hardware", family_name: "Hardware", ok: true, missing: [], issues: [] } });
    template.mockResolvedValue({ data: { family: "hardware", family_name: "Hardware", preview_html: HTML,
      payment_terms: "100% upfront on purchase order." } });
    const onFill = vi.fn();
    render(<QuotationNotesHelper ctx={ctx()} onFill={onFill} delayMs={0} />);
    await userEvent.selectOptions(screen.getByLabelText("Notes language"), "en");
    await userEvent.click(screen.getByRole("button", { name: /Fill notes from template/ }));
    await waitFor(() => expect(onFill).toHaveBeenCalledWith(HTML, "100% upfront on purchase order."));
    expect(template).toHaveBeenCalledWith(expect.objectContaining({ language: "en", currency: "JPY" }));
  });

  it("asks before replacing notes already written, and keeps them on No", async () => {
    check.mockResolvedValue({ data: { family: "hardware", family_name: "Hardware", ok: true, missing: [], issues: [] } });
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const onFill = vi.fn();
    render(<QuotationNotesHelper ctx={ctx({ notes: "<p>Our own notes</p>" })} onFill={onFill} delayMs={0} />);
    await userEvent.click(screen.getByRole("button", { name: /Fill notes from template/ }));
    expect(confirm).toHaveBeenCalled();
    expect(template).not.toHaveBeenCalled();
    expect(onFill).not.toHaveBeenCalled();
    confirm.mockRestore();
  });

  it("needs lines before it can fill", async () => {
    render(<QuotationNotesHelper ctx={ctx({ items: [] })} onFill={vi.fn()} delayMs={0} />);
    await userEvent.click(screen.getByRole("button", { name: /Fill notes from template/ }));
    expect(screen.getByText(/Add the lines first/)).toBeInTheDocument();
    expect(check).not.toHaveBeenCalled();
  });
});
