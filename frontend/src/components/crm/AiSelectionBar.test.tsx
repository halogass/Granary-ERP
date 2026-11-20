import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import AiSelectionBar from "./AiSelectionBar";
import { listHref } from "../ai/RecordsCard";

const bulkRunAiField = vi.fn();
const listAttributes = vi.fn();

vi.mock("@/lib/api", () => ({
  crmRecordsApi: {
    bulkRunAiField: (...a: unknown[]) => bulkRunAiField(...a),
    listAttributes: (...a: unknown[]) => listAttributes(...a),
  },
}));

beforeEach(() => {
  bulkRunAiField.mockReset().mockResolvedValue({ data: { queued: true, message: "Running over 2 record(s)." } });
  listAttributes.mockReset().mockResolvedValue({
    data: { items: [{ key: "icp_tier", label: "ICP Tier", is_ai: true, ai_kind: "classify" },
                    { key: "owner", label: "Account Owner", is_ai: false }] },
  });
});

function renderBar(selected = ["a", "b"], objectType: "organization" | "contact" = "organization") {
  const onClear = vi.fn();
  const onMessage = vi.fn();
  render(<AiSelectionBar objectType={objectType} selected={selected} onClear={onClear} onMessage={onMessage} />);
  return { onClear, onMessage };
}

describe("AiSelectionBar", () => {
  it("stays out of the way until rows are selected", async () => {
    renderBar([]);
    await waitFor(() => expect(listAttributes).toHaveBeenCalled());
    expect(screen.queryByText(/Fill with AI/)).toBeNull();
  });

  it("offers web research for companies alongside their AI attributes only", async () => {
    renderBar();
    await userEvent.click(await screen.findByText(/Fill with AI/));
    expect(screen.getByText("Enrich from the web")).toBeTruthy();
    expect(screen.getByText("ICP Tier")).toBeTruthy();
    expect(screen.queryByText("Account Owner")).toBeNull();
  });

  it("does not offer web research on anything but companies", async () => {
    renderBar(["a"], "contact");
    await userEvent.click(await screen.findByText(/Fill with AI/));
    expect(screen.queryByText("Enrich from the web")).toBeNull();
  });

  it("sends the selection and reports that the run was queued", async () => {
    const { onClear, onMessage } = renderBar();
    await userEvent.click(await screen.findByText(/Fill with AI/));
    await userEvent.click(screen.getByText("ICP Tier"));

    await waitFor(() => expect(bulkRunAiField).toHaveBeenCalledWith({
      object_type: "organization", record_ids: ["a", "b"], action: "attribute", attribute_key: "icp_tier",
    }));
    expect(onMessage).toHaveBeenCalledWith("info", "Running over 2 record(s).");
    expect(onClear).toHaveBeenCalled();
  });

  it("keeps the selection when the run could not be started", async () => {
    bulkRunAiField.mockRejectedValue(new Error("nope"));
    const { onClear, onMessage } = renderBar();
    await userEvent.click(await screen.findByText(/Fill with AI/));
    await userEvent.click(screen.getByText("Enrich from the web"));

    await waitFor(() => expect(onMessage).toHaveBeenCalledWith("error", expect.any(String)));
    expect(onClear).not.toHaveBeenCalled();
  });
});

describe("listHref", () => {
  it("round-trips the filters the assistant used", () => {
    const filters = [{ field: "city", op: "eq", value: "Osaka" }];
    const href = listHref({ object_type: "organization", filters });
    const encoded = decodeURIComponent(href.split("q=")[1]);
    expect(JSON.parse(atob(encoded))).toEqual({ object_type: "organization", filters });
  });

  it("survives non-ASCII values", () => {
    const filters = [{ field: "city", op: "eq", value: "Osaka, Kansai" }];
    const href = listHref({ object_type: "contact", filters });
    const encoded = decodeURIComponent(href.split("q=")[1]);
    const json = decodeURIComponent(escape(atob(encoded)));
    expect(JSON.parse(json).filters[0].value).toBe("Osaka, Kansai");
  });
});
