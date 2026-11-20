import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// The Radix switch inside the form measures its hidden input (jsdom has no ResizeObserver).
globalThis.ResizeObserver ??= class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;

const preview = vi.fn();
const NOTE = "Prices exclude VAT and withholding tax.";
const createQuotation = vi.fn();
const updateQuotation = vi.fn();
const me = vi.fn();

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock("@/lib/api", () => ({
  salesApi: {
    listCatalog: () => Promise.resolve({ data: [
      { id: "p-box", sku: "BOX", name: "Gateway", sale_model: "one_time", price_metric: "unit", pricing_mode: "list",
        product_type: "physical", is_active: true, is_orderable: true, addon_for: [] },
      { id: "p-care", sku: "CARE", name: "Care", sale_model: "recurring", price_metric: "unit", pricing_mode: "percent_of_parent",
        percent_rate: 18, rate_period_months: 12, billing_interval_months: 12, min_term_months: 12,
        product_type: "service", is_active: true, is_orderable: true, addon_for: ["BOX"] },
    ] }),
    getProduct: () => Promise.reject(new Error("not needed")),
    pricingPreview: (...a: unknown[]) => preview(...a),
    createQuotation: (...a: unknown[]) => createQuotation(...a),
    updateQuotation: (...a: unknown[]) => updateQuotation(...a),
    taxNotes: () => Promise.resolve({ data: { note: NOTE, exclusive_phrases: ["exclude vat", "excluding vat"],
      inclusive_phrases: ["including vat", "vat included"] } }),
  },
  systemApi: { getCurrencySettings: () => Promise.resolve({ data: { active: ["USD"], functional: "USD" } }) },
  financeApi: {
    listTaxes: () => Promise.resolve({ data: [{ id: "t-vat", name: "VAT", code: "VAT", rate: 11 }] }),
    getSpotRate: () => Promise.resolve({ data: { rate: null } }),
  },
  authApi: { me: (...a: unknown[]) => me(...a) },
  crmApi: {
    listDeals: () => Promise.resolve({ data: [] }),
    getDeal: () => Promise.reject(new Error("not needed")),
  },
}));
vi.mock("@/components/ui/RichTextEditor", () => ({ default: () => null }));
vi.mock("@/components/notes/NoteTemplatePicker", () => ({ default: () => null }));
vi.mock("@/components/sales/QuotationNotesHelper", () => ({ default: () => null }));
vi.mock("@/components/crm/OrgContactPicker", () => ({ default: () => null }));
vi.mock("@/lib/entitySources", () => ({ catalogSource: { fetch: () => Promise.resolve([]) } }));
vi.mock("@/components/ui/SearchSelect", () => ({ default: () => <span>picker</span> }));

import QuotationForm, { type LineItem } from "./QuotationForm";

const line = (over: Partial<LineItem>): LineItem => ({
  key: "k", product_id: "", parent_key: null, name: "", description: "", quantity: 1, unit_price: null, manual: false,
  discount_pct: null, discount_type: "none", is_optional: false, term_months: null, tax_mode: "tax", tax_id: "t-vat",
  tax_pct: null, tax_auto: false, follow_parent_qty: false, ...over,
});

const priced = (over: Record<string, unknown>) => ({
  key: "k", product_id: null, parent_key: null, sku: null, name: "", description: null, unit: "Unit", sale_model: "one_time",
  price_metric: "unit", rate_period_months: null, billing_interval_months: null, term_months: null, periods: 1, quantity: 1,
  list_unit_price: null, unit_price: 0, discount_pct: 0, discount_type: "none", price_rule: "list", is_list: true, amount: 0,
  tax_id: "t-vat", tax_code: "VAT", tax_pct: 11, tax_amount: 0, terms_note: null, notes: [], is_optional: false, ...over,
});

const LINES = [
  line({ key: "care", product_id: "p-care", parent_key: "box", name: "Care", quantity: 25, term_months: 24 }),
  line({ key: "box", product_id: "p-box", name: "Gateway", quantity: 25 }),
];

