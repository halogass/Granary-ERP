import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import RecordTimeline, { type TimelineEntry } from "./RecordTimeline";

const getActivity = vi.fn();

vi.mock("@/lib/api", () => ({
  crmApi: { getActivity: (...a: unknown[]) => getActivity(...a) },
}));

// 23 Oct: the Hooli Consulting note: stored in full, shown cut at the 280-character preview.
const full = "## Decisions\n\n1. Scope: **two sites** first.\n" + "Detail. ".repeat(60) + "\n\n5. Reply to Franklin.";
const long: TimelineEntry = {
  kind: "activity", sub_kind: "note", id: "a1", title: "Call notes", preview: [...full].slice(0, 280).join(""),
  at: new Date().toISOString(),
};
const short: TimelineEntry = { kind: "activity", sub_kind: "note", id: "a2", title: "Short", preview: "All good." };

describe("RecordTimeline", () => {
  beforeEach(() => { getActivity.mockReset(); });

  it("offers Show more only on a preview that may be cut", () => {
    render(<RecordTimeline entries={[long, short]} />);
    expect(screen.getAllByRole("button", { name: /show more/i })).toHaveLength(1);
  });

  it("loads the whole note and shows it as Markdown, then Show less", async () => {
    getActivity.mockResolvedValue({ data: { id: "a1", content: full } });
    render(<RecordTimeline entries={[long]} />);
    await userEvent.click(screen.getByRole("button", { name: /show more/i }));
    await waitFor(() => expect(screen.getByTestId("activity-full")).toBeInTheDocument());
    expect(getActivity).toHaveBeenCalledWith("a1");
    expect(screen.getByRole("heading", { name: "Decisions" })).toBeInTheDocument();
    expect(screen.getByText("two sites")).toBeInTheDocument();
    expect(screen.getByText(/Reply to Franklin/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /show less/i }));
    expect(screen.queryByTestId("activity-full")).not.toBeInTheDocument();
    // opening it again does not fetch again
    await userEvent.click(screen.getByRole("button", { name: /show more/i }));
    expect(screen.getByTestId("activity-full")).toBeInTheDocument();
    expect(getActivity).toHaveBeenCalledTimes(1);
  });

  it("says so when the note can't be loaded", async () => {
    getActivity.mockRejectedValue({ response: { status: 403, data: { detail: "This activity belongs to someone else's record" } } });
    render(<RecordTimeline entries={[long]} />);
    await userEvent.click(screen.getByRole("button", { name: /show more/i }));
    expect(await screen.findByText(/someone else.s record/)).toBeInTheDocument();
  });
});
