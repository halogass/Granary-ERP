import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import HistoryProgress from "./HistoryProgress";
import type { ImportChat } from "./HistoryReview";

const row = (chat_id: string, name: string, import_status: string, imported_count = 0): ImportChat => ({
  chat_id, name, message_count: 10, last_message_at: null, saved_on_phone: false, decision: "include", import_status,
  imported_count, read_status: "waiting", classification: null, summary: null, missed: null, created: null,
  person: null, opportunity: null, error: null,
});

const ROWS = [row("1@lid", "+1 555-010-0155", "waiting"), row("2@c.us", "Northwind Warehousing", "done", 40)];

describe("History import progress", () => {
  it("finds a chat by number and imports it now", async () => {
    const onImportNow = vi.fn().mockResolvedValue(undefined);
    render(<HistoryProgress rows={ROWS} onImportNow={onImportNow} />);
    await userEvent.type(screen.getByLabelText("Search the import"), "0155");
    expect(screen.queryByText("Northwind Warehousing")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: /Import now/ }));
    await waitFor(() => expect(onImportNow).toHaveBeenCalledWith("1@lid"));
  });

  it("offers Import now only for chats still to import", () => {
    render(<HistoryProgress rows={ROWS} onImportNow={vi.fn()} />);
    expect(screen.getAllByRole("button", { name: /Import now/ })).toHaveLength(1);
    expect(screen.getByText(/imported · waiting/)).toBeTruthy();
  });
});