beforeEach(() => {
  me.mockResolvedValue({ data: { permissions: ["sales:create_quotation"] } });
  preview.mockResolvedValue({ data: {
    lines: [
      priced({ key: "box", product_id: "p-box", sku: "BOX", name: "Gateway", quantity: 25, list_unit_price: 1000, unit_price: 1000,
        discount_pct: 5, discount_type: "volume", price_rule: "volume_tier", amount: 23750, tax_amount: 2612.5 }),
      priced({ key: "care", product_id: "p-care", parent_key: "box", sku: "CARE", name: "Care", sale_model: "recurring",
        rate_period_months: 12, billing_interval_months: 12, term_months: 24, periods: 2, quantity: 25,
        unit_price: 171, price_rule: "percent_of_parent", amount: 8550, tax_amount: 940.5 }),
    ],
    subtotal: 32300, discount_amount: 0, tax_lines: [{ tax_id: "t-vat", code: "VAT", name: "VAT", pct: 11, amount: 3553 }],
    tax_amount: 3553, total: 35853, all_list: true, default_tax: { id: "t-vat", code: "VAT", pct: 11 },
    payment_terms: { text: "100% on order.", schedule: [{ label: "On order", percentage: 100, due_trigger: "on_order" }] },
  } });
  updateQuotation.mockResolvedValue({ data: {} });
});

