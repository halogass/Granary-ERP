import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const topics = vi.fn();
const close = vi.fn();
vi.mock("@/lib/api", () => ({ naryApi: { ownerTopics: (...a: unknown[]) => topics(...a), closeOwnerTopic: (...a: unknown[]) => close(...a) } }));

import OwnerTopics from "./OwnerTopics";

beforeEach(() => {
  topics.mockReset().mockResolvedValue({ data: { topics: [
    { id: "t1", kind: "quotation_consult", tag: "Acme Ltd · quotation", status: "waiting", text: "3 × Monitor 27",
      urgent: false, sent_at: "2026-10-11T03:00:00Z", card_id: null, ref_type: "deal", ref_id: "d1" },
    { id: "t2", kind: "approval", tag: "Nary asks: Send a WhatsApp reply", status: "open", text: "Hello",
      urgent: true, sent_at: null, card_id: "c1", ref_type: "nary_card", ref_id: "c1" },
  ] } });
  close.mockReset().mockResolvedValue({ data: {} });
});

describe("open with you on WhatsApp", () => {
  it("lists what waits for the Owner, links cards, and can close a topic", async () => {
    render(<OwnerTopics />);
    expect(await screen.findByText("Acme Ltd · quotation")).toBeTruthy();
    expect(screen.getByText("waiting for you")).toBeTruthy();
    expect(screen.getByRole("link", { name: "open the card" }).getAttribute("href")).toBe("/nary/approvals?card=c1");
    await userEvent.click(screen.getByLabelText("Close Acme Ltd · quotation"));
    expect(close).toHaveBeenCalledWith("t1");
  });
});
