import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import ProposalEditor, { type Block, type Section } from "./ProposalEditor";
import CostSheet, { type Sheet } from "./CostSheet";
import ConfirmPanel from "./ConfirmPanel";
import RateCard from "./RateCard";

const sections: Section[] = [
  { key: "executive_summary", title: "Executive Summary" },
  { key: "requirements", title: "Requirement Coverage" },
  { key: "commercial", title: { en: "Commercial Summary" } },
];
const blocks: Block[] = [
  { section: "executive_summary", block_type: "text", data: { md: "**Laptops** for 15 staff" } },
  { section: "requirements", block_type: "coverage", data: { rows: [{ requirement: "Charger", status: "included", notes: "" }] } },
  { section: "commercial", block_type: "commercial", data: {} },
];

describe("Solution Proposal screens", () => {
  it("edits blocks by section and never lets the commercial summary be typed", async () => {
    const onChange = vi.fn();
    render(<ProposalEditor sections={sections} blocks={blocks} assets={[]} language="en" editable loadAsset={async () => new Blob()} onChange={onChange} />);
    expect(screen.getByText("1. Executive Summary")).toBeInTheDocument();
    expect(screen.getByText(/generated from the linked quotation/)).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText("Status 1"), "optional_custom");
    expect(onChange.mock.calls.at(-1)![0][1].data.rows[0].status).toBe("optional_custom");
    await userEvent.click(screen.getByRole("button", { name: "Add Table to Requirement Coverage" }));
    const added = onChange.mock.calls.at(-1)![0];
    expect(added.map((b: Block) => b.block_type)).toEqual(["text", "coverage", "table", "commercial"]);
  });

  it("reads an older stored {en} title and is read-only once approved", () => {
    render(<ProposalEditor sections={sections} blocks={blocks} assets={[]} language="en" editable={false} loadAsset={async () => new Blob()} onChange={vi.fn()} />);
    expect(screen.getByText("3. Commercial Summary")).toBeInTheDocument();
    expect(screen.queryByLabelText("Text (Markdown)")).toBeNull();
    expect(screen.getByText("Laptops")).toBeInTheDocument();                      // Markdown rendered
  });

  it("shows the internal cost sheet with the target and floor prices", () => {
    const pt = { labour: "5000000", non_labour: "18000000", contingency: "3450000", cost: "26450000", band: "Project Services",
                 target_pct: "45", floor_pct: "35", price_at_target: "48090909.09", price_at_floor: "40692307.69" };
    const sheet: Sheet = { labour: { workstreams: [], missing_rates: ["Astronaut (senior)"] }, non_labour: { items: [], to_confirm: ["Travel"] },
                           contingency_pct: "15", summary: { low: pt, likely: pt, high: pt }, assumptions: [], currency: "JPY" };
    render(<CostSheet sheet={sheet} />);
    expect(screen.getByText(/Never sent, never in the PDF/)).toBeInTheDocument();
    expect(screen.getByText("Price at target margin (45%)")).toBeInTheDocument();
    expect(screen.getByText(/No rate for: Astronaut/)).toBeInTheDocument();
  });

  it("confirms with the Owner's prices and warns under the floor", async () => {
    const onConfirm = vi.fn();
    render(<ConfirmPanel suggested="48000000" floor="40000000" currency="JPY" onConfirm={onConfirm} />);
    const price = screen.getByLabelText("Line 1 unit price (JPY)");
    expect(price).toHaveValue("48000000");
    await userEvent.clear(price);
    await userEvent.type(price, "35000000");
    expect(screen.getByText(/Under the floor price/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Confirm and draft the quotation/ }));
    expect(onConfirm.mock.calls[0][0][0]).toMatchObject({ name: "Implementation services", unit_price: "35000000" });
  });

  it("sets a new rate with its source", async () => {
    const onAdd = vi.fn();
    render(<RateCard canManage onAdd={onAdd} rates={[{ id: "r", role: "Backend Engineer", seniority: "mid", cost_base: "525000", source: "estimate",
      valid_from: "2026-10-01", status: "active", proposed_by_nary: false }]} />);
    expect(screen.getByText("525,000")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Role"), "QA Engineer");
    await userEvent.type(screen.getByLabelText("Cost per man-day"), "320000");
    await userEvent.type(screen.getByLabelText("Source"), "Indeed 2026");
    await userEvent.click(screen.getByRole("button", { name: "Set rate" }));
    expect(onAdd).toHaveBeenCalledWith(expect.objectContaining({ role: "QA Engineer", cost_base: "320000", source: "Indeed 2026", seniority: "mid" }));
  });
});