describe("QuotationForm", () => {
  // first: /auth/me is loaded once per file (usePermissions caches it), with the discount permission here
  it("keeps the discount note and notes-only display, and the preview takes nothing off then", async () => {
    me.mockResolvedValue({ data: { permissions: ["sales:create_quotation", "sales:apply_discount"] } });
    render(<QuotationForm mode="edit" quotationId="q1" initialLines={LINES}
      initialHeader={{ currency: "USD", discount_type: "percentage", discount_value: 10,
        discount_note: "Owner approved", discount_display: "pdf" }} />);
    const note = await screen.findByRole("textbox", { name: "Discount note" });
    expect(note).toHaveValue("Owner approved");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: /how the pdf shows the discount/i }), "notes_only");
    await waitFor(() => {
      const last = preview.mock.calls.at(-1)?.[0] as Record<string, unknown>;
      expect(last.discount_type).toBeUndefined();
    });
    await userEvent.click(screen.getByRole("button", { name: /save changes/i }));
    await waitFor(() => expect(updateQuotation).toHaveBeenCalled());
    const payload = updateQuotation.mock.calls.at(-1)![1];
    expect(payload).toMatchObject({ discount_note: "Owner approved", discount_display: "notes_only" });
    updateQuotation.mockClear();
    preview.mockClear();
  });

  it("prices every line on the server, keyed, with add-ons under their parent", async () => {
    render(<QuotationForm mode="edit" quotationId="q1" initialHeader={{ currency: "USD" }} initialLines={LINES} />);

    expect(await screen.findByText("Volume −5%")).toBeInTheDocument();
    const body = preview.mock.calls.at(-1)![0];
    expect(body.currency).toBe("USD");
    const sent = Object.fromEntries(body.lines.map((l: { key: string }) => [l.key, l]));
    expect(sent.box).toMatchObject({ product_id: "p-box", quantity: 25, unit_price: null, tax_id: "t-vat" });
    expect(sent.care).toMatchObject({ product_id: "p-care", parent_key: "box", term_months: 24 });

    // Server amounts and the tax summary are shown as returned.
    expect(screen.getByText(/23,750/)).toBeInTheDocument();
    expect(screen.getAllByText("VAT (11%)").some((el) => el.tagName === "SPAN")).toBe(true);
    // The add-on row comes after its parent even though it was listed first.
    const rows = screen.getAllByRole("row").map((r) => r.textContent ?? "");
    expect(rows.findIndex((t) => t.includes("BOX"))).toBeLessThan(rows.findIndex((t) => t.includes("CARE")));
    // Without sales:price_manual the unit price is shown, not editable.
    expect(screen.queryByRole("spinbutton", { name: /unit price/i })).toBeNull();
  });

  it("offers the suggested payment terms and saves keys, parents and no list prices", async () => {
    render(<QuotationForm mode="edit" quotationId="q1" initialHeader={{ currency: "USD", payment_terms: "NET30" }} initialLines={LINES} />);
    await userEvent.click(await screen.findByRole("button", { name: "Use" }));
    await waitFor(() => expect(screen.getByDisplayValue("100% on order.")).toBeInTheDocument());

    await userEvent.click(screen.getByRole("button", { name: /save changes/i }));
    await waitFor(() => expect(updateQuotation).toHaveBeenCalled());
    const payload = updateQuotation.mock.calls[0][1];
    expect(payload.payment_terms).toEqual({ text: "100% on order.", schedule: [{ label: "On order", percentage: 100, due_trigger: "on_order" }] });
    expect(payload.items.map((i: { key: string }) => i.key)).toEqual(["box", "care"]);
    expect(payload.items[1]).toMatchObject({ parent_key: "box", term_months: 24, sort_order: 2 });
    expect(payload.items[0]).not.toHaveProperty("unit_price");
  });

  // 2 Nov 2026: a quotation with VAT on every line while the notes say prices exclude VAT
  it("warns when the notes say prices exclude tax but lines carry it, and Prices exclude tax clears every line", async () => {
    updateQuotation.mockClear();
    render(<QuotationForm mode="edit" quotationId="q1" initialLines={LINES}
      initialHeader={{ currency: "USD", notes: `<p>${NOTE}</p>` }} />);
    expect(await screen.findByText(/notes say prices exclude tax, but 2 of 2 lines carry tax/i)).toBeInTheDocument();
    const toggle = screen.getByRole("switch", { name: "Prices exclude tax" });
    // the notes carry the sentence: the switch shows on; off and on again clears every line
    expect(toggle).toHaveAttribute("aria-checked", "true");
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "false");
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
    await waitFor(() => expect(screen.queryByText(/notes say prices exclude tax/i)).toBeNull());
    // the preview prices them with no tax: an explicit 0%, not the customer's default
    await waitFor(() => {
      const body = preview.mock.calls.at(-1)![0];
      expect(body.lines.every((l: { tax_pct: number | null; tax_id: string | null }) => l.tax_pct === 0 && l.tax_id === null)).toBe(true);
    });
    await userEvent.click(screen.getByRole("button", { name: /save changes/i }));
    await waitFor(() => expect(updateQuotation).toHaveBeenCalled());
    const payload = updateQuotation.mock.calls.at(-1)![1];
    for (const it of payload.items) {
      expect(it.tax_pct).toBe(0);
      expect(it.tax_id).toBeUndefined();
    }
    expect(payload.notes).toContain(NOTE);
    expect(payload.notes.split(NOTE).length).toBe(2);
  });

  it("adds the sentence to the notes when switched on and takes it out when switched off", async () => {
    updateQuotation.mockClear();
    render(<QuotationForm mode="edit" quotationId="q1" initialLines={LINES}
      initialHeader={{ currency: "USD", notes: "<p>Delivery in 4 weeks.</p>" }} />);
    const toggle = await screen.findByRole("switch", { name: "Prices exclude tax" });
    await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "false"));
    await userEvent.click(toggle);
    await userEvent.click(screen.getByRole("button", { name: /save changes/i }));
    await waitFor(() => expect(updateQuotation).toHaveBeenCalledTimes(1));
    expect(updateQuotation.mock.calls[0][1].notes).toBe(`<p>Delivery in 4 weeks.</p><p>${NOTE}</p>`);
    await userEvent.click(toggle);
    await userEvent.click(screen.getByRole("button", { name: /save changes/i }));
    await waitFor(() => expect(updateQuotation).toHaveBeenCalledTimes(2));
    expect(updateQuotation.mock.calls[1][1].notes).toBe("<p>Delivery in 4 weeks.</p>");
  });

  it("clears one line's tax with No tax and saves it as an explicit 0%", async () => {
    updateQuotation.mockClear();
    render(<QuotationForm mode="edit" quotationId="q1" initialLines={LINES} initialHeader={{ currency: "USD" }} />);
    const taxes = await screen.findAllByRole("combobox", { name: "Tax" });
    await userEvent.selectOptions(taxes[0], "");
    await userEvent.click(screen.getByRole("button", { name: /save changes/i }));
    await waitFor(() => expect(updateQuotation).toHaveBeenCalled());
    const items = updateQuotation.mock.calls.at(-1)![1].items;
    expect(items.filter((i: { tax_pct?: number }) => i.tax_pct === 0)).toHaveLength(1);
    expect(items.filter((i: { tax_id?: string }) => i.tax_id === "t-vat")).toHaveLength(1);
  });

  it("can't be saved while it waits for approval, and says so", async () => {
    updateQuotation.mockClear();
    render(<QuotationForm mode="edit" quotationId="q1" initialLines={LINES} initialHeader={{ currency: "USD" }}
      lockedReason="Q-2026-11-00001-1 is waiting for approval: withdraw it first, then save your changes." />);
    expect(await screen.findByText(/waiting for approval: withdraw it first/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /save changes/i })).toBeDisabled();
  });

  it("tags an optional line, keeps it out of the total and shows the optional items under it", async () => {
    const lines = [
      line({ key: "laptops", name: "Laptop set", unit_price: 218300000, manual: true, tax_mode: "none", tax_id: "" }),
      line({ key: "dash", name: "Analytics dashboard", unit_price: 31200000, manual: true, is_optional: true, tax_mode: "none", tax_id: "" }),
    ];
    preview.mockResolvedValue({ data: {
      lines: [
        priced({ key: "laptops", name: "Laptop set", unit_price: 218300000, amount: 218300000, tax_id: null, tax_pct: 0, is_list: false, price_rule: "manual" }),
        priced({ key: "dash", name: "Analytics dashboard", unit_price: 31200000, amount: 31200000, tax_id: null, tax_pct: 0,
          is_list: false, price_rule: "manual", is_optional: true }),
      ],
      subtotal: 218300000, discount_amount: 0, tax_lines: [], tax_amount: 0, total: 218300000, all_list: false,
      default_tax: null, optional_subtotal: 31200000, optional_tax: 0, optional_total: 31200000, total_with_optional: 249500000,
    } });
    render(<QuotationForm mode="edit" quotationId="q1" initialHeader={{ currency: "JPY" }} initialLines={lines} />);

    const block = await screen.findByTestId("optional-totals");
    expect(block).toHaveTextContent("Optional items (not in the total)");
    expect(block).toHaveTextContent(/31[.,]200[.,]000/);
    expect(block).toHaveTextContent("Total with optional items");
    expect(block).toHaveTextContent(/249[.,]500[.,]000/);
    expect(screen.getByText("not in the total")).toBeInTheDocument();
    // the Optional tag on the line (besides the checkbox label)
    expect(screen.getAllByText("Optional").length).toBeGreaterThanOrEqual(3);
    // the Total row is the server's total without the optional line
    const totalRow = screen.getAllByText("Total").find((el) => el.className.includes("font-semibold"))!.parentElement!;
    expect(totalRow).toHaveTextContent(/218[.,]300[.,]000/);
    expect(totalRow).not.toHaveTextContent(/249[.,]500[.,]000/);
  });

  it("shows no optional block without optional lines", async () => {
    render(<QuotationForm mode="edit" quotationId="q1" initialHeader={{ currency: "USD" }} initialLines={LINES} />);
    expect(await screen.findByText("Volume −5%")).toBeInTheDocument();
    expect(screen.queryByTestId("optional-totals")).toBeNull();
    expect(screen.queryByText("not in the total")).toBeNull();
  });
});
