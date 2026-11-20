import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import ProposalCard from "./ProposalCard";
import type { Proposal } from "./types";

const runAction = vi.fn();
const asset = vi.fn();
vi.mock("@/lib/api", () => ({
  naryApi: { runAction: (...a: unknown[]) => runAction(...a) }, salesDocumentsApi: { asset: (...a: unknown[]) => asset(...a) },
  remindersApi: {}, researchApi: {}, salesApi: {}, financeApi: {}, kbApi: {},
}));
vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a>,
}));

const spec = { kind: "gantt", title: "Plan", tasks: [{ name: "Install", start: 0, length: 2 }], time_unit: "week",
  source: { type: "analysis", note: "from the delivery section" } };

beforeEach(() => { runAction.mockReset(); asset.mockReset(); });

describe("proposal figure and edit cards", () => {
  it("shows the figure, caption, place and warnings, and confirms the action", async () => {
    runAction.mockResolvedValue({ data: { id: "b9", label: "Figure added" } });
    const card = { kind: "nary_action", action: "proposal.visual", status: "pending", summary: "Add the project timeline",
      payload: { document_id: "d1", op: "add", section: "Implementation Plan", spec, caption: "Figure: delivery plan" },
      preview_png: "iVBORw0KGgo=", warnings: ["Task names cut to fit"], document_label: "PRP-2026-0012" } as unknown as Proposal;
    render(<ProposalCard proposal={card} />);
    expect(screen.getByText("Add a figure: Timeline (Gantt)")).toBeInTheDocument();
    const img = screen.getByRole("img", { name: "Figure: delivery plan" });
    expect(img.getAttribute("src")).toBe("data:image/png;base64,iVBORw0KGgo=");
    expect(screen.getByText("Source: from the delivery section")).toBeInTheDocument();
    expect(screen.getByText(/where: Implementation Plan/)).toBeInTheDocument();
    expect(screen.getByText("Task names cut to fit")).toBeInTheDocument();
    expect(screen.queryByText(/Quotation/i)).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: /Confirm/ }));
    expect(runAction).toHaveBeenCalledWith({ action: "proposal.visual", payload: (card as unknown as { payload: unknown }).payload, message: null });
    expect(await screen.findByText("Figure added")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Open PRP-2026-0012/ })).toHaveAttribute("href", "/sales/proposals/d1");
  });

  it("loads a stored preview PNG when the card carries its asset id", async () => {
    asset.mockResolvedValue({ data: new Blob(["png"], { type: "image/png" }) });
    globalThis.URL.createObjectURL ??= () => "blob:x";
    globalThis.URL.revokeObjectURL ??= () => undefined;
    const card = { kind: "nary_action", action: "proposal.visual", status: "pending", summary: "Change the architecture",
      payload: { document_id: "d1", op: "change", block_id: "b2", instruction: "add the database" }, preview_png_asset_id: "a7", caption: "System" } as unknown as Proposal;
    render(<ProposalCard proposal={card} />);
    expect(screen.getByText("Change a figure")).toBeInTheDocument();
    expect(await screen.findByRole("img", { name: "System" })).toBeInTheDocument();
    expect(asset).toHaveBeenCalledWith("d1", "a7");
  });

  it("shows an Ask Nary to change card with its section diff", () => {
    const card = { kind: "nary_action", action: "proposal.edit", status: "pending", summary: "Change the proposal",
      payload: { document_id: "d1", instruction: "make the plan 8 weeks" },
      diff: ["Implementation Plan: 6 weeks -> 8 weeks", "Adds a pilot phase"] } as unknown as Proposal;
    render(<ProposalCard proposal={card} />);
    expect(screen.getByLabelText("Change the proposal")).toBeInTheDocument();
    expect(screen.getByText("make the plan 8 weeks")).toBeInTheDocument();
    expect(screen.getByText("Adds a pilot phase")).toBeInTheDocument();
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("never falls into the quotation card, even as its own kind or without a payload", () => {
    const own = { kind: "proposal_visual", status: "pending", summary: "Remove the old chart",
      payload: { document_id: "d1", op: "remove", block_id: "b3" }, caption: "Old chart" } as unknown as Proposal;
    render(<ProposalCard proposal={own} />);
    expect(screen.getByText("Remove a figure")).toBeInTheDocument();
    expect(screen.getByText("Figure: Old chart")).toBeInTheDocument();
    expect(screen.queryByText(/Quotation/i)).toBeNull();
  });

  it("keeps an applied card applied when reopened", () => {
    const done = { kind: "nary_action", action: "proposal.visual", status: "applied", summary: "Added the timeline", payload: { document_id: "d1" } } as unknown as Proposal;
    render(<ProposalCard proposal={done} />);
    expect(screen.getByText("Added the timeline")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Confirm/ })).toBeNull();
  });
});

describe("picture cards", () => {
  it("shows a picture card with the staged picture and where it came from", async () => {
    asset.mockResolvedValue({ data: new Blob(["jpg"], { type: "image/jpeg" }) });
    globalThis.URL.createObjectURL ??= () => "blob:x";
    globalThis.URL.revokeObjectURL ??= () => undefined;
    const card = { kind: "nary_action", action: "proposal.visual", status: "pending", summary: "Add the picture \"Gate today\" to SP-1",
      payload: { document_id: "d1", op: "add_image", section: "solution", asset_id: "a9", preview_png_asset_id: "a9",
        caption: "Gate today", size: "half", source_label: "gate.jpg (a file sent in the chat)" },
      location: "Solution, at the end" } as unknown as Proposal;
    render(<ProposalCard proposal={card} />);
    expect(screen.getByText("Add a picture")).toBeInTheDocument();
    expect(await screen.findByRole("img", { name: "Gate today" })).toBeInTheDocument();
    expect(asset).toHaveBeenCalledWith("d1", "a9");
    expect(screen.getByText("From: gate.jpg (a file sent in the chat)")).toBeInTheDocument();
  });
});
