import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import ProposalCard from "./ProposalCard";

// Batch F: the reminder card used to show and submit the time in the browser's zone. A card from a
// company in Novosibirsk, opened anywhere, must show 09:00 Novosibirsk time and save that instant.
const create = vi.fn();
vi.mock("@/lib/api", () => ({
  naryApi: {}, remindersApi: { create: (...a: unknown[]) => create(...a), snooze: vi.fn(), done: vi.fn(), cancel: vi.fn() },
  researchApi: {}, salesApi: {}, financeApi: {}, kbApi: {},
}));
vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a>,
}));

const card = {
  kind: "followup_reminder" as const, title: "Call Diana back", entity_type: "deal", entity_id: "d1",
  due_at: "2026-11-21T09:00:00+07:00", time_zone: "Asia/Novosibirsk", summary: "Remind you to “Call Diana back” on Sat 21 Nov 09:00",
};

describe("follow-up reminder card", () => {
  it("shows the time in the company zone, labelled", () => {
    render(<ProposalCard proposal={card} />);
    expect((screen.getByLabelText("Due") as HTMLInputElement).value).toBe("2026-11-21T09:00");
    expect(screen.getByText(/Company time: Asia\/Novosibirsk \(GMT\+7\)/)).toBeTruthy();
  });

  it("submits the edited time as company time", async () => {
    create.mockResolvedValue({ data: { id: "r1" } });
    render(<ProposalCard proposal={card} />);
    fireEvent.change(screen.getByLabelText("Due"), { target: { value: "2026-11-21T10:30" } });
    await userEvent.click(screen.getByRole("button", { name: /Set reminder/ }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      title: "Call Diana back", due_at: "2026-11-21T03:30:00.000Z" })));
  });

  it("an old card without a zone says it is the viewer's own time", () => {
    const { time_zone: _tz, ...old } = card;
    void _tz;
    render(<ProposalCard proposal={old} />);
    expect(screen.getByText(/Your time:/)).toBeTruthy();
  });

  it("labels a meeting card's time as company time", () => {
    render(<ProposalCard proposal={{ kind: "meeting", title: "Diana call", start: "2026-11-21T11:00:00+07:00",
      duration_minutes: 45, staff_ids: [], staff: [], time_zone: "Asia/Novosibirsk",
      summary: "Book “Diana call” Sat 21 Nov 11:00, 45 min" }} />);
    expect(screen.getByText(/Company time: Asia\/Novosibirsk \(GMT\+7\)/)).toBeTruthy();
  });
});
