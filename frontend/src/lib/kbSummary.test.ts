import { describe, expect, it } from "vitest";
import { displaySummary } from "./kbSummary";

const SOP = "## Purpose\nEvery delivery is checked against its purchase order.\n\n## Steps\n1. Count cartons.";

describe("displaySummary (article header)", () => {
  it("hides a summary that only repeats a body heading", () => {
    expect(displaySummary("Purpose", SOP)).toBeNull();
    expect(displaySummary(" purpose: ", SOP)).toBeNull();
    expect(displaySummary("Steps", SOP)).toBeNull();
  });

  it("shows a real summary", () => {
    expect(displaySummary("How deliveries are checked and put away.", SOP)).toBe("How deliveries are checked and put away.");
    expect(displaySummary("Purpose", "No headings here, just Purpose in text.")).toBe("Purpose");
  });

  it("returns null for an empty summary", () => {
    expect(displaySummary("  ", SOP)).toBeNull();
    expect(displaySummary(undefined, SOP)).toBeNull();
  });
});
