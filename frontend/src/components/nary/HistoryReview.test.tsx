import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import HistoryReview, { pendingDecisions, type ImportChat } from "./HistoryReview";

const chat = (id: string, name: string, extra: Partial<ImportChat> = {}): ImportChat => ({
  chat_id: id, name, message_count: 12, last_message_at: "2026-09-01T03:00:00Z", saved_on_phone: false,
  decision: "pending", import_status: "waiting", imported_count: 0, read_status: "waiting", classification: null,
  summary: null, missed: null, created: null, person: null, opportunity: null, error: null, ...extra,
});

describe("WhatsApp history review", () => {
  it("imports everything except the chats ticked personal", async () => {
    const onSubmit = vi.fn();
    const rows = [chat("1@c.us", "Northwind Warehousing"), chat("2@c.us", "John's football club", { saved_on_phone: true }),
                  chat("3@c.us", "Already decided", { decision: "include" })];
    render(<HistoryReview rows={rows} busy={false} onSubmit={onSubmit} />);
    expect(screen.queryByText("Already decided")).not.toBeInTheDocument();
    expect(screen.getByText(/Nothing has been read yet/)).toBeInTheDocument();
    await userEvent.click(screen.getByLabelText("John's football club is personal"));
    await userEvent.click(screen.getByRole("button", { name: /1 personal, 1 to import/ }));
    expect(onSubmit).toHaveBeenCalledWith({ "1@c.us": "include", "2@c.us": "exclude" });
  });

  it("decides only the pending chats", () => {
    const rows = [chat("a", "A"), chat("b", "B", { decision: "exclude" })];
    expect(pendingDecisions(rows, new Set(["a"]))).toEqual({ a: "exclude" });
  });

  it("shows nothing once every chat is decided", () => {
    const { container } = render(<HistoryReview rows={[chat("a", "A", { decision: "include" })]} busy={false} onSubmit={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });
});
