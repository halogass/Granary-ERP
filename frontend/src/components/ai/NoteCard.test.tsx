import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

import ProposalCard from "./ProposalCard";
import type { Proposal } from "./types";

vi.mock("@/lib/api", () => ({ naryApi: {}, remindersApi: {}, researchApi: {}, salesApi: {}, financeApi: {}, kbApi: {} }));
vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a>,
}));

describe("cards with no form of their own", () => {
  it("shows an Owner-tool record (no payload) as a note instead of crashing", () => {
    const decision = { kind: "nary_decision", auto: true, status: "applied", card_id: "c1",
      summary: "Reject: Send a WhatsApp reply." } as unknown as Proposal;
    render(<ProposalCard proposal={decision} />);
    expect(screen.getByText("Reject: Send a WhatsApp reply.")).toBeInTheDocument();
  });

  it("shows a topic answer by its summary", () => {
    const answer = { kind: "nary_topic_answer", status: "applied", topic_id: "t1",
      answer: "Alert resolved.", summary: "Answer to Nary: Alert resolved." } as unknown as Proposal;
    render(<ProposalCard proposal={answer} />);
    expect(screen.getByText("Answer to Nary: Alert resolved.")).toBeInTheDocument();
  });

  it("never sends a quotation card without its payload to the quotation form", () => {
    const broken = { kind: "quotation", status: "applied", summary: "Quotation saved" } as unknown as Proposal;
    render(<ProposalCard proposal={broken} />);
    expect(screen.getByText("Quotation saved")).toBeInTheDocument();
  });
});
