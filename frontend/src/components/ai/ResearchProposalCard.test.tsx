import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import ProposalCard from "./ProposalCard";

const start = vi.fn();
vi.mock("@/lib/api", () => ({
  researchApi: { start: (...a: unknown[]) => start(...a) },
  salesApi: {}, financeApi: {}, kbApi: {}, remindersApi: {},
}));
vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a>,
}));

const PROPOSAL = { kind: "research" as const, object_type: "organization" as const, record_id: "o1",
  label: "Contoso Steel", summary: "Deep research on Contoso Steel." };

beforeEach(() => start.mockReset());

describe("research card in the assistant", () => {
  it("starts nothing until the user clicks Start research", async () => {
    start.mockResolvedValue({ data: { run: { status: "queued" }, queued: true } });
    render(<ProposalCard proposal={PROPOSAL} />);
    expect(start).not.toHaveBeenCalled();
    await userEvent.click(screen.getByText("Start research"));
    await waitFor(() => expect(start).toHaveBeenCalledWith("organization", "o1"));
    expect(await screen.findByText(/Research started/)).toBeTruthy();
    expect(screen.getByText(/Open Contoso Steel/).closest("a")?.getAttribute("href")).toBe("/crm/organizations/o1");
  });

  it("shows why a run could not start", async () => {
    start.mockResolvedValue({ data: { run: { status: "failed", error: "No web-search backend is configured." }, queued: false } });
    render(<ProposalCard proposal={PROPOSAL} />);
    await userEvent.click(screen.getByText("Start research"));
    expect(await screen.findByText(/No web-search backend/)).toBeTruthy();
  });

  it("can be cancelled", async () => {
    render(<ProposalCard proposal={PROPOSAL} />);
    await userEvent.click(screen.getByText("Cancel"));
    expect(screen.getByText("Research not started.")).toBeTruthy();
    expect(start).not.toHaveBeenCalled();
  });
});
