import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

const getStages = vi.fn();
vi.mock("@/lib/api", () => ({ crmApi: { getStages: (...a: unknown[]) => getStages(...a) } }));

import SegmentFact from "./SegmentFact";
import { resetCrmStagesCache } from "@/hooks/useCrmStages";

const REGISTRY = {
  deal_stages: [], lead_statuses: [], lead_transitions: {}, lead_start_statuses: [], lead_sources: [],
  loss_reasons: [], disqualify_reasons: [],
};

beforeEach(() => { resetCrmStagesCache(); });

describe("segment on a record", () => {
  it("picks from the configured segments", async () => {
    getStages.mockResolvedValue({ data: { ...REGISTRY, segments: [{ key: "gov", label: "Public sector" }] } });
    const onChange = vi.fn().mockResolvedValue(undefined);
    render(<SegmentFact segment="" onChange={onChange} />);
    await screen.findByRole("option", { name: "Public sector" });
    fireEvent.change(screen.getByLabelText("Segment"), { target: { value: "gov" } });
    await waitFor(() => expect(onChange).toHaveBeenCalledWith({ segment: "gov" }));
  });

  it("is free text when no segments are configured, saved on blur", async () => {
    getStages.mockResolvedValue({ data: REGISTRY });
    const onChange = vi.fn().mockRejectedValue({ response: { data: { detail: "Could not save it" } } });
    render(<SegmentFact segment={null} onChange={onChange} />);
    const input = screen.getByLabelText("Segment") as HTMLInputElement;
    expect(input.tagName).toBe("INPUT");
    fireEvent.change(input, { target: { value: "Retail" } });
    fireEvent.blur(input);
    await waitFor(() => expect(onChange).toHaveBeenCalledWith({ segment: "Retail" }));
    expect(await screen.findByRole("alert")).toBeTruthy();
  });
});
