import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import RecordChangeCard from "./RecordChangeCard";
import type { RecordChange } from "./types";

const applyChange = vi.fn();
const previewChange = vi.fn();

vi.mock("@/lib/api", () => ({
  crmRecordsApi: {
    applyChange: (...a: unknown[]) => applyChange(...a),
    previewChange: (...a: unknown[]) => previewChange(...a),
  },
}));

const UPDATE: RecordChange = {
  kind: "record_change",
  object_type: "deal",
  action: "update",
  record_id: "deal-1",
  label: "Face recognition — 12 sites",
  before: { stage: "proposal", probability: 50 },
  after: { stage: "contract", probability: 70 },
};

const CREATE: RecordChange = {
  kind: "record_change",
  object_type: "contact",
  action: "create",
  before: {},
  after: { first_name: "Robert", last_name: "Smith" },
};

beforeEach(() => {
  applyChange.mockReset();
  applyChange.mockResolvedValue({ data: { id: "new-1", label: "Robert Smith" } });
  previewChange.mockReset();
  previewChange.mockResolvedValue({ data: { before: UPDATE.before, after: UPDATE.after } });
});

describe("RecordChangeCard", () => {
  it("shows the before → after for every changed field", () => {
    render(<RecordChangeCard proposal={UPDATE} />);
    expect(screen.getByText("proposal")).toBeInTheDocument();
    expect(screen.getByText("contract")).toBeInTheDocument();
    expect(screen.getByText("50")).toBeInTheDocument();
    expect(screen.getByText("70")).toBeInTheDocument();
  });

  it("labels an update as Update and a create as Create", () => {
    const { unmount } = render(<RecordChangeCard proposal={UPDATE} />);
    expect(screen.getByRole("button", { name: /apply/i })).toBeInTheDocument();
    unmount();

    render(<RecordChangeCard proposal={CREATE} />);
    expect(screen.getByRole("button", { name: /^create$/i })).toBeInTheDocument();
  });

  it("writes nothing until the user confirms", async () => {
    render(<RecordChangeCard proposal={UPDATE} />);
    expect(applyChange).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: /apply/i }));
    await waitFor(() => expect(applyChange).toHaveBeenCalledTimes(1));
    expect(applyChange).toHaveBeenCalledWith({
      object_type: "deal",
      action: "update",
      record_id: "deal-1",
      fields: { stage: "contract", probability: 70 },
    });
  });

  it("discards without calling the API", async () => {
    render(<RecordChangeCard proposal={UPDATE} />);
    await userEvent.click(screen.getByRole("button", { name: /discard/i }));
    expect(applyChange).not.toHaveBeenCalled();
    expect(screen.getByText(/nothing was saved/i)).toBeInTheDocument();
  });

  it("confirms success and links to the new record", async () => {
    render(<RecordChangeCard proposal={CREATE} />);
    await userEvent.click(screen.getByRole("button", { name: /^create$/i }));
    expect(await screen.findByText(/Created Contact/i)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /open record/i }))
      .toHaveAttribute("href", "/crm/contacts/new-1");
  });

  it("surfaces a server rejection instead of claiming success", async () => {
    applyChange.mockRejectedValue({
      response: { data: { detail: "'created_by_id' is not writable on deal" } },
    });
    render(<RecordChangeCard proposal={UPDATE} />);
    await userEvent.click(screen.getByRole("button", { name: /apply/i }));
    expect(await screen.findByText(/not writable/i)).toBeInTheDocument();
    // Still on the confirm card — no false "applied" state.
    expect(screen.getByRole("button", { name: /apply/i })).toBeInTheDocument();
  });

  it("refreshes the before values of an update when it opens", async () => {
    previewChange.mockResolvedValue({ data: { before: { stage: "qualifying", probability: 40 } } });
    render(<RecordChangeCard proposal={UPDATE} />);
    expect(await screen.findByText("qualifying")).toBeInTheDocument();
    expect(screen.queryByText("proposal")).not.toBeInTheDocument();
  });

  it("reports the outcome so a reopened chat remembers it", async () => {
    const onResolved = vi.fn();
    const { unmount } = render(<RecordChangeCard proposal={CREATE} onResolved={onResolved} />);
    await userEvent.click(screen.getByRole("button", { name: /^create$/i }));
    await waitFor(() => expect(onResolved).toHaveBeenCalledWith("applied", { id: "new-1", label: "Robert Smith" }));
    unmount();

    render(<RecordChangeCard proposal={UPDATE} onResolved={onResolved} />);
    await userEvent.click(screen.getByRole("button", { name: /discard/i }));
    expect(onResolved).toHaveBeenLastCalledWith("discarded");
  });

  it("shows a saved outcome instead of a live card", () => {
    render(<RecordChangeCard proposal={{ ...UPDATE, status: "applied", result: { id: "deal-1", label: "Deal A" } }} />);
    expect(screen.queryByRole("button", { name: /apply/i })).not.toBeInTheDocument();
    expect(screen.getByText(/Updated Deal/i)).toBeInTheDocument();
    expect(previewChange).not.toHaveBeenCalled();
  });

  it("cannot be applied when there is nothing to change", () => {
    render(<RecordChangeCard proposal={{ ...UPDATE, after: {} }} />);
    expect(screen.getByRole("button", { name: /apply/i })).toBeDisabled();
  });
});
